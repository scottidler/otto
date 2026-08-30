use crate::cfg::otto::RetentionSpec;
use crate::cfg::param::Value;
use crate::cli::commands::history::HistoryCommand;
use crate::cli::commands::stats::StatsCommand;
use crate::cli::parser::{Task, ottofile_base_dir};
use crate::cli::{CleanCommand, ConvertCommand, ParseOutcome, Parser};
use crate::executor::{DagVisualizer, TaskScheduler, Workspace};
use eyre::{Report, Result, eyre};
use log::info;
use std::collections::HashMap;
use std::env;
use std::io::IsTerminal;
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
#[derive(Debug, Clone, PartialEq)]
pub struct CleanParams {
    pub keep_days: u64,
    pub dry_run: bool,
    pub project_filter: Option<String>,
}

impl Default for CleanParams {
    fn default() -> Self {
        CleanParams {
            keep_days: 30,
            dry_run: false,
            project_filter: None,
        }
    }
}

/// Extract Clean command parameters from task values.
/// This is a pure function - no I/O, easily testable.
pub fn extract_clean_params(values: &HashMap<String, Value>) -> CleanParams {
    let keep_days = if let Some(Value::Item(s)) = values.get("keep") {
        s.parse::<u64>().unwrap_or(30)
    } else {
        30
    };

    let dry_run = values
        .get("dry-run")
        .and_then(|v| if let Value::Item(s) = v { Some(s == "true") } else { None })
        .unwrap_or(false);

    let project_filter = values
        .get("project")
        .and_then(|v| if let Value::Item(s) = v { Some(s.clone()) } else { None });

    CleanParams {
        keep_days,
        dry_run,
        project_filter,
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

impl Default for HistoryParams {
    fn default() -> Self {
        HistoryParams {
            task_name: None,
            limit: 20,
            status: None,
            project: None,
            json: false,
        }
    }
}

/// Extract History command parameters from task values.
/// This is a pure function - no I/O, easily testable.
pub fn extract_history_params(values: &HashMap<String, Value>) -> HistoryParams {
    let task_name = values
        .get("task")
        .and_then(|v| if let Value::Item(s) = v { Some(s.clone()) } else { None });

    let limit = if let Some(Value::Item(s)) = values.get("limit") {
        s.parse::<usize>().unwrap_or(20)
    } else {
        20
    };

    let status = values
        .get("status")
        .and_then(|v| if let Value::Item(s) = v { Some(s.clone()) } else { None });

    let project = values
        .get("project")
        .and_then(|v| if let Value::Item(s) = v { Some(s.clone()) } else { None });

    let json = values
        .get("json")
        .and_then(|v| if let Value::Item(s) = v { Some(s == "true") } else { None })
        .unwrap_or(false);

    HistoryParams {
        task_name,
        limit,
        status,
        project,
        json,
    }
}

/// Parameters for the Stats command, extracted from task values.
#[derive(Debug, Clone, PartialEq)]
pub struct StatsParams {
    pub task_name: Option<String>,
    pub limit: usize,
    pub json: bool,
}

impl Default for StatsParams {
    fn default() -> Self {
        StatsParams {
            task_name: None,
            limit: 10,
            json: false,
        }
    }
}

/// Extract Stats command parameters from task values.
/// This is a pure function - no I/O, easily testable.
pub fn extract_stats_params(values: &HashMap<String, Value>) -> StatsParams {
    let task_name = values
        .get("task")
        .and_then(|v| if let Value::Item(s) = v { Some(s.clone()) } else { None });

    let limit = if let Some(Value::Item(s)) = values.get("limit") {
        s.parse::<usize>().unwrap_or(10)
    } else {
        10
    };

    let json = values
        .get("json")
        .and_then(|v| if let Value::Item(s) = v { Some(s == "true") } else { None })
        .unwrap_or(false);

    StatsParams { task_name, limit, json }
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
        output: values
            .get("output")
            .and_then(|v| if let Value::Item(s) = v { Some(PathBuf::from(s)) } else { None }),
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
}

/// Extract Upgrade command parameters from task values.
/// This is a pure function - no I/O, easily testable.
pub fn extract_upgrade_params(values: &HashMap<String, Value>) -> UpgradeParams {
    UpgradeParams {
        dry_run: flag_value(values, "dry-run"),
        version: values
            .get("version")
            .and_then(|v| if let Value::Item(s) = v { Some(s.clone()) } else { None }),
        list_versions: flag_value(values, "list-versions"),
        rollback: flag_value(values, "rollback"),
        force: flag_value(values, "force"),
        no_backup: flag_value(values, "no-backup"),
    }
}

/// Read a boolean param the way the parser writes them: `Value::Item("true")`.
fn flag_value(values: &HashMap<String, Value>, name: &str) -> bool {
    values
        .get(name)
        .and_then(|v| if let Value::Item(s) = v { Some(s == "true") } else { None })
        .unwrap_or(false)
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
        Builtin::Graph => DagVisualizer::execute_command(task).await,
        Builtin::History => execute_history_from_task(task),
        Builtin::Stats => execute_stats_from_task(task),
        Builtin::Upgrade => execute_upgrade_from_task(task).await,
    }
}

// ============================================================================
// Pure Functions - Task Filtering
// ============================================================================

/// Filter out built-in commands from a list of tasks.
/// Returns only tasks that should be executed by the scheduler.
/// This is a pure function - no I/O, easily testable.
pub fn filter_execution_tasks(tasks: Vec<Task>) -> Vec<Task> {
    tasks
        .into_iter()
        .filter(|task| !crate::cli::is_builtin(&task.name))
        .collect()
}

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

    // Filter out built-in commands for normal execution using pure function
    let execution_tasks = filter_execution_tasks(tasks);

    if execution_tasks.is_empty() {
        println!("No tasks to execute");
        return Ok(());
    }

    let cwd = env::current_dir()?;
    let root = ottofile_base_dir(ottofile_path.as_deref(), &cwd).to_path_buf();
    let workspace = Workspace::new(root).await?;
    workspace.init().await?;

    let mut execution_context = crate::executor::workspace::ExecutionContext::new();
    execution_context.ottofile = ottofile_path;
    execution_context.hash = hash;

    // Save execution context to run directory
    workspace.save_execution_context(execution_context.clone()).await?;

    // Convert parser tasks to executor tasks
    let executor_tasks = build_executor_tasks(execution_tasks);

    // The scheduler takes ownership; teardown still needs the workspace to close
    // out the run in the database.
    let workspace = Arc::new(workspace);
    let mut scheduler = TaskScheduler::new(executor_tasks, workspace.clone(), execution_context, jobs, false).await?;
    scheduler.set_no_prefix(no_prefix);

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

    // Filter out built-in commands for normal execution using pure function
    let execution_tasks = filter_execution_tasks(tasks);

    if execution_tasks.is_empty() {
        eprintln!("No tasks to execute");
        return Ok(());
    }

    let cwd = env::current_dir()?;
    let root = ottofile_base_dir(ottofile_path.as_deref(), &cwd).to_path_buf();
    let workspace = Workspace::new(root).await?;
    workspace.init().await?;

    let mut execution_context = crate::executor::workspace::ExecutionContext::new();
    execution_context.ottofile = ottofile_path;
    execution_context.hash = hash;

    // Save execution context to run directory
    workspace.save_execution_context(execution_context.clone()).await?;

    let executor_tasks = build_executor_tasks(execution_tasks);

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

    let ctrl_c_pressed = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let ctrl_c_flag = ctrl_c_pressed.clone();

    tokio::spawn(async move {
        if tokio::signal::ctrl_c().await.is_ok() {
            ctrl_c_flag.store(true, std::sync::atomic::Ordering::SeqCst);
        }
    });

    app.set_shutdown_flag(ctrl_c_pressed);

    // Run TUI (blocks until user quits or Ctrl+C)
    let tui_result = app.run(terminal.terminal_mut());

    // Restore explicitly rather than leaving it to the guard's Drop: everything
    // printed from here on has to land on the user's real screen, not on the
    // alternate one that is about to be discarded.
    let still_running = app.running_task_names();
    if let Err(e) = terminal.restore() {
        eprintln!("Warning: Failed to restore terminal: {}", e);
    }

    // Handle TUI errors
    tui_result.map_err(|e| eyre::eyre!("TUI error: {}", e))?;

    // The dashboard is gone, so nothing is watching the run any more. Cancel it and
    // say so. Before this, quitting left the user staring at a bare prompt while
    // children they could no longer see ran to completion, with no way to stop them.
    if !scheduler_handle.is_finished() {
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

    result
}

/// Execute Clean command from a parsed task.
pub async fn execute_clean_from_task(task: &Task) -> Result<(), Report> {
    let params = extract_clean_params(&task.values);

    let clean_cmd = CleanCommand {
        keep_days: params.keep_days,
        keep_last: None,
        keep_failed: None,
        dry_run: params.dry_run,
        project_filter: params.project_filter,
        no_db: false,
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
    // `#[arg(env = "GITHUB_TOKEN")]` only fires when clap parses the args;
    // constructing the command directly has to read the env itself, or the
    // task route silently loses the token and rate-limits.
    upgrade_cmd.github_token = env::var("GITHUB_TOKEN").ok();

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
