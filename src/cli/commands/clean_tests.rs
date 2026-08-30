#![cfg(test)]

use super::*;
use crate::executor::state::RunStatus;
use crate::ports::MemoryStateStore;
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
async fn test_calculate_dir_size() -> Result<()> {
    let temp_dir = TempDir::new()?;
    create_test_run(temp_dir.path(), "testhash", 1234567890, 100)?;

    let run_dir = temp_dir.path().join("widget-testhash").join("1234567890");
    let size = CleanCommand::calculate_dir_size(&run_dir)?;

    // Should be approximately 100KB (may vary slightly due to filesystem overhead)
    assert!(size >= 100 * 1024);
    assert!(size < 110 * 1024);
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
    let yaml_content = serde_yaml::to_string(&metadata)?;
    fs::write(run_dir.join("run.yaml"), yaml_content)?;

    let cmd = CleanCommand {
        keep_days: 30,
        keep_last: None,
        keep_failed: None,
        dry_run: true,
        project_filter: None,
        no_db: true,
        quiet: false,
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
    let yaml_content = serde_yaml::to_string(&metadata)?;
    fs::write(run_dir.join("run.yaml"), yaml_content)?;

    let cmd = CleanCommand {
        keep_days: 30,
        keep_last: None,
        keep_failed: None,
        dry_run: true,
        project_filter: None,
        no_db: true,
        quiet: false,
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
    };
    let ottofile_path = cmd.read_ottofile_path(&run_dir);

    assert_eq!(ottofile_path, None);
    Ok(())
}

#[tokio::test]
async fn test_format_timestamp() -> Result<()> {
    let cmd = CleanCommand {
        keep_days: 30,
        keep_last: None,
        keep_failed: None,
        dry_run: true,
        project_filter: None,
        no_db: true,
        quiet: false,
    };

    // Test a known timestamp
    let timestamp = 1609459200; // 2021-01-01 00:00:00 UTC
    let formatted = cmd.format_timestamp(timestamp);

    assert_eq!(formatted, "2021-01-01 00:00:00");
    Ok(())
}

#[tokio::test]
async fn test_format_size_bytes() -> Result<()> {
    let cmd = CleanCommand {
        keep_days: 30,
        keep_last: None,
        keep_failed: None,
        dry_run: true,
        project_filter: None,
        no_db: true,
        quiet: false,
    };

    assert_eq!(cmd.format_size(0), "0 B");
    assert_eq!(cmd.format_size(512), "512 B");
    assert_eq!(cmd.format_size(1023), "1023 B");
    Ok(())
}

#[tokio::test]
async fn test_format_size_kilobytes() -> Result<()> {
    let cmd = CleanCommand {
        keep_days: 30,
        keep_last: None,
        keep_failed: None,
        dry_run: true,
        project_filter: None,
        no_db: true,
        quiet: false,
    };

    assert_eq!(cmd.format_size(1024), "1.0 KB");
    assert_eq!(cmd.format_size(2048), "2.0 KB");
    assert_eq!(cmd.format_size(1536), "1.5 KB");
    assert_eq!(cmd.format_size(100 * 1024), "100.0 KB");
    Ok(())
}

#[tokio::test]
async fn test_format_size_megabytes() -> Result<()> {
    let cmd = CleanCommand {
        keep_days: 30,
        keep_last: None,
        keep_failed: None,
        dry_run: true,
        project_filter: None,
        no_db: true,
        quiet: false,
    };

    assert_eq!(cmd.format_size(1024 * 1024), "1.0 MB");
    assert_eq!(cmd.format_size(5 * 1024 * 1024), "5.0 MB");
    assert_eq!(cmd.format_size(1536 * 1024), "1.5 MB");
    assert_eq!(cmd.format_size(100 * 1024 * 1024), "100.0 MB");
    Ok(())
}

#[tokio::test]
async fn test_format_size_gigabytes() -> Result<()> {
    let cmd = CleanCommand {
        keep_days: 30,
        keep_last: None,
        keep_failed: None,
        dry_run: true,
        project_filter: None,
        no_db: true,
        quiet: false,
    };

    assert_eq!(cmd.format_size(1024 * 1024 * 1024), "1.0 GB");
    assert_eq!(cmd.format_size(5 * 1024 * 1024 * 1024), "5.0 GB");
    assert_eq!(cmd.format_size(1536 * 1024 * 1024), "1.5 GB");
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
    let yaml_content = serde_yaml::to_string(&metadata)?;
    fs::write(run_dir.join("run.yaml"), yaml_content)?;

    let cmd = CleanCommand {
        keep_days: 30,
        keep_last: None,
        keep_failed: None,
        dry_run: true,
        project_filter: None,
        no_db: true,
        quiet: false,
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
    let store = Arc::new(MemoryStateStore::new());
    let cmd = CleanCommand {
        keep_days: 30,
        keep_last: None,
        keep_failed: None,
        dry_run: true,
        project_filter: None,
        no_db: false,
        quiet: false,
    };

    let result = cmd.execute_with_store(Some(store)).await;
    assert!(result.is_ok());
    Ok(())
}

#[tokio::test]
async fn test_execute_with_database_dry_run() -> Result<()> {
    let store = create_store_with_runs();

    // Verify initial state
    let initial_runs = store.get_recent_runs(100, None)?;
    assert_eq!(initial_runs.len(), 4);

    let cmd = CleanCommand {
        keep_days: 30,
        keep_last: None,
        keep_failed: None,
        dry_run: true,
        project_filter: None,
        no_db: false,
        quiet: false,
    };

    let result = cmd.execute_with_store(Some(store.clone())).await;
    assert!(result.is_ok());

    // Dry run should not delete anything
    let final_runs = store.get_recent_runs(100, None)?;
    assert_eq!(final_runs.len(), 4);
    Ok(())
}

#[tokio::test]
async fn test_execute_with_database_actual_delete() -> Result<()> {
    let store = create_store_with_runs();

    // Verify initial state
    let initial_runs = store.get_recent_runs(100, None)?;
    assert_eq!(initial_runs.len(), 4);

    let cmd = CleanCommand {
        keep_days: 30,
        keep_last: None,
        keep_failed: None,
        dry_run: false,
        project_filter: None,
        no_db: false,
        quiet: false,
    };

    let result = cmd.execute_with_store(Some(store.clone())).await;
    assert!(result.is_ok());

    // Should have deleted 3 old runs (40 day, 35 day, 45 day)
    let final_runs = store.get_recent_runs(100, None)?;
    assert_eq!(final_runs.len(), 1);
    Ok(())
}

#[tokio::test]
async fn test_execute_with_database_project_filter() -> Result<()> {
    let store = create_store_with_runs();

    let cmd = CleanCommand {
        keep_days: 30,
        keep_last: None,
        keep_failed: None,
        dry_run: false,
        project_filter: Some("abc12345".to_string()),
        no_db: false,
        quiet: false,
    };

    let result = cmd.execute_with_store(Some(store.clone())).await;
    assert!(result.is_ok());

    // Should have deleted 2 old runs from abc123, kept def456's old run
    let final_runs = store.get_recent_runs(100, None)?;
    // Remaining: recent abc123 + old def456
    assert_eq!(final_runs.len(), 2);
    Ok(())
}

#[tokio::test]
async fn test_execute_with_database_keep_last() -> Result<()> {
    let store = create_store_with_runs();

    let cmd = CleanCommand {
        keep_days: 0, // Would delete everything
        keep_last: Some(2),
        keep_failed: None,
        dry_run: false,
        project_filter: None,
        no_db: false,
        quiet: false,
    };

    let result = cmd.execute_with_store(Some(store.clone())).await;
    assert!(result.is_ok());

    // Should keep the 2 most recent runs
    let final_runs = store.get_recent_runs(100, None)?;
    assert_eq!(final_runs.len(), 2);
    Ok(())
}

#[tokio::test]
async fn test_execute_with_database_keep_failed_longer() -> Result<()> {
    let store = create_store_with_runs();

    let cmd = CleanCommand {
        keep_days: 30,
        keep_last: None,
        keep_failed: Some(60), // Keep failed runs for 60 days
        dry_run: false,
        project_filter: None,
        no_db: false,
        quiet: false,
    };

    let result = cmd.execute_with_store(Some(store.clone())).await;
    assert!(result.is_ok());

    // Should have deleted 2 old successful runs (40 day, 45 day)
    // but kept the 35-day failed run (within 60-day retention)
    let final_runs = store.get_recent_runs(100, None)?;
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
    let initial_runs = store.get_recent_runs(100, None)?;
    let target = initial_runs
        .iter()
        .find(|r| r.timestamp == old_timestamp)
        .expect("the 40-day-old run is in the store");

    // Delete it by id, which is what identifies a run.
    let deleted = store.delete_run(target.id, false)?;
    assert!(deleted.is_some());

    // Verify it's gone
    let final_runs = store.get_recent_runs(100, None)?;
    assert!(!final_runs.iter().any(|r| r.timestamp == old_timestamp));
    Ok(())
}

#[tokio::test]
async fn test_delete_nonexistent_run() -> Result<()> {
    let store = Arc::new(MemoryStateStore::new());

    let deleted = store.delete_run(9999, false)?;
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
    };

    let result = cmd.execute_with_store(Some(store.clone())).await;
    unsafe {
        std::env::remove_var("OTTO_HOME");
    }
    result?;

    // The 3 old runs (40, 35, 45 days) should have been deleted via the
    // injected store despite `OTTO_HOME` not existing on disk.
    let final_runs = store.get_recent_runs(100, None)?;
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
    };
    let resolved = cmd.get_otto_home();
    unsafe {
        std::env::remove_var("OTTO_HOME");
    }

    assert_eq!(resolved?, temp_dir.path());
    Ok(())
}
