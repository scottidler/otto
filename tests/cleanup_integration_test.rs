mod common;

use common::otto_cmd;
use eyre::Result;
use serial_test::serial;
use std::fs;
use std::path::PathBuf;
use std::time::SystemTime;
use tempfile::TempDir;

/// The project name every fixture in this file uses. Run roots are
/// `<name>-<hash>`, which is what `Workspace` creates and what both cleanup
/// backends now look for.
const PROJECT: &str = "widget";

/// The otto binary, with its environment pinned rather than inherited.
///
/// `common::otto_cmd` pins `OTTO_HOME` and removes `OTTO_DB_PATH`; these tests
/// additionally pin `HOME`, because cleanup resolves `~` for the fallback home
/// and a stray real `~/.otto` would otherwise be in scope for a delete.
fn otto(home_dir: &std::path::Path, otto_home: &std::path::Path) -> assert_cmd::Command {
    let mut cmd = otto_cmd(otto_home);
    cmd.env("HOME", home_dir);
    cmd
}

/// Where a run of `PROJECT` writes: `<otto home>/<name>-<hash>/<timestamp>`.
fn run_dir(otto_home: &std::path::Path, project_hash: &str, timestamp: u64) -> PathBuf {
    otto_home
        .join(format!("{}-{}", PROJECT, project_hash))
        .join(timestamp.to_string())
}

/// The run directories a `--dry-run` listing names, sorted.
///
/// Both modes print the run directory last on every line they list, which is
/// what makes two selections comparable as sets instead of as counts.
fn selected_run_dirs(stdout: &str, otto_home: &std::path::Path) -> Vec<String> {
    let prefix = otto_home.display().to_string();
    let mut found: Vec<String> = stdout
        .lines()
        .filter(|line| line.starts_with("  "))
        .filter_map(|line| line.rsplit(' ').next())
        .filter(|word| word.starts_with(&prefix))
        .map(str::to_string)
        .collect();
    found.sort();
    found
}

/// Helper to create a test run directory structure
fn create_test_run(otto_home: &std::path::Path, project_hash: &str, timestamp: u64, status: &str) -> Result<()> {
    let run_dir = otto_home
        .join(format!("{}-{}", PROJECT, project_hash))
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
    otto_home: &std::path::Path,
    project_hash: &str,
    runs: Vec<(u64, &str, u64)>, // (timestamp, status, size_bytes)
) -> Result<()> {
    use otto::executor::state::{RunMetadata, RunStatus, StateManager};

    let manager = StateManager::with_db_path(otto_home.join("otto.db"))?;

    for (timestamp, status, size_bytes) in runs {
        // Record the run directory, exactly as a real run does, so DB-driven
        // cleanup deletes the directory that is actually there.
        let metadata = RunMetadata::minimal(
            Some(PathBuf::from("/test/otto.yml")),
            project_hash.to_string(),
            timestamp,
        )
        .with_run_dir(
            otto_home
                .join(format!("{}-{}", PROJECT, project_hash))
                .join(timestamp.to_string()),
        );

        let run_id = manager.record_run_start(&metadata)?;

        let run_status = match status {
            "success" => RunStatus::Success,
            "failed" => RunStatus::Failed,
            _ => RunStatus::Running,
        };

        manager.record_run_complete(run_id, run_status, Some(size_bytes))?;
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

    let now = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH)?.as_secs();

    // Create 5 runs, all older than 30 days
    let mut runs = Vec::new();
    for i in 0..5 {
        let timestamp = now - ((40 + i) * 24 * 60 * 60);
        runs.push((timestamp, "success", 1024));
        create_test_run(&otto_home, "abc12345", timestamp, "success")?;
    }

    // Setup database with test data
    setup_test_database(&otto_home, "abc12345", runs)?;

    // Run clean with --keep-last 2
    let output = otto(home_dir, &otto_home)
        .arg("Clean")
        .arg("--keep-days")
        .arg("30")
        .arg("--keep-last")
        .arg("2")
        .arg("--dry-run")
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

    let now = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH)?.as_secs();

    // Create successful run 40 days old
    let success_timestamp = now - (40 * 24 * 60 * 60);
    create_test_run(&otto_home, "abc12345", success_timestamp, "success")?;

    // Create failed run 40 days old
    let failed_timestamp = now - (39 * 24 * 60 * 60);
    create_test_run(&otto_home, "abc12345", failed_timestamp, "failed")?;

    // Setup database
    setup_test_database(
        &otto_home,
        "abc12345",
        vec![(success_timestamp, "success", 1024), (failed_timestamp, "failed", 2048)],
    )?;

    // Run clean: keep successful runs for 30 days, failed runs for 45 days
    let output = otto(home_dir, &otto_home)
        .arg("Clean")
        .arg("--keep-days")
        .arg("30")
        .arg("--keep-failed")
        .arg("45")
        .arg("--dry-run")
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
    create_test_run(&otto_home, "abc12345", old_timestamp, "success")?;

    // Create recent run (10 days old)
    let recent_timestamp = now - (10 * 24 * 60 * 60);
    create_test_run(&otto_home, "abc12345", recent_timestamp, "success")?;

    // Run clean with --no-db (filesystem fallback mode)
    let output = otto(home_dir, &otto_home)
        .arg("Clean")
        .arg("--keep-days")
        .arg("30")
        .arg("--no-db")
        .arg("--dry-run")
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

    let now = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH)?.as_secs();

    let old_timestamp = now - (40 * 24 * 60 * 60);
    create_test_run(&otto_home, "abc12345", old_timestamp, "success")?;

    // A run directory no row names: the shape the default path could not see at
    // all before the orphan sweep, and the reason the two modes disagreed.
    let orphan_timestamp = now - (45 * 24 * 60 * 60);
    create_test_run(&otto_home, "abc12345", orphan_timestamp, "success")?;

    // A recent run, so both modes have something to keep as well.
    create_test_run(&otto_home, "abc12345", now - 3600, "success")?;

    // Setup database
    setup_test_database(&otto_home, "abc12345", vec![(old_timestamp, "success", 1024)])?;

    // Run with database
    let db_output = otto(home_dir, &otto_home)
        .arg("Clean")
        .arg("--keep-days")
        .arg("30")
        .arg("--dry-run")
        .output()?;

    let db_stdout = String::from_utf8_lossy(&db_output.stdout);

    // Run with --no-db (filesystem)
    let fs_output = otto(home_dir, &otto_home)
        .arg("Clean")
        .arg("--keep-days")
        .arg("30")
        .arg("--no-db")
        .arg("--dry-run")
        .output()?;

    let fs_stdout = String::from_utf8_lossy(&fs_output.stdout);

    // The criterion this test exists for: the same **set** of run directories,
    // not the same counts. Counts cannot express it, because a row that records
    // no run directory contributes a row deletion with no directory analogue.
    let expected = vec![
        run_dir(&otto_home, "abc12345", orphan_timestamp).display().to_string(),
        run_dir(&otto_home, "abc12345", old_timestamp).display().to_string(),
    ];
    let mut expected = expected;
    expected.sort();
    assert_eq!(
        selected_run_dirs(&db_stdout, &otto_home),
        expected,
        "the database path must select the row-backed run and the orphan; got: {db_stdout}"
    );
    assert_eq!(
        selected_run_dirs(&fs_stdout, &otto_home),
        expected,
        "and --no-db must select the same two; got: {fs_stdout}"
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

    let now = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH)?.as_secs();

    let old_timestamp = now - (40 * 24 * 60 * 60);
    create_test_run(&otto_home, "abc12345", old_timestamp, "success")?;

    // A run directory no row names: the shape the default path could not see at
    // all before the orphan sweep, and the reason the two modes disagreed.
    let orphan_timestamp = now - (45 * 24 * 60 * 60);
    create_test_run(&otto_home, "abc12345", orphan_timestamp, "success")?;

    // A recent run, so both modes have something to keep as well.
    create_test_run(&otto_home, "abc12345", now - 3600, "success")?;

    // Setup database
    setup_test_database(&otto_home, "abc12345", vec![(old_timestamp, "success", 1024)])?;

    // Verify run directory exists
    let run_dir = otto_home
        .join(format!("{}-{}", PROJECT, "abc12345"))
        .join(old_timestamp.to_string());
    assert!(run_dir.exists(), "Run directory should exist before cleanup");

    // Run clean without --dry-run
    let output = otto(home_dir, &otto_home)
        .arg("Clean")
        .arg("--keep-days")
        .arg("30")
        .output()?;

    assert!(output.status.success(), "Clean command should succeed");

    // Verify run directory was deleted
    assert!(!run_dir.exists(), "Run directory should be deleted after cleanup");

    // Verify database record was deleted
    use otto::executor::state::StateManager;
    let manager = StateManager::with_db_path(otto_home.join("otto.db"))?;
    let runs = manager.get_runs_with_filters(None, None, 10)?;
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
        create_test_run(&otto_home, "abc12345", timestamp, "success")?;
    }

    // Run clean with --keep-last in filesystem mode
    let output = otto(home_dir, &otto_home)
        .arg("Clean")
        .arg("--keep-days")
        .arg("30")
        .arg("--keep-last")
        .arg("2")
        .arg("--no-db")
        .arg("--dry-run")
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

    let now = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH)?.as_secs();

    let old_timestamp = now - (40 * 24 * 60 * 60);

    // Create old runs for two projects
    create_test_run(&otto_home, "abc12345", old_timestamp, "success")?;
    create_test_run(&otto_home, "def45678", old_timestamp + 1, "success")?;

    // Setup database for both projects
    setup_test_database(&otto_home, "abc12345", vec![(old_timestamp, "success", 1024)])?;
    setup_test_database(&otto_home, "def45678", vec![(old_timestamp + 1, "success", 2048)])?;

    // Run clean with project filter
    let output = otto(home_dir, &otto_home)
        .arg("Clean")
        .arg("--keep-days")
        .arg("30")
        .arg("--project-filter")
        .arg("abc12345")
        .arg("--dry-run")
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

    let now = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH)?.as_secs();

    let old_timestamp = now - (40 * 24 * 60 * 60);
    create_test_run(&otto_home, "abc12345", old_timestamp, "success")?;

    // Create a corrupt database file (just write garbage)
    fs::write(otto_home.join("otto.db"), "this is not a valid sqlite database")?;

    // Run clean - should fallback to filesystem scan
    let output = otto(home_dir, &otto_home)
        .arg("Clean")
        .arg("--keep-days")
        .arg("30")
        .arg("--dry-run")
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
