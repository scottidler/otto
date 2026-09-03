#![cfg(test)]

use super::*;
use crate::executor::state::{RunMetadata, TaskStatus};
use crate::ports::MemoryStateStore;
use std::path::PathBuf;

fn create_test_store_with_runs() -> Arc<MemoryStateStore> {
    let store = MemoryStateStore::new();

    // Add some test runs
    let metadata1 = RunMetadata::minimal(
        Some(PathBuf::from("/test/project1/otto.yml")),
        "abc123".to_string(),
        1700000000,
    );
    let run_id1 = store.record_run_start(&metadata1).unwrap();
    store
        .record_run_complete(run_id1, RunStatus::Success, Some(1024))
        .unwrap();

    // Add tasks to the run
    let task_id1 = store
        .record_task_start(run_id1, "build", Some("hash1"), None, None, None)
        .unwrap();
    store.record_task_complete(task_id1, 0, TaskStatus::Completed).unwrap();

    let task_id2 = store
        .record_task_start(run_id1, "test", Some("hash2"), None, None, None)
        .unwrap();
    store.record_task_complete(task_id2, 0, TaskStatus::Completed).unwrap();

    // Add a second run
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

    Arc::new(store)
}

fn create_empty_store() -> Arc<MemoryStateStore> {
    Arc::new(MemoryStateStore::new())
}

#[test]
fn an_absent_duration_or_size_renders_as_a_dash() {
    // `History` owns the Option handling; `format::format_duration` and
    // `format::format_size` now take concrete values.
    let duration: Option<f64> = None;
    let size: Option<u64> = None;
    assert_eq!(duration.map_or_else(|| "-".to_string(), format_duration), "-");
    assert_eq!(size.map_or_else(|| "-".to_string(), format_size), "-");
    assert_eq!(Some(65.0).map_or_else(|| "-".to_string(), format_duration), "1m5s");
    assert_eq!(Some(1536u64).map_or_else(|| "-".to_string(), format_size), "1.5 KB");
}

#[test]
#[serial_test::serial]
fn only_the_leading_home_prefix_becomes_a_tilde() {
    // The bug: `s.replace(&home, "~")` rewrote every occurrence, so a path
    // repeating the home prefix inside itself lost its interior segments.
    // `HOME` is mutated in place (matching `clean_tests.rs`'s pattern for
    // env-dependent tests) and restored on the way out, `#[serial]` because
    // Rust tests share a process and an unguarded `set_var` racing another
    // test's `HOME` read is a data race, not just a flaky assertion.
    let original = std::env::var("HOME").ok();
    unsafe {
        std::env::set_var("HOME", "/home/u");
    }

    assert_eq!(abbreviate_home("/home/u/proj/home/u/x"), "~/proj/home/u/x");
    assert_eq!(abbreviate_home("/home/u"), "~");
    assert_eq!(abbreviate_home("/var/tmp/home/u"), "/var/tmp/home/u");

    unsafe {
        match &original {
            Some(v) => std::env::set_var("HOME", v),
            None => std::env::remove_var("HOME"),
        }
    }
}

#[test]
fn test_format_run_status() {
    let success = format_run_status(&RunStatus::Success);
    let failed = format_run_status(&RunStatus::Failed);
    let running = format_run_status(&RunStatus::Running);

    assert!(success.contains("✓") || success.contains("green"));
    assert!(failed.contains("✗") || failed.contains("red"));
    assert!(running.contains("⋯") || running.contains("yellow"));
}

#[test]
fn test_format_task_status() {
    let completed = format_task_status(&TaskStatus::Completed);
    let failed = format_task_status(&TaskStatus::Failed);
    let running = format_task_status(&TaskStatus::Running);
    let skipped = format_task_status(&TaskStatus::Skipped);
    let pending = format_task_status(&TaskStatus::Pending);

    assert!(!completed.is_empty());
    assert!(!failed.is_empty());
    assert!(!running.is_empty());
    assert!(!skipped.is_empty());
    assert!(!pending.is_empty());
}

#[test]
fn test_display_width() {
    assert_eq!(display_width("hello"), 5);
    assert_eq!(display_width(""), 0);
    assert_eq!(display_width("test string"), 11);
}

#[test]
fn test_pad_left() {
    assert_eq!(pad_left("hi", 5), "hi   ");
    assert_eq!(pad_left("hello", 5), "hello");
    assert_eq!(pad_left("toolong", 3), "toolong");
}

#[test]
fn test_pad_right() {
    assert_eq!(pad_right("hi", 5), "   hi");
    assert_eq!(pad_right("hello", 5), "hello");
    assert_eq!(pad_right("toolong", 3), "toolong");
}

#[test]
fn test_pad_center() {
    assert_eq!(pad_center("hi", 6), "  hi  ");
    assert_eq!(pad_center("hello", 5), "hello");
    assert_eq!(pad_center("x", 4), " x  ");
}

#[test]
fn test_execute_with_empty_store() {
    let store = create_empty_store();
    let cmd = HistoryCommand {
        task_name: None,
        limit: 20,
        status: None,
        project: None,
        json: false,
    };

    let result = cmd.execute_with_store(Some(store));
    assert!(result.is_ok());
}

#[test]
fn test_execute_with_runs() {
    let store = create_test_store_with_runs();
    let cmd = HistoryCommand {
        task_name: None,
        limit: 20,
        status: None,
        project: None,
        json: false,
    };

    let result = cmd.execute_with_store(Some(store));
    assert!(result.is_ok());
}

#[test]
fn test_execute_with_json_output() {
    let store = create_test_store_with_runs();
    let cmd = HistoryCommand {
        task_name: None,
        limit: 20,
        status: None,
        project: None,
        json: true,
    };

    let result = cmd.execute_with_store(Some(store));
    assert!(result.is_ok());
}

#[test]
fn test_execute_with_status_filter_success() {
    let store = create_test_store_with_runs();
    let cmd = HistoryCommand {
        task_name: None,
        limit: 20,
        status: Some(StatusFilter::Success),
        project: None,
        json: false,
    };

    let result = cmd.execute_with_store(Some(store));
    assert!(result.is_ok());
}

#[test]
fn test_execute_with_status_filter_failed() {
    let store = create_test_store_with_runs();
    let cmd = HistoryCommand {
        task_name: None,
        limit: 20,
        status: Some(StatusFilter::Failed),
        project: None,
        json: false,
    };

    let result = cmd.execute_with_store(Some(store));
    assert!(result.is_ok());
}

#[test]
fn test_execute_with_status_filter_running() {
    let store = create_test_store_with_runs();
    let cmd = HistoryCommand {
        task_name: None,
        limit: 20,
        status: Some(StatusFilter::Running),
        project: None,
        json: false,
    };

    let result = cmd.execute_with_store(Some(store));
    assert!(result.is_ok());
}

/// Was `test_execute_with_invalid_status_filter`, which asserted that
/// `--status invalid` was accepted and quietly ignored. That is the defect;
/// the assertion is inverted rather than deleted.
#[test]
fn an_invalid_status_is_rejected_at_parse_time() {
    use clap::Parser as _;

    let err = HistoryCommand::try_parse_from(["history", "--status", "invalid"])
        .expect_err("'invalid' is not a status")
        .to_string();
    assert!(err.contains("invalid value 'invalid'"), "{err}");
}

#[test]
fn a_status_is_accepted_in_any_case() {
    use clap::Parser as _;

    let cmd = HistoryCommand::try_parse_from(["history", "--status", "FAILED"]).expect("case does not matter");
    assert_eq!(cmd.status, Some(StatusFilter::Failed));
}

#[test]
fn status_filter_parse_names_the_accepted_values() {
    assert_eq!(StatusFilter::parse("Running").unwrap(), StatusFilter::Running);
    let err = StatusFilter::parse("bogus")
        .expect_err("bogus is not a status")
        .to_string();
    assert!(err.contains("success, failed, running"), "{err}");
}

#[test]
fn test_execute_with_project_filter() {
    let store = create_test_store_with_runs();
    let cmd = HistoryCommand {
        task_name: None,
        limit: 20,
        status: None,
        project: Some("abc123".to_string()),
        json: false,
    };

    let result = cmd.execute_with_store(Some(store));
    assert!(result.is_ok());
}

#[test]
fn test_execute_with_limit() {
    let store = create_test_store_with_runs();
    let cmd = HistoryCommand {
        task_name: None,
        limit: 1,
        status: None,
        project: None,
        json: false,
    };

    let result = cmd.execute_with_store(Some(store));
    assert!(result.is_ok());
}

#[test]
fn test_execute_task_history() {
    let store = create_test_store_with_runs();
    let cmd = HistoryCommand {
        task_name: Some("build".to_string()),
        limit: 20,
        status: None,
        project: None,
        json: false,
    };

    let result = cmd.execute_with_store(Some(store));
    assert!(result.is_ok());
}

#[test]
fn test_execute_task_history_json() {
    let store = create_test_store_with_runs();
    let cmd = HistoryCommand {
        task_name: Some("build".to_string()),
        limit: 20,
        status: None,
        project: None,
        json: true,
    };

    let result = cmd.execute_with_store(Some(store));
    assert!(result.is_ok());
}

#[test]
fn test_execute_task_history_nonexistent() {
    let store = create_test_store_with_runs();
    let cmd = HistoryCommand {
        task_name: Some("nonexistent".to_string()),
        limit: 20,
        status: None,
        project: None,
        json: false,
    };

    let result = cmd.execute_with_store(Some(store));
    assert!(result.is_ok());
}

#[test]
fn test_execute_task_history_empty_store() {
    let store = create_empty_store();
    let cmd = HistoryCommand {
        task_name: Some("build".to_string()),
        limit: 20,
        status: None,
        project: None,
        json: false,
    };

    let result = cmd.execute_with_store(Some(store));
    assert!(result.is_ok());
}

#[test]
fn test_show_run_history_directly() {
    let store = create_test_store_with_runs();
    let cmd = HistoryCommand {
        task_name: None,
        limit: 20,
        status: None,
        project: None,
        json: false,
    };

    let result = cmd.show_run_history(store.as_ref());
    assert!(result.is_ok());
}

#[test]
fn test_show_task_history_directly() {
    let store = create_test_store_with_runs();
    let cmd = HistoryCommand {
        task_name: None,
        limit: 20,
        status: None,
        project: None,
        json: false,
    };

    let result = cmd.show_task_history(store.as_ref(), "build");
    assert!(result.is_ok());
}
