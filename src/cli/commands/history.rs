use chrono::{Local, TimeZone};
use colored::Colorize;
use console::measure_text_width;
use eyre::Result;
use std::sync::Arc;

use crate::executor::{RunStatus, StateManager};
use crate::ports::StateStore;

fn display_width(s: &str) -> usize {
    measure_text_width(s)
}

/// Pad string to exact width (left-align)
fn pad_left(s: &str, width: usize) -> String {
    let w = display_width(s);
    if w >= width {
        s.to_string()
    } else {
        format!("{}{}", s, " ".repeat(width - w))
    }
}

/// Pad string to exact width (right-align)
fn pad_right(s: &str, width: usize) -> String {
    let w = display_width(s);
    if w >= width {
        s.to_string()
    } else {
        format!("{}{}", " ".repeat(width - w), s)
    }
}

/// Center within a field
fn pad_center(s: &str, width: usize) -> String {
    let w = display_width(s);
    if w >= width {
        s.to_string()
    } else {
        let total = width - w;
        let left = total / 2;
        let right = total - left;
        format!("{}{}{}", " ".repeat(left), s, " ".repeat(right))
    }
}

/// The run statuses `--status` can filter on.
///
/// A `ValueEnum`, not a free string: `--status FAILED` and `--status bogus`
/// both used to fall through a `_ => None` arm and print the full unfiltered
/// list, so a typo looked exactly like a filter that matched everything.
#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
#[clap(rename_all = "kebab-case")]
pub enum StatusFilter {
    Success,
    Failed,
    Running,
}

impl From<StatusFilter> for RunStatus {
    fn from(filter: StatusFilter) -> Self {
        match filter {
            StatusFilter::Success => RunStatus::Success,
            StatusFilter::Failed => RunStatus::Failed,
            StatusFilter::Running => RunStatus::Running,
        }
    }
}

impl StatusFilter {
    /// Parse a status name, ignoring case, naming the accepted values on failure.
    pub fn parse(value: &str) -> Result<Self> {
        <Self as clap::ValueEnum>::from_str(value, true)
            .map_err(|_| eyre::eyre!("invalid status '{value}'; expected one of: success, failed, running"))
    }
}

/// Show execution history
#[derive(Debug, clap::Parser)]
#[command(name = "history")]
pub struct HistoryCommand {
    /// Show history for a specific task
    #[arg(value_name = "TASK")]
    pub task_name: Option<String>,

    /// Limit number of results
    #[arg(short = 'n', long, default_value = "20")]
    pub limit: usize,

    /// Filter by status (success, failed, running)
    #[arg(short, long, value_enum, ignore_case = true)]
    pub status: Option<StatusFilter>,

    /// Filter by project hash
    #[arg(short, long)]
    pub project: Option<String>,

    /// Output as JSON
    #[arg(long)]
    pub json: bool,
}

impl HistoryCommand {
    pub fn execute(&self) -> Result<()> {
        self.execute_with_store(None)
    }

    /// Execute with an optional injected StateStore (for testing)
    pub fn execute_with_store(&self, store: Option<Arc<dyn StateStore>>) -> Result<()> {
        // Use injected store or create default StateManager
        let store: Arc<dyn StateStore> = match store {
            Some(s) => s,
            None => match StateManager::try_new() {
                Some(m) => Arc::new(m),
                None => {
                    eprintln!("{}", "No history database found. Run otto to create it.".yellow());
                    return Ok(());
                }
            },
        };

        if let Some(ref task_name) = self.task_name {
            self.show_task_history(store.as_ref(), task_name)
        } else {
            self.show_run_history(store.as_ref())
        }
    }

    fn show_run_history(&self, store: &dyn StateStore) -> Result<()> {
        let status_filter = self.status.map(RunStatus::from);

        let runs = store.get_runs_with_filters(status_filter, self.project.as_deref(), self.limit)?;

        if runs.is_empty() {
            println!("{}", "No runs found.".yellow());
            return Ok(());
        }

        if self.json {
            println!("{}", serde_json::to_string_pretty(&runs)?);
            return Ok(());
        }

        let mut rows: Vec<(String, String, String, String, String, String)> = Vec::new();

        for run in &runs {
            let path = run
                .cwd
                .as_ref()
                .and_then(|p| p.to_str())
                .map(|s| {
                    if let Ok(home) = std::env::var("HOME")
                        && s.starts_with(&home)
                    {
                        s.replace(&home, "~")
                    } else {
                        s.to_string()
                    }
                })
                .unwrap_or_else(|| "-".to_string());

            rows.push((
                format_timestamp(run.timestamp),
                format_run_status(&run.status),
                format_duration(run.duration_seconds),
                format_size(run.size_bytes),
                run.user.clone().unwrap_or_else(|| "-".to_string()),
                path,
            ));
        }

        // Calculate max width for each column
        let mut w1 = display_width("Timestamp");
        let mut w2 = display_width("Status");
        let mut w3 = display_width("Duration");
        let mut w4 = display_width("Size");
        let mut w5 = display_width("User");
        let mut w6 = display_width("Path");

        for (c1, c2, c3, c4, c5, c6) in &rows {
            w1 = w1.max(display_width(c1));
            w2 = w2.max(display_width(c2));
            w3 = w3.max(display_width(c3));
            w4 = w4.max(display_width(c4));
            w5 = w5.max(display_width(c5));
            w6 = w6.max(display_width(c6));
        }

        // Print header
        println!();
        println!(
            "{}  {}  {}  {}  {}  {}",
            pad_left("Timestamp", w1).bold(),
            pad_center("Status", w2).bold(),
            pad_right("Duration", w3).bold(),
            pad_right("Size", w4).bold(),
            pad_left("User", w5).bold(),
            pad_left("Path", w6).bold(),
        );

        let total_width = w1 + w2 + w3 + w4 + w5 + w6 + 10;
        println!("{}", "─".repeat(total_width).dimmed());

        // Print rows
        for (c1, c2, c3, c4, c5, c6) in &rows {
            println!(
                "{}  {}  {}  {}  {}  {}",
                pad_left(c1, w1),
                pad_center(c2, w2),
                pad_right(c3, w3),
                pad_right(c4, w4),
                pad_left(c5, w5),
                pad_left(c6, w6),
            );
        }

        println!("\nTotal runs: {}", runs.len());
        Ok(())
    }

    fn show_task_history(&self, store: &dyn StateStore, task_name: &str) -> Result<()> {
        let history = store.get_task_history(task_name, self.limit)?;

        if history.is_empty() {
            println!("{}", format!("No history found for task '{}'.", task_name).yellow());
            return Ok(());
        }

        if self.json {
            println!("{}", serde_json::to_string_pretty(&history)?);
            return Ok(());
        }

        println!("\n{} for task '{}'", "History".bold(), task_name.cyan());

        let mut rows: Vec<(String, String, String, String, String)> = Vec::new();

        for task in &history {
            rows.push((
                task.started_at.map(format_timestamp).unwrap_or_else(|| "-".to_string()),
                format_task_status(&task.status),
                format_duration(task.duration_seconds),
                task.exit_code.map(|c| c.to_string()).unwrap_or_else(|| "-".to_string()),
                task.run_id.to_string(),
            ));
        }

        let mut w1 = display_width("Timestamp");
        let mut w2 = display_width("Status");
        let mut w3 = display_width("Duration");
        let mut w4 = display_width("Exit Code");
        let mut w5 = display_width("Run ID");

        for (c1, c2, c3, c4, c5) in &rows {
            w1 = w1.max(display_width(c1));
            w2 = w2.max(display_width(c2));
            w3 = w3.max(display_width(c3));
            w4 = w4.max(display_width(c4));
            w5 = w5.max(display_width(c5));
        }

        println!();
        println!(
            "{}  {}  {}  {}  {}",
            pad_left("Timestamp", w1).bold(),
            pad_center("Status", w2).bold(),
            pad_right("Duration", w3).bold(),
            pad_center("Exit Code", w4).bold(),
            pad_right("Run ID", w5).bold(),
        );

        let total_width = w1 + w2 + w3 + w4 + w5 + 8;
        println!("{}", "─".repeat(total_width).dimmed());

        for (c1, c2, c3, c4, c5) in &rows {
            println!(
                "{}  {}  {}  {}  {}",
                pad_left(c1, w1),
                pad_center(c2, w2),
                pad_right(c3, w3),
                pad_center(c4, w4),
                pad_right(c5, w5),
            );
        }

        println!("\nTotal executions: {}", history.len());

        let successful = history
            .iter()
            .filter(|t| matches!(t.status, crate::executor::state::TaskStatus::Completed))
            .count();
        let failed = history
            .iter()
            .filter(|t| matches!(t.status, crate::executor::state::TaskStatus::Failed))
            .count();

        if successful + failed > 0 {
            let success_rate = (successful as f64 / (successful + failed) as f64) * 100.0;
            println!("Success rate: {:.1}%", success_rate);
        }

        Ok(())
    }
}

fn format_timestamp(timestamp: u64) -> String {
    let dt = Local.timestamp_opt(timestamp as i64, 0).unwrap();
    dt.format("%Y-%m-%d %H:%M:%S").to_string()
}

fn format_duration(duration: Option<f64>) -> String {
    match duration {
        Some(d) if d < 1.0 => format!("{:.0}ms", d * 1000.0),
        Some(d) if d < 60.0 => format!("{:.1}s", d),
        Some(d) if d < 3600.0 => {
            let minutes = (d / 60.0) as u64;
            let seconds = (d % 60.0) as u64;
            format!("{}m{}s", minutes, seconds)
        }
        Some(d) => {
            let hours = (d / 3600.0) as u64;
            let minutes = ((d % 3600.0) / 60.0) as u64;
            format!("{}h{}m", hours, minutes)
        }
        None => "-".to_string(),
    }
}

fn format_size(size: Option<u64>) -> String {
    match size {
        Some(s) if s < 1024 => format!("{} B", s),
        Some(s) if s < 1024 * 1024 => format!("{:.1} KB", s as f64 / 1024.0),
        Some(s) if s < 1024 * 1024 * 1024 => format!("{:.1} MB", s as f64 / (1024.0 * 1024.0)),
        Some(s) => format!("{:.2} GB", s as f64 / (1024.0 * 1024.0 * 1024.0)),
        None => "-".to_string(),
    }
}

fn format_run_status(status: &RunStatus) -> String {
    match status {
        RunStatus::Success => "✓".green().to_string(),
        RunStatus::Failed => "✗".red().to_string(),
        RunStatus::Running => "⋯".yellow().to_string(),
    }
}

fn format_task_status(status: &crate::executor::state::TaskStatus) -> String {
    use crate::executor::state::TaskStatus;
    match status {
        TaskStatus::Completed => "✓".green().to_string(),
        TaskStatus::Failed => "✗".red().to_string(),
        TaskStatus::Running => "⋯".yellow().to_string(),
        TaskStatus::Skipped => "○".blue().to_string(),
        TaskStatus::Pending => "·".dimmed().to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::executor::state::{RunMetadata, TaskStatus};
    use crate::ports::MemoryStateStore;
    use std::path::PathBuf;

    fn create_test_store_with_runs() -> Arc<MemoryStateStore> {
        let store = MemoryStateStore::new();

        // Add some test runs
        let metadata1 = RunMetadata::minimal(
            Some(PathBuf::from("/test/project1/otto.yml")),
            "abc123".to_string(),
            1700000000,
        );
        let run_id1 = store.record_run_start(&metadata1).unwrap();
        store
            .record_run_complete(run_id1, RunStatus::Success, Some(1024))
            .unwrap();

        // Add tasks to the run
        let task_id1 = store
            .record_task_start(run_id1, "build", Some("hash1"), None, None, None)
            .unwrap();
        store.record_task_complete(task_id1, 0, TaskStatus::Completed).unwrap();

        let task_id2 = store
            .record_task_start(run_id1, "test", Some("hash2"), None, None, None)
            .unwrap();
        store.record_task_complete(task_id2, 0, TaskStatus::Completed).unwrap();

        // Add a second run
        let metadata2 = RunMetadata::minimal(
            Some(PathBuf::from("/test/project1/otto.yml")),
            "abc123".to_string(),
            1700001000,
        );
        let run_id2 = store.record_run_start(&metadata2).unwrap();
        store
            .record_run_complete(run_id2, RunStatus::Failed, Some(2048))
            .unwrap();

        let task_id3 = store
            .record_task_start(run_id2, "build", Some("hash1"), None, None, None)
            .unwrap();
        store.record_task_complete(task_id3, 1, TaskStatus::Failed).unwrap();

        Arc::new(store)
    }

    fn create_empty_store() -> Arc<MemoryStateStore> {
        Arc::new(MemoryStateStore::new())
    }

    #[test]
    fn test_format_duration() {
        assert_eq!(format_duration(Some(0.5)), "500ms");
        assert_eq!(format_duration(Some(1.5)), "1.5s");
        assert_eq!(format_duration(Some(65.0)), "1m5s");
        assert_eq!(format_duration(Some(3665.0)), "1h1m");
        assert_eq!(format_duration(None), "-");
    }

    #[test]
    fn test_format_size() {
        assert_eq!(format_size(Some(500)), "500 B");
        assert_eq!(format_size(Some(1536)), "1.5 KB");
        assert_eq!(format_size(Some(1572864)), "1.5 MB");
        assert_eq!(format_size(Some(1610612736)), "1.50 GB");
        assert_eq!(format_size(None), "-");
    }

    #[test]
    fn test_format_timestamp() {
        let timestamp = 1234567890;
        let result = format_timestamp(timestamp);
        assert!(result.contains("-"));
        assert!(result.contains(":"));
    }

    #[test]
    fn test_format_run_status() {
        let success = format_run_status(&RunStatus::Success);
        let failed = format_run_status(&RunStatus::Failed);
        let running = format_run_status(&RunStatus::Running);

        assert!(success.contains("✓") || success.contains("green"));
        assert!(failed.contains("✗") || failed.contains("red"));
        assert!(running.contains("⋯") || running.contains("yellow"));
    }

    #[test]
    fn test_format_task_status() {
        let completed = format_task_status(&TaskStatus::Completed);
        let failed = format_task_status(&TaskStatus::Failed);
        let running = format_task_status(&TaskStatus::Running);
        let skipped = format_task_status(&TaskStatus::Skipped);
        let pending = format_task_status(&TaskStatus::Pending);

        assert!(!completed.is_empty());
        assert!(!failed.is_empty());
        assert!(!running.is_empty());
        assert!(!skipped.is_empty());
        assert!(!pending.is_empty());
    }

    #[test]
    fn test_display_width() {
        assert_eq!(display_width("hello"), 5);
        assert_eq!(display_width(""), 0);
        assert_eq!(display_width("test string"), 11);
    }

    #[test]
    fn test_pad_left() {
        assert_eq!(pad_left("hi", 5), "hi   ");
        assert_eq!(pad_left("hello", 5), "hello");
        assert_eq!(pad_left("toolong", 3), "toolong");
    }

    #[test]
    fn test_pad_right() {
        assert_eq!(pad_right("hi", 5), "   hi");
        assert_eq!(pad_right("hello", 5), "hello");
        assert_eq!(pad_right("toolong", 3), "toolong");
    }

    #[test]
    fn test_pad_center() {
        assert_eq!(pad_center("hi", 6), "  hi  ");
        assert_eq!(pad_center("hello", 5), "hello");
        assert_eq!(pad_center("x", 4), " x  ");
    }

    #[test]
    fn test_execute_with_empty_store() {
        let store = create_empty_store();
        let cmd = HistoryCommand {
            task_name: None,
            limit: 20,
            status: None,
            project: None,
            json: false,
        };

        let result = cmd.execute_with_store(Some(store));
        assert!(result.is_ok());
    }

    #[test]
    fn test_execute_with_runs() {
        let store = create_test_store_with_runs();
        let cmd = HistoryCommand {
            task_name: None,
            limit: 20,
            status: None,
            project: None,
            json: false,
        };

        let result = cmd.execute_with_store(Some(store));
        assert!(result.is_ok());
    }

    #[test]
    fn test_execute_with_json_output() {
        let store = create_test_store_with_runs();
        let cmd = HistoryCommand {
            task_name: None,
            limit: 20,
            status: None,
            project: None,
            json: true,
        };

        let result = cmd.execute_with_store(Some(store));
        assert!(result.is_ok());
    }

    #[test]
    fn test_execute_with_status_filter_success() {
        let store = create_test_store_with_runs();
        let cmd = HistoryCommand {
            task_name: None,
            limit: 20,
            status: Some(StatusFilter::Success),
            project: None,
            json: false,
        };

        let result = cmd.execute_with_store(Some(store));
        assert!(result.is_ok());
    }

    #[test]
    fn test_execute_with_status_filter_failed() {
        let store = create_test_store_with_runs();
        let cmd = HistoryCommand {
            task_name: None,
            limit: 20,
            status: Some(StatusFilter::Failed),
            project: None,
            json: false,
        };

        let result = cmd.execute_with_store(Some(store));
        assert!(result.is_ok());
    }

    #[test]
    fn test_execute_with_status_filter_running() {
        let store = create_test_store_with_runs();
        let cmd = HistoryCommand {
            task_name: None,
            limit: 20,
            status: Some(StatusFilter::Running),
            project: None,
            json: false,
        };

        let result = cmd.execute_with_store(Some(store));
        assert!(result.is_ok());
    }

    /// Was `test_execute_with_invalid_status_filter`, which asserted that
    /// `--status invalid` was accepted and quietly ignored. That is the defect;
    /// the assertion is inverted rather than deleted.
    #[test]
    fn an_invalid_status_is_rejected_at_parse_time() {
        use clap::Parser as _;

        let err = HistoryCommand::try_parse_from(["history", "--status", "invalid"])
            .expect_err("'invalid' is not a status")
            .to_string();
        assert!(err.contains("invalid value 'invalid'"), "{err}");
    }

    #[test]
    fn a_status_is_accepted_in_any_case() {
        use clap::Parser as _;

        let cmd = HistoryCommand::try_parse_from(["history", "--status", "FAILED"]).expect("case does not matter");
        assert_eq!(cmd.status, Some(StatusFilter::Failed));
    }

    #[test]
    fn status_filter_parse_names_the_accepted_values() {
        assert_eq!(StatusFilter::parse("Running").unwrap(), StatusFilter::Running);
        let err = StatusFilter::parse("bogus")
            .expect_err("bogus is not a status")
            .to_string();
        assert!(err.contains("success, failed, running"), "{err}");
    }

    #[test]
    fn test_execute_with_project_filter() {
        let store = create_test_store_with_runs();
        let cmd = HistoryCommand {
            task_name: None,
            limit: 20,
            status: None,
            project: Some("abc123".to_string()),
            json: false,
        };

        let result = cmd.execute_with_store(Some(store));
        assert!(result.is_ok());
    }

    #[test]
    fn test_execute_with_limit() {
        let store = create_test_store_with_runs();
        let cmd = HistoryCommand {
            task_name: None,
            limit: 1,
            status: None,
            project: None,
            json: false,
        };

        let result = cmd.execute_with_store(Some(store));
        assert!(result.is_ok());
    }

    #[test]
    fn test_execute_task_history() {
        let store = create_test_store_with_runs();
        let cmd = HistoryCommand {
            task_name: Some("build".to_string()),
            limit: 20,
            status: None,
            project: None,
            json: false,
        };

        let result = cmd.execute_with_store(Some(store));
        assert!(result.is_ok());
    }

    #[test]
    fn test_execute_task_history_json() {
        let store = create_test_store_with_runs();
        let cmd = HistoryCommand {
            task_name: Some("build".to_string()),
            limit: 20,
            status: None,
            project: None,
            json: true,
        };

        let result = cmd.execute_with_store(Some(store));
        assert!(result.is_ok());
    }

    #[test]
    fn test_execute_task_history_nonexistent() {
        let store = create_test_store_with_runs();
        let cmd = HistoryCommand {
            task_name: Some("nonexistent".to_string()),
            limit: 20,
            status: None,
            project: None,
            json: false,
        };

        let result = cmd.execute_with_store(Some(store));
        assert!(result.is_ok());
    }

    #[test]
    fn test_execute_task_history_empty_store() {
        let store = create_empty_store();
        let cmd = HistoryCommand {
            task_name: Some("build".to_string()),
            limit: 20,
            status: None,
            project: None,
            json: false,
        };

        let result = cmd.execute_with_store(Some(store));
        assert!(result.is_ok());
    }

    #[test]
    fn test_show_run_history_directly() {
        let store = create_test_store_with_runs();
        let cmd = HistoryCommand {
            task_name: None,
            limit: 20,
            status: None,
            project: None,
            json: false,
        };

        let result = cmd.show_run_history(store.as_ref());
        assert!(result.is_ok());
    }

    #[test]
    fn test_show_task_history_directly() {
        let store = create_test_store_with_runs();
        let cmd = HistoryCommand {
            task_name: None,
            limit: 20,
            status: None,
            project: None,
            json: false,
        };

        let result = cmd.show_task_history(store.as_ref(), "build");
        assert!(result.is_ok());
    }
}
