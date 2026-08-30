#![cfg(test)]

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
