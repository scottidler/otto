//! Skip provenance decides `when:` classification.
//!
//! Phase 2 of docs/design/2026-06-10-code-review-remediation.md. A Skipped source no
//! longer short-circuits every `when:` variant to Unreachable; it is resolved against
//! the edge using its `SkipKind`. The nine-cell contract itself is pinned as a unit
//! test (`classify_edge_skip_provenance_matrix` in `src/executor/scheduler.rs`); this
//! file exercises the four behaviors that contract exists to deliver, end to end
//! through the real parser and scheduler.

mod common;

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

fn write_fixture(dir: &Path, body: &str) -> Result<PathBuf> {
    let path = dir.join("otto.yml");
    std::fs::write(&path, body)?;
    Ok(path)
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

/// A warm cache is a success, not a gate.
///
/// Every subtask of the foreach is up to date, so the parent aggregates Completed and
/// the downstream task runs. Before this phase the parent aggregated Skipped, which
/// killed everything downstream at exit 0 - while the identical non-foreach shape ran
/// its downstream task.
#[tokio::test]
#[serial]
async fn test_warm_cache_foreach_runs_its_downstream_task() -> Result<()> {
    let temp_dir = TempDir::new()?;
    let work_dir = PathBuf::from(temp_dir.path());
    setup_test_env(&work_dir);

    let input = work_dir.join("in.txt");
    let output = work_dir.join("out.txt");
    std::fs::write(&input, "input\n")?;
    std::thread::sleep(Duration::from_millis(20));
    std::fs::write(&output, "output\n")?;

    let marker = work_dir.join("deployed.txt");
    let ottofile = write_fixture(
        temp_dir.path(),
        &format!(
            r#"
tasks:
  build:
    foreach:
      items: [a, b]
      as: pkg
    input:
      - {input}
    output:
      - {output}
    bash: echo "building ${{pkg}}"

  deploy:
    before: [build]
    bash: echo deployed > {marker}
"#,
            input = input.display(),
            output = output.display(),
            marker = marker.display()
        ),
    )?;

    let tasks = plan(&ottofile, &["deploy"])?;
    let (scheduler, ok) = run(work_dir, tasks, 4).await?;
    assert!(ok, "a warm-cache run must succeed");

    let statuses = scheduler.get_task_statuses().await;
    assert_eq!(statuses["build:a"], TaskStatus::Skipped(SkipKind::UpToDate));
    assert_eq!(statuses["build:b"], TaskStatus::Skipped(SkipKind::UpToDate));
    assert_eq!(
        statuses["build"],
        TaskStatus::Completed,
        "an all-up-to-date foreach is a warm cache, so the parent is Completed; got {:?}",
        statuses["build"]
    );
    assert_eq!(statuses["deploy"], TaskStatus::Completed);
    assert!(marker.exists(), "the downstream task must actually have run");
    Ok(())
}

/// `when: always` cleanup fires on a cascade-skipped source, which is exactly when a
/// chain broke and cleanup matters most.
#[tokio::test]
#[serial]
async fn test_always_cleanup_runs_on_a_cascade_skipped_source() -> Result<()> {
    let temp_dir = TempDir::new()?;
    let work_dir = PathBuf::from(temp_dir.path());
    setup_test_env(&work_dir);

    let marker = work_dir.join("cleaned.txt");
    let ottofile = write_fixture(
        temp_dir.path(),
        &format!(
            r#"
tasks:
  a:
    bash: exit 1

  b:
    before: [a]
    bash: echo b

  cleanup:
    before:
      - task: b
        when: always
    bash: echo cleaned > {marker}
"#,
            marker = marker.display()
        ),
    )?;

    let tasks = plan(&ottofile, &["cleanup"])?;
    let (scheduler, ok) = run(work_dir, tasks, 4).await?;
    assert!(!ok, "a's failure must still surface as the run's error");

    let statuses = scheduler.get_task_statuses().await;
    assert!(matches!(statuses["a"], TaskStatus::Failed(_)));
    assert_eq!(statuses["b"], TaskStatus::Skipped(SkipKind::Unreachable));
    assert_eq!(
        statuses["cleanup"],
        TaskStatus::Completed,
        "when: always is satisfied by every terminal state, skips included; got {:?}",
        statuses["cleanup"]
    );
    assert!(marker.exists(), "cleanup must actually have run");
    Ok(())
}

/// A serial foreach whose middle member fails aggregates the parent to Failed, so an
/// `on-failure:` fixer attached to the group fires. Serial and parallel foreach now
/// agree; before this phase only the parallel form ran the fixer.
#[tokio::test]
#[serial]
async fn test_serial_foreach_mid_chain_failure_runs_its_on_failure_fixer() -> Result<()> {
    let temp_dir = TempDir::new()?;
    let work_dir = PathBuf::from(temp_dir.path());
    setup_test_env(&work_dir);

    let marker = work_dir.join("fixed.txt");
    let ottofile = write_fixture(
        temp_dir.path(),
        &format!(
            r#"
tasks:
  step:
    foreach:
      items: [a, b, c]
      as: item
      parallel: false
    on-failure: [fixer]
    bash: |
      echo "running ${{item}}"
      if [ "${{item}}" = "b" ]; then exit 1; fi

  fixer:
    bash: echo fixed > {marker}
"#,
            marker = marker.display()
        ),
    )?;

    let tasks = plan(&ottofile, &["step"])?;
    let (scheduler, ok) = run(work_dir, tasks, 4).await?;
    assert!(!ok, "the mid-chain failure must still surface");

    let statuses = scheduler.get_task_statuses().await;
    assert_eq!(statuses["step:a"], TaskStatus::Completed);
    assert!(matches!(statuses["step:b"], TaskStatus::Failed(_)));
    assert_eq!(statuses["step:c"], TaskStatus::Skipped(SkipKind::SerialPredecessor));
    assert!(
        matches!(statuses["step"], TaskStatus::Failed(_)),
        "the parent aggregates Failed because a member failed; got {:?}",
        statuses["step"]
    );
    assert_eq!(statuses["fixer"], TaskStatus::Completed);
    assert!(marker.exists(), "the on-failure fixer must actually have run");
    Ok(())
}

/// An up-to-date skip is success-like, but it is still not a failure: a `when: failure`
/// handler must not fire off a warm cache. This is the `UpToDate` x `failure` cell of
/// the provenance table, exercised end to end.
#[tokio::test]
#[serial]
async fn test_up_to_date_source_does_not_satisfy_when_failure() -> Result<()> {
    let temp_dir = TempDir::new()?;
    let work_dir = PathBuf::from(temp_dir.path());
    setup_test_env(&work_dir);

    let input = work_dir.join("in.txt");
    let output = work_dir.join("out.txt");
    std::fs::write(&input, "input\n")?;
    std::thread::sleep(Duration::from_millis(20));
    std::fs::write(&output, "output\n")?;

    let marker = work_dir.join("fixer-ran.txt");
    let ottofile = write_fixture(
        temp_dir.path(),
        &format!(
            r#"
tasks:
  build:
    input:
      - {input}
    output:
      - {output}
    bash: echo building

  fixer:
    before:
      - task: build
        when: failure
    bash: echo ran > {marker}
"#,
            input = input.display(),
            output = output.display(),
            marker = marker.display()
        ),
    )?;

    let tasks = plan(&ottofile, &["fixer"])?;
    let (scheduler, ok) = run(work_dir, tasks, 4).await?;
    assert!(ok, "nothing failed, so the run succeeds");

    let statuses = scheduler.get_task_statuses().await;
    assert_eq!(statuses["build"], TaskStatus::Skipped(SkipKind::UpToDate));
    assert_eq!(statuses["fixer"], TaskStatus::Skipped(SkipKind::Unreachable));
    assert!(
        !marker.exists(),
        "a warm cache is not a failure; the fixer must not run"
    );
    Ok(())
}

/// Paired `when: success` + `when: failure` on the same source is rejected at config
/// load. One of the two can never be satisfied, so the dependent could never run - and
/// it used to be skipped silently at exit 0.
#[test]
#[serial]
fn test_paired_success_and_failure_edges_are_rejected_at_config_load() -> Result<()> {
    let temp_dir = TempDir::new()?;
    setup_test_env(temp_dir.path());

    let ottofile = write_fixture(
        temp_dir.path(),
        r#"
tasks:
  src:
    bash: echo src

  dep:
    before:
      - task: src
        when: success
      - task: src
        when: failure
    bash: echo dep
"#,
    )?;

    let err = plan(&ottofile, &["dep"]).expect_err("paired success+failure edges must be rejected");
    let message = format!("{err:#}");
    assert!(
        message.contains("'dep'") && message.contains("'src'"),
        "the error must name both tasks; got: {message}"
    );
    assert!(
        message.contains("when: success") && message.contains("when: failure"),
        "the error must name both conditions; got: {message}"
    );
    Ok(())
}

/// File-based data passing survives a multiline value, end to end through the real
/// binary. `MULTI='line1\nline2\nline3'` in the `.env` used to arrive as `'line1` -
/// a stray quote and one line of what the task wrote - while the run exited 0.
///
/// The reads used to be spelled `producer.single`/`.quoted`/`.multi` against a
/// producer that wrote `SINGLE`/`QUOTED`/`MULTI`, and passed - because the
/// reader lowercased every key on the way in. That was the bug fixed alongside
/// this: the key a consumer asks for is now the key the producer wrote.
#[test]
#[serial]
fn test_multiline_output_reaches_the_consumer_intact() {
    let dir = TempDir::new().expect("tempdir");
    let otto_home = dir.path().join("otto-home");
    std::fs::create_dir_all(&otto_home).expect("create otto home");
    std::fs::write(
        dir.path().join("otto.yml"),
        r#"
tasks:
  producer:
    bash: |
      otto_set_output SINGLE "one-line"
      otto_set_output MULTI "$(printf 'line1\nline2\nline3')"
      otto_set_output QUOTED "it's quoted"

  consumer:
    before: [producer]
    bash: |
      echo "SINGLE=[$(otto_get_input producer.SINGLE)]"
      echo "QUOTED=[$(otto_get_input producer.QUOTED)]"
      echo "MULTI=[$(otto_get_input producer.MULTI)]"
"#,
    )
    .expect("write ottofile");

    let output = common::otto_std_cmd(&otto_home)
        .current_dir(dir.path())
        .env_remove("OTTOFILE")
        .arg("consumer")
        .output()
        .expect("failed to run otto");

    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    assert_eq!(output.status.code(), Some(0), "run failed: {stdout}");
    assert!(stdout.contains("SINGLE=[one-line]"), "single-line value lost: {stdout}");
    assert!(stdout.contains("QUOTED=[it's quoted]"), "quoted value lost: {stdout}");
    assert!(
        stdout.contains("MULTI=[line1") && stdout.contains("line2") && stdout.contains("line3]"),
        "multiline value truncated or dropped: {stdout}"
    );
    assert!(
        !stdout.contains("MULTI=[]") && !stdout.contains("MULTI=['line1"),
        "multiline value must be neither empty nor quote-prefixed: {stdout}"
    );
}
