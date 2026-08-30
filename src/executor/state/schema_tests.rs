#![cfg(test)]

use super::*;
use rusqlite::Connection;

#[test]
fn test_init_schema() -> Result<()> {
    let conn = Connection::open_in_memory()?;
    init_schema(&conn)?;

    let mut stmt = conn.prepare("SELECT name FROM sqlite_master WHERE type='table' AND name='schema_version'")?;
    let exists = stmt.exists([])?;
    assert!(exists);

    let mut stmt = conn.prepare("SELECT name FROM sqlite_master WHERE type='table' AND name='projects'")?;
    let exists = stmt.exists([])?;
    assert!(exists);

    let mut stmt = conn.prepare("SELECT name FROM sqlite_master WHERE type='table' AND name='runs'")?;
    let exists = stmt.exists([])?;
    assert!(exists);

    let mut stmt = conn.prepare("SELECT name FROM sqlite_master WHERE type='table' AND name='tasks'")?;
    let exists = stmt.exists([])?;
    assert!(exists);

    Ok(())
}

#[test]
fn test_run_status_conversions() {
    assert_eq!(RunStatus::Running.as_str(), "running");
    assert_eq!(RunStatus::Success.as_str(), "success");
    assert_eq!(RunStatus::Failed.as_str(), "failed");

    assert_eq!(RunStatus::parse("running"), Some(RunStatus::Running));
    assert_eq!(RunStatus::parse("success"), Some(RunStatus::Success));
    assert_eq!(RunStatus::parse("failed"), Some(RunStatus::Failed));
    assert_eq!(RunStatus::parse("invalid"), None);
}

#[test]
fn test_task_status_conversions() {
    assert_eq!(TaskStatus::Pending.as_str(), "pending");
    assert_eq!(TaskStatus::Running.as_str(), "running");
    assert_eq!(TaskStatus::Completed.as_str(), "completed");
    assert_eq!(TaskStatus::Failed.as_str(), "failed");
    assert_eq!(TaskStatus::Skipped.as_str(), "skipped");

    assert_eq!(TaskStatus::parse("pending"), Some(TaskStatus::Pending));
    assert_eq!(TaskStatus::parse("running"), Some(TaskStatus::Running));
    assert_eq!(TaskStatus::parse("completed"), Some(TaskStatus::Completed));
    assert_eq!(TaskStatus::parse("failed"), Some(TaskStatus::Failed));
    assert_eq!(TaskStatus::parse("skipped"), Some(TaskStatus::Skipped));
    assert_eq!(TaskStatus::parse("invalid"), None);
}
