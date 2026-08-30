use std::{
    collections::HashMap,
    io::{self, Write},
    path::Path,
    sync::Arc,
    time::Duration,
};

use serde_json;

use eyre::{Result, eyre};
use log::{debug, error, info};
use tokio::{
    io::BufReader,
    process::Command,
    sync::{Mutex, Semaphore, mpsc},
    task::JoinSet,
    time::timeout,
};

use crate::ports::FileSystem;

use super::state::SkipKind;
use super::task::{Task, TaskEdge};
use super::{
    action::{ActionProcessor, ProcessedAction},
    colors::{colorize_task_name, colorize_task_prefix, set_global_task_order},
    output::{OutputType, TaskMessage, TaskStreams, TuiTaskStatus},
    workspace::{ExecutionContext, Workspace},
};
use crate::cfg::edge::When;

/// Timeout for output processing after task completion
const OUTPUT_PROCESSING_TIMEOUT_SECS: u64 = 5;

/// Capacity of the task-completion channel. Each started task sends exactly one
/// report, and the drain loop consumes them as fast as they arrive, so this only
/// has to absorb a burst of simultaneous finishes.
const COMPLETION_CHANNEL_CAPACITY: usize = 32;

/// Single line written into a `tty: true` task's stdout/stderr logs.
///
/// History records both log paths when the task starts, so an empty file would
/// claim the task was silent when its output actually went straight to the
/// terminal. The marker keeps the record honest.
pub const TTY_LOG_MARKER: &str = "otto: tty task, output not captured";

/// One task's terminal report, sent exactly once per started task.
///
/// The task name is a field, not something to be recovered from the error
/// text. The channel used to carry `Result<String>` and the failure arm dug the
/// name out with `error_str.split_whitespace().nth(1)`; for a spawn failure
/// ("No such file or directory (os error 2)") that yielded "such", so the real
/// task was never removed from the active set and the run hung until killed.
/// `exit_code` is carried for the same reason: the database used to re-parse it
/// out of the message and fall back to 1, so `exit 7` was recorded as 1.
#[derive(Debug)]
pub struct TaskReport {
    /// The task this report is about.
    pub name: String,
    /// The process exit code, when a process ran and exited with one.
    pub exit_code: Option<i32>,
    /// `None` when the task succeeded.
    pub error: Option<eyre::Report>,
}

impl TaskReport {
    fn success(name: String) -> Self {
        Self {
            name,
            exit_code: Some(0),
            error: None,
        }
    }

    fn failure(name: String, error: eyre::Report, exit_code: Option<i32>) -> Self {
        Self {
            name,
            exit_code,
            error: Some(error),
        }
    }
}

/// Failure of a task body, carrying the process exit code when there was one.
///
/// Every `?` inside the body used to abandon the task without sending anything;
/// this type is what lets the whole body be one fallible expression whose single
/// exit path reports.
struct TaskFailure {
    error: eyre::Report,
    exit_code: Option<i32>,
}

impl From<eyre::Report> for TaskFailure {
    fn from(error: eyre::Report) -> Self {
        Self { error, exit_code: None }
    }
}

impl From<std::io::Error> for TaskFailure {
    fn from(error: std::io::Error) -> Self {
        Self {
            error: error.into(),
            exit_code: None,
        }
    }
}

impl From<tokio::sync::AcquireError> for TaskFailure {
    fn from(error: tokio::sync::AcquireError) -> Self {
        Self {
            error: error.into(),
            exit_code: None,
        }
    }
}

/// A one-way cancellation signal for a run.
///
/// The flag makes a cancellation that arrives before anyone waits impossible to
/// miss; the `Notify` wakes the drain loop that is parked on the report channel.
/// Checking the flag alone would leave the loop asleep until the next task
/// reported, which for a long-running child is exactly the wait being cancelled.
#[derive(Debug, Default)]
pub struct CancelSignal {
    cancelled: std::sync::atomic::AtomicBool,
    notify: tokio::sync::Notify,
}

impl CancelSignal {
    /// Ask the run to stop. Idempotent.
    pub fn cancel(&self) {
        self.cancelled.store(true, std::sync::atomic::Ordering::SeqCst);
        self.notify.notify_waiters();
    }

    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(std::sync::atomic::Ordering::SeqCst)
    }

    /// Resolves once cancelled, and never resolves otherwise, so it can sit in a
    /// `select!` arm.
    async fn cancelled(&self) {
        loop {
            // Register before re-checking, or a cancel landing between the check
            // and the await is lost.
            let notified = self.notify.notified();
            if self.is_cancelled() {
                return;
            }
            notified.await;
            if self.is_cancelled() {
                return;
            }
        }
    }
}

/// The tasks currently running, with their join handles.
///
/// Handles are reaped rather than dropped on the floor: a body that ends
/// without reporting (a panic) would otherwise leave the scheduler waiting
/// forever on a message that can never arrive.
#[derive(Default)]
struct ActiveTasks {
    /// Spawned bodies that have not been joined yet, by join id.
    names: HashMap<tokio::task::Id, String>,
    /// Names the scheduler still counts as in flight.
    running: std::collections::HashSet<String>,
    joins: JoinSet<()>,
}

impl ActiveTasks {
    fn len(&self) -> usize {
        self.running.len()
    }

    fn is_empty(&self) -> bool {
        self.running.is_empty()
    }

    fn spawn<Fut>(&mut self, name: String, body: Fut)
    where
        Fut: std::future::Future<Output = ()> + Send + 'static,
    {
        let handle = self.joins.spawn(body);
        self.names.insert(handle.id(), name.clone());
        self.running.insert(name);
    }

    /// Mark a task as no longer in flight, having received its report.
    fn reported(&mut self, name: &str) {
        self.running.remove(name);
        self.names.retain(|_, n| n != name);
    }

    /// Resolves only when a body ends *without* reporting, yielding its name.
    ///
    /// Bodies that ended normally are joined and discarded; when none are left
    /// this never resolves, so it can sit in a `select!` arm alongside the
    /// report channel without starving it.
    async fn reap_unreported(&mut self) -> String {
        loop {
            match self.joins.join_next_with_id().await {
                None => std::future::pending::<()>().await,
                Some(Ok((id, ()))) => {
                    self.names.remove(&id);
                }
                Some(Err(e)) => {
                    let id = e.id();
                    match self.names.remove(&id) {
                        Some(name) => {
                            error!("Task {name} ended without reporting: {e}");
                            self.running.remove(&name);
                            return name;
                        }
                        None => error!("A task body ended without reporting and could not be identified: {e}"),
                    }
                }
            }
        }
    }

    /// Abort every in-flight body. Each child process was spawned with
    /// `kill_on_drop`, so dropping the body's future is what actually kills it.
    fn abort_all(&mut self) {
        self.joins.abort_all();
        self.running.clear();
        self.names.clear();
    }

    /// Join every remaining body, logging any that panicked.
    async fn drain(&mut self) {
        while let Some(joined) = self.joins.join_next_with_id().await {
            match joined {
                Ok((id, ())) => {
                    self.names.remove(&id);
                }
                Err(e) => {
                    let name = self.names.remove(&e.id());
                    error!(
                        "Task {} ended without reporting: {e}",
                        name.as_deref().unwrap_or("<unknown>")
                    );
                }
            }
        }
        self.running.clear();
    }
}

/// Permits a task must hold to run: one for an ordinary task, the semaphore's
/// entire initial count for a `tty: true` task, which is what makes it exclusive.
///
/// The tty count must be the count the semaphore was *built* with, never the
/// count free at acquisition time, or "exclusive" would mean "whatever happened
/// to be idle". A zero count would make `acquire_many(0)` exclude nothing, so it
/// is asserted rather than assumed - `-j 0` already hangs earlier, in the launch
/// loop, before any task reaches here.
fn permits_for(tty: bool, max_parallel: usize) -> Result<u32> {
    debug_assert!(
        max_parallel >= 1,
        "scheduler built with max_parallel = 0; a tty task could not be made exclusive"
    );
    let wanted = if tty { max_parallel } else { 1 };
    debug_assert!(
        !tty || wanted == max_parallel,
        "a tty task must request the semaphore's initial permit count ({max_parallel}), asked for {wanted}"
    );
    u32::try_from(wanted).map_err(|_| eyre!("max_parallel {max_parallel} exceeds the semaphore's permit limit"))
}

/// Write the tty marker into a task's stdout/stderr logs.
///
/// A tty task skips `TaskStreams` entirely, so nothing else creates these files;
/// without this the paths recorded in history would dangle or read as empty.
async fn write_tty_log_markers(tasks_dir: &Path, task_name: &str) -> Result<()> {
    let task_dir = tasks_dir.join(task_name);
    tokio::fs::create_dir_all(&task_dir).await?;
    for file in ["stdout.log", "stderr.log"] {
        tokio::fs::write(task_dir.join(file), format!("{TTY_LOG_MARKER}\n")).await?;
    }
    Ok(())
}

/// Convert JSON object to shell-sourceable .env format
/// Handles proper escaping for bash safety
fn json_to_env(json: &serde_json::Value, task_name: &str) -> String {
    let mut lines = Vec::new();
    lines.push(format!("# Auto-generated by Otto from {task_name} output"));
    lines.push("# Source this file to load variables: source input.<task>.env".to_string());
    lines.push(String::new());

    if let Some(obj) = json.as_object() {
        for (key, value) in obj {
            // Create variable name: OTTO_INPUT_<TASK>_<KEY> (uppercase, safe chars only)
            let safe_task = task_name.to_uppercase().replace(['-', '.'], "_");
            let safe_key = key.to_uppercase().replace(['-', '.'], "_");
            let var_name = format!("OTTO_INPUT_{safe_task}_{safe_key}");

            // Convert value to string and escape for bash
            let str_value = match value {
                serde_json::Value::String(s) => s.clone(),
                serde_json::Value::Null => String::new(),
                other => other.to_string(),
            };

            // Escape single quotes for bash single-quoted string
            let escaped = str_value.replace('\'', "'\\''");
            lines.push(format!("{var_name}='{escaped}'"));
        }
    }

    lines.join("\n") + "\n"
}

/// Convert shell .env format back to JSON.
///
/// The inverse of `env_to_json`'s producer (`otto_serialize_output`, which emits
/// `KEY='value'` and escapes an embedded quote as `'\''`). A value is quoted, so it
/// may contain newlines: the record boundary is the closing quote, not the end of a
/// physical line. Splitting on lines instead truncated every multiline value at its
/// first newline and handed the consumer `'line1` - a leading quote and one line of
/// what the task actually wrote - while the run exited 0.
fn env_to_json(env_content: &str) -> serde_json::Value {
    /// Bash's literal-quote escape inside a single-quoted string: close, escaped
    /// quote, reopen.
    const QUOTE_ESCAPE: &[u8] = br"'\''";

    let mut map = serde_json::Map::new();
    let bytes = env_content.as_bytes();
    let mut pos = 0usize;

    while pos < bytes.len() {
        let line_end = env_content[pos..].find('\n').map_or(bytes.len(), |n| pos + n);
        let line = &env_content[pos..line_end];
        let trimmed = line.trim();

        // Comments and blank lines exist only between records, never inside a value.
        if trimmed.is_empty() || trimmed.starts_with('#') {
            pos = line_end + 1;
            continue;
        }

        let Some(eq) = line.find('=') else {
            pos = line_end + 1;
            continue;
        };
        let key = line[..eq].trim().to_string();
        let mut cursor = pos + eq + 1;

        if bytes.get(cursor) == Some(&b'\'') {
            cursor += 1;
            let mut value = String::new();
            let mut segment_start = cursor;
            loop {
                if cursor >= bytes.len() {
                    // Unterminated quote: keep what the task wrote rather than
                    // dropping it, and stop.
                    value.push_str(&env_content[segment_start..]);
                    break;
                }
                if bytes[cursor] != b'\'' {
                    cursor += 1;
                    continue;
                }
                value.push_str(&env_content[segment_start..cursor]);
                if bytes[cursor..].starts_with(QUOTE_ESCAPE) {
                    value.push('\'');
                    cursor += QUOTE_ESCAPE.len();
                    segment_start = cursor;
                    continue;
                }
                // The closing quote.
                cursor += 1;
                break;
            }
            map.insert(key, serde_json::Value::String(value));
            pos = cursor;
        } else {
            // Unquoted: the value is the remainder of this physical line.
            map.insert(key, serde_json::Value::String(line[eq + 1..].to_string()));
            pos = line_end + 1;
        }
    }

    serde_json::Value::Object(map)
}

/// The word a status line ends with, per outcome.
fn task_outcome_word(status: &TaskStatus) -> &'static str {
    match status {
        TaskStatus::Completed => "finished successfully",
        TaskStatus::Failed(_) => "failed",
        TaskStatus::Skipped(_) => "skipped",
        _ => "finished",
    }
}

/// Status of a task during execution
#[derive(Debug, Clone, PartialEq)]
pub enum TaskStatus {
    /// Task is waiting for dependencies
    Pending,
    /// Task is currently running
    Running,
    /// Task completed successfully
    Completed,
    /// Task was skipped, carrying why: an up-to-date output set, a serial-group
    /// predecessor that did not succeed, or an unreachable conditional edge.
    /// Downstream `when:` classification branches on the kind, so it is part of
    /// the status rather than a parallel lookup the two gates could disagree on.
    Skipped(SkipKind),
    /// Task failed during execution
    Failed(String),
}

/// Per-edge satisfaction state computed from the runtime state of its source task.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum EdgeState {
    /// The edge is satisfied; the source reached the expected terminal state.
    Satisfied,
    /// The edge can never be satisfied; the source reached a terminal state
    /// that contradicts the edge's `when:` condition.
    Unreachable,
    /// The source has not reached a terminal state yet.
    Pending,
}

/// Names of tasks that reached a terminal Skipped state, keyed by why.
///
/// Only the skipped set is keyed: `completed_set` and `failed_set` need no
/// reason distinguished, because "it ran and exited zero" and "it ran and
/// exited non-zero" are each one thing.
type SkippedSet = HashMap<String, SkipKind>;

/// A skip as both halves of one fact: the kind the scheduler branches on and the
/// sentence the operator reads. They are produced together so they cannot drift,
/// and both are persisted to the run record.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkipRecord {
    pub kind: SkipKind,
    pub detail: String,
}

impl SkipRecord {
    fn new(kind: SkipKind, detail: String) -> Self {
        Self { kind, detail }
    }
}

/// Evaluate a single dependency edge against the runtime sets.
///
/// A Skipped source is resolved against the edge's `when:` using its provenance,
/// not short-circuited to Unreachable. The full contract, nine cells, pinned by
/// `classify_edge_skip_provenance_matrix`:
///
/// | source `SkipKind`   | `when: success` | `when: failure` | `when: always` |
/// |---------------------|-----------------|-----------------|----------------|
/// | `UpToDate`          | Satisfied       | Unreachable     | Satisfied      |
/// | `SerialPredecessor` | Unreachable     | Unreachable     | Satisfied      |
/// | `Unreachable`       | Unreachable     | Unreachable     | Satisfied      |
///
/// `UpToDate` is success-like: the task's outputs are current, which is the
/// outcome a `when: success` dependent is waiting for. `always` is satisfied by
/// every terminal state, which is what makes cleanup reliable. The whole
/// `failure` column is Unreachable because `when: failure` means the source task
/// ran and its action exited non-zero; widening it would fan failure handlers
/// across the entire downstream cone of any single failure.
fn classify_edge(
    edge: &TaskEdge,
    completed: &std::collections::HashSet<String>,
    failed: &std::collections::HashSet<String>,
    skipped: &SkippedSet,
) -> EdgeState {
    if let Some(kind) = skipped.get(&edge.task) {
        return match edge.when {
            When::Success if kind.is_success_like() => EdgeState::Satisfied,
            When::Success => EdgeState::Unreachable,
            When::Failure => EdgeState::Unreachable,
            When::Always => EdgeState::Satisfied,
        };
    }
    match edge.when {
        When::Success => {
            if completed.contains(&edge.task) {
                EdgeState::Satisfied
            } else if failed.contains(&edge.task) {
                EdgeState::Unreachable
            } else {
                EdgeState::Pending
            }
        }
        When::Failure => {
            if failed.contains(&edge.task) {
                EdgeState::Satisfied
            } else if completed.contains(&edge.task) {
                EdgeState::Unreachable
            } else {
                EdgeState::Pending
            }
        }
        When::Always => {
            if completed.contains(&edge.task) || failed.contains(&edge.task) {
                EdgeState::Satisfied
            } else {
                EdgeState::Pending
            }
        }
    }
}

/// Serial foreach ordering, indexed for the ready loop.
///
/// Serial ordering is a property of the tasks, not a dependency edge: it constrains the
/// order in which members of a group may start, and never expands the run set. The only
/// consumer is the readiness check below.
#[derive(Debug, Default)]
struct SerialGroups {
    /// Group name -> member names sorted by declared foreach order. Only members that
    /// are actually in the run set appear here, which is what makes `otto up:gamma`
    /// schedule gamma alone.
    members: HashMap<String, Vec<String>>,
}

impl SerialGroups {
    fn new(tasks: &[Task]) -> Self {
        let mut ordered: HashMap<String, Vec<(usize, String)>> = HashMap::new();
        for task in tasks {
            if let Some(group) = &task.serial_group {
                ordered
                    .entry(group.clone())
                    .or_default()
                    .push((task.serial_index, task.name.clone()));
            }
        }
        let members = ordered
            .into_iter()
            .map(|(group, mut entries)| {
                entries.sort();
                (group, entries.into_iter().map(|(_, name)| name).collect())
            })
            .collect();
        Self { members }
    }

    /// The nearest preceding member of this task's group that is in the run set.
    fn predecessor(&self, task: &Task) -> Option<&str> {
        let group = task.serial_group.as_ref()?;
        let members = self.members.get(group)?;
        let position = members.iter().position(|name| name == &task.name)?;
        members.get(position.checked_sub(1)?).map(String::as_str)
    }

    /// Classify the serial ordering gate for a task, using the same terminal sets the
    /// dependency edges are classified against.
    ///
    /// - no predecessor in the run set -> Satisfied (ordering never expands the run set)
    /// - predecessor Completed, or skipped as `UpToDate` -> Satisfied
    /// - predecessor Failed, or skipped for any other reason -> Unreachable, so this
    ///   member is skipped as `SerialPredecessor` and the cascade propagates through
    ///   the same rule
    /// - otherwise the predecessor has not finished -> Pending
    ///
    /// The success-like treatment of `UpToDate` is the same rule `classify_edge`
    /// applies; this gate reads the kind rather than deciding independently.
    fn classify(
        &self,
        task: &Task,
        completed: &std::collections::HashSet<String>,
        failed: &std::collections::HashSet<String>,
        skipped: &SkippedSet,
    ) -> EdgeState {
        match self.predecessor(task) {
            None => EdgeState::Satisfied,
            Some(pred) => {
                if let Some(kind) = skipped.get(pred) {
                    if kind.is_success_like() {
                        EdgeState::Satisfied
                    } else {
                        EdgeState::Unreachable
                    }
                } else if completed.contains(pred) {
                    EdgeState::Satisfied
                } else if failed.contains(pred) {
                    EdgeState::Unreachable
                } else {
                    EdgeState::Pending
                }
            }
        }
    }
}

/// Classify every gate on a task: its dependency edges plus the serial ordering gate.
/// The serial gate composes with dependency readiness, it never replaces it.
fn classify_gates(
    task: &Task,
    serial_groups: &SerialGroups,
    completed: &std::collections::HashSet<String>,
    failed: &std::collections::HashSet<String>,
    skipped: &SkippedSet,
) -> Vec<EdgeState> {
    let mut states: Vec<EdgeState> = task
        .task_deps
        .iter()
        .map(|e| classify_edge(e, completed, failed, skipped))
        .collect();
    states.push(serial_groups.classify(task, completed, failed, skipped));
    states
}

/// Build the skip record for a task whose gates say it can never run: the typed
/// kind plus the sentence naming which edge or ordering gate made it unreachable.
///
/// The kind is decided by which gate fired, so the text is a rendering of the kind
/// rather than an independent guess at the same question.
fn skip_record_for(
    task: &Task,
    serial_groups: &SerialGroups,
    completed: &std::collections::HashSet<String>,
    failed: &std::collections::HashSet<String>,
    skipped: &SkippedSet,
) -> SkipRecord {
    for edge in &task.task_deps {
        if matches!(classify_edge(edge, completed, failed, skipped), EdgeState::Unreachable) {
            let detail = if skipped.contains_key(&edge.task) {
                format!("dep {} skipped; cascade", edge.task)
            } else {
                match edge.when {
                    When::Success => format!("dep {} failed; this task required when: success", edge.task),
                    When::Failure => format!("dep {} succeeded; this task required when: failure", edge.task),
                    When::Always => format!("dep {} unreachable", edge.task),
                }
            };
            return SkipRecord::new(SkipKind::Unreachable, detail);
        }
    }
    if matches!(
        serial_groups.classify(task, completed, failed, skipped),
        EdgeState::Unreachable
    ) && let Some(pred) = serial_groups.predecessor(task)
    {
        let detail = if failed.contains(pred) {
            format!("serial predecessor {pred} failed")
        } else {
            format!("serial predecessor {pred} skipped; cascade")
        };
        return SkipRecord::new(SkipKind::SerialPredecessor, detail);
    }
    SkipRecord::new(SkipKind::Unreachable, "unreachable dependency".to_string())
}

/// Task scheduler that manages concurrent execution
pub struct TaskScheduler<F: FileSystem = crate::ports::RealFs> {
    /// Task status tracking
    task_statuses: Arc<Mutex<HashMap<String, TaskStatus>>>,
    /// Task start times tracking (for duration calculation)
    task_start_times: Arc<Mutex<HashMap<String, std::time::Instant>>>,
    /// Why each skipped task was skipped, for tasks skipped by an unreachable
    /// dependency edge or a serial-group cascade. Up-to-date skips are not recorded
    /// here: they are successes, not gated-out tasks.
    skip_records: Arc<Mutex<HashMap<String, SkipRecord>>>,
    /// Semaphore for task limiting
    semaphore: Arc<Semaphore>,
    /// The permit count the semaphore was built with. A `tty: true` task acquires
    /// all of them at once, and the live `available_permits()` is the wrong number
    /// to ask for at acquisition time (it is whatever is free right then).
    max_parallel: usize,
    /// Workspace for path management
    workspace: Arc<Workspace<F>>,
    /// Execution context for metadata
    execution_context: ExecutionContext,
    /// Tasks to execute
    tasks: Vec<Task>,
    /// Whether TUI mode is enabled (suppresses terminal output)
    tui_mode: bool,
    /// Whether to omit the `[task]` prefix from terminal output (`--no-prefix`;
    /// see docs/design/2026-08-28-boundary-fixes-and-dynamic-foreach.md Phase 8).
    /// Defaults to false; set via `set_no_prefix()` so the many existing
    /// `TaskScheduler::new()` call sites (tests included) don't have to
    /// thread a flag that almost none of them exercise.
    no_prefix: bool,
    /// Optional broadcast channel for TUI status updates
    message_tx: Option<tokio::sync::broadcast::Sender<TaskMessage>>,
    /// Pre-created TaskStreams for TUI mode (task_name -> TaskStreams)
    task_streams: Option<Arc<std::collections::HashMap<String, TaskStreams>>>,
    /// Trip this to stop the run. The drain loop honors it; the TUI quit path is
    /// what trips it, so closing the dashboard no longer strands the user waiting
    /// on children with no way to cancel.
    cancel: Arc<CancelSignal>,
}

impl<F: FileSystem + 'static> TaskScheduler<F> {
    pub async fn new(
        tasks: Vec<Task>,
        workspace: Arc<Workspace<F>>,
        execution_context: ExecutionContext,
        max_parallel: usize,
        tui_mode: bool,
    ) -> Result<Self> {
        // Topological validation before anything is scheduled: a cycle makes the
        // run set unsatisfiable, and the ready loop's only response to that used
        // to be marking everything Skipped and returning Ok.
        crate::executor::graph::DagVisualizer::validate_acyclic(&tasks)?;

        let task_statuses = Arc::new(Mutex::new(HashMap::new()));
        let task_start_times = Arc::new(Mutex::new(HashMap::new()));

        // Set up global task ordering for consistent color assignment
        let task_names: Vec<String> = tasks.iter().map(|t| t.name.clone()).collect();
        set_global_task_order(task_names);

        Ok(Self {
            task_statuses,
            task_start_times,
            skip_records: Arc::new(Mutex::new(HashMap::new())),
            semaphore: Arc::new(Semaphore::new(max_parallel)),
            max_parallel,
            workspace,
            execution_context,
            tasks,
            tui_mode,
            no_prefix: false,
            message_tx: None,
            task_streams: None,
            cancel: Arc::new(CancelSignal::default()),
        })
    }

    /// A handle the caller can trip to cancel this run.
    pub fn cancel_signal(&self) -> Arc<CancelSignal> {
        self.cancel.clone()
    }

    pub fn set_message_channel(&mut self, tx: tokio::sync::broadcast::Sender<TaskMessage>) {
        self.message_tx = Some(tx);
    }

    /// Enable `--no-prefix`: terminal output for every task streams raw, with
    /// no `[task]` prefix. No effect on file logs (never prefixed) or on TUI
    /// mode (terminal output is already fully suppressed there).
    pub fn set_no_prefix(&mut self, no_prefix: bool) {
        self.no_prefix = no_prefix;
    }

    /// What a scheduler status line leads with: `[task]`, or a bare `task`
    /// under `--no-prefix`.
    ///
    /// The flag used to reach task output only, so a `--no-prefix` run mixed
    /// unprefixed output with prefixed status lines.
    fn status_label(&self, task_name: &str) -> String {
        if self.no_prefix {
            colorize_task_name(task_name)
        } else {
            colorize_task_prefix(task_name)
        }
    }

    /// Set pre-created TaskStreams for TUI mode
    pub fn set_task_streams(&mut self, streams: std::collections::HashMap<String, TaskStreams>) {
        self.task_streams = Some(Arc::new(streams));
    }

    /// Helper to broadcast a TaskMessage to TUI
    fn broadcast_message(&self, message: TaskMessage) {
        if let Some(tx) = &self.message_tx {
            let _ = tx.send(message);
        }
    }

    /// Convert internal TaskStatus to TUI TaskStatus
    fn to_tui_status(status: &TaskStatus) -> TuiTaskStatus {
        match status {
            TaskStatus::Pending => TuiTaskStatus::Pending,
            TaskStatus::Running => TuiTaskStatus::Running,
            TaskStatus::Completed => TuiTaskStatus::Completed,
            TaskStatus::Skipped(_) => TuiTaskStatus::Skipped,
            TaskStatus::Failed(_) => TuiTaskStatus::Failed,
        }
    }

    /// Record a task as Skipped with its provenance and a user-visible reason.
    ///
    /// Every skip that is not an up-to-date skip goes through here: unreachable
    /// dependency edges and serial-group cascades alike. The record lands in
    /// `skip_records`, the name and kind land in `skipped_set` so downstream gates
    /// classify against the kind, and the terminal prints the detail so a skipped
    /// task is never silent.
    async fn mark_skipped(&self, task: &Task, record: SkipRecord, skipped_set: &mut SkippedSet) {
        let SkipRecord { kind, detail } = &record;
        info!("Skipping task {} ({detail})", task.name);
        if !self.tui_mode {
            let msg = format!("{} skipped ({detail})\n", self.status_label(&task.name));
            print!("{msg}");
            io::stdout().flush().unwrap_or(());
        }
        skipped_set.insert(task.name.clone(), *kind);
        {
            let mut statuses = self.task_statuses.lock().await;
            statuses.insert(task.name.clone(), TaskStatus::Skipped(*kind));
        }
        self.skip_records.lock().await.insert(task.name.clone(), record);
        self.broadcast_message(TaskMessage::Finished {
            task_name: task.name.clone(),
            status: TuiTaskStatus::Skipped,
            timestamp: std::time::SystemTime::now(),
            duration_ms: 0,
        });
    }

    /// Try to start a ready task, handling skipping and errors
    #[allow(clippy::too_many_arguments)]
    async fn try_start_ready_task(
        &self,
        task: Task,
        tx: mpsc::Sender<TaskReport>,
        active_tasks: &mut ActiveTasks,
        completed_set: &mut std::collections::HashSet<String>,
        blocked_tasks: &mut Vec<Task>,
        ready_queue: &mut std::collections::VecDeque<Task>,
        completed_tasks: &mut usize,
        total_tasks: usize,
        serial_groups: &SerialGroups,
        failed_set: &std::collections::HashSet<String>,
        skipped_set: &mut SkippedSet,
    ) -> Result<()> {
        match self.needs_rebuild(&task).await {
            Ok(true) => {
                // Task needs to run
                info!("Starting task {} ({}/{})", task.name, *completed_tasks + 1, total_tasks);

                // Broadcast task started to TUI
                self.broadcast_message(TaskMessage::Started {
                    task_name: task.name.clone(),
                    timestamp: std::time::SystemTime::now(),
                });

                self.execute_task(task.clone(), tx.clone(), active_tasks).await?;
            }
            Ok(false) => {
                // Task can be skipped - outputs are up to date
                info!(
                    "Skipping task {} - outputs are up to date ({}/{})",
                    task.name,
                    *completed_tasks + 1,
                    total_tasks
                );

                // Print user-visible skipped message (only in terminal mode)
                if !self.tui_mode {
                    let skipped_msg = format!("{} skipped (up to date)\n", self.status_label(&task.name));
                    print!("{skipped_msg}");
                    io::stdout().flush().unwrap_or(());
                }

                // Broadcast task skipped to TUI
                self.broadcast_message(TaskMessage::StatusChange {
                    task_name: task.name.clone(),
                    status: TuiTaskStatus::Skipped,
                    timestamp: std::time::SystemTime::now(),
                });

                {
                    let mut statuses = self.task_statuses.lock().await;
                    statuses.insert(task.name.clone(), TaskStatus::Skipped(SkipKind::UpToDate));
                }
                // An up-to-date skip is success-like, so it lands in `completed_set`
                // like any other success. It is also recorded in `skipped_set` with
                // its kind, because it is still terminal-Skipped and the gates must
                // be able to tell it apart from a gated-out skip.
                completed_set.insert(task.name.clone());
                skipped_set.insert(task.name.clone(), SkipKind::UpToDate);
                *completed_tasks += 1;

                blocked_tasks.retain(|blocked_task| {
                    let task_deps_completed = blocked_task
                        .task_deps
                        .iter()
                        .all(|task_dep| completed_set.contains(&task_dep.task));
                    if !task_deps_completed {
                        return true; // Keep the task in blocked list
                    }

                    // The serial gate composes with dependency readiness: a member whose
                    // predecessor has not finished stays blocked.
                    if !matches!(
                        serial_groups.classify(blocked_task, completed_set, failed_set, skipped_set),
                        EdgeState::Satisfied
                    ) {
                        return true;
                    }

                    // All dependencies are completed, move to ready queue
                    ready_queue.push_back(blocked_task.clone());
                    false // Remove from blocked list
                });
            }
            Err(e) => {
                error!("Error checking file dependencies for task {}: {}", task.name, e);
                // On error, default to running the task
                info!(
                    "Starting task {} (file check failed, defaulting to run) ({}/{})",
                    task.name,
                    *completed_tasks + 1,
                    total_tasks
                );

                // Broadcast task started to TUI
                self.broadcast_message(TaskMessage::Started {
                    task_name: task.name.clone(),
                    timestamp: std::time::SystemTime::now(),
                });

                self.execute_task(task.clone(), tx.clone(), active_tasks).await?;
            }
        }
        Ok(())
    }

    /// Stop the run: kill the in-flight children and report the cancellation.
    ///
    /// The children are killed rather than waited on, which is the whole point of a
    /// cancel: `kill_on_drop(true)` on every spawned command means aborting the task
    /// bodies takes the processes with them.
    async fn abandon_run(&self, active_tasks: &mut ActiveTasks) -> Result<()> {
        let abandoned = active_tasks.len();
        info!("Run cancelled; killing {abandoned} in-flight task(s)");
        active_tasks.abort_all();
        if !self.tui_mode {
            eprintln!("otto: run cancelled; {abandoned} running task(s) killed");
        }
        self.persist_skip_records().await;
        Err(eyre!("run cancelled; {abandoned} running task(s) were killed"))
    }

    pub async fn execute_all(&self) -> Result<()> {
        let (tx, mut rx) = mpsc::channel(COMPLETION_CHANNEL_CAPACITY);

        // Initialize task statuses and start times
        {
            let mut statuses = self.task_statuses.lock().await;
            let mut start_times = self.task_start_times.lock().await;
            let now = std::time::Instant::now();
            for task in &self.tasks {
                statuses.insert(task.name.clone(), TaskStatus::Pending);
                // Record start time for all tasks at the beginning
                // Tasks are conceptually "in progress" while waiting for deps
                start_times.insert(task.name.clone(), now);
            }
        }

        let mut ready_queue = std::collections::VecDeque::new();
        let mut blocked_tasks = Vec::new();

        // Serial foreach ordering, resolved against the run set (not the whole config).
        let serial_groups = SerialGroups::new(&self.tasks);

        for task in &self.tasks {
            // Tasks with no dependencies AND no serial predecessor can be queued
            // immediately. A member waiting on a predecessor starts blocked so the
            // ready loop never has to spin on a gate that cannot yet be satisfied.
            if task.task_deps.is_empty() && serial_groups.predecessor(task).is_none() {
                ready_queue.push_back(task.clone());
            } else {
                blocked_tasks.push(task.clone());
            }
        }

        let mut completed_tasks = 0;
        let total_tasks = self.tasks.len();
        let mut active_tasks = ActiveTasks::default();
        let max_concurrent = self.max_parallel;

        // Track completed and failed tasks for dependency satisfaction checking.
        let mut completed_set = std::collections::HashSet::new();
        let mut failed_set: std::collections::HashSet<String> = std::collections::HashSet::new();
        let mut skipped_set: SkippedSet = SkippedSet::new();

        // Record the first failure to surface as the run's error; conditional follow-ups
        // (when: failure / when: always) still get a chance to run.
        let mut final_error: Option<eyre::Report> = None;

        while completed_tasks < total_tasks {
            if self.cancel.is_cancelled() {
                return self.abandon_run(&mut active_tasks).await;
            }

            // Start as many tasks as we can
            while active_tasks.len() < max_concurrent && !ready_queue.is_empty() {
                let task = ready_queue.pop_front().unwrap();

                let states = classify_gates(&task, &serial_groups, &completed_set, &failed_set, &skipped_set);

                if states.iter().any(|s| matches!(s, EdgeState::Unreachable)) {
                    // Dep state contradicts this task's edge condition, or its serial
                    // predecessor reached a non-success terminal state -> Skip.
                    let record = skip_record_for(&task, &serial_groups, &completed_set, &failed_set, &skipped_set);
                    self.mark_skipped(&task, record, &mut skipped_set).await;
                    completed_tasks += 1;
                    continue;
                }

                if !states.iter().all(|s| matches!(s, EdgeState::Satisfied)) {
                    // Some gate is still Pending, so this task is blocked, not ready.
                    // It goes to `blocked_tasks` and is swept back out when a gate
                    // resolves. Re-queuing it instead made this loop's termination
                    // depend on `ready_queue.len() == 1` - a guard that holds only
                    // while the queue contains exactly this one task, and that any
                    // future enqueue path would silently turn into a busy spin.
                    blocked_tasks.push(task);
                    continue;
                }

                // Try to start the task (handles rebuild check, skipping, and errors)
                self.try_start_ready_task(
                    task,
                    tx.clone(),
                    &mut active_tasks,
                    &mut completed_set,
                    &mut blocked_tasks,
                    &mut ready_queue,
                    &mut completed_tasks,
                    total_tasks,
                    &serial_groups,
                    &failed_set,
                    &mut skipped_set,
                )
                .await?;
            }

            // Only wait for task completion if there are active tasks.
            // If nothing is active and the ready queue is empty, sweep for newly-resolvable
            // blocked tasks (typically downstream of a Skipped task).
            if active_tasks.is_empty() {
                let mut newly_ready = Vec::new();
                let mut newly_skipped: Vec<Task> = Vec::new();
                blocked_tasks.retain(|task| {
                    let states = classify_gates(task, &serial_groups, &completed_set, &failed_set, &skipped_set);
                    if states.iter().any(|s| matches!(s, EdgeState::Unreachable)) {
                        newly_skipped.push(task.clone());
                        return false;
                    }
                    if states.iter().all(|s| matches!(s, EdgeState::Satisfied)) {
                        newly_ready.push(task.clone());
                        return false;
                    }
                    true
                });
                let progressed = !newly_ready.is_empty() || !newly_skipped.is_empty();
                for t in newly_ready {
                    ready_queue.push_back(t);
                }
                for t in newly_skipped {
                    let record = skip_record_for(&t, &serial_groups, &completed_set, &failed_set, &skipped_set);
                    self.mark_skipped(&t, record, &mut skipped_set).await;
                    completed_tasks += 1;
                }
                if !progressed && ready_queue.is_empty() {
                    // Nothing more we can do; bail out of the loop.
                    break;
                }
                continue;
            }

            // Wait for any task to report. A body that ends without reporting
            // (a panic) is reaped instead, and synthesized into a failure so the
            // run terminates rather than waiting on a message that never comes.
            let received = tokio::select! {
                message = rx.recv() => message,
                name = active_tasks.reap_unreported() => Some(TaskReport::failure(
                    name.clone(),
                    eyre!("Task {name} panicked"),
                    None,
                )),
                () = self.cancel.cancelled() => {
                    return self.abandon_run(&mut active_tasks).await;
                }
            };

            match received {
                Some(TaskReport {
                    name: completed_task,
                    error: None,
                    ..
                }) => {
                    info!("Task {completed_task} completed successfully");

                    // Calculate task duration
                    let duration_ms = {
                        let mut start_times = self.task_start_times.lock().await;
                        start_times
                            .remove(&completed_task)
                            .map(|start| start.elapsed().as_millis() as u64)
                            .unwrap_or(0)
                    };

                    // Aggregation override for virtual parent tasks: the parent's
                    // nominal Completed status is overridden based on its subtasks.
                    // This MUST run before completed_set.insert and before the
                    // blocked_tasks sweep - otherwise downstream when: success
                    // dependents would be queued for a parent that should be Failed.
                    let parent_task = self.tasks.iter().find(|t| t.name == completed_task);
                    let final_status = if let Some(t) = parent_task
                        && t.is_virtual_parent
                    {
                        let statuses_guard = self.task_statuses.lock().await;
                        let subtask_statuses: Vec<(String, TaskStatus)> = self
                            .tasks
                            .iter()
                            .filter(|st| st.parent.as_deref() == Some(t.name.as_str()))
                            .filter_map(|st| statuses_guard.get(&st.name).cloned().map(|s| (st.name.clone(), s)))
                            .collect();
                        drop(statuses_guard);
                        // Skip provenance decides this, not the bare Skipped status:
                        // a foreach whose subtasks were all up to date is a warm cache,
                        // i.e. a success, and must not skip the parent (and with it
                        // everything downstream). Only a gated-out subtask skips the
                        // parent, and it propagates that subtask's kind.
                        let gated_out = subtask_statuses.iter().find_map(|(name, s)| match s {
                            TaskStatus::Skipped(kind) if !kind.is_success_like() => Some((name.clone(), *kind)),
                            _ => None,
                        });
                        if subtask_statuses.iter().any(|(_, s)| matches!(s, TaskStatus::Failed(_))) {
                            TaskStatus::Failed("virtual parent: subtask failed".to_string())
                        } else if let Some((subtask, kind)) = gated_out {
                            // The parent did not pass through mark_skipped, so record its
                            // reason here: a parent skipped by aggregation must be as
                            // visible in the run record as one skipped by a gate.
                            self.skip_records.lock().await.insert(
                                completed_task.clone(),
                                SkipRecord::new(kind, format!("subtask {subtask} skipped; nothing to aggregate")),
                            );
                            TaskStatus::Skipped(kind)
                        } else {
                            TaskStatus::Completed
                        }
                    } else {
                        TaskStatus::Completed
                    };

                    // Print user-visible success/failure message (only in terminal mode)
                    if !self.tui_mode {
                        // `--no-prefix` suppressed the `[task]` prefix on task
                        // output but not here, so a no-prefix run printed
                        // `OTHER` followed by `[other] finished successfully`.
                        let msg = format!(
                            "{} {}\n",
                            self.status_label(&completed_task),
                            task_outcome_word(&final_status)
                        );
                        print!("{msg}");
                        io::stdout().flush().unwrap_or(());
                    }

                    // Broadcast task completion to TUI
                    let tui_status = Self::to_tui_status(&final_status);
                    self.broadcast_message(TaskMessage::Finished {
                        task_name: completed_task.clone(),
                        status: tui_status,
                        timestamp: std::time::SystemTime::now(),
                        duration_ms,
                    });

                    // Route the (possibly overridden) status to the right tracking set.
                    match &final_status {
                        TaskStatus::Completed => {
                            completed_set.insert(completed_task.clone());
                        }
                        TaskStatus::Failed(_) => {
                            failed_set.insert(completed_task.clone());
                        }
                        TaskStatus::Skipped(kind) => {
                            // A virtual parent that aggregated to Skipped (subtasks Skipped
                            // because their prerequisites failed) must enter skipped_set
                            // carrying the kind it inherited, so downstream edges to this
                            // parent classify against the same table every other gate uses
                            // rather than sitting Pending forever.
                            skipped_set.insert(completed_task.clone(), *kind);
                        }
                        _ => {}
                    }
                    {
                        let mut statuses = self.task_statuses.lock().await;
                        statuses.insert(completed_task.clone(), final_status);
                    }
                    completed_tasks += 1;
                    active_tasks.reported(&completed_task);

                    // Sweep blocked_tasks: newly-ready and newly-unreachable.
                    let mut newly_ready = Vec::new();
                    let mut newly_skipped: Vec<Task> = Vec::new();
                    blocked_tasks.retain(|task| {
                        let states = classify_gates(task, &serial_groups, &completed_set, &failed_set, &skipped_set);
                        if states.iter().any(|s| matches!(s, EdgeState::Unreachable)) {
                            newly_skipped.push(task.clone());
                            return false;
                        }
                        if states.iter().all(|s| matches!(s, EdgeState::Satisfied)) {
                            newly_ready.push(task.clone());
                            return false;
                        }
                        true
                    });
                    for t in newly_ready {
                        ready_queue.push_back(t);
                    }
                    for t in newly_skipped {
                        let record = skip_record_for(&t, &serial_groups, &completed_set, &failed_set, &skipped_set);
                        self.mark_skipped(&t, record, &mut skipped_set).await;
                        completed_tasks += 1;
                    }
                }
                Some(TaskReport {
                    name: task_name,
                    error: Some(e),
                    ..
                }) => {
                    error!("Task {task_name} failed: {e}");

                    // Calculate task duration
                    let duration_ms = {
                        let mut start_times = self.task_start_times.lock().await;
                        start_times
                            .remove(&task_name)
                            .map(|start| start.elapsed().as_millis() as u64)
                            .unwrap_or(0)
                    };

                    // Print user-visible failure message (only in terminal mode)
                    if !self.tui_mode {
                        let failure_msg = format!("{} failed\n", self.status_label(&task_name));
                        eprint!("{failure_msg}");
                        io::stderr().flush().unwrap_or(());
                    }

                    // Broadcast task failure to TUI
                    self.broadcast_message(TaskMessage::Finished {
                        task_name: task_name.clone(),
                        status: TuiTaskStatus::Failed,
                        timestamp: std::time::SystemTime::now(),
                        duration_ms,
                    });

                    // Record failure in tracking sets and statuses.
                    {
                        let mut statuses = self.task_statuses.lock().await;
                        statuses.insert(task_name.clone(), TaskStatus::Failed(e.to_string()));
                    }
                    failed_set.insert(task_name.clone());
                    active_tasks.reported(&task_name);
                    completed_tasks += 1;

                    // Sweep blocked_tasks: newly-ready and newly-unreachable.
                    let mut newly_ready = Vec::new();
                    let mut newly_skipped: Vec<Task> = Vec::new();
                    blocked_tasks.retain(|task| {
                        let states = classify_gates(task, &serial_groups, &completed_set, &failed_set, &skipped_set);
                        if states.iter().any(|s| matches!(s, EdgeState::Unreachable)) {
                            newly_skipped.push(task.clone());
                            return false;
                        }
                        if states.iter().all(|s| matches!(s, EdgeState::Satisfied)) {
                            newly_ready.push(task.clone());
                            return false;
                        }
                        true
                    });
                    for t in newly_ready {
                        ready_queue.push_back(t);
                    }
                    for t in newly_skipped {
                        let record = skip_record_for(&t, &serial_groups, &completed_set, &failed_set, &skipped_set);
                        self.mark_skipped(&t, record, &mut skipped_set).await;
                        completed_tasks += 1;
                    }

                    if final_error.is_none() {
                        final_error = Some(e);
                    }
                    // Fall through to next loop iteration - drain in-flight tasks.
                }
                None => {
                    error!("Task completion channel closed unexpectedly");
                    return Err(eyre!("Task completion channel closed unexpectedly"));
                }
            }
        }

        // Post-loop reconciliation: any task still blocked is marked Skipped (its
        // edges never resolved one way or another, likely because an upstream dep
        // failed and is unreachable for this task's when: condition).
        for task in blocked_tasks.drain(..) {
            let record = skip_record_for(&task, &serial_groups, &completed_set, &failed_set, &skipped_set);
            self.mark_skipped(&task, record, &mut skipped_set).await;
        }

        // Join every spawned body so a panic is logged rather than silently
        // aborted when the JoinSet drops.
        active_tasks.drain().await;

        // Persist skip provenance to the run record. Built and dropped before
        // this: the accessor had no caller in the whole tree.
        self.persist_skip_records().await;

        if let Some(err) = final_error {
            return Err(err);
        }

        // Nothing ran. Every task in the run set was declared unreachable, which
        // means the run accomplished nothing and must not report success. An
        // up-to-date skip is not this case: those land in `completed_set`.
        if completed_set.is_empty() && failed_set.is_empty() && !skipped_set.is_empty() {
            return Err(eyre!(
                "no task reached a terminal state: all {} task(s) in this run were skipped as unreachable",
                skipped_set.len()
            ));
        }
        Ok(())
    }

    /// Write each skipped task and why it was skipped into the run record, so
    /// `otto History` can say why a task did not run.
    ///
    /// Both halves of the record are persisted: `skip_kind` for anything that
    /// wants to filter by reason class, `skip_reason` for the operator.
    async fn persist_skip_records(&self) {
        let records = self.get_skip_records().await;
        if records.is_empty() {
            return;
        }
        let (Some(run_id), Some(store)) = (self.workspace.db_run_id(), self.workspace.state_store()) else {
            return;
        };
        for (task_name, record) in records {
            let name = task_name.clone();
            let detail = record.detail.clone();
            let kind = record.kind;
            let recorded = crate::ports::record_blocking(store, move |store| {
                store.record_task_skipped(run_id, &name, None, Some(&detail), Some(kind))
            })
            .await;
            if let Err(e) = recorded {
                log::warn!("Failed to record skipped task {task_name} in database: {e}");
            }
        }
    }

    /// Spawn `task`'s body into `active`, reporting exactly once when it ends.
    ///
    /// The body is one fallible expression: every early exit inside it - the
    /// semaphore, the dependency double-check, `create_dir_all`, the symlinks,
    /// the action processor - lands in the same place, and that place is the
    /// only sender. Before this, a `?` on any of those abandoned the task with
    /// nothing sent, and the scheduler waited on it forever.
    async fn execute_task(&self, task: Task, tx: mpsc::Sender<TaskReport>, active: &mut ActiveTasks) -> Result<()> {
        let semaphore = self.semaphore.clone();

        let task_name = task.name.clone();
        let task_dir = self.workspace.task(&task_name);
        let task_statuses = self.task_statuses.clone();
        let task_deps = task.task_deps.clone();
        let workspace = self.workspace.clone();
        let envs = task.envs.clone();
        let tasks_dir = self.workspace.run().join("tasks");
        let execution_context = self.execution_context.clone();
        let suppress_terminal = self.tui_mode;
        let no_prefix = self.no_prefix;
        let task_streams = self.task_streams.clone();
        let is_virtual_parent = task.is_virtual_parent;
        let action_is_empty = task.action.is_empty();
        let tty = task.tty;
        let permits = permits_for(tty, self.max_parallel)?;

        let spawn_name = task_name.clone();
        active.spawn(spawn_name, async move {
            // Set by the run block when a process actually exited, so the
            // database records the real code instead of re-parsing it back out
            // of the error message and defaulting to 1.
            let mut exit_code: Option<i32> = None;
            // Assigned once the task has a database row; the completion write
            // happens after the body, on the single report path.
            let mut db_task_id: Option<i64> = None;
            // A virtual parent has no script, no output files and no database
            // row: it only aggregates its subtasks' statuses.
            let mut aggregator_only = false;

            let outcome: std::result::Result<(), TaskFailure> = async {
                // Acquire semaphore permit. A tty task takes every permit, so nothing
                // else runs while it owns the terminal; tokio's semaphore is FIFO, so
                // this wait cannot be starved by later single-permit acquires.
                let _permit = semaphore.acquire_many(permits).await?;

                // Empty-action fast path (used by virtual parent tasks). The parent
                // task has no script to run - it exists only to aggregate subtask
                // statuses. We mark it Running briefly so the scheduler's success
                // arm picks it up and applies the aggregation override.
                if is_virtual_parent && action_is_empty {
                    {
                        let mut statuses = task_statuses.lock().await;
                        statuses.insert(task_name.clone(), TaskStatus::Running);
                    }
                    aggregator_only = true;
                    return Ok(());
                }

                {
                    let statuses = task_statuses.lock().await;
                    for dep in &task_deps {
                        let status = statuses.get(&dep.task);
                        // Second gate on the same edge, and it must answer exactly what
                        // `classify_edge` answered: same nine cells, asserted together by
                        // `classify_edge_skip_provenance_matrix`. A disagreement here
                        // aborts at spawn time a task the scheduler just admitted.
                        let satisfied = match (dep.when, status) {
                            // when: success requires Completed, or a skip that is
                            // success-like: an up-to-date source produced current outputs.
                            (When::Success, Some(TaskStatus::Completed)) => true,
                            (When::Success, Some(TaskStatus::Skipped(kind))) => kind.is_success_like(),
                            // when: failure requires a Failed status on the source: it ran
                            // and its action exited non-zero. No skip satisfies it.
                            (When::Failure, Some(TaskStatus::Failed(_))) => true,
                            // when: always is satisfied by any terminal state, whatever the
                            // skip kind, which is what makes cleanup reliable.
                            (When::Always, Some(TaskStatus::Completed)) => true,
                            (When::Always, Some(TaskStatus::Skipped(_))) => true,
                            (When::Always, Some(TaskStatus::Failed(_))) => true,
                            _ => false,
                        };
                        if !satisfied {
                            return Err(eyre!(
                                "Dependency {} not satisfied (when: {:?}, status: {:?}) for task {}",
                                dep.task,
                                dep.when,
                                status,
                                task_name
                            )
                            .into());
                        }
                    }
                }

                // Update task status to Running ONLY after dependency check
                {
                    let mut statuses = task_statuses.lock().await;
                    statuses.insert(task_name.clone(), TaskStatus::Running);
                }

                info!("Starting task {task_name}");

                tokio::fs::create_dir_all(&task_dir).await?;

                // Setup dependency input files (symlink outputs from dependencies)
                for dep_edge in &task_deps {
                    let dep_name = &dep_edge.task;
                    let dep_output_file = workspace.task_output_file(dep_name);
                    let current_input_file = workspace.task_input_file(&task_name, dep_name);
                    let current_input_env_file = workspace.task_input_env_file(&task_name, dep_name);

                    // Only create symlink if dependency output exists
                    if dep_output_file.exists() {
                        if current_input_file.exists() {
                            tokio::fs::remove_file(&current_input_file).await.ok();
                        }

                        // Create symlink from dependency output to current task input
                        #[cfg(unix)]
                        {
                            use std::os::unix::fs;
                            // Use relative path for portability
                            let relative_dep_path = workspace.relative_task_dependency_path(dep_name);
                            fs::symlink(&relative_dep_path, &current_input_file)?;
                        }
                        #[cfg(not(unix))]
                        {
                            // Fallback: copy file on non-Unix systems
                            tokio::fs::copy(&dep_output_file, &current_input_file).await?;
                        }

                        // Generate .env file from JSON for jq-free bash deserialization
                        // This allows bash to source the .env file instead of parsing JSON with jq
                        if let Ok(json_content) = tokio::fs::read_to_string(&dep_output_file).await
                            && let Ok(json_data) = serde_json::from_str::<serde_json::Value>(&json_content)
                        {
                            let env_content = json_to_env(&json_data, dep_name);
                            tokio::fs::write(&current_input_env_file, env_content).await.ok();
                        }
                    }
                }

                // Process the user's action script with Otto enhancements
                let action_processor = ActionProcessor::new(workspace.clone(), &task_name)?;
                let processed_action = action_processor.process(&task.action, &task)?;

                // Extract script path and determine interpreter
                let (script_path, interpreter) = match processed_action {
                    ProcessedAction::Bash { path, .. } => (path, "bash"),
                    ProcessedAction::Python3 { path, .. } => (path, "python3"),
                };

                // Record task start in database with paths (graceful degradation)
                db_task_id = if let (Some(run_id), Some(store)) = (workspace.db_run_id(), workspace.state_store()) {
                    let stdout_path = tasks_dir.join(&task_name).join("stdout.log");
                    let stderr_path = tasks_dir.join(&task_name).join("stderr.log");
                    let name = task_name.clone();
                    let script = script_path.clone();

                    // rusqlite is synchronous and holds a mutex across the
                    // write, so it runs on the blocking pool rather than
                    // parking a tokio worker mid-task.
                    let recorded = crate::ports::record_blocking(store, move |store| {
                        store.record_task_start(
                            run_id,
                            &name,
                            None, // TODO: Compute script hash in future phase
                            Some(&stdout_path),
                            Some(&stderr_path),
                            Some(&script),
                        )
                    })
                    .await;

                    match recorded {
                        Ok(task_id) => Some(task_id),
                        Err(e) => {
                            log::warn!("Failed to record task start in database: {}", e);
                            None
                        }
                    }
                } else {
                    None
                };

                // Setup command environment
                let mut cmd = Command::new(interpreter);
                cmd.arg(&script_path)
                    .current_dir(workspace.root())
                    // Inherit current environment by default (no env_clear())
                    .envs(&envs) // Override with user-specified env vars
                    .env("OTTO_TASK", &task_name)
                    .env("OTTO_TASK_DIR", task_dir.to_string_lossy().to_string())
                    .env("OTTO_WORKSPACE", workspace.root().to_string_lossy().to_string())
                    .env("OTTO_TASKS_DIR", tasks_dir.to_string_lossy().to_string())
                    .env("OTTO_USER", &execution_context.user)
                    // The child dies with its task body. Without this, cancelling or
                    // dropping the scheduler left orphaned processes holding the
                    // workspace open.
                    .kill_on_drop(true);

                // Its own process group, so a signal aimed at otto does not race
                // ahead of otto's own teardown and so the child's own children are
                // reachable as a group. NOT for a `tty: true` task: that task owns
                // the terminal, and a background process group reading from it gets
                // SIGTTIN and stops.
                #[cfg(unix)]
                if !tty {
                    cmd.process_group(0);
                }

                // Execute without timeout - runs until completion or failure
                let result = async {
                    if tty {
                        // The task owns the terminal: inherit stdout/stderr (stdin is
                        // already inherited - otto never redirects it) and skip
                        // TaskStreams entirely, so there is no capture and no [task]
                        // prefix. The logs still exist, carrying the marker line.
                        write_tty_log_markers(&tasks_dir, &task_name).await?;
                        let mut child = cmd
                            .stdout(std::process::Stdio::inherit())
                            .stderr(std::process::Stdio::inherit())
                            .spawn()
                            .map_err(|e| eyre!("Task {task_name} could not start {interpreter}: {e}"))?;
                        let status = child.wait().await?;
                        exit_code = status.code();
                        if status.success() {
                            return Ok(());
                        }
                        let stdout_log = tasks_dir.join(&task_name).join("stdout.log");
                        let stderr_log = tasks_dir.join(&task_name).join("stderr.log");
                        // No stderr preview: nothing was captured, and echoing the
                        // marker line back as "error output" would be a lie.
                        return Err(eyre!(
                            "Task {} failed with exit code {:?}\n\nLogs:\n  stdout: {}\n  stderr: {}",
                            task_name,
                            status.code(),
                            stdout_log.display(),
                            stderr_log.display()
                        ));
                    }

                    let mut child = cmd
                        .stdout(std::process::Stdio::piped())
                        .stderr(std::process::Stdio::piped())
                        .spawn()
                        .map_err(|e| eyre!("Task {task_name} could not start {interpreter}: {e}"))?;

                    // Setup output streams
                    let stdout = child.stdout.take().ok_or_else(|| eyre!("Failed to capture stdout"))?;
                    let stderr = child.stderr.take().ok_or_else(|| eyre!("Failed to capture stderr"))?;

                    let streams = if let Some(streams_map) = &task_streams {
                        streams_map
                            .get(&task_name)
                            .ok_or_else(|| eyre!("TaskStreams not found for task {}", task_name))?
                            .clone()
                    } else {
                        TaskStreams::new(&task_name, &tasks_dir).await?
                    };

                    // Start output handling
                    let stdout_handle = {
                        let streams = streams.clone();
                        let task_name = task_name.clone();
                        tokio::spawn(async move {
                            let reader = BufReader::new(stdout);
                            streams
                                .process_output(task_name, OutputType::Stdout, reader, suppress_terminal, no_prefix)
                                .await
                        })
                    };

                    let stderr_handle = {
                        let streams = streams.clone();
                        let task_name = task_name.clone();
                        tokio::spawn(async move {
                            let reader = BufReader::new(stderr);
                            streams
                                .process_output(task_name, OutputType::Stderr, reader, suppress_terminal, no_prefix)
                                .await
                        })
                    };

                    // Wait for process to complete
                    let status = child.wait().await?;
                    exit_code = status.code();

                    // Wait for output handling to complete with timeout (only for output processing)
                    let output_timeout = Duration::from_secs(OUTPUT_PROCESSING_TIMEOUT_SECS);

                    match timeout(output_timeout, stdout_handle).await {
                        Ok(Ok(Ok(()))) => {
                            // Stdout processing completed successfully
                        }
                        Ok(Ok(Err(e))) => {
                            error!("Stdout processing failed for task {task_name}: {e}");
                        }
                        Ok(Err(e)) => {
                            error!("Stdout processing join failed for task {task_name}: {e}");
                        }
                        Err(_) => {
                            error!("Stdout processing timed out for task {task_name}");
                        }
                    }

                    match timeout(output_timeout, stderr_handle).await {
                        Ok(Ok(Ok(()))) => {
                            // Stderr processing completed successfully
                        }
                        Ok(Ok(Err(e))) => {
                            error!("Stderr processing failed for task {task_name}: {e}");
                        }
                        Ok(Err(e)) => {
                            error!("Stderr processing join failed for task {task_name}: {e}");
                        }
                        Err(_) => {
                            error!("Stderr processing timed out for task {task_name}");
                        }
                    }

                    if status.success() {
                        Ok(())
                    } else {
                        // Get fully qualified log file paths
                        let stdout_log = tasks_dir.join(&task_name).join("stdout.log");
                        let stderr_log = tasks_dir.join(&task_name).join("stderr.log");

                        // Read stderr content to include in error message
                        let stderr_content = tokio::fs::read_to_string(&stderr_log).await.unwrap_or_default();
                        let stderr_preview = if !stderr_content.trim().is_empty() {
                            let lines: Vec<&str> = stderr_content.lines().collect();
                            let preview_lines = if lines.len() > 20 { &lines[lines.len() - 20..] } else { &lines[..] };
                            format!(
                                "\n\nError output (last {} lines):\n{}",
                                preview_lines.len(),
                                preview_lines.join("\n")
                            )
                        } else {
                            String::new()
                        };

                        Err(eyre!(
                            "Task {} failed with exit code {:?}{}\n\nLogs:\n  stdout: {}\n  stderr: {}",
                            task_name,
                            status.code(),
                            stderr_preview,
                            stdout_log.canonicalize().unwrap_or(stdout_log).display(),
                            stderr_log.canonicalize().unwrap_or(stderr_log).display()
                        ))
                    }
                }
                .await;

                result.map_err(TaskFailure::from)
            }
            .await;

            // Exactly one report per started task, whichever way the body ended.
            let report = match outcome {
                Ok(()) if aggregator_only => TaskReport::success(task_name.clone()),
                Ok(()) => {
                    info!("Task {task_name} completed successfully");

                    // Convert .env output to JSON for downstream tasks
                    // This allows bash to write simple key=value format while maintaining JSON compatibility
                    let env_output_file = workspace.task_output_env_file(&task_name);
                    let json_output_file = workspace.task_output_file(&task_name);

                    if env_output_file.exists() {
                        // Read .env file and convert to JSON
                        if let Ok(env_content) = tokio::fs::read_to_string(&env_output_file).await
                            && let Ok(json_str) = serde_json::to_string_pretty(&env_to_json(&env_content))
                            && let Err(e) = tokio::fs::write(&json_output_file, json_str).await
                        {
                            log::warn!("Failed to write JSON output for task {task_name}: {e}");
                        }
                    } else if !json_output_file.exists() {
                        // If no output was written, create empty JSON
                        if let Err(e) = tokio::fs::write(&json_output_file, "{}").await {
                            log::warn!("Failed to write empty JSON output for task {task_name}: {e}");
                        }
                    }

                    // Record task completion in database (graceful degradation)
                    if let Some(task_id) = db_task_id
                        && let Some(store) = workspace.state_store()
                    {
                        let code = exit_code.unwrap_or(0);
                        let recorded = crate::ports::record_blocking(store, move |store| {
                            store.record_task_complete(task_id, code, super::state::TaskStatus::Completed)
                        })
                        .await;
                        if let Err(e) = recorded {
                            log::warn!("Failed to record task completion in database: {}", e);
                        }
                    }

                    TaskReport::success(task_name.clone())
                }
                Err(TaskFailure { error, exit_code: code }) => {
                    error!("Task {task_name} failed: {error}");
                    // The body's own observation wins; `code` only carries a
                    // value on paths that set it before failing.
                    let recorded = code.or(exit_code);

                    // Record task failure in database (graceful degradation)
                    if let Some(task_id) = db_task_id
                        && let Some(store) = workspace.state_store()
                    {
                        let code = recorded.unwrap_or(1);
                        // Degrading gracefully is not the same as saying
                        // nothing: this used to be `let _ =`, so a failure to
                        // record a failure vanished entirely.
                        let stored = crate::ports::record_blocking(store, move |store| {
                            store.record_task_complete(task_id, code, super::state::TaskStatus::Failed)
                        })
                        .await;
                        if let Err(e) = stored {
                            log::warn!("Failed to record task failure in database: {}", e);
                        }
                    }

                    {
                        let mut statuses = task_statuses.lock().await;
                        statuses.insert(task_name.clone(), TaskStatus::Failed(error.to_string()));
                    }
                    TaskReport::failure(task_name.clone(), error, recorded)
                }
            };

            if let Err(e) = tx.send(report).await {
                error!("Failed to report outcome for task {task_name}: {e}");
            }
        });

        Ok(())
    }

    pub async fn get_task_statuses(&self) -> HashMap<String, TaskStatus> {
        self.task_statuses.lock().await.clone()
    }

    /// Why each task skipped by an unreachable dependency edge or a serial-group
    /// cascade was skipped, keyed by task name. Up-to-date skips are absent by
    /// design: they are successes, not gated-out tasks.
    pub async fn get_skip_records(&self) -> HashMap<String, SkipRecord> {
        self.skip_records.lock().await.clone()
    }

    pub async fn get_task_status(&self, task_name: &str) -> TaskStatus {
        let statuses = self.task_statuses.lock().await;
        statuses.get(task_name).cloned().unwrap_or(TaskStatus::Pending)
    }

    pub async fn needs_rebuild(&self, task: &Task) -> Result<bool> {
        // If no file dependencies, always run (traditional task-only mode)
        if task.file_deps.is_empty() {
            debug!("Task {} has no file dependencies, will run", task.name);
            return Ok(true);
        }

        let output_files = &task.output_deps;

        // If no output files exist, need to run
        if output_files.is_empty() {
            debug!("Task {} has no output files defined, will run", task.name);
            return Ok(true);
        }

        for output_path in output_files {
            if !Path::new(output_path).exists() {
                debug!(
                    "Output file {} does not exist, task {} needs to run",
                    output_path, task.name
                );
                return Ok(true);
            }
        }

        let input_timestamps = self.get_file_timestamps(&task.file_deps).await?;
        let output_timestamps = self.get_file_timestamps(output_files).await?;

        // Find the newest input and oldest output
        let newest_input = input_timestamps.iter().filter_map(|(_, time)| *time).max();
        let oldest_output = output_timestamps.iter().filter_map(|(_, time)| *time).min();

        match (newest_input, oldest_output) {
            (Some(input_time), Some(output_time)) => {
                let needs_rebuild = input_time > output_time;
                if needs_rebuild {
                    debug!("Input files newer than outputs, task {} needs to run", task.name);
                } else {
                    debug!("Outputs up to date, task {} can be skipped", task.name);
                }
                Ok(needs_rebuild)
            }
            (None, _) => {
                debug!("No input files found, task {} will run", task.name);
                Ok(true) // No inputs found, run the task
            }
            (_, None) => {
                debug!("No output files found, task {} needs to run", task.name);
                Ok(true) // No outputs found, need to run
            }
        }
    }

    async fn get_file_timestamps(&self, file_paths: &[String]) -> Result<Vec<(String, Option<std::time::SystemTime>)>> {
        let mut timestamps = Vec::new();

        for file_path in file_paths {
            let path = Path::new(file_path);
            let timestamp = if path.exists() {
                match tokio::fs::metadata(path).await {
                    Ok(metadata) => match metadata.modified() {
                        Ok(time) => Some(time),
                        Err(e) => {
                            debug!("Could not get modification time for {file_path}: {e}");
                            None
                        }
                    },
                    Err(e) => {
                        debug!("Could not get metadata for {file_path}: {e}");
                        None
                    }
                }
            } else {
                debug!("File {file_path} does not exist");
                None
            };
            timestamps.push((file_path.clone(), timestamp));
        }

        Ok(timestamps)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serial_test::serial;
    use std::path::PathBuf;
    use tempfile::TempDir;

    /// Point this test's otto home at a scratch directory.
    ///
    /// `OTTO_DB_PATH` is deliberately cleared rather than set: the database now
    /// derives from `OTTO_HOME`, so setting the home alone must be enough to
    /// keep a test off the developer's real database. `OTTO_DB_PATH` still
    /// overrides, and that override is pinned by its own test in `state/db.rs`.
    fn setup_test_db(temp_dir: &std::path::Path) {
        let otto_home = temp_dir.join(".otto");
        // SAFETY: This is safe in tests because we control the execution environment
        // and tests are isolated. The env var is set before any StateManager is created.
        unsafe {
            std::env::remove_var("OTTO_DB_PATH");
            std::env::set_var("OTTO_HOME", &otto_home);
        }
    }

    /// A multiline value survives the round trip. It used to be truncated at the
    /// first newline and handed back with a stray leading quote (`'line1`), while the
    /// run exited 0.
    #[test]
    fn test_env_to_json_preserves_multiline_values() {
        let content = "# header\n\nMULTI='line1\nline2\nline3'\nAFTER='tail'\n";
        let json = env_to_json(content);

        assert_eq!(
            json["MULTI"],
            serde_json::Value::String("line1\nline2\nline3".to_string())
        );
        assert_eq!(
            json["AFTER"],
            serde_json::Value::String("tail".to_string()),
            "the record after a multiline value must still be parsed"
        );
        assert_eq!(json.as_object().map(|o| o.len()), Some(2));
    }

    /// `'\''` is bash's literal single quote inside a single-quoted string, not a
    /// record terminator.
    #[test]
    fn test_env_to_json_unescapes_embedded_quotes() {
        let content = "Q='it'\\''s here'\nNEXT='ok'\n";
        let json = env_to_json(content);

        assert_eq!(json["Q"], serde_json::Value::String("it's here".to_string()));
        assert_eq!(json["NEXT"], serde_json::Value::String("ok".to_string()));
    }

    /// The producer and the parser are inverses, including for the values that used
    /// to break the parser.
    #[test]
    fn test_json_to_env_round_trips_through_env_to_json() {
        for value in [
            "plain",
            "line1\nline2",
            "it's got a quote",
            "# not a comment",
            "trailing=equals=signs",
            "",
        ] {
            let json = serde_json::json!({ "VALUE": value });
            let env = json_to_env(&json, "task");
            let back = env_to_json(&env);
            assert_eq!(
                back["OTTO_INPUT_TASK_VALUE"],
                serde_json::Value::String(value.to_string()),
                "round trip lost {value:?}"
            );
        }
    }

    /// Unterminated quote: keep what the task wrote rather than dropping the record.
    #[test]
    fn test_env_to_json_keeps_an_unterminated_value() {
        let json = env_to_json("BROKEN='no closing quote\n");
        assert_eq!(
            json["BROKEN"],
            serde_json::Value::String("no closing quote\n".to_string())
        );
    }

    /// Cancelling stops the run instead of waiting for the children.
    #[tokio::test]
    #[serial]
    async fn test_cancel_signal_stops_the_run() -> Result<()> {
        let temp_dir = TempDir::new()?;
        let work_dir = PathBuf::from(temp_dir.path());
        setup_test_db(&work_dir);

        let tasks = vec![Task::new(
            "sleeper".to_string(),
            None,
            vec![],
            vec![],
            vec![],
            HashMap::new(),
            HashMap::new(),
            "sleep 60".to_string(),
        )];

        let workspace = Workspace::new(work_dir).await?;
        workspace.init().await?;
        let scheduler = TaskScheduler::new(tasks, Arc::new(workspace), ExecutionContext::new(), 2, false).await?;
        let cancel = scheduler.cancel_signal();

        tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_millis(200)).await;
            cancel.cancel();
        });

        // The task sleeps for 60s; if cancellation did not work this times out.
        let outcome = tokio::time::timeout(std::time::Duration::from_secs(10), scheduler.execute_all()).await?;
        let err = outcome.expect_err("a cancelled run must not report success");
        assert!(
            format!("{err:#}").contains("cancelled"),
            "the error must say the run was cancelled; got {err:#}"
        );
        Ok(())
    }

    /// Cancelling before the run starts stops it immediately: the flag is checked at
    /// the top of the drain loop, not only in the `select!` arm.
    #[tokio::test]
    #[serial]
    async fn test_cancel_before_start_is_not_lost() -> Result<()> {
        let temp_dir = TempDir::new()?;
        let work_dir = PathBuf::from(temp_dir.path());
        setup_test_db(&work_dir);

        let workspace = Workspace::new(work_dir).await?;
        workspace.init().await?;
        let scheduler = TaskScheduler::new(
            vec![plain_task("only", vec![])],
            Arc::new(workspace),
            ExecutionContext::new(),
            2,
            false,
        )
        .await?;
        scheduler.cancel_signal().cancel();

        let outcome = tokio::time::timeout(std::time::Duration::from_secs(10), scheduler.execute_all()).await?;
        assert!(outcome.is_err(), "a run cancelled before it started must not succeed");
        Ok(())
    }

    fn plain_task(name: &str, deps: Vec<TaskEdge>) -> Task {
        Task::new(
            name.to_string(),
            None,
            deps,
            vec![],
            vec![],
            HashMap::new(),
            HashMap::new(),
            "echo hi".to_string(),
        )
    }

    #[tokio::test]
    #[serial]
    async fn test_scheduler_rejects_a_dependency_cycle_at_init() {
        let temp_dir = TempDir::new().unwrap();
        setup_test_db(temp_dir.path());
        let workspace = Workspace::new(PathBuf::from(temp_dir.path())).await.unwrap();
        workspace.init().await.unwrap();

        let tasks = vec![
            plain_task("a", vec![TaskEdge::success("b")]),
            plain_task("b", vec![TaskEdge::success("a")]),
        ];

        let err = match TaskScheduler::new(tasks, Arc::new(workspace), ExecutionContext::new(), 2, false).await {
            Ok(_) => panic!("a 2-cycle must be rejected before anything is scheduled"),
            Err(e) => e,
        };
        assert!(err.to_string().contains("dependency cycle detected"), "{err}");
    }

    /// A run in which every task is gated out accomplished nothing, and used to
    /// report that as success.
    #[tokio::test]
    #[serial]
    async fn test_execute_all_errors_when_no_task_reached_a_terminal_state() {
        let temp_dir = TempDir::new().unwrap();
        setup_test_db(temp_dir.path());
        let workspace = Workspace::new(PathBuf::from(temp_dir.path())).await.unwrap();
        workspace.init().await.unwrap();

        // `b` depends on a task that is not in the run set, so its edge never
        // resolves and post-loop reconciliation marks it Skipped.
        let tasks = vec![plain_task("b", vec![TaskEdge::success("absent")])];
        let scheduler = TaskScheduler::new(tasks, Arc::new(workspace), ExecutionContext::new(), 2, false)
            .await
            .unwrap();

        let err = scheduler
            .execute_all()
            .await
            .expect_err("a run where nothing ran must not report success");
        assert!(err.to_string().contains("no task reached a terminal state"), "{err}");
    }

    #[test]
    fn test_task_report_carries_the_name_and_code_structurally() {
        let ok = TaskReport::success("build".to_string());
        assert_eq!(ok.name, "build");
        assert_eq!(ok.exit_code, Some(0));
        assert!(ok.error.is_none());

        let failed = TaskReport::failure("build".to_string(), eyre!("No such file or directory"), Some(7));
        assert_eq!(failed.name, "build", "the name must not come from the error text");
        assert_eq!(failed.exit_code, Some(7));
        assert!(failed.error.is_some());
    }

    fn serial_member(name: &str, group: &str, index: usize) -> Task {
        let mut task = Task::new(
            name.to_string(),
            Some(group.to_string()),
            vec![],
            vec![],
            vec![],
            HashMap::new(),
            HashMap::new(),
            "echo hi".to_string(),
        );
        task.serial_group = Some(group.to_string());
        task.serial_index = index;
        task
    }

    /// Ordering constrains the run set, it never expands it: the predecessor of a member
    /// is the nearest preceding member THAT IS IN THE RUN SET, so a targeted subtask has
    /// no predecessor at all.
    #[test]
    fn test_serial_predecessor_is_nearest_member_in_the_run_set() {
        let tasks = vec![serial_member("up:alpha", "up", 0), serial_member("up:gamma", "up", 2)];
        let groups = SerialGroups::new(&tasks);

        assert_eq!(groups.predecessor(&tasks[0]), None);
        assert_eq!(groups.predecessor(&tasks[1]), Some("up:alpha"));

        let lone = vec![serial_member("up:gamma", "up", 2)];
        assert_eq!(SerialGroups::new(&lone).predecessor(&lone[0]), None);
    }

    /// Every terminal state of a predecessor is covered, so the gate can never leave a
    /// successor waiting forever.
    #[test]
    fn test_serial_gate_classifies_every_predecessor_terminal_state() {
        let tasks = vec![serial_member("up:alpha", "up", 0), serial_member("up:beta", "up", 1)];
        let groups = SerialGroups::new(&tasks);
        let beta = &tasks[1];

        let empty = std::collections::HashSet::new();
        let no_skips = SkippedSet::new();
        let one = |name: &str| std::collections::HashSet::from([name.to_string()]);
        let skipped_as = |name: &str, kind: SkipKind| SkippedSet::from([(name.to_string(), kind)]);

        // Nothing terminal yet.
        assert!(matches!(
            groups.classify(beta, &empty, &empty, &no_skips),
            EdgeState::Pending
        ));
        // Completed predecessor.
        assert!(matches!(
            groups.classify(beta, &one("up:alpha"), &empty, &no_skips),
            EdgeState::Satisfied
        ));
        // Failed predecessor.
        assert!(matches!(
            groups.classify(beta, &empty, &one("up:alpha"), &no_skips),
            EdgeState::Unreachable
        ));
        // An up-to-date predecessor is success-like and does not block its successor,
        // even though it is terminal-Skipped.
        assert!(matches!(
            groups.classify(
                beta,
                &one("up:alpha"),
                &empty,
                &skipped_as("up:alpha", SkipKind::UpToDate)
            ),
            EdgeState::Satisfied
        ));
        // Skipped for any other reason, including an ordinary when: edge going unreachable.
        assert!(matches!(
            groups.classify(beta, &empty, &empty, &skipped_as("up:alpha", SkipKind::Unreachable)),
            EdgeState::Unreachable
        ));
        assert!(matches!(
            groups.classify(
                beta,
                &empty,
                &empty,
                &skipped_as("up:alpha", SkipKind::SerialPredecessor)
            ),
            EdgeState::Unreachable
        ));
        // A task with no group is never gated.
        let plain = Task::new(
            "plain".to_string(),
            None,
            vec![],
            vec![],
            vec![],
            HashMap::new(),
            HashMap::new(),
            "echo hi".to_string(),
        );
        assert!(matches!(
            groups.classify(&plain, &empty, &empty, &no_skips),
            EdgeState::Satisfied
        ));
    }

    /// The gate composes with dependency readiness rather than replacing it.
    #[test]
    fn test_classify_gates_reports_both_edges_and_ordering() {
        let mut beta = serial_member("up:beta", "up", 1);
        beta.task_deps = vec![TaskEdge::success("dep")];
        let tasks = vec![serial_member("up:alpha", "up", 0), beta];
        let groups = SerialGroups::new(&tasks);

        let completed = std::collections::HashSet::from(["dep".to_string()]);
        let empty = std::collections::HashSet::new();
        let no_skips = SkippedSet::new();

        // Dependency satisfied, ordering still pending -> not ready.
        let states = classify_gates(&tasks[1], &groups, &completed, &empty, &no_skips);
        assert_eq!(states.len(), 2);
        assert!(states.iter().any(|s| matches!(s, EdgeState::Pending)));

        // Both satisfied -> ready.
        let completed = std::collections::HashSet::from(["dep".to_string(), "up:alpha".to_string()]);
        let states = classify_gates(&tasks[1], &groups, &completed, &empty, &no_skips);
        assert!(states.iter().all(|s| matches!(s, EdgeState::Satisfied)));
    }

    #[test]
    fn test_skip_record_names_the_serial_predecessor_and_carries_its_kind() {
        let tasks = vec![serial_member("up:alpha", "up", 0), serial_member("up:beta", "up", 1)];
        let groups = SerialGroups::new(&tasks);
        let empty = std::collections::HashSet::new();
        let no_skips = SkippedSet::new();

        let failed = std::collections::HashSet::from(["up:alpha".to_string()]);
        assert_eq!(
            skip_record_for(&tasks[1], &groups, &empty, &failed, &no_skips),
            SkipRecord::new(
                SkipKind::SerialPredecessor,
                "serial predecessor up:alpha failed".to_string()
            )
        );

        let skipped = SkippedSet::from([("up:alpha".to_string(), SkipKind::Unreachable)]);
        assert_eq!(
            skip_record_for(&tasks[1], &groups, &empty, &empty, &skipped),
            SkipRecord::new(
                SkipKind::SerialPredecessor,
                "serial predecessor up:alpha skipped; cascade".to_string()
            )
        );
    }

    /// An unreachable dependency edge is provenance `Unreachable`, not
    /// `SerialPredecessor`: the kind names which gate fired.
    #[test]
    fn test_skip_record_for_an_unreachable_edge_is_kind_unreachable() {
        let mut dependent = Task::new(
            "dependent".to_string(),
            None,
            vec![],
            vec![],
            vec![],
            HashMap::new(),
            HashMap::new(),
            "echo hi".to_string(),
        );
        dependent.task_deps = vec![TaskEdge::success("src")];
        let groups = SerialGroups::default();
        let empty = std::collections::HashSet::new();
        let failed = std::collections::HashSet::from(["src".to_string()]);

        assert_eq!(
            skip_record_for(&dependent, &groups, &empty, &failed, &SkippedSet::new()),
            SkipRecord::new(
                SkipKind::Unreachable,
                "dep src failed; this task required when: success".to_string()
            )
        );
    }

    /// The `SkipKind` x `when:` contract, all nine cells, asserted explicitly rather
    /// than left to whichever cells the other fixes happen to exercise.
    ///
    /// Both gates that read this contract are asserted against the same table:
    /// `classify_edge` (the scheduler's admission gate) and the worker's dependency
    /// double-check inside `execute_task`. They ran opposite policies before this
    /// phase, and a disagreement aborts at spawn time a task the scheduler admitted.
    #[test]
    fn classify_edge_skip_provenance_matrix() {
        use EdgeState::{Satisfied, Unreachable};

        let cells = [
            (SkipKind::UpToDate, When::Success, Satisfied),
            (SkipKind::UpToDate, When::Failure, Unreachable),
            (SkipKind::UpToDate, When::Always, Satisfied),
            (SkipKind::SerialPredecessor, When::Success, Unreachable),
            (SkipKind::SerialPredecessor, When::Failure, Unreachable),
            (SkipKind::SerialPredecessor, When::Always, Satisfied),
            (SkipKind::Unreachable, When::Success, Unreachable),
            (SkipKind::Unreachable, When::Failure, Unreachable),
            (SkipKind::Unreachable, When::Always, Satisfied),
        ];
        assert_eq!(cells.len(), 9, "the contract is a 3x3 table; every cell is asserted");

        let completed = std::collections::HashSet::new();
        let failed = std::collections::HashSet::new();

        for (kind, when, expected) in cells {
            let edge = TaskEdge::new("src".to_string(), when);
            let skipped = SkippedSet::from([("src".to_string(), kind)]);

            // Gate 1: the scheduler's edge classification.
            assert_eq!(
                classify_edge(&edge, &completed, &failed, &skipped),
                expected,
                "classify_edge: source skipped as {kind:?} against when: {when:?}"
            );

            // Gate 2: the worker's dependency double-check, which reads the kind off
            // the source's TaskStatus rather than off the runtime sets.
            let status = TaskStatus::Skipped(kind);
            let double_check_satisfied = match (when, Some(&status)) {
                (When::Success, Some(TaskStatus::Completed)) => true,
                (When::Success, Some(TaskStatus::Skipped(k))) => k.is_success_like(),
                (When::Failure, Some(TaskStatus::Failed(_))) => true,
                (When::Always, Some(TaskStatus::Completed)) => true,
                (When::Always, Some(TaskStatus::Skipped(_))) => true,
                (When::Always, Some(TaskStatus::Failed(_))) => true,
                _ => false,
            };
            assert_eq!(
                double_check_satisfied,
                expected == Satisfied,
                "worker double-check disagrees with classify_edge: {kind:?} against when: {when:?}"
            );
        }
    }

    #[tokio::test]
    #[serial]
    async fn test_task_execution() -> Result<()> {
        let temp_dir = TempDir::new()?;
        let work_dir = PathBuf::from(temp_dir.path());
        setup_test_db(&work_dir);

        let task = Task::new(
            "test".to_string(),
            None,
            vec![],
            vec![],
            vec![],
            HashMap::new(),
            HashMap::new(),
            "echo hello".to_string(),
        );

        let workspace = Workspace::new(work_dir).await?;
        workspace.init().await?;
        let scheduler = TaskScheduler::new(vec![task], Arc::new(workspace), ExecutionContext::new(), 2, false).await?;
        scheduler.execute_all().await?;

        let status = scheduler.get_task_status("test").await;
        assert_eq!(status, TaskStatus::Completed);

        Ok(())
    }

    #[tokio::test]
    #[serial]
    async fn test_task_dependencies() -> Result<()> {
        let temp_dir = TempDir::new()?;
        let work_dir = PathBuf::from(temp_dir.path());
        setup_test_db(&work_dir);

        let tasks = vec![
            Task::new(
                "task1".to_string(),
                None,
                vec![crate::executor::task::TaskEdge::success("task2")],
                vec![],
                vec![],
                HashMap::new(),
                HashMap::new(),
                "echo task1".to_string(),
            ),
            Task::new(
                "task2".to_string(),
                None,
                vec![],
                vec![],
                vec![],
                HashMap::new(),
                HashMap::new(),
                "echo task2".to_string(),
            ),
        ];

        let workspace = Workspace::new(work_dir).await?;
        workspace.init().await?;
        let scheduler = TaskScheduler::new(tasks, Arc::new(workspace), ExecutionContext::new(), 2, false).await?;
        scheduler.execute_all().await?;

        let task1_status = scheduler.get_task_status("task1").await;
        let task2_status = scheduler.get_task_status("task2").await;

        assert_eq!(task1_status, TaskStatus::Completed);
        assert_eq!(task2_status, TaskStatus::Completed);

        Ok(())
    }

    #[tokio::test]
    #[serial]
    async fn test_task_failure() -> Result<()> {
        let temp_dir = TempDir::new()?;
        let work_dir = PathBuf::from(temp_dir.path());
        setup_test_db(&work_dir);

        let tasks = vec![
            Task::new(
                "task1".to_string(),
                None,
                vec![],
                vec![],
                vec![],
                HashMap::new(),
                HashMap::new(),
                "exit 1".to_string(),
            ),
            Task::new(
                "task2".to_string(),
                None,
                vec![crate::executor::task::TaskEdge::success("task1")],
                vec![],
                vec![],
                HashMap::new(),
                HashMap::new(),
                "echo task2".to_string(),
            ),
        ];

        let workspace = Workspace::new(work_dir).await?;
        workspace.init().await?;
        let scheduler = TaskScheduler::new(tasks, Arc::new(workspace), ExecutionContext::new(), 2, false).await?;
        let result = scheduler.execute_all().await;

        assert!(result.is_err());

        Ok(())
    }

    #[tokio::test]
    #[serial]
    async fn test_file_dependencies() -> Result<()> {
        let temp_dir = TempDir::new()?;
        let work_dir = PathBuf::from(temp_dir.path());
        setup_test_db(&work_dir);

        let input_file = work_dir.join("input.txt");
        let output_file = work_dir.join("output.txt");
        tokio::fs::write(&input_file, "test content").await?;

        let task = Task::new(
            "copy_task".to_string(),
            None,
            vec![],
            vec![input_file.to_string_lossy().to_string()],
            vec![output_file.to_string_lossy().to_string()],
            HashMap::new(),
            HashMap::new(),
            format!("cp {} {}", input_file.display(), output_file.display()),
        );

        let workspace = Workspace::new(work_dir.clone()).await?;
        workspace.init().await?;
        let scheduler = TaskScheduler::new(
            vec![task.clone()],
            Arc::new(workspace),
            ExecutionContext::new(),
            2,
            false,
        )
        .await?;

        let needs_rebuild = scheduler.needs_rebuild(&task).await?;
        assert!(needs_rebuild, "Task should need to run when output doesn't exist");

        // Simulate file creation with newer timestamp
        tokio::fs::write(&output_file, "output content").await?;

        let now = std::time::SystemTime::now();
        let future_time = filetime::FileTime::from_system_time(now + std::time::Duration::from_secs(1));
        filetime::set_file_times(&output_file, future_time, future_time)?;

        // Now the task should not need to run (output newer than input)
        let needs_rebuild_after = scheduler.needs_rebuild(&task).await?;
        assert!(
            !needs_rebuild_after,
            "Task should not need to run when output is newer than inputs"
        );

        Ok(())
    }

    #[tokio::test]
    #[serial]
    async fn test_file_timestamp_checking() -> Result<()> {
        let temp_dir = TempDir::new()?;
        let work_dir = PathBuf::from(temp_dir.path());
        setup_test_db(&work_dir);

        let file1 = work_dir.join("file1.txt");
        let file2 = work_dir.join("file2.txt");

        tokio::fs::write(&file1, "content1").await?;
        tokio::fs::write(&file2, "content2").await?;

        let workspace = Workspace::new(work_dir).await?;
        workspace.init().await?;
        let scheduler = TaskScheduler::new(vec![], Arc::new(workspace), ExecutionContext::new(), 2, false).await?;

        // Test timestamp retrieval
        let timestamps = scheduler
            .get_file_timestamps(&[file1.to_string_lossy().to_string(), file2.to_string_lossy().to_string()])
            .await?;

        assert_eq!(timestamps.len(), 2);
        assert!(timestamps[0].1.is_some(), "Should have timestamp for existing file");
        assert!(timestamps[1].1.is_some(), "Should have timestamp for existing file");

        Ok(())
    }

    #[tokio::test]
    #[serial]
    async fn test_file_dependencies_nonexistent_files() -> Result<()> {
        let temp_dir = TempDir::new()?;
        let work_dir = PathBuf::from(temp_dir.path());
        setup_test_db(&work_dir);

        let nonexistent_file = work_dir.join("nonexistent.txt");
        let output_file = work_dir.join("output.txt");

        let task = Task::new(
            "test_nonexistent".to_string(),
            None,
            vec![],
            vec![nonexistent_file.to_string_lossy().to_string()],
            vec![output_file.to_string_lossy().to_string()],
            HashMap::new(),
            HashMap::new(),
            format!("touch {}", output_file.display()),
        );

        let workspace = Workspace::new(work_dir.clone()).await?;
        workspace.init().await?;
        let scheduler = TaskScheduler::new(
            vec![task.clone()],
            Arc::new(workspace),
            ExecutionContext::new(),
            2,
            false,
        )
        .await?;

        // Should need to rebuild when input file doesn't exist (conservative approach)
        let needs_rebuild = scheduler.needs_rebuild(&task).await?;
        assert!(needs_rebuild, "Task should need to run when input file doesn't exist");

        Ok(())
    }

    #[tokio::test]
    #[serial]
    async fn test_file_dependencies_multiple_inputs_outputs() -> Result<()> {
        let temp_dir = TempDir::new()?;
        let work_dir = PathBuf::from(temp_dir.path());
        setup_test_db(&work_dir);

        let input1 = work_dir.join("input1.txt");
        let input2 = work_dir.join("input2.txt");
        let input3 = work_dir.join("input3.txt");
        let output1 = work_dir.join("output1.txt");
        let output2 = work_dir.join("output2.txt");

        tokio::fs::write(&input1, "content1").await?;
        tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;
        tokio::fs::write(&input2, "content2").await?;
        tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;
        tokio::fs::write(&input3, "content3").await?;

        let task = Task::new(
            "multi_files".to_string(),
            None,
            vec![],
            vec![
                input1.to_string_lossy().to_string(),
                input2.to_string_lossy().to_string(),
                input3.to_string_lossy().to_string(),
            ],
            vec![
                output1.to_string_lossy().to_string(),
                output2.to_string_lossy().to_string(),
            ],
            HashMap::new(),
            HashMap::new(),
            format!(
                "cat {} {} {} > {} && cp {} {}",
                input1.display(),
                input2.display(),
                input3.display(),
                output1.display(),
                output1.display(),
                output2.display()
            ),
        );

        let workspace = Workspace::new(work_dir.clone()).await?;
        workspace.init().await?;
        let scheduler = TaskScheduler::new(
            vec![task.clone()],
            Arc::new(workspace),
            ExecutionContext::new(),
            2,
            false,
        )
        .await?;

        // Should need to rebuild when outputs don't exist
        let needs_rebuild = scheduler.needs_rebuild(&task).await?;
        assert!(needs_rebuild, "Task should need to run when outputs don't exist");

        tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;
        tokio::fs::write(&output1, "combined output").await?;
        tokio::fs::write(&output2, "combined output copy").await?;

        // Should not need to rebuild when all outputs are newer than all inputs
        let needs_rebuild_after = scheduler.needs_rebuild(&task).await?;
        assert!(
            !needs_rebuild_after,
            "Task should not need to run when all outputs are newer than all inputs"
        );

        // Touch one of the input files to make it newer
        tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;
        tokio::fs::write(&input2, "modified content2").await?;

        // Should need to rebuild when any input is newer than any output
        let needs_rebuild_final = scheduler.needs_rebuild(&task).await?;
        assert!(
            needs_rebuild_final,
            "Task should need to run when any input is newer than outputs"
        );

        Ok(())
    }

    #[tokio::test]
    #[serial]
    async fn test_file_dependencies_with_task_dependencies() -> Result<()> {
        let temp_dir = TempDir::new()?;
        let work_dir = PathBuf::from(temp_dir.path());
        setup_test_db(&work_dir);

        let input_file = work_dir.join("input.txt");
        let intermediate_file = work_dir.join("intermediate.txt");
        let output_file = work_dir.join("output.txt");

        tokio::fs::write(&input_file, "initial content").await?;

        let task1 = Task::new(
            "step1".to_string(),
            None,
            vec![],
            vec![input_file.to_string_lossy().to_string()],
            vec![intermediate_file.to_string_lossy().to_string()],
            HashMap::new(),
            HashMap::new(),
            format!("cp {} {}", input_file.display(), intermediate_file.display()),
        );

        let task2 = Task::new(
            "step2".to_string(),
            None,
            vec![crate::executor::task::TaskEdge::success("step1")], // Task dependency
            vec![intermediate_file.to_string_lossy().to_string()],   // File dependency
            vec![output_file.to_string_lossy().to_string()],
            HashMap::new(),
            HashMap::new(),
            format!("cp {} {}", intermediate_file.display(), output_file.display()),
        );

        let workspace = Workspace::new(work_dir.clone()).await?;
        workspace.init().await?;
        let scheduler = TaskScheduler::new(
            vec![task1.clone(), task2.clone()],
            Arc::new(workspace),
            ExecutionContext::new(),
            2,
            false,
        )
        .await?;

        // Both tasks should need to run initially
        let task1_needs_rebuild = scheduler.needs_rebuild(&task1).await?;
        let task2_needs_rebuild = scheduler.needs_rebuild(&task2).await?;
        assert!(task1_needs_rebuild, "Task1 should need to run initially");
        assert!(task2_needs_rebuild, "Task2 should need to run initially");

        // Execute all tasks
        scheduler.execute_all().await?;

        let task1_status = scheduler.get_task_status("step1").await;
        let task2_status = scheduler.get_task_status("step2").await;
        assert_eq!(task1_status, TaskStatus::Completed);
        assert_eq!(task2_status, TaskStatus::Completed);

        assert!(intermediate_file.exists(), "Intermediate file should exist");
        assert!(output_file.exists(), "Output file should exist");

        Ok(())
    }

    #[tokio::test]
    #[serial]
    async fn test_file_dependencies_timestamp_precision() -> Result<()> {
        let temp_dir = TempDir::new()?;
        // Without this the workspace lands wherever the previously-run
        // serial test left OTTO_HOME pointing (the MemFs workspace tests set it
        // to /otto-home and never restore it), which is not writable.
        setup_test_db(temp_dir.path());
        let work_dir = PathBuf::from(temp_dir.path());

        let input_file = work_dir.join("input.txt");
        let output_file = work_dir.join("output.txt");

        tokio::fs::write(&input_file, "content").await?;

        tokio::fs::write(&output_file, "output").await?;

        let task = Task::new(
            "timestamp_test".to_string(),
            None,
            vec![],
            vec![input_file.to_string_lossy().to_string()],
            vec![output_file.to_string_lossy().to_string()],
            HashMap::new(),
            HashMap::new(),
            format!("cp {} {}", input_file.display(), output_file.display()),
        );

        let workspace = Workspace::new(work_dir).await?;
        workspace.init().await?;
        let scheduler = TaskScheduler::new(
            vec![task.clone()],
            Arc::new(workspace),
            ExecutionContext::new(),
            2,
            false,
        )
        .await?;

        // When timestamps are very close, should be conservative and rebuild
        let needs_rebuild = scheduler.needs_rebuild(&task).await?;
        // This might be true or false depending on timestamp precision, but should be consistent
        println!("Timestamp precision test - needs rebuild: {needs_rebuild}");

        Ok(())
    }

    #[tokio::test]
    #[serial]
    async fn test_file_dependencies_empty_lists() -> Result<()> {
        let temp_dir = TempDir::new()?;
        // Without this the workspace lands wherever the previously-run
        // serial test left OTTO_HOME pointing (the MemFs workspace tests set it
        // to /otto-home and never restore it), which is not writable.
        setup_test_db(temp_dir.path());
        let work_dir = PathBuf::from(temp_dir.path());

        // Task with no file dependencies
        let task = Task::new(
            "no_file_deps".to_string(),
            None,
            vec![],
            vec![], // No input files
            vec![], // No output files
            HashMap::new(),
            HashMap::new(),
            "echo 'no file dependencies'".to_string(),
        );

        let workspace = Workspace::new(work_dir).await?;
        workspace.init().await?;
        let scheduler = TaskScheduler::new(
            vec![task.clone()],
            Arc::new(workspace),
            ExecutionContext::new(),
            2,
            false,
        )
        .await?;

        // Should always need to run when there are no file dependencies to check
        let needs_rebuild = scheduler.needs_rebuild(&task).await?;
        assert!(needs_rebuild, "Task with no file dependencies should always run");

        Ok(())
    }

    #[tokio::test]
    #[serial]
    async fn test_file_dependencies_directory_as_input() -> Result<()> {
        let temp_dir = TempDir::new()?;
        let work_dir = PathBuf::from(temp_dir.path());
        setup_test_db(&work_dir);

        let src_dir = work_dir.join("src");
        tokio::fs::create_dir_all(&src_dir).await?;
        tokio::fs::write(src_dir.join("file1.txt"), "content1").await?;
        tokio::fs::write(src_dir.join("file2.txt"), "content2").await?;

        let output_file = work_dir.join("output.txt");

        let task = Task::new(
            "dir_input".to_string(),
            None,
            vec![],
            vec![src_dir.to_string_lossy().to_string()], // Directory as input
            vec![output_file.to_string_lossy().to_string()],
            HashMap::new(),
            HashMap::new(),
            format!(
                "find {} -name '*.txt' | wc -l > {}",
                src_dir.display(),
                output_file.display()
            ),
        );

        let workspace = Workspace::new(work_dir).await?;
        workspace.init().await?;
        let scheduler = TaskScheduler::new(
            vec![task.clone()],
            Arc::new(workspace),
            ExecutionContext::new(),
            2,
            false,
        )
        .await?;

        // Should handle directory dependencies (gets modification time of directory)
        let needs_rebuild = scheduler.needs_rebuild(&task).await?;
        assert!(needs_rebuild, "Task should need to run when output doesn't exist");

        Ok(())
    }

    #[tokio::test]
    #[serial]
    async fn test_large_number_of_file_dependencies() -> Result<()> {
        let temp_dir = TempDir::new()?;
        let work_dir = PathBuf::from(temp_dir.path());
        setup_test_db(&work_dir);

        let mut input_files = Vec::new();
        for i in 0..100 {
            let file = work_dir.join(format!("input_{i:03}.txt"));
            tokio::fs::write(&file, format!("content {i}")).await?;
            input_files.push(file.to_string_lossy().to_string());
        }

        let output_file = work_dir.join("combined.txt");

        let task = Task::new(
            "many_inputs".to_string(),
            None,
            vec![],
            input_files,
            vec![output_file.to_string_lossy().to_string()],
            HashMap::new(),
            HashMap::new(),
            format!("cat input_*.txt > {}", output_file.display()),
        );

        let workspace = Workspace::new(work_dir).await?;
        workspace.init().await?;
        let scheduler = TaskScheduler::new(
            vec![task.clone()],
            Arc::new(workspace),
            ExecutionContext::new(),
            2,
            false,
        )
        .await?;

        // Should handle large numbers of file dependencies efficiently
        let start = std::time::Instant::now();
        let needs_rebuild = scheduler.needs_rebuild(&task).await?;
        let duration = start.elapsed();

        assert!(needs_rebuild, "Task should need to run when output doesn't exist");
        assert!(
            duration.as_millis() < 1000,
            "File dependency checking should be fast even with many files"
        );

        Ok(())
    }

    #[tokio::test]
    #[serial]
    async fn test_file_dependencies_circular_detection() -> Result<()> {
        let temp_dir = TempDir::new()?;
        let work_dir = PathBuf::from(temp_dir.path());
        setup_test_db(&work_dir);

        let file_a = work_dir.join("a.txt");
        let file_b = work_dir.join("b.txt");

        tokio::fs::write(&file_a, "content a").await?;
        tokio::fs::write(&file_b, "content b").await?;

        // Task that uses its output as input (circular dependency)
        let task = Task::new(
            "circular".to_string(),
            None,
            vec![],
            vec![file_a.to_string_lossy().to_string()],
            vec![file_a.to_string_lossy().to_string()], // Same file as input and output
            HashMap::new(),
            HashMap::new(),
            format!("echo 'modified' >> {}", file_a.display()),
        );

        let workspace = Workspace::new(work_dir).await?;
        workspace.init().await?;
        let scheduler = TaskScheduler::new(
            vec![task.clone()],
            Arc::new(workspace),
            ExecutionContext::new(),
            2,
            false,
        )
        .await?;

        // Should handle circular file dependencies gracefully
        let needs_rebuild = scheduler.needs_rebuild(&task).await?;
        // Should be conservative when input and output are the same file
        println!("Circular dependency test - needs rebuild: {needs_rebuild}");

        Ok(())
    }

    #[tokio::test]
    #[serial]
    async fn test_file_dependencies_integration_with_real_execution() -> Result<()> {
        let temp_dir = TempDir::new()?;
        let work_dir = PathBuf::from(temp_dir.path());
        setup_test_db(&work_dir);

        let input_file = work_dir.join("source.txt");
        let output_file = work_dir.join("result.txt");

        tokio::fs::write(&input_file, "Hello, World!").await?;

        let task = Task::new(
            "real_execution".to_string(),
            None,
            vec![],
            vec![input_file.to_string_lossy().to_string()],
            vec![output_file.to_string_lossy().to_string()],
            HashMap::new(),
            HashMap::new(),
            format!("cp {} {}", input_file.display(), output_file.display()),
        );

        let workspace = Workspace::new(work_dir.clone()).await?;
        workspace.init().await?;
        let scheduler = TaskScheduler::new(
            vec![task.clone()],
            Arc::new(workspace),
            ExecutionContext::new(),
            2,
            false,
        )
        .await?;

        let needs_rebuild_1 = scheduler.needs_rebuild(&task).await?;
        assert!(needs_rebuild_1, "Should need to run initially");

        scheduler.execute_all().await?;

        let status = scheduler.get_task_status("real_execution").await;
        assert_eq!(status, TaskStatus::Completed);
        assert!(output_file.exists(), "Output file should exist after execution");

        let output_content = tokio::fs::read_to_string(&output_file).await?;
        assert_eq!(output_content, "Hello, World!", "Output should match input");

        let needs_rebuild_2 = scheduler.needs_rebuild(&task).await?;
        assert!(!needs_rebuild_2, "Should not need to run when output is up-to-date");

        // Modify input file to trigger rebuild
        tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;
        tokio::fs::write(&input_file, "Modified content!").await?;

        let needs_rebuild_3 = scheduler.needs_rebuild(&task).await?;
        assert!(needs_rebuild_3, "Should need to run when input is modified");

        Ok(())
    }

    #[tokio::test]
    #[serial]
    async fn test_parallel_execution_limit() -> Result<()> {
        let temp_dir = TempDir::new()?;
        let work_dir = temp_dir.path().to_path_buf();
        setup_test_db(&work_dir);

        let mut tasks = vec![];
        for i in 1..=4 {
            let task = Task::new(
                format!("task{i}"),
                None,
                vec![],
                vec![],
                vec![],
                HashMap::new(),
                HashMap::new(),
                format!("sleep 0.1 && echo task{i}"),
            );
            tasks.push(task);
        }

        let workspace = Workspace::new(work_dir).await?;
        workspace.init().await?;

        let scheduler = TaskScheduler::new(tasks, Arc::new(workspace), ExecutionContext::new(), 2, false).await?;

        assert_eq!(scheduler.semaphore.available_permits(), 2);

        // Execute all tasks
        scheduler.execute_all().await?;

        for i in 1..=4 {
            let status = scheduler.get_task_status(&format!("task{i}")).await;
            assert_eq!(status, TaskStatus::Completed);
        }

        Ok(())
    }

    #[tokio::test]
    #[serial]
    async fn test_scheduler_respects_max_parallel() -> Result<()> {
        // Test with different job limits
        for max_parallel in [1, 2, 4, 8] {
            let temp_dir = TempDir::new()?;
            let work_dir = temp_dir.path().to_path_buf();
            setup_test_db(&work_dir);

            let workspace = Workspace::new(work_dir).await?;
            workspace.init().await?;

            let tasks = vec![Task::new(
                "test".to_string(),
                None,
                vec![],
                vec![],
                vec![],
                HashMap::new(),
                HashMap::new(),
                "echo test".to_string(),
            )];

            let scheduler =
                TaskScheduler::new(tasks, Arc::new(workspace), ExecutionContext::new(), max_parallel, false).await?;

            assert_eq!(
                scheduler.semaphore.available_permits(),
                max_parallel,
                "Scheduler should have {max_parallel} permits"
            );
        }

        Ok(())
    }

    #[tokio::test]
    #[serial]
    async fn test_task_skipped_when_outputs_up_to_date() -> Result<()> {
        let temp_dir = TempDir::new()?;
        let work_dir = PathBuf::from(temp_dir.path());
        setup_test_db(&work_dir);

        let input_file = work_dir.join("input.txt");
        let output_file = work_dir.join("output.txt");

        // Create input and output, with output newer than input
        tokio::fs::write(&input_file, "input content").await?;
        tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;
        tokio::fs::write(&output_file, "output content").await?;

        let task = Task::new(
            "skip_test".to_string(),
            None,
            vec![],
            vec![input_file.to_string_lossy().to_string()],
            vec![output_file.to_string_lossy().to_string()],
            HashMap::new(),
            HashMap::new(),
            "echo should be skipped".to_string(),
        );

        let workspace = Workspace::new(work_dir.clone()).await?;
        workspace.init().await?;
        let scheduler = TaskScheduler::new(
            vec![task.clone()],
            Arc::new(workspace),
            ExecutionContext::new(),
            2,
            false,
        )
        .await?;

        // Execute should skip the task
        scheduler.execute_all().await?;

        let status = scheduler.get_task_status("skip_test").await;
        assert_eq!(
            status,
            TaskStatus::Skipped(SkipKind::UpToDate),
            "Task should be skipped as up-to-date when outputs are current"
        );

        Ok(())
    }

    #[tokio::test]
    #[serial]
    async fn test_tui_mode_message_broadcasting() -> Result<()> {
        let temp_dir = TempDir::new()?;
        let work_dir = PathBuf::from(temp_dir.path());
        setup_test_db(&work_dir);

        let task = Task::new(
            "tui_test".to_string(),
            None,
            vec![],
            vec![],
            vec![],
            HashMap::new(),
            HashMap::new(),
            "echo testing tui".to_string(),
        );

        let workspace = Workspace::new(work_dir).await?;
        workspace.init().await?;

        let mut scheduler = TaskScheduler::new(
            vec![task],
            Arc::new(workspace),
            ExecutionContext::new(),
            2,
            true, // TUI mode enabled
        )
        .await?;

        // Set up message channel
        let (tx, mut rx) = tokio::sync::broadcast::channel(100);
        scheduler.set_message_channel(tx);

        // Execute task
        let exec_handle = tokio::spawn(async move { scheduler.execute_all().await });

        // Collect messages
        let mut _received_started = false;
        let mut _received_finished = false;

        while let Ok(msg) = rx.try_recv() {
            match msg {
                TaskMessage::Started { task_name, .. } => {
                    assert_eq!(task_name, "tui_test");
                    _received_started = true;
                }
                TaskMessage::Finished {
                    task_name,
                    status,
                    duration_ms,
                    ..
                } => {
                    assert_eq!(task_name, "tui_test");
                    assert_eq!(status, TuiTaskStatus::Completed);
                    assert!(duration_ms > 0, "Duration should be tracked");
                    _received_finished = true;
                }
                _ => {}
            }
        }

        exec_handle.await??;

        // Note: Messages might be dropped if we don't subscribe early enough
        // This test mainly verifies the broadcasting mechanism works
        Ok(())
    }

    #[tokio::test]
    #[serial]
    async fn test_duration_tracking_accuracy() -> Result<()> {
        let temp_dir = TempDir::new()?;
        let work_dir = PathBuf::from(temp_dir.path());
        setup_test_db(&work_dir);

        let task = Task::new(
            "duration_test".to_string(),
            None,
            vec![],
            vec![],
            vec![],
            HashMap::new(),
            HashMap::new(),
            "sleep 0.2".to_string(),
        );

        let workspace = Workspace::new(work_dir).await?;
        workspace.init().await?;

        let mut scheduler =
            TaskScheduler::new(vec![task], Arc::new(workspace), ExecutionContext::new(), 2, true).await?;

        // Set up message channel to capture duration
        let (tx, mut rx) = tokio::sync::broadcast::channel(100);
        scheduler.set_message_channel(tx);

        let start = std::time::Instant::now();

        let exec_handle = tokio::spawn(async move { scheduler.execute_all().await });

        let mut captured_duration_ms = None;

        // Wait for task to complete and capture duration
        loop {
            match tokio::time::timeout(tokio::time::Duration::from_secs(5), rx.recv()).await {
                Ok(Ok(TaskMessage::Finished { duration_ms, .. })) => {
                    captured_duration_ms = Some(duration_ms);
                    break;
                }
                Ok(Ok(_)) => continue,
                Ok(Err(_)) => break, // Channel closed
                Err(_) => break,     // Timeout
            }
        }

        exec_handle.await??;

        let total_elapsed = start.elapsed().as_millis() as u64;

        // Verify duration was captured and is reasonable
        if let Some(duration_ms) = captured_duration_ms {
            assert!(
                duration_ms >= 200,
                "Duration should be at least 200ms, got {}",
                duration_ms
            );
            assert!(
                duration_ms <= total_elapsed + 100,
                "Duration should not exceed total time"
            );
        }

        Ok(())
    }

    #[tokio::test]
    #[serial]
    async fn test_file_dependency_check_error_handling() -> Result<()> {
        let temp_dir = TempDir::new()?;
        let work_dir = PathBuf::from(temp_dir.path());
        setup_test_db(&work_dir);

        // Create a task with a file dependency in a path that will cause issues
        let task = Task::new(
            "error_test".to_string(),
            None,
            vec![],
            vec!["/dev/null/impossible/path".to_string()],
            vec![work_dir.join("output.txt").to_string_lossy().to_string()],
            HashMap::new(),
            HashMap::new(),
            "echo handled error".to_string(),
        );

        let workspace = Workspace::new(work_dir).await?;
        workspace.init().await?;
        let scheduler = TaskScheduler::new(vec![task], Arc::new(workspace), ExecutionContext::new(), 2, false).await?;

        // Should handle the error gracefully and still run the task
        let result = scheduler.execute_all().await;

        // The task should complete despite file dependency check errors
        assert!(result.is_ok(), "Should handle file dependency errors gracefully");

        Ok(())
    }

    #[tokio::test]
    #[serial]
    async fn test_multiple_tasks_with_mixed_states() -> Result<()> {
        let temp_dir = TempDir::new()?;
        let work_dir = PathBuf::from(temp_dir.path());
        setup_test_db(&work_dir);

        // Create files for skip test
        let input1 = work_dir.join("input1.txt");
        let output1 = work_dir.join("output1.txt");
        tokio::fs::write(&input1, "content").await?;
        tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;
        tokio::fs::write(&output1, "output").await?;

        let tasks = vec![
            // Task that will be skipped (output up to date)
            Task::new(
                "skip_task".to_string(),
                None,
                vec![],
                vec![input1.to_string_lossy().to_string()],
                vec![output1.to_string_lossy().to_string()],
                HashMap::new(),
                HashMap::new(),
                "echo skipped".to_string(),
            ),
            // Task that will run
            Task::new(
                "run_task".to_string(),
                None,
                vec![],
                vec![],
                vec![],
                HashMap::new(),
                HashMap::new(),
                "echo running".to_string(),
            ),
            // Task with dependency on both
            Task::new(
                "dependent_task".to_string(),
                None,
                vec![
                    crate::executor::task::TaskEdge::success("skip_task"),
                    crate::executor::task::TaskEdge::success("run_task"),
                ],
                vec![],
                vec![],
                HashMap::new(),
                HashMap::new(),
                "echo dependent".to_string(),
            ),
        ];

        let workspace = Workspace::new(work_dir).await?;
        workspace.init().await?;
        let scheduler = TaskScheduler::new(tasks, Arc::new(workspace), ExecutionContext::new(), 2, false).await?;

        scheduler.execute_all().await?;

        let skip_status = scheduler.get_task_status("skip_task").await;
        let run_status = scheduler.get_task_status("run_task").await;
        let dep_status = scheduler.get_task_status("dependent_task").await;

        assert_eq!(skip_status, TaskStatus::Skipped(SkipKind::UpToDate));
        assert_eq!(run_status, TaskStatus::Completed);
        assert_eq!(dep_status, TaskStatus::Completed);

        Ok(())
    }

    // ------------------------------------------------------------------
    // Phase 7: tty: true
    // ------------------------------------------------------------------

    /// An ordinary task takes one permit; a tty task takes the semaphore's whole
    /// initial count, which is the entire mechanism behind "runs exclusively".
    #[test]
    fn test_permits_for_tty_takes_the_whole_semaphore() {
        for max_parallel in [1usize, 2, 4, 32] {
            assert_eq!(permits_for(false, max_parallel).unwrap(), 1);
            assert_eq!(
                permits_for(true, max_parallel).unwrap(),
                u32::try_from(max_parallel).unwrap(),
                "a tty task must request all {max_parallel} permits"
            );
        }
    }

    /// A permit count that cannot be expressed as a u32 is a loud error, not a
    /// silently clamped request that would fail to be exclusive.
    #[test]
    fn test_permits_for_rejects_counts_beyond_u32() {
        let err = permits_for(true, usize::MAX).unwrap_err().to_string();
        assert!(
            err.contains("exceeds the semaphore\'s permit limit"),
            "unexpected error: {err}"
        );
    }

    /// The scheduler keeps the count it was built with; `available_permits()` is
    /// the count free *right now*, which is the wrong number to hand acquire_many.
    #[tokio::test]
    #[serial]
    async fn test_scheduler_records_its_initial_permit_count() -> Result<()> {
        let temp_dir = TempDir::new()?;
        let work_dir = temp_dir.path().to_path_buf();
        setup_test_db(&work_dir);

        let workspace = Workspace::new(work_dir).await?;
        workspace.init().await?;
        let scheduler = TaskScheduler::new(vec![], Arc::new(workspace), ExecutionContext::new(), 7, false).await?;

        assert_eq!(scheduler.max_parallel, 7);
        let _held = scheduler.semaphore.acquire_many(3).await?;
        assert_eq!(scheduler.semaphore.available_permits(), 4);
        assert_eq!(
            scheduler.max_parallel, 7,
            "max_parallel must not track live availability"
        );

        Ok(())
    }

    /// A tty task never opens TaskStreams, so nothing else would create these
    /// files. History records both paths at task start; empty files would claim a
    /// silent task.
    #[tokio::test]
    async fn test_tty_log_markers_are_written() -> Result<()> {
        let temp_dir = TempDir::new()?;
        let tasks_dir = temp_dir.path().join("tasks");

        write_tty_log_markers(&tasks_dir, "login").await?;

        for file in ["stdout.log", "stderr.log"] {
            let content = tokio::fs::read_to_string(tasks_dir.join("login").join(file)).await?;
            assert_eq!(content, format!("{TTY_LOG_MARKER}\n"), "{file} marker");
        }

        Ok(())
    }

    /// End-to-end through the real scheduler: a tty task's output is not captured
    /// (marker only), while a plain task in the same run still is.
    #[tokio::test]
    #[serial]
    async fn test_tty_task_logs_carry_the_marker_and_plain_tasks_still_capture() -> Result<()> {
        let temp_dir = TempDir::new()?;
        let work_dir = temp_dir.path().to_path_buf();
        setup_test_db(&work_dir);

        let mut tty_task = Task::new(
            "interactive".to_string(),
            None,
            vec![],
            vec![],
            vec![],
            HashMap::new(),
            HashMap::new(),
            "echo from-the-tty-task".to_string(),
        );
        tty_task.tty = true;
        let plain_task = Task::new(
            "plain".to_string(),
            None,
            vec![],
            vec![],
            vec![],
            HashMap::new(),
            HashMap::new(),
            "echo from-the-plain-task".to_string(),
        );

        let workspace = Workspace::new(work_dir).await?;
        workspace.init().await?;
        let tasks_dir = workspace.run().join("tasks");
        let scheduler = TaskScheduler::new(
            vec![tty_task, plain_task],
            Arc::new(workspace),
            ExecutionContext::new(),
            4,
            false,
        )
        .await?;

        scheduler.execute_all().await?;

        assert_eq!(scheduler.get_task_status("interactive").await, TaskStatus::Completed);
        assert_eq!(scheduler.get_task_status("plain").await, TaskStatus::Completed);

        let tty_log = tokio::fs::read_to_string(tasks_dir.join("interactive").join("stdout.log")).await?;
        assert_eq!(tty_log, format!("{TTY_LOG_MARKER}\n"));
        assert!(
            !tty_log.contains("from-the-tty-task"),
            "a tty task must not be captured: {tty_log}"
        );

        let plain_log = tokio::fs::read_to_string(tasks_dir.join("plain").join("stdout.log")).await?;
        assert!(
            plain_log.contains("from-the-plain-task"),
            "a plain task must still be captured: {plain_log}"
        );

        Ok(())
    }
}
