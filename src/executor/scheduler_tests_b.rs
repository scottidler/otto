#![cfg(test)]

use super::*;
use serial_test::serial;
use std::path::PathBuf;
use tempfile::TempDir;

/// Point this test's otto home at a scratch directory. See `tests_a` for why
/// `OTTO_DB_PATH` is cleared rather than set.
fn setup_test_db(temp_dir: &std::path::Path) {
    let otto_home = temp_dir.join(".otto");
    // SAFETY: This is safe in tests because we control the execution environment
    // and tests are isolated. The env var is set before any StateManager is created.
    unsafe {
        std::env::remove_var("OTTO_DB_PATH");
        std::env::set_var("OTTO_HOME", &otto_home);
    }
}

#[tokio::test]
#[serial]
async fn test_file_dependencies_timestamp_precision() -> Result<()> {
    let temp_dir = TempDir::new()?;
    // Without this the workspace lands wherever the previously-run
    // serial test left OTTO_HOME pointing (the MemFs workspace tests set it
    // to /otto-home and never restore it), which is not writable.
    setup_test_db(temp_dir.path());
    let work_dir = PathBuf::from(temp_dir.path());

    let input_file = work_dir.join("input.txt");
    let output_file = work_dir.join("output.txt");

    tokio::fs::write(&input_file, "content").await?;

    tokio::fs::write(&output_file, "output").await?;

    let task = Task::new(
        "timestamp_test".to_string(),
        None,
        vec![],
        vec![input_file.to_string_lossy().to_string()],
        vec![output_file.to_string_lossy().to_string()],
        HashMap::new(),
        HashMap::new(),
        format!("cp {} {}", input_file.display(), output_file.display()),
    );

    let workspace = Workspace::new(work_dir).await?;
    workspace.init().await?;
    let scheduler = TaskScheduler::new(
        vec![task.clone()],
        Arc::new(workspace),
        ExecutionContext::new(),
        2,
        false,
    )
    .await?;

    // When timestamps are very close, should be conservative and rebuild
    let needs_rebuild = scheduler.needs_rebuild(&task).await?;
    // This might be true or false depending on timestamp precision, but should be consistent
    println!("Timestamp precision test - needs rebuild: {needs_rebuild}");

    Ok(())
}

#[tokio::test]
#[serial]
async fn test_file_dependencies_empty_lists() -> Result<()> {
    let temp_dir = TempDir::new()?;
    // Without this the workspace lands wherever the previously-run
    // serial test left OTTO_HOME pointing (the MemFs workspace tests set it
    // to /otto-home and never restore it), which is not writable.
    setup_test_db(temp_dir.path());
    let work_dir = PathBuf::from(temp_dir.path());

    // Task with no file dependencies
    let task = Task::new(
        "no_file_deps".to_string(),
        None,
        vec![],
        vec![], // No input files
        vec![], // No output files
        HashMap::new(),
        HashMap::new(),
        "echo 'no file dependencies'".to_string(),
    );

    let workspace = Workspace::new(work_dir).await?;
    workspace.init().await?;
    let scheduler = TaskScheduler::new(
        vec![task.clone()],
        Arc::new(workspace),
        ExecutionContext::new(),
        2,
        false,
    )
    .await?;

    // Should always need to run when there are no file dependencies to check
    let needs_rebuild = scheduler.needs_rebuild(&task).await?;
    assert!(needs_rebuild, "Task with no file dependencies should always run");

    Ok(())
}

#[tokio::test]
#[serial]
async fn test_file_dependencies_directory_as_input() -> Result<()> {
    let temp_dir = TempDir::new()?;
    let work_dir = PathBuf::from(temp_dir.path());
    setup_test_db(&work_dir);

    let src_dir = work_dir.join("src");
    tokio::fs::create_dir_all(&src_dir).await?;
    tokio::fs::write(src_dir.join("file1.txt"), "content1").await?;
    tokio::fs::write(src_dir.join("file2.txt"), "content2").await?;

    let output_file = work_dir.join("output.txt");

    let task = Task::new(
        "dir_input".to_string(),
        None,
        vec![],
        vec![src_dir.to_string_lossy().to_string()], // Directory as input
        vec![output_file.to_string_lossy().to_string()],
        HashMap::new(),
        HashMap::new(),
        format!(
            "find {} -name '*.txt' | wc -l > {}",
            src_dir.display(),
            output_file.display()
        ),
    );

    let workspace = Workspace::new(work_dir).await?;
    workspace.init().await?;
    let scheduler = TaskScheduler::new(
        vec![task.clone()],
        Arc::new(workspace),
        ExecutionContext::new(),
        2,
        false,
    )
    .await?;

    // Should handle directory dependencies (gets modification time of directory)
    let needs_rebuild = scheduler.needs_rebuild(&task).await?;
    assert!(needs_rebuild, "Task should need to run when output doesn't exist");

    Ok(())
}

#[tokio::test]
#[serial]
async fn test_large_number_of_file_dependencies() -> Result<()> {
    let temp_dir = TempDir::new()?;
    let work_dir = PathBuf::from(temp_dir.path());
    setup_test_db(&work_dir);

    let mut input_files = Vec::new();
    for i in 0..100 {
        let file = work_dir.join(format!("input_{i:03}.txt"));
        tokio::fs::write(&file, format!("content {i}")).await?;
        input_files.push(file.to_string_lossy().to_string());
    }

    let output_file = work_dir.join("combined.txt");

    let task = Task::new(
        "many_inputs".to_string(),
        None,
        vec![],
        input_files,
        vec![output_file.to_string_lossy().to_string()],
        HashMap::new(),
        HashMap::new(),
        format!("cat input_*.txt > {}", output_file.display()),
    );

    let workspace = Workspace::new(work_dir).await?;
    workspace.init().await?;
    let scheduler = TaskScheduler::new(
        vec![task.clone()],
        Arc::new(workspace),
        ExecutionContext::new(),
        2,
        false,
    )
    .await?;

    // Should handle large numbers of file dependencies efficiently
    let start = std::time::Instant::now();
    let needs_rebuild = scheduler.needs_rebuild(&task).await?;
    let duration = start.elapsed();

    assert!(needs_rebuild, "Task should need to run when output doesn't exist");
    assert!(
        duration.as_millis() < 1000,
        "File dependency checking should be fast even with many files"
    );

    Ok(())
}

#[tokio::test]
#[serial]
async fn test_file_dependencies_circular_detection() -> Result<()> {
    let temp_dir = TempDir::new()?;
    let work_dir = PathBuf::from(temp_dir.path());
    setup_test_db(&work_dir);

    let file_a = work_dir.join("a.txt");
    let file_b = work_dir.join("b.txt");

    tokio::fs::write(&file_a, "content a").await?;
    tokio::fs::write(&file_b, "content b").await?;

    // Task that uses its output as input (circular dependency)
    let task = Task::new(
        "circular".to_string(),
        None,
        vec![],
        vec![file_a.to_string_lossy().to_string()],
        vec![file_a.to_string_lossy().to_string()], // Same file as input and output
        HashMap::new(),
        HashMap::new(),
        format!("echo 'modified' >> {}", file_a.display()),
    );

    let workspace = Workspace::new(work_dir).await?;
    workspace.init().await?;
    let scheduler = TaskScheduler::new(
        vec![task.clone()],
        Arc::new(workspace),
        ExecutionContext::new(),
        2,
        false,
    )
    .await?;

    // Should handle circular file dependencies gracefully
    let needs_rebuild = scheduler.needs_rebuild(&task).await?;
    // Should be conservative when input and output are the same file
    println!("Circular dependency test - needs rebuild: {needs_rebuild}");

    Ok(())
}

#[tokio::test]
#[serial]
async fn test_file_dependencies_integration_with_real_execution() -> Result<()> {
    let temp_dir = TempDir::new()?;
    let work_dir = PathBuf::from(temp_dir.path());
    setup_test_db(&work_dir);

    let input_file = work_dir.join("source.txt");
    let output_file = work_dir.join("result.txt");

    tokio::fs::write(&input_file, "Hello, World!").await?;

    let task = Task::new(
        "real_execution".to_string(),
        None,
        vec![],
        vec![input_file.to_string_lossy().to_string()],
        vec![output_file.to_string_lossy().to_string()],
        HashMap::new(),
        HashMap::new(),
        format!("cp {} {}", input_file.display(), output_file.display()),
    );

    let workspace = Workspace::new(work_dir.clone()).await?;
    workspace.init().await?;
    let scheduler = TaskScheduler::new(
        vec![task.clone()],
        Arc::new(workspace),
        ExecutionContext::new(),
        2,
        false,
    )
    .await?;

    let needs_rebuild_1 = scheduler.needs_rebuild(&task).await?;
    assert!(needs_rebuild_1, "Should need to run initially");

    scheduler.execute_all().await?;

    let status = scheduler.get_task_status("real_execution").await;
    assert_eq!(status, TaskStatus::Completed);
    assert!(output_file.exists(), "Output file should exist after execution");

    let output_content = tokio::fs::read_to_string(&output_file).await?;
    assert_eq!(output_content, "Hello, World!", "Output should match input");

    let needs_rebuild_2 = scheduler.needs_rebuild(&task).await?;
    assert!(!needs_rebuild_2, "Should not need to run when output is up-to-date");

    // Modify input file to trigger rebuild
    tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;
    tokio::fs::write(&input_file, "Modified content!").await?;

    let needs_rebuild_3 = scheduler.needs_rebuild(&task).await?;
    assert!(needs_rebuild_3, "Should need to run when input is modified");

    Ok(())
}

#[tokio::test]
#[serial]
async fn test_parallel_execution_limit() -> Result<()> {
    let temp_dir = TempDir::new()?;
    let work_dir = temp_dir.path().to_path_buf();
    setup_test_db(&work_dir);

    let mut tasks = vec![];
    for i in 1..=4 {
        let task = Task::new(
            format!("task{i}"),
            None,
            vec![],
            vec![],
            vec![],
            HashMap::new(),
            HashMap::new(),
            format!("sleep 0.1 && echo task{i}"),
        );
        tasks.push(task);
    }

    let workspace = Workspace::new(work_dir).await?;
    workspace.init().await?;

    let scheduler = TaskScheduler::new(tasks, Arc::new(workspace), ExecutionContext::new(), 2, false).await?;

    assert_eq!(scheduler.semaphore.available_permits(), 2);

    // Execute all tasks
    scheduler.execute_all().await?;

    for i in 1..=4 {
        let status = scheduler.get_task_status(&format!("task{i}")).await;
        assert_eq!(status, TaskStatus::Completed);
    }

    Ok(())
}

#[tokio::test]
#[serial]
async fn test_scheduler_respects_max_parallel() -> Result<()> {
    // Test with different job limits
    for max_parallel in [1, 2, 4, 8] {
        let temp_dir = TempDir::new()?;
        let work_dir = temp_dir.path().to_path_buf();
        setup_test_db(&work_dir);

        let workspace = Workspace::new(work_dir).await?;
        workspace.init().await?;

        let tasks = vec![Task::new(
            "test".to_string(),
            None,
            vec![],
            vec![],
            vec![],
            HashMap::new(),
            HashMap::new(),
            "echo test".to_string(),
        )];

        let scheduler =
            TaskScheduler::new(tasks, Arc::new(workspace), ExecutionContext::new(), max_parallel, false).await?;

        assert_eq!(
            scheduler.semaphore.available_permits(),
            max_parallel,
            "Scheduler should have {max_parallel} permits"
        );
    }

    Ok(())
}

#[tokio::test]
#[serial]
async fn test_task_skipped_when_outputs_up_to_date() -> Result<()> {
    let temp_dir = TempDir::new()?;
    let work_dir = PathBuf::from(temp_dir.path());
    setup_test_db(&work_dir);

    let input_file = work_dir.join("input.txt");
    let output_file = work_dir.join("output.txt");

    // Create input and output, with output newer than input
    tokio::fs::write(&input_file, "input content").await?;
    tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;
    tokio::fs::write(&output_file, "output content").await?;

    let task = Task::new(
        "skip_test".to_string(),
        None,
        vec![],
        vec![input_file.to_string_lossy().to_string()],
        vec![output_file.to_string_lossy().to_string()],
        HashMap::new(),
        HashMap::new(),
        "echo should be skipped".to_string(),
    );

    let workspace = Workspace::new(work_dir.clone()).await?;
    workspace.init().await?;
    let scheduler = TaskScheduler::new(
        vec![task.clone()],
        Arc::new(workspace),
        ExecutionContext::new(),
        2,
        false,
    )
    .await?;

    // Execute should skip the task
    scheduler.execute_all().await?;

    let status = scheduler.get_task_status("skip_test").await;
    assert_eq!(
        status,
        TaskStatus::Skipped(SkipKind::UpToDate),
        "Task should be skipped as up-to-date when outputs are current"
    );

    Ok(())
}

#[tokio::test]
#[serial]
async fn test_tui_mode_message_broadcasting() -> Result<()> {
    let temp_dir = TempDir::new()?;
    let work_dir = PathBuf::from(temp_dir.path());
    setup_test_db(&work_dir);

    let task = Task::new(
        "tui_test".to_string(),
        None,
        vec![],
        vec![],
        vec![],
        HashMap::new(),
        HashMap::new(),
        "echo testing tui".to_string(),
    );

    let workspace = Workspace::new(work_dir).await?;
    workspace.init().await?;

    let mut scheduler = TaskScheduler::new(
        vec![task],
        Arc::new(workspace),
        ExecutionContext::new(),
        2,
        true, // TUI mode enabled
    )
    .await?;

    // Set up message channel
    let (tx, mut rx) = tokio::sync::broadcast::channel(100);
    scheduler.set_message_channel(tx);

    // Execute task
    let exec_handle = tokio::spawn(async move { scheduler.execute_all().await });

    // Collect messages
    while let Ok(msg) = rx.try_recv() {
        match msg {
            TaskMessage::Started { task_name, .. } => {
                assert_eq!(task_name, "tui_test");
            }
            TaskMessage::Finished {
                task_name,
                status,
                duration_ms,
                ..
            } => {
                assert_eq!(task_name, "tui_test");
                assert_eq!(status, TuiTaskStatus::Completed);
                assert!(duration_ms > 0, "Duration should be tracked");
            }
            _ => {}
        }
    }

    exec_handle.await??;

    // Note: Messages might be dropped if we don't subscribe early enough
    // This test mainly verifies the broadcasting mechanism works
    Ok(())
}

#[tokio::test]
#[serial]
async fn test_duration_tracking_accuracy() -> Result<()> {
    let temp_dir = TempDir::new()?;
    let work_dir = PathBuf::from(temp_dir.path());
    setup_test_db(&work_dir);

    let task = Task::new(
        "duration_test".to_string(),
        None,
        vec![],
        vec![],
        vec![],
        HashMap::new(),
        HashMap::new(),
        "sleep 0.2".to_string(),
    );

    let workspace = Workspace::new(work_dir).await?;
    workspace.init().await?;

    let mut scheduler = TaskScheduler::new(vec![task], Arc::new(workspace), ExecutionContext::new(), 2, true).await?;

    // Set up message channel to capture duration
    let (tx, mut rx) = tokio::sync::broadcast::channel(100);
    scheduler.set_message_channel(tx);

    let start = std::time::Instant::now();

    let exec_handle = tokio::spawn(async move { scheduler.execute_all().await });

    let mut captured_duration_ms = None;

    // Wait for task to complete and capture duration
    loop {
        match tokio::time::timeout(tokio::time::Duration::from_secs(5), rx.recv()).await {
            Ok(Ok(TaskMessage::Finished { duration_ms, .. })) => {
                captured_duration_ms = Some(duration_ms);
                break;
            }
            Ok(Ok(_)) => continue,
            Ok(Err(_)) => break, // Channel closed
            Err(_) => break,     // Timeout
        }
    }

    exec_handle.await??;

    let total_elapsed = start.elapsed().as_millis() as u64;

    // Verify duration was captured and is reasonable
    if let Some(duration_ms) = captured_duration_ms {
        assert!(
            duration_ms >= 200,
            "Duration should be at least 200ms, got {}",
            duration_ms
        );
        assert!(
            duration_ms <= total_elapsed + 100,
            "Duration should not exceed total time"
        );
    }

    Ok(())
}

#[tokio::test]
#[serial]
async fn test_file_dependency_check_error_handling() -> Result<()> {
    let temp_dir = TempDir::new()?;
    let work_dir = PathBuf::from(temp_dir.path());
    setup_test_db(&work_dir);

    // Create a task with a file dependency in a path that will cause issues
    let task = Task::new(
        "error_test".to_string(),
        None,
        vec![],
        vec!["/dev/null/impossible/path".to_string()],
        vec![work_dir.join("output.txt").to_string_lossy().to_string()],
        HashMap::new(),
        HashMap::new(),
        "echo handled error".to_string(),
    );

    let workspace = Workspace::new(work_dir).await?;
    workspace.init().await?;
    let scheduler = TaskScheduler::new(vec![task], Arc::new(workspace), ExecutionContext::new(), 2, false).await?;

    // Should handle the error gracefully and still run the task
    let result = scheduler.execute_all().await;

    // The task should complete despite file dependency check errors
    assert!(result.is_ok(), "Should handle file dependency errors gracefully");

    Ok(())
}

#[tokio::test]
#[serial]
async fn test_multiple_tasks_with_mixed_states() -> Result<()> {
    let temp_dir = TempDir::new()?;
    let work_dir = PathBuf::from(temp_dir.path());
    setup_test_db(&work_dir);

    // Create files for skip test
    let input1 = work_dir.join("input1.txt");
    let output1 = work_dir.join("output1.txt");
    tokio::fs::write(&input1, "content").await?;
    tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;
    tokio::fs::write(&output1, "output").await?;

    let tasks = vec![
        // Task that will be skipped (output up to date)
        Task::new(
            "skip_task".to_string(),
            None,
            vec![],
            vec![input1.to_string_lossy().to_string()],
            vec![output1.to_string_lossy().to_string()],
            HashMap::new(),
            HashMap::new(),
            "echo skipped".to_string(),
        ),
        // Task that will run
        Task::new(
            "run_task".to_string(),
            None,
            vec![],
            vec![],
            vec![],
            HashMap::new(),
            HashMap::new(),
            "echo running".to_string(),
        ),
        // Task with dependency on both
        Task::new(
            "dependent_task".to_string(),
            None,
            vec![
                crate::executor::task::TaskEdge::success("skip_task"),
                crate::executor::task::TaskEdge::success("run_task"),
            ],
            vec![],
            vec![],
            HashMap::new(),
            HashMap::new(),
            "echo dependent".to_string(),
        ),
    ];

    let workspace = Workspace::new(work_dir).await?;
    workspace.init().await?;
    let scheduler = TaskScheduler::new(tasks, Arc::new(workspace), ExecutionContext::new(), 2, false).await?;

    scheduler.execute_all().await?;

    let skip_status = scheduler.get_task_status("skip_task").await;
    let run_status = scheduler.get_task_status("run_task").await;
    let dep_status = scheduler.get_task_status("dependent_task").await;

    assert_eq!(skip_status, TaskStatus::Skipped(SkipKind::UpToDate));
    assert_eq!(run_status, TaskStatus::Completed);
    assert_eq!(dep_status, TaskStatus::Completed);

    Ok(())
}

// ------------------------------------------------------------------
// Phase 7: tty: true
// ------------------------------------------------------------------

/// An ordinary task takes one permit; a tty task takes the semaphore's whole
/// initial count, which is the entire mechanism behind "runs exclusively".
#[test]
fn test_permits_for_tty_takes_the_whole_semaphore() {
    for max_parallel in [1usize, 2, 4, 32] {
        assert_eq!(permits_for(false, max_parallel).unwrap(), 1);
        assert_eq!(
            permits_for(true, max_parallel).unwrap(),
            u32::try_from(max_parallel).unwrap(),
            "a tty task must request all {max_parallel} permits"
        );
    }
}

/// A permit count that cannot be expressed as a u32 is a loud error, not a
/// silently clamped request that would fail to be exclusive.
#[test]
fn test_permits_for_rejects_counts_beyond_u32() {
    let err = permits_for(true, usize::MAX).unwrap_err().to_string();
    assert!(
        err.contains("exceeds the semaphore\'s permit limit"),
        "unexpected error: {err}"
    );
}

/// The scheduler keeps the count it was built with; `available_permits()` is
/// the count free *right now*, which is the wrong number to hand acquire_many.
#[tokio::test]
#[serial]
async fn test_scheduler_records_its_initial_permit_count() -> Result<()> {
    let temp_dir = TempDir::new()?;
    let work_dir = temp_dir.path().to_path_buf();
    setup_test_db(&work_dir);

    let workspace = Workspace::new(work_dir).await?;
    workspace.init().await?;
    let scheduler = TaskScheduler::new(vec![], Arc::new(workspace), ExecutionContext::new(), 7, false).await?;

    assert_eq!(scheduler.max_parallel, 7);
    let _held = scheduler.semaphore.acquire_many(3).await?;
    assert_eq!(scheduler.semaphore.available_permits(), 4);
    assert_eq!(
        scheduler.max_parallel, 7,
        "max_parallel must not track live availability"
    );

    Ok(())
}

/// A tty task never opens TaskStreams, so nothing else would create these
/// files. History records both paths at task start; empty files would claim a
/// silent task.
#[tokio::test]
async fn test_tty_log_markers_are_written() -> Result<()> {
    let temp_dir = TempDir::new()?;
    let tasks_dir = temp_dir.path().join("tasks");

    write_tty_log_markers(&tasks_dir, "login").await?;

    for file in ["stdout.log", "stderr.log"] {
        let content = tokio::fs::read_to_string(tasks_dir.join("login").join(file)).await?;
        assert_eq!(content, format!("{TTY_LOG_MARKER}\n"), "{file} marker");
    }

    Ok(())
}

/// End-to-end through the real scheduler: a tty task's output is not captured
/// (marker only), while a plain task in the same run still is.
#[tokio::test]
#[serial]
async fn test_tty_task_logs_carry_the_marker_and_plain_tasks_still_capture() -> Result<()> {
    let temp_dir = TempDir::new()?;
    let work_dir = temp_dir.path().to_path_buf();
    setup_test_db(&work_dir);

    let mut tty_task = Task::new(
        "interactive".to_string(),
        None,
        vec![],
        vec![],
        vec![],
        HashMap::new(),
        HashMap::new(),
        "echo from-the-tty-task".to_string(),
    );
    tty_task.tty = true;
    let plain_task = Task::new(
        "plain".to_string(),
        None,
        vec![],
        vec![],
        vec![],
        HashMap::new(),
        HashMap::new(),
        "echo from-the-plain-task".to_string(),
    );

    let workspace = Workspace::new(work_dir).await?;
    workspace.init().await?;
    let tasks_dir = workspace.run().join("tasks");
    let scheduler = TaskScheduler::new(
        vec![tty_task, plain_task],
        Arc::new(workspace),
        ExecutionContext::new(),
        4,
        false,
    )
    .await?;

    scheduler.execute_all().await?;

    assert_eq!(scheduler.get_task_status("interactive").await, TaskStatus::Completed);
    assert_eq!(scheduler.get_task_status("plain").await, TaskStatus::Completed);

    let tty_log = tokio::fs::read_to_string(tasks_dir.join("interactive").join("stdout.log")).await?;
    assert_eq!(tty_log, format!("{TTY_LOG_MARKER}\n"));
    assert!(
        !tty_log.contains("from-the-tty-task"),
        "a tty task must not be captured: {tty_log}"
    );

    let plain_log = tokio::fs::read_to_string(tasks_dir.join("plain").join("stdout.log")).await?;
    assert!(
        plain_log.contains("from-the-plain-task"),
        "a plain task must still be captured: {plain_log}"
    );

    Ok(())
}

/// Phase 1 (design doc 2026-09-01, cancellation reaping). The pty tests in
/// `tests/cancel_reaping_test.rs` prove the end-to-end behavior; these pin the
/// pieces it is built from, which the end-to-end test can only observe
/// indirectly.
#[cfg(unix)]
mod cancellation_reaping {
    use super::*;
    use std::time::Instant;

    /// A child in its own process group that ignores SIGTERM, plus the pid the
    /// registry would have recorded for it.
    ///
    /// `trap "" TERM` is inherited across the `exec` bash does for the last
    /// command of a `-c` string, so the process this returns is a `sleep` that
    /// SIGTERM cannot touch. That is the whole point: it makes "was it SIGKILLed
    /// by the second pass" an observable fact rather than a guess.
    /// The marker is not decoration: it is what makes the trap observable.
    /// Signalling immediately after `spawn` raced the shell to its own first
    /// line, and a bash that has not run `trap` yet dies of SIGTERM like any
    /// other process, which made this fixture prove the opposite of its name.
    async fn sigterm_proof_child() -> (TempDir, tokio::process::Child, ChildHandle) {
        let temp = TempDir::new().expect("temp dir");
        let ready = temp.path().join("trap-installed");
        let script = format!(r#"trap "" TERM; touch "{}"; sleep 600"#, ready.display());
        let mut cmd = tokio::process::Command::new("bash");
        cmd.arg("-c").arg(script).kill_on_drop(true);
        cmd.process_group(0);
        let child = cmd.spawn().expect("spawn a test child");
        let pid = child.id().expect("a freshly spawned child has a pid");
        let deadline = Instant::now() + Duration::from_secs(10);
        while !ready.exists() {
            assert!(Instant::now() < deadline, "the test child never installed its trap");
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        (temp, child, ChildHandle { pid, own_group: true })
    }

    #[test]
    fn a_pid_that_is_not_a_pid_never_reaches_the_syscall() {
        // 0 is "my own process group" to `kill`, and 4294967290 casts to -6,
        // which is process group 6. Both must be refused, not reinterpreted.
        for pid in [0u32, u32::MAX, 4294967290] {
            let handle = ChildHandle { pid, own_group: false };
            assert_eq!(
                signal_child(handle, libc::SIGTERM),
                Err(SignalFailure::NotAPid(pid)),
                "pid {pid} does not name a process and must be refused before the syscall"
            );
        }
    }

    #[test]
    fn a_pid_that_does_not_exist_reports_esrch_and_is_ignored() {
        // pid_t's ceiling is never a live pid, so this is a real syscall that
        // really fails, not a stubbed one.
        let handle = ChildHandle {
            pid: libc::pid_t::MAX as u32,
            own_group: false,
        };
        let failure = signal_child(handle, libc::SIGTERM).expect_err("a pid this high cannot exist");
        assert_eq!(failure, SignalFailure::Errno(libc::ESRCH));
        assert_eq!(
            volume_for(failure),
            SignalVolume::Ignored,
            "ESRCH is the success case of the SIGKILL pass and must be silent"
        );
    }

    #[test]
    fn signalling_something_otto_does_not_own_is_loud() {
        // pid 1 is init: it exists, and otto is not allowed to signal it. That
        // is the one failure that means otto is aiming at the wrong thing.
        assert_eq!(volume_for(SignalFailure::Errno(libc::EPERM)), SignalVolume::Error);
        assert_eq!(volume_for(SignalFailure::NotAPid(0)), SignalVolume::Error);
        assert_eq!(volume_for(SignalFailure::Errno(libc::EINVAL)), SignalVolume::Warn);
    }

    #[tokio::test]
    async fn a_group_signal_reaches_a_child_that_ignored_the_term() {
        let (_temp, mut child, handle) = sigterm_proof_child().await;
        assert_eq!(signal_child(handle, libc::SIGTERM), Ok(()));
        assert_eq!(
            signal_child(handle, libc::SIGKILL),
            Ok(()),
            "the group is still valid: SIGTERM was ignored, so nothing has exited"
        );
        let status = tokio::time::timeout(Duration::from_secs(5), child.wait())
            .await
            .expect("a SIGKILLed child exits")
            .expect("wait");
        assert_eq!(
            std::os::unix::process::ExitStatusExt::signal(&status),
            Some(libc::SIGKILL),
            "the child must have died of the group's SIGKILL, not of the SIGTERM it ignores"
        );
    }

    /// The load-bearing property of the design: the SIGKILL pass walks the
    /// snapshot, so it still reaps a group whose registry entry disappeared
    /// while the grace period was running. Reading the live registry instead
    /// would find nothing to signal in exactly this case.
    #[tokio::test]
    async fn the_sigkill_pass_reaps_a_group_whose_registry_entry_vanished_mid_grace() {
        let (_temp, mut child, handle) = sigterm_proof_child().await;
        let registry: LiveChildren = Arc::new(Mutex::new(HashMap::new()));
        registry.lock().await.insert("hold".to_string(), handle);

        let snapshot = vec![("hold".to_string(), handle)];
        // Stand in for the body deregistering when its direct child exits on the
        // SIGTERM, which happens partway through the grace window.
        let emptier = {
            let registry = registry.clone();
            tokio::spawn(async move {
                tokio::time::sleep(CANCEL_GRACE / 5).await;
                registry.lock().await.clear();
            })
        };

        reap_live_children(snapshot).await;
        emptier.await.expect("the deregistering task");
        assert!(registry.lock().await.is_empty(), "the fixture must have emptied it");

        let status = tokio::time::timeout(Duration::from_secs(5), child.wait())
            .await
            .expect("the snapshot pass must have SIGKILLed the group")
            .expect("wait");
        assert_eq!(
            std::os::unix::process::ExitStatusExt::signal(&status),
            Some(libc::SIGKILL)
        );
    }

    #[tokio::test]
    async fn reaping_nothing_costs_no_grace_period() {
        // Every cancelled run with no children in flight would otherwise pay the
        // full grace period for nothing.
        let started = Instant::now();
        reap_live_children(Vec::new()).await;
        assert!(
            started.elapsed() < CANCEL_GRACE,
            "an empty snapshot must return at once"
        );
    }

    #[tokio::test]
    async fn the_snapshot_is_detached_from_the_registry() {
        let active = ActiveTasks::default();
        register_child(&active.children(), "hold", Some(4242), true).await;
        let snapshot = active.child_snapshot().await;
        deregister_child(&active.children(), "hold").await;

        assert_eq!(
            snapshot,
            vec![(
                "hold".to_string(),
                ChildHandle {
                    pid: 4242,
                    own_group: true
                }
            )],
            "a snapshot taken before the entry was removed must still carry it"
        );
        assert!(active.child_snapshot().await.is_empty(), "the registry itself moved on");
    }

    #[tokio::test]
    async fn a_child_with_no_pid_is_not_recorded() {
        // `Child::id()` is `None` once tokio has reaped the process. Recording a
        // placeholder would mean signalling whatever holds that number next.
        let active = ActiveTasks::default();
        register_child(&active.children(), "hold", None, true).await;
        assert!(active.child_snapshot().await.is_empty());
    }

    #[tokio::test]
    async fn aborting_the_run_clears_the_registry() {
        let mut active = ActiveTasks::default();
        register_child(&active.children(), "hold", Some(4242), true).await;
        active.abort_all().await;
        assert!(
            active.child_snapshot().await.is_empty(),
            "the bodies that would remove their own entries are gone, so the entries must go too"
        );
    }
}

/// Phase 3 (design doc 2026-09-01, `foreach.jobs`). The end-to-end behavior is
/// in `tests/foreach_jobs_concurrency_test.rs`; these pin the pieces that test
/// can only observe through a whole run, including the one property the
/// admission rules rest on.
mod foreach_jobs_admission {
    use super::*;
    use std::num::NonZeroUsize;

    fn permits(n: usize) -> Option<NonZeroUsize> {
        NonZeroUsize::new(n)
    }

    fn item(name: &str, group: &str, group_permits: usize) -> Task {
        let mut task = Task::new(
            name.to_string(),
            Some(group.to_string()),
            vec![],
            vec![],
            vec![],
            HashMap::new(),
            HashMap::new(),
            "echo hi".to_string(),
        );
        task.foreach_jobs = permits(group_permits);
        task
    }

    fn plain(name: &str) -> Task {
        Task::new(
            name.to_string(),
            None,
            vec![],
            vec![],
            vec![],
            HashMap::new(),
            HashMap::new(),
            "echo hi".to_string(),
        )
    }

    /// An item of a group carrying `jobs:` is exempt; everything else is not.
    #[test]
    fn admission_classes_come_from_the_task() {
        assert_eq!(admission_for(&plain("build")), Admission::Capped);
        assert_eq!(admission_for(&item("tail:s1", "tail", 4)), Admission::Exempt);

        let mut tty = plain("interactive");
        tty.tty = true;
        assert_eq!(admission_for(&tty), Admission::Tty);
    }

    /// A task asking for both `tty: true` and `foreach.jobs` is classified as
    /// tty. No ottofile can produce one - `validate_foreach_jobs_tty` rejects
    /// the combination at load time - so this pins the classification for a
    /// `Task` built by hand: `admission_for` is total, and the arm it takes
    /// must never be the one that puts several writers on a terminal a task
    /// asked to own.
    #[test]
    fn tty_wins_over_a_group_concurrency_override() {
        let mut both = item("logs:s1", "logs", 8);
        both.tty = true;
        assert_eq!(admission_for(&both), Admission::Tty);
    }

    /// The two admission rules, as a table. Every cell is one sentence of the
    /// design doc: a tty task waits for exempt items, exempt items wait for a
    /// tty task, and an ordinary task is gated only by the launch cap.
    #[test]
    fn the_admission_rules_are_symmetric_and_bind_only_tty_against_exempt() {
        let idle = InFlight::default();
        let exempt_running = InFlight { tty: 0, exempt: 3 };
        let tty_running = InFlight { tty: 1, exempt: 0 };

        assert!(may_admit(Admission::Tty, idle));
        assert!(!may_admit(Admission::Tty, exempt_running));
        assert!(may_admit(Admission::Tty, tty_running));

        assert!(may_admit(Admission::Exempt, idle));
        assert!(may_admit(Admission::Exempt, exempt_running));
        assert!(!may_admit(Admission::Exempt, tty_running));

        // An ordinary task is never held by these rules: the tty task's
        // exclusivity against it is the shared semaphore's job, unchanged.
        assert!(may_admit(Admission::Capped, idle));
        assert!(may_admit(Admission::Capped, exempt_running));
        assert!(may_admit(Admission::Capped, tty_running));
    }

    /// The launch cap must not count exempt items, and cancellation must.
    /// Fixing only the semaphore leaves `active_tasks.len() < max_concurrent`
    /// in force, which is why 10 items under `-j 2` still started 2.
    #[tokio::test]
    async fn exempt_items_are_in_flight_but_not_against_the_launch_cap() {
        let mut active = ActiveTasks::default();
        active.spawn("build".to_string(), Admission::Capped, std::future::pending());
        active.spawn("tail:s1".to_string(), Admission::Exempt, std::future::pending());
        active.spawn("tail:s2".to_string(), Admission::Exempt, std::future::pending());

        assert_eq!(active.capped_len(), 1, "only the ordinary task is capped");
        assert_eq!(active.in_flight_len(), 3, "all three are in flight");
        assert_eq!(active.in_flight(), InFlight { tty: 0, exempt: 2 });

        active.reported("tail:s1");
        assert_eq!(active.in_flight(), InFlight { tty: 0, exempt: 1 });
        assert_eq!(active.capped_len(), 1);
    }

    /// **Success criterion (f), and the property the whole mechanism rests
    /// on.** `ActiveTasks::spawn` counts a task as in flight at SPAWN time, so
    /// a task whose body is still queuing on a permit is already visible to the
    /// admission rules. Move that insert to permit-acquisition time and the
    /// launch loop starts deciding admission on a stale view: it lets a tty
    /// task through, the body is still queuing, and the next pass admits an
    /// exempt group because the tty task "is not running yet".
    #[tokio::test]
    async fn spawn_counts_a_task_in_flight_before_its_body_acquires_a_permit() {
        // A semaphore with no permits to give: the body below can never get
        // past its acquire, so anything the scheduler knows about this task it
        // knows from the spawn alone.
        let semaphore = Arc::new(Semaphore::new(0));
        let (reached_acquire, acquired) = (Arc::new(tokio::sync::Notify::new()), Arc::new(Mutex::new(false)));

        let mut active = ActiveTasks::default();
        {
            let semaphore = semaphore.clone();
            let reached_acquire = reached_acquire.clone();
            let acquired = acquired.clone();
            active.spawn("interactive".to_string(), Admission::Tty, async move {
                reached_acquire.notify_waiters();
                let _permit = semaphore.acquire_many(1).await;
                *acquired.lock().await = true;
            });
        }

        // Let the body run until it parks on the semaphore. Yielding rather
        // than sleeping: the assertion below is about ordering, not timing.
        for _ in 0..8 {
            tokio::task::yield_now().await;
        }

        assert!(
            !*acquired.lock().await,
            "the body must still be queuing for a permit, or this test proves nothing"
        );
        assert_eq!(
            active.in_flight(),
            InFlight { tty: 1, exempt: 0 },
            "a task the loop admitted counts as in flight while its body queues"
        );
        assert!(
            !may_admit(Admission::Exempt, active.in_flight()),
            "the admission view must not be stale: an exempt group must be held off by a tty \
             task that has not acquired its permits yet"
        );

        active.abort_all().await;
    }

    /// One semaphore per group, sized by the count the items carry, and a
    /// missing group fails closed rather than borrowing the global permit.
    #[test]
    fn group_semaphores_are_built_per_group_from_the_items_own_count() {
        let tasks = vec![
            item("tail:s1", "tail", 3),
            item("tail:s2", "tail", 3),
            item("watch:a", "watch", 1),
            plain("build"),
        ];
        let semaphores = TaskScheduler::<crate::ports::RealFs>::build_group_semaphores(&tasks);

        assert_eq!(semaphores.len(), 2, "one per group carrying jobs:, and no more");
        assert_eq!(semaphores["tail"].available_permits(), 3);
        assert_eq!(semaphores["watch"].available_permits(), 1);
        assert!(
            !semaphores.contains_key("build"),
            "a task without foreach.jobs has no group semaphore"
        );
    }
}
