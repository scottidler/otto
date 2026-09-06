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

/// AC4: `otto Stats --json` carries the per-task table the text output prints.
#[test]
fn test_execute_overall_stats_json() {
    let store = create_test_store_with_data();
    let cmd = StatsCommand {
        task_name: None,
        limit: 10,
        json: true,
    };

    let result = cmd.execute_with_store(Some(Arc::clone(&store) as Arc<dyn StateStore>));
    assert!(result.is_ok());

    let payload = overall_json_value(store.as_ref(), 10);
    let tasks = payload["tasks"].as_array().expect("the payload has a tasks array");
    assert!(!tasks.is_empty(), "tasks must not be empty, got:\n{payload:#}");
    assert!(
        payload["total_runs"].is_number(),
        "jq '.total_runs' must still resolve at the top level, got:\n{payload:#}"
    );
}

/// The `tasks` key is additive: the seven overall keys keep their names, their
/// order, and their number formatting, byte for byte, so `jq '.total_runs'` in
/// the wild keeps working.
#[test]
fn overall_json_leaves_the_seven_existing_keys_unmoved() {
    let store = create_test_store_with_data();
    let stats = store.get_overall_stats().unwrap();
    let task_stats = store.get_all_task_stats(Some(10)).unwrap();

    let before = serde_json::to_string_pretty(&stats).unwrap();
    let after = render_overall_json(&stats, &task_stats).unwrap();

    // Everything up to the closing brace of the old object, verbatim.
    let unmoved = before.strip_suffix("\n}").expect("pretty JSON object ends in a brace");
    assert!(
        after.starts_with(unmoved),
        "the seven pairs must serialize identically and in the same order\nbefore:\n{before}\nafter:\n{after}"
    );
    assert_eq!(
        after[unmoved.len()..].lines().next(),
        Some(","),
        "the seven pairs must be followed only by a comma and the new key, got:\n{after}"
    );

    // Read the key order off the text, not off a parsed map: serde_json's map is
    // sorted unless `preserve_order` is on, which would hide a reordering.
    let top_level_keys: Vec<&str> = after
        .lines()
        .filter_map(|line| line.strip_prefix("  \""))
        .filter_map(|line| line.split('"').next())
        .collect();
    assert_eq!(
        top_level_keys,
        vec![
            "total_runs",
            "successful_runs",
            "failed_runs",
            "running_runs",
            "total_tasks",
            "total_disk_usage",
            "total_duration_seconds",
            "tasks",
        ],
        "tasks is appended eighth, nothing is nested"
    );
}

/// `--limit` reached the text path only: the JSON branch returned before the
/// per-task fetch, so the flag was unreachable there.
#[test]
fn overall_json_honors_the_limit() {
    let store = create_test_store_with_data();
    let unlimited = overall_json_value(store.as_ref(), 10);
    assert!(
        unlimited["tasks"].as_array().unwrap().len() > 1,
        "the fixture store must hold more than one task row to make the limit observable"
    );

    let limited = overall_json_value(store.as_ref(), 1);
    assert_eq!(
        limited["tasks"].as_array().unwrap().len(),
        1,
        "-n 1 must cap the JSON tasks array, got:\n{limited:#}"
    );
}

#[test]
fn overall_json_on_an_empty_store_still_has_a_tasks_key() {
    let store = create_empty_store();
    let payload = overall_json_value(store.as_ref(), 10);

    assert_eq!(
        payload["tasks"].as_array().map(|t| t.len()),
        Some(0),
        "an empty store gets an empty array, not a missing key, got:\n{payload:#}"
    );
}

/// The payload `show_overall_stats` emits on the JSON path, parsed.
fn overall_json_value(store: &dyn StateStore, limit: usize) -> serde_json::Value {
    let stats = store.get_overall_stats().unwrap();
    let task_stats = store.get_all_task_stats(Some(limit)).unwrap();
    serde_json::from_str(&render_overall_json(&stats, &task_stats).unwrap()).unwrap()
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

/// AC7: one denominator. A store carrying eight runs that are still `Running`
/// must not drag the overall rate below the per-task rate computed from the same
/// rows: 1 success + 1 failure is 50.0% in both tables, not 10.0% in one of them.
fn create_store_with_running_runs() -> Arc<MemoryStateStore> {
    let store = MemoryStateStore::new();
    let ottofile = PathBuf::from("/test/inflight/otto.yml");

    let succeeded = store
        .record_run_start(&RunMetadata::minimal(
            Some(ottofile.clone()),
            "inflight".to_string(),
            1700000000,
        ))
        .unwrap();
    store
        .record_run_complete(succeeded, RunStatus::Success, Some(1024))
        .unwrap();
    let task_id = store
        .record_task_start(succeeded, "build", Some("hash1"), None, None, None)
        .unwrap();
    store.record_task_complete(task_id, 0, TaskStatus::Completed).unwrap();

    let failed = store
        .record_run_start(&RunMetadata::minimal(
            Some(ottofile.clone()),
            "inflight".to_string(),
            1700000100,
        ))
        .unwrap();
    store
        .record_run_complete(failed, RunStatus::Failed, Some(1024))
        .unwrap();
    let task_id = store
        .record_task_start(failed, "build", Some("hash1"), None, None, None)
        .unwrap();
    store.record_task_complete(task_id, 1, TaskStatus::Failed).unwrap();

    // Eight runs left mid-flight, the shape SIGKILL leaves behind.
    for offset in 0..8 {
        let run_id = store
            .record_run_start(&RunMetadata::minimal(
                Some(ottofile.clone()),
                "inflight".to_string(),
                1700000200 + offset,
            ))
            .unwrap();
        store
            .record_task_start(run_id, "build", Some("hash1"), None, None, None)
            .unwrap();
    }

    Arc::new(store)
}

#[test]
fn overall_success_rate_ignores_running_runs() {
    let store = create_store_with_running_runs();
    let stats = store.get_overall_stats().unwrap();

    assert_eq!(stats.total_runs, 10);
    assert_eq!(stats.running_runs, 8);

    let rendered = render_overall_table(&stats).to_string();
    assert!(
        rendered.contains("1 (50.0%)"),
        "overall table must divide by terminal runs, got:\n{rendered}"
    );
    assert!(
        !rendered.contains("10.0%"),
        "overall table must not divide by total runs, got:\n{rendered}"
    );
    assert!(
        rendered.contains("Running"),
        "the in-flight population stays visible, got:\n{rendered}"
    );
}

#[test]
fn per_task_table_shares_the_overall_denominator() {
    let store = create_store_with_running_runs();
    let overall = render_overall_table(&store.get_overall_stats().unwrap()).to_string();
    let per_task = render_task_stats_table(&store.get_all_task_stats(Some(10)).unwrap()).to_string();

    assert!(
        per_task.contains("50.0%"),
        "per-task table must render 50.0% for 1 success and 1 failure, got:\n{per_task}"
    );
    assert!(
        overall.contains("50.0%") && per_task.contains("50.0%"),
        "both tables render the same rate off the same rows:\noverall:\n{overall}\nper task:\n{per_task}"
    );
}

#[test]
fn test_format_success_rate() {
    assert_eq!(format_success_rate(1, 1), "50.0%");
    assert_eq!(format_success_rate(3, 0), "100.0%");
    assert_eq!(format_success_rate(0, 2), "0.0%");
}

#[test]
fn format_success_rate_with_nothing_terminal_is_not_zero_percent() {
    assert_eq!(format_success_rate(0, 0), "n/a");
}
