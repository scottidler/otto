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
    fn get_recent_runs(&self, limit: usize, project_filter: Option<&str>) -> Result<Vec<RunRecord>>;
    fn get_run_tasks(&self, run_id: i64) -> Result<Vec<TaskRecord>>;
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
        let name = ottofile_path
            .and_then(|p| p.parent())
            .and_then(|p| p.file_name())
            .and_then(|n| n.to_str())
            .unwrap_or(hash)
            .to_string();

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
        if let Some(task) = tasks.iter_mut().find(|t| t.id == task_id) {
            task.status = status;
            task.exit_code = Some(exit_code);
            task.ended_at = Some(ended_at);
            if let Some(started_at) = task.started_at {
                task.duration_seconds = Some((ended_at - started_at) as f64);
            }
        }

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

    fn get_recent_runs(&self, limit: usize, project_filter: Option<&str>) -> Result<Vec<RunRecord>> {
        let runs = self.runs.read().unwrap();
        let projects = self.projects.read().unwrap();

        let mut result: Vec<RunRecord> = runs
            .iter()
            .filter(|r| {
                if let Some(hash) = project_filter {
                    projects.iter().any(|p| p.id == r.project_id && p.hash == hash)
                } else {
                    true
                }
            })
            .cloned()
            .collect();

        result.sort_by_key(|r| std::cmp::Reverse(r.timestamp));
        result.truncate(limit);

        Ok(result)
    }

    fn get_run_tasks(&self, run_id: i64) -> Result<Vec<TaskRecord>> {
        let tasks = self.tasks.read().unwrap();

        let mut result: Vec<TaskRecord> = tasks.iter().filter(|t| t.run_id == run_id).cloned().collect();

        result.sort_by_key(|r| r.started_at);

        Ok(result)
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

                if let Some(started_at) = task.started_at
                    && (stats.last_executed.is_none() || started_at > stats.last_executed.unwrap())
                {
                    stats.last_executed = Some(started_at);
                    stats.last_status = Some(task.status.clone());
                }
            }
        }

        Ok(stats_map.into_values().collect())
    }

    fn get_all_task_stats(&self, limit: Option<usize>) -> Result<Vec<TaskStats>> {
        let tasks = self.tasks.read().unwrap();

        let task_names: std::collections::HashSet<&str> = tasks.iter().map(|t| t.name.as_str()).collect();

        let mut all_stats = Vec::new();
        for task_name in task_names {
            all_stats.extend(self.get_task_stats(task_name)?);
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

#[cfg(test)]
mod tests {
    use super::*;

    fn create_test_metadata(hash: &str, timestamp: u64) -> RunMetadata {
        RunMetadata::minimal(Some(PathBuf::from("/test/otto.yml")), hash.to_string(), timestamp)
    }

    /// The in-memory fake and the SQLite store must agree about retention, or a
    /// test that passes against the fake proves nothing about the real thing.
    /// Their retention loops used to be duplicated line for line.
    #[test]
    fn memory_and_sqlite_stores_agree_about_retention() {
        use crate::executor::state::StateManager;

        let temp_dir = tempfile::TempDir::new().unwrap();
        let sqlite = StateManager::with_db_path(temp_dir.path().join("parity.db")).unwrap();
        let memory = MemoryStateStore::new();

        let now = std::time::SystemTime::now()
            .duration_since(std::time::SystemTime::UNIX_EPOCH)
            .unwrap()
            .as_secs();

        // Ten runs, 40 to 49 days old, alternating success and failure.
        for i in 0..10u64 {
            let metadata = create_test_metadata("abc12345", now - ((40 + i) * 86400));
            let status = if i % 2 == 0 { RunStatus::Success } else { RunStatus::Failed };

            let sqlite_id = sqlite.record_run_start(&metadata).unwrap();
            sqlite.record_run_complete(sqlite_id, status.clone(), Some(1)).unwrap();
            let memory_id = memory.record_run_start(&metadata).unwrap();
            memory.record_run_complete(memory_id, status, Some(1)).unwrap();
        }

        for (keep_days, keep_last, keep_failed) in [
            (30, None, None),
            (30, Some(3), None),
            (30, None, Some(45)),
            (30, Some(2), Some(45)),
            (0, Some(4), None),
            (365, None, None),
        ] {
            let from_sqlite: Vec<u64> = sqlite
                .find_old_runs(keep_days, keep_last, keep_failed, None)
                .unwrap()
                .iter()
                .map(|r| r.timestamp)
                .collect();
            let from_memory: Vec<u64> = memory
                .find_old_runs(keep_days, keep_last, keep_failed, None)
                .unwrap()
                .iter()
                .map(|r| r.timestamp)
                .collect();

            assert_eq!(
                from_sqlite, from_memory,
                "backends disagree for keep_days={keep_days} keep_last={keep_last:?} keep_failed={keep_failed:?}"
            );
        }
    }

    /// Both backends refuse to complete a run that does not exist.
    #[test]
    fn memory_store_reports_completing_a_missing_run() {
        let store = MemoryStateStore::new();
        let err = store.record_run_complete(99, RunStatus::Success, None).unwrap_err();
        assert!(err.to_string().contains("No run with id 99"), "{err}");
    }

    #[test]
    fn test_memory_store_record_run_start() {
        let store = MemoryStateStore::new();

        let metadata = create_test_metadata("abc123", 1234567890);
        let run_id = store.record_run_start(&metadata).unwrap();

        assert!(run_id > 0);

        let runs = store.get_recent_runs(10, None).unwrap();
        assert_eq!(runs.len(), 1);
        assert_eq!(runs[0].timestamp, 1234567890);
        assert_eq!(runs[0].status, RunStatus::Running);
    }

    #[test]
    fn test_memory_store_record_run_complete() {
        let store = MemoryStateStore::new();

        let metadata = create_test_metadata("abc123", 1234567890);
        let run_id = store.record_run_start(&metadata).unwrap();
        store
            .record_run_complete(run_id, RunStatus::Success, Some(1024))
            .unwrap();

        let runs = store.get_recent_runs(10, None).unwrap();
        assert_eq!(runs[0].status, RunStatus::Success);
        assert_eq!(runs[0].size_bytes, Some(1024));
    }

    #[test]
    fn test_memory_store_record_task() {
        let store = MemoryStateStore::new();

        let metadata = create_test_metadata("abc123", 1234567890);
        let run_id = store.record_run_start(&metadata).unwrap();

        let task_id = store
            .record_task_start(run_id, "build", Some("hash123"), None, None, None)
            .unwrap();

        assert!(task_id > 0);

        let tasks = store.get_run_tasks(run_id).unwrap();
        assert_eq!(tasks.len(), 1);
        assert_eq!(tasks[0].name, "build");
        assert_eq!(tasks[0].status, TaskStatus::Running);
    }

    #[test]
    fn test_memory_store_task_complete() {
        let store = MemoryStateStore::new();

        let metadata = create_test_metadata("abc123", 1234567890);
        let run_id = store.record_run_start(&metadata).unwrap();
        let task_id = store
            .record_task_start(run_id, "build", None, None, None, None)
            .unwrap();

        store.record_task_complete(task_id, 0, TaskStatus::Completed).unwrap();

        let tasks = store.get_run_tasks(run_id).unwrap();
        assert_eq!(tasks[0].status, TaskStatus::Completed);
        assert_eq!(tasks[0].exit_code, Some(0));
    }

    #[test]
    fn test_memory_store_task_skipped() {
        let store = MemoryStateStore::new();

        let metadata = create_test_metadata("abc123", 1234567890);
        let run_id = store.record_run_start(&metadata).unwrap();

        let task_id = store
            .record_task_skipped(
                run_id,
                "build",
                Some("hash123"),
                Some("dep fetch skipped; cascade"),
                Some(SkipKind::Unreachable),
            )
            .unwrap();

        assert!(task_id > 0);

        let tasks = store.get_run_tasks(run_id).unwrap();
        assert_eq!(tasks.len(), 1);
        assert_eq!(tasks[0].status, TaskStatus::Skipped);
        assert_eq!(tasks[0].skip_kind, Some(SkipKind::Unreachable));
    }

    #[test]
    fn test_memory_store_get_recent_runs_with_filter() {
        let store = MemoryStateStore::new();

        let metadata1 = create_test_metadata("abc123", 1234567890);
        store.record_run_start(&metadata1).unwrap();

        let metadata2 = create_test_metadata("def456", 1234567891);
        store.record_run_start(&metadata2).unwrap();

        let all_runs = store.get_recent_runs(10, None).unwrap();
        assert_eq!(all_runs.len(), 2);

        let filtered_runs = store.get_recent_runs(10, Some("abc123")).unwrap();
        assert_eq!(filtered_runs.len(), 1);
        assert_eq!(filtered_runs[0].timestamp, 1234567890);
    }

    #[test]
    fn test_memory_store_overall_stats() {
        let store = MemoryStateStore::new();

        let metadata1 = create_test_metadata("abc123", 1234567890);
        let run_id1 = store.record_run_start(&metadata1).unwrap();
        store
            .record_run_complete(run_id1, RunStatus::Success, Some(1024))
            .unwrap();

        let metadata2 = create_test_metadata("abc123", 1234567891);
        let run_id2 = store.record_run_start(&metadata2).unwrap();
        store
            .record_run_complete(run_id2, RunStatus::Failed, Some(2048))
            .unwrap();

        let stats = store.get_overall_stats().unwrap();
        assert_eq!(stats.total_runs, 2);
        assert_eq!(stats.successful_runs, 1);
        assert_eq!(stats.failed_runs, 1);
        assert_eq!(stats.total_disk_usage, 3072);
    }

    #[test]
    fn test_memory_store_get_all_projects() {
        let store = MemoryStateStore::new();

        let metadata1 = create_test_metadata("abc123", 1234567890);
        store.record_run_start(&metadata1).unwrap();

        let metadata2 = create_test_metadata("def456", 1234567891);
        store.record_run_start(&metadata2).unwrap();

        let projects = store.get_all_projects().unwrap();
        assert_eq!(projects.len(), 2);
    }

    #[test]
    fn test_memory_store_delete_run() {
        let store = MemoryStateStore::new();

        let metadata = create_test_metadata("abc123", 1234567890);
        let run_id = store.record_run_start(&metadata).unwrap();
        store
            .record_task_start(run_id, "build", None, None, None, None)
            .unwrap();

        let deleted = store.delete_run(run_id, false).unwrap();
        assert!(deleted.is_some());

        let runs = store.get_recent_runs(10, None).unwrap();
        assert_eq!(runs.len(), 0);

        let tasks = store.get_run_tasks(run_id).unwrap();
        assert_eq!(tasks.len(), 0);
    }

    #[test]
    fn test_memory_store_delete_nonexistent() {
        let store = MemoryStateStore::new();

        let deleted = store.delete_run(9999, false).unwrap();
        assert!(deleted.is_none());
    }

    #[test]
    fn test_memory_store_get_task_history() {
        let store = MemoryStateStore::new();

        for i in 0..5 {
            let metadata = create_test_metadata("abc123", 1234567890 + i);
            let run_id = store.record_run_start(&metadata).unwrap();
            let task_id = store
                .record_task_start(run_id, "build", None, None, None, None)
                .unwrap();
            store.record_task_complete(task_id, 0, TaskStatus::Completed).unwrap();
        }

        let history = store.get_task_history("build", 3).unwrap();
        assert_eq!(history.len(), 3);
    }

    #[test]
    fn test_memory_store_get_runs_with_filters() {
        let store = MemoryStateStore::new();

        let metadata1 = create_test_metadata("abc123", 1234567890);
        let run_id1 = store.record_run_start(&metadata1).unwrap();
        store.record_run_complete(run_id1, RunStatus::Success, None).unwrap();

        let metadata2 = create_test_metadata("abc123", 1234567891);
        let run_id2 = store.record_run_start(&metadata2).unwrap();
        store.record_run_complete(run_id2, RunStatus::Failed, None).unwrap();

        let success_runs = store.get_runs_with_filters(Some(RunStatus::Success), None, 10).unwrap();
        assert_eq!(success_runs.len(), 1);
        assert_eq!(success_runs[0].status, RunStatus::Success);

        let failed_runs = store.get_runs_with_filters(Some(RunStatus::Failed), None, 10).unwrap();
        assert_eq!(failed_runs.len(), 1);
        assert_eq!(failed_runs[0].status, RunStatus::Failed);
    }
}
