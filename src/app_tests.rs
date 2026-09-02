#![cfg(test)]

use super::*;

#[test]
fn test_runtime_config_fields() {
    let config = RuntimeConfig {
        tasks: vec![],
        hash: "abc123".to_string(),
        ottofile_path: Some(PathBuf::from("/tmp/otto.yml")),
        jobs: 4,
        tui_mode: false,
        no_prefix: false,
        retention: crate::cfg::otto::RetentionSpec::default(),
    };

    assert_eq!(config.tasks.len(), 0);
    assert_eq!(config.hash, "abc123");
    assert_eq!(config.ottofile_path, Some(PathBuf::from("/tmp/otto.yml")));
    assert_eq!(config.jobs, 4);
    assert!(!config.tui_mode);
    assert!(!config.no_prefix);
    assert_eq!(config.retention, crate::cfg::otto::RetentionSpec::default());
}

// =========================================================================
// CleanParams Tests
// =========================================================================

#[test]
fn test_clean_params_default() {
    let params = CleanParams::default();
    assert_eq!(params.keep_days, 30);
    assert!(!params.dry_run);
    assert_eq!(params.project_filter, None);
}

#[test]
fn test_extract_clean_params_empty() {
    let values = HashMap::new();
    let params = extract_clean_params(&values);
    assert_eq!(params, CleanParams::default());
}

#[test]
fn test_extract_clean_params_with_keep_days() {
    let mut values = HashMap::new();
    values.insert("keep".to_string(), Value::Item("7".to_string()));
    let params = extract_clean_params(&values);
    assert_eq!(params.keep_days, 7);
}

#[test]
fn test_extract_clean_params_with_invalid_keep_days() {
    let mut values = HashMap::new();
    values.insert("keep".to_string(), Value::Item("invalid".to_string()));
    let params = extract_clean_params(&values);
    assert_eq!(params.keep_days, 30); // Falls back to default
}

#[test]
fn test_extract_clean_params_with_dry_run_true() {
    let mut values = HashMap::new();
    values.insert("dry-run".to_string(), Value::Item("true".to_string()));
    let params = extract_clean_params(&values);
    assert!(params.dry_run);
}

#[test]
fn test_extract_clean_params_with_dry_run_false() {
    let mut values = HashMap::new();
    values.insert("dry-run".to_string(), Value::Item("false".to_string()));
    let params = extract_clean_params(&values);
    assert!(!params.dry_run);
}

#[test]
fn test_extract_clean_params_with_project() {
    let mut values = HashMap::new();
    values.insert("project".to_string(), Value::Item("my-project".to_string()));
    let params = extract_clean_params(&values);
    assert_eq!(params.project_filter, Some("my-project".to_string()));
}

#[test]
fn test_extract_clean_params_all_fields() {
    let mut values = HashMap::new();
    values.insert("keep".to_string(), Value::Item("14".to_string()));
    values.insert("dry-run".to_string(), Value::Item("true".to_string()));
    values.insert("project".to_string(), Value::Item("test-project".to_string()));
    let params = extract_clean_params(&values);
    assert_eq!(params.keep_days, 14);
    assert!(params.dry_run);
    assert_eq!(params.project_filter, Some("test-project".to_string()));
}

// =========================================================================
// HistoryParams Tests
// =========================================================================

#[test]
fn test_history_params_default() {
    let params = HistoryParams::default();
    assert_eq!(params.task_name, None);
    assert_eq!(params.limit, 20);
    assert_eq!(params.status, None);
    assert_eq!(params.project, None);
    assert!(!params.json);
}

#[test]
fn test_extract_history_params_empty() {
    let values = HashMap::new();
    let params = extract_history_params(&values);
    assert_eq!(params, HistoryParams::default());
}

#[test]
fn test_extract_history_params_with_task() {
    let mut values = HashMap::new();
    values.insert("task".to_string(), Value::Item("build".to_string()));
    let params = extract_history_params(&values);
    assert_eq!(params.task_name, Some("build".to_string()));
}

#[test]
fn test_extract_history_params_with_limit() {
    let mut values = HashMap::new();
    values.insert("limit".to_string(), Value::Item("50".to_string()));
    let params = extract_history_params(&values);
    assert_eq!(params.limit, 50);
}

#[test]
fn test_extract_history_params_with_invalid_limit() {
    let mut values = HashMap::new();
    values.insert("limit".to_string(), Value::Item("not-a-number".to_string()));
    let params = extract_history_params(&values);
    assert_eq!(params.limit, 20); // Falls back to default
}

#[test]
fn test_extract_history_params_with_status() {
    let mut values = HashMap::new();
    values.insert("status".to_string(), Value::Item("failed".to_string()));
    let params = extract_history_params(&values);
    assert_eq!(params.status, Some("failed".to_string()));
}

#[test]
fn test_extract_history_params_with_project() {
    let mut values = HashMap::new();
    values.insert("project".to_string(), Value::Item("otto".to_string()));
    let params = extract_history_params(&values);
    assert_eq!(params.project, Some("otto".to_string()));
}

#[test]
fn test_extract_history_params_with_json() {
    let mut values = HashMap::new();
    values.insert("json".to_string(), Value::Item("true".to_string()));
    let params = extract_history_params(&values);
    assert!(params.json);
}

#[test]
fn test_extract_history_params_all_fields() {
    let mut values = HashMap::new();
    values.insert("task".to_string(), Value::Item("test".to_string()));
    values.insert("limit".to_string(), Value::Item("100".to_string()));
    values.insert("status".to_string(), Value::Item("passed".to_string()));
    values.insert("project".to_string(), Value::Item("my-proj".to_string()));
    values.insert("json".to_string(), Value::Item("true".to_string()));
    let params = extract_history_params(&values);
    assert_eq!(params.task_name, Some("test".to_string()));
    assert_eq!(params.limit, 100);
    assert_eq!(params.status, Some("passed".to_string()));
    assert_eq!(params.project, Some("my-proj".to_string()));
    assert!(params.json);
}

// =========================================================================
// StatsParams Tests
// =========================================================================

#[test]
fn test_stats_params_default() {
    let params = StatsParams::default();
    assert_eq!(params.task_name, None);
    assert_eq!(params.limit, 10);
    assert!(!params.json);
}

#[test]
fn test_extract_stats_params_empty() {
    let values = HashMap::new();
    let params = extract_stats_params(&values);
    assert_eq!(params, StatsParams::default());
}

#[test]
fn test_extract_stats_params_with_task() {
    let mut values = HashMap::new();
    values.insert("task".to_string(), Value::Item("lint".to_string()));
    let params = extract_stats_params(&values);
    assert_eq!(params.task_name, Some("lint".to_string()));
}

#[test]
fn test_extract_stats_params_with_limit() {
    let mut values = HashMap::new();
    values.insert("limit".to_string(), Value::Item("25".to_string()));
    let params = extract_stats_params(&values);
    assert_eq!(params.limit, 25);
}

#[test]
fn test_extract_stats_params_with_invalid_limit() {
    let mut values = HashMap::new();
    values.insert("limit".to_string(), Value::Item("xyz".to_string()));
    let params = extract_stats_params(&values);
    assert_eq!(params.limit, 10); // Falls back to default
}

#[test]
fn test_extract_stats_params_with_json() {
    let mut values = HashMap::new();
    values.insert("json".to_string(), Value::Item("true".to_string()));
    let params = extract_stats_params(&values);
    assert!(params.json);
}

#[test]
fn test_extract_stats_params_all_fields() {
    let mut values = HashMap::new();
    values.insert("task".to_string(), Value::Item("deploy".to_string()));
    values.insert("limit".to_string(), Value::Item("5".to_string()));
    values.insert("json".to_string(), Value::Item("true".to_string()));
    let params = extract_stats_params(&values);
    assert_eq!(params.task_name, Some("deploy".to_string()));
    assert_eq!(params.limit, 5);
    assert!(params.json);
}

// =========================================================================
// Task Filtering Tests
// =========================================================================

fn create_test_task(name: &str) -> Task {
    Task {
        name: name.to_string(),
        task_deps: vec![],
        file_deps: vec![],
        output_deps: vec![],
        envs: HashMap::new(),
        values: HashMap::new(),
        action: String::new(),
        hash: String::new(),
        is_virtual_parent: false,
        serial_group: None,
        serial_index: 0,
        tty: false,
        foreach_display_order: None,
        buffered: false,
        foreach_jobs: None,
    }
}

// =========================================================================
// Builtin dispatch parity (Phase 8)
// =========================================================================

#[test]
fn every_builtin_is_dispatchable_by_name() {
    for builtin in Builtin::all() {
        let tasks = vec![create_test_task(builtin.task_name())];
        let found = find_builtin(&tasks).expect("the builtin is in the dispatch table");
        assert_eq!(found.0, builtin);
        assert_eq!(found.1.name, builtin.task_name());
    }
}

#[test]
fn the_dispatch_table_covers_every_registered_builtin() {
    // The two lists must agree, or a builtin the parser injects has no
    // handler and its invocation prints "No tasks to execute".
    let dispatchable: Vec<&str> = Builtin::all().iter().map(|b| b.task_name()).collect();
    for name in crate::cli::BUILTIN_COMMANDS {
        assert!(dispatchable.contains(name), "builtin '{name}' has no dispatch entry");
    }
    assert_eq!(dispatchable.len(), crate::cli::BUILTIN_COMMANDS.len());
}

#[test]
fn a_plain_task_list_dispatches_no_builtin() {
    let tasks = vec![create_test_task("build"), create_test_task("test")];
    assert!(find_builtin(&tasks).is_none());
}

#[test]
fn a_builtin_is_found_alongside_ordinary_tasks() {
    // This is the TUI/terminal parity case: both paths ask this one
    // question, so `otto --tui Graph` and `otto Graph` resolve alike.
    let tasks = vec![create_test_task("build"), create_test_task("Graph")];
    let (builtin, task) = find_builtin(&tasks).expect("Graph is a builtin");
    assert_eq!(builtin, Builtin::Graph);
    assert_eq!(task.name, "Graph");
}

#[test]
fn build_executor_tasks_carries_every_task_across() {
    let tasks = vec![create_test_task("build"), create_test_task("test")];
    let executor_tasks = build_executor_tasks(tasks);
    let names: Vec<&str> = executor_tasks.iter().map(|t| t.name.as_str()).collect();
    assert_eq!(names, vec!["build", "test"]);
}

#[test]
fn build_executor_tasks_preserves_the_virtual_parent_flag() {
    let mut parent = create_test_task("up");
    parent.is_virtual_parent = true;
    let executor_tasks = build_executor_tasks(vec![parent]);
    assert!(executor_tasks[0].is_virtual_parent);
}

// =========================================================================
// Convert / Upgrade param extraction
// =========================================================================

#[test]
fn extract_convert_params_defaults_to_stdout_and_lenient() {
    let params = extract_convert_params(&HashMap::new());
    assert_eq!(params, ConvertParams::default());
    assert!(!params.strict);
    assert_eq!(params.output, None);
}

#[test]
fn extract_convert_params_reads_strict_and_output() {
    let mut values = HashMap::new();
    values.insert("strict".to_string(), Value::Item("true".to_string()));
    values.insert("output".to_string(), Value::Item("out.yml".to_string()));
    let params = extract_convert_params(&values);
    assert!(params.strict);
    assert_eq!(params.output, Some(PathBuf::from("out.yml")));
}

#[test]
fn extract_upgrade_params_defaults_to_a_plain_upgrade() {
    let params = extract_upgrade_params(&HashMap::new());
    assert_eq!(params, UpgradeParams::default());
}

#[test]
fn extract_upgrade_params_reads_every_flag() {
    let mut values = HashMap::new();
    for name in ["dry-run", "list-versions", "rollback", "force", "no-backup"] {
        values.insert(name.to_string(), Value::Item("true".to_string()));
    }
    values.insert("version".to_string(), Value::Item("1.2.3".to_string()));
    let params = extract_upgrade_params(&values);
    assert!(params.dry_run);
    assert!(params.list_versions);
    assert!(params.rollback);
    assert!(params.force);
    assert!(params.no_backup);
    assert_eq!(params.version.as_deref(), Some("1.2.3"));
}

#[test]
fn a_flag_value_that_is_not_true_reads_as_false() {
    let mut values = HashMap::new();
    values.insert("force".to_string(), Value::Item("false".to_string()));
    assert!(!extract_upgrade_params(&values).force);
}

#[test]
fn test_filter_execution_tasks_empty() {
    let tasks: Vec<Task> = vec![];
    let filtered = filter_execution_tasks(tasks);
    assert!(filtered.is_empty());
}

#[test]
fn test_filter_execution_tasks_removes_builtins() {
    let tasks = vec![
        create_test_task("build"),
        create_test_task("Clean"),
        create_test_task("test"),
        create_test_task("History"),
        create_test_task("deploy"),
    ];
    let filtered = filter_execution_tasks(tasks);
    assert_eq!(filtered.len(), 3);
    assert_eq!(filtered[0].name, "build");
    assert_eq!(filtered[1].name, "test");
    assert_eq!(filtered[2].name, "deploy");
}

#[test]
fn test_filter_execution_tasks_no_builtins() {
    let tasks = vec![
        create_test_task("build"),
        create_test_task("test"),
        create_test_task("lint"),
    ];
    let filtered = filter_execution_tasks(tasks);
    assert_eq!(filtered.len(), 3);
}

#[test]
fn test_filter_execution_tasks_all_builtins() {
    let tasks = vec![
        create_test_task("Clean"),
        create_test_task("History"),
        create_test_task("Stats"),
        create_test_task("Graph"),
    ];
    let filtered = filter_execution_tasks(tasks);
    assert!(filtered.is_empty());
}

#[test]
fn test_find_tasks_by_name_empty() {
    let tasks: Vec<Task> = vec![];
    let found = find_tasks_by_name(&tasks, "build");
    assert!(found.is_empty());
}

#[test]
fn test_find_tasks_by_name_found() {
    let tasks = vec![
        create_test_task("build"),
        create_test_task("test"),
        create_test_task("build"), // Duplicate
    ];
    let found = find_tasks_by_name(&tasks, "build");
    assert_eq!(found.len(), 2);
}

#[test]
fn test_find_tasks_by_name_not_found() {
    let tasks = vec![create_test_task("build"), create_test_task("test")];
    let found = find_tasks_by_name(&tasks, "deploy");
    assert!(found.is_empty());
}

#[test]
fn test_find_tasks_by_name_case_sensitive() {
    let tasks = vec![create_test_task("Build"), create_test_task("build")];
    let found = find_tasks_by_name(&tasks, "build");
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].name, "build");
}

// =========================================================================
// Integration Tests for Param Structs
// =========================================================================

#[test]
fn test_clean_params_equality() {
    let a = CleanParams {
        keep_days: 30,
        dry_run: false,
        project_filter: None,
    };
    let b = CleanParams::default();
    assert_eq!(a, b);
}

#[test]
fn test_history_params_equality() {
    let a = HistoryParams {
        task_name: Some("test".to_string()),
        limit: 20,
        status: None,
        project: None,
        json: false,
    };
    let b = HistoryParams {
        task_name: Some("test".to_string()),
        ..Default::default()
    };
    assert_eq!(a, b);
}

#[test]
fn test_stats_params_equality() {
    let a = StatsParams {
        task_name: None,
        limit: 10,
        json: false,
    };
    let b = StatsParams::default();
    assert_eq!(a, b);
}

#[test]
fn test_params_clone() {
    let params = CleanParams {
        keep_days: 7,
        dry_run: true,
        project_filter: Some("proj".to_string()),
    };
    let cloned = params.clone();
    assert_eq!(params, cloned);
}

#[test]
fn test_params_debug() {
    let params = StatsParams::default();
    let debug = format!("{:?}", params);
    assert!(debug.contains("StatsParams"));
    assert!(debug.contains("limit"));
}
