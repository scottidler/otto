#![cfg(test)]

use super::*;
use crate::executor::state::RunStatus;
use crate::executor::workspace::{ExecutionContext, Workspace};
use crate::ports::{MemoryStateStore, RealFs};
use std::fs;
use tempfile::TempDir;

/// A run directory shaped exactly like one `Workspace` creates:
/// `<otto home>/<name>-<hash>/<timestamp>/tasks`.
fn create_test_run(base_dir: &Path, project_hash: &str, timestamp: u64, size_kb: usize) -> Result<()> {
    let run_dir = base_dir
        .join(crate::executor::layout::project_dir_name("widget", project_hash))
        .join(timestamp.to_string())
        .join("tasks");
    fs::create_dir_all(&run_dir)?;

    let file_path = run_dir.join("test.log");
    let content = vec![0u8; size_kb * 1024];
    fs::write(file_path, content)?;

    Ok(())
}

#[tokio::test]
async fn test_scan_empty_directory() -> Result<()> {
    let temp_dir = TempDir::new()?;
    let cmd = CleanCommand {
        keep_days: 30,
        keep_last: None,
        keep_failed: None,
        dry_run: true,
        project_filter: None,
        no_db: true,
        quiet: false,
        otto_home: None,
    };

    let runs = cmd.scan_runs(temp_dir.path(), now_timestamp())?;
    assert_eq!(runs.len(), 0);
    Ok(())
}

/// Reproduced under a scratch HOME: a symlinked project directory under
/// `~/.otto` reported `Deleted [deadbeef] ...`, the real directory it pointed
/// at was gone, and the symlink was still there. `is_dir()` follows links.
#[tokio::test]
async fn clean_never_deletes_through_a_symlinked_project_directory() -> Result<()> {
    let temp_dir = TempDir::new()?;
    let otto_home = temp_dir.path().join(".otto");
    fs::create_dir_all(&otto_home)?;

    let victim = temp_dir.path().join("victim-project");
    fs::create_dir_all(victim.join("1000000000").join("tasks"))?;
    fs::write(victim.join("1000000000").join("tasks").join("data.log"), "precious")?;

    std::os::unix::fs::symlink(&victim, otto_home.join("widget-deadbeef"))?;

    let cmd = CleanCommand {
        keep_days: 1,
        keep_last: None,
        keep_failed: None,
        dry_run: false,
        project_filter: None,
        no_db: true,
        quiet: true,
        otto_home: None,
    };

    cmd.execute_with_filesystem(&otto_home).await?;

    assert!(
        victim.join("1000000000").join("tasks").join("data.log").exists(),
        "the symlink target must survive"
    );
    assert!(
        otto_home.join("widget-deadbeef").exists(),
        "the link itself is left alone"
    );
    Ok(())
}

/// Same rule one level down: a symlinked *run* directory inside a real
/// project directory is not a deletion candidate either.
#[tokio::test]
async fn clean_never_deletes_through_a_symlinked_run_directory() -> Result<()> {
    let temp_dir = TempDir::new()?;
    let otto_home = temp_dir.path().join(".otto");
    let project = otto_home.join("widget-abc12345");
    fs::create_dir_all(&project)?;

    let victim = temp_dir.path().join("victim");
    fs::create_dir_all(&victim)?;
    fs::write(victim.join("precious.txt"), "keep me")?;

    std::os::unix::fs::symlink(&victim, project.join("1000000000"))?;

    let cmd = CleanCommand {
        keep_days: 1,
        keep_last: None,
        keep_failed: None,
        dry_run: false,
        project_filter: None,
        no_db: true,
        quiet: true,
        otto_home: None,
    };

    cmd.execute_with_filesystem(&otto_home).await?;

    assert!(victim.join("precious.txt").exists(), "the symlink target must survive");
    Ok(())
}

#[tokio::test]
async fn test_scan_with_old_runs() -> Result<()> {
    let temp_dir = TempDir::new()?;

    let now = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH)?.as_secs();
    let old_timestamp = now - (40 * 86400); // 40 days old
    let recent_timestamp = now - (10 * 86400); // 10 days old

    create_test_run(temp_dir.path(), "abc12345", old_timestamp, 100)?;
    create_test_run(temp_dir.path(), "abc12345", recent_timestamp, 50)?;

    let cmd = CleanCommand {
        keep_days: 30,
        keep_last: None,
        keep_failed: None,
        dry_run: true,
        project_filter: None,
        no_db: true,
        quiet: false,
        otto_home: None,
    };
    // The scan reports every run; retention decides which ones expire, so
    // `--keep-last` can see the recent runs it is supposed to protect.
    let mut runs = cmd.scan_runs(temp_dir.path(), now_timestamp())?;
    runs.sort_by_key(|r| r.timestamp);

    assert_eq!(runs.len(), 2, "both runs are reported by the scan");
    assert!(runs[0].age_days >= 39 && runs[0].age_days <= 41);
    assert!(runs[1].age_days >= 9 && runs[1].age_days <= 11);
    Ok(())
}

#[tokio::test]
async fn test_directory_size_of_a_run() -> Result<()> {
    let temp_dir = TempDir::new()?;
    create_test_run(temp_dir.path(), "testhash", 1234567890, 100)?;

    let run_dir = temp_dir.path().join("widget-testhash").join("1234567890");
    let size = directory_size(&run_dir)?;

    // Should be approximately 100KB (may vary slightly due to filesystem overhead)
    assert!(size >= 100 * 1024);
    assert!(size < 110 * 1024);
    Ok(())
}

/// AC8: the `Clean` caller and the `Workspace` caller report the **same** size
/// for the same run directory, and that size excludes the 1 MB file a symlink
/// inside the run points at.
///
/// This drives both production callers, not the shared function twice: `Clean`
/// through `scan_runs`, `Workspace` through the size it records at run
/// completion. They used to carry separate implementations that disagreed on
/// symlinks, so the same directory had two sizes depending on who asked.
#[tokio::test]
#[serial_test::serial]
async fn both_callers_report_one_size_that_excludes_a_symlink_target() -> Result<()> {
    let temp_dir = TempDir::new()?;
    let otto_home = temp_dir.path().join("otto-home");
    let outside = temp_dir.path().join("outside");
    fs::create_dir_all(&outside)?;
    fs::write(outside.join("blob"), vec![0u8; 1024 * 1024])?;

    unsafe {
        std::env::set_var("OTTO_HOME", &otto_home);
    }

    let store = Arc::new(MemoryStateStore::new()) as Arc<dyn StateStore>;
    let workspace = Workspace::new_with_hash_and_fs(
        temp_dir.path().join("widget"),
        "widget".to_string(),
        "abc12345".to_string(),
        Arc::new(RealFs),
    )
    .await?
    .with_state_store(Arc::clone(&store));
    workspace.init().await?;

    // Everything the run owns is in place before either caller measures, so a
    // difference between them can only come from how they measure.
    workspace.save_execution_context(ExecutionContext::new()).await?;
    fs::write(workspace.run().join("tasks").join("stdout.log"), vec![0u8; 100 * 1024])?;
    // Two links to the same 1 MB byte: one straight at the file, and one at the
    // directory holding it, which is the shape a run's `.cache` link actually
    // has. A size function that follows either one charges the shared blob to
    // this run.
    std::os::unix::fs::symlink(outside.join("blob"), workspace.run().join("blob"))?;
    std::os::unix::fs::symlink(&outside, workspace.run().join("cache"))?;

    workspace.record_run_complete_in_db(true).await;
    let rows = store.get_runs_with_filters(None, None, 10)?;
    assert_eq!(rows.len(), 1, "the run was recorded once");
    let workspace_size = rows[0].size_bytes.expect("run completion records a size");

    // The run is over, so it lets go of its directory. Dropping the workspace
    // is what a finished otto process does, and the scan below skips any
    // directory whose run lock is still held.
    let run_dir = workspace.run().clone();
    drop(workspace);

    let cmd = CleanCommand {
        keep_days: 30,
        keep_last: None,
        keep_failed: None,
        dry_run: true,
        project_filter: None,
        no_db: true,
        quiet: true,
        otto_home: None,
    };
    let runs = cmd.scan_runs(&otto_home, now_timestamp())?;
    unsafe {
        std::env::remove_var("OTTO_HOME");
    }
    let scanned = runs
        .iter()
        .find(|r| r.path == run_dir)
        .expect("Clean's scan finds the run directory");

    assert_eq!(
        scanned.size_bytes, workspace_size,
        "one implementation, so one number for one directory"
    );
    assert!(
        workspace_size >= 100 * 1024,
        "the run's own 100 KB is counted: {workspace_size}"
    );
    assert!(
        workspace_size < 1024 * 1024,
        "the 1 MB symlink target is not counted: {workspace_size}"
    );
    Ok(())
}

#[tokio::test]
async fn test_project_filter() -> Result<()> {
    let temp_dir = TempDir::new()?;
    let now = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH)?.as_secs();
    let old_timestamp = now - (40 * 86400);

    create_test_run(temp_dir.path(), "abc12345", old_timestamp, 100)?;
    create_test_run(temp_dir.path(), "def45678", old_timestamp, 100)?;

    let cmd = CleanCommand {
        keep_days: 30,
        keep_last: None,
        keep_failed: None,
        dry_run: true,
        project_filter: Some("abc12345".to_string()),
        no_db: true,
        quiet: false,
        otto_home: None,
    };
    let runs = cmd.scan_runs(temp_dir.path(), now_timestamp())?;

    // Should only find runs from abc123 project
    assert_eq!(runs.len(), 1);
    assert!(runs[0].path.to_string_lossy().contains("abc12345"));
    Ok(())
}

#[tokio::test]
async fn test_read_ottofile_path_with_metadata() -> Result<()> {
    let temp_dir = TempDir::new()?;
    let run_dir = temp_dir.path().join("test_run");
    fs::create_dir_all(&run_dir)?;

    let metadata = RunMetadata::minimal(
        Some(PathBuf::from("/path/to/otto.yml")),
        "abc12345".to_string(),
        1234567890,
    );
    let yaml_content = yaml_serde::to_string(&metadata)?;
    fs::write(run_dir.join("run.yaml"), yaml_content)?;

    let cmd = CleanCommand {
        keep_days: 30,
        keep_last: None,
        keep_failed: None,
        dry_run: true,
        project_filter: None,
        no_db: true,
        quiet: false,
        otto_home: None,
    };
    let ottofile_path = cmd.read_ottofile_path(&run_dir);

    assert_eq!(ottofile_path, Some(PathBuf::from("/path/to/otto.yml")));
    Ok(())
}

#[tokio::test]
async fn test_read_ottofile_path_missing_file() -> Result<()> {
    let temp_dir = TempDir::new()?;
    let run_dir = temp_dir.path().join("test_run");
    fs::create_dir_all(&run_dir)?;

    let cmd = CleanCommand {
        keep_days: 30,
        keep_last: None,
        keep_failed: None,
        dry_run: true,
        project_filter: None,
        no_db: true,
        quiet: false,
        otto_home: None,
    };
    let ottofile_path = cmd.read_ottofile_path(&run_dir);

    assert_eq!(ottofile_path, None);
    Ok(())
}

#[tokio::test]
async fn test_read_ottofile_path_no_ottofile_field() -> Result<()> {
    let temp_dir = TempDir::new()?;
    let run_dir = temp_dir.path().join("test_run");
    fs::create_dir_all(&run_dir)?;

    let metadata = RunMetadata::minimal(None, "abc12345".to_string(), 1234567890);
    let yaml_content = yaml_serde::to_string(&metadata)?;
    fs::write(run_dir.join("run.yaml"), yaml_content)?;

    let cmd = CleanCommand {
        keep_days: 30,
        keep_last: None,
        keep_failed: None,
        dry_run: true,
        project_filter: None,
        no_db: true,
        quiet: false,
        otto_home: None,
    };
    let ottofile_path = cmd.read_ottofile_path(&run_dir);

    assert_eq!(ottofile_path, None);
    Ok(())
}

#[tokio::test]
async fn test_read_ottofile_path_malformed_yaml() -> Result<()> {
    let temp_dir = TempDir::new()?;
    let run_dir = temp_dir.path().join("test_run");
    fs::create_dir_all(&run_dir)?;

    fs::write(run_dir.join("run.yaml"), "invalid: yaml: content: {")?;

    let cmd = CleanCommand {
        keep_days: 30,
        keep_last: None,
        keep_failed: None,
        dry_run: true,
        project_filter: None,
        no_db: true,
        quiet: false,
        otto_home: None,
    };
    let ottofile_path = cmd.read_ottofile_path(&run_dir);

    assert_eq!(ottofile_path, None);
    Ok(())
}

#[tokio::test]
async fn test_runs_sorted_by_timestamp() -> Result<()> {
    let temp_dir = TempDir::new()?;
    let now = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH)?.as_secs();

    let timestamp1 = now - (60 * 86400); // 60 days old
    let timestamp2 = now - (45 * 86400); // 45 days old
    let timestamp3 = now - (50 * 86400); // 50 days old

    create_test_run(temp_dir.path(), "abc12345", timestamp2, 100)?;
    create_test_run(temp_dir.path(), "abc12345", timestamp1, 100)?;
    create_test_run(temp_dir.path(), "abc12345", timestamp3, 100)?;

    let cmd = CleanCommand {
        keep_days: 30,
        keep_last: None,
        keep_failed: None,
        dry_run: true,
        project_filter: None,
        no_db: true,
        quiet: false,
        otto_home: None,
    };
    let mut runs = cmd.scan_runs(temp_dir.path(), now_timestamp())?;

    // Sort by timestamp
    runs.sort_by_key(|r| r.timestamp);

    // Should be sorted oldest to newest
    assert_eq!(runs.len(), 3);
    assert_eq!(runs[0].timestamp, timestamp1);
    assert_eq!(runs[1].timestamp, timestamp3);
    assert_eq!(runs[2].timestamp, timestamp2);
    Ok(())
}

#[tokio::test]
async fn test_scan_with_ottofile_metadata() -> Result<()> {
    let temp_dir = TempDir::new()?;
    let now = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH)?.as_secs();
    let old_timestamp = now - (40 * 86400);

    let project_dir = temp_dir.path().join("widget-abc12345");
    let run_dir = project_dir.join(old_timestamp.to_string());
    let tasks_dir = run_dir.join("tasks");
    fs::create_dir_all(&tasks_dir)?;

    let file_path = tasks_dir.join("test.log");
    let content = vec![0u8; 100 * 1024];
    fs::write(file_path, content)?;

    let metadata = RunMetadata::minimal(
        Some(PathBuf::from("/test/project/otto.yml")),
        "abc12345".to_string(),
        old_timestamp,
    );
    let yaml_content = yaml_serde::to_string(&metadata)?;
    fs::write(run_dir.join("run.yaml"), yaml_content)?;

    let cmd = CleanCommand {
        keep_days: 30,
        keep_last: None,
        keep_failed: None,
        dry_run: true,
        project_filter: None,
        no_db: true,
        quiet: false,
        otto_home: None,
    };
    let runs = cmd.scan_runs(temp_dir.path(), now_timestamp())?;

    assert_eq!(runs.len(), 1);
    assert_eq!(runs[0].project_hash, "abc12345");
    assert_eq!(runs[0].timestamp, old_timestamp);
    assert_eq!(runs[0].ottofile_path, Some(PathBuf::from("/test/project/otto.yml")));
    assert!(runs[0].size_bytes >= 100 * 1024);
    Ok(())
}

#[tokio::test]
async fn test_scan_without_ottofile_metadata() -> Result<()> {
    let temp_dir = TempDir::new()?;
    let now = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH)?.as_secs();
    let old_timestamp = now - (40 * 86400);

    create_test_run(temp_dir.path(), "def45678", old_timestamp, 100)?;

    let cmd = CleanCommand {
        keep_days: 30,
        keep_last: None,
        keep_failed: None,
        dry_run: true,
        project_filter: None,
        no_db: true,
        quiet: false,
        otto_home: None,
    };
    let runs = cmd.scan_runs(temp_dir.path(), now_timestamp())?;

    assert_eq!(runs.len(), 1);
    assert_eq!(runs[0].project_hash, "def45678");
    assert_eq!(runs[0].ottofile_path, None);
    Ok(())
}

/// `--keep-last 2` in filesystem mode keeps the two *newest* runs.
///
/// It kept the two oldest: the candidate list was sorted ascending and then
/// `split_off(len - keep_last)` returned the tail, so the delete list *was*
/// the runs the flag was meant to protect. Every `keep_last` test in this
/// file was database-mode, which is why it shipped.
#[tokio::test]
async fn keep_last_in_filesystem_mode_keeps_the_newest_runs() -> Result<()> {
    let temp_dir = TempDir::new()?;
    let otto_home = temp_dir.path().join(".otto");
    fs::create_dir_all(&otto_home)?;
    let now = now_timestamp();

    // Five runs, 44 down to 40 days old, all past the 30-day cutoff.
    let timestamps: Vec<u64> = (0..5).map(|i| now - ((44 - i) * 86400)).collect();
    for ts in &timestamps {
        create_test_run(&otto_home, "abc12345", *ts, 1)?;
    }

    let cmd = CleanCommand {
        keep_days: 30,
        keep_last: Some(2),
        keep_failed: None,
        dry_run: false,
        project_filter: None,
        no_db: true,
        quiet: true,
        otto_home: None,
    };
    cmd.execute_with_filesystem(&otto_home).await?;

    let project = otto_home.join(crate::executor::layout::project_dir_name("widget", "abc12345"));
    let survivors: Vec<u64> = {
        let mut found: Vec<u64> = fs::read_dir(&project)?
            .filter_map(|e| e.ok())
            .filter_map(|e| e.file_name().to_str().and_then(|n| n.parse::<u64>().ok()))
            .collect();
        found.sort_unstable();
        found
    };

    assert_eq!(
        survivors,
        vec![timestamps[3], timestamps[4]],
        "the two newest runs survive, not the two oldest"
    );
    Ok(())
}

/// `--keep-last` larger than the run count deletes nothing.
#[tokio::test]
async fn keep_last_larger_than_the_run_count_deletes_nothing() -> Result<()> {
    let temp_dir = TempDir::new()?;
    let otto_home = temp_dir.path().join(".otto");
    fs::create_dir_all(&otto_home)?;
    let now = now_timestamp();

    for i in 0..2u64 {
        create_test_run(&otto_home, "abc12345", now - ((40 + i) * 86400), 1)?;
    }

    let cmd = CleanCommand {
        keep_days: 30,
        keep_last: Some(9),
        keep_failed: None,
        dry_run: false,
        project_filter: None,
        no_db: true,
        quiet: true,
        otto_home: None,
    };
    cmd.execute_with_filesystem(&otto_home).await?;

    let project = otto_home.join(crate::executor::layout::project_dir_name("widget", "abc12345"));
    assert_eq!(fs::read_dir(&project)?.count(), 2, "nothing is deleted");
    Ok(())
}

/// The filesystem scan finds every project, not only ones named "otto".
/// It tested for an `otto-` prefix, which in a real `~/.otto` matched 2 of
/// 222 directories.
#[tokio::test]
async fn filesystem_scan_finds_projects_not_named_otto() -> Result<()> {
    let temp_dir = TempDir::new()?;
    let now = now_timestamp();
    let old = now - (40 * 86400);

    create_test_run(temp_dir.path(), "abc12345", old, 1)?;
    // A stray directory that is not a run root must be ignored.
    fs::create_dir_all(temp_dir.path().join("notes"))?;

    let cmd = CleanCommand {
        keep_days: 30,
        keep_last: None,
        keep_failed: None,
        dry_run: true,
        project_filter: None,
        no_db: true,
        quiet: true,
        otto_home: None,
    };
    let runs = cmd.scan_runs(temp_dir.path(), now)?;

    assert_eq!(runs.len(), 1);
    assert_eq!(runs[0].project_hash, "abc12345");
    Ok(())
}

/// The filesystem filter matches the hash exactly, as the database filter
/// does. A substring match swept up every project whose hash merely
/// contained the filter.
#[tokio::test]
async fn filesystem_project_filter_matches_the_hash_exactly() -> Result<()> {
    let temp_dir = TempDir::new()?;
    let now = now_timestamp();
    let old = now - (40 * 86400);

    create_test_run(temp_dir.path(), "abc12345", old, 1)?;
    create_test_run(temp_dir.path(), "abc12340", old + 1, 1)?;

    let cmd = CleanCommand {
        keep_days: 30,
        keep_last: None,
        keep_failed: None,
        dry_run: true,
        project_filter: Some("abc1234".to_string()),
        no_db: true,
        quiet: true,
        otto_home: None,
    };
    let runs = cmd.scan_runs(temp_dir.path(), now)?;

    assert!(runs.is_empty(), "a prefix is not a hash");
    Ok(())
}

// ========================================
// Database-based cleanup tests using MemoryStateStore
// ========================================

fn now_timestamp() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::SystemTime::UNIX_EPOCH)
        .unwrap()
        .as_secs()
}

fn create_store_with_runs() -> Arc<MemoryStateStore> {
    let store = MemoryStateStore::new();
    let now = now_timestamp();

    // Create old successful run (40 days old)
    let old_meta = RunMetadata::minimal(
        Some(PathBuf::from("/project1/otto.yml")),
        "abc12345".to_string(),
        now - (40 * 86400),
    );
    let old_id = store.record_run_start(&old_meta).unwrap();
    store
        .record_run_complete(old_id, RunStatus::Success, Some(100_000))
        .unwrap();

    // Create old failed run (35 days old)
    let old_failed_meta = RunMetadata::minimal(
        Some(PathBuf::from("/project1/otto.yml")),
        "abc12345".to_string(),
        now - (35 * 86400),
    );
    let old_failed_id = store.record_run_start(&old_failed_meta).unwrap();
    store
        .record_run_complete(old_failed_id, RunStatus::Failed, Some(50_000))
        .unwrap();

    // Create recent run (10 days old)
    let recent_meta = RunMetadata::minimal(
        Some(PathBuf::from("/project1/otto.yml")),
        "abc12345".to_string(),
        now - (10 * 86400),
    );
    let recent_id = store.record_run_start(&recent_meta).unwrap();
    store
        .record_run_complete(recent_id, RunStatus::Success, Some(75_000))
        .unwrap();

    // Create run from different project (45 days old)
    let other_project_meta = RunMetadata::minimal(
        Some(PathBuf::from("/project2/otto.yml")),
        "def45678".to_string(),
        now - (45 * 86400),
    );
    let other_id = store.record_run_start(&other_project_meta).unwrap();
    store
        .record_run_complete(other_id, RunStatus::Success, Some(200_000))
        .unwrap();

    Arc::new(store)
}

#[tokio::test]
async fn test_execute_with_database_empty_store() -> Result<()> {
    let home = TempDir::new()?;
    let store = Arc::new(MemoryStateStore::new());
    let cmd = CleanCommand {
        keep_days: 30,
        keep_last: None,
        keep_failed: None,
        dry_run: true,
        project_filter: None,
        no_db: false,
        quiet: false,
        otto_home: Some(home.path().to_path_buf()),
    };

    let result = cmd.execute_with_store(Some(store)).await;
    assert!(result.is_ok());
    Ok(())
}

#[tokio::test]
async fn test_execute_with_database_dry_run() -> Result<()> {
    // An empty home of this test's own: the sweep walks whatever tree it is
    // given, and the default is the developer's real `~/.otto`.
    let home = TempDir::new()?;
    let store = create_store_with_runs();

    // Verify initial state
    let initial_runs = store.get_runs_with_filters(None, None, 100)?;
    assert_eq!(initial_runs.len(), 4);

    let cmd = CleanCommand {
        keep_days: 30,
        keep_last: None,
        keep_failed: None,
        dry_run: true,
        project_filter: None,
        no_db: false,
        quiet: false,
        otto_home: Some(home.path().to_path_buf()),
    };

    let result = cmd.execute_with_store(Some(store.clone())).await;
    assert!(result.is_ok());

    // Dry run should not delete anything
    let final_runs = store.get_runs_with_filters(None, None, 100)?;
    assert_eq!(final_runs.len(), 4);
    Ok(())
}

#[tokio::test]
async fn test_execute_with_database_actual_delete() -> Result<()> {
    // An empty home of this test's own: the sweep walks whatever tree it is
    // given, and the default is the developer's real `~/.otto`.
    let home = TempDir::new()?;
    let store = create_store_with_runs();

    // Verify initial state
    let initial_runs = store.get_runs_with_filters(None, None, 100)?;
    assert_eq!(initial_runs.len(), 4);

    let cmd = CleanCommand {
        keep_days: 30,
        keep_last: None,
        keep_failed: None,
        dry_run: false,
        project_filter: None,
        no_db: false,
        quiet: false,
        otto_home: Some(home.path().to_path_buf()),
    };

    let result = cmd.execute_with_store(Some(store.clone())).await;
    assert!(result.is_ok());

    // Should have deleted 3 old runs (40 day, 35 day, 45 day)
    let final_runs = store.get_runs_with_filters(None, None, 100)?;
    assert_eq!(final_runs.len(), 1);
    Ok(())
}

#[tokio::test]
async fn test_execute_with_database_project_filter() -> Result<()> {
    // An empty home of this test's own: the sweep walks whatever tree it is
    // given, and the default is the developer's real `~/.otto`.
    let home = TempDir::new()?;
    let store = create_store_with_runs();

    let cmd = CleanCommand {
        keep_days: 30,
        keep_last: None,
        keep_failed: None,
        dry_run: false,
        project_filter: Some("abc12345".to_string()),
        no_db: false,
        quiet: false,
        otto_home: Some(home.path().to_path_buf()),
    };

    let result = cmd.execute_with_store(Some(store.clone())).await;
    assert!(result.is_ok());

    // Should have deleted 2 old runs from abc123, kept def456's old run
    let final_runs = store.get_runs_with_filters(None, None, 100)?;
    // Remaining: recent abc123 + old def456
    assert_eq!(final_runs.len(), 2);
    Ok(())
}

/// `--keep-last` keeps the N newest **run directories**, and a row with no
/// directory has none to keep.
///
/// This test used to assert the opposite - four directoryless rows, `--keep-last
/// 2`, two survivors - and that is the behaviour the sweep had to give up.
/// `--keep-last` is applied once, over the directories, because applying it to
/// the rows as well keeps N of each population, up to 2N, where `--no-db` keeps
/// N in total. Directoryless rows are still deleted by `--keep-days` and
/// `--keep-failed`, which is what stops the 388 of them on the author's machine
/// from growing without bound.
#[tokio::test]
async fn keep_last_does_not_hold_back_rows_with_no_run_directory() -> Result<()> {
    // An empty home of this test's own: the sweep walks whatever tree it is
    // given, and the default is the developer's real `~/.otto`.
    let home = TempDir::new()?;
    let store = create_store_with_runs();

    let cmd = CleanCommand {
        keep_days: 0,
        keep_last: Some(2),
        keep_failed: None,
        dry_run: false,
        project_filter: None,
        no_db: false,
        quiet: false,
        otto_home: Some(home.path().to_path_buf()),
    };

    cmd.execute_with_store(Some(store.clone())).await?;

    let final_runs = store.get_runs_with_filters(None, None, 100)?;
    assert!(
        final_runs.is_empty(),
        "every row named no directory, so nothing was in the population --keep-last keeps from: {} left",
        final_runs.len()
    );
    Ok(())
}

/// The other half of the rule above: `--keep-last` does hold back the newest
/// directories, and it counts them once across both populations.
///
/// Four run directories past the cutoff, two of them named by rows and two not,
/// with `--keep-last 2`. A `--keep-last` applied per population would keep two
/// of each and delete two; applied once over the union it keeps the two newest
/// overall - one row-backed and one orphan - and deletes the other two.
#[tokio::test]
async fn keep_last_counts_rows_and_orphaned_directories_as_one_population() -> Result<()> {
    let home = TempDir::new()?;
    let now = now_timestamp();
    let store = MemoryStateStore::new();

    // Newest first: orphan, row, orphan, row.
    let timestamps: Vec<u64> = (1..=4).map(|days| now - (days * 86400)).collect();
    for (position, &timestamp) in timestamps.iter().enumerate() {
        create_test_run(home.path(), "abc12345", timestamp, 1)?;
        if position % 2 == 1 {
            let metadata =
                RunMetadata::minimal(Some(PathBuf::from("/test/otto.yml")), "abc12345".to_string(), timestamp)
                    .with_run_dir(
                        home.path()
                            .join(crate::executor::layout::project_dir_name("widget", "abc12345"))
                            .join(timestamp.to_string()),
                    );
            let id = store.record_run_start(&metadata)?;
            store.record_run_complete(id, RunStatus::Success, Some(1024))?;
        }
    }

    let cmd = CleanCommand {
        keep_days: 0,
        keep_last: Some(2),
        keep_failed: None,
        dry_run: false,
        project_filter: None,
        no_db: false,
        quiet: false,
        otto_home: Some(home.path().to_path_buf()),
    };
    let store: Arc<dyn StateStore> = Arc::new(store);
    cmd.execute_with_store(Some(store.clone())).await?;

    let surviving: Vec<u64> = store
        .get_runs_with_filters(None, None, 100)?
        .iter()
        .map(|run| run.timestamp)
        .collect();
    assert_eq!(
        surviving,
        vec![timestamps[1]],
        "only the row inside the two newest directories survives"
    );

    let project = home
        .path()
        .join(crate::executor::layout::project_dir_name("widget", "abc12345"));
    // `MemoryStateStore::delete_run` has no filesystem to touch, so the
    // row-backed directories are still there; the sweep's own two are the
    // observable part.
    assert!(project.join(timestamps[0].to_string()).exists(), "newest orphan kept");
    assert!(
        !project.join(timestamps[2].to_string()).exists(),
        "the third-newest directory is an orphan past the cutoff and outside --keep-last"
    );
    Ok(())
}

#[tokio::test]
async fn test_execute_with_database_keep_failed_longer() -> Result<()> {
    // An empty home of this test's own: the sweep walks whatever tree it is
    // given, and the default is the developer's real `~/.otto`.
    let home = TempDir::new()?;
    let store = create_store_with_runs();

    let cmd = CleanCommand {
        keep_days: 30,
        keep_last: None,
        keep_failed: Some(60), // Keep failed runs for 60 days
        dry_run: false,
        project_filter: None,
        no_db: false,
        quiet: false,
        otto_home: Some(home.path().to_path_buf()),
    };

    let result = cmd.execute_with_store(Some(store.clone())).await;
    assert!(result.is_ok());

    // Should have deleted 2 old successful runs (40 day, 45 day)
    // but kept the 35-day failed run (within 60-day retention)
    let final_runs = store.get_runs_with_filters(None, None, 100)?;
    assert_eq!(final_runs.len(), 2);
    Ok(())
}

#[tokio::test]
async fn test_find_old_runs_basic() -> Result<()> {
    let store = create_store_with_runs();

    let old_runs = store.find_old_runs(30, None, None, None)?;

    // Should find 3 runs older than 30 days
    assert_eq!(old_runs.len(), 3);
    Ok(())
}

#[tokio::test]
async fn test_find_old_runs_with_keep_last() -> Result<()> {
    let store = create_store_with_runs();

    let old_runs = store.find_old_runs(0, Some(2), None, None)?;

    // Should find 2 runs to delete (keeping 2 most recent)
    assert_eq!(old_runs.len(), 2);
    Ok(())
}

#[tokio::test]
async fn test_find_old_runs_with_project_filter() -> Result<()> {
    let store = create_store_with_runs();

    let old_runs = store.find_old_runs(30, None, None, Some("abc12345"))?;

    // Should find 2 runs older than 30 days from abc123 project
    assert_eq!(old_runs.len(), 2);
    Ok(())
}

#[tokio::test]
async fn test_find_old_runs_with_keep_failed() -> Result<()> {
    let store = create_store_with_runs();

    let old_runs = store.find_old_runs(30, None, Some(60), None)?;

    // Should find 2 successful runs older than 30 days
    // Failed run (35 days) should not be included (within 60-day retention)
    assert_eq!(old_runs.len(), 2);
    Ok(())
}

#[tokio::test]
async fn test_delete_run_from_store() -> Result<()> {
    let store = create_store_with_runs();
    let now = now_timestamp();
    let old_timestamp = now - (40 * 86400);

    // Verify run exists
    let initial_runs = store.get_runs_with_filters(None, None, 100)?;
    let target = initial_runs
        .iter()
        .find(|r| r.timestamp == old_timestamp)
        .expect("the 40-day-old run is in the store");

    // Delete it by id, which is what identifies a run.
    let deleted = store.delete_run(target.id, false, Path::new("/unused"))?;
    assert!(deleted.is_some());

    // Verify it's gone
    let final_runs = store.get_runs_with_filters(None, None, 100)?;
    assert!(!final_runs.iter().any(|r| r.timestamp == old_timestamp));
    Ok(())
}

#[tokio::test]
async fn test_delete_nonexistent_run() -> Result<()> {
    let store = Arc::new(MemoryStateStore::new());

    let deleted = store.delete_run(9999, false, Path::new("/unused"))?;
    assert!(deleted.is_none());
    Ok(())
}

// ========================================
// Regression: db-backed cleanup must not depend on a pre-existing
// `~/.otto` directory (Phase 0, `2026-06-10-code-review-remediation.md`).
// `execute_with_store` used to check `otto_home.exists()` before ever
// looking at the injected store, so on a runner with no populated
// `~/.otto` (unlike a developer's machine), every db-backed clean test
// silently short-circuited to "No ~/.otto directory found" and passed
// or failed on stale assumptions rather than on the store's state.
// ========================================

#[tokio::test]
#[serial_test::serial]
async fn test_execute_with_database_ignores_missing_otto_home() -> Result<()> {
    // Point OTTO_HOME at a directory that does not exist. If the old
    // `otto_home.exists()` early-return were still in place, this would
    // print "No ~/.otto directory found" and skip the store entirely,
    // leaving all 4 runs in place.
    let temp_dir = TempDir::new()?;
    let missing_otto_home = temp_dir.path().join("does-not-exist");
    unsafe {
        std::env::set_var("OTTO_HOME", &missing_otto_home);
    }

    let store = create_store_with_runs();
    let cmd = CleanCommand {
        keep_days: 30,
        keep_last: None,
        keep_failed: None,
        dry_run: false,
        project_filter: None,
        no_db: false,
        quiet: true,
        otto_home: None,
    };

    let result = cmd.execute_with_store(Some(store.clone())).await;
    unsafe {
        std::env::remove_var("OTTO_HOME");
    }
    result?;

    // The 3 old runs (40, 35, 45 days) should have been deleted via the
    // injected store despite `OTTO_HOME` not existing on disk.
    let final_runs = store.get_runs_with_filters(None, None, 100)?;
    assert_eq!(final_runs.len(), 1);
    assert!(
        !missing_otto_home.exists(),
        "clean must not create the otto home directory"
    );
    Ok(())
}

#[tokio::test]
#[serial_test::serial]
async fn test_get_otto_home_honors_otto_home_env() -> Result<()> {
    let temp_dir = TempDir::new()?;
    unsafe {
        std::env::set_var("OTTO_HOME", temp_dir.path());
    }

    let cmd = CleanCommand {
        keep_days: 30,
        keep_last: None,
        keep_failed: None,
        dry_run: true,
        project_filter: None,
        no_db: true,
        quiet: true,
        otto_home: None,
    };
    let resolved = cmd.get_otto_home();
    unsafe {
        std::env::remove_var("OTTO_HOME");
    }

    assert_eq!(resolved?, temp_dir.path());
    Ok(())
}

/// Phase 7 success criterion: a DB-path `Clean` with one refused directory
/// exits non-zero. `MemoryStateStore` never refuses a delete (it has no
/// filesystem to protect), so this drives the real `StateManager`, whose
/// `delete_run` calls `ensure_deletable_under_root` and returns `Err` for a
/// run directory that has been replaced by a symlink - the same defect
/// `delete_run_never_deletes_through_a_symlinked_run_directory`
/// (`manager_tests.rs`) covers one layer down. Before this phase,
/// `execute_with_database` printed that `Err` to stderr and returned `Ok(())`
/// anyway, so a script driving `Clean` could not see the refusal.
#[tokio::test]
#[serial_test::serial]
async fn a_db_path_clean_with_one_refused_directory_exits_non_zero() -> Result<()> {
    let temp_dir = TempDir::new()?;
    let db_path = temp_dir.path().join("test.db");
    let manager = StateManager::with_db_path(db_path)?;

    let otto_home = temp_dir.path().join("otto-home");
    let project = otto_home.join("widget-abc12345");
    fs::create_dir_all(&project)?;

    let victim = temp_dir.path().join("victim");
    fs::create_dir_all(&victim)?;
    fs::write(victim.join("precious.txt"), "keep me")?;
    let run_dir = project.join("1000000000");
    std::os::unix::fs::symlink(&victim, &run_dir)?;

    let metadata = RunMetadata::minimal(
        Some(PathBuf::from("/test/otto.yml")),
        "abc12345".to_string(),
        1_000_000_000,
    )
    .with_run_dir(run_dir);
    manager.record_run_start(&metadata)?;

    let store: Arc<dyn StateStore> = Arc::new(manager);
    let cmd = CleanCommand {
        keep_days: 0,
        keep_last: None,
        keep_failed: None,
        dry_run: false,
        project_filter: None,
        no_db: false,
        quiet: true,
        otto_home: None,
    };

    let previous = std::env::var("OTTO_HOME").ok();
    unsafe {
        std::env::set_var("OTTO_HOME", &otto_home);
    }
    let result = cmd.execute_with_store(Some(store)).await;
    unsafe {
        match &previous {
            Some(home) => std::env::set_var("OTTO_HOME", home),
            None => std::env::remove_var("OTTO_HOME"),
        }
    }

    let err = result.unwrap_err().to_string();
    assert!(err.contains("failed"), "{err}");
    assert!(
        victim.join("precious.txt").exists(),
        "the symlink target must survive the refused delete"
    );
    Ok(())
}

/// The `quiet` gate on the already-gone notice, pinned where it can be.
///
/// `auto_prune` builds its `CleanCommand` with `quiet: true` and runs after
/// every task, so an ungated notice landed in the middle of the user's build
/// output. The race that produces this arm cannot be staged deterministically
/// through the binary and stderr cannot be captured from a unit test, so the
/// decision is a pure function and this asserts the decision.
#[test]
fn the_already_gone_notice_is_silent_under_quiet() {
    assert_eq!(already_gone_notice(true, 1_788_665_760), None);
    assert_eq!(
        already_gone_notice(false, 1_788_665_760).as_deref(),
        Some("  Warning: Run 1788665760 not found in database")
    );
}

/// The per-run delete errors are capped, so a prune that keeps failing does not
/// print a longer report every interval as its backlog grows.
#[test]
fn delete_errors_are_capped_at_a_summary_line() {
    let line = |shown| delete_error_notice(shown, 1_788_665_760, "Refusing to delete run directory");

    for shown in 0..MAX_DELETE_ERRORS_SHOWN {
        assert_eq!(
            line(shown).as_deref(),
            Some("  Error deleting run 1788665760: Refusing to delete run directory"),
            "the first {MAX_DELETE_ERRORS_SHOWN} errors print in full"
        );
    }
    assert_eq!(
        line(MAX_DELETE_ERRORS_SHOWN).as_deref(),
        Some("  ... further delete errors not shown; the count is in the failure below")
    );
    assert_eq!(line(MAX_DELETE_ERRORS_SHOWN + 1), None);
    assert_eq!(line(MAX_DELETE_ERRORS_SHOWN + 500), None);
}

/// A row someone else deleted first is not a failed delete: `Clean` exits 0, so
/// a script driving it beside a run whose own `auto_prune` fires does not see a
/// spurious failure.
///
/// The store is a real `MemoryStateStore` told to lose every delete race
/// (`lose_every_delete_race`), which is what the losing pruner sees:
/// `find_old_runs` reported rows and every `delete_run` then answered
/// `Ok(None)`. That switch replaced a 14-method delegating double in this file,
/// which churned every time `StateStore` grew a method.
///
/// Scope: this asserts the exit code and nothing else. The `quiet` gate on the
/// per-run warning is not asserted here (stderr is not capturable from a unit
/// test), and `auto_prune`'s fall-through past a `Clean` error is pinned in
/// `executor::pruning_tests`, not here.
#[tokio::test]
async fn a_run_deleted_by_someone_else_first_is_not_a_failed_delete() -> Result<()> {
    // An empty home of this test's own: the sweep walks whatever tree it is
    // given, and the default is the developer's real `~/.otto`.
    let home = TempDir::new()?;
    let memory = create_store_with_runs();
    memory.lose_every_delete_race();
    let store: Arc<dyn StateStore> = memory;

    let cmd = CleanCommand {
        keep_days: 30,
        keep_last: None,
        keep_failed: None,
        dry_run: false,
        project_filter: None,
        no_db: false,
        quiet: true,
        otto_home: Some(home.path().to_path_buf()),
    };

    let result = cmd.execute_with_store(Some(store)).await;
    assert!(result.is_ok(), "rows already gone must exit 0: {}", result.unwrap_err());
    Ok(())
}

// ========================================
// The orphan sweep: run directories the database cannot name.
// ========================================

/// A run directory with a row that records it, aged `days_old`.
fn row_backed_run(store: &MemoryStateStore, otto_home: &Path, timestamp: u64) -> Result<PathBuf> {
    create_test_run(otto_home, "abc12345", timestamp, 1)?;
    let run_dir = otto_home
        .join(crate::executor::layout::project_dir_name("widget", "abc12345"))
        .join(timestamp.to_string());
    let metadata = RunMetadata::minimal(Some(PathBuf::from("/test/otto.yml")), "abc12345".to_string(), timestamp)
        .with_run_dir(run_dir.clone());
    let id = store.record_run_start(&metadata)?;
    store.record_run_complete(id, RunStatus::Success, Some(1024))?;
    Ok(run_dir)
}

/// AC1, at the level a unit test can reach: the two modes select the same set
/// of run directories.
///
/// The fixture is the shape the criterion exists for - one run the database
/// knows about, one it does not, both past retention - and the selections are
/// compared as sets of directories rather than as counts, because a row that
/// records no directory contributes a row deletion with no directory analogue.
#[tokio::test]
async fn both_modes_select_the_same_set_of_run_directories() -> Result<()> {
    let home = TempDir::new()?;
    let now = now_timestamp();
    let store = MemoryStateStore::new();

    let row_backed = row_backed_run(&store, home.path(), now - (40 * 86400))?;
    // Nothing records this one: the shape of all 1993 orphans on the author's
    // machine, and invisible to the default path before the sweep.
    create_test_run(home.path(), "abc12345", now - (45 * 86400), 1)?;
    let orphan = home
        .path()
        .join(crate::executor::layout::project_dir_name("widget", "abc12345"))
        .join((now - (45 * 86400)).to_string());
    // Recent, so retention keeps it in both modes.
    create_test_run(home.path(), "abc12345", now - 3600, 1)?;

    let store: Arc<dyn StateStore> = Arc::new(store);
    let selected = |no_db: bool| CleanCommand {
        keep_days: 30,
        keep_last: None,
        keep_failed: None,
        dry_run: true,
        project_filter: None,
        no_db,
        quiet: false,
        otto_home: Some(home.path().to_path_buf()),
    };

    let database_mode = selected(false).select_for_test(Some(Arc::clone(&store)), now)?;
    let filesystem_mode = selected(true).select_for_test(None, now)?;

    let mut expected = vec![row_backed.display().to_string(), orphan.display().to_string()];
    expected.sort();
    assert_eq!(database_mode, expected, "the database path must see both directories");
    assert_eq!(filesystem_mode, expected, "so must the filesystem path");
    assert_eq!(database_mode, filesystem_mode);
    Ok(())
}

/// A row that records no run directory: the row goes, and the directory it left
/// behind is reclaimed by path rather than guessed at.
///
/// The guess that used to be made here rebuilt `<name>-<hash>/<timestamp>` from
/// the project's *content* hash, which is not the hash the directory name
/// carries, so it missed, deleted the row, and left the directory with nothing
/// pointing at it. 388 rows on the author's machine were armed this way.
#[tokio::test]
async fn a_row_with_no_run_directory_leaves_no_orphan_behind() -> Result<()> {
    let home = TempDir::new()?;
    let now = now_timestamp();
    let timestamp = now - (40 * 86400);

    let store = MemoryStateStore::new();
    create_test_run(home.path(), "abc12345", timestamp, 1)?;
    let orphaned = home
        .path()
        .join(crate::executor::layout::project_dir_name("widget", "abc12345"))
        .join(timestamp.to_string());
    // `minimal` records no run directory, exactly like a pre-v5 row.
    let metadata = RunMetadata::minimal(Some(PathBuf::from("/test/otto.yml")), "abc12345".to_string(), timestamp);
    let id = store.record_run_start(&metadata)?;
    store.record_run_complete(id, RunStatus::Success, Some(1024))?;

    let store: Arc<dyn StateStore> = Arc::new(store);
    let cmd = CleanCommand {
        keep_days: 30,
        keep_last: None,
        keep_failed: None,
        dry_run: false,
        project_filter: None,
        no_db: false,
        quiet: true,
        otto_home: Some(home.path().to_path_buf()),
    };
    cmd.execute_with_store(Some(Arc::clone(&store))).await?;

    assert!(
        store.get_runs_with_filters(None, None, 100)?.is_empty(),
        "the row is deleted rather than resolved to a guessed path"
    );
    assert!(
        !orphaned.exists(),
        "and the directory it never named is reclaimed by path"
    );
    Ok(())
}

/// The regression the phase exists for: one row-backed run and one
/// directory-only run, both past retention, both removed by the default path.
///
/// Both deletions are fenced against the same directory: the home this command
/// was given. The row pass used to resolve its own from `$OTTO_HOME`, so this
/// test had to pin the environment as well as pass the home.
#[tokio::test]
async fn the_default_path_removes_both_a_row_backed_run_and_a_directory_only_run() -> Result<()> {
    let home = TempDir::new()?;
    let now = now_timestamp();
    let db_path = home.path().join("otto.db");
    let manager = StateManager::with_db_path(db_path)?;

    let row_backed = row_backed_run_in_manager(&manager, home.path(), now - (40 * 86400))?;
    create_test_run(home.path(), "abc12345", now - (45 * 86400), 1)?;
    let directory_only = home
        .path()
        .join(crate::executor::layout::project_dir_name("widget", "abc12345"))
        .join((now - (45 * 86400)).to_string());

    let cmd = CleanCommand {
        keep_days: 30,
        keep_last: None,
        keep_failed: None,
        dry_run: false,
        project_filter: None,
        no_db: false,
        quiet: true,
        otto_home: Some(home.path().to_path_buf()),
    };
    let store: Arc<dyn StateStore> = Arc::new(manager);
    cmd.execute_with_store(Some(Arc::clone(&store))).await?;

    assert!(!row_backed.exists(), "the row-backed directory is gone");
    assert!(!directory_only.exists(), "and so is the one no row named");
    assert!(
        store.get_runs_with_filters(None, None, 100)?.is_empty(),
        "the row went with its directory"
    );
    Ok(())
}

/// A run directory with a row, in a real `StateManager` so `delete_run` reaches
/// the filesystem.
fn row_backed_run_in_manager(manager: &StateManager, otto_home: &Path, timestamp: u64) -> Result<PathBuf> {
    row_backed_run_with_status(manager, otto_home, timestamp, RunStatus::Success)
}

fn row_backed_run_with_status(
    manager: &StateManager,
    otto_home: &Path,
    timestamp: u64,
    status: RunStatus,
) -> Result<PathBuf> {
    create_test_run(otto_home, "abc12345", timestamp, 1)?;
    let run_dir = otto_home
        .join(crate::executor::layout::project_dir_name("widget", "abc12345"))
        .join(timestamp.to_string());
    let metadata = RunMetadata::minimal(Some(PathBuf::from("/test/otto.yml")), "abc12345".to_string(), timestamp)
        .with_run_dir(run_dir.clone());
    let id = manager.record_run_start(&metadata)?;
    manager.record_run_complete(id, status, Some(1024))?;
    Ok(run_dir)
}

/// `--keep-failed` needs a status, so a directory with no row takes the longer
/// of the two cutoffs while a row-backed run takes the one its status earns.
///
/// The filesystem path widens `keep_days` for everything it scans, because it
/// has no statuses at all. Widening the whole selection here would throw away
/// the status the database does have, and keep every successful run for the
/// failed-run retention. This is the one place the two modes differ by design,
/// and it is why the parity criterion is scoped to invocations without the flag.
#[tokio::test]
async fn keep_failed_widens_the_cutoff_only_for_directories_with_no_row() -> Result<()> {
    let home = TempDir::new()?;
    let now = now_timestamp();
    let manager = StateManager::with_db_path(home.path().join("otto.db"))?;

    let succeeded = row_backed_run_with_status(&manager, home.path(), now - (40 * 86400), RunStatus::Success)?;
    let failed = row_backed_run_with_status(&manager, home.path(), now - (41 * 86400), RunStatus::Failed)?;
    create_test_run(home.path(), "abc12345", now - (42 * 86400), 1)?;
    let orphan = home
        .path()
        .join(crate::executor::layout::project_dir_name("widget", "abc12345"))
        .join((now - (42 * 86400)).to_string());

    let cmd = CleanCommand {
        keep_days: 30,
        keep_last: None,
        keep_failed: Some(60),
        dry_run: false,
        project_filter: None,
        no_db: false,
        quiet: true,
        otto_home: Some(home.path().to_path_buf()),
    };
    cmd.execute_with_store(Some(Arc::new(manager) as Arc<dyn StateStore>))
        .await?;

    assert!(!succeeded.exists(), "a successful run past --keep-days goes");
    assert!(failed.exists(), "a failed run inside --keep-failed stays");
    assert!(
        orphan.exists(),
        "and a directory with no row is kept for the longer cutoff rather than deleted by a flag meant to protect it"
    );
    Ok(())
}

/// The sweep never deletes through a symlinked run directory, on either path.
///
/// The risk this phase carries is that it deletes something it should not, and a
/// symlinked run directory is the shape that turns one deletion into somebody
/// else's data: `remove_dir_all` through a link empties the target and leaves
/// the link. The scan refuses the link outright, so it never reaches a
/// selection, and `ensure_deletable_under_root` fences the delete besides.
#[tokio::test]
async fn the_sweep_never_deletes_through_a_symlinked_run_directory() -> Result<()> {
    let home = TempDir::new()?;
    let now = now_timestamp();
    let outside = home.path().join("outside");
    fs::create_dir_all(&outside)?;
    fs::write(outside.join("precious.txt"), "keep me")?;

    let project = home
        .path()
        .join(crate::executor::layout::project_dir_name("widget", "abc12345"));
    fs::create_dir_all(&project)?;
    // Named like a run directory, aged well past any cutoff, and a link.
    let link = project.join((now - (400 * 86400)).to_string());
    std::os::unix::fs::symlink(&outside, &link)?;

    let store: Arc<dyn StateStore> = Arc::new(MemoryStateStore::new());
    let cmd = CleanCommand {
        keep_days: 0,
        keep_last: None,
        keep_failed: None,
        dry_run: false,
        project_filter: None,
        no_db: false,
        quiet: true,
        otto_home: Some(home.path().to_path_buf()),
    };
    cmd.execute_with_store(Some(store)).await?;

    assert!(
        outside.join("precious.txt").exists(),
        "the symlink target must survive a sweep at --keep-days 0"
    );
    assert!(link.exists(), "and the link itself is left alone");
    Ok(())
}

/// A run that is still going holds the lock on its own directory, and neither
/// mode selects it - not even at `--keep-days 0`, and not even in `--dry-run`,
/// which has to select what a real invocation would delete.
#[tokio::test]
async fn a_directory_a_run_still_holds_is_not_selected_by_either_mode() -> Result<()> {
    let home = TempDir::new()?;
    let now = now_timestamp();
    let timestamp = now - (40 * 86400);
    create_test_run(home.path(), "abc12345", timestamp, 1)?;
    let run_dir = home
        .path()
        .join(crate::executor::layout::project_dir_name("widget", "abc12345"))
        .join(timestamp.to_string());

    let live = crate::executor::runlock::hold(&run_dir)?;

    let store: Arc<dyn StateStore> = Arc::new(MemoryStateStore::new());
    let cmd = |no_db: bool| CleanCommand {
        keep_days: 0,
        keep_last: None,
        keep_failed: None,
        dry_run: true,
        project_filter: None,
        no_db,
        quiet: false,
        otto_home: Some(home.path().to_path_buf()),
    };

    assert!(
        cmd(false).select_for_test(Some(Arc::clone(&store)), now)?.is_empty(),
        "the database path must not select a directory a run is using"
    );
    assert!(
        cmd(true).select_for_test(None, now)?.is_empty(),
        "and neither must the filesystem path"
    );

    // The run ends, and the same directory becomes selectable again.
    //
    // Retried rather than asserted once, and the reason is a property of `flock`
    // worth knowing: the lock lives on the open file description, and a `fork`
    // anywhere in this process duplicates the descriptor, so the release waits
    // for that copy to close. Otto's own task children close theirs at `exec`
    // (the descriptor is close-on-exec), but this suite runs tests in threads
    // that spawn processes, so an unrelated test forking between `hold` and
    // `drop` here holds the lock open for the microseconds until its child
    // execs. Measured: a forked child that has not yet exec'd keeps the lock,
    // and it is free the moment that child is gone.
    drop(live);
    let mut selected = cmd(true).select_for_test(None, now)?;
    for _ in 0..100 {
        if !selected.is_empty() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
        selected = cmd(true).select_for_test(None, now)?;
    }
    assert_eq!(
        selected,
        vec![run_dir.display().to_string()],
        "a released lock leaves nothing unreclaimable"
    );
    Ok(())
}
