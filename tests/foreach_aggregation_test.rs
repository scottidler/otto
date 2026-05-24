use eyre::Result;
use serial_test::serial;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tempfile::TempDir;
use tokio::time::timeout;

use otto::Parser;
use otto::cfg::edge::When;
use otto::executor::task::TaskEdge;
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

/// All foreach subtasks succeed -> parent aggregates Completed.
#[tokio::test]
#[serial]
async fn test_foreach_parent_aggregates_completed_when_all_subtasks_succeed() -> Result<()> {
    let temp_dir = TempDir::new()?;
    let work_dir = PathBuf::from(temp_dir.path());
    setup_test_db(&work_dir);

    let ottofile = work_dir.join("otto.yml");
    std::fs::write(
        &ottofile,
        r#"
tasks:
  install:
    foreach:
      items: [a, b, c]
      as: pkg
    bash: echo ${pkg}
"#,
    )?;

    let args = vec![
        "otto".to_string(),
        "--ottofile".to_string(),
        ottofile.to_string_lossy().to_string(),
        "install".to_string(),
    ];
    let mut parser = Parser::new(args)?;
    let (parser_tasks, _, _, _, _) = parser.parse()?;

    let tasks = convert_to_executor_tasks(parser_tasks);
    let workspace = Workspace::new(work_dir).await?;
    workspace.init().await?;
    let scheduler = TaskScheduler::new(tasks, Arc::new(workspace), ExecutionContext::new(), 4, false).await?;
    timeout(Duration::from_secs(10), scheduler.execute_all()).await??;

    let statuses = scheduler.get_task_statuses().await;
    assert_eq!(statuses["install:a"], TaskStatus::Completed);
    assert_eq!(statuses["install:b"], TaskStatus::Completed);
    assert_eq!(statuses["install:c"], TaskStatus::Completed);
    assert_eq!(statuses["install"], TaskStatus::Completed);
    Ok(())
}

/// Any subtask fails -> parent aggregates Failed.
#[tokio::test]
#[serial]
async fn test_foreach_parent_aggregates_failed_when_any_subtask_fails() -> Result<()> {
    let temp_dir = TempDir::new()?;
    let work_dir = PathBuf::from(temp_dir.path());
    setup_test_db(&work_dir);

    let ottofile = work_dir.join("otto.yml");
    std::fs::write(
        &ottofile,
        r#"
tasks:
  install:
    foreach:
      items: [a, b, c]
      as: pkg
    bash: |
      if [ "${pkg}" = "b" ]; then exit 1; fi
      echo ${pkg}
"#,
    )?;

    let args = vec![
        "otto".to_string(),
        "--ottofile".to_string(),
        ottofile.to_string_lossy().to_string(),
        "install".to_string(),
    ];
    let mut parser = Parser::new(args)?;
    let (parser_tasks, _, _, _, _) = parser.parse()?;

    let tasks = convert_to_executor_tasks(parser_tasks);
    let workspace = Workspace::new(work_dir).await?;
    workspace.init().await?;
    let scheduler = TaskScheduler::new(tasks, Arc::new(workspace), ExecutionContext::new(), 4, false).await?;
    let result = timeout(Duration::from_secs(10), scheduler.execute_all()).await?;
    assert!(result.is_err(), "expected subtask failure to surface");

    let statuses = scheduler.get_task_statuses().await;
    assert_eq!(statuses["install:a"], TaskStatus::Completed);
    match &statuses["install:b"] {
        TaskStatus::Failed(_) => {}
        other => panic!("expected install:b Failed, got {:?}", other),
    }
    assert_eq!(statuses["install:c"], TaskStatus::Completed);
    match &statuses["install"] {
        TaskStatus::Failed(_) => {}
        other => panic!("expected install (parent) Failed, got {:?}", other),
    }
    Ok(())
}

/// A `when: failure` dependent on a foreach parent fires when any subtask fails.
#[tokio::test]
#[serial]
async fn test_when_failure_on_foreach_parent_fires_on_subtask_failure() -> Result<()> {
    let temp_dir = TempDir::new()?;
    let work_dir = PathBuf::from(temp_dir.path());
    setup_test_db(&work_dir);

    let ottofile = work_dir.join("otto.yml");
    // In otto's `after:` semantic, "X has after:[Y]" means Y runs after X.
    // To express "fixer runs after install fails", put after: on install (the host)
    // naming fixer (the target).
    std::fs::write(
        &ottofile,
        r#"
tasks:
  install:
    foreach:
      items: [a, b]
      as: pkg
    bash: |
      if [ "${pkg}" = "b" ]; then exit 1; fi
      echo ${pkg}
    after:
      - task: fixer
        when: failure

  fixer:
    bash: echo fixing
"#,
    )?;

    let args = vec![
        "otto".to_string(),
        "--ottofile".to_string(),
        ottofile.to_string_lossy().to_string(),
        "install".to_string(),
    ];
    let mut parser = Parser::new(args)?;
    let (parser_tasks, _, _, _, _) = parser.parse()?;

    let tasks = convert_to_executor_tasks(parser_tasks);
    let workspace = Workspace::new(work_dir).await?;
    workspace.init().await?;
    let scheduler = TaskScheduler::new(tasks, Arc::new(workspace), ExecutionContext::new(), 4, false).await?;
    let result = timeout(Duration::from_secs(10), scheduler.execute_all()).await?;
    assert!(result.is_err(), "expected subtask failure to surface");

    let statuses = scheduler.get_task_statuses().await;
    match &statuses["install"] {
        TaskStatus::Failed(_) => {}
        other => panic!("expected install Failed, got {:?}", other),
    }
    assert_eq!(statuses["fixer"], TaskStatus::Completed);
    Ok(())
}

/// A prerequisite of the foreach group fails -> subtasks Skipped -> parent aggregates Skipped.
/// A downstream `when: failure` does NOT fire (Skipped is not Failure).
#[tokio::test]
#[serial]
async fn test_foreach_parent_aggregates_skipped_when_prereq_fails() -> Result<()> {
    let temp_dir = TempDir::new()?;
    let work_dir = PathBuf::from(temp_dir.path());
    setup_test_db(&work_dir);

    let ottofile = work_dir.join("otto.yml");
    std::fs::write(
        &ottofile,
        r#"
tasks:
  preflight:
    bash: exit 1

  install:
    before: [preflight]
    foreach:
      items: [a, b]
      as: pkg
    bash: echo ${pkg}
    after:
      - task: fixer
        when: failure

  fixer:
    bash: echo fixing
"#,
    )?;

    let args = vec![
        "otto".to_string(),
        "--ottofile".to_string(),
        ottofile.to_string_lossy().to_string(),
        "install".to_string(),
    ];
    let mut parser = Parser::new(args)?;
    let (parser_tasks, _, _, _, _) = parser.parse()?;

    let tasks = convert_to_executor_tasks(parser_tasks);
    let workspace = Workspace::new(work_dir).await?;
    workspace.init().await?;
    let scheduler = TaskScheduler::new(tasks, Arc::new(workspace), ExecutionContext::new(), 4, false).await?;
    let result = timeout(Duration::from_secs(10), scheduler.execute_all()).await?;
    assert!(result.is_err(), "preflight failure must surface");

    let statuses = scheduler.get_task_statuses().await;
    match &statuses["preflight"] {
        TaskStatus::Failed(_) => {}
        other => panic!("expected preflight Failed, got {:?}", other),
    }
    assert_eq!(statuses["install:a"], TaskStatus::Skipped);
    assert_eq!(statuses["install:b"], TaskStatus::Skipped);
    assert_eq!(statuses["install"], TaskStatus::Skipped);
    // fixer's when: failure on install is Unreachable (install was Skipped, not Failed).
    assert_eq!(statuses["fixer"], TaskStatus::Skipped);
    Ok(())
}

/// Hand-rolled virtual parent (no foreach) - just verify the executor accepts a
/// task with is_virtual_parent=true and aggregates correctly.
#[tokio::test]
#[serial]
async fn test_manual_virtual_parent_aggregates_correctly() -> Result<()> {
    let temp_dir = TempDir::new()?;
    let work_dir = PathBuf::from(temp_dir.path());
    setup_test_db(&work_dir);

    let mut child = Task::new(
        "group:child".to_string(),
        Some("group".to_string()),
        vec![],
        vec![],
        vec![],
        HashMap::new(),
        HashMap::new(),
        "echo child".to_string(),
    );
    child.is_virtual_parent = false;

    let mut parent = Task::new(
        "group".to_string(),
        None,
        vec![TaskEdge::new("group:child", When::Always)],
        vec![],
        vec![],
        HashMap::new(),
        HashMap::new(),
        String::new(), // empty action -> fast path
    );
    parent.is_virtual_parent = true;

    let workspace = Workspace::new(work_dir).await?;
    workspace.init().await?;
    let scheduler = TaskScheduler::new(
        vec![child, parent],
        Arc::new(workspace),
        ExecutionContext::new(),
        2,
        false,
    )
    .await?;
    timeout(Duration::from_secs(5), scheduler.execute_all()).await??;

    let statuses = scheduler.get_task_statuses().await;
    assert_eq!(statuses["group:child"], TaskStatus::Completed);
    assert_eq!(statuses["group"], TaskStatus::Completed);
    Ok(())
}
