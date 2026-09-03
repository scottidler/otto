use chrono::{DateTime, Local, TimeZone, Utc};
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
#[command(name = "History")]
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
    // An out-of-range or ambiguous local timestamp falls back to the epoch
    // rather than panicking, matching the pattern already established in
    // clean.rs for the same class of "cosmetic display" timestamp.
    let dt = Local
        .timestamp_opt(timestamp as i64, 0)
        .single()
        .unwrap_or_else(|| DateTime::<Local>::from(DateTime::<Utc>::MIN_UTC));
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

#[path = "history_tests.rs"]
mod tests;
