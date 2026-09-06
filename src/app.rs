use crate::cfg::otto::RetentionSpec;
use crate::cfg::param::Value;
use crate::cli::commands::graph::{GraphCommand, GraphFormatArg};
use crate::cli::commands::history::HistoryCommand;
use crate::cli::commands::stats::StatsCommand;
use crate::cli::parser::{Task, ottofile_base_dir};
use crate::cli::{CleanCommand, ConvertCommand, ParseOutcome, Parser};
use crate::executor::{TaskScheduler, Workspace};
use clap::ValueEnum as _;
use eyre::{Report, Result, eyre};
use log::{error, info};
use std::collections::HashMap;
use std::env;
use std::io::{IsTerminal, Write};
use std::path::PathBuf;
use std::sync::Arc;

/// Capacity of the TUI's task-message broadcast channel. Sized for the scheduler's
/// status traffic (start/finish/status-change per task), not for task output, which
/// has its own per-task channel.
const TUI_MESSAGE_CHANNEL_CAPACITY: usize = 1_000;

// ============================================================================
// Pure Functions - Parameter Extraction
// ============================================================================

/// Parameters for the Clean command, extracted from task values.
///
/// No `Default`: every field comes from `CleanCommand`'s clap args, so the
/// only honest source for `keep_days`' 30 is that declaration. A `Default`
/// here was a second copy of it that nothing outside its own test read.
#[derive(Debug, Clone, PartialEq)]
pub struct CleanParams {
    pub keep_days: u64,
    pub keep_last: Option<usize>,
    pub keep_failed: Option<u64>,
    pub dry_run: bool,
    pub project_filter: Option<String>,
    pub no_db: bool,
}

/// Extract Clean command parameters from task values.
/// This is a pure function - no I/O, easily testable.
pub fn extract_clean_params(values: &HashMap<String, Value>) -> CleanParams {
    CleanParams {
        keep_days: derived_item(values, "Clean", "keep-days")
            .parse::<u64>()
            .expect("Clean's --keep-days is derived from CleanCommand's u64 arg, which clap parses before binding"),
        keep_last: optional_item(values, "keep-last").map(|s| {
            s.parse::<usize>().expect(
                "Clean's --keep-last is derived from CleanCommand's usize arg, which clap parses before binding",
            )
        }),
        keep_failed: optional_item(values, "keep-failed").map(|s| {
            s.parse::<u64>().expect(
                "Clean's --keep-failed is derived from CleanCommand's u64 arg, which clap parses before binding",
            )
        }),
        dry_run: flag_value(values, "dry-run"),
        project_filter: optional_item(values, "project-filter").map(str::to_string),
        no_db: flag_value(values, "no-db"),
    }
}

/// Parameters for the History command, extracted from task values.
#[derive(Debug, Clone, PartialEq)]
pub struct HistoryParams {
    pub task_name: Option<String>,
    pub limit: usize,
    pub status: Option<String>,
    pub project: Option<String>,
    pub json: bool,
}

/// Extract History command parameters from task values.
/// This is a pure function - no I/O, easily testable.
pub fn extract_history_params(values: &HashMap<String, Value>) -> HistoryParams {
    HistoryParams {
        task_name: optional_item(values, "task-name").map(str::to_string),
        limit: derived_item(values, "History", "limit")
            .parse::<usize>()
            .expect("History's --limit is derived from HistoryCommand's usize arg, which clap parses before binding"),
        status: optional_item(values, "status").map(str::to_string),
        project: optional_item(values, "project").map(str::to_string),
        json: flag_value(values, "json"),
    }
}

/// Parameters for the Stats command, extracted from task values.
#[derive(Debug, Clone, PartialEq)]
pub struct StatsParams {
    pub task_name: Option<String>,
    pub limit: usize,
    pub json: bool,
}

/// Extract Stats command parameters from task values.
/// This is a pure function - no I/O, easily testable.
pub fn extract_stats_params(values: &HashMap<String, Value>) -> StatsParams {
    StatsParams {
        task_name: optional_item(values, "task-name").map(str::to_string),
        limit: derived_item(values, "Stats", "limit")
            .parse::<usize>()
            .expect("Stats' --limit is derived from StatsCommand's usize arg, which clap parses before binding"),
        json: flag_value(values, "json"),
    }
}

/// Parameters for the Graph command, extracted from task values.
#[derive(Debug, Clone, PartialEq)]
pub struct GraphParams {
    pub format: GraphFormatArg,
    pub output: Option<PathBuf>,
}

/// Extract Graph command parameters from task values.
/// This is a pure function - no I/O, easily testable.
pub fn extract_graph_params(values: &HashMap<String, Value>) -> GraphParams {
    let format = derived_item(values, "Graph", "format");
    GraphParams {
        // Typed here, not deep inside the visualizer: the format's value set
        // is `GraphCommand`'s, otto binds the param against those same
        // choices, and an unknown one is therefore impossible rather than
        // quietly rendered as ascii.
        format: GraphFormatArg::from_str(format, true)
            .expect("Graph's --format is bound against GraphCommand's choices, which reject an unknown format"),
        output: optional_item(values, "output").map(PathBuf::from),
    }
}

/// Parameters for the Convert command, extracted from task values.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ConvertParams {
    pub strict: bool,
    pub output: Option<PathBuf>,
}

/// Extract Convert command parameters from task values.
/// This is a pure function - no I/O, easily testable.
pub fn extract_convert_params(values: &HashMap<String, Value>) -> ConvertParams {
    ConvertParams {
        strict: flag_value(values, "strict"),
        output: optional_item(values, "output").map(PathBuf::from),
    }
}

/// Parameters for the Upgrade command, extracted from task values.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct UpgradeParams {
    pub dry_run: bool,
    pub version: Option<String>,
    pub list_versions: bool,
    pub rollback: bool,
    pub force: bool,
    pub no_backup: bool,
    pub backup_dir: Option<PathBuf>,
    pub github_token: Option<String>,
}

/// Extract Upgrade command parameters from task values.
/// This is a pure function - no I/O, easily testable.
pub fn extract_upgrade_params(values: &HashMap<String, Value>) -> UpgradeParams {
    UpgradeParams {
        dry_run: flag_value(values, "dry-run"),
        version: optional_item(values, "version").map(str::to_string),
        list_versions: flag_value(values, "list-versions"),
        rollback: flag_value(values, "rollback"),
        force: flag_value(values, "force"),
        no_backup: flag_value(values, "no-backup"),
        backup_dir: optional_item(values, "backup-dir").map(PathBuf::from),
        github_token: optional_item(values, "github-token").map(str::to_string),
    }
}

/// Read a boolean param the way the parser writes them: `Value::Item("true")`.
fn flag_value(values: &HashMap<String, Value>, name: &str) -> bool {
    values
        .get(name)
        .and_then(|v| if let Value::Item(s) = v { Some(s == "true") } else { None })
        .unwrap_or(false)
}

/// A param the user may or may not have given: no `default:`, so an absent
/// value is a real answer and stays `None`.
fn optional_item<'a>(values: &'a HashMap<String, Value>, name: &str) -> Option<&'a str> {
    match values.get(name) {
        Some(Value::Item(value)) => Some(value.as_str()),
        _ => None,
    }
}

/// A param the derivation guarantees is bound.
///
/// Every builtin param comes from its clap `Command`
/// (`cli/parser/meta_tasks.rs`), and one carrying a `default_value` is written
/// onto the task whether or not the user typed it (`process_tasks_with_filter`
/// Phase 3). Missing here means the derivation or the bind broke, which is a
/// bug in otto and not something a user typed - so it says so instead of
/// silently substituting a second copy of the default.
fn derived_item<'a>(values: &'a HashMap<String, Value>, builtin: &str, name: &str) -> &'a str {
    optional_item(values, name).unwrap_or_else(|| {
        panic!("{builtin}'s --{name} is derived from its clap Command, which gives it a default; a bound {builtin} task always carries a value")
    })
}

// ============================================================================
// Pure Functions - Builtin Dispatch
// ============================================================================

/// The built-in commands a run can resolve to.
///
/// One table, consulted by both output modes. The TUI path used to dispatch
/// none of these, so `otto --tui Graph` under a TTY printed "No tasks to
/// execute"; the terminal path dispatched four of the six, so `otto Convert`
/// behind any global flag did the same.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Builtin {
    Clean,
    Convert,
    Graph,
    History,
    Stats,
    Upgrade,
}

impl Builtin {
    /// The task name this builtin is invoked as.
    pub fn task_name(&self) -> &'static str {
        match self {
            Builtin::Clean => "Clean",
            Builtin::Convert => "Convert",
            Builtin::Graph => "Graph",
            Builtin::History => "History",
            Builtin::Stats => "Stats",
            Builtin::Upgrade => "Upgrade",
        }
    }

    /// Every builtin, in dispatch order.
    pub fn all() -> [Builtin; 6] {
        [
            Builtin::Clean,
            Builtin::Convert,
            Builtin::Graph,
            Builtin::History,
            Builtin::Stats,
            Builtin::Upgrade,
        ]
    }
}

/// The builtin this task list invokes, if any.
/// This is a pure function - no I/O, easily testable.
pub fn find_builtin(tasks: &[Task]) -> Option<(Builtin, &Task)> {
    Builtin::all().into_iter().find_map(|builtin| {
        find_tasks_by_name(tasks, builtin.task_name())
            .first()
            .map(|t| (builtin, *t))
    })
}

/// Run a builtin. Shared by the terminal and TUI paths.
pub async fn dispatch_builtin(builtin: Builtin, task: &Task) -> Result<(), Report> {
    match builtin {
        Builtin::Clean => execute_clean_from_task(task).await,
        Builtin::Convert => execute_convert_from_task(task),
        Builtin::Graph => execute_graph_from_task(task),
        Builtin::History => execute_history_from_task(task),
        Builtin::Stats => execute_stats_from_task(task),
        Builtin::Upgrade => execute_upgrade_from_task(task).await,
    }
}

// ============================================================================
// Pure Functions - Task Filtering
// ============================================================================

/// Convert parser tasks into executor tasks.
///
/// One conversion for both output modes: the terminal path and the TUI path
/// each built this list themselves, so a field that only one of them carried
/// (`is_virtual_parent`, once) was a silent behaviour difference between
/// `otto build` and `otto --tui build`.
pub fn build_executor_tasks(tasks: Vec<Task>) -> Vec<crate::executor::Task> {
    tasks.into_iter().map(Into::into).collect()
}

/// Find tasks by name in a task list.
/// Returns all tasks matching the given name (case-sensitive).
/// This is a pure function - no I/O, easily testable.
pub fn find_tasks_by_name<'a>(tasks: &'a [Task], name: &str) -> Vec<&'a Task> {
    tasks.iter().filter(|task| task.name == name).collect()
}

// ============================================================================
// Application Code
// ============================================================================

/// Runtime configuration built from CLI arguments.
/// This provides a validated, ready-to-use configuration for the application.
#[derive(Debug, Clone)]
pub struct RuntimeConfig {
    pub tasks: Vec<Task>,
    pub hash: String,
    pub ottofile_path: Option<PathBuf>,
    pub jobs: usize,
    pub tui_mode: bool,
    /// `--no-prefix`: suppress the `[task]` prefix on terminal output.
    /// See docs/design/2026-08-28-boundary-fixes-and-dynamic-foreach.md Phase 8.
    pub no_prefix: bool,
    pub retention: RetentionSpec,
}

/// What `main` should do once the command line has been parsed.
///
/// `Exit` carries the code for an invocation that already wrote its output
/// (help, version, `--tasks`, `--list-subtasks`) and must run nothing.
pub enum Startup {
    Run(Box<RuntimeConfig>),
    Exit(i32),
}

impl RuntimeConfig {
    /// Build RuntimeConfig from parsed CLI arguments.
    pub fn from_parser(parser: &mut Parser) -> Result<Startup> {
        let plan = match parser.parse()? {
            ParseOutcome::Run(plan) => plan,
            ParseOutcome::Exit(code) => return Ok(Startup::Exit(code)),
        };
        let retention = parser.retention();
        Ok(Startup::Run(Box::new(Self {
            tasks: plan.tasks,
            hash: plan.hash,
            ottofile_path: plan.ottofile,
            jobs: plan.jobs,
            tui_mode: plan.tui_mode,
            no_prefix: plan.no_prefix,
            retention,
        })))
    }
}

/// Main application entry point.
pub async fn run(config: RuntimeConfig) -> Result<()> {
    info!("Running otto with {} tasks", config.tasks.len());

    execute_tasks(
        config.tasks,
        config.hash,
        config.ottofile_path,
        config.jobs,
        config.tui_mode,
        config.no_prefix,
        config.retention,
    )
    .await
}

/// Execute tasks based on configuration.
#[allow(clippy::too_many_arguments)]
pub async fn execute_tasks(
    tasks: Vec<Task>,
    hash: String,
    ottofile_path: Option<PathBuf>,
    jobs: usize,
    tui_mode: bool,
    no_prefix: bool,
    retention: RetentionSpec,
) -> Result<(), Report> {
    if tui_mode {
        // Checked before the TTY fallback and before any task runs: the TUI owns
        // the terminal and a tty task needs it, so there is no run that satisfies
        // both. Loud error, nothing executed.
        let tty_tasks: Vec<&str> = tasks.iter().filter(|t| t.tty).map(|t| t.name.as_str()).collect();
        if !tty_tasks.is_empty() {
            return Err(eyre!(
                "--tui cannot run alongside a tty task; the TUI owns the terminal. \
                 Tasks declaring tty: true in this run: {}. \
                 Drop --tui, or drop tty: true from the task.",
                tty_tasks.join(", ")
            ));
        }

        if !std::io::stdout().is_terminal() {
            eprintln!("Warning: --tui requires a TTY, falling back to standard output");
            return execute_with_terminal_output(tasks, hash, ottofile_path, jobs, no_prefix, retention).await;
        }

        // TUI mode already suppresses all terminal output (suppress_terminal
        // is derived from tui_mode in the scheduler), so no_prefix has nothing
        // to act on here: no prefix is ever printed to a terminal the TUI owns.
        execute_with_tui(tasks, hash, ottofile_path, jobs, retention).await
    } else {
        execute_with_terminal_output(tasks, hash, ottofile_path, jobs, no_prefix, retention).await
    }
}

/// Execute tasks with terminal output (non-TUI mode).
pub async fn execute_with_terminal_output(
    tasks: Vec<Task>,
    hash: String,
    ottofile_path: Option<PathBuf>,
    jobs: usize,
    no_prefix: bool,
    retention: RetentionSpec,
) -> Result<(), Report> {
    if tasks.is_empty() {
        println!("No tasks to execute");
        return Ok(());
    }

    // Built-in commands, from the table both output modes share.
    if let Some((builtin, task)) = find_builtin(&tasks) {
        return dispatch_builtin(builtin, task).await;
    }

    let cwd = crate::executor::workspace::current_dir()?;
    let root = ottofile_base_dir(ottofile_path.as_deref(), &cwd).to_path_buf();
    let workspace = Workspace::new(root).await?;
    workspace.init().await?;

    let mut execution_context = crate::executor::workspace::ExecutionContext::new();
    execution_context.ottofile = ottofile_path;
    execution_context.hash = hash;

    // Save execution context to run directory
    workspace.save_execution_context(execution_context.clone()).await?;

    // Convert parser tasks to executor tasks
    let executor_tasks = build_executor_tasks(tasks);

    // The scheduler takes ownership; teardown still needs the workspace to close
    // out the run in the database.
    let workspace = Arc::new(workspace);
    let mut scheduler = TaskScheduler::new(executor_tasks, workspace.clone(), execution_context, jobs, false).await?;
    scheduler.set_no_prefix(no_prefix);

    // Ctrl+C has to reach the scheduler, not just the process. Without this the
    // terminal's SIGINT default-killed otto outright and `abandon_run` never
    // ran, so a buffered foreach group lost every completed-but-unreplayed
    // block. Taken before `execute_all` borrows the scheduler.
    // Nothing extra to do on either signal here: the plain path has no
    // dashboard flag to set and no terminal to hand back.
    install_stop_handler(scheduler.cancel_signal(), || {}, || {});

    // Execute all tasks, capturing result
    let result = scheduler.execute_all().await;

    // Close the run out in the database before anything else reads it: prune
    // included, since a run left `running` is a run `Clean` cannot age out.
    workspace.record_run_complete_in_db(result.is_ok()).await;

    // Auto-prune runs even if tasks failed — failing CI jobs that never prune
    // are exactly the scenario that fills disks
    if let Ok(otto_home) = crate::executor::layout::resolve_otto_home() {
        crate::executor::pruning::auto_prune(&otto_home, &retention).await;
    }

    result
}

/// Await the first of SIGINT, SIGTERM and SIGHUP; report which one it was.
///
/// All three mean the same thing to a run - stop - and all three are fatal by
/// default, which is the bug: a default disposition kills otto between "children
/// spawned" and "children reaped" and orphans every one of them. Only SIGINT was
/// handled before this, so `kill <otto>` and a closing terminal both left the
/// subtree running. `None` means the signal machinery could not be installed at
/// all, which leaves the default dispositions in place rather than pretending a
/// handler exists.
async fn next_stop_signal() -> Option<(&'static str, i32)> {
    use tokio::signal::unix::{SignalKind, signal};

    let mut terminate = signal(SignalKind::terminate())
        .inspect_err(|e| error!("could not listen for SIGTERM: {e}"))
        .ok()?;
    let mut hangup = signal(SignalKind::hangup())
        .inspect_err(|e| error!("could not listen for SIGHUP: {e}"))
        .ok()?;
    tokio::select! {
        received = tokio::signal::ctrl_c() => received.ok().map(|()| ("SIGINT", libc::SIGINT)),
        received = terminate.recv() => received.map(|()| ("SIGTERM", libc::SIGTERM)),
        received = hangup.recv() => received.map(|()| ("SIGHUP", libc::SIGHUP)),
    }
}

/// Turn the first stop signal into a scheduler cancel, and the second into an
/// immediate exit. Used by both the plain path and the `--tui` path.
///
/// The first signal sets the `CancelSignal`, so the run goes through
/// `abandon_run`: the report channel is drained, completed buffered blocks are
/// replayed in item order, killed children get their log paths printed, and the
/// unstarted items get did-not-start lines. Installing nothing, which is what
/// shipped through v2.0.5, meant the terminal's default SIGINT disposition
/// killed otto between those two states and printed nothing. SIGTERM and SIGHUP
/// are here for the same reason and not by analogy: a `kill` from a supervisor
/// and a terminal hangup each killed otto outright, `abandon_run` never ran, and
/// the children survived because they are not in otto's group.
///
/// **otto does not forward the terminal's signal to task children, by design.**
/// A terminal Ctrl+C, and a hangup, are delivered to the pty's foreground
/// process group and session, but every non-`tty` task child runs in its own
/// session (`task_execution.rs`, `setsid`), so it receives neither and otto's
/// teardown is not raced. They are killed by otto instead, on its own schedule:
/// `abandon_run` signals each recorded process group, SIGTERM then SIGKILL, so
/// the task's whole subtree dies rather than only its direct child. The one
/// exception is a `tty: true` task: it deliberately stays in otto's session and
/// group so it can own the terminal, so it takes the terminal's signal directly,
/// exactly as it would under any other runner, and cancellation signals its pid
/// alone - its own grandchildren are outside the reaping, as the 2026-09-01 doc
/// records.
///
/// The second signal is the escape hatch, and it is not optional: with a handler
/// installed the default disposition is gone, so a teardown wedged on a child
/// that will not die would otherwise be unkillable from the keyboard. It exits
/// 128 plus the signal's number (130, 143, 129) without closing the run out in
/// the database - the run stays `running` and `otto Clean` ages it out, which is
/// the honest record of what happened.
///
/// The ordinary path keeps the normal failure exit: `abandon_run` returns `Err`,
/// so `execute_all` fails, the run is recorded not-ok, and `main` exits
/// non-zero.
///
/// `on_first` runs after the cancel and `before_exit` runs before the
/// second-signal exit, because the `--tui` path has a quit flag to set and a
/// terminal to hand back and the plain path has neither. Everything else about
/// the two paths' signal handling is identical, so it lives here once.
fn install_stop_handler(
    cancel: Arc<crate::executor::scheduler::CancelSignal>,
    on_first: impl FnOnce() + Send + 'static,
    before_exit: impl FnOnce() + Send + 'static,
) {
    tokio::spawn(async move {
        let Some((name, _)) = next_stop_signal().await else {
            return;
        };
        info!("{name} received; cancelling the run");
        cancel.cancel();
        on_first();

        if let Some((name, number)) = next_stop_signal().await {
            // Whatever the caller has to undo comes first: a message printed
            // onto the alternate screen is a message nobody ever sees.
            before_exit();
            eprintln!("otto: second signal ({name}); exiting without finishing teardown");
            std::process::exit(128 + number);
        }
    });
}

/// Execute tasks with TUI mode.
pub async fn execute_with_tui(
    tasks: Vec<Task>,
    hash: String,
    ottofile_path: Option<PathBuf>,
    jobs: usize,
    retention: RetentionSpec,
) -> Result<(), Report> {
    use crate::tui::{TaskPane, TuiApp};

    if tasks.is_empty() {
        eprintln!("No tasks to execute");
        return Ok(());
    }

    // Same builtin table as the terminal path: a builtin is a builtin whether
    // or not the user asked for a dashboard, and it prints to the terminal, so
    // it runs before the TUI takes the screen.
    if let Some((builtin, task)) = find_builtin(&tasks) {
        return dispatch_builtin(builtin, task).await;
    }

    let cwd = crate::executor::workspace::current_dir()?;
    let root = ottofile_base_dir(ottofile_path.as_deref(), &cwd).to_path_buf();
    let workspace = Workspace::new(root).await?;
    workspace.init().await?;

    let mut execution_context = crate::executor::workspace::ExecutionContext::new();
    execution_context.ottofile = ottofile_path;
    execution_context.hash = hash;

    // Save execution context to run directory
    workspace.save_execution_context(execution_context.clone()).await?;

    let executor_tasks = build_executor_tasks(tasks);

    let mut task_streams_map = std::collections::HashMap::new();
    let output_dir = workspace.run().join("tasks");
    for task in &executor_tasks {
        let streams = crate::executor::output::TaskStreams::new(&task.name, &output_dir).await?;
        task_streams_map.insert(task.name.clone(), streams);
    }

    // Initialize the TUI. The guard restores the terminal when it drops, so
    // every `?` below is safe: before this, an error between here and the
    // single restore call left the shell in raw mode on the alternate screen.
    let mut terminal = crate::tui::init_terminal().map_err(|e| eyre::eyre!("Failed to initialize TUI: {}", e))?;

    let mut app = TuiApp::new();

    // Create message broadcast channel for status updates (larger buffer for fast tasks)
    let (message_tx, _) =
        tokio::sync::broadcast::channel::<crate::executor::output::TaskMessage>(TUI_MESSAGE_CHANNEL_CAPACITY);

    for task in &executor_tasks {
        if let Some(streams) = task_streams_map.get(&task.name) {
            let mut pane = TaskPane::new(task.name.clone(), streams.output_tx.clone());
            pane.set_message_channel(message_tx.clone());
            app.layout_mut().add_pane(Box::new(pane));
        }
    }

    // Start scheduler in background with TUI mode enabled
    let workspace = Arc::new(workspace);
    let mut scheduler = TaskScheduler::new(
        executor_tasks,
        workspace.clone(),
        execution_context,
        jobs,
        true, // tui_mode = true
    )
    .await?;

    // Set message channel on scheduler for broadcasting status updates
    scheduler.set_message_channel(message_tx);

    // Pass the pre-created task streams to the scheduler
    scheduler.set_task_streams(task_streams_map);

    // Draw initial TUI state before starting tasks (ensures receivers are ready)
    terminal.terminal_mut().draw(|f| {
        app.layout_mut().render(f, f.area());
    })?;

    // Taken before the scheduler moves into the spawned task: quitting the TUI has
    // to be able to stop the run, not just stop watching it.
    let cancel = scheduler.cancel_signal();
    let scheduler_handle = tokio::spawn(async move { scheduler.execute_all().await });

    // The flag alone is not enough, and a hangup is why. `TuiApp::run` reads it
    // once per loop iteration AFTER drawing, but after SIGHUP every write to the
    // terminal fails EIO, so the draw returns `Err` and the flag is never read.
    // Cancelling directly from the handler is what makes the run stop in that
    // case; the flag is what makes the dashboard quit in the ordinary one.
    let shutdown_requested = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let shutdown_flag = shutdown_requested.clone();
    install_stop_handler(
        cancel.clone(),
        move || shutdown_flag.store(true, std::sync::atomic::Ordering::SeqCst),
        // A second signal exits the process outright, so the guard's `Drop`
        // never runs and the user would keep the alternate screen and raw mode.
        crate::tui::restore_terminal_best_effort,
    );

    app.set_shutdown_flag(shutdown_requested);

    // Run TUI (blocks until user quits or Ctrl+C)
    let tui_result = app.run(terminal.terminal_mut());

    // Restore explicitly rather than leaving it to the guard's Drop: everything
    // printed from here on has to land on the user's real screen, not on the
    // alternate one that is about to be discarded.
    let still_running = app.running_task_names();
    if let Err(e) = terminal.restore() {
        // Not `eprintln!`: after a hangup this write fails EIO too, and
        // `eprintln!` panics on a failed write, which would unwind past the
        // cancel and the run record below. The warning is best effort.
        let _ = writeln!(std::io::stderr(), "Warning: Failed to restore terminal: {e}");
        // A terminal that cannot be restored is gone. ratatui's `Terminal`
        // shows the cursor in its `Drop` and `eprintln!`s when that fails,
        // which on a dead terminal is a panic at the very end of an otherwise
        // clean shutdown: exit 101 instead of 1, measured with the pty master
        // closed. Skipping the drop leaks a handle the process is about to
        // release anyway.
        std::mem::forget(terminal);
    }

    // Handle TUI errors. A hangup is how this path is reached: the terminal's
    // descriptor reports it, or every write fails EIO, `run()` returns `Err`,
    // and propagating that straight out - which is what this used to do -
    // returned BEFORE the cancel below, leaving the scheduler and its whole
    // subtree running with nobody watching. Then it returned before
    // `record_run_complete_in_db`, so a hung-up run stayed `running` in the
    // database until `Clean` aged it out. Cancel, then fall through to the same
    // drain, record and prune as the ordinary path, and propagate at the end.
    // No "dashboard closed" message: the dashboard did not close, the terminal
    // went away, and saying otherwise would name the wrong cause.
    let tui_error = tui_result.err();
    if tui_error.is_some() {
        if !scheduler_handle.is_finished() {
            cancel.cancel();
        }
    } else if !scheduler_handle.is_finished() {
        // The dashboard is gone, so nothing is watching the run any more. Cancel it and
        // say so. Before this, quitting left the user staring at a bare prompt while
        // children they could no longer see ran to completion, with no way to stop them.
        // `CancelSignal::cancel` is idempotent (`scheduler.rs`), so a run the signal
        // handler already cancelled passes through here harmlessly.
        eprintln!("otto: dashboard closed, cancelling the run...");
        cancel.cancel();
        // Naming what is still running is the difference between "otto hung"
        // and "otto is waiting for these": the await below can take as long as
        // the slowest child takes to notice the cancellation.
        if !still_running.is_empty() {
            eprintln!(
                "otto: waiting for {} running task(s) to stop: {}",
                still_running.len(),
                still_running.join(", ")
            );
        }
    }

    // Wait for scheduler to complete
    let result = match scheduler_handle.await {
        Ok(Ok(())) => Ok(()),
        Ok(Err(e)) => Err(e),
        Err(e) => Err(eyre::eyre!("Scheduler panicked: {}", e)),
    };

    // Same teardown as the non-TUI path: a run that ended is a run marked ended.
    workspace.record_run_complete_in_db(result.is_ok()).await;

    // Auto-prune runs even if tasks failed
    if let Ok(otto_home) = crate::executor::layout::resolve_otto_home() {
        crate::executor::pruning::auto_prune(&otto_home, &retention).await;
    }

    if let Some(e) = tui_error {
        return Err(eyre::eyre!("TUI error: {}", e));
    }

    result
}

/// Execute Clean command from a parsed task.
pub async fn execute_clean_from_task(task: &Task) -> Result<(), Report> {
    let params = extract_clean_params(&task.values);

    let clean_cmd = CleanCommand {
        keep_days: params.keep_days,
        keep_last: params.keep_last,
        keep_failed: params.keep_failed,
        dry_run: params.dry_run,
        project_filter: params.project_filter,
        no_db: params.no_db,
        // Not a CLI flag: `#[arg(skip)]` on the field, so it has no meta param
        // either. Only auto-prune sets it.
        quiet: false,
    };
    clean_cmd.execute().await?;

    Ok(())
}

/// Execute History command from a parsed task.
pub fn execute_history_from_task(task: &Task) -> Result<(), Report> {
    let params = extract_history_params(&task.values);

    // The status arrives as a string from the meta task's params; parsing it
    // here is what keeps `otto History --status bogus` an error rather than a
    // full unfiltered listing.
    let status = params
        .status
        .as_deref()
        .map(crate::cli::commands::history::StatusFilter::parse)
        .transpose()?;

    let history_cmd = HistoryCommand {
        task_name: params.task_name,
        limit: params.limit,
        status,
        project: params.project,
        json: params.json,
    };
    history_cmd.execute()?;

    Ok(())
}

/// Execute Stats command from a parsed task.
pub fn execute_stats_from_task(task: &Task) -> Result<(), Report> {
    let params = extract_stats_params(&task.values);

    let stats_cmd = StatsCommand {
        task_name: params.task_name,
        limit: params.limit,
        json: params.json,
    };
    stats_cmd.execute()?;

    Ok(())
}

/// Execute Graph command from a parsed task.
pub fn execute_graph_from_task(task: &Task) -> Result<(), Report> {
    let params = extract_graph_params(&task.values);

    let graph_cmd = GraphCommand {
        format: params.format,
        output: params.output,
    };
    graph_cmd.execute()?;

    Ok(())
}

/// Execute Convert command from a parsed task.
pub fn execute_convert_from_task(task: &Task) -> Result<(), Report> {
    let params = extract_convert_params(&task.values);

    let convert_cmd = ConvertCommand {
        strict: params.strict,
        output: params.output,
    };
    convert_cmd.execute()?;

    Ok(())
}

/// Execute Upgrade command from a parsed task.
pub async fn execute_upgrade_from_task(task: &Task) -> Result<(), Report> {
    use crate::cli::commands::UpgradeCommand;

    let params = extract_upgrade_params(&task.values);

    // Field-by-field on a default, not a struct literal: `releases_url` and
    // `install_target` are private to cli/commands/upgrade.rs so that nothing
    // outside it - including this task route - can redirect where otto downloads
    // and installs a binary from.
    let mut upgrade_cmd = UpgradeCommand::default();
    upgrade_cmd.dry_run = params.dry_run;
    upgrade_cmd.version = params.version;
    upgrade_cmd.list_versions = params.list_versions;
    upgrade_cmd.rollback = params.rollback;
    upgrade_cmd.force = params.force;
    upgrade_cmd.no_backup = params.no_backup;
    upgrade_cmd.backup_dir = params.backup_dir;
    // `#[arg(env = "GITHUB_TOKEN")]` only fires when clap parses the args;
    // constructing the command directly has to read the env itself, or the
    // task route silently loses the token and rate-limits. The flag wins over
    // the environment, same as clap's own precedence.
    upgrade_cmd.github_token = params.github_token.or_else(|| env::var("GITHUB_TOKEN").ok());

    upgrade_cmd.execute().await?;

    Ok(())
}

/// Execute Clean subcommand from CLI args.
pub async fn execute_clean_command(args: &[String]) -> Result<(), Report> {
    use clap::Parser;

    let clean_cmd = CleanCommand::parse_from(args);
    clean_cmd.execute().await?;
    Ok(())
}

/// Execute History subcommand from CLI args.
pub fn execute_history_command(args: &[String]) -> Result<(), Report> {
    use clap::Parser;

    let history_cmd = HistoryCommand::parse_from(args);
    history_cmd.execute()?;
    Ok(())
}

/// Execute Convert subcommand from CLI args.
pub fn execute_convert_command(args: &[String]) -> Result<(), Report> {
    use clap::Parser;

    let convert_cmd = ConvertCommand::parse_from(args);
    convert_cmd.execute()?;
    Ok(())
}

/// Execute Stats subcommand from CLI args.
pub fn execute_stats_command(args: &[String]) -> Result<(), Report> {
    use clap::Parser;

    let stats_cmd = StatsCommand::parse_from(args);
    stats_cmd.execute()?;
    Ok(())
}

/// Execute Upgrade subcommand from CLI args.
pub async fn execute_upgrade_command(args: &[String]) -> Result<(), Report> {
    use crate::cli::commands::UpgradeCommand;
    use clap::Parser;

    let upgrade_cmd = UpgradeCommand::parse_from(args);
    upgrade_cmd.execute().await?;
    Ok(())
}

#[path = "app_tests.rs"]
mod tests;
