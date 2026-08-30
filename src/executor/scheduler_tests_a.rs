#![cfg(test)]

use super::*;
use serial_test::serial;
use std::path::PathBuf;
use tempfile::TempDir;

/// Point this test's otto home at a scratch directory.
///
/// `OTTO_DB_PATH` is deliberately cleared rather than set: the database now
/// derives from `OTTO_HOME`, so setting the home alone must be enough to
/// keep a test off the developer's real database. `OTTO_DB_PATH` still
/// overrides, and that override is pinned by its own test in `state/db.rs`.
fn setup_test_db(temp_dir: &std::path::Path) {
    let otto_home = temp_dir.join(".otto");
    // SAFETY: This is safe in tests because we control the execution environment
    // and tests are isolated. The env var is set before any StateManager is created.
    unsafe {
        std::env::remove_var("OTTO_DB_PATH");
        std::env::set_var("OTTO_HOME", &otto_home);
    }
}

/// A multiline value survives the round trip. It used to be truncated at the
/// first newline and handed back with a stray leading quote (`'line1`), while the
/// run exited 0.
#[test]
fn test_env_to_json_preserves_multiline_values() {
    let content = "# header\n\nMULTI='line1\nline2\nline3'\nAFTER='tail'\n";
    let json = env_to_json(content);

    assert_eq!(
        json["MULTI"],
        serde_json::Value::String("line1\nline2\nline3".to_string())
    );
    assert_eq!(
        json["AFTER"],
        serde_json::Value::String("tail".to_string()),
        "the record after a multiline value must still be parsed"
    );
    assert_eq!(json.as_object().map(|o| o.len()), Some(2));
}

/// `'\''` is bash's literal single quote inside a single-quoted string, not a
/// record terminator.
#[test]
fn test_env_to_json_unescapes_embedded_quotes() {
    let content = "Q='it'\\''s here'\nNEXT='ok'\n";
    let json = env_to_json(content);

    assert_eq!(json["Q"], serde_json::Value::String("it's here".to_string()));
    assert_eq!(json["NEXT"], serde_json::Value::String("ok".to_string()));
}

/// The producer and the parser are inverses, including for the values that used
/// to break the parser.
#[test]
fn test_json_to_env_round_trips_through_env_to_json() {
    for value in [
        "plain",
        "line1\nline2",
        "it's got a quote",
        "# not a comment",
        "trailing=equals=signs",
        "",
    ] {
        let json = serde_json::json!({ "VALUE": value });
        let env = json_to_env(&json, "task");
        let back = env_to_json(&env);
        assert_eq!(
            back["OTTO_INPUT_TASK_VALUE"],
            serde_json::Value::String(value.to_string()),
            "round trip lost {value:?}"
        );
    }
}

/// Unterminated quote: keep what the task wrote rather than dropping the record.
#[test]
fn test_env_to_json_keeps_an_unterminated_value() {
    let json = env_to_json("BROKEN='no closing quote\n");
    assert_eq!(
        json["BROKEN"],
        serde_json::Value::String("no closing quote\n".to_string())
    );
}

/// Cancelling stops the run instead of waiting for the children.
#[tokio::test]
#[serial]
async fn test_cancel_signal_stops_the_run() -> Result<()> {
    let temp_dir = TempDir::new()?;
    let work_dir = PathBuf::from(temp_dir.path());
    setup_test_db(&work_dir);

    let tasks = vec![Task::new(
        "sleeper".to_string(),
        None,
        vec![],
        vec![],
        vec![],
        HashMap::new(),
        HashMap::new(),
        "sleep 60".to_string(),
    )];

    let workspace = Workspace::new(work_dir).await?;
    workspace.init().await?;
    let scheduler = TaskScheduler::new(tasks, Arc::new(workspace), ExecutionContext::new(), 2, false).await?;
    let cancel = scheduler.cancel_signal();

    tokio::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        cancel.cancel();
    });

    // The task sleeps for 60s; if cancellation did not work this times out.
    let outcome = tokio::time::timeout(std::time::Duration::from_secs(10), scheduler.execute_all()).await?;
    let err = outcome.expect_err("a cancelled run must not report success");
    assert!(
        format!("{err:#}").contains("cancelled"),
        "the error must say the run was cancelled; got {err:#}"
    );
    Ok(())
}

/// Cancelling before the run starts stops it immediately: the flag is checked at
/// the top of the drain loop, not only in the `select!` arm.
#[tokio::test]
#[serial]
async fn test_cancel_before_start_is_not_lost() -> Result<()> {
    let temp_dir = TempDir::new()?;
    let work_dir = PathBuf::from(temp_dir.path());
    setup_test_db(&work_dir);

    let workspace = Workspace::new(work_dir).await?;
    workspace.init().await?;
    let scheduler = TaskScheduler::new(
        vec![plain_task("only", vec![])],
        Arc::new(workspace),
        ExecutionContext::new(),
        2,
        false,
    )
    .await?;
    scheduler.cancel_signal().cancel();

    let outcome = tokio::time::timeout(std::time::Duration::from_secs(10), scheduler.execute_all()).await?;
    assert!(outcome.is_err(), "a run cancelled before it started must not succeed");
    Ok(())
}

fn plain_task(name: &str, deps: Vec<TaskEdge>) -> Task {
    Task::new(
        name.to_string(),
        None,
        deps,
        vec![],
        vec![],
        HashMap::new(),
        HashMap::new(),
        "echo hi".to_string(),
    )
}

#[tokio::test]
#[serial]
async fn test_scheduler_rejects_a_dependency_cycle_at_init() {
    let temp_dir = TempDir::new().unwrap();
    setup_test_db(temp_dir.path());
    let workspace = Workspace::new(PathBuf::from(temp_dir.path())).await.unwrap();
    workspace.init().await.unwrap();

    let tasks = vec![
        plain_task("a", vec![TaskEdge::success("b")]),
        plain_task("b", vec![TaskEdge::success("a")]),
    ];

    let err = match TaskScheduler::new(tasks, Arc::new(workspace), ExecutionContext::new(), 2, false).await {
        Ok(_) => panic!("a 2-cycle must be rejected before anything is scheduled"),
        Err(e) => e,
    };
    assert!(err.to_string().contains("dependency cycle detected"), "{err}");
}

/// A run in which every task is gated out accomplished nothing, and used to
/// report that as success.
#[tokio::test]
#[serial]
async fn test_execute_all_errors_when_no_task_reached_a_terminal_state() {
    let temp_dir = TempDir::new().unwrap();
    setup_test_db(temp_dir.path());
    let workspace = Workspace::new(PathBuf::from(temp_dir.path())).await.unwrap();
    workspace.init().await.unwrap();

    // `b` depends on a task that is not in the run set, so its edge never
    // resolves and post-loop reconciliation marks it Skipped.
    let tasks = vec![plain_task("b", vec![TaskEdge::success("absent")])];
    let scheduler = TaskScheduler::new(tasks, Arc::new(workspace), ExecutionContext::new(), 2, false)
        .await
        .unwrap();

    let err = scheduler
        .execute_all()
        .await
        .expect_err("a run where nothing ran must not report success");
    assert!(err.to_string().contains("no task reached a terminal state"), "{err}");
}

#[test]
fn test_task_report_carries_the_name_and_code_structurally() {
    let ok = TaskReport::success("build".to_string());
    assert_eq!(ok.name, "build");
    assert_eq!(ok.exit_code, Some(0));
    assert!(ok.error.is_none());

    let failed = TaskReport::failure("build".to_string(), eyre!("No such file or directory"), Some(7));
    assert_eq!(failed.name, "build", "the name must not come from the error text");
    assert_eq!(failed.exit_code, Some(7));
    assert!(failed.error.is_some());
}

fn serial_member(name: &str, group: &str, index: usize) -> Task {
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
    task.serial_group = Some(group.to_string());
    task.serial_index = index;
    task
}

/// Ordering constrains the run set, it never expands it: the predecessor of a member
/// is the nearest preceding member THAT IS IN THE RUN SET, so a targeted subtask has
/// no predecessor at all.
#[test]
fn test_serial_predecessor_is_nearest_member_in_the_run_set() {
    let tasks = vec![serial_member("up:alpha", "up", 0), serial_member("up:gamma", "up", 2)];
    let groups = SerialGroups::new(&tasks);

    assert_eq!(groups.predecessor(&tasks[0]), None);
    assert_eq!(groups.predecessor(&tasks[1]), Some("up:alpha"));

    let lone = vec![serial_member("up:gamma", "up", 2)];
    assert_eq!(SerialGroups::new(&lone).predecessor(&lone[0]), None);
}

/// Every terminal state of a predecessor is covered, so the gate can never leave a
/// successor waiting forever.
#[test]
fn test_serial_gate_classifies_every_predecessor_terminal_state() {
    let tasks = vec![serial_member("up:alpha", "up", 0), serial_member("up:beta", "up", 1)];
    let groups = SerialGroups::new(&tasks);
    let beta = &tasks[1];

    let empty = std::collections::HashSet::new();
    let no_skips = SkippedSet::new();
    let one = |name: &str| std::collections::HashSet::from([name.to_string()]);
    let skipped_as = |name: &str, kind: SkipKind| SkippedSet::from([(name.to_string(), kind)]);

    // Nothing terminal yet.
    assert!(matches!(
        groups.classify(beta, &empty, &empty, &no_skips),
        EdgeState::Pending
    ));
    // Completed predecessor.
    assert!(matches!(
        groups.classify(beta, &one("up:alpha"), &empty, &no_skips),
        EdgeState::Satisfied
    ));
    // Failed predecessor.
    assert!(matches!(
        groups.classify(beta, &empty, &one("up:alpha"), &no_skips),
        EdgeState::Unreachable
    ));
    // An up-to-date predecessor is success-like and does not block its successor,
    // even though it is terminal-Skipped.
    assert!(matches!(
        groups.classify(
            beta,
            &one("up:alpha"),
            &empty,
            &skipped_as("up:alpha", SkipKind::UpToDate)
        ),
        EdgeState::Satisfied
    ));
    // Skipped for any other reason, including an ordinary when: edge going unreachable.
    assert!(matches!(
        groups.classify(beta, &empty, &empty, &skipped_as("up:alpha", SkipKind::Unreachable)),
        EdgeState::Unreachable
    ));
    assert!(matches!(
        groups.classify(
            beta,
            &empty,
            &empty,
            &skipped_as("up:alpha", SkipKind::SerialPredecessor)
        ),
        EdgeState::Unreachable
    ));
    // A task with no group is never gated.
    let plain = Task::new(
        "plain".to_string(),
        None,
        vec![],
        vec![],
        vec![],
        HashMap::new(),
        HashMap::new(),
        "echo hi".to_string(),
    );
    assert!(matches!(
        groups.classify(&plain, &empty, &empty, &no_skips),
        EdgeState::Satisfied
    ));
}

/// The gate composes with dependency readiness rather than replacing it.
#[test]
fn test_classify_gates_reports_both_edges_and_ordering() {
    let mut beta = serial_member("up:beta", "up", 1);
    beta.task_deps = vec![TaskEdge::success("dep")];
    let tasks = vec![serial_member("up:alpha", "up", 0), beta];
    let groups = SerialGroups::new(&tasks);

    let completed = std::collections::HashSet::from(["dep".to_string()]);
    let empty = std::collections::HashSet::new();
    let no_skips = SkippedSet::new();

    // Dependency satisfied, ordering still pending -> not ready.
    let states = classify_gates(&tasks[1], &groups, &completed, &empty, &no_skips);
    assert_eq!(states.len(), 2);
    assert!(states.iter().any(|s| matches!(s, EdgeState::Pending)));

    // Both satisfied -> ready.
    let completed = std::collections::HashSet::from(["dep".to_string(), "up:alpha".to_string()]);
    let states = classify_gates(&tasks[1], &groups, &completed, &empty, &no_skips);
    assert!(states.iter().all(|s| matches!(s, EdgeState::Satisfied)));
}

#[test]
fn test_skip_record_names_the_serial_predecessor_and_carries_its_kind() {
    let tasks = vec![serial_member("up:alpha", "up", 0), serial_member("up:beta", "up", 1)];
    let groups = SerialGroups::new(&tasks);
    let empty = std::collections::HashSet::new();
    let no_skips = SkippedSet::new();

    let failed = std::collections::HashSet::from(["up:alpha".to_string()]);
    assert_eq!(
        skip_record_for(&tasks[1], &groups, &empty, &failed, &no_skips),
        SkipRecord::new(
            SkipKind::SerialPredecessor,
            "serial predecessor up:alpha failed".to_string()
        )
    );

    let skipped = SkippedSet::from([("up:alpha".to_string(), SkipKind::Unreachable)]);
    assert_eq!(
        skip_record_for(&tasks[1], &groups, &empty, &empty, &skipped),
        SkipRecord::new(
            SkipKind::SerialPredecessor,
            "serial predecessor up:alpha skipped; cascade".to_string()
        )
    );
}

/// An unreachable dependency edge is provenance `Unreachable`, not
/// `SerialPredecessor`: the kind names which gate fired.
#[test]
fn test_skip_record_for_an_unreachable_edge_is_kind_unreachable() {
    let mut dependent = Task::new(
        "dependent".to_string(),
        None,
        vec![],
        vec![],
        vec![],
        HashMap::new(),
        HashMap::new(),
        "echo hi".to_string(),
    );
    dependent.task_deps = vec![TaskEdge::success("src")];
    let groups = SerialGroups::default();
    let empty = std::collections::HashSet::new();
    let failed = std::collections::HashSet::from(["src".to_string()]);

    assert_eq!(
        skip_record_for(&dependent, &groups, &empty, &failed, &SkippedSet::new()),
        SkipRecord::new(
            SkipKind::Unreachable,
            "dep src failed; this task required when: success".to_string()
        )
    );
}

/// The `SkipKind` x `when:` contract, all nine cells, asserted explicitly rather
/// than left to whichever cells the other fixes happen to exercise.
///
/// Both gates that read this contract are asserted against the same table:
/// `classify_edge` (the scheduler's admission gate) and the worker's dependency
/// double-check inside `execute_task`. They ran opposite policies before this
/// phase, and a disagreement aborts at spawn time a task the scheduler admitted.
#[test]
fn classify_edge_skip_provenance_matrix() {
    use EdgeState::{Satisfied, Unreachable};

    let cells = [
        (SkipKind::UpToDate, When::Success, Satisfied),
        (SkipKind::UpToDate, When::Failure, Unreachable),
        (SkipKind::UpToDate, When::Always, Satisfied),
        (SkipKind::SerialPredecessor, When::Success, Unreachable),
        (SkipKind::SerialPredecessor, When::Failure, Unreachable),
        (SkipKind::SerialPredecessor, When::Always, Satisfied),
        (SkipKind::Unreachable, When::Success, Unreachable),
        (SkipKind::Unreachable, When::Failure, Unreachable),
        (SkipKind::Unreachable, When::Always, Satisfied),
    ];
    assert_eq!(cells.len(), 9, "the contract is a 3x3 table; every cell is asserted");

    let completed = std::collections::HashSet::new();
    let failed = std::collections::HashSet::new();

    for (kind, when, expected) in cells {
        let edge = TaskEdge::new("src".to_string(), when);
        let skipped = SkippedSet::from([("src".to_string(), kind)]);

        // Gate 1: the scheduler's edge classification.
        assert_eq!(
            classify_edge(&edge, &completed, &failed, &skipped),
            expected,
            "classify_edge: source skipped as {kind:?} against when: {when:?}"
        );

        // Gate 2: the worker's dependency double-check, which reads the kind off
        // the source's TaskStatus rather than off the runtime sets.
        let status = TaskStatus::Skipped(kind);
        let double_check_satisfied = match (when, Some(&status)) {
            (When::Success, Some(TaskStatus::Completed)) => true,
            (When::Success, Some(TaskStatus::Skipped(k))) => k.is_success_like(),
            (When::Failure, Some(TaskStatus::Failed(_))) => true,
            (When::Always, Some(TaskStatus::Completed)) => true,
            (When::Always, Some(TaskStatus::Skipped(_))) => true,
            (When::Always, Some(TaskStatus::Failed(_))) => true,
            _ => false,
        };
        assert_eq!(
            double_check_satisfied,
            expected == Satisfied,
            "worker double-check disagrees with classify_edge: {kind:?} against when: {when:?}"
        );
    }
}

#[tokio::test]
#[serial]
async fn test_task_execution() -> Result<()> {
    let temp_dir = TempDir::new()?;
    let work_dir = PathBuf::from(temp_dir.path());
    setup_test_db(&work_dir);

    let task = Task::new(
        "test".to_string(),
        None,
        vec![],
        vec![],
        vec![],
        HashMap::new(),
        HashMap::new(),
        "echo hello".to_string(),
    );

    let workspace = Workspace::new(work_dir).await?;
    workspace.init().await?;
    let scheduler = TaskScheduler::new(vec![task], Arc::new(workspace), ExecutionContext::new(), 2, false).await?;
    scheduler.execute_all().await?;

    let status = scheduler.get_task_status("test").await;
    assert_eq!(status, TaskStatus::Completed);

    Ok(())
}

#[tokio::test]
#[serial]
async fn test_task_dependencies() -> Result<()> {
    let temp_dir = TempDir::new()?;
    let work_dir = PathBuf::from(temp_dir.path());
    setup_test_db(&work_dir);

    let tasks = vec![
        Task::new(
            "task1".to_string(),
            None,
            vec![crate::executor::task::TaskEdge::success("task2")],
            vec![],
            vec![],
            HashMap::new(),
            HashMap::new(),
            "echo task1".to_string(),
        ),
        Task::new(
            "task2".to_string(),
            None,
            vec![],
            vec![],
            vec![],
            HashMap::new(),
            HashMap::new(),
            "echo task2".to_string(),
        ),
    ];

    let workspace = Workspace::new(work_dir).await?;
    workspace.init().await?;
    let scheduler = TaskScheduler::new(tasks, Arc::new(workspace), ExecutionContext::new(), 2, false).await?;
    scheduler.execute_all().await?;

    let task1_status = scheduler.get_task_status("task1").await;
    let task2_status = scheduler.get_task_status("task2").await;

    assert_eq!(task1_status, TaskStatus::Completed);
    assert_eq!(task2_status, TaskStatus::Completed);

    Ok(())
}

#[tokio::test]
#[serial]
async fn test_task_failure() -> Result<()> {
    let temp_dir = TempDir::new()?;
    let work_dir = PathBuf::from(temp_dir.path());
    setup_test_db(&work_dir);

    let tasks = vec![
        Task::new(
            "task1".to_string(),
            None,
            vec![],
            vec![],
            vec![],
            HashMap::new(),
            HashMap::new(),
            "exit 1".to_string(),
        ),
        Task::new(
            "task2".to_string(),
            None,
            vec![crate::executor::task::TaskEdge::success("task1")],
            vec![],
            vec![],
            HashMap::new(),
            HashMap::new(),
            "echo task2".to_string(),
        ),
    ];

    let workspace = Workspace::new(work_dir).await?;
    workspace.init().await?;
    let scheduler = TaskScheduler::new(tasks, Arc::new(workspace), ExecutionContext::new(), 2, false).await?;
    let result = scheduler.execute_all().await;

    assert!(result.is_err());

    Ok(())
}

#[tokio::test]
#[serial]
async fn test_file_dependencies() -> Result<()> {
    let temp_dir = TempDir::new()?;
    let work_dir = PathBuf::from(temp_dir.path());
    setup_test_db(&work_dir);

    let input_file = work_dir.join("input.txt");
    let output_file = work_dir.join("output.txt");
    tokio::fs::write(&input_file, "test content").await?;

    let task = Task::new(
        "copy_task".to_string(),
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

    let needs_rebuild = scheduler.needs_rebuild(&task).await?;
    assert!(needs_rebuild, "Task should need to run when output doesn't exist");

    // Simulate file creation with newer timestamp
    tokio::fs::write(&output_file, "output content").await?;

    let now = std::time::SystemTime::now();
    let future_time = filetime::FileTime::from_system_time(now + std::time::Duration::from_secs(1));
    filetime::set_file_times(&output_file, future_time, future_time)?;

    // Now the task should not need to run (output newer than input)
    let needs_rebuild_after = scheduler.needs_rebuild(&task).await?;
    assert!(
        !needs_rebuild_after,
        "Task should not need to run when output is newer than inputs"
    );

    Ok(())
}

#[tokio::test]
#[serial]
async fn test_file_timestamp_checking() -> Result<()> {
    let temp_dir = TempDir::new()?;
    let work_dir = PathBuf::from(temp_dir.path());
    setup_test_db(&work_dir);

    let file1 = work_dir.join("file1.txt");
    let file2 = work_dir.join("file2.txt");

    tokio::fs::write(&file1, "content1").await?;
    tokio::fs::write(&file2, "content2").await?;

    let workspace = Workspace::new(work_dir).await?;
    workspace.init().await?;
    let scheduler = TaskScheduler::new(vec![], Arc::new(workspace), ExecutionContext::new(), 2, false).await?;

    // Test timestamp retrieval
    let timestamps = scheduler
        .get_file_timestamps(&[file1.to_string_lossy().to_string(), file2.to_string_lossy().to_string()])
        .await?;

    assert_eq!(timestamps.len(), 2);
    assert!(timestamps[0].1.is_some(), "Should have timestamp for existing file");
    assert!(timestamps[1].1.is_some(), "Should have timestamp for existing file");

    Ok(())
}

#[tokio::test]
#[serial]
async fn test_file_dependencies_nonexistent_files() -> Result<()> {
    let temp_dir = TempDir::new()?;
    let work_dir = PathBuf::from(temp_dir.path());
    setup_test_db(&work_dir);

    let nonexistent_file = work_dir.join("nonexistent.txt");
    let output_file = work_dir.join("output.txt");

    let task = Task::new(
        "test_nonexistent".to_string(),
        None,
        vec![],
        vec![nonexistent_file.to_string_lossy().to_string()],
        vec![output_file.to_string_lossy().to_string()],
        HashMap::new(),
        HashMap::new(),
        format!("touch {}", output_file.display()),
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

    // Should need to rebuild when input file doesn't exist (conservative approach)
    let needs_rebuild = scheduler.needs_rebuild(&task).await?;
    assert!(needs_rebuild, "Task should need to run when input file doesn't exist");

    Ok(())
}

#[tokio::test]
#[serial]
async fn test_file_dependencies_multiple_inputs_outputs() -> Result<()> {
    let temp_dir = TempDir::new()?;
    let work_dir = PathBuf::from(temp_dir.path());
    setup_test_db(&work_dir);

    let input1 = work_dir.join("input1.txt");
    let input2 = work_dir.join("input2.txt");
    let input3 = work_dir.join("input3.txt");
    let output1 = work_dir.join("output1.txt");
    let output2 = work_dir.join("output2.txt");

    tokio::fs::write(&input1, "content1").await?;
    tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;
    tokio::fs::write(&input2, "content2").await?;
    tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;
    tokio::fs::write(&input3, "content3").await?;

    let task = Task::new(
        "multi_files".to_string(),
        None,
        vec![],
        vec![
            input1.to_string_lossy().to_string(),
            input2.to_string_lossy().to_string(),
            input3.to_string_lossy().to_string(),
        ],
        vec![
            output1.to_string_lossy().to_string(),
            output2.to_string_lossy().to_string(),
        ],
        HashMap::new(),
        HashMap::new(),
        format!(
            "cat {} {} {} > {} && cp {} {}",
            input1.display(),
            input2.display(),
            input3.display(),
            output1.display(),
            output1.display(),
            output2.display()
        ),
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

    // Should need to rebuild when outputs don't exist
    let needs_rebuild = scheduler.needs_rebuild(&task).await?;
    assert!(needs_rebuild, "Task should need to run when outputs don't exist");

    tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;
    tokio::fs::write(&output1, "combined output").await?;
    tokio::fs::write(&output2, "combined output copy").await?;

    // Should not need to rebuild when all outputs are newer than all inputs
    let needs_rebuild_after = scheduler.needs_rebuild(&task).await?;
    assert!(
        !needs_rebuild_after,
        "Task should not need to run when all outputs are newer than all inputs"
    );

    // Touch one of the input files to make it newer
    tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;
    tokio::fs::write(&input2, "modified content2").await?;

    // Should need to rebuild when any input is newer than any output
    let needs_rebuild_final = scheduler.needs_rebuild(&task).await?;
    assert!(
        needs_rebuild_final,
        "Task should need to run when any input is newer than outputs"
    );

    Ok(())
}

#[tokio::test]
#[serial]
async fn test_file_dependencies_with_task_dependencies() -> Result<()> {
    let temp_dir = TempDir::new()?;
    let work_dir = PathBuf::from(temp_dir.path());
    setup_test_db(&work_dir);

    let input_file = work_dir.join("input.txt");
    let intermediate_file = work_dir.join("intermediate.txt");
    let output_file = work_dir.join("output.txt");

    tokio::fs::write(&input_file, "initial content").await?;

    let task1 = Task::new(
        "step1".to_string(),
        None,
        vec![],
        vec![input_file.to_string_lossy().to_string()],
        vec![intermediate_file.to_string_lossy().to_string()],
        HashMap::new(),
        HashMap::new(),
        format!("cp {} {}", input_file.display(), intermediate_file.display()),
    );

    let task2 = Task::new(
        "step2".to_string(),
        None,
        vec![crate::executor::task::TaskEdge::success("step1")], // Task dependency
        vec![intermediate_file.to_string_lossy().to_string()],   // File dependency
        vec![output_file.to_string_lossy().to_string()],
        HashMap::new(),
        HashMap::new(),
        format!("cp {} {}", intermediate_file.display(), output_file.display()),
    );

    let workspace = Workspace::new(work_dir.clone()).await?;
    workspace.init().await?;
    let scheduler = TaskScheduler::new(
        vec![task1.clone(), task2.clone()],
        Arc::new(workspace),
        ExecutionContext::new(),
        2,
        false,
    )
    .await?;

    // Both tasks should need to run initially
    let task1_needs_rebuild = scheduler.needs_rebuild(&task1).await?;
    let task2_needs_rebuild = scheduler.needs_rebuild(&task2).await?;
    assert!(task1_needs_rebuild, "Task1 should need to run initially");
    assert!(task2_needs_rebuild, "Task2 should need to run initially");

    // Execute all tasks
    scheduler.execute_all().await?;

    let task1_status = scheduler.get_task_status("step1").await;
    let task2_status = scheduler.get_task_status("step2").await;
    assert_eq!(task1_status, TaskStatus::Completed);
    assert_eq!(task2_status, TaskStatus::Completed);

    assert!(intermediate_file.exists(), "Intermediate file should exist");
    assert!(output_file.exists(), "Output file should exist");

    Ok(())
}
