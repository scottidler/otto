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

/// Poll until the task body writes `marker`, so a step is gated on the run's own
/// progress instead of on a duration. A fixed sleep here passes even when the
/// child never spawned at all, which makes it a test of the timer.
async fn wait_for_marker(marker: &std::path::Path) {
    const POLL: std::time::Duration = std::time::Duration::from_millis(25);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    while std::time::Instant::now() < deadline {
        if marker.exists() {
            return;
        }
        tokio::time::sleep(POLL).await;
    }
    panic!("timed out waiting for {}", marker.display());
}

/// Cancelling stops the run instead of waiting for the children.
#[tokio::test]
#[serial]
async fn test_cancel_signal_stops_the_run() -> Result<()> {
    let temp_dir = TempDir::new()?;
    let work_dir = PathBuf::from(temp_dir.path());
    setup_test_db(&work_dir);

    // The child announces itself before it sleeps, and the cancel waits for
    // that announcement: what is being tested is that a RUNNING child is
    // stopped, so the child has to be running when the signal arrives.
    let marker = work_dir.join("sleeper.started");
    let tasks = vec![Task::new(
        "sleeper".to_string(),
        None,
        vec![],
        vec![],
        vec![],
        HashMap::new(),
        HashMap::new(),
        format!("touch {}\nsleep 60", marker.display()),
    )];

    let workspace = Workspace::new(work_dir).await?;
    workspace.init().await?;
    let scheduler = TaskScheduler::new(tasks, Arc::new(workspace), ExecutionContext::new(), 2, false).await?;
    let cancel = scheduler.cancel_signal();

    tokio::spawn(async move {
        wait_for_marker(&marker).await;
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

/// A task body that panics before sending its report must still be observed,
/// not leave the scheduler waiting on a message that never arrives. This is
/// the exact mechanism `reap_unreported`'s doc comment describes: a spawned
/// body reaped via `JoinSet` rather than dropped on the floor.
#[tokio::test]
async fn reap_unreported_observes_a_panicking_task_body() {
    let mut active = ActiveTasks::default();
    active.spawn("boom".to_string(), Admission::Capped, async {
        panic!("deliberate panic for reap_unreported's own test");
    });
    assert_eq!(active.in_flight_len(), 1);

    let name = active.reap_unreported().await;

    assert_eq!(
        name, "boom",
        "the panicking task's name must be reported, not silently dropped"
    );
    assert!(
        active.is_empty(),
        "a reaped panic must no longer count as an in-flight task"
    );
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

/// The name is a field, so it never has to be recovered from the error text.
/// The exit code is not a field: nothing downstream read it, and the body that
/// knows the code writes it to the database itself.
#[test]
fn test_task_report_carries_the_name_structurally() {
    let ok = TaskReport::success("build".to_string());
    assert_eq!(ok.name, "build");
    assert!(ok.error.is_none());

    let failed = TaskReport::failure("build".to_string(), eyre!("No such file or directory"));
    assert_eq!(failed.name, "build", "the name must not come from the error text");
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

/// The blocked sweep reads every gate, including each edge's `when:` condition.
///
/// A dependent gated on `when: failure` whose dep SUCCEEDED can never run, and
/// one gated on `when: success` is ready the moment the dep completes. The
/// fourth copy of this walk (in `try_start_ready_task`, deleted when the four
/// were folded into one) tested only `completed_set.contains`, so it read the
/// dep's membership and never its condition: this task went onto the ready
/// queue and the answer was left to the dispatch gate to reach again.
#[test]
fn test_the_blocked_sweep_reads_the_edge_condition_not_bare_completion() {
    let groups = SerialGroups::new(&[]);
    let completed = std::collections::HashSet::from(["dep".to_string()]);
    let empty = std::collections::HashSet::new();
    let no_skips = SkippedSet::new();

    let mut blocked = vec![
        plain_task("on_failure", vec![TaskEdge::new("dep", When::Failure)]),
        plain_task("on_success", vec![TaskEdge::success("dep")]),
        plain_task("waiting", vec![TaskEdge::success("not_yet")]),
    ];
    let swept = partition_blocked(&mut blocked, &groups, &completed, &empty, &no_skips);

    let names = |tasks: &[Task]| tasks.iter().map(|t| t.name.clone()).collect::<Vec<_>>();
    assert_eq!(names(&swept.unreachable), ["on_failure"]);
    assert_eq!(names(&swept.ready), ["on_success"]);
    assert_eq!(
        names(&blocked),
        ["waiting"],
        "a gate that is merely Pending stays blocked"
    );
}

/// An up-to-date skip resolves its dependents through that same sweep.
///
/// The up-to-date path is a terminal transition like any other, so a dependent
/// it makes unreachable is skipped with its provenance recorded rather than
/// left for a later pass to notice.
#[tokio::test]
#[serial]
async fn test_an_up_to_date_skip_marks_its_unreachable_dependents() -> Result<()> {
    let temp_dir = TempDir::new()?;
    let work_dir = PathBuf::from(temp_dir.path());
    setup_test_db(&work_dir);

    let input = work_dir.join("input.txt");
    let output = work_dir.join("output.txt");
    tokio::fs::write(&input, "in").await?;
    tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    tokio::fs::write(&output, "out").await?;

    let mut cached = plain_task("cached", vec![]);
    cached.file_deps = vec![input.to_string_lossy().to_string()];
    cached.output_deps = vec![output.to_string_lossy().to_string()];
    let dependent = plain_task("on_failure", vec![TaskEdge::new("cached", When::Failure)]);

    let workspace = Workspace::new(work_dir).await?;
    workspace.init().await?;
    let scheduler = TaskScheduler::new(
        vec![cached.clone(), dependent],
        Arc::new(workspace),
        ExecutionContext::new(),
        2,
        false,
    )
    .await?;

    assert!(
        !scheduler.needs_rebuild(&cached).await,
        "the fixture must be up to date, or nothing exercises the skip path"
    );
    scheduler.execute_all().await?;

    assert_eq!(
        scheduler.get_task_status("cached").await,
        TaskStatus::Skipped(SkipKind::UpToDate)
    );
    assert_eq!(
        scheduler.get_task_status("on_failure").await,
        TaskStatus::Skipped(SkipKind::Unreachable),
        "a when: failure dependent of a warm-cache success can never run"
    );
    let records = scheduler.get_skip_records().await;
    let record = records.get("on_failure").expect("the skip must be recorded");
    assert!(
        record.detail.contains("cached"),
        "the reason must name the dep that made it unreachable; got {}",
        record.detail
    );
    Ok(())
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
        // the source's TaskStatus rather than off the runtime sets. This calls
        // the function the worker itself calls; transcribing its arms into the
        // test instead would assert the copy and stay green while the real gate
        // drifted.
        let status = TaskStatus::Skipped(kind);
        assert_eq!(
            edge_satisfied_by_status(when, Some(&status)),
            expected == Satisfied,
            "worker double-check disagrees with classify_edge: {kind:?} against when: {when:?}"
        );
    }
}

/// Every gate that decides "may this task start, given the state of its source"
/// must reach the same answer. There are three: the scheduler's edge admission
/// (`classify_edge`), the worker's terminal-status double-check
/// (`edge_satisfied_by_status`), and the serial chain's ordering gate
/// (`SerialGroups::classify`).
///
/// Two of them *run* the shared ladder: `classify_edge` and `SerialGroups::classify`
/// both call `classify_source`. The worker's is a separate `match` on `TaskStatus`
/// and cannot call it without an adapter, because it reads a terminal status
/// rather than the runtime sets. So for that one this test asserts **agreement,
/// not shared execution** - it is the thing standing between the two spellings.
/// Stated exactly, because the commit that introduced this test (`21e0fb6`) said
/// "all three gates run it", which a review panel correctly called overstated.
///
/// The nine-cell matrix test above covers the skipped rows. This covers the
/// whole lattice (skipped for each kind, completed, failed, and
/// not-yet-finished) against every `when:`, and asserts the serial gate is
/// exactly the `When::Success` column rather than a second opinion about it.
/// The serial gate open-coded that column until this test existed; nothing had
/// ever compared them.
#[test]
fn every_gate_runs_the_same_ladder_over_the_whole_source_lattice() {
    use EdgeState::{Pending, Satisfied, Unreachable};

    let empty = std::collections::HashSet::new();
    let src_completed = std::collections::HashSet::from(["src".to_string()]);
    let src_failed = std::collections::HashSet::from(["src".to_string()]);

    /// One row of the source lattice: how the runtime sets and the terminal
    /// status both describe the same source task.
    struct SourceState<'a> {
        label: &'a str,
        completed: &'a std::collections::HashSet<String>,
        failed: &'a std::collections::HashSet<String>,
        skipped: SkippedSet,
        /// `None` when the source has not finished, which is the one row with no
        /// terminal status for the worker's double-check to read.
        status: Option<TaskStatus>,
    }

    let row = |label, completed, failed, skipped, status| SourceState {
        label,
        completed,
        failed,
        skipped,
        status,
    };

    let states: Vec<SourceState<'_>> = vec![
        row(
            "skipped up-to-date",
            &empty,
            &empty,
            SkippedSet::from([("src".to_string(), SkipKind::UpToDate)]),
            Some(TaskStatus::Skipped(SkipKind::UpToDate)),
        ),
        row(
            "skipped serial-predecessor",
            &empty,
            &empty,
            SkippedSet::from([("src".to_string(), SkipKind::SerialPredecessor)]),
            Some(TaskStatus::Skipped(SkipKind::SerialPredecessor)),
        ),
        row(
            "skipped unreachable",
            &empty,
            &empty,
            SkippedSet::from([("src".to_string(), SkipKind::Unreachable)]),
            Some(TaskStatus::Skipped(SkipKind::Unreachable)),
        ),
        row(
            "completed",
            &src_completed,
            &empty,
            SkippedSet::new(),
            Some(TaskStatus::Completed),
        ),
        row(
            "failed",
            &empty,
            &src_failed,
            SkippedSet::new(),
            Some(TaskStatus::Failed("boom".to_string())),
        ),
        row("not finished", &empty, &empty, SkippedSet::new(), None),
    ];

    let mut serial_column_checked = 0;

    for SourceState {
        label,
        completed,
        failed,
        skipped,
        status,
    } in &states
    {
        for when in [When::Success, When::Failure, When::Always] {
            let edge = TaskEdge::new("src".to_string(), when);
            let via_edge = classify_edge(&edge, completed, failed, skipped);
            let via_source = classify_source("src", when, completed, failed, skipped);
            assert_eq!(
                via_edge, via_source,
                "{label} / when: {when:?}: classify_edge and classify_source disagree"
            );

            // The worker's double-check is defined on terminal states only; a
            // source that has not finished has no status to read.
            if let Some(status) = status {
                assert_eq!(
                    edge_satisfied_by_status(when, Some(status)),
                    via_edge == Satisfied,
                    "{label} / when: {when:?}: worker double-check disagrees with the admission gate"
                );
            } else {
                assert_eq!(via_edge, Pending, "{label} / when: {when:?} must be Pending");
                assert!(
                    !edge_satisfied_by_status(when, None),
                    "{label}: an unfinished source satisfies nothing"
                );
            }

            // The serial ordering gate is the `when: success` column, and only
            // that column: a serial predecessor is a success-gated source.
            if when == When::Success {
                let mut member = Task::new(
                    "member".to_string(),
                    None,
                    vec![],
                    vec![],
                    vec![],
                    HashMap::new(),
                    HashMap::new(),
                    String::new(),
                );
                member.serial_group = Some("g".to_string());
                member.serial_index = 1;
                let mut predecessor = Task::new(
                    "src".to_string(),
                    None,
                    vec![],
                    vec![],
                    vec![],
                    HashMap::new(),
                    HashMap::new(),
                    String::new(),
                );
                predecessor.serial_group = Some("g".to_string());
                predecessor.serial_index = 0;

                let groups = SerialGroups::new(&[predecessor, member.clone()]);
                assert_eq!(
                    groups.classify(&member, completed, failed, skipped),
                    via_edge,
                    "{label}: the serial ordering gate disagrees with the when: success column"
                );
                serial_column_checked += 1;
            }
        }
    }

    assert_eq!(
        serial_column_checked,
        states.len(),
        "the serial gate must be checked against every source state"
    );
    // Sanity: the lattice actually produces all three outcomes, so an
    // always-Satisfied ladder could not pass this test.
    let outcomes: Vec<EdgeState> = states
        .iter()
        .flat_map(|s| {
            let (c, f, sk) = (s.completed, s.failed, &s.skipped);
            [When::Success, When::Failure, When::Always].map(move |w| classify_source("src", w, c, f, sk))
        })
        .collect();
    for required in [Satisfied, Unreachable, Pending] {
        assert!(
            outcomes.contains(&required),
            "the lattice must exercise {required:?}; an always-{required:?} ladder would pass otherwise. got {outcomes:?}"
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

/// The completion channel's buffer (`COMPLETION_CHANNEL_CAPACITY`, 32) is
/// smaller than this run's task count. This does not reliably force the
/// channel past capacity by itself (these tasks finish near-instantly and
/// the scheduler's own drain loop keeps up in practice, spot-checked by
/// temporarily changing the real `tx.send(...).await` to `tx.try_send(...)`
/// - a genuine drop-on-full regression - and finding this test still green);
/// what it does prove is that a run with more tasks than the channel's
/// buffer size still completes every one of them correctly. The channel's
/// actual backpressure-not-drop guarantee is pinned deterministically, at
/// the channel level, by `a_slow_receiver_does_not_lose_sends_past_the_channel_capacity`
/// below.
#[tokio::test]
#[serial]
async fn more_than_the_channel_capacity_completing_at_once_all_get_reported() -> Result<()> {
    let temp_dir = TempDir::new()?;
    let work_dir = PathBuf::from(temp_dir.path());
    setup_test_db(&work_dir);

    let task_count = COMPLETION_CHANNEL_CAPACITY + 8;
    let tasks: Vec<Task> = (0..task_count)
        .map(|i| {
            Task::new(
                format!("t{i}"),
                None,
                vec![],
                vec![],
                vec![],
                HashMap::new(),
                HashMap::new(),
                "true".to_string(),
            )
        })
        .collect();

    let workspace = Workspace::new(work_dir).await?;
    workspace.init().await?;
    // max_parallel >= task_count: every task is ready and spawned at once,
    // so completions can genuinely race past the channel's capacity rather
    // than trickling in one at a time.
    let scheduler = TaskScheduler::new(tasks, Arc::new(workspace), ExecutionContext::new(), task_count, false).await?;
    tokio::time::timeout(std::time::Duration::from_secs(10), scheduler.execute_all()).await??;

    for i in 0..task_count {
        let status = scheduler.get_task_status(&format!("t{i}")).await;
        assert_eq!(
            status,
            TaskStatus::Completed,
            "t{i} must be reported completed, not lost"
        );
    }

    Ok(())
}

/// The deterministic pin for the channel *property* the test above cannot
/// reliably force: with more in-flight sends than the channel's capacity and
/// a receiver too slow to keep up, a bounded `mpsc::Sender::send(...).await`
/// backpressures (the sender waits) rather than drops. Uses the exact same
/// constant production code sizes its channel with.
///
/// What this does *not* catch: a future edit to `scheduler/task_execution.rs`
/// that swaps its real `tx.send(report).await` for `tx.try_send(report)`
/// (drop-on-full - exactly the regression this bullet is about). Spot-checked
/// live by making that exact edit: this test is unaffected (it owns its own
/// channel and never touches the scheduler's), and the end-to-end test above
/// stayed green too, since these tasks finish fast enough that the drop
/// never actually triggered in that run. Neither test mechanically guards
/// against that specific edit; the send call site's own doc comment is what
/// does today.
#[tokio::test]
async fn a_slow_receiver_does_not_lose_sends_past_the_channel_capacity() {
    let (tx, mut rx) = tokio::sync::mpsc::channel::<usize>(COMPLETION_CHANNEL_CAPACITY);
    let send_count = COMPLETION_CHANNEL_CAPACITY + 8;

    let sender = tokio::spawn(async move {
        for i in 0..send_count {
            // `.send(...).await` must block once the buffer is full rather
            // than losing `i`; `try_send` would return `Err(Full(..))` here
            // instead of waiting, and this is exactly what would drop a
            // completion report in production.
            tx.send(i).await.expect("receiver must still be alive");
        }
    });

    // Give every sender a chance to queue up before draining anything, so
    // the channel is genuinely at (and, since send_count > capacity, past)
    // its buffer size before the receiver starts taking messages out.
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    let mut received = Vec::with_capacity(send_count);
    while received.len() < send_count {
        received.push(rx.recv().await.expect("sender must not have dropped early"));
    }
    sender.await.expect("sender task must not panic");

    received.sort_unstable();
    assert_eq!(
        received,
        (0..send_count).collect::<Vec<_>>(),
        "every send past the channel's capacity must still arrive, none dropped"
    );
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

    let needs_rebuild = scheduler.needs_rebuild(&task).await;
    assert!(needs_rebuild, "Task should need to run when output doesn't exist");

    // Simulate file creation with newer timestamp
    tokio::fs::write(&output_file, "output content").await?;

    let now = std::time::SystemTime::now();
    let future_time = filetime::FileTime::from_system_time(now + std::time::Duration::from_secs(1));
    filetime::set_file_times(&output_file, future_time, future_time)?;

    // Now the task should not need to run (output newer than input)
    let needs_rebuild_after = scheduler.needs_rebuild(&task).await;
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
        .await;

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
    let needs_rebuild = scheduler.needs_rebuild(&task).await;
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
    let needs_rebuild = scheduler.needs_rebuild(&task).await;
    assert!(needs_rebuild, "Task should need to run when outputs don't exist");

    tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;
    tokio::fs::write(&output1, "combined output").await?;
    tokio::fs::write(&output2, "combined output copy").await?;

    // Should not need to rebuild when all outputs are newer than all inputs
    let needs_rebuild_after = scheduler.needs_rebuild(&task).await;
    assert!(
        !needs_rebuild_after,
        "Task should not need to run when all outputs are newer than all inputs"
    );

    // Touch one of the input files to make it newer
    tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;
    tokio::fs::write(&input2, "modified content2").await?;

    // Should need to rebuild when any input is newer than any output
    let needs_rebuild_final = scheduler.needs_rebuild(&task).await;
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
    let task1_needs_rebuild = scheduler.needs_rebuild(&task1).await;
    let task2_needs_rebuild = scheduler.needs_rebuild(&task2).await;
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

/// The mechanical guard the two tests above cannot be: a completion report
/// must go through a blocking, backpressuring send, never a `try_send` that
/// would drop it under load. Neither a scheduler-level test (these tasks
/// finish too fast to force real congestion) nor a bare-channel test (it
/// cannot see the scheduler's own call site) catches a future edit that
/// swaps one for the other; a direct source check does.
#[test]
fn task_execution_never_uses_try_send_for_completion_reports() {
    let source = include_str!("scheduler/task_execution.rs");
    assert!(
        !source.contains("try_send"),
        "the completion report must use a blocking `send(...).await`, not `try_send`, \
         or completions can be silently dropped under load"
    );
}
