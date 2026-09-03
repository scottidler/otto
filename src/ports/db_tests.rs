#![cfg(test)]

use super::*;

fn create_test_metadata(hash: &str, timestamp: u64) -> RunMetadata {
    RunMetadata::minimal(Some(PathBuf::from("/test/otto.yml")), hash.to_string(), timestamp)
}

/// Fetch every task recorded for `run_id`, ordered by `started_at` ascending.
///
/// The `StateStore` port dropped `get_run_tasks` (design doc
/// `2026-09-02-second-code-review-remediation.md`, Phase 10): nothing in
/// production ever called it through the trait, only tests. Tests still need
/// to inspect a run's tasks, so this reads `MemoryStateStore`'s own state
/// directly rather than through a port method with no other caller.
fn run_tasks(store: &MemoryStateStore, run_id: i64) -> Vec<TaskRecord> {
    let mut result: Vec<TaskRecord> = store
        .tasks
        .read()
        .unwrap()
        .iter()
        .filter(|t| t.run_id == run_id)
        .cloned()
        .collect();
    result.sort_by_key(|t| t.started_at);
    result
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

    // Ten runs, alternating success and failure, spanning both sides of the
    // 45-day `keep_failed_days` cutoff exercised below but never landing
    // exactly on it (or on the 30-day `keep_days` cutoff). `find_old_runs`
    // reads `SystemTime::now()` independently on each backend, once per
    // call below; a run placed exactly on a cutoff would flip from "kept" to
    // "expired" the instant any wall-clock time elapsed between the sqlite
    // call and the memory call, which is exactly what made this test flaky
    // once (Phase 9). A multi-day margin around every cutoff makes that
    // impossible; the exact-boundary tie-break itself is pinned
    // deterministically, with an injected `now`, by
    // `executor::state::retention_tests::test_keep_failed_days_exact_boundary_is_kept_not_expired`.
    let ages_and_status: [(u64, RunStatus); 10] = [
        (40, RunStatus::Success),
        (41, RunStatus::Failed),
        (42, RunStatus::Success),
        (43, RunStatus::Failed),
        (44, RunStatus::Success),
        (46, RunStatus::Failed),
        (47, RunStatus::Success),
        (48, RunStatus::Failed),
        (49, RunStatus::Success),
        (50, RunStatus::Failed),
    ];
    for (days_ago, status) in ages_and_status {
        let metadata = create_test_metadata("abc12345", now - (days_ago * 86400));

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

/// The two backends must agree about task stats, durations included.
///
/// `stats_tests.rs` drives `otto Stats` through the fake alone, so anything the
/// fake did not compute was never exercised end to end - and the fake never
/// computed `avg`/`min`/`max` at all, leaving all three `None` while SQLite's
/// `AVG`/`MIN`/`MAX` returned numbers. The duration columns of `otto Stats`
/// were therefore untested by construction.
///
/// The durations here are sub-second, so both backends land on 0.0: the assertion
/// is `Some(0.0) == Some(0.0)`, and the defect showed up as `None` on one side.
/// `last_executed` is deliberately not compared - each backend reads its own
/// `SystemTime::now()`.
#[test]
fn memory_and_sqlite_stores_agree_about_task_stats() {
    use crate::executor::state::StateManager;

    let temp_dir = tempfile::TempDir::new().unwrap();
    let sqlite = StateManager::with_db_path(temp_dir.path().join("stats-parity.db")).unwrap();
    let memory = MemoryStateStore::new();

    // A completed task, a failed one, and a skipped one - the last of which has
    // no duration, so it must not drag the average down on either side.
    for store in [&sqlite as &dyn StateStore, &memory as &dyn StateStore] {
        let run_id = store
            .record_run_start(&create_test_metadata("abc12345", 1_700_000_000))
            .unwrap();
        let ok = store
            .record_task_start(run_id, "build", Some("h1"), None, None, None)
            .unwrap();
        store.record_task_complete(ok, 0, TaskStatus::Completed).unwrap();
        let bad = store
            .record_task_start(run_id, "build", Some("h2"), None, None, None)
            .unwrap();
        store.record_task_complete(bad, 1, TaskStatus::Failed).unwrap();
        store
            .record_task_skipped(
                run_id,
                "build",
                Some("h3"),
                Some("dep failed"),
                Some(SkipKind::Unreachable),
            )
            .unwrap();
    }

    /// The deterministic half of a `TaskStats` row: the four counts and the
    /// three durations. Named, because comparing seven-tuples by hand is how a
    /// parity assertion silently stops comparing a field.
    #[derive(Debug, PartialEq)]
    struct Comparable {
        total: u64,
        successful: u64,
        failed: u64,
        skipped: u64,
        avg: Option<f64>,
        min: Option<f64>,
        max: Option<f64>,
    }

    fn comparable(stats: Vec<TaskStats>) -> Vec<Comparable> {
        let mut rows: Vec<Comparable> = stats
            .into_iter()
            .map(|s| Comparable {
                total: s.total_executions,
                successful: s.successful_executions,
                failed: s.failed_executions,
                skipped: s.skipped_executions,
                avg: s.avg_duration_seconds,
                min: s.min_duration_seconds,
                max: s.max_duration_seconds,
            })
            .collect();
        rows.sort_by_key(|row| std::cmp::Reverse(row.total));
        rows
    }

    let from_sqlite = comparable(sqlite.get_task_stats("build").unwrap());
    let from_memory = comparable(memory.get_task_stats("build").unwrap());
    assert_eq!(from_sqlite, from_memory, "backends disagree about get_task_stats");
    assert_eq!(
        from_sqlite,
        vec![Comparable {
            total: 3,
            successful: 1,
            failed: 1,
            skipped: 1,
            avg: Some(0.0),
            min: Some(0.0),
            max: Some(0.0),
        }],
        "three executions, one of each status, with durations computed over the two that have one"
    );

    assert_eq!(
        comparable(sqlite.get_all_task_stats(None).unwrap()),
        comparable(memory.get_all_task_stats(None).unwrap()),
        "backends disagree about get_all_task_stats"
    );
}

/// Both backends refuse to complete a run that does not exist.
#[test]
fn memory_store_reports_completing_a_missing_run() {
    let store = MemoryStateStore::new();
    let err = store.record_run_complete(99, RunStatus::Success, None).unwrap_err();
    assert!(err.to_string().contains("No run with id 99"), "{err}");
}

/// And both refuse to complete a task that does not exist. The fake returned
/// `Ok(())` where SQLite reports zero rows updated, so a test written against
/// the fake proved the opposite of the real store's behavior.
#[test]
fn memory_store_reports_completing_a_missing_task() {
    let store = MemoryStateStore::new();
    let err = store.record_task_complete(99, 0, TaskStatus::Completed).unwrap_err();
    assert!(err.to_string().contains("No task with id 99"), "{err}");
}

#[test]
fn test_memory_store_record_run_start() {
    let store = MemoryStateStore::new();

    let metadata = create_test_metadata("abc123", 1234567890);
    let run_id = store.record_run_start(&metadata).unwrap();

    assert!(run_id > 0);

    let runs = store.get_runs_with_filters(None, None, 10).unwrap();
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

    let runs = store.get_runs_with_filters(None, None, 10).unwrap();
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

    let tasks = run_tasks(&store, run_id);
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

    let tasks = run_tasks(&store, run_id);
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

    let tasks = run_tasks(&store, run_id);
    assert_eq!(tasks.len(), 1);
    assert_eq!(tasks[0].status, TaskStatus::Skipped);
    assert_eq!(tasks[0].skip_kind, Some(SkipKind::Unreachable));
}

#[test]
fn test_memory_store_get_runs_with_filters_project_filter() {
    let store = MemoryStateStore::new();

    let metadata1 = create_test_metadata("abc123", 1234567890);
    store.record_run_start(&metadata1).unwrap();

    let metadata2 = create_test_metadata("def456", 1234567891);
    store.record_run_start(&metadata2).unwrap();

    let all_runs = store.get_runs_with_filters(None, None, 10).unwrap();
    assert_eq!(all_runs.len(), 2);

    let filtered_runs = store.get_runs_with_filters(None, Some("abc123"), 10).unwrap();
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

    let runs = store.get_runs_with_filters(None, None, 10).unwrap();
    assert_eq!(runs.len(), 0);

    let tasks = run_tasks(&store, run_id);
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
