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

        if !atty::is(atty::Stream::Stdout) {
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

    // Check for built-in commands using pure function
    let clean_tasks = find_tasks_by_name(&tasks, "Clean");
    if !clean_tasks.is_empty() {
        return execute_clean_from_task(clean_tasks[0]).await;
    }

    let graph_tasks = find_tasks_by_name(&tasks, "Graph");
    if !graph_tasks.is_empty() {
        return DagVisualizer::execute_command(graph_tasks[0]).await;
    }

    let history_tasks = find_tasks_by_name(&tasks, "History");
    if !history_tasks.is_empty() {
        return execute_history_from_task(history_tasks[0]);
    }

    let stats_tasks = find_tasks_by_name(&tasks, "Stats");
    if !stats_tasks.is_empty() {
        return execute_stats_from_task(stats_tasks[0]);
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
    let executor_tasks: Vec<crate::executor::Task> = execution_tasks.into_iter().map(Into::into).collect();

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

    let mut executor_tasks = Vec::new();
    let mut task_streams_map = std::collections::HashMap::new();
    let output_dir = workspace.run().join("tasks");

    for parser_task in execution_tasks {
        let task_name = parser_task.name.clone();

        let streams = crate::executor::output::TaskStreams::new(&task_name, &output_dir).await?;
        task_streams_map.insert(task_name.clone(), streams);

        executor_tasks.push(crate::executor::Task::from(parser_task));
    }

    // Initialize TUI
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
    terminal.draw(|f| {
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
    let tui_result = app.run(&mut terminal);

    // Always restore terminal, even on Ctrl+C or error
    if let Err(e) = crate::tui::restore_terminal(&mut terminal) {
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

    let history_cmd = HistoryCommand {
        task_name: params.task_name,
        limit: params.limit,
        status: params.status,
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_runtime_config_fields() {
        let config = RuntimeConfig {
            tasks: vec![],
            hash: "abc123".to_string(),
            ottofile_path: Some(PathBuf::from("/tmp/otto.yml")),
            jobs: 4,
            tui_mode: false,
            no_prefix: false,
            retention: crate::cfg::otto::RetentionSpec::default(),
        };

        assert_eq!(config.tasks.len(), 0);
        assert_eq!(config.hash, "abc123");
        assert_eq!(config.ottofile_path, Some(PathBuf::from("/tmp/otto.yml")));
        assert_eq!(config.jobs, 4);
        assert!(!config.tui_mode);
        assert!(!config.no_prefix);
        assert_eq!(config.retention, crate::cfg::otto::RetentionSpec::default());
    }

    // =========================================================================
    // CleanParams Tests
    // =========================================================================

    #[test]
    fn test_clean_params_default() {
        let params = CleanParams::default();
        assert_eq!(params.keep_days, 30);
        assert!(!params.dry_run);
        assert_eq!(params.project_filter, None);
    }

    #[test]
    fn test_extract_clean_params_empty() {
        let values = HashMap::new();
        let params = extract_clean_params(&values);
        assert_eq!(params, CleanParams::default());
    }

    #[test]
    fn test_extract_clean_params_with_keep_days() {
        let mut values = HashMap::new();
        values.insert("keep".to_string(), Value::Item("7".to_string()));
        let params = extract_clean_params(&values);
        assert_eq!(params.keep_days, 7);
    }

    #[test]
    fn test_extract_clean_params_with_invalid_keep_days() {
        let mut values = HashMap::new();
        values.insert("keep".to_string(), Value::Item("invalid".to_string()));
        let params = extract_clean_params(&values);
        assert_eq!(params.keep_days, 30); // Falls back to default
    }

    #[test]
    fn test_extract_clean_params_with_dry_run_true() {
        let mut values = HashMap::new();
        values.insert("dry-run".to_string(), Value::Item("true".to_string()));
        let params = extract_clean_params(&values);
        assert!(params.dry_run);
    }

    #[test]
    fn test_extract_clean_params_with_dry_run_false() {
        let mut values = HashMap::new();
        values.insert("dry-run".to_string(), Value::Item("false".to_string()));
        let params = extract_clean_params(&values);
        assert!(!params.dry_run);
    }

    #[test]
    fn test_extract_clean_params_with_project() {
        let mut values = HashMap::new();
        values.insert("project".to_string(), Value::Item("my-project".to_string()));
        let params = extract_clean_params(&values);
        assert_eq!(params.project_filter, Some("my-project".to_string()));
    }

    #[test]
    fn test_extract_clean_params_all_fields() {
        let mut values = HashMap::new();
        values.insert("keep".to_string(), Value::Item("14".to_string()));
        values.insert("dry-run".to_string(), Value::Item("true".to_string()));
        values.insert("project".to_string(), Value::Item("test-project".to_string()));
        let params = extract_clean_params(&values);
        assert_eq!(params.keep_days, 14);
        assert!(params.dry_run);
        assert_eq!(params.project_filter, Some("test-project".to_string()));
    }

    // =========================================================================
    // HistoryParams Tests
    // =========================================================================

    #[test]
    fn test_history_params_default() {
        let params = HistoryParams::default();
        assert_eq!(params.task_name, None);
        assert_eq!(params.limit, 20);
        assert_eq!(params.status, None);
        assert_eq!(params.project, None);
        assert!(!params.json);
    }

    #[test]
    fn test_extract_history_params_empty() {
        let values = HashMap::new();
        let params = extract_history_params(&values);
        assert_eq!(params, HistoryParams::default());
    }

    #[test]
    fn test_extract_history_params_with_task() {
        let mut values = HashMap::new();
        values.insert("task".to_string(), Value::Item("build".to_string()));
        let params = extract_history_params(&values);
        assert_eq!(params.task_name, Some("build".to_string()));
    }

    #[test]
    fn test_extract_history_params_with_limit() {
        let mut values = HashMap::new();
        values.insert("limit".to_string(), Value::Item("50".to_string()));
        let params = extract_history_params(&values);
        assert_eq!(params.limit, 50);
    }

    #[test]
    fn test_extract_history_params_with_invalid_limit() {
        let mut values = HashMap::new();
        values.insert("limit".to_string(), Value::Item("not-a-number".to_string()));
        let params = extract_history_params(&values);
        assert_eq!(params.limit, 20); // Falls back to default
    }

    #[test]
    fn test_extract_history_params_with_status() {
        let mut values = HashMap::new();
        values.insert("status".to_string(), Value::Item("failed".to_string()));
        let params = extract_history_params(&values);
        assert_eq!(params.status, Some("failed".to_string()));
    }

    #[test]
    fn test_extract_history_params_with_project() {
        let mut values = HashMap::new();
        values.insert("project".to_string(), Value::Item("otto".to_string()));
        let params = extract_history_params(&values);
        assert_eq!(params.project, Some("otto".to_string()));
    }

    #[test]
    fn test_extract_history_params_with_json() {
        let mut values = HashMap::new();
        values.insert("json".to_string(), Value::Item("true".to_string()));
        let params = extract_history_params(&values);
        assert!(params.json);
    }

    #[test]
    fn test_extract_history_params_all_fields() {
        let mut values = HashMap::new();
        values.insert("task".to_string(), Value::Item("test".to_string()));
        values.insert("limit".to_string(), Value::Item("100".to_string()));
        values.insert("status".to_string(), Value::Item("passed".to_string()));
        values.insert("project".to_string(), Value::Item("my-proj".to_string()));
        values.insert("json".to_string(), Value::Item("true".to_string()));
        let params = extract_history_params(&values);
        assert_eq!(params.task_name, Some("test".to_string()));
        assert_eq!(params.limit, 100);
        assert_eq!(params.status, Some("passed".to_string()));
        assert_eq!(params.project, Some("my-proj".to_string()));
        assert!(params.json);
    }

    // =========================================================================
    // StatsParams Tests
    // =========================================================================

    #[test]
    fn test_stats_params_default() {
        let params = StatsParams::default();
        assert_eq!(params.task_name, None);
        assert_eq!(params.limit, 10);
        assert!(!params.json);
    }

    #[test]
    fn test_extract_stats_params_empty() {
        let values = HashMap::new();
        let params = extract_stats_params(&values);
        assert_eq!(params, StatsParams::default());
    }

    #[test]
    fn test_extract_stats_params_with_task() {
        let mut values = HashMap::new();
        values.insert("task".to_string(), Value::Item("lint".to_string()));
        let params = extract_stats_params(&values);
        assert_eq!(params.task_name, Some("lint".to_string()));
    }

    #[test]
    fn test_extract_stats_params_with_limit() {
        let mut values = HashMap::new();
        values.insert("limit".to_string(), Value::Item("25".to_string()));
        let params = extract_stats_params(&values);
        assert_eq!(params.limit, 25);
    }

    #[test]
    fn test_extract_stats_params_with_invalid_limit() {
        let mut values = HashMap::new();
        values.insert("limit".to_string(), Value::Item("xyz".to_string()));
        let params = extract_stats_params(&values);
        assert_eq!(params.limit, 10); // Falls back to default
    }

    #[test]
    fn test_extract_stats_params_with_json() {
        let mut values = HashMap::new();
        values.insert("json".to_string(), Value::Item("true".to_string()));
        let params = extract_stats_params(&values);
        assert!(params.json);
    }

    #[test]
    fn test_extract_stats_params_all_fields() {
        let mut values = HashMap::new();
        values.insert("task".to_string(), Value::Item("deploy".to_string()));
        values.insert("limit".to_string(), Value::Item("5".to_string()));
        values.insert("json".to_string(), Value::Item("true".to_string()));
        let params = extract_stats_params(&values);
        assert_eq!(params.task_name, Some("deploy".to_string()));
        assert_eq!(params.limit, 5);
        assert!(params.json);
    }

    // =========================================================================
    // Task Filtering Tests
    // =========================================================================

    fn create_test_task(name: &str) -> Task {
        Task {
            name: name.to_string(),
            task_deps: vec![],
            file_deps: vec![],
            output_deps: vec![],
            envs: HashMap::new(),
            values: HashMap::new(),
            action: String::new(),
            hash: String::new(),
            is_virtual_parent: false,
            serial_group: None,
            serial_index: 0,
            tty: false,
        }
    }

    #[test]
    fn test_filter_execution_tasks_empty() {
        let tasks: Vec<Task> = vec![];
        let filtered = filter_execution_tasks(tasks);
        assert!(filtered.is_empty());
    }

    #[test]
    fn test_filter_execution_tasks_removes_builtins() {
        let tasks = vec![
            create_test_task("build"),
            create_test_task("Clean"),
            create_test_task("test"),
            create_test_task("History"),
            create_test_task("deploy"),
        ];
        let filtered = filter_execution_tasks(tasks);
        assert_eq!(filtered.len(), 3);
        assert_eq!(filtered[0].name, "build");
        assert_eq!(filtered[1].name, "test");
        assert_eq!(filtered[2].name, "deploy");
    }

    #[test]
    fn test_filter_execution_tasks_no_builtins() {
        let tasks = vec![
            create_test_task("build"),
            create_test_task("test"),
            create_test_task("lint"),
        ];
        let filtered = filter_execution_tasks(tasks);
        assert_eq!(filtered.len(), 3);
    }

    #[test]
    fn test_filter_execution_tasks_all_builtins() {
        let tasks = vec![
            create_test_task("Clean"),
            create_test_task("History"),
            create_test_task("Stats"),
            create_test_task("Graph"),
        ];
        let filtered = filter_execution_tasks(tasks);
        assert!(filtered.is_empty());
    }

    #[test]
    fn test_find_tasks_by_name_empty() {
        let tasks: Vec<Task> = vec![];
        let found = find_tasks_by_name(&tasks, "build");
        assert!(found.is_empty());
    }

    #[test]
    fn test_find_tasks_by_name_found() {
        let tasks = vec![
            create_test_task("build"),
            create_test_task("test"),
            create_test_task("build"), // Duplicate
        ];
        let found = find_tasks_by_name(&tasks, "build");
        assert_eq!(found.len(), 2);
    }

    #[test]
    fn test_find_tasks_by_name_not_found() {
        let tasks = vec![create_test_task("build"), create_test_task("test")];
        let found = find_tasks_by_name(&tasks, "deploy");
        assert!(found.is_empty());
    }

    #[test]
    fn test_find_tasks_by_name_case_sensitive() {
        let tasks = vec![create_test_task("Build"), create_test_task("build")];
        let found = find_tasks_by_name(&tasks, "build");
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].name, "build");
    }

    // =========================================================================
    // Integration Tests for Param Structs
    // =========================================================================

    #[test]
    fn test_clean_params_equality() {
        let a = CleanParams {
            keep_days: 30,
            dry_run: false,
            project_filter: None,
        };
        let b = CleanParams::default();
        assert_eq!(a, b);
    }

    #[test]
    fn test_history_params_equality() {
        let a = HistoryParams {
            task_name: Some("test".to_string()),
            limit: 20,
            status: None,
            project: None,
            json: false,
        };
        let b = HistoryParams {
            task_name: Some("test".to_string()),
            ..Default::default()
        };
        assert_eq!(a, b);
    }

    #[test]
    fn test_stats_params_equality() {
        let a = StatsParams {
            task_name: None,
            limit: 10,
            json: false,
        };
        let b = StatsParams::default();
        assert_eq!(a, b);
    }

    #[test]
    fn test_params_clone() {
        let params = CleanParams {
            keep_days: 7,
            dry_run: true,
            project_filter: Some("proj".to_string()),
        };
        let cloned = params.clone();
        assert_eq!(params, cloned);
    }

    #[test]
    fn test_params_debug() {
        let params = StatsParams::default();
        let debug = format!("{:?}", params);
        assert!(debug.contains("StatsParams"));
        assert!(debug.contains("limit"));
    }
}
