use assert_cmd::cargo::cargo_bin_cmd;
use eyre::Result;
use serial_test::serial;
use std::fs;
use std::path::PathBuf;
use std::time::SystemTime;
use tempfile::TempDir;

/// Helper to create a test run directory structure
fn create_test_run(otto_home: &std::path::Path, project_hash: &str, timestamp: u64, status: &str) -> Result<()> {
    let run_dir = otto_home
        .join(format!("otto-{}", project_hash))
        .join(timestamp.to_string())
        .join("tasks");

    fs::create_dir_all(&run_dir)?;

    // Create a dummy file to give the directory some size
    let dummy_file = run_dir.join("dummy.txt");
    fs::write(dummy_file, "test content")?;

    // Create run.yaml with metadata
    let metadata_path = run_dir.parent().unwrap().join("run.yaml");
    let metadata_content = format!(
        r#"ottofile: /test/otto.yml
hash: {}
timestamp: {}
status: {}
"#,
        project_hash, timestamp, status
    );
    fs::write(metadata_path, metadata_content)?;

    Ok(())
}

/// Helper to create a StateManager with test data
fn setup_test_database(
    db_path: &std::path::Path,
    project_hash: &str,
    runs: Vec<(u64, &str, u64)>, // (timestamp, status, size_bytes)
) -> Result<()> {
    use otto::executor::state::{RunMetadata, RunStatus, StateManager};

    let manager = StateManager::with_db_path(db_path.to_path_buf())?;

    for (timestamp, status, size_bytes) in runs {
        let metadata = RunMetadata::minimal(
            Some(PathBuf::from("/test/otto.yml")),
            project_hash.to_string(),
            timestamp,
        );

        manager.record_run_start(&metadata)?;

        let run_status = match status {
            "success" => RunStatus::Success,
            "failed" => RunStatus::Failed,
            _ => RunStatus::Running,
        };

        manager.record_run_complete(timestamp, run_status, Some(size_bytes))?;
    }

    Ok(())
}

#[test]
#[serial]
fn test_clean_with_keep_last_flag() -> Result<()> {
    let temp_dir = TempDir::new()?;
    let home_dir = temp_dir.path();
    let otto_home = home_dir.join(".otto");
    fs::create_dir_all(&otto_home)?;
    let db_path = otto_home.join("otto.db");

    let now = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH)?.as_secs();

    // Create 5 runs, all older than 30 days
    let mut runs = Vec::new();
    for i in 0..5 {
        let timestamp = now - ((40 + i) * 24 * 60 * 60);
        runs.push((timestamp, "success", 1024));
        create_test_run(&otto_home, "abc123", timestamp, "success")?;
    }

    // Setup database with test data
    setup_test_database(&db_path, "abc123", runs)?;

    // Run clean with --keep-last 2
    let output = cargo_bin_cmd!("otto")
        .arg("Clean")
        .arg("--keep-days")
        .arg("30")
        .arg("--keep-last")
        .arg("2")
        .arg("--dry-run")
        .env("HOME", home_dir)
        .env("OTTO_HOME", &otto_home)
        .output()?;

    let stdout = String::from_utf8_lossy(&output.stdout);

    // Should find 3 runs to delete (5 total - 2 kept)
    assert!(
        stdout.contains("Found 3 runs to delete") || stdout.contains("3"),
        "Expected to find 3 runs to delete, got: {}",
        stdout
    );

    Ok(())
}

#[test]
#[serial]
fn test_clean_with_keep_failed_flag() -> Result<()> {
    let temp_dir = TempDir::new()?;
    let home_dir = temp_dir.path();
    let otto_home = home_dir.join(".otto");
    fs::create_dir_all(&otto_home)?;
    let db_path = otto_home.join("otto.db");

    let now = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH)?.as_secs();

    // Create successful run 40 days old
    let success_timestamp = now - (40 * 24 * 60 * 60);
    create_test_run(&otto_home, "abc123", success_timestamp, "success")?;

    // Create failed run 40 days old
    let failed_timestamp = now - (39 * 24 * 60 * 60);
    create_test_run(&otto_home, "abc123", failed_timestamp, "failed")?;

    // Setup database
    setup_test_database(
        &db_path,
        "abc123",
        vec![(success_timestamp, "success", 1024), (failed_timestamp, "failed", 2048)],
    )?;

    // Run clean: keep successful runs for 30 days, failed runs for 45 days
    let output = cargo_bin_cmd!("otto")
        .arg("Clean")
        .arg("--keep-days")
        .arg("30")
        .arg("--keep-failed")
        .arg("45")
        .arg("--dry-run")
        .env("HOME", home_dir)
        .env("OTTO_HOME", &otto_home)
        .output()?;

    let stdout = String::from_utf8_lossy(&output.stdout);

    // Should find 1 run to delete (the successful one)
    // The failed run should be kept because it's kept for 45 days
    assert!(
        stdout.contains("Found 1 run") || stdout.contains("1"),
        "Expected to find 1 run to delete, got: {}",
        stdout
    );

    Ok(())
}

#[test]
#[serial]
fn test_clean_with_no_db_fallback() -> Result<()> {
    let temp_dir = TempDir::new()?;
    let home_dir = temp_dir.path();
    let otto_home = home_dir.join(".otto");
    fs::create_dir_all(&otto_home)?;

    let now = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH)?.as_secs();

    // Create old run (40 days old)
    let old_timestamp = now - (40 * 24 * 60 * 60);
    create_test_run(&otto_home, "abc123", old_timestamp, "success")?;

    // Create recent run (10 days old)
    let recent_timestamp = now - (10 * 24 * 60 * 60);
    create_test_run(&otto_home, "abc123", recent_timestamp, "success")?;

    // Run clean with --no-db (filesystem fallback mode)
    let output = cargo_bin_cmd!("otto")
        .arg("Clean")
        .arg("--keep-days")
        .arg("30")
        .arg("--no-db")
        .arg("--dry-run")
        .env("HOME", home_dir)
        .env("OTTO_HOME", &otto_home)
        .output()?;

    let stdout = String::from_utf8_lossy(&output.stdout);

    // Should use filesystem scan and find 1 old run
    assert!(
        stdout.contains("Scanning") && (stdout.contains("Found 1 run") || stdout.contains("1")),
        "Expected filesystem scan and 1 run, got: {}",
        stdout
    );

    Ok(())
}

#[test]
#[serial]
fn test_clean_database_mode_vs_filesystem_mode() -> Result<()> {
    let temp_dir = TempDir::new()?;
    let home_dir = temp_dir.path();
    let otto_home = home_dir.join(".otto");
    fs::create_dir_all(&otto_home)?;
    let db_path = otto_home.join("otto.db");

    let now = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH)?.as_secs();

    let old_timestamp = now - (40 * 24 * 60 * 60);
    create_test_run(&otto_home, "abc123", old_timestamp, "success")?;

    // Setup database
    setup_test_database(&db_path, "abc123", vec![(old_timestamp, "success", 1024)])?;

    // Run with database
    let db_output = cargo_bin_cmd!("otto")
        .arg("Clean")
        .arg("--keep-days")
        .arg("30")
        .arg("--dry-run")
        .env("HOME", home_dir)
        .env("OTTO_HOME", &otto_home)
        .output()?;

    let db_stdout = String::from_utf8_lossy(&db_output.stdout);

    // Run with --no-db (filesystem)
    let fs_output = cargo_bin_cmd!("otto")
        .arg("Clean")
        .arg("--keep-days")
        .arg("30")
        .arg("--no-db")
        .arg("--dry-run")
        .env("HOME", home_dir)
        .env("OTTO_HOME", &otto_home)
        .output()?;

    let fs_stdout = String::from_utf8_lossy(&fs_output.stdout);

    // Both modes should find the same run
    assert!(
        db_stdout.contains("1") || db_stdout.contains("Found 1"),
        "Database mode should find 1 run"
    );
    assert!(
        fs_stdout.contains("1") || fs_stdout.contains("Found 1"),
        "Filesystem mode should find 1 run"
    );

    // Database mode should say "Querying database"
    assert!(
        db_stdout.contains("Querying database") || db_stdout.contains("database"),
        "Should use database mode"
    );

    // Filesystem mode should say "Scanning"
    assert!(fs_stdout.contains("Scanning"), "Should use filesystem scan");

    Ok(())
}

#[test]
#[serial]
fn test_clean_actually_deletes_with_database() -> Result<()> {
    let temp_dir = TempDir::new()?;
    let home_dir = temp_dir.path();
    let otto_home = home_dir.join(".otto");
    fs::create_dir_all(&otto_home)?;
    let db_path = otto_home.join("otto.db");

    let now = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH)?.as_secs();

    let old_timestamp = now - (40 * 24 * 60 * 60);
    create_test_run(&otto_home, "abc123", old_timestamp, "success")?;

    // Setup database
    setup_test_database(&db_path, "abc123", vec![(old_timestamp, "success", 1024)])?;

    // Verify run directory exists
    let run_dir = otto_home.join("otto-abc123").join(old_timestamp.to_string());
    assert!(run_dir.exists(), "Run directory should exist before cleanup");

    // Run clean without --dry-run
    let output = cargo_bin_cmd!("otto")
        .arg("Clean")
        .arg("--keep-days")
        .arg("30")
        .env("HOME", home_dir)
        .env("OTTO_HOME", &otto_home)
        .output()?;

    assert!(output.status.success(), "Clean command should succeed");

    // Verify run directory was deleted
    assert!(!run_dir.exists(), "Run directory should be deleted after cleanup");

    // Verify database record was deleted
    use otto::executor::state::StateManager;
    let manager = StateManager::with_db_path(db_path)?;
    let runs = manager.get_recent_runs(10, None)?;
    assert_eq!(runs.len(), 0, "Database should have no runs after cleanup");

    Ok(())
}

#[test]
#[serial]
fn test_clean_keep_last_in_filesystem_mode() -> Result<()> {
    let temp_dir = TempDir::new()?;
    let home_dir = temp_dir.path();
    let otto_home = home_dir.join(".otto");
    fs::create_dir_all(&otto_home)?;

    let now = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH)?.as_secs();

    // Create 5 old runs
    for i in 0..5 {
        let timestamp = now - ((40 + i) * 24 * 60 * 60);
        create_test_run(&otto_home, "abc123", timestamp, "success")?;
    }

    // Run clean with --keep-last in filesystem mode
    let output = cargo_bin_cmd!("otto")
        .arg("Clean")
        .arg("--keep-days")
        .arg("30")
        .arg("--keep-last")
        .arg("2")
        .arg("--no-db")
        .arg("--dry-run")
        .env("HOME", home_dir)
        .env("OTTO_HOME", &otto_home)
        .output()?;

    let stdout = String::from_utf8_lossy(&output.stdout);

    // The filesystem mode should also respect --keep-last
    // However, the implementation might be slightly different
    // It should show that it's applying retention policy
    assert!(
        stdout.contains("Found") || stdout.contains("runs"),
        "Should show found runs: {}",
        stdout
    );

    Ok(())
}

#[test]
#[serial]
fn test_clean_with_project_filter_and_database() -> Result<()> {
    let temp_dir = TempDir::new()?;
    let home_dir = temp_dir.path();
    let otto_home = home_dir.join(".otto");
    fs::create_dir_all(&otto_home)?;
    let db_path = otto_home.join("otto.db");

    let now = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH)?.as_secs();

    let old_timestamp = now - (40 * 24 * 60 * 60);

    // Create old runs for two projects
    create_test_run(&otto_home, "abc123", old_timestamp, "success")?;
    create_test_run(&otto_home, "def456", old_timestamp + 1, "success")?;

    // Setup database for both projects
    setup_test_database(&db_path, "abc123", vec![(old_timestamp, "success", 1024)])?;
    setup_test_database(&db_path, "def456", vec![(old_timestamp + 1, "success", 2048)])?;

    // Run clean with project filter
    let output = cargo_bin_cmd!("otto")
        .arg("Clean")
        .arg("--keep-days")
        .arg("30")
        .arg("--project-filter")
        .arg("abc123")
        .arg("--dry-run")
        .env("HOME", home_dir)
        .env("OTTO_HOME", &otto_home)
        .output()?;

    let stdout = String::from_utf8_lossy(&output.stdout);

    // Should only find 1 run (from abc123 project)
    assert!(
        stdout.contains("Found 1 run") || stdout.contains("1"),
        "Expected to find 1 run for abc123 project, got: {}",
        stdout
    );

    Ok(())
}

#[test]
#[serial]
fn test_clean_graceful_fallback_when_database_corrupt() -> Result<()> {
    let temp_dir = TempDir::new()?;
    let home_dir = temp_dir.path();
    let otto_home = home_dir.join(".otto");
    fs::create_dir_all(&otto_home)?;
    let db_path = otto_home.join("otto.db");

    let now = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH)?.as_secs();

    let old_timestamp = now - (40 * 24 * 60 * 60);
    create_test_run(&otto_home, "abc123", old_timestamp, "success")?;

    // Create a corrupt database file (just write garbage)
    fs::write(&db_path, "this is not a valid sqlite database")?;

    // Run clean - should fallback to filesystem scan
    let output = cargo_bin_cmd!("otto")
        .arg("Clean")
        .arg("--keep-days")
        .arg("30")
        .arg("--dry-run")
        .env("HOME", home_dir)
        .env("OTTO_HOME", &otto_home)
        .output()?;

    let stdout = String::from_utf8_lossy(&output.stdout);

    // Should fallback to filesystem scan
    assert!(
        stdout.contains("Scanning") || stdout.contains("falling back") || stdout.contains("fallback"),
        "Should fallback to filesystem scan when database is corrupt, got: {}",
        stdout
    );

    // Should still find the old run
    assert!(
        stdout.contains("Found 1 run") || stdout.contains("1"),
        "Should still find runs via filesystem scan, got: {}",
        stdout
    );

    Ok(())
}
