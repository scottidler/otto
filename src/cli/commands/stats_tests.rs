#![cfg(test)]

use super::*;
use crate::executor::state::{RunMetadata, RunStatus, TaskStatus};
use crate::ports::MemoryStateStore;
use std::path::PathBuf;

fn create_test_store_with_data() -> Arc<MemoryStateStore> {
    let store = MemoryStateStore::new();

    // Add runs with tasks
    let metadata1 = RunMetadata::minimal(
        Some(PathBuf::from("/test/project1/otto.yml")),
        "abc123".to_string(),
        1700000000,
    );
    let run_id1 = store.record_run_start(&metadata1).unwrap();
    store
        .record_run_complete(run_id1, RunStatus::Success, Some(1024))
        .unwrap();

    let task_id1 = store
        .record_task_start(run_id1, "build", Some("hash1"), None, None, None)
        .unwrap();
    store.record_task_complete(task_id1, 0, TaskStatus::Completed).unwrap();

    let task_id2 = store
        .record_task_start(run_id1, "test", Some("hash2"), None, None, None)
        .unwrap();
    store.record_task_complete(task_id2, 0, TaskStatus::Completed).unwrap();

    // Second run - failed
    let metadata2 = RunMetadata::minimal(
        Some(PathBuf::from("/test/project1/otto.yml")),
        "abc123".to_string(),
        1700001000,
    );
    let run_id2 = store.record_run_start(&metadata2).unwrap();
    store
        .record_run_complete(run_id2, RunStatus::Failed, Some(2048))
        .unwrap();

    let task_id3 = store
        .record_task_start(run_id2, "build", Some("hash1"), None, None, None)
        .unwrap();
    store.record_task_complete(task_id3, 1, TaskStatus::Failed).unwrap();

    // Third run - different project
    let metadata3 = RunMetadata::minimal(
        Some(PathBuf::from("/test/project2/otto.yml")),
        "def456".to_string(),
        1700002000,
    );
    let run_id3 = store.record_run_start(&metadata3).unwrap();
    store
        .record_run_complete(run_id3, RunStatus::Success, Some(512))
        .unwrap();

    let task_id4 = store
        .record_task_start(run_id3, "deploy", Some("hash3"), None, None, None)
        .unwrap();
    store.record_task_complete(task_id4, 0, TaskStatus::Completed).unwrap();

    Arc::new(store)
}

fn create_empty_store() -> Arc<MemoryStateStore> {
    Arc::new(MemoryStateStore::new())
}

#[test]
fn test_format_duration() {
    assert_eq!(format_duration(0.5), "500ms");
    assert_eq!(format_duration(1.5), "1.5s");
    assert_eq!(format_duration(65.0), "1m5s");
    assert_eq!(format_duration(3665.0), "1h1m");
}

#[test]
fn test_format_size() {
    assert_eq!(format_size(500), "500 B");
    assert_eq!(format_size(1536), "1.5 KB");
    assert_eq!(format_size(1572864), "1.5 MB");
    assert_eq!(format_size(1610612736), "1.50 GB");
}

#[test]
fn test_format_percentage() {
    assert_eq!(format_percentage(75.5), "75.5%");
    assert_eq!(format_percentage(100.0), "100.0%");
    assert_eq!(format_percentage(0.0), "0.0%");
}

#[test]
fn test_format_task_status() {
    let completed = format_task_status(&TaskStatus::Completed);
    let failed = format_task_status(&TaskStatus::Failed);
    let running = format_task_status(&TaskStatus::Running);
    let skipped = format_task_status(&TaskStatus::Skipped);
    let pending = format_task_status(&TaskStatus::Pending);

    assert!(completed.contains("Completed"));
    assert!(failed.contains("Failed"));
    assert!(running.contains("Running"));
    assert!(skipped.contains("Skipped"));
    assert!(pending.contains("Pending"));
}

#[test]
fn test_execute_with_empty_store() {
    let store = create_empty_store();
    let cmd = StatsCommand {
        task_name: None,
        limit: 10,
        json: false,
    };

    let result = cmd.execute_with_store(Some(store));
    assert!(result.is_ok());
}

#[test]
fn test_execute_overall_stats() {
    let store = create_test_store_with_data();
    let cmd = StatsCommand {
        task_name: None,
        limit: 10,
        json: false,
    };

    let result = cmd.execute_with_store(Some(store));
    assert!(result.is_ok());
}

#[test]
fn test_execute_overall_stats_json() {
    let store = create_test_store_with_data();
    let cmd = StatsCommand {
        task_name: None,
        limit: 10,
        json: true,
    };

    let result = cmd.execute_with_store(Some(store));
    assert!(result.is_ok());
}

#[test]
fn test_execute_overall_stats_with_limit() {
    let store = create_test_store_with_data();
    let cmd = StatsCommand {
        task_name: None,
        limit: 1,
        json: false,
    };

    let result = cmd.execute_with_store(Some(store));
    assert!(result.is_ok());
}

#[test]
fn test_execute_task_stats() {
    let store = create_test_store_with_data();
    let cmd = StatsCommand {
        task_name: Some("build".to_string()),
        limit: 10,
        json: false,
    };

    let result = cmd.execute_with_store(Some(store));
    assert!(result.is_ok());
}

#[test]
fn test_execute_task_stats_json() {
    let store = create_test_store_with_data();
    let cmd = StatsCommand {
        task_name: Some("build".to_string()),
        limit: 10,
        json: true,
    };

    let result = cmd.execute_with_store(Some(store));
    assert!(result.is_ok());
}

#[test]
fn test_execute_task_stats_nonexistent() {
    let store = create_test_store_with_data();
    let cmd = StatsCommand {
        task_name: Some("nonexistent".to_string()),
        limit: 10,
        json: false,
    };

    let result = cmd.execute_with_store(Some(store));
    assert!(result.is_ok());
}

#[test]
fn test_execute_task_stats_empty_store() {
    let store = create_empty_store();
    let cmd = StatsCommand {
        task_name: Some("build".to_string()),
        limit: 10,
        json: false,
    };

    let result = cmd.execute_with_store(Some(store));
    assert!(result.is_ok());
}

#[test]
fn test_execute_task_stats_single_project() {
    let store = Arc::new(MemoryStateStore::new());

    // Only one project
    let metadata = RunMetadata::minimal(
        Some(PathBuf::from("/test/single/otto.yml")),
        "single123".to_string(),
        1700000000,
    );
    let run_id = store.record_run_start(&metadata).unwrap();
    store
        .record_run_complete(run_id, RunStatus::Success, Some(1024))
        .unwrap();

    let task_id = store
        .record_task_start(run_id, "build", Some("hash1"), None, None, None)
        .unwrap();
    store.record_task_complete(task_id, 0, TaskStatus::Completed).unwrap();

    let cmd = StatsCommand {
        task_name: Some("build".to_string()),
        limit: 10,
        json: false,
    };

    let result = cmd.execute_with_store(Some(store));
    assert!(result.is_ok());
}

#[test]
fn test_execute_task_stats_multiple_projects() {
    let store = create_test_store_with_data();
    let cmd = StatsCommand {
        task_name: Some("build".to_string()),
        limit: 10,
        json: false,
    };

    let result = cmd.execute_with_store(Some(store));
    assert!(result.is_ok());
}

#[test]
fn test_show_overall_stats_directly() {
    let store = create_test_store_with_data();
    let cmd = StatsCommand {
        task_name: None,
        limit: 10,
        json: false,
    };

    let result = cmd.show_overall_stats(store.as_ref());
    assert!(result.is_ok());
}

#[test]
fn test_show_task_stats_directly() {
    let store = create_test_store_with_data();
    let cmd = StatsCommand {
        task_name: None,
        limit: 10,
        json: false,
    };

    let result = cmd.show_task_stats(store.as_ref(), "build");
    assert!(result.is_ok());
}

#[test]
fn test_show_task_stats_deploy() {
    let store = create_test_store_with_data();
    let cmd = StatsCommand {
        task_name: None,
        limit: 10,
        json: false,
    };

    let result = cmd.show_task_stats(store.as_ref(), "deploy");
    assert!(result.is_ok());
}
