use eyre::Result;
use serial_test::serial;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tempfile::TempDir;
use tokio::time::timeout;

use otto::Parser;
use otto::executor::state::SkipKind;
use otto::executor::{Task, TaskScheduler, TaskStatus, Workspace, workspace::ExecutionContext};

fn setup_test_db(temp_dir: &std::path::Path) -> PathBuf {
    let db_path = temp_dir.join("test_otto.db");
    let otto_home = temp_dir.join(".otto");
    unsafe {
        std::env::set_var("OTTO_DB_PATH", &db_path);
        std::env::set_var("OTTO_HOME", &otto_home);
    }
    db_path
}

fn convert_to_executor_tasks(parser_tasks: Vec<otto::cli::parser::Task>) -> Vec<Task> {
    parser_tasks
        .into_iter()
        .map(|pt| {
            let parent = if pt.name.contains(':') {
                pt.name.split(':').next().map(|s| s.to_string())
            } else {
                None
            };
            let mut t = Task::new(
                pt.name,
                parent,
                pt.task_deps,
                pt.file_deps,
                pt.output_deps,
                pt.envs,
                pt.values,
                pt.action,
            );
            t.is_virtual_parent = pt.is_virtual_parent;
            t
        })
        .collect()
}

/// The cargo-fmt motivating case: `on-failure:` on a check task fires the fixer
/// only when the check fails.
#[tokio::test]
#[serial]
async fn test_on_failure_sugar_fires_on_check_failure() -> Result<()> {
    let temp_dir = TempDir::new()?;
    let work_dir = PathBuf::from(temp_dir.path());
    setup_test_db(&work_dir);

    let ottofile = work_dir.join("otto.yml");
    std::fs::write(
        &ottofile,
        r#"
tasks:
  fmt-check:
    bash: exit 1
    on-failure: [fmt-fix]

  fmt-fix:
    bash: echo fixing
"#,
    )?;

    let args = vec![
        "otto".to_string(),
        "--ottofile".to_string(),
        ottofile.to_string_lossy().to_string(),
        "fmt-check".to_string(),
    ];
    let mut parser = Parser::new(args)?;
    let (parser_tasks, _, _, _, _, _) = parser.parse()?.into_run()?.into_parts();

    let tasks = convert_to_executor_tasks(parser_tasks);
    let workspace = Workspace::new(work_dir).await?;
    workspace.init().await?;
    let scheduler = TaskScheduler::new(tasks, Arc::new(workspace), ExecutionContext::new(), 2, false).await?;
    let result = timeout(Duration::from_secs(5), scheduler.execute_all()).await?;
    assert!(result.is_err(), "fmt-check failure must propagate to exit");

    let statuses = scheduler.get_task_statuses().await;
    match &statuses["fmt-check"] {
        TaskStatus::Failed(_) => {}
        other => panic!("expected fmt-check Failed, got {:?}", other),
    }
    assert_eq!(statuses["fmt-fix"], TaskStatus::Completed);
    Ok(())
}

/// When the check succeeds, the on-failure fixer does NOT fire.
#[tokio::test]
#[serial]
async fn test_on_failure_sugar_does_not_fire_on_success() -> Result<()> {
    let temp_dir = TempDir::new()?;
    let work_dir = PathBuf::from(temp_dir.path());
    setup_test_db(&work_dir);

    let ottofile = work_dir.join("otto.yml");
    std::fs::write(
        &ottofile,
        r#"
tasks:
  fmt-check:
    bash: echo all good
    on-failure: [fmt-fix]

  fmt-fix:
    bash: echo fixing
"#,
    )?;

    let args = vec![
        "otto".to_string(),
        "--ottofile".to_string(),
        ottofile.to_string_lossy().to_string(),
        "fmt-check".to_string(),
    ];
    let mut parser = Parser::new(args)?;
    let (parser_tasks, _, _, _, _, _) = parser.parse()?.into_run()?.into_parts();

    let tasks = convert_to_executor_tasks(parser_tasks);
    let workspace = Workspace::new(work_dir).await?;
    workspace.init().await?;
    let scheduler = TaskScheduler::new(tasks, Arc::new(workspace), ExecutionContext::new(), 2, false).await?;
    timeout(Duration::from_secs(5), scheduler.execute_all()).await??;

    let statuses = scheduler.get_task_statuses().await;
    assert_eq!(statuses["fmt-check"], TaskStatus::Completed);
    assert_eq!(statuses["fmt-fix"], TaskStatus::Skipped(SkipKind::Unreachable));
    Ok(())
}

/// Self-reference is rejected with a clear error.
#[test]
#[serial]
fn test_on_failure_self_reference_rejected() {
    let temp_dir = TempDir::new().unwrap();
    let ottofile = temp_dir.path().join("otto.yml");
    std::fs::write(
        &ottofile,
        r#"
tasks:
  loop:
    bash: echo loop
    on-failure: [loop]
"#,
    )
    .unwrap();

    let args = vec![
        "otto".to_string(),
        "--ottofile".to_string(),
        ottofile.to_string_lossy().to_string(),
        "loop".to_string(),
    ];
    let mut parser = Parser::new(args).unwrap();
    let result = parser.parse();
    assert!(result.is_err(), "self-reference should be rejected");
    let err = result.unwrap_err().to_string();
    assert!(
        err.contains("cannot depend on its own failure") || err.contains("on-failure on task"),
        "error should mention self-reference: {}",
        err
    );
}

/// Unknown target task in `on-failure:` is rejected.
#[test]
#[serial]
fn test_on_failure_unknown_target_rejected() {
    let temp_dir = TempDir::new().unwrap();
    let ottofile = temp_dir.path().join("otto.yml");
    std::fs::write(
        &ottofile,
        r#"
tasks:
  check:
    bash: exit 1
    on-failure: [nonexistent]
"#,
    )
    .unwrap();

    let args = vec![
        "otto".to_string(),
        "--ottofile".to_string(),
        ottofile.to_string_lossy().to_string(),
        "check".to_string(),
    ];
    let mut parser = Parser::new(args).unwrap();
    let result = parser.parse();
    assert!(result.is_err(), "unknown target should be rejected");
}

/// Round-trip: an ottofile with `on-failure:` serializes back the same way -
/// the synthetic `after:` edge is filtered out.
#[test]
fn test_on_failure_round_trip_preserves_host_field() {
    use otto::cfg::config::ConfigSpec;
    let yaml_in = r#"
tasks:
  fmt-check:
    bash: cargo fmt --check
    on-failure:
    - fmt-fix
  fmt-fix:
    bash: cargo fmt
"#;
    let config: ConfigSpec = serde_yaml_ng::from_str(yaml_in).unwrap();
    // The desugar pass runs in process_tasks_with_filter, not at deserialize time -
    // so the raw config still has on-failure on fmt-check and no synthetic after on fmt-fix.
    let fmt_check = config.tasks.get("fmt-check").expect("fmt-check");
    assert_eq!(fmt_check.on_failure, vec!["fmt-fix".to_string()]);
    assert!(fmt_check.after.is_empty());
    let fmt_fix = config.tasks.get("fmt-fix").expect("fmt-fix");
    assert!(fmt_fix.after.is_empty());
    assert!(fmt_fix.on_failure.is_empty());

    // Re-serialize. The output should contain `on-failure:` on fmt-check and
    // no synthetic `after:` entry.
    let yaml_out = serde_yaml_ng::to_string(&config).unwrap();
    assert!(
        yaml_out.contains("on-failure"),
        "expected on-failure: in serialized output, got:\n{}",
        yaml_out
    );
    assert!(
        !yaml_out.contains("when: failure"),
        "synthetic after edge should not appear in serialized output, got:\n{}",
        yaml_out
    );
}
