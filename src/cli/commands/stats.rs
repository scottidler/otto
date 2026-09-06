use crate::cli::commands::format::{format_duration, format_size, format_timestamp};
use colored::Colorize;
use comfy_table::{Cell, CellAlignment, Table, presets::UTF8_FULL};
use eyre::Result;
use std::sync::Arc;

use crate::executor::{OverallStats, StateManager, TaskStats};
use crate::ports::StateStore;

/// Show execution statistics
#[derive(Debug, clap::Parser)]
#[command(name = "Stats", bin_name = "otto Stats")]
pub struct StatsCommand {
    /// Show stats for a specific task
    #[arg(value_name = "TASK")]
    pub task_name: Option<String>,

    /// Limit number of tasks shown (when showing all tasks)
    #[arg(short = 'n', long, default_value = "10")]
    pub limit: usize,

    /// Output as JSON
    #[arg(long)]
    pub json: bool,
}

impl StatsCommand {
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
                    eprintln!("{}", "No statistics database found. Run otto to create it.".yellow());
                    return Ok(());
                }
            },
        };

        if let Some(ref task_name) = self.task_name {
            self.show_task_stats(store.as_ref(), task_name)
        } else {
            self.show_overall_stats(store.as_ref())
        }
    }

    fn show_overall_stats(&self, store: &dyn StateStore) -> Result<()> {
        let stats = store.get_overall_stats()?;

        if self.json {
            println!("{}", serde_json::to_string_pretty(&stats)?);
            return Ok(());
        }

        // Show overall statistics
        println!("\n{}", "Overall Statistics".bold());
        println!("{}", render_overall_table(&stats));

        // Show top tasks
        let task_stats = store.get_all_task_stats(Some(self.limit))?;

        if !task_stats.is_empty() {
            println!("\n{}", format!("Top {} Tasks by Execution Count", self.limit).bold());
            println!("{}", render_task_stats_table(&task_stats));
        }

        Ok(())
    }

    fn show_task_stats(&self, store: &dyn StateStore, task_name: &str) -> Result<()> {
        let stats = store.get_task_stats(task_name)?;

        if stats.is_empty() {
            println!("{}", format!("No statistics found for task '{}'.", task_name).yellow());
            return Ok(());
        }

        if self.json {
            println!("{}", serde_json::to_string_pretty(&stats)?);
            return Ok(());
        }

        println!("\n{} for task '{}'", "Statistics".bold(), task_name.cyan());

        // If there's only one project, show simplified view
        if stats.len() == 1 {
            let stat = &stats[0];

            let mut table = Table::new();
            table.load_style(UTF8_FULL.with_rounded_corners()).set_header(vec![
                Cell::new("Metric").set_alignment(CellAlignment::Left),
                Cell::new("Value").set_alignment(CellAlignment::Right),
            ]);

            table.add_row(vec![
                Cell::new("Project").set_alignment(CellAlignment::Left),
                Cell::new(&stat.project_name).set_alignment(CellAlignment::Right),
            ]);
            table.add_row(vec![
                Cell::new("Total Executions").set_alignment(CellAlignment::Left),
                Cell::new(stat.total_executions.to_string()).set_alignment(CellAlignment::Right),
            ]);
            table.add_row(vec![
                Cell::new("Successful").set_alignment(CellAlignment::Left),
                Cell::new(format!(
                    "{} ({})",
                    stat.successful_executions,
                    format_success_rate(stat.successful_executions, stat.failed_executions)
                ))
                .set_alignment(CellAlignment::Right),
            ]);
            table.add_row(vec![
                Cell::new("Failed").set_alignment(CellAlignment::Left),
                Cell::new(stat.failed_executions.to_string()).set_alignment(CellAlignment::Right),
            ]);
            table.add_row(vec![
                Cell::new("Skipped").set_alignment(CellAlignment::Left),
                Cell::new(stat.skipped_executions.to_string()).set_alignment(CellAlignment::Right),
            ]);
            table.add_row(vec![
                Cell::new("Average Duration").set_alignment(CellAlignment::Left),
                Cell::new(
                    stat.avg_duration_seconds
                        .map(format_duration)
                        .unwrap_or_else(|| "-".to_string()),
                )
                .set_alignment(CellAlignment::Right),
            ]);
            table.add_row(vec![
                Cell::new("Min Duration").set_alignment(CellAlignment::Left),
                Cell::new(
                    stat.min_duration_seconds
                        .map(format_duration)
                        .unwrap_or_else(|| "-".to_string()),
                )
                .set_alignment(CellAlignment::Right),
            ]);
            table.add_row(vec![
                Cell::new("Max Duration").set_alignment(CellAlignment::Left),
                Cell::new(
                    stat.max_duration_seconds
                        .map(format_duration)
                        .unwrap_or_else(|| "-".to_string()),
                )
                .set_alignment(CellAlignment::Right),
            ]);
            table.add_row(vec![
                Cell::new("Last Executed").set_alignment(CellAlignment::Left),
                Cell::new(
                    stat.last_executed
                        .map(format_timestamp)
                        .unwrap_or_else(|| "-".to_string()),
                )
                .set_alignment(CellAlignment::Right),
            ]);
            table.add_row(vec![
                Cell::new("Last Status").set_alignment(CellAlignment::Left),
                Cell::new(
                    stat.last_status
                        .as_ref()
                        .map(format_task_status)
                        .unwrap_or_else(|| "-".to_string()),
                )
                .set_alignment(CellAlignment::Right),
            ]);

            println!("{}", table);
        } else {
            // Multiple projects - show table view
            let mut table = Table::new();
            table.load_style(UTF8_FULL.with_rounded_corners()).set_header(vec![
                Cell::new("Project").set_alignment(CellAlignment::Left),
                Cell::new("Total").set_alignment(CellAlignment::Right),
                Cell::new("Success").set_alignment(CellAlignment::Right),
                Cell::new("Failed").set_alignment(CellAlignment::Right),
                Cell::new("Success Rate").set_alignment(CellAlignment::Right),
                Cell::new("Avg Duration").set_alignment(CellAlignment::Right),
            ]);

            for stat in &stats {
                table.add_row(vec![
                    Cell::new(&stat.project_name).set_alignment(CellAlignment::Left),
                    Cell::new(stat.total_executions.to_string()).set_alignment(CellAlignment::Right),
                    Cell::new(stat.successful_executions.to_string()).set_alignment(CellAlignment::Right),
                    Cell::new(stat.failed_executions.to_string()).set_alignment(CellAlignment::Right),
                    Cell::new(format_success_rate(stat.successful_executions, stat.failed_executions))
                        .set_alignment(CellAlignment::Right),
                    Cell::new(
                        stat.avg_duration_seconds
                            .map(format_duration)
                            .unwrap_or_else(|| "-".to_string()),
                    )
                    .set_alignment(CellAlignment::Right),
                ]);
            }

            println!("{}", table);
        }

        Ok(())
    }
}

fn render_overall_table(stats: &OverallStats) -> Table {
    let mut table = Table::new();
    table.load_style(UTF8_FULL.with_rounded_corners()).set_header(vec![
        Cell::new("Metric").set_alignment(CellAlignment::Left),
        Cell::new("Value").set_alignment(CellAlignment::Right),
    ]);

    table.add_row(vec![
        Cell::new("Total Runs").set_alignment(CellAlignment::Left),
        Cell::new(stats.total_runs.to_string()).set_alignment(CellAlignment::Right),
    ]);
    table.add_row(vec![
        Cell::new("Successful").set_alignment(CellAlignment::Left),
        Cell::new(format!(
            "{} ({})",
            stats.successful_runs,
            format_success_rate(stats.successful_runs, stats.failed_runs)
        ))
        .set_alignment(CellAlignment::Right),
    ]);
    table.add_row(vec![
        Cell::new("Failed").set_alignment(CellAlignment::Left),
        Cell::new(stats.failed_runs.to_string()).set_alignment(CellAlignment::Right),
    ]);
    table.add_row(vec![
        Cell::new("Running").set_alignment(CellAlignment::Left),
        Cell::new(stats.running_runs.to_string()).set_alignment(CellAlignment::Right),
    ]);
    table.add_row(vec![
        Cell::new("Total Tasks Executed").set_alignment(CellAlignment::Left),
        Cell::new(stats.total_tasks.to_string()).set_alignment(CellAlignment::Right),
    ]);
    table.add_row(vec![
        Cell::new("Total Disk Usage").set_alignment(CellAlignment::Left),
        Cell::new(format_size(stats.total_disk_usage)).set_alignment(CellAlignment::Right),
    ]);
    table.add_row(vec![
        Cell::new("Total Execution Time").set_alignment(CellAlignment::Left),
        Cell::new(format_duration(stats.total_duration_seconds)).set_alignment(CellAlignment::Right),
    ]);

    table
}

fn render_task_stats_table(task_stats: &[TaskStats]) -> Table {
    let mut table = Table::new();
    table.load_style(UTF8_FULL.with_rounded_corners()).set_header(vec![
        Cell::new("Project").set_alignment(CellAlignment::Left),
        Cell::new("Task").set_alignment(CellAlignment::Left),
        Cell::new("Total").set_alignment(CellAlignment::Right),
        Cell::new("Success").set_alignment(CellAlignment::Right),
        Cell::new("Failed").set_alignment(CellAlignment::Right),
        Cell::new("Success Rate").set_alignment(CellAlignment::Right),
        Cell::new("Avg Duration").set_alignment(CellAlignment::Right),
    ]);

    for task in task_stats {
        table.add_row(vec![
            Cell::new(&task.project_name).set_alignment(CellAlignment::Left),
            Cell::new(&task.task_name).set_alignment(CellAlignment::Left),
            Cell::new(task.total_executions.to_string()).set_alignment(CellAlignment::Right),
            Cell::new(task.successful_executions.to_string()).set_alignment(CellAlignment::Right),
            Cell::new(task.failed_executions.to_string()).set_alignment(CellAlignment::Right),
            Cell::new(format_success_rate(task.successful_executions, task.failed_executions))
                .set_alignment(CellAlignment::Right),
            Cell::new(
                task.avg_duration_seconds
                    .map(format_duration)
                    .unwrap_or_else(|| "-".to_string()),
            )
            .set_alignment(CellAlignment::Right),
        ]);
    }

    table
}

/// The one denominator every Success Rate in this command divides by.
///
/// Runs and executions still `Running` can never reach the numerator, so counting
/// them drags the rate toward zero for as long as they sit there: the live store
/// reported 42.7% overall against 100.0% per task off the same rows. With nothing
/// terminal to divide by, the rate is unknown rather than `0.0%`, which reads as
/// "everything failed".
fn format_success_rate(successful: u64, failed: u64) -> String {
    let terminal = successful + failed;
    if terminal == 0 {
        return "n/a".to_string();
    }
    format_percentage((successful as f64 / terminal as f64) * 100.0)
}

fn format_percentage(rate: f64) -> String {
    format!("{:.1}%", rate)
}

fn format_task_status(status: &crate::executor::state::TaskStatus) -> String {
    use crate::executor::state::TaskStatus;
    match status {
        TaskStatus::Completed => "✓ Completed".green().to_string(),
        TaskStatus::Failed => "✗ Failed".red().to_string(),
        TaskStatus::Running => "⋯ Running".yellow().to_string(),
        TaskStatus::Skipped => "○ Skipped".blue().to_string(),
        TaskStatus::Pending => "· Pending".dimmed().to_string(),
    }
}

#[path = "stats_tests.rs"]
mod tests;
