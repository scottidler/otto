#![cfg(test)]

use super::*;
use serial_test::serial;
use std::path::PathBuf;
use tempfile::TempDir;

fn create_test_manager() -> Result<(StateManager, TempDir)> {
    let temp_dir = TempDir::new()?;
    let db_path = temp_dir.path().join("test.db");
    let manager = StateManager::with_db_path(db_path)?;
    Ok((manager, temp_dir))
}

/// Fetch every task recorded for `run_id`, ordered by `started_at` ascending.
///
/// The `StateStore` port dropped `get_run_tasks` (design doc
/// `2026-09-02-second-code-review-remediation.md`, Phase 10): nothing in
/// production ever called it through the trait, only tests. Tests still need
/// to inspect a run's tasks, so the query moves here rather than disappearing
/// with no replacement.
fn run_tasks(manager: &StateManager, run_id: i64) -> Result<Vec<TaskRecord>> {
    manager.db.with_connection(|conn| {
        let mut stmt = conn.prepare(
            "SELECT id, run_id, name, status, script_hash, exit_code,
                    started_at, ended_at, duration_seconds,
                    stdout_path, stderr_path, script_path, skip_reason, skip_kind
             FROM tasks
             WHERE run_id = ?1
             ORDER BY started_at ASC",
        )?;

        let rows = stmt.query_map(params![run_id], StateManager::row_to_task_record)?;

        rows.collect::<Result<Vec<_>, _>>().context("Failed to fetch tasks")
    })
}

/// Run `f` with `OTTO_HOME` pointed at `otto_home`, restoring it afterwards.
fn with_otto_home<T>(otto_home: &std::path::Path, f: impl FnOnce() -> T) -> T {
    let previous = std::env::var("OTTO_HOME").ok();
    // SAFETY: single-threaded test body, serialized against every other
    // test that reads the environment.
    unsafe { std::env::set_var("OTTO_HOME", otto_home) };
    let out = f();
    unsafe {
        match previous {
            Some(home) => std::env::set_var("OTTO_HOME", home),
            None => std::env::remove_var("OTTO_HOME"),
        }
    }
    out
}

/// The DB-driven delete had no symlink or containment check at all - it was
/// the second half of the same defect as the filesystem scan, and the one
/// with no `is_dir()` in front of it.
///
/// The refusal also has to happen before the rows go away. It used to commit
/// the row deletion first and fence afterwards, so this exact case - the one
/// the fence exists for - lost the row and kept the directory, orphaned with
/// nothing left pointing at it.
#[test]
#[serial]
fn delete_run_never_deletes_through_a_symlinked_run_directory() -> Result<()> {
    let (manager, temp_dir) = create_test_manager()?;

    let otto_home = temp_dir.path().join("otto-home");
    let project = otto_home.join("widget-abc12345");
    std::fs::create_dir_all(&project)?;

    let victim = temp_dir.path().join("victim");
    std::fs::create_dir_all(&victim)?;
    std::fs::write(victim.join("precious.txt"), "keep me")?;
    let run_dir = project.join("1234567890");
    std::os::unix::fs::symlink(&victim, &run_dir)?;

    let metadata = RunMetadata::minimal(
        Some(PathBuf::from("/test/otto.yml")),
        "abc12345".to_string(),
        1234567890,
    )
    .with_run_dir(run_dir);
    let run_id = manager.record_run_start(&metadata)?;

    let result = with_otto_home(&otto_home, || manager.delete_run(run_id, true));

    let err = result.unwrap_err().to_string();
    assert!(err.contains("Refusing to delete run directory"), "{err}");
    assert!(victim.join("precious.txt").exists(), "the symlink target must survive");
    let rows = manager.get_runs_with_filters(None, None, 10)?;
    assert_eq!(
        rows.len(),
        1,
        "and the row survives: a refused delete must leave the database untouched"
    );
    assert_eq!(rows[0].id, run_id);
    let projects = manager.get_all_projects()?;
    assert_eq!(projects[0].run_count, 1, "the run count is untouched too");
    Ok(())
}

/// The directory a run actually created is the directory cleanup removes.
/// Cleanup used to rebuild `$HOME/.otto/otto-<hash>`, which matched neither
/// the `<name>-<hash>` convention nor `OTTO_HOME`, so it deleted the rows
/// and left 220 of 222 real project directories orphaned.
#[test]
#[serial]
fn delete_run_removes_the_recorded_run_directory() -> Result<()> {
    let (manager, temp_dir) = create_test_manager()?;

    let otto_home = temp_dir.path().join("otto-home");
    let run_dir = otto_home.join("widget-abc12345").join("1234567890");
    std::fs::create_dir_all(run_dir.join("tasks"))?;

    let metadata = RunMetadata::minimal(
        Some(PathBuf::from("/test/otto.yml")),
        "abc12345".to_string(),
        1234567890,
    )
    .with_run_dir(run_dir.clone());
    let run_id = manager.record_run_start(&metadata)?;

    let deleted = with_otto_home(&otto_home, || manager.delete_run(run_id, true))?;

    assert!(deleted.is_some(), "the run row is deleted");
    assert!(!run_dir.exists(), "the run directory is deleted too");
    Ok(())
}

/// Rows written before schema v5 carry no run directory, so the path is
/// derived - from `OTTO_HOME` and the project's own name, not from `$HOME`
/// and a hardcoded `otto-` prefix. The derivation can only work when the
/// recorded project hash is also the one in the directory name, which is
/// what this fixture arranges.
#[test]
#[serial]
fn delete_run_derives_the_directory_for_a_pre_v5_row() -> Result<()> {
    let (manager, temp_dir) = create_test_manager()?;

    let otto_home = temp_dir.path().join("otto-home");
    // `ensure_project` names this project after the ottofile's directory.
    let run_dir = otto_home.join("widget-abc12345").join("1234567890");
    std::fs::create_dir_all(run_dir.join("tasks"))?;

    let metadata = RunMetadata::minimal(
        Some(PathBuf::from("/repos/widget/otto.yml")),
        "abc12345".to_string(),
        1234567890,
    );
    let run_id = manager.record_run_start(&metadata)?;
    manager
        .db
        .with_connection(|conn| Ok(conn.execute("UPDATE runs SET run_dir = NULL", [])?))?;

    with_otto_home(&otto_home, || manager.delete_run(run_id, true))?;

    assert!(!run_dir.exists(), "the derived path finds the real directory");
    Ok(())
}

/// Two runs in the same second are two runs. The global
/// `UNIQUE(runs.timestamp)` made the second one fail outright, taking every
/// task record that would have hung off it.
#[test]
fn two_runs_in_the_same_second_both_persist() -> Result<()> {
    let (manager, _temp_dir) = create_test_manager()?;

    let metadata = RunMetadata::minimal(
        Some(PathBuf::from("/test/otto.yml")),
        "abc12345".to_string(),
        1700000000,
    );
    let first = manager.record_run_start(&metadata)?;
    let second = manager.record_run_start(&metadata)?;
    assert_ne!(first, second);

    // Completion is keyed on the id, so one run does not close the other.
    manager.record_run_complete(first, RunStatus::Success, Some(1))?;
    manager.record_run_complete(second, RunStatus::Failed, Some(2))?;

    let runs = manager.get_runs_with_filters(None, None, 10)?;
    assert_eq!(runs.len(), 2, "both runs survive");
    let mut statuses: Vec<&str> = runs.iter().map(|r| r.status.as_str()).collect();
    statuses.sort_unstable();
    assert_eq!(statuses, vec!["failed", "success"]);
    Ok(())
}

#[test]
fn record_run_complete_reports_a_run_that_is_not_there() -> Result<()> {
    let (manager, _temp_dir) = create_test_manager()?;

    let err = manager
        .record_run_complete(4242, RunStatus::Success, None)
        .unwrap_err()
        .to_string();

    assert!(err.contains("No run with id 4242"), "{err}");
    Ok(())
}

/// A non-zero exit is stored as itself, not flattened to 1.
#[test]
fn a_task_exit_code_is_recorded_verbatim() -> Result<()> {
    let (manager, _temp_dir) = create_test_manager()?;

    let metadata = RunMetadata::minimal(
        Some(PathBuf::from("/test/otto.yml")),
        "abc12345".to_string(),
        1700000000,
    );
    let run_id = manager.record_run_start(&metadata)?;
    let task_id = manager.record_task_start(run_id, "boom", None, None, None, None)?;
    manager.record_task_complete(task_id, 7, TaskStatus::Failed)?;

    let tasks = run_tasks(&manager, run_id)?;
    assert_eq!(tasks[0].exit_code, Some(7));
    assert_eq!(tasks[0].status, TaskStatus::Failed);
    Ok(())
}

/// An unknown status in the database is reported, not silently read back as
/// `Failed`, which made a corrupt row indistinguishable from a failed run.
#[test]
fn an_unknown_run_status_is_an_error_not_a_failure() -> Result<()> {
    let (manager, _temp_dir) = create_test_manager()?;

    let metadata = RunMetadata::minimal(
        Some(PathBuf::from("/test/otto.yml")),
        "abc12345".to_string(),
        1700000000,
    );
    manager.record_run_start(&metadata)?;
    manager
        .db
        .with_connection(|conn| Ok(conn.execute("UPDATE runs SET status = 'wat'", [])?))?;

    let err = manager.get_runs_with_filters(None, None, 10).unwrap_err().to_string();
    assert!(err.contains("Failed to fetch runs"), "{err}");
    Ok(())
}

/// The SELECT-then-INSERT race is gone: recording the same project twice
/// upserts instead of colliding on the hash, and the second call keeps the
/// same id.
#[test]
fn ensure_project_is_idempotent() -> Result<()> {
    let (manager, _temp_dir) = create_test_manager()?;

    let metadata = RunMetadata::minimal(Some(PathBuf::from("/repos/widget/otto.yml")), "abc12345".into(), 1);
    manager.record_run_start(&metadata)?;
    manager.record_run_start(&metadata)?;

    let projects = manager.get_all_projects()?;
    assert_eq!(projects.len(), 1);
    assert_eq!(projects[0].name, "widget");
    assert_eq!(projects[0].run_count, 2);
    Ok(())
}

/// A project's run count is a count, so it never goes below zero even when
/// more runs are deleted than the counter ever saw.
#[test]
#[serial]
fn delete_run_never_drives_the_run_count_negative() -> Result<()> {
    let (manager, temp_dir) = create_test_manager()?;
    let otto_home = temp_dir.path().join("otto-home");
    std::fs::create_dir_all(&otto_home)?;

    let metadata = RunMetadata::minimal(Some(PathBuf::from("/repos/widget/otto.yml")), "abc12345".into(), 1);
    let run_id = manager.record_run_start(&metadata)?;
    manager
        .db
        .with_connection(|conn| Ok(conn.execute("UPDATE projects SET run_count = 0", [])?))?;

    with_otto_home(&otto_home, || manager.delete_run(run_id, false))?;

    let projects = manager.get_all_projects()?;
    assert_eq!(projects[0].run_count, 0);
    Ok(())
}

/// A skipped task carries a start time, so an ordering-by-`started_at` query
/// over a run's tasks does not sort every skip ahead of the run.
#[test]
fn a_skipped_task_records_when_it_was_skipped() -> Result<()> {
    let (manager, _temp_dir) = create_test_manager()?;

    let metadata = RunMetadata::minimal(Some(PathBuf::from("/test/otto.yml")), "abc12345".into(), 1700000000);
    let run_id = manager.record_run_start(&metadata)?;
    manager.record_task_skipped(run_id, "gated", None, Some("dep failed"), Some(SkipKind::Unreachable))?;

    let tasks = run_tasks(&manager, run_id)?;
    assert_eq!(tasks[0].status, TaskStatus::Skipped);
    assert!(tasks[0].started_at.is_some(), "a skip happens at a moment");
    assert_eq!(tasks[0].skip_kind, Some(SkipKind::Unreachable));
    Ok(())
}

/// A stored `skip_kind` no `SkipKind` variant parses is reported, on the same
/// rule as the `status` column beside it. It used to go through
/// `and_then(SkipKind::parse)`, so a corrupt kind read back as "skipped for no
/// recorded reason" and history could not be filtered by reason class.
#[test]
fn an_unknown_skip_kind_is_an_error_not_a_missing_reason() -> Result<()> {
    let (manager, _temp_dir) = create_test_manager()?;

    let metadata = RunMetadata::minimal(Some(PathBuf::from("/test/otto.yml")), "abc12345".into(), 1700000000);
    let run_id = manager.record_run_start(&metadata)?;
    manager.record_task_skipped(run_id, "gated", None, Some("dep failed"), Some(SkipKind::Unreachable))?;
    manager
        .db
        .with_connection(|conn| Ok(conn.execute("UPDATE tasks SET skip_kind = 'not-a-kind'", [])?))?;

    let err =
        run_tasks(&manager, run_id).expect_err("an unrecognized skip kind must be a named error, not a silent None");
    assert!(format!("{err:#}").contains("unknown skip kind"), "{err:#}");
    Ok(())
}

/// The project upsert, the run row, and the run counter are one transaction, so
/// a failure anywhere in them leaves none of the three behind.
///
/// They used to be autocommit statements: a failure - or a crash - after the
/// project upsert left a project row for a run that was never recorded, and one
/// after the run INSERT left `run_count` permanently short. The failure is
/// injected with a trigger that aborts the run INSERT, which is the statement
/// between the other two.
#[test]
fn record_run_start_rolls_back_the_project_when_the_run_insert_fails() -> Result<()> {
    let (manager, _temp_dir) = create_test_manager()?;
    manager.db.with_connection(|conn| {
        Ok(conn.execute_batch(
            "CREATE TRIGGER abort_the_run_insert BEFORE INSERT ON runs
             BEGIN SELECT RAISE(ABORT, 'injected failure'); END",
        )?)
    })?;

    let metadata = RunMetadata::minimal(
        Some(PathBuf::from("/repos/widget/otto.yml")),
        "abc12345".into(),
        1700000000,
    );
    let err = manager
        .record_run_start(&metadata)
        .expect_err("the injected trigger must surface");
    assert!(format!("{err:#}").contains("injected failure"), "{err:#}");

    assert!(
        manager.get_all_projects()?.is_empty(),
        "the project upsert must roll back with the run it was recorded for"
    );
    assert!(manager.get_runs_with_filters(None, None, 10)?.is_empty());
    Ok(())
}

/// A duration is never negative. The subtraction happens in SQL and is clamped
/// at zero, the same shape `record_run_complete` already used; it used to be
/// done in Rust with no clamp, so a clock that stepped backwards between a
/// task's start and its end wrote a negative duration straight into the stats.
#[test]
fn record_task_complete_clamps_a_backwards_clock() -> Result<()> {
    let (manager, _temp_dir) = create_test_manager()?;

    let metadata = RunMetadata::minimal(Some(PathBuf::from("/test/otto.yml")), "abc12345".into(), 1700000000);
    let run_id = manager.record_run_start(&metadata)?;
    let task_id = manager.record_task_start(run_id, "build", None, None, None, None)?;

    // An hour in the future, as a clock stepping backwards mid-task looks from
    // the completion's point of view.
    let ahead = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH)?.as_secs() + 3600;
    manager.db.with_connection(|conn| {
        Ok(conn.execute(
            "UPDATE tasks SET started_at = ?1 WHERE id = ?2",
            rusqlite::params![ahead as i64, task_id],
        )?)
    })?;

    manager.record_task_complete(task_id, 0, TaskStatus::Completed)?;

    let tasks = run_tasks(&manager, run_id)?;
    assert_eq!(
        tasks[0].duration_seconds,
        Some(0.0),
        "a backwards clock clamps to zero rather than recording a negative duration"
    );
    Ok(())
}

/// `started_at` is nullable, so a task with no recorded start gets no duration
/// - not one measured from the epoch - and the completion still succeeds.
#[test]
fn record_task_complete_leaves_no_duration_when_the_task_never_started() -> Result<()> {
    let (manager, _temp_dir) = create_test_manager()?;

    let metadata = RunMetadata::minimal(Some(PathBuf::from("/test/otto.yml")), "abc12345".into(), 1700000000);
    let run_id = manager.record_run_start(&metadata)?;
    let task_id = manager.record_task_start(run_id, "build", None, None, None, None)?;
    manager
        .db
        .with_connection(|conn| Ok(conn.execute("UPDATE tasks SET started_at = NULL", [])?))?;

    manager.record_task_complete(task_id, 0, TaskStatus::Completed)?;

    let tasks = run_tasks(&manager, run_id)?;
    assert_eq!(tasks[0].status, TaskStatus::Completed);
    assert_eq!(tasks[0].duration_seconds, None);
    Ok(())
}

/// Completing a task that is not there is an error, not a no-op: the row would
/// stay `running` forever and skew every stat. Same rule as
/// `record_run_complete`, and the rule the in-memory fake now follows too.
#[test]
fn record_task_complete_reports_a_task_that_is_not_there() -> Result<()> {
    let (manager, _temp_dir) = create_test_manager()?;

    let err = manager
        .record_task_complete(4242, 0, TaskStatus::Completed)
        .expect_err("a completion for a missing task must be reported");
    assert!(format!("{err:#}").contains("No task with id 4242"), "{err:#}");
    Ok(())
}

/// The `script_hash` column carries the hash of the rendered script, so a
/// bash task's row has one. The scheduler passed a literal `None` here with a
/// TODO beside it while `ProcessedAction::Bash` carried the hash in the same
/// match; the end-to-end half of this is
/// `tests/history_records_the_run_test.rs`.
#[test]
fn record_task_start_records_the_script_hash() -> Result<()> {
    let (manager, _temp_dir) = create_test_manager()?;

    let metadata = RunMetadata::minimal(Some(PathBuf::from("/test/otto.yml")), "abc12345".into(), 1700000000);
    let run_id = manager.record_run_start(&metadata)?;
    manager.record_task_start(run_id, "build", Some("deadbeefcafe"), None, None, None)?;

    let tasks = run_tasks(&manager, run_id)?;
    assert_eq!(tasks[0].script_hash.as_deref(), Some("deadbeefcafe"));
    Ok(())
}

#[test]
fn test_record_run_start() -> Result<()> {
    let (manager, _temp_dir) = create_test_manager()?;

    let metadata = RunMetadata::minimal(Some(PathBuf::from("/test/otto.yml")), "abc123".to_string(), 1234567890);

    let run_id = manager.record_run_start(&metadata)?;
    assert!(run_id > 0);

    Ok(())
}

#[test]
fn test_record_run_complete() -> Result<()> {
    let (manager, _temp_dir) = create_test_manager()?;

    let metadata = RunMetadata::minimal(Some(PathBuf::from("/test/otto.yml")), "abc123".to_string(), 1234567890);

    let run_id_1 = manager.record_run_start(&metadata)?;
    manager.record_run_complete(run_id_1, RunStatus::Success, Some(1024))?;

    let runs = manager.get_runs_with_filters(None, None, 1)?;
    assert_eq!(runs.len(), 1);
    assert_eq!(runs[0].status, RunStatus::Success);
    assert_eq!(runs[0].size_bytes, Some(1024));
    assert!(runs[0].duration_seconds.is_some());

    Ok(())
}

#[test]
fn test_get_runs_with_filters() -> Result<()> {
    let (manager, _temp_dir) = create_test_manager()?;

    for i in 0..5 {
        let metadata = RunMetadata::minimal(
            Some(PathBuf::from("/test/otto.yml")),
            "abc123".to_string(),
            1234567890 + i,
        );
        manager.record_run_start(&metadata)?;
    }

    let runs = manager.get_runs_with_filters(None, None, 3)?;
    assert_eq!(runs.len(), 3);

    assert!(runs[0].timestamp > runs[1].timestamp);
    assert!(runs[1].timestamp > runs[2].timestamp);

    Ok(())
}

#[test]
fn test_get_runs_with_filters_project_filter() -> Result<()> {
    let (manager, _temp_dir) = create_test_manager()?;

    for i in 0..3 {
        let metadata1 = RunMetadata::minimal(
            Some(PathBuf::from("/test/otto.yml")),
            "abc123".to_string(),
            1234567890 + i,
        );
        manager.record_run_start(&metadata1)?;

        let metadata2 = RunMetadata::minimal(
            Some(PathBuf::from("/test/otto.yml")),
            "def456".to_string(),
            1234567890 + i + 100,
        );
        manager.record_run_start(&metadata2)?;
    }

    let runs = manager.get_runs_with_filters(None, Some("abc123"), 10)?;
    assert_eq!(runs.len(), 3);

    // All runs should be for abc123
    // We can verify by checking timestamps match what we inserted
    assert!(runs.iter().all(|r| r.timestamp < 1234567890 + 100));

    Ok(())
}

#[test]
fn test_ensure_project_creates_new() -> Result<()> {
    let (manager, _temp_dir) = create_test_manager()?;

    manager.db.with_connection(|conn| {
        let project_id1 = manager.ensure_project(conn, "test123", Some(&PathBuf::from("/test/otto.yml")))?;
        assert!(project_id1 > 0);

        // Calling again should return same ID
        let project_id2 = manager.ensure_project(conn, "test123", Some(&PathBuf::from("/test/otto.yml")))?;
        assert_eq!(project_id1, project_id2);

        Ok(())
    })
}

#[test]
fn test_full_metadata_recording() -> Result<()> {
    let (manager, _temp_dir) = create_test_manager()?;

    let metadata = RunMetadata::full(
        Some(PathBuf::from("/test/otto.yml")),
        "abc123".to_string(),
        1234567890,
        Some(PathBuf::from("/home/user/project")),
        Some("testuser".to_string()),
        Some("testhost".to_string()),
        Some(vec!["build".to_string(), "test".to_string()]),
    );

    manager.record_run_start(&metadata)?;

    let runs = manager.get_runs_with_filters(None, None, 1)?;
    assert_eq!(runs.len(), 1);
    assert_eq!(runs[0].cwd, Some(PathBuf::from("/home/user/project")));
    assert_eq!(runs[0].user, Some("testuser".to_string()));
    assert_eq!(runs[0].hostname, Some("testhost".to_string()));
    assert_eq!(runs[0].args, Some(vec!["build".to_string(), "test".to_string()]));

    Ok(())
}

#[test]
fn test_try_new_graceful_failure() {
    // Both branches of `try_new`, under an OTTO_HOME this test owns.
    //
    // This used to call `StateManager::try_new()` and throw the result away,
    // with a comment conceding it only "verified it compiles". It asserted
    // nothing, and worse, it read whatever `OTTO_HOME` happened to be set to by
    // whichever other test was running at the time - several `--lib` tests set
    // it process-globally. On a developer machine some other test usually had it
    // pointed somewhere harmless; on a fresh CI runner it did not, so `try_new`
    // resolved to `$HOME/.otto/otto.db` and tripped the guard in `db.rs` that
    // exists precisely to catch a test opening the real database. Green locally,
    // red on the first runner that ever saw it.
    let temp_dir = TempDir::new().unwrap();

    // Usable home -> Some, and the store it opened is the one we named.
    let opened = with_otto_home(temp_dir.path(), StateManager::try_new);
    assert!(opened.is_some(), "try_new must succeed against a writable OTTO_HOME");
    assert!(
        temp_dir.path().join("otto.db").exists(),
        "try_new must have created its database under the OTTO_HOME it was given"
    );

    // Unusable home -> None rather than a panic or a propagated error. The path
    // is a regular file, so the directory underneath the database cannot be
    // created.
    let blocker = TempDir::new().unwrap();
    let not_a_dir = blocker.path().join("occupied");
    std::fs::write(&not_a_dir, b"this is a file, not a directory").unwrap();
    let refused = with_otto_home(&not_a_dir, StateManager::try_new);
    assert!(
        refused.is_none(),
        "try_new must degrade to None when the database cannot be opened, not fail the run"
    );
}

#[test]
fn test_record_task_start() -> Result<()> {
    let (manager, _temp_dir) = create_test_manager()?;

    let metadata = RunMetadata::minimal(Some(PathBuf::from("/test/otto.yml")), "abc123".to_string(), 1234567890);
    let run_id = manager.record_run_start(&metadata)?;

    let task_id = manager.record_task_start(
        run_id,
        "test-task",
        Some("hash123"),
        Some(&PathBuf::from("/tmp/stdout.log")),
        Some(&PathBuf::from("/tmp/stderr.log")),
        Some(&PathBuf::from("/tmp/script.sh")),
    )?;

    assert!(task_id > 0);

    let tasks = run_tasks(&manager, run_id)?;
    assert_eq!(tasks.len(), 1);
    assert_eq!(tasks[0].name, "test-task");
    assert_eq!(tasks[0].status, TaskStatus::Running);
    assert_eq!(tasks[0].script_hash, Some("hash123".to_string()));

    Ok(())
}

#[test]
fn test_record_task_complete() -> Result<()> {
    let (manager, _temp_dir) = create_test_manager()?;

    let metadata = RunMetadata::minimal(Some(PathBuf::from("/test/otto.yml")), "abc123".to_string(), 1234567890);
    let run_id = manager.record_run_start(&metadata)?;

    let task_id = manager.record_task_start(run_id, "test-task", None, None, None, None)?;
    manager.record_task_complete(task_id, 0, TaskStatus::Completed)?;

    let tasks = run_tasks(&manager, run_id)?;
    assert_eq!(tasks.len(), 1);
    assert_eq!(tasks[0].status, TaskStatus::Completed);
    assert_eq!(tasks[0].exit_code, Some(0));
    assert!(tasks[0].ended_at.is_some());
    assert!(tasks[0].duration_seconds.is_some());

    Ok(())
}

#[test]
fn test_record_task_failed() -> Result<()> {
    let (manager, _temp_dir) = create_test_manager()?;

    let metadata = RunMetadata::minimal(Some(PathBuf::from("/test/otto.yml")), "abc123".to_string(), 1234567890);
    let run_id = manager.record_run_start(&metadata)?;

    let task_id = manager.record_task_start(run_id, "test-task", None, None, None, None)?;
    manager.record_task_complete(task_id, 1, TaskStatus::Failed)?;

    let tasks = run_tasks(&manager, run_id)?;
    assert_eq!(tasks.len(), 1);
    assert_eq!(tasks[0].status, TaskStatus::Failed);
    assert_eq!(tasks[0].exit_code, Some(1));

    Ok(())
}

#[test]
fn test_record_task_skipped() -> Result<()> {
    let (manager, _temp_dir) = create_test_manager()?;

    let metadata = RunMetadata::minimal(Some(PathBuf::from("/test/otto.yml")), "abc123".to_string(), 1234567890);
    let run_id = manager.record_run_start(&metadata)?;

    let task_id = manager.record_task_skipped(
        run_id,
        "test-task",
        Some("hash123"),
        Some("dep build failed; this task required when: success"),
        Some(SkipKind::Unreachable),
    )?;
    assert!(task_id > 0);

    let tasks = run_tasks(&manager, run_id)?;
    assert_eq!(tasks.len(), 1);
    assert_eq!(tasks[0].name, "test-task");
    assert_eq!(tasks[0].status, TaskStatus::Skipped);
    assert_eq!(tasks[0].script_hash, Some("hash123".to_string()));
    assert_eq!(
        tasks[0].skip_reason.as_deref(),
        Some("dep build failed; this task required when: success")
    );
    assert_eq!(
        tasks[0].skip_kind,
        Some(SkipKind::Unreachable),
        "the typed kind round-trips through the tasks.skip_kind column"
    );

    Ok(())
}

#[test]
fn tasks_for_a_run_are_ordered_by_started_at() -> Result<()> {
    let (manager, _temp_dir) = create_test_manager()?;

    let metadata = RunMetadata::minimal(Some(PathBuf::from("/test/otto.yml")), "abc123".to_string(), 1234567890);
    let run_id = manager.record_run_start(&metadata)?;

    let task_id1 = manager.record_task_start(run_id, "task-1", None, None, None, None)?;
    let task_id2 = manager.record_task_start(run_id, "task-2", None, None, None, None)?;
    let task_id3 = manager.record_task_start(run_id, "task-3", None, None, None, None)?;

    manager.record_task_complete(task_id1, 0, TaskStatus::Completed)?;
    manager.record_task_complete(task_id2, 1, TaskStatus::Failed)?;
    manager.record_task_complete(task_id3, 0, TaskStatus::Completed)?;

    let tasks = run_tasks(&manager, run_id)?;
    assert_eq!(tasks.len(), 3);

    // Tasks should be ordered by started_at
    assert_eq!(tasks[0].name, "task-1");
    assert_eq!(tasks[1].name, "task-2");
    assert_eq!(tasks[2].name, "task-3");

    Ok(())
}

#[test]
fn test_get_task_history() -> Result<()> {
    let (manager, _temp_dir) = create_test_manager()?;

    for i in 0..5 {
        let metadata = RunMetadata::minimal(
            Some(PathBuf::from("/test/otto.yml")),
            "abc123".to_string(),
            1234567890 + i,
        );
        let run_id = manager.record_run_start(&metadata)?;

        let task_id = manager.record_task_start(run_id, "build", None, None, None, None)?;
        manager.record_task_complete(task_id, 0, TaskStatus::Completed)?;

        std::thread::sleep(std::time::Duration::from_millis(10));
    }

    let history = manager.get_task_history("build", 3)?;
    assert_eq!(history.len(), 3);

    // Should be ordered by started_at descending (newest first)
    // Use >= instead of > since timestamps might be the same in fast execution
    assert!(history[0].started_at >= history[1].started_at);
    assert!(history[1].started_at >= history[2].started_at);

    // All should be the same task name
    assert!(history.iter().all(|t| t.name == "build"));

    Ok(())
}

#[test]
fn test_task_with_all_fields() -> Result<()> {
    let (manager, _temp_dir) = create_test_manager()?;

    let metadata = RunMetadata::minimal(Some(PathBuf::from("/test/otto.yml")), "abc123".to_string(), 1234567890);
    let run_id = manager.record_run_start(&metadata)?;

    let task_id = manager.record_task_start(
        run_id,
        "complex-task",
        Some("script_hash_123"),
        Some(&PathBuf::from("/tmp/stdout.log")),
        Some(&PathBuf::from("/tmp/stderr.log")),
        Some(&PathBuf::from("/tmp/script.sh")),
    )?;

    manager.record_task_complete(task_id, 0, TaskStatus::Completed)?;

    let tasks = run_tasks(&manager, run_id)?;
    assert_eq!(tasks.len(), 1);

    let task = &tasks[0];
    assert_eq!(task.name, "complex-task");
    assert_eq!(task.script_hash, Some("script_hash_123".to_string()));
    assert_eq!(task.stdout_path, Some(PathBuf::from("/tmp/stdout.log")));
    assert_eq!(task.stderr_path, Some(PathBuf::from("/tmp/stderr.log")));
    assert_eq!(task.script_path, Some(PathBuf::from("/tmp/script.sh")));
    assert_eq!(task.exit_code, Some(0));
    assert!(task.started_at.is_some());
    assert!(task.ended_at.is_some());
    assert!(task.duration_seconds.is_some());

    Ok(())
}

#[test]
fn test_find_old_runs_basic() -> Result<()> {
    let (manager, _temp_dir) = create_test_manager()?;

    let now = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH)?.as_secs();

    let old_timestamp = now - (40 * 24 * 60 * 60); // 40 days old
    let recent_timestamp = now - (10 * 24 * 60 * 60); // 10 days old

    let metadata1 = RunMetadata::minimal(
        Some(PathBuf::from("/test/otto.yml")),
        "abc123".to_string(),
        old_timestamp,
    );
    let run_id_2 = manager.record_run_start(&metadata1)?;
    manager.record_run_complete(run_id_2, RunStatus::Success, Some(1024))?;

    let metadata2 = RunMetadata::minimal(
        Some(PathBuf::from("/test/otto.yml")),
        "abc123".to_string(),
        recent_timestamp,
    );
    let run_id_3 = manager.record_run_start(&metadata2)?;
    manager.record_run_complete(run_id_3, RunStatus::Success, Some(2048))?;

    // Find runs older than 30 days
    let old_runs = manager.find_old_runs(30, None, None, None)?;

    assert_eq!(old_runs.len(), 1);
    assert_eq!(old_runs[0].timestamp, old_timestamp);

    Ok(())
}

#[test]
fn test_find_old_runs_with_keep_last() -> Result<()> {
    let (manager, _temp_dir) = create_test_manager()?;

    let now = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH)?.as_secs();

    for i in 0..5 {
        let timestamp = now - ((40 + i) * 24 * 60 * 60);
        let metadata = RunMetadata::minimal(Some(PathBuf::from("/test/otto.yml")), "abc123".to_string(), timestamp);
        let run_id_4 = manager.record_run_start(&metadata)?;
        manager.record_run_complete(run_id_4, RunStatus::Success, Some(1024))?;
    }

    // Find old runs but keep the 2 most recent
    let old_runs = manager.find_old_runs(30, Some(2), None, None)?;

    // Should only return 3 runs (5 - 2 kept)
    assert_eq!(old_runs.len(), 3);

    // The oldest runs should be returned
    assert!(old_runs[0].timestamp < old_runs[1].timestamp);
    assert!(old_runs[1].timestamp < old_runs[2].timestamp);

    Ok(())
}

#[test]
fn test_find_old_runs_with_keep_failed() -> Result<()> {
    let (manager, _temp_dir) = create_test_manager()?;

    let now = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH)?.as_secs();

    let success_timestamp = now - (40 * 24 * 60 * 60);
    let metadata1 = RunMetadata::minimal(
        Some(PathBuf::from("/test/otto.yml")),
        "abc123".to_string(),
        success_timestamp,
    );
    let run_id_5 = manager.record_run_start(&metadata1)?;
    manager.record_run_complete(run_id_5, RunStatus::Success, Some(1024))?;

    let failed_timestamp = now - (39 * 24 * 60 * 60);
    let metadata2 = RunMetadata::minimal(
        Some(PathBuf::from("/test/otto.yml")),
        "abc123".to_string(),
        failed_timestamp,
    );
    let run_id_6 = manager.record_run_start(&metadata2)?;
    manager.record_run_complete(run_id_6, RunStatus::Failed, Some(2048))?;

    // Find runs older than 30 days, but keep failed runs for 45 days
    let old_runs = manager.find_old_runs(30, None, Some(45), None)?;

    // Should only return the successful run (failed run kept longer)
    assert_eq!(old_runs.len(), 1);
    assert_eq!(old_runs[0].timestamp, success_timestamp);
    assert_eq!(old_runs[0].status, RunStatus::Success);

    Ok(())
}

#[test]
fn test_find_old_runs_with_project_filter() -> Result<()> {
    let (manager, _temp_dir) = create_test_manager()?;

    let now = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH)?.as_secs();
    let old_timestamp = now - (40 * 24 * 60 * 60);

    let metadata1 = RunMetadata::minimal(
        Some(PathBuf::from("/test/otto.yml")),
        "abc123".to_string(),
        old_timestamp,
    );
    let run_id_7 = manager.record_run_start(&metadata1)?;
    manager.record_run_complete(run_id_7, RunStatus::Success, Some(1024))?;

    let metadata2 = RunMetadata::minimal(
        Some(PathBuf::from("/test/otto2.yml")),
        "def456".to_string(),
        old_timestamp + 1,
    );
    let run_id_8 = manager.record_run_start(&metadata2)?;
    manager.record_run_complete(run_id_8, RunStatus::Success, Some(2048))?;

    // Find old runs for specific project
    let old_runs = manager.find_old_runs(30, None, None, Some("abc123"))?;

    assert_eq!(old_runs.len(), 1);
    assert_eq!(old_runs[0].timestamp, old_timestamp);

    Ok(())
}

#[test]
fn test_delete_run_database_only() -> Result<()> {
    let (manager, _temp_dir) = create_test_manager()?;

    let metadata = RunMetadata::minimal(Some(PathBuf::from("/test/otto.yml")), "abc123".to_string(), 1234567890);
    let run_id_9 = manager.record_run_start(&metadata)?;
    manager.record_run_complete(run_id_9, RunStatus::Success, Some(1024))?;

    let runs_before = manager.get_runs_with_filters(None, None, 10)?;
    assert_eq!(runs_before.len(), 1);

    let deleted = manager.delete_run(run_id_9, false)?;
    assert!(deleted.is_some());
    assert_eq!(deleted.unwrap().timestamp, 1234567890);

    let runs_after = manager.get_runs_with_filters(None, None, 10)?;
    assert_eq!(runs_after.len(), 0);

    Ok(())
}

#[test]
fn test_delete_run_with_tasks() -> Result<()> {
    let (manager, _temp_dir) = create_test_manager()?;

    let metadata = RunMetadata::minimal(Some(PathBuf::from("/test/otto.yml")), "abc123".to_string(), 1234567890);
    let run_id = manager.record_run_start(&metadata)?;

    let task_id1 = manager.record_task_start(run_id, "task1", None, None, None, None)?;
    manager.record_task_complete(task_id1, 0, TaskStatus::Completed)?;

    let task_id2 = manager.record_task_start(run_id, "task2", None, None, None, None)?;
    manager.record_task_complete(task_id2, 1, TaskStatus::Failed)?;

    let tasks_before = run_tasks(&manager, run_id)?;
    assert_eq!(tasks_before.len(), 2);

    manager.delete_run(run_id, false)?;

    let tasks_after = run_tasks(&manager, run_id)?;
    assert_eq!(tasks_after.len(), 0);

    Ok(())
}

/// The counter and the rows move together, in both directions: three starts
/// then one delete leaves two rows AND a `run_count` of two. The test used to
/// assert only the row count, so the counter this test is named for was never
/// read.
#[test]
fn test_delete_run_updates_project_count() -> Result<()> {
    let (manager, _temp_dir) = create_test_manager()?;

    let mut run_ids = Vec::new();
    for i in 0..3 {
        let metadata = RunMetadata::minimal(
            Some(PathBuf::from("/test/otto.yml")),
            "abc123".to_string(),
            1234567890 + i,
        );
        run_ids.push(manager.record_run_start(&metadata)?);
    }
    assert_eq!(manager.get_all_projects()?[0].run_count, 3);

    manager.delete_run(run_ids[1], false)?;

    let runs = manager.get_runs_with_filters(None, Some("abc123"), 10)?;
    assert_eq!(runs.len(), 2);
    assert_eq!(
        manager.get_all_projects()?[0].run_count,
        2,
        "run_count must track the rows, not drift away from them"
    );

    Ok(())
}

#[test]
fn test_delete_nonexistent_run() -> Result<()> {
    let (manager, _temp_dir) = create_test_manager()?;

    // Try to delete a run that doesn't exist
    let deleted = manager.delete_run(9999, false)?;
    assert!(deleted.is_none());

    Ok(())
}

#[test]
fn test_find_old_runs_empty_database() -> Result<()> {
    let (manager, _temp_dir) = create_test_manager()?;

    // Find old runs in empty database
    let old_runs = manager.find_old_runs(30, None, None, None)?;
    assert_eq!(old_runs.len(), 0);

    Ok(())
}

#[test]
fn test_find_old_runs_all_recent() -> Result<()> {
    let (manager, _temp_dir) = create_test_manager()?;

    let now = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH)?.as_secs();
    let recent_timestamp = now - (5 * 24 * 60 * 60); // 5 days old

    let metadata = RunMetadata::minimal(
        Some(PathBuf::from("/test/otto.yml")),
        "abc123".to_string(),
        recent_timestamp,
    );
    let run_id_10 = manager.record_run_start(&metadata)?;
    manager.record_run_complete(run_id_10, RunStatus::Success, Some(1024))?;

    // Find runs older than 30 days (should find nothing)
    let old_runs = manager.find_old_runs(30, None, None, None)?;
    assert_eq!(old_runs.len(), 0);

    Ok(())
}

#[test]
fn test_find_old_runs_complex_policy() -> Result<()> {
    let (manager, _temp_dir) = create_test_manager()?;

    let now = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH)?.as_secs();

    for i in 0..10 {
        let timestamp = now - ((40 + i) * 24 * 60 * 60);
        let metadata = RunMetadata::minimal(Some(PathBuf::from("/test/otto.yml")), "abc123".to_string(), timestamp);
        let run_id = manager.record_run_start(&metadata)?;
        let status = if i % 2 == 0 { RunStatus::Success } else { RunStatus::Failed };
        manager.record_run_complete(run_id, status, Some(1024))?;
    }

    // Keep 3 most recent, delete successful runs older than 30 days, keep failed runs for 50 days
    let old_runs = manager.find_old_runs(30, Some(3), Some(50), None)?;

    // The fixture's answer is exactly 3, not "at most 7". `keep_last` protects
    // the newest three (40, 41 and 42 days old); of the remaining seven, the
    // four failed runs are 43 to 49 days old and `keep_failed_days = 50` keeps
    // every one of them, leaving the successful runs at 44, 46 and 48 days.
    // `<= 7` was also true of an empty answer, so this test passed with the
    // policy returning nothing at all.
    let ages_returned: Vec<u64> = old_runs.iter().map(|r| (now - r.timestamp) / (24 * 60 * 60)).collect();
    assert_eq!(
        ages_returned,
        vec![48, 46, 44],
        "unexpected expiry set: {ages_returned:?}"
    );
    assert!(
        old_runs.iter().all(|r| r.status == RunStatus::Success),
        "no failed run is older than keep_failed_days, so none may expire"
    );

    Ok(())
}

/// A row whose `args` column is not valid JSON must surface a named error,
/// not panic and not silently drop the row.
#[test]
fn get_runs_with_filters_reports_corrupt_args_json_rather_than_panicking() -> Result<()> {
    let (manager, _temp_dir) = create_test_manager()?;

    let metadata = RunMetadata::minimal(
        Some(PathBuf::from("/test/otto.yml")),
        "abc12345".to_string(),
        1234567890,
    );
    let run_id = manager.record_run_start(&metadata)?;
    manager.db.with_connection(|conn| {
        Ok(conn.execute(
            "UPDATE runs SET args = 'not json at all' WHERE id = ?1",
            rusqlite::params![run_id],
        )?)
    })?;

    let err = manager
        .get_runs_with_filters(None, None, 10)
        .expect_err("corrupt args JSON must be a named error, not a panic or a silently empty list");
    // eyre's plain Display shows only the outermost context; `{:#}` walks
    // the full chain down to the actual `bad_column` message.
    assert!(
        format!("{err:#}").contains("run args are not a JSON string list"),
        "{err:#}"
    );

    Ok(())
}

/// A row whose `status` column holds a value no `RunStatus` variant parses
/// must surface a named error, not panic and not silently drop the row.
#[test]
fn get_runs_with_filters_reports_an_unknown_status_rather_than_panicking() -> Result<()> {
    let (manager, _temp_dir) = create_test_manager()?;

    let metadata = RunMetadata::minimal(
        Some(PathBuf::from("/test/otto.yml")),
        "abc12345".to_string(),
        1234567890,
    );
    let run_id = manager.record_run_start(&metadata)?;
    manager.db.with_connection(|conn| {
        Ok(conn.execute(
            "UPDATE runs SET status = 'this-is-not-a-status' WHERE id = ?1",
            rusqlite::params![run_id],
        )?)
    })?;

    let err = manager
        .get_runs_with_filters(None, None, 10)
        .expect_err("an unrecognized status must be a named error, not a panic or a silently empty list");
    assert!(format!("{err:#}").contains("unknown run status"), "{err:#}");

    Ok(())
}

/// Two-process concurrency, proxied by separate `StateManager` connections
/// (each opens its own SQLite handle, exactly as two real `otto` processes
/// would) pointed at the same database file. `ensure_project`'s upsert plus
/// WAL mode and a 5-second busy timeout (`src/executor/state/db.rs`) must
/// make this safe: no thread should see `SQLITE_BUSY` propagate as an error,
/// there must be exactly one project row for the shared hash, and every
/// concurrently-started run must persist as its own row.
#[test]
fn concurrent_connections_racing_ensure_project_stay_consistent() -> Result<()> {
    let temp_dir = TempDir::new()?;
    let db_path = temp_dir.path().join("concurrent.db");
    // Pre-create the schema on one connection so every thread below is
    // racing `ensure_project`/`record_run_start`, not schema creation too.
    drop(StateManager::with_db_path(db_path.clone())?);

    let thread_count = 8usize;
    let handles: Vec<_> = (0..thread_count)
        .map(|i| {
            let db_path = db_path.clone();
            std::thread::spawn(move || -> Result<i64> {
                let manager = StateManager::with_db_path(db_path)?;
                let metadata = RunMetadata::minimal(
                    Some(PathBuf::from("/repos/widget/otto.yml")),
                    "sharedhash".to_string(),
                    // Distinct timestamps: this also doubles as a same-second
                    // stress test once threads interleave.
                    1_700_000_000 + i as u64,
                );
                manager.record_run_start(&metadata)
            })
        })
        .collect();

    let mut run_ids = Vec::new();
    for handle in handles {
        let run_id = handle
            .join()
            .expect("thread must not panic")
            .expect("record_run_start must not surface SQLITE_BUSY under WAL + busy_timeout");
        run_ids.push(run_id);
    }

    run_ids.sort_unstable();
    run_ids.dedup();
    assert_eq!(
        run_ids.len(),
        thread_count,
        "every concurrent run must get its own distinct row"
    );

    let manager = StateManager::with_db_path(db_path)?;
    let projects = manager.get_all_projects()?;
    let matching: Vec<_> = projects.iter().filter(|p| p.hash == "sharedhash").collect();
    assert_eq!(
        matching.len(),
        1,
        "ensure_project's upsert must leave exactly one row for the shared hash, not one per racing thread"
    );
    assert_eq!(matching[0].run_count as usize, thread_count);

    Ok(())
}
