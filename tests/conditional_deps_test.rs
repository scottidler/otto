use eyre::Result;
use serial_test::serial;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tempfile::TempDir;
use tokio::time::timeout;

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

/// when: success dep that succeeds -> dependent runs, run exits ok.
#[tokio::test]
#[serial]
async fn test_when_success_dep_succeeds_dependent_runs() -> Result<()> {
    let temp_dir = TempDir::new()?;
    let work_dir = PathBuf::from(temp_dir.path());
    setup_test_db(&work_dir);

    let dep = Task::new(
        "dep".to_string(),
        None,
        vec![],
        vec![],
        vec![],
        HashMap::new(),
        HashMap::new(),
        "echo dep".to_string(),
    );
    let dependent = Task::new(
        "dependent".to_string(),
        None,
        vec![TaskEdge::new("dep", When::Success)],
        vec![],
        vec![],
        HashMap::new(),
        HashMap::new(),
        "echo dependent".to_string(),
    );
    let workspace = Workspace::new(work_dir).await?;
    workspace.init().await?;
    let scheduler = TaskScheduler::new(
        vec![dep, dependent],
        Arc::new(workspace),
        ExecutionContext::new(),
        2,
        false,
    )
    .await?;
    timeout(Duration::from_secs(5), scheduler.execute_all()).await??;
    let statuses = scheduler.get_task_statuses().await;
    assert_eq!(statuses["dep"], TaskStatus::Completed);
    assert_eq!(statuses["dependent"], TaskStatus::Completed);
    Ok(())
}

/// when: success dep that fails -> dependent is Skipped, run exits non-zero.
#[tokio::test]
#[serial]
async fn test_when_success_dep_fails_dependent_skipped() -> Result<()> {
    let temp_dir = TempDir::new()?;
    let work_dir = PathBuf::from(temp_dir.path());
    setup_test_db(&work_dir);

    let dep = Task::new(
        "dep".to_string(),
        None,
        vec![],
        vec![],
        vec![],
        HashMap::new(),
        HashMap::new(),
        "exit 1".to_string(),
    );
    let dependent = Task::new(
        "dependent".to_string(),
        None,
        vec![TaskEdge::new("dep", When::Success)],
        vec![],
        vec![],
        HashMap::new(),
        HashMap::new(),
        "echo dependent".to_string(),
    );
    let workspace = Workspace::new(work_dir).await?;
    workspace.init().await?;
    let scheduler = TaskScheduler::new(
        vec![dep, dependent],
        Arc::new(workspace),
        ExecutionContext::new(),
        2,
        false,
    )
    .await?;

    let result = timeout(Duration::from_secs(5), scheduler.execute_all()).await?;
    assert!(result.is_err(), "expected run to surface dep's failure");

    let statuses = scheduler.get_task_statuses().await;
    match &statuses["dep"] {
        TaskStatus::Failed(_) => {}
        other => panic!("expected dep Failed, got {:?}", other),
    }
    assert_eq!(statuses["dependent"], TaskStatus::Skipped);
    Ok(())
}

/// when: failure dep that succeeds -> dependent is Skipped, run exits ok.
#[tokio::test]
#[serial]
async fn test_when_failure_dep_succeeds_dependent_skipped() -> Result<()> {
    let temp_dir = TempDir::new()?;
    let work_dir = PathBuf::from(temp_dir.path());
    setup_test_db(&work_dir);

    let dep = Task::new(
        "dep".to_string(),
        None,
        vec![],
        vec![],
        vec![],
        HashMap::new(),
        HashMap::new(),
        "echo dep".to_string(),
    );
    let fixer = Task::new(
        "fixer".to_string(),
        None,
        vec![TaskEdge::new("dep", When::Failure)],
        vec![],
        vec![],
        HashMap::new(),
        HashMap::new(),
        "echo fixer".to_string(),
    );
    let workspace = Workspace::new(work_dir).await?;
    workspace.init().await?;
    let scheduler =
        TaskScheduler::new(vec![dep, fixer], Arc::new(workspace), ExecutionContext::new(), 2, false).await?;
    timeout(Duration::from_secs(5), scheduler.execute_all()).await??;
    let statuses = scheduler.get_task_statuses().await;
    assert_eq!(statuses["dep"], TaskStatus::Completed);
    assert_eq!(statuses["fixer"], TaskStatus::Skipped);
    Ok(())
}

/// when: failure dep that fails -> dependent runs, run exits non-zero (host failure preserved).
#[tokio::test]
#[serial]
async fn test_when_failure_dep_fails_dependent_runs() -> Result<()> {
    let temp_dir = TempDir::new()?;
    let work_dir = PathBuf::from(temp_dir.path());
    setup_test_db(&work_dir);

    let dep = Task::new(
        "dep".to_string(),
        None,
        vec![],
        vec![],
        vec![],
        HashMap::new(),
        HashMap::new(),
        "exit 1".to_string(),
    );
    let fixer = Task::new(
        "fixer".to_string(),
        None,
        vec![TaskEdge::new("dep", When::Failure)],
        vec![],
        vec![],
        HashMap::new(),
        HashMap::new(),
        "echo fixer".to_string(),
    );
    let workspace = Workspace::new(work_dir).await?;
    workspace.init().await?;
    let scheduler =
        TaskScheduler::new(vec![dep, fixer], Arc::new(workspace), ExecutionContext::new(), 2, false).await?;

    let result = timeout(Duration::from_secs(5), scheduler.execute_all()).await?;
    assert!(result.is_err(), "host failure must propagate to exit");

    let statuses = scheduler.get_task_statuses().await;
    match &statuses["dep"] {
        TaskStatus::Failed(_) => {}
        other => panic!("expected dep Failed, got {:?}", other),
    }
    assert_eq!(statuses["fixer"], TaskStatus::Completed);
    Ok(())
}

/// when: always runs whether dep succeeds or fails.
#[tokio::test]
#[serial]
async fn test_when_always_runs_on_dep_success() -> Result<()> {
    let temp_dir = TempDir::new()?;
    let work_dir = PathBuf::from(temp_dir.path());
    setup_test_db(&work_dir);

    let dep = Task::new(
        "dep".to_string(),
        None,
        vec![],
        vec![],
        vec![],
        HashMap::new(),
        HashMap::new(),
        "echo dep".to_string(),
    );
    let cleanup = Task::new(
        "cleanup".to_string(),
        None,
        vec![TaskEdge::new("dep", When::Always)],
        vec![],
        vec![],
        HashMap::new(),
        HashMap::new(),
        "echo cleanup".to_string(),
    );
    let workspace = Workspace::new(work_dir).await?;
    workspace.init().await?;
    let scheduler = TaskScheduler::new(
        vec![dep, cleanup],
        Arc::new(workspace),
        ExecutionContext::new(),
        2,
        false,
    )
    .await?;
    timeout(Duration::from_secs(5), scheduler.execute_all()).await??;
    let statuses = scheduler.get_task_statuses().await;
    assert_eq!(statuses["dep"], TaskStatus::Completed);
    assert_eq!(statuses["cleanup"], TaskStatus::Completed);
    Ok(())
}

#[tokio::test]
#[serial]
async fn test_when_always_runs_on_dep_failure() -> Result<()> {
    let temp_dir = TempDir::new()?;
    let work_dir = PathBuf::from(temp_dir.path());
    setup_test_db(&work_dir);

    let dep = Task::new(
        "dep".to_string(),
        None,
        vec![],
        vec![],
        vec![],
        HashMap::new(),
        HashMap::new(),
        "exit 1".to_string(),
    );
    let cleanup = Task::new(
        "cleanup".to_string(),
        None,
        vec![TaskEdge::new("dep", When::Always)],
        vec![],
        vec![],
        HashMap::new(),
        HashMap::new(),
        "echo cleanup".to_string(),
    );
    let workspace = Workspace::new(work_dir).await?;
    workspace.init().await?;
    let scheduler = TaskScheduler::new(
        vec![dep, cleanup],
        Arc::new(workspace),
        ExecutionContext::new(),
        2,
        false,
    )
    .await?;

    let result = timeout(Duration::from_secs(5), scheduler.execute_all()).await?;
    assert!(result.is_err(), "host failure must propagate to exit");

    let statuses = scheduler.get_task_statuses().await;
    match &statuses["dep"] {
        TaskStatus::Failed(_) => {}
        other => panic!("expected dep Failed, got {:?}", other),
    }
    assert_eq!(statuses["cleanup"], TaskStatus::Completed);
    Ok(())
}

/// Two parallel branches: branch A succeeds, branch B fails. B's when: failure
/// dependent runs; A's downstream completes; run exits non-zero.
#[tokio::test]
#[serial]
async fn test_parallel_branches_partial_failure_drains_all() -> Result<()> {
    let temp_dir = TempDir::new()?;
    let work_dir = PathBuf::from(temp_dir.path());
    setup_test_db(&work_dir);

    let a = Task::new(
        "a".to_string(),
        None,
        vec![],
        vec![],
        vec![],
        HashMap::new(),
        HashMap::new(),
        "echo a".to_string(),
    );
    let a_downstream = Task::new(
        "a-down".to_string(),
        None,
        vec![TaskEdge::new("a", When::Success)],
        vec![],
        vec![],
        HashMap::new(),
        HashMap::new(),
        "echo a-down".to_string(),
    );
    let b = Task::new(
        "b".to_string(),
        None,
        vec![],
        vec![],
        vec![],
        HashMap::new(),
        HashMap::new(),
        "exit 1".to_string(),
    );
    let b_fixer = Task::new(
        "b-fix".to_string(),
        None,
        vec![TaskEdge::new("b", When::Failure)],
        vec![],
        vec![],
        HashMap::new(),
        HashMap::new(),
        "echo b-fix".to_string(),
    );

    let workspace = Workspace::new(work_dir).await?;
    workspace.init().await?;
    let scheduler = TaskScheduler::new(
        vec![a, a_downstream, b, b_fixer],
        Arc::new(workspace),
        ExecutionContext::new(),
        4,
        false,
    )
    .await?;

    let result = timeout(Duration::from_secs(10), scheduler.execute_all()).await?;
    assert!(result.is_err(), "b's failure should surface as the run error");

    let statuses = scheduler.get_task_statuses().await;
    assert_eq!(statuses["a"], TaskStatus::Completed);
    assert_eq!(statuses["a-down"], TaskStatus::Completed);
    match &statuses["b"] {
        TaskStatus::Failed(_) => {}
        other => panic!("expected b Failed, got {:?}", other),
    }
    assert_eq!(statuses["b-fix"], TaskStatus::Completed);
    Ok(())
}
