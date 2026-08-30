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
        // Shell identifiers cannot hold `-` or `.`, so distinct JSON keys can
        // fold onto one variable name: `a-b`, `a.b` and `A_B` all become
        // `..._A_B`. Every one of them used to be emitted under that single
        // name, so the last assignment won and the other values disappeared
        // with nothing said. Measured before this: three keys in, one value
        // out, `otto_get_input producer.a-b` -> `[]` at exit 0.
        //
        // Suffixing a collision keeps every value reachable rather than merely
        // reporting the loss. The reader does not care what the variable is
        // called - it recovers the real key from the companion
        // `OTTO_INPUTKEY_` variable, which is suffixed in lockstep.
        let mut used: std::collections::HashSet<String> = std::collections::HashSet::new();

        for (key, value) in obj {
            // Create variable name: OTTO_INPUT_<TASK>_<KEY> (uppercase, safe chars only)
            let safe_task = task_name.to_uppercase().replace(['-', '.'], "_");
            let safe_key = key.to_uppercase().replace(['-', '.'], "_");
            let base = format!("OTTO_INPUT_{safe_task}_{safe_key}");
            let mut var_name = base.clone();
            let mut disambiguator = 1;
            while !used.insert(var_name.clone()) {
                disambiguator += 1;
                var_name = format!("{base}_{disambiguator}");
            }
            if var_name != base {
                log::warn!(
                    "task '{task_name}' output keys collide as shell variables: '{key}' folds onto                      {base}, which is taken; it is carried as {var_name} instead"
                );
            }

            // Convert value to string and escape for bash
            let str_value = match value {
                serde_json::Value::String(s) => s.clone(),
                serde_json::Value::Null => String::new(),
                other => other.to_string(),
            };

            // Escape single quotes for bash single-quoted string
            let escaped = str_value.replace('\'', "'\\''");
            lines.push(format!("{var_name}='{escaped}'"));

            // The key's original spelling, carried alongside the value.
            //
            // `var_name` is uppercased and has `-`/`.` folded to `_`, so the
            // key cannot be recovered from it. The bash side used to guess by
            // lowercasing, which meant `otto_get_input producer.MIXED_Case`
            // returned empty at exit 0 while `producer.mixed_case` returned the
            // value - a silent wrong answer for any key that was not already
            // lowercase. The JSON is the source of truth for key names (the
            // Python generator reads it directly and has never had this bug);
            // this line is how bash gets at it.
            //
            // `OTTO_INPUTKEY_` rather than `OTTO_INPUT_KEY_`: the reader scans
            // for the prefix `OTTO_INPUT_<TASK>_`, and a task literally named
            // `key` would collide with the latter.
            let escaped_key = key.replace('\'', "'\\''");
            let key_var = var_name.replacen("OTTO_INPUT_", "OTTO_INPUTKEY_", 1);
            lines.push(format!("{key_var}='{escaped_key}'"));
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

/// The same contract as `classify_edge`, read off a source's terminal
/// `TaskStatus` instead of off the runtime sets.
///
/// This is the worker's dependency double-check, the second gate on the same
/// edge. It lives here, as a named function both the worker and
/// `classify_edge_skip_provenance_matrix` call, because the guarantee the design
/// bought with the five-site change is that the two gates cannot drift: a test
/// that re-transcribes these arms asserts its own copy and stays green while the
/// gate it claims to pin moves underneath it.
///
/// A disagreement with `classify_edge` aborts at spawn time a task the scheduler
/// just admitted, so the nine cells here are the nine cells in that table.
fn edge_satisfied_by_status(when: When, status: Option<&TaskStatus>) -> bool {
    match (when, status) {
        // when: success requires Completed, or a skip that is success-like: an
        // up-to-date source produced current outputs.
        (When::Success, Some(TaskStatus::Completed)) => true,
        (When::Success, Some(TaskStatus::Skipped(kind))) => kind.is_success_like(),
        // when: failure requires a Failed status on the source: it ran and its
        // action exited non-zero. No skip satisfies it.
        (When::Failure, Some(TaskStatus::Failed(_))) => true,
        // when: always is satisfied by any terminal state, whatever the skip
        // kind, which is what makes cleanup reliable.
        (When::Always, Some(TaskStatus::Completed)) => true,
        (When::Always, Some(TaskStatus::Skipped(_))) => true,
        (When::Always, Some(TaskStatus::Failed(_))) => true,
        _ => false,
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
}

// Split across sibling files to keep this file under the 1500-line cap
// (Phase 9). Each `include!`d file is a self-contained
// `impl<F: FileSystem + 'static> TaskScheduler<F> { ... }` block for the same
// type in the same module.
include!("scheduler/support.rs");

impl<F: FileSystem + 'static> TaskScheduler<F> {
    pub async fn execute_all(&self) -> Result<()> {
        debug!(
            "execute_all: task_count={} max_parallel={}",
            self.tasks.len(),
            self.max_parallel
        );
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
}

include!("scheduler/task_execution.rs");

#[path = "scheduler_tests_a.rs"]
mod tests_a;
#[path = "scheduler_tests_b.rs"]
mod tests_b;
