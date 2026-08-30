use chrono::TimeZone;
use colored::Colorize;
use comfy_table::{Cell, CellAlignment, Table, modifiers::UTF8_ROUND_CORNERS, presets::UTF8_FULL};
use eyre::Result;
use std::sync::Arc;

use crate::executor::StateManager;
use crate::ports::StateStore;

/// Show execution statistics
#[derive(Debug, clap::Parser)]
#[command(name = "stats")]
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

        let success_rate = if stats.total_runs > 0 {
            (stats.successful_runs as f64 / stats.total_runs as f64) * 100.0
        } else {
            0.0
        };

        let mut table = Table::new();
        table
            .load_preset(UTF8_FULL)
            .apply_modifier(UTF8_ROUND_CORNERS)
            .set_header(vec![
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
                format_percentage(success_rate)
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

        println!("{}", table);

        // Show top tasks
        let task_stats = store.get_all_task_stats(Some(self.limit))?;

        if !task_stats.is_empty() {
            println!("\n{}", format!("Top {} Tasks by Execution Count", self.limit).bold());

            let mut task_table = Table::new();
            task_table
                .load_preset(UTF8_FULL)
                .apply_modifier(UTF8_ROUND_CORNERS)
                .set_header(vec![
                    Cell::new("Project").set_alignment(CellAlignment::Left),
                    Cell::new("Task").set_alignment(CellAlignment::Left),
                    Cell::new("Total").set_alignment(CellAlignment::Right),
                    Cell::new("Success").set_alignment(CellAlignment::Right),
                    Cell::new("Failed").set_alignment(CellAlignment::Right),
                    Cell::new("Success Rate").set_alignment(CellAlignment::Right),
                    Cell::new("Avg Duration").set_alignment(CellAlignment::Right),
                ]);

            for task in &task_stats {
                let total_attempted = task.successful_executions + task.failed_executions;
                let success_rate = if total_attempted > 0 {
                    (task.successful_executions as f64 / total_attempted as f64) * 100.0
                } else {
                    0.0
                };

                task_table.add_row(vec![
                    Cell::new(&task.project_name).set_alignment(CellAlignment::Left),
                    Cell::new(&task.task_name).set_alignment(CellAlignment::Left),
                    Cell::new(task.total_executions.to_string()).set_alignment(CellAlignment::Right),
                    Cell::new(task.successful_executions.to_string()).set_alignment(CellAlignment::Right),
                    Cell::new(task.failed_executions.to_string()).set_alignment(CellAlignment::Right),
                    Cell::new(format_percentage(success_rate)).set_alignment(CellAlignment::Right),
                    Cell::new(
                        task.avg_duration_seconds
                            .map(format_duration)
                            .unwrap_or_else(|| "-".to_string()),
                    )
                    .set_alignment(CellAlignment::Right),
                ]);
            }

            println!("{}", task_table);
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
            let total_attempted = stat.successful_executions + stat.failed_executions;
            let success_rate = if total_attempted > 0 {
                (stat.successful_executions as f64 / total_attempted as f64) * 100.0
            } else {
                0.0
            };

            let mut table = Table::new();
            table
                .load_preset(UTF8_FULL)
                .apply_modifier(UTF8_ROUND_CORNERS)
                .set_header(vec![
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
                    format_percentage(success_rate)
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
                        .map(|t| {
                            let dt = chrono::Local.timestamp_opt(t as i64, 0).unwrap();
                            dt.format("%Y-%m-%d %H:%M:%S").to_string()
                        })
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
            table
                .load_preset(UTF8_FULL)
                .apply_modifier(UTF8_ROUND_CORNERS)
                .set_header(vec![
                    Cell::new("Project").set_alignment(CellAlignment::Left),
                    Cell::new("Total").set_alignment(CellAlignment::Right),
                    Cell::new("Success").set_alignment(CellAlignment::Right),
                    Cell::new("Failed").set_alignment(CellAlignment::Right),
                    Cell::new("Success Rate").set_alignment(CellAlignment::Right),
                    Cell::new("Avg Duration").set_alignment(CellAlignment::Right),
                ]);

            for stat in &stats {
                let total_attempted = stat.successful_executions + stat.failed_executions;
                let success_rate = if total_attempted > 0 {
                    (stat.successful_executions as f64 / total_attempted as f64) * 100.0
                } else {
                    0.0
                };

                table.add_row(vec![
                    Cell::new(&stat.project_name).set_alignment(CellAlignment::Left),
                    Cell::new(stat.total_executions.to_string()).set_alignment(CellAlignment::Right),
                    Cell::new(stat.successful_executions.to_string()).set_alignment(CellAlignment::Right),
                    Cell::new(stat.failed_executions.to_string()).set_alignment(CellAlignment::Right),
                    Cell::new(format_percentage(success_rate)).set_alignment(CellAlignment::Right),
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

fn format_duration(duration: f64) -> String {
    if duration < 1.0 {
        format!("{:.0}ms", duration * 1000.0)
    } else if duration < 60.0 {
        format!("{:.1}s", duration)
    } else if duration < 3600.0 {
        let minutes = (duration / 60.0) as u64;
        let seconds = (duration % 60.0) as u64;
        format!("{}m{}s", minutes, seconds)
    } else {
        let hours = (duration / 3600.0) as u64;
        let minutes = ((duration % 3600.0) / 60.0) as u64;
        format!("{}h{}m", hours, minutes)
    }
}

fn format_size(size: u64) -> String {
    if size < 1024 {
        format!("{} B", size)
    } else if size < 1024 * 1024 {
        format!("{:.1} KB", size as f64 / 1024.0)
    } else if size < 1024 * 1024 * 1024 {
        format!("{:.1} MB", size as f64 / (1024.0 * 1024.0))
    } else {
        format!("{:.2} GB", size as f64 / (1024.0 * 1024.0 * 1024.0))
    }
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::executor::state::{RunMetadata, RunStatus, TaskStatus};
    use crate::ports::MemoryStateStore;
    use std::path::PathBuf;

    fn create_test_store_with_data() -> Arc<MemoryStateStore> {
        let store = MemoryStateStore::new();

        // Add runs with tasks
        let metadata1 = RunMetadata::minimal(
            Some(PathBuf::from("/test/project1/otto.yml")),
            "abc123".to_string(),
            1700000000,
        );
        let run_id1 = store.record_run_start(&metadata1).unwrap();
        store
            .record_run_complete(run_id1, RunStatus::Success, Some(1024))
            .unwrap();

        let task_id1 = store
            .record_task_start(run_id1, "build", Some("hash1"), None, None, None)
            .unwrap();
        store.record_task_complete(task_id1, 0, TaskStatus::Completed).unwrap();

        let task_id2 = store
            .record_task_start(run_id1, "test", Some("hash2"), None, None, None)
            .unwrap();
        store.record_task_complete(task_id2, 0, TaskStatus::Completed).unwrap();

        // Second run - failed
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

        // Third run - different project
        let metadata3 = RunMetadata::minimal(
            Some(PathBuf::from("/test/project2/otto.yml")),
            "def456".to_string(),
            1700002000,
        );
        let run_id3 = store.record_run_start(&metadata3).unwrap();
        store
            .record_run_complete(run_id3, RunStatus::Success, Some(512))
            .unwrap();

        let task_id4 = store
            .record_task_start(run_id3, "deploy", Some("hash3"), None, None, None)
            .unwrap();
        store.record_task_complete(task_id4, 0, TaskStatus::Completed).unwrap();

        Arc::new(store)
    }

    fn create_empty_store() -> Arc<MemoryStateStore> {
        Arc::new(MemoryStateStore::new())
    }

    #[test]
    fn test_format_duration() {
        assert_eq!(format_duration(0.5), "500ms");
        assert_eq!(format_duration(1.5), "1.5s");
        assert_eq!(format_duration(65.0), "1m5s");
        assert_eq!(format_duration(3665.0), "1h1m");
    }

    #[test]
    fn test_format_size() {
        assert_eq!(format_size(500), "500 B");
        assert_eq!(format_size(1536), "1.5 KB");
        assert_eq!(format_size(1572864), "1.5 MB");
        assert_eq!(format_size(1610612736), "1.50 GB");
    }

    #[test]
    fn test_format_percentage() {
        assert_eq!(format_percentage(75.5), "75.5%");
        assert_eq!(format_percentage(100.0), "100.0%");
        assert_eq!(format_percentage(0.0), "0.0%");
    }

    #[test]
    fn test_format_task_status() {
        let completed = format_task_status(&TaskStatus::Completed);
        let failed = format_task_status(&TaskStatus::Failed);
        let running = format_task_status(&TaskStatus::Running);
        let skipped = format_task_status(&TaskStatus::Skipped);
        let pending = format_task_status(&TaskStatus::Pending);

        assert!(completed.contains("Completed"));
        assert!(failed.contains("Failed"));
        assert!(running.contains("Running"));
        assert!(skipped.contains("Skipped"));
        assert!(pending.contains("Pending"));
    }

    #[test]
    fn test_execute_with_empty_store() {
        let store = create_empty_store();
        let cmd = StatsCommand {
            task_name: None,
            limit: 10,
            json: false,
        };

        let result = cmd.execute_with_store(Some(store));
        assert!(result.is_ok());
    }

    #[test]
    fn test_execute_overall_stats() {
        let store = create_test_store_with_data();
        let cmd = StatsCommand {
            task_name: None,
            limit: 10,
            json: false,
        };

        let result = cmd.execute_with_store(Some(store));
        assert!(result.is_ok());
    }

    #[test]
    fn test_execute_overall_stats_json() {
        let store = create_test_store_with_data();
        let cmd = StatsCommand {
            task_name: None,
            limit: 10,
            json: true,
        };

        let result = cmd.execute_with_store(Some(store));
        assert!(result.is_ok());
    }

    #[test]
    fn test_execute_overall_stats_with_limit() {
        let store = create_test_store_with_data();
        let cmd = StatsCommand {
            task_name: None,
            limit: 1,
            json: false,
        };

        let result = cmd.execute_with_store(Some(store));
        assert!(result.is_ok());
    }

    #[test]
    fn test_execute_task_stats() {
        let store = create_test_store_with_data();
        let cmd = StatsCommand {
            task_name: Some("build".to_string()),
            limit: 10,
            json: false,
        };

        let result = cmd.execute_with_store(Some(store));
        assert!(result.is_ok());
    }

    #[test]
    fn test_execute_task_stats_json() {
        let store = create_test_store_with_data();
        let cmd = StatsCommand {
            task_name: Some("build".to_string()),
            limit: 10,
            json: true,
        };

        let result = cmd.execute_with_store(Some(store));
        assert!(result.is_ok());
    }

    #[test]
    fn test_execute_task_stats_nonexistent() {
        let store = create_test_store_with_data();
        let cmd = StatsCommand {
            task_name: Some("nonexistent".to_string()),
            limit: 10,
            json: false,
        };

        let result = cmd.execute_with_store(Some(store));
        assert!(result.is_ok());
    }

    #[test]
    fn test_execute_task_stats_empty_store() {
        let store = create_empty_store();
        let cmd = StatsCommand {
            task_name: Some("build".to_string()),
            limit: 10,
            json: false,
        };

        let result = cmd.execute_with_store(Some(store));
        assert!(result.is_ok());
    }

    #[test]
    fn test_execute_task_stats_single_project() {
        let store = Arc::new(MemoryStateStore::new());

        // Only one project
        let metadata = RunMetadata::minimal(
            Some(PathBuf::from("/test/single/otto.yml")),
            "single123".to_string(),
            1700000000,
        );
        let run_id = store.record_run_start(&metadata).unwrap();
        store
            .record_run_complete(run_id, RunStatus::Success, Some(1024))
            .unwrap();

        let task_id = store
            .record_task_start(run_id, "build", Some("hash1"), None, None, None)
            .unwrap();
        store.record_task_complete(task_id, 0, TaskStatus::Completed).unwrap();

        let cmd = StatsCommand {
            task_name: Some("build".to_string()),
            limit: 10,
            json: false,
        };

        let result = cmd.execute_with_store(Some(store));
        assert!(result.is_ok());
    }

    #[test]
    fn test_execute_task_stats_multiple_projects() {
        let store = create_test_store_with_data();
        let cmd = StatsCommand {
            task_name: Some("build".to_string()),
            limit: 10,
            json: false,
        };

        let result = cmd.execute_with_store(Some(store));
        assert!(result.is_ok());
    }

    #[test]
    fn test_show_overall_stats_directly() {
        let store = create_test_store_with_data();
        let cmd = StatsCommand {
            task_name: None,
            limit: 10,
            json: false,
        };

        let result = cmd.show_overall_stats(store.as_ref());
        assert!(result.is_ok());
    }

    #[test]
    fn test_show_task_stats_directly() {
        let store = create_test_store_with_data();
        let cmd = StatsCommand {
            task_name: None,
            limit: 10,
            json: false,
        };

        let result = cmd.show_task_stats(store.as_ref(), "build");
        assert!(result.is_ok());
    }

    #[test]
    fn test_show_task_stats_deploy() {
        let store = create_test_store_with_data();
        let cmd = StatsCommand {
            task_name: None,
            limit: 10,
            json: false,
        };

        let result = cmd.show_task_stats(store.as_ref(), "deploy");
        assert!(result.is_ok());
    }
}
