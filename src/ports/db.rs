//! Database abstraction for dependency injection
//!
//! This module provides a trait for state storage operations, allowing
//! the real SQLite implementation to be swapped with an in-memory fake for testing.

use eyre::Result;
use std::path::PathBuf;

use std::sync::Arc;

use crate::executor::state::{
    OverallStats, ProjectSummary, Retention, RunAge, RunMetadata, RunRecord, RunStatus, SkipKind, TaskRecord,
    TaskStats, TaskStatus,
};

/// Abstraction for state storage operations
///
/// This trait defines the interface for recording and querying run/task state.
/// Implementations include the real SQLite-backed StateManager and an in-memory
/// fake for testing.
pub trait StateStore: Send + Sync {
    // Recording methods
    fn record_run_start(&self, metadata: &RunMetadata) -> Result<i64>;
    /// Mark a run finished. Keyed on the run id returned by `record_run_start`,
    /// not on its timestamp: two runs can share a second.
    fn record_run_complete(&self, run_id: i64, status: RunStatus, size_bytes: Option<u64>) -> Result<()>;
    fn record_task_start(
        &self,
        run_id: i64,
        task_name: &str,
        script_hash: Option<&str>,
        stdout_path: Option<&PathBuf>,
        stderr_path: Option<&PathBuf>,
        script_path: Option<&PathBuf>,
    ) -> Result<i64>;
    fn record_task_complete(&self, task_id: i64, exit_code: i32, status: TaskStatus) -> Result<()>;
    fn record_task_skipped(
        &self,
        run_id: i64,
        task_name: &str,
        script_hash: Option<&str>,
        skip_reason: Option<&str>,
        skip_kind: Option<SkipKind>,
    ) -> Result<i64>;

    // Query methods
    fn get_task_history(&self, task_name: &str, limit: usize) -> Result<Vec<TaskRecord>>;
    fn get_overall_stats(&self) -> Result<OverallStats>;
    fn get_all_projects(&self) -> Result<Vec<ProjectSummary>>;
    fn get_task_stats(&self, task_name: &str) -> Result<Vec<TaskStats>>;
    fn get_all_task_stats(&self, limit: Option<usize>) -> Result<Vec<TaskStats>>;
    fn get_runs_with_filters(
        &self,
        status_filter: Option<RunStatus>,
        project_filter: Option<&str>,
        limit: usize,
    ) -> Result<Vec<RunRecord>>;

    // Management methods
    fn find_old_runs(
        &self,
        keep_days: u64,
        keep_last: Option<usize>,
        keep_failed_days: Option<u64>,
        project_filter: Option<&str>,
    ) -> Result<Vec<RunRecord>>;
    fn delete_run(&self, run_id: i64, delete_filesystem: bool) -> Result<Option<RunRecord>>;
}

/// Run one blocking `StateStore` call on tokio's blocking pool.
///
/// Every implementation of this trait is synchronous, and the SQLite one takes a
/// mutex and does disk I/O while holding it. Called straight from an async task,
/// it parks a tokio worker thread for the length of a database write.
pub async fn record_blocking<T, F>(store: &Arc<dyn StateStore>, f: F) -> Result<T>
where
    F: FnOnce(&dyn StateStore) -> Result<T> + Send + 'static,
    T: Send + 'static,
{
    let store = Arc::clone(store);
    tokio::task::spawn_blocking(move || f(store.as_ref()))
        .await
        .map_err(|e| eyre::eyre!("State-store task failed to run: {e}"))?
}

/// In-memory state store for testing
///
/// This implementation stores all state in memory, making it suitable for
/// unit tests that need to verify state storage behavior without touching
/// the real database.
#[derive(Debug, Default)]
pub struct MemoryStateStore {
    runs: std::sync::RwLock<Vec<RunRecord>>,
    tasks: std::sync::RwLock<Vec<TaskRecord>>,
    projects: std::sync::RwLock<Vec<ProjectSummary>>,
    next_run_id: std::sync::atomic::AtomicI64,
    next_task_id: std::sync::atomic::AtomicI64,
    next_project_id: std::sync::atomic::AtomicI64,
}

impl MemoryStateStore {
    pub fn new() -> Self {
        Self {
            runs: std::sync::RwLock::new(Vec::new()),
            tasks: std::sync::RwLock::new(Vec::new()),
            projects: std::sync::RwLock::new(Vec::new()),
            next_run_id: std::sync::atomic::AtomicI64::new(1),
            next_task_id: std::sync::atomic::AtomicI64::new(1),
            next_project_id: std::sync::atomic::AtomicI64::new(1),
        }
    }

    fn get_or_create_project(&self, hash: &str, ottofile_path: Option<&PathBuf>) -> i64 {
        let mut projects = self.projects.write().unwrap();

        // Check if project exists
        if let Some(project) = projects.iter().find(|p| p.hash == hash) {
            return project.id;
        }

        // Create new project
        let id = self.next_project_id.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let name = crate::naming::project_name_from(ottofile_path.map(PathBuf::as_path), hash);

        let now = std::time::SystemTime::now()
            .duration_since(std::time::SystemTime::UNIX_EPOCH)
            .unwrap()
            .as_secs();

        projects.push(ProjectSummary {
            id,
            hash: hash.to_string(),
            name,
            ottofile_path: ottofile_path.cloned(),
            run_count: 0,
            last_seen: now,
        });

        id
    }
}

impl StateStore for MemoryStateStore {
    fn record_run_start(&self, metadata: &RunMetadata) -> Result<i64> {
        let project_id = self.get_or_create_project(&metadata.hash, metadata.ottofile.as_ref());

        let run_id = self.next_run_id.fetch_add(1, std::sync::atomic::Ordering::SeqCst);

        let run = RunRecord {
            id: run_id,
            project_id,
            timestamp: metadata.timestamp,
            status: RunStatus::Running,
            duration_seconds: None,
            size_bytes: None,
            ottofile_path: metadata.ottofile.clone(),
            cwd: metadata.cwd.clone(),
            user: metadata.user.clone(),
            hostname: metadata.hostname.clone(),
            args: metadata.args.clone(),
            ended_at: None,
            run_dir: metadata.run_dir.clone(),
        };

        self.runs.write().unwrap().push(run);

        // Update project run count
        let mut projects = self.projects.write().unwrap();
        if let Some(project) = projects.iter_mut().find(|p| p.id == project_id) {
            project.run_count += 1;
            project.last_seen = metadata.timestamp;
        }

        Ok(run_id)
    }

    fn record_run_complete(&self, run_id: i64, status: RunStatus, size_bytes: Option<u64>) -> Result<()> {
        let ended_at = std::time::SystemTime::now()
            .duration_since(std::time::SystemTime::UNIX_EPOCH)
            .unwrap()
            .as_secs();

        let mut runs = self.runs.write().unwrap();
        let Some(run) = runs.iter_mut().find(|r| r.id == run_id) else {
            // Same rule as the SQLite store: completing a run that is not there
            // leaves it `running` forever, so it is an error, not a no-op.
            return Err(eyre::eyre!("No run with id {run_id} to mark complete"));
        };

        run.status = status;
        run.size_bytes = size_bytes;
        run.ended_at = Some(ended_at);
        run.duration_seconds = Some(ended_at.saturating_sub(run.timestamp) as f64);

        Ok(())
    }

    fn record_task_start(
        &self,
        run_id: i64,
        task_name: &str,
        script_hash: Option<&str>,
        stdout_path: Option<&PathBuf>,
        stderr_path: Option<&PathBuf>,
        script_path: Option<&PathBuf>,
    ) -> Result<i64> {
        let task_id = self.next_task_id.fetch_add(1, std::sync::atomic::Ordering::SeqCst);

        let started_at = std::time::SystemTime::now()
            .duration_since(std::time::SystemTime::UNIX_EPOCH)
            .unwrap()
            .as_secs();

        let task = TaskRecord {
            id: task_id,
            run_id,
            name: task_name.to_string(),
            status: TaskStatus::Running,
            script_hash: script_hash.map(String::from),
            exit_code: None,
            started_at: Some(started_at),
            ended_at: None,
            duration_seconds: None,
            stdout_path: stdout_path.cloned(),
            stderr_path: stderr_path.cloned(),
            script_path: script_path.cloned(),
            skip_reason: None,
            skip_kind: None,
        };

        self.tasks.write().unwrap().push(task);

        Ok(task_id)
    }

    fn record_task_complete(&self, task_id: i64, exit_code: i32, status: TaskStatus) -> Result<()> {
        let ended_at = std::time::SystemTime::now()
            .duration_since(std::time::SystemTime::UNIX_EPOCH)
            .unwrap()
            .as_secs();

        let mut tasks = self.tasks.write().unwrap();
        let Some(task) = tasks.iter_mut().find(|t| t.id == task_id) else {
            // Same rule as the SQLite store, which reports zero rows updated:
            // completing a task that is not there leaves it `running` forever.
            // This used to return `Ok(())`, so a test against the fake proved
            // the opposite of what the real store does.
            return Err(eyre::eyre!("No task with id {task_id} to mark complete"));
        };

        task.status = status;
        task.exit_code = Some(exit_code);
        task.ended_at = Some(ended_at);
        // `saturating_sub`, matching the SQLite store's `MAX(?3 - started_at, 0)`:
        // a clock that stepped backwards used to underflow this u64 subtraction
        // and panic in debug, or write a duration of ~1.8e19 seconds in release.
        task.duration_seconds = task
            .started_at
            .map(|started_at| ended_at.saturating_sub(started_at) as f64);

        Ok(())
    }

    fn record_task_skipped(
        &self,
        run_id: i64,
        task_name: &str,
        script_hash: Option<&str>,
        skip_reason: Option<&str>,
        skip_kind: Option<SkipKind>,
    ) -> Result<i64> {
        let task_id = self.next_task_id.fetch_add(1, std::sync::atomic::Ordering::SeqCst);

        let task = TaskRecord {
            id: task_id,
            run_id,
            name: task_name.to_string(),
            status: TaskStatus::Skipped,
            script_hash: script_hash.map(String::from),
            exit_code: None,
            started_at: Some(
                std::time::SystemTime::now()
                    .duration_since(std::time::SystemTime::UNIX_EPOCH)
                    .unwrap()
                    .as_secs(),
            ),
            ended_at: None,
            duration_seconds: None,
            stdout_path: None,
            stderr_path: None,
            script_path: None,
            skip_reason: skip_reason.map(String::from),
            skip_kind,
        };

        self.tasks.write().unwrap().push(task);

        Ok(task_id)
    }

    fn get_task_history(&self, task_name: &str, limit: usize) -> Result<Vec<TaskRecord>> {
        let tasks = self.tasks.read().unwrap();

        let mut result: Vec<TaskRecord> = tasks.iter().filter(|t| t.name == task_name).cloned().collect();

        result.sort_by_key(|r| std::cmp::Reverse(r.started_at));
        result.truncate(limit);

        Ok(result)
    }

    fn get_overall_stats(&self) -> Result<OverallStats> {
        let runs = self.runs.read().unwrap();
        let tasks = self.tasks.read().unwrap();

        let total_runs = runs.len() as u64;
        let successful_runs = runs.iter().filter(|r| r.status == RunStatus::Success).count() as u64;
        let failed_runs = runs.iter().filter(|r| r.status == RunStatus::Failed).count() as u64;
        let running_runs = runs.iter().filter(|r| r.status == RunStatus::Running).count() as u64;
        let total_tasks = tasks.len() as u64;
        let total_disk_usage: u64 = runs.iter().filter_map(|r| r.size_bytes).sum();
        let total_duration_seconds: f64 = runs.iter().filter_map(|r| r.duration_seconds).sum();

        Ok(OverallStats {
            total_runs,
            successful_runs,
            failed_runs,
            running_runs,
            total_tasks,
            total_disk_usage,
            total_duration_seconds,
        })
    }

    fn get_all_projects(&self) -> Result<Vec<ProjectSummary>> {
        let projects = self.projects.read().unwrap();
        let mut result = projects.clone();
        result.sort_by_key(|r| std::cmp::Reverse(r.last_seen));
        Ok(result)
    }

    fn get_task_stats(&self, task_name: &str) -> Result<Vec<TaskStats>> {
        let tasks = self.tasks.read().unwrap();
        let runs = self.runs.read().unwrap();
        let projects = self.projects.read().unwrap();

        let mut stats_map: std::collections::HashMap<i64, TaskStats> = std::collections::HashMap::new();
        // Every recorded duration per project, so avg/min/max are computed the
        // same way the SQLite store's AVG/MIN/MAX do: over the rows that have a
        // duration, ignoring the ones that do not. The fake used to leave all
        // three `None` unconditionally, so `otto Stats`'s duration columns were
        // never exercised by anything that asserted a number.
        let mut durations: std::collections::HashMap<i64, Vec<f64>> = std::collections::HashMap::new();

        for task in tasks.iter().filter(|t| t.name == task_name) {
            if let Some(run) = runs.iter().find(|r| r.id == task.run_id)
                && let Some(project) = projects.iter().find(|p| p.id == run.project_id)
            {
                let stats = stats_map.entry(project.id).or_insert_with(|| TaskStats {
                    project_id: project.id,
                    project_hash: project.hash.clone(),
                    project_name: project.name.clone(),
                    task_name: task_name.to_string(),
                    total_executions: 0,
                    successful_executions: 0,
                    failed_executions: 0,
                    skipped_executions: 0,
                    avg_duration_seconds: None,
                    min_duration_seconds: None,
                    max_duration_seconds: None,
                    last_executed: None,
                    last_status: None,
                });

                stats.total_executions += 1;
                match task.status {
                    TaskStatus::Completed => stats.successful_executions += 1,
                    TaskStatus::Failed => stats.failed_executions += 1,
                    TaskStatus::Skipped => stats.skipped_executions += 1,
                    _ => {}
                }

                if let Some(duration) = task.duration_seconds {
                    durations.entry(project.id).or_default().push(duration);
                }

                if let Some(started_at) = task.started_at
                    && (stats.last_executed.is_none() || started_at > stats.last_executed.unwrap())
                {
                    stats.last_executed = Some(started_at);
                    stats.last_status = Some(task.status.clone());
                }
            }
        }

        for (project_id, recorded) in durations {
            let Some(stats) = stats_map.get_mut(&project_id) else {
                continue;
            };
            stats.avg_duration_seconds = Some(recorded.iter().sum::<f64>() / recorded.len() as f64);
            stats.min_duration_seconds = recorded.iter().copied().reduce(f64::min);
            stats.max_duration_seconds = recorded.iter().copied().reduce(f64::max);
        }

        Ok(stats_map.into_values().collect())
    }

    fn get_all_task_stats(&self, limit: Option<usize>) -> Result<Vec<TaskStats>> {
        // The read guard is dropped before the loop: `get_task_stats` takes the
        // same lock again, and `RwLock`'s read side is not reentrant when a
        // writer is queued behind it.
        let task_names: std::collections::HashSet<String> = {
            let tasks = self.tasks.read().unwrap();
            tasks.iter().map(|t| t.name.clone()).collect()
        };

        let mut all_stats = Vec::new();
        for task_name in task_names {
            all_stats.extend(self.get_task_stats(&task_name)?);
        }

        all_stats.sort_by_key(|s| std::cmp::Reverse(s.total_executions));

        if let Some(limit) = limit {
            all_stats.truncate(limit);
        }

        Ok(all_stats)
    }

    fn get_runs_with_filters(
        &self,
        status_filter: Option<RunStatus>,
        project_filter: Option<&str>,
        limit: usize,
    ) -> Result<Vec<RunRecord>> {
        let runs = self.runs.read().unwrap();
        let projects = self.projects.read().unwrap();

        let mut result: Vec<RunRecord> = runs
            .iter()
            .filter(|r| {
                let status_match = status_filter.as_ref().is_none_or(|s| r.status == *s);
                let project_match =
                    project_filter.is_none_or(|hash| projects.iter().any(|p| p.id == r.project_id && p.hash == hash));
                status_match && project_match
            })
            .cloned()
            .collect();

        result.sort_by_key(|r| std::cmp::Reverse(r.timestamp));
        result.truncate(limit);

        Ok(result)
    }

    fn find_old_runs(
        &self,
        keep_days: u64,
        keep_last: Option<usize>,
        keep_failed_days: Option<u64>,
        project_filter: Option<&str>,
    ) -> Result<Vec<RunRecord>> {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::SystemTime::UNIX_EPOCH)
            .unwrap()
            .as_secs();

        let runs = self.runs.read().unwrap();
        let projects = self.projects.read().unwrap();

        let filtered_runs: Vec<RunRecord> = runs
            .iter()
            .filter(|r| {
                project_filter.is_none_or(|hash| projects.iter().any(|p| p.id == r.project_id && p.hash == hash))
            })
            .cloned()
            .collect();

        // The same pure function the SQLite store uses. This loop used to be a
        // line-for-line copy of the one in `manager.rs`, which is a promise the
        // two would drift.
        let policy = Retention {
            keep_days,
            keep_last,
            keep_failed_days,
        };
        let ages: Vec<RunAge> = filtered_runs.iter().map(RunAge::from).collect();

        let mut runs_to_delete: Vec<RunRecord> = policy
            .expired(&ages, now)
            .into_iter()
            .map(|idx| filtered_runs[idx].clone())
            .collect();

        runs_to_delete.sort_by_key(|r| r.timestamp);

        Ok(runs_to_delete)
    }

    fn delete_run(&self, run_id: i64, _delete_filesystem: bool) -> Result<Option<RunRecord>> {
        let mut runs = self.runs.write().unwrap();
        let mut tasks = self.tasks.write().unwrap();
        let mut projects = self.projects.write().unwrap();

        if let Some(idx) = runs.iter().position(|r| r.id == run_id) {
            let run = runs.remove(idx);

            // Remove associated tasks
            tasks.retain(|t| t.run_id != run.id);

            // Update project run count
            if let Some(project) = projects.iter_mut().find(|p| p.id == run.project_id) {
                project.run_count = project.run_count.saturating_sub(1);
            }

            Ok(Some(run))
        } else {
            Ok(None)
        }
    }
}

#[path = "db_tests.rs"]
mod tests;
