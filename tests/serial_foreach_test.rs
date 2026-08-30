//! Serial `foreach` is a scheduler property, not a dependency edge.
//!
//! Phase 4 of docs/design/2026-08-28-boundary-fixes-and-dynamic-foreach.md. Ordering
//! constrains the order in which group members may start; it never expands the run set,
//! and a predecessor that reaches any non-success terminal state skips the rest of the
//! group with a visible reason instead of leaving them waiting.

use eyre::Result;
use serial_test::serial;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;
use tempfile::TempDir;
use tokio::time::timeout;

use otto::Parser;
use otto::executor::state::SkipKind;
use otto::executor::{Task, TaskScheduler, TaskStatus, Workspace, workspace::ExecutionContext};

fn setup_test_env(temp_dir: &Path) {
    unsafe {
        std::env::set_var("OTTO_DB_PATH", temp_dir.join("test_otto.db"));
        std::env::set_var("OTTO_HOME", temp_dir.join(".otto"));
    }
}

/// Parse an ottofile and hand back executor tasks for the requested targets.
fn plan(ottofile: &Path, targets: &[&str]) -> Result<Vec<Task>> {
    let mut args = vec![
        "otto".to_string(),
        "--ottofile".to_string(),
        ottofile.to_string_lossy().to_string(),
    ];
    args.extend(targets.iter().map(|t| (*t).to_string()));
    let mut parser = Parser::new(args)?;
    let (parser_tasks, _, _, _, _, _) = parser.parse()?.into_run()?.into_parts();
    Ok(parser_tasks.into_iter().map(Task::from).collect())
}

async fn run(work_dir: PathBuf, tasks: Vec<Task>, jobs: usize) -> Result<(TaskScheduler, bool)> {
    let workspace = Workspace::new(work_dir).await?;
    workspace.init().await?;
    let scheduler = TaskScheduler::new(tasks, Arc::new(workspace), ExecutionContext::new(), jobs, false).await?;
    let outcome = timeout(Duration::from_secs(30), scheduler.execute_all()).await?;
    let ok = outcome.is_ok();
    Ok((scheduler, ok))
}

const SERIAL_FIXTURE: &str = r#"
tasks:
  up:
    foreach:
      items: [alpha, beta, gamma]
      as: svc
      parallel: false
    bash: echo "running ${svc}"
"#;

fn write_fixture(dir: &Path, body: &str) -> Result<PathBuf> {
    let path = dir.join("otto.yml");
    std::fs::write(&path, body)?;
    Ok(path)
}

/// Criterion (a): targeting a serial subtask schedules exactly that subtask.
/// On main this run pulled in up:alpha and up:beta as well, because serial ordering
/// was implemented with real `before:` edges that `collect_transitive_deps` walked.
#[test]
#[serial]
fn test_targeting_serial_subtask_schedules_only_that_subtask() -> Result<()> {
    let temp_dir = TempDir::new()?;
    let ottofile = write_fixture(temp_dir.path(), SERIAL_FIXTURE)?;

    let tasks = plan(&ottofile, &["up:gamma"])?;
    let names: Vec<&str> = tasks.iter().map(|t| t.name.as_str()).collect();

    assert_eq!(names, vec!["up:gamma"], "serial targeting must not expand the run set");
    Ok(())
}

/// The control from the design doc: under `parallel: true`, targeting was already exact.
#[test]
#[serial]
fn test_targeting_parallel_subtask_schedules_only_that_subtask() -> Result<()> {
    let temp_dir = TempDir::new()?;
    let ottofile = write_fixture(
        temp_dir.path(),
        r#"
tasks:
  up:
    foreach:
      items: [alpha, beta, gamma]
      as: svc
      parallel: true
    bash: echo "running ${svc}"
"#,
    )?;

    let tasks = plan(&ottofile, &["up:gamma"])?;
    let names: Vec<&str> = tasks.iter().map(|t| t.name.as_str()).collect();

    assert_eq!(names, vec!["up:gamma"]);
    Ok(())
}

/// Serial members carry group membership instead of sibling `before:` edges.
#[test]
#[serial]
fn test_serial_members_carry_group_not_edges() -> Result<()> {
    let temp_dir = TempDir::new()?;
    let ottofile = write_fixture(temp_dir.path(), SERIAL_FIXTURE)?;

    let tasks = plan(&ottofile, &["up"])?;

    for name in ["up:alpha", "up:beta", "up:gamma"] {
        let task = tasks.iter().find(|t| t.name == name).expect("subtask scheduled");
        assert!(
            !task.task_deps.iter().any(|d| d.task.starts_with("up:")),
            "{name} must not carry a sibling edge, got {:?}",
            task.task_deps
        );
        assert_eq!(task.serial_group.as_deref(), Some("up"), "{name} must be in group up");
    }

    let index_of = |name: &str| {
        tasks
            .iter()
            .find(|t| t.name == name)
            .map(|t| t.serial_index)
            .expect("subtask scheduled")
    };
    assert_eq!(index_of("up:alpha"), 0);
    assert_eq!(index_of("up:beta"), 1);
    assert_eq!(index_of("up:gamma"), 2);
    Ok(())
}

/// The `--Serial` CLI flag and `parallel: false` in config flow through the same
/// `run_serial` decision, so one implementation covers both.
#[test]
#[serial]
fn test_serial_cli_flag_produces_the_same_group() -> Result<()> {
    let temp_dir = TempDir::new()?;
    let ottofile = write_fixture(
        temp_dir.path(),
        r#"
tasks:
  up:
    foreach:
      items: [alpha, beta, gamma]
      as: svc
    bash: echo "running ${svc}"
"#,
    )?;

    let tasks = plan(&ottofile, &["up", "--Serial"])?;
    for (name, index) in [("up:alpha", 0), ("up:beta", 1), ("up:gamma", 2)] {
        let task = tasks.iter().find(|t| t.name == name).expect("subtask scheduled");
        assert_eq!(task.serial_group.as_deref(), Some("up"));
        assert_eq!(task.serial_index, index);
    }
    Ok(())
}

/// Criterion (b): a full run executes the group in declared order and never overlaps
/// two members, even with a job budget wide enough to run all three at once.
#[tokio::test]
#[serial]
async fn test_serial_group_never_interleaves() -> Result<()> {
    let temp_dir = TempDir::new()?;
    let work_dir = PathBuf::from(temp_dir.path());
    setup_test_env(&work_dir);

    let trace = work_dir.join("trace.txt");
    let ottofile = write_fixture(
        temp_dir.path(),
        &format!(
            r#"
tasks:
  up:
    foreach:
      items: [alpha, beta, gamma]
      as: svc
      parallel: false
    bash: |
      echo "start ${{svc}}" >> {trace}
      sleep 0.2
      echo "end ${{svc}}" >> {trace}
"#,
            trace = trace.display()
        ),
    )?;

    let tasks = plan(&ottofile, &["up"])?;
    let (scheduler, ok) = run(work_dir, tasks, 8).await?;
    assert!(ok, "serial run should succeed");

    let recorded = std::fs::read_to_string(&trace)?;
    let lines: Vec<&str> = recorded.lines().collect();
    assert_eq!(
        lines,
        vec![
            "start alpha",
            "end alpha",
            "start beta",
            "end beta",
            "start gamma",
            "end gamma",
        ],
        "serial members must not interleave; got {recorded}"
    );

    let statuses = scheduler.get_task_statuses().await;
    assert_eq!(statuses["up"], TaskStatus::Completed);
    Ok(())
}

/// Terminal-state matrix, failure arm: a failed predecessor skips the rest of the group
/// with a visible reason, and the run still exits non-zero because of the failure.
///
/// INVERTED, deliberately, by Phase 2 of docs/design/2026-06-10-code-review-remediation.md.
/// This test previously asserted `statuses["up"] == Skipped` and documented that
/// aggregation "never gets a chance to fire": the parent's `when: always` edges to its
/// Skipped subtasks short-circuited to Unreachable, so the parent was skipped before the
/// aggregation override could run, and an `on-failure:` fixer attached to a serial group
/// never fired. `when: always` is now satisfied by every terminal state including a skip,
/// so the parent reaches aggregation and aggregates to Failed because a member failed.
/// The same ottofile with `parallel: true` already behaved this way; serial and parallel
/// foreach now agree.
#[tokio::test]
#[serial]
async fn test_failed_predecessor_skips_remaining_group_members() -> Result<()> {
    let temp_dir = TempDir::new()?;
    let work_dir = PathBuf::from(temp_dir.path());
    setup_test_env(&work_dir);

    let ottofile = write_fixture(
        temp_dir.path(),
        r#"
tasks:
  up:
    foreach:
      items: [alpha, beta, gamma]
      as: svc
      parallel: false
    bash: |
      echo "running ${svc}"
      if [ "${svc}" = "alpha" ]; then exit 1; fi
"#,
    )?;

    let tasks = plan(&ottofile, &["up"])?;
    let (scheduler, ok) = run(work_dir, tasks, 8).await?;
    assert!(!ok, "a failed subtask must make the run exit non-zero");

    let statuses = scheduler.get_task_statuses().await;
    assert!(matches!(statuses["up:alpha"], TaskStatus::Failed(_)));
    assert_eq!(statuses["up:beta"], TaskStatus::Skipped(SkipKind::SerialPredecessor));
    assert_eq!(statuses["up:gamma"], TaskStatus::Skipped(SkipKind::SerialPredecessor));
    assert!(
        matches!(statuses["up"], TaskStatus::Failed(_)),
        "the parent aggregates Failed when a member failed, serial exactly as parallel; got {:?}",
        statuses["up"]
    );

    let records = scheduler.get_skip_records().await;
    assert_eq!(
        records.get("up:beta").map(|r| r.detail.as_str()),
        Some("serial predecessor up:alpha failed")
    );
    assert_eq!(
        records.get("up:gamma").map(|r| r.detail.as_str()),
        Some("serial predecessor up:beta skipped; cascade")
    );
    Ok(())
}

/// Terminal-state matrix, up-to-date arm: a predecessor that lands in `completed_set`
/// via the up-to-date skip does NOT block its successor. All three members are reached
/// and skipped as up-to-date, which carries no skip reason (it is a success, not a gate).
#[tokio::test]
#[serial]
async fn test_up_to_date_skipped_predecessor_does_not_block_successor() -> Result<()> {
    let temp_dir = TempDir::new()?;
    let work_dir = PathBuf::from(temp_dir.path());
    setup_test_env(&work_dir);

    let input = work_dir.join("in.txt");
    let output = work_dir.join("out.txt");
    std::fs::write(&input, "input\n")?;
    std::thread::sleep(Duration::from_millis(20));
    std::fs::write(&output, "output\n")?;

    let ottofile = write_fixture(
        temp_dir.path(),
        &format!(
            r#"
tasks:
  up:
    foreach:
      items: [alpha, beta, gamma]
      as: svc
      parallel: false
    input:
      - {input}
    output:
      - {output}
    bash: echo "running ${{svc}}"
"#,
            input = input.display(),
            output = output.display()
        ),
    )?;

    let tasks = plan(&ottofile, &["up"])?;
    let (scheduler, ok) = run(work_dir, tasks, 8).await?;
    assert!(ok, "an up-to-date run should succeed");

    let statuses = scheduler.get_task_statuses().await;
    for name in ["up:alpha", "up:beta", "up:gamma"] {
        assert_eq!(
            statuses[name],
            TaskStatus::Skipped(SkipKind::UpToDate),
            "{name} should be up-to-date skipped"
        );
    }

    let records = scheduler.get_skip_records().await;
    for name in ["up:beta", "up:gamma"] {
        assert!(
            !records.contains_key(name),
            "{name} was gated out ({:?}) instead of being reached and found up to date",
            records.get(name)
        );
    }
    Ok(())
}

/// Terminal-state matrix, ordinary-`when:`-edge arm (panel fixture 1): the group members
/// inherit `before: [dep]`; dep fails, so every member is unreachable. The run exits
/// non-zero because dep failed, and every skip carries a visible reason - it does not hang.
#[tokio::test]
#[serial]
async fn test_failing_dependency_skips_group_visibly_and_exits_non_zero() -> Result<()> {
    let temp_dir = TempDir::new()?;
    let work_dir = PathBuf::from(temp_dir.path());
    setup_test_env(&work_dir);

    let ottofile = write_fixture(
        temp_dir.path(),
        r#"
tasks:
  dep:
    bash: |
      echo "dep running"
      exit 1
  up:
    before: [dep]
    foreach:
      items: [alpha, beta, gamma]
      as: svc
      parallel: false
    bash: echo "running ${svc}"
"#,
    )?;

    let tasks = plan(&ottofile, &["up"])?;
    let (scheduler, ok) = run(work_dir, tasks, 8).await?;
    assert!(!ok, "the run must exit non-zero because dep failed");

    let statuses = scheduler.get_task_statuses().await;
    assert!(matches!(statuses["dep"], TaskStatus::Failed(_)));
    let records = scheduler.get_skip_records().await;
    for name in ["up:alpha", "up:beta", "up:gamma", "up"] {
        assert_eq!(statuses[name], TaskStatus::Skipped(SkipKind::Unreachable));
        assert!(
            records.contains_key(name),
            "{name} must be skipped with a visible reason"
        );
    }
    Ok(())
}

/// Terminal-state matrix, ordinary-`when:`-edge arm (panel fixture 2): a failing task
/// with `after: [up:alpha]` puts only the first member behind a conditional edge. That
/// member enters `skipped_set` (not `failed_set`), and the serial gate must cascade the
/// skip to beta and gamma rather than leave them waiting for a predecessor that will
/// never finish.
#[tokio::test]
#[serial]
async fn test_predecessor_skipped_by_conditional_edge_cascades_visibly() -> Result<()> {
    let temp_dir = TempDir::new()?;
    let work_dir = PathBuf::from(temp_dir.path());
    setup_test_env(&work_dir);

    let ottofile = write_fixture(
        temp_dir.path(),
        r#"
tasks:
  boom:
    after: ["up:alpha"]
    bash: |
      echo "boom running"
      exit 1
  up:
    foreach:
      items: [alpha, beta, gamma]
      as: svc
      parallel: false
    bash: echo "running ${svc}"
"#,
    )?;

    let tasks = plan(&ottofile, &["up"])?;
    let (scheduler, ok) = run(work_dir, tasks, 8).await?;
    assert!(!ok, "the run must exit non-zero because boom failed");

    let statuses = scheduler.get_task_statuses().await;
    assert!(matches!(statuses["boom"], TaskStatus::Failed(_)));
    assert_eq!(statuses["up:alpha"], TaskStatus::Skipped(SkipKind::Unreachable));

    let records = scheduler.get_skip_records().await;
    assert_eq!(
        records.get("up:beta").map(|r| r.detail.as_str()),
        Some("serial predecessor up:alpha skipped; cascade"),
        "a predecessor skipped by an ordinary when: edge must cascade, not hang"
    );
    assert_eq!(
        records.get("up:gamma").map(|r| r.detail.as_str()),
        Some("serial predecessor up:beta skipped; cascade")
    );
    Ok(())
}

/// Risks row 1: a serial group member that also carries an explicit `before:` edge must
/// satisfy both gates and must not deadlock. The serial gate composes with dependency
/// readiness, it does not replace it.
#[tokio::test]
#[serial]
async fn test_mixed_edges_group_runs_in_order_without_deadlock() -> Result<()> {
    let temp_dir = TempDir::new()?;
    let work_dir = PathBuf::from(temp_dir.path());
    setup_test_env(&work_dir);

    let trace = work_dir.join("trace.txt");
    let ottofile = write_fixture(
        temp_dir.path(),
        &format!(
            r#"
tasks:
  dep:
    bash: echo "dep" >> {trace}
  up:
    before: [dep]
    foreach:
      items: [alpha, beta, gamma]
      as: svc
      parallel: false
    bash: echo "${{svc}}" >> {trace}
"#,
            trace = trace.display()
        ),
    )?;

    let tasks = plan(&ottofile, &["up"])?;
    let (scheduler, ok) = run(work_dir, tasks, 8).await?;
    assert!(ok, "mixed edges must not deadlock or fail");

    let recorded = std::fs::read_to_string(&trace)?;
    assert_eq!(
        recorded.lines().collect::<Vec<_>>(),
        vec!["dep", "alpha", "beta", "gamma"],
        "dependency then serial order; got {recorded}"
    );

    let statuses = scheduler.get_task_statuses().await;
    assert_eq!(statuses["up"], TaskStatus::Completed);
    Ok(())
}
