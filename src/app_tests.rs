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
        requested_tasks: vec![],
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
// Builtin param extraction
// =========================================================================
//
// Every builtin's params are derived from its clap `Command`
// (`cli/parser/meta_tasks.rs`), and a bound builtin task carries every
// derived param that has a default - `process_tasks_with_filter` Phase 3
// writes it whether or not the user typed the flag. So these build the values
// map the same way, and the extractors `expect` rather than substituting a
// second copy of the default.

/// The `values` map a bound task carries, from `(param, value)` pairs.
fn values(pairs: &[(&str, &str)]) -> HashMap<String, Value> {
    pairs
        .iter()
        .map(|(name, value)| (name.to_string(), Value::Item(value.to_string())))
        .collect()
}

/// What Phase 3 writes for a `Clean` invocation with no flags at all: the one
/// defaulted param, plus `false` for each flag.
fn bound_clean_values() -> HashMap<String, Value> {
    values(&[("keep-days", "30"), ("dry-run", "false"), ("no-db", "false")])
}

/// Same, for `History`.
fn bound_history_values() -> HashMap<String, Value> {
    values(&[("limit", "20"), ("json", "false")])
}

/// Same, for `Stats`.
fn bound_stats_values() -> HashMap<String, Value> {
    values(&[("limit", "10"), ("json", "false")])
}

// =========================================================================
// CleanParams Tests
// =========================================================================

#[test]
fn test_extract_clean_params_from_a_bound_task_with_no_flags() {
    let params = extract_clean_params(&bound_clean_values());
    assert_eq!(params.keep_days, 30);
    assert_eq!(params.keep_last, None);
    assert_eq!(params.keep_failed, None);
    assert!(!params.dry_run);
    assert_eq!(params.project_filter, None);
    assert!(!params.no_db);
}

#[test]
fn test_extract_clean_params_with_keep_days() {
    let mut values = bound_clean_values();
    values.insert("keep-days".to_string(), Value::Item("7".to_string()));
    let params = extract_clean_params(&values);
    assert_eq!(params.keep_days, 7);
}

/// The inverse of the old "falls back to 30" test: a value clap's `u64` parser
/// would never have bound is a bug in otto, and says so instead of silently
/// cleaning to a different depth than asked.
#[test]
#[should_panic(expected = "Clean's --keep-days is derived from CleanCommand's u64 arg")]
fn test_extract_clean_params_refuses_an_unparseable_keep_days() {
    let mut values = bound_clean_values();
    values.insert("keep-days".to_string(), Value::Item("invalid".to_string()));
    let _ = extract_clean_params(&values);
}

/// The inverse of the old "empty map is the default" test: `--keep-days` is
/// derived with a `default_value`, so an unbound task means the derivation or
/// the bind broke.
#[test]
#[should_panic(expected = "Clean's --keep-days is derived from its clap Command")]
fn test_extract_clean_params_refuses_an_unbound_task() {
    let _ = extract_clean_params(&HashMap::new());
}

#[test]
fn test_extract_clean_params_with_dry_run_true() {
    let mut values = bound_clean_values();
    values.insert("dry-run".to_string(), Value::Item("true".to_string()));
    let params = extract_clean_params(&values);
    assert!(params.dry_run);
}

#[test]
fn test_extract_clean_params_with_project_filter() {
    let mut values = bound_clean_values();
    values.insert("project-filter".to_string(), Value::Item("my-project".to_string()));
    let params = extract_clean_params(&values);
    assert_eq!(params.project_filter, Some("my-project".to_string()));
}

/// `--keep-last`, `--keep-failed` and `--no-db` exist on `CleanCommand` and
/// were missing from the hand-written meta task, so the task route dropped
/// them on the floor. Derived now, and read here.
#[test]
fn test_extract_clean_params_all_fields() {
    let params = extract_clean_params(&values(&[
        ("keep-days", "14"),
        ("keep-last", "3"),
        ("keep-failed", "90"),
        ("dry-run", "true"),
        ("project-filter", "test-project"),
        ("no-db", "true"),
    ]));
    assert_eq!(params.keep_days, 14);
    assert_eq!(params.keep_last, Some(3));
    assert_eq!(params.keep_failed, Some(90));
    assert!(params.dry_run);
    assert_eq!(params.project_filter, Some("test-project".to_string()));
    assert!(params.no_db);
}

// =========================================================================
// HistoryParams Tests
// =========================================================================

#[test]
fn test_extract_history_params_from_a_bound_task_with_no_flags() {
    let params = extract_history_params(&bound_history_values());
    assert_eq!(params.task_name, None);
    assert_eq!(params.limit, 20);
    assert_eq!(params.status, None);
    assert_eq!(params.project, None);
    assert!(!params.json);
}

/// `TASK` is positional on `HistoryCommand`, so the derived param is named
/// after the field (`task-name`); the meta task used to declare `-t|--task`,
/// a flag `otto History --help` never showed.
#[test]
fn test_extract_history_params_with_task() {
    let mut values = bound_history_values();
    values.insert("task-name".to_string(), Value::Item("build".to_string()));
    let params = extract_history_params(&values);
    assert_eq!(params.task_name, Some("build".to_string()));
}

#[test]
fn test_extract_history_params_with_limit() {
    let mut values = bound_history_values();
    values.insert("limit".to_string(), Value::Item("50".to_string()));
    let params = extract_history_params(&values);
    assert_eq!(params.limit, 50);
}

#[test]
#[should_panic(expected = "History's --limit is derived from HistoryCommand's usize arg")]
fn test_extract_history_params_refuses_an_unparseable_limit() {
    let mut values = bound_history_values();
    values.insert("limit".to_string(), Value::Item("not-a-number".to_string()));
    let _ = extract_history_params(&values);
}

#[test]
#[should_panic(expected = "History's --limit is derived from its clap Command")]
fn test_extract_history_params_refuses_an_unbound_task() {
    let _ = extract_history_params(&HashMap::new());
}

#[test]
fn test_extract_history_params_with_status() {
    let mut values = bound_history_values();
    values.insert("status".to_string(), Value::Item("failed".to_string()));
    let params = extract_history_params(&values);
    assert_eq!(params.status, Some("failed".to_string()));
}

#[test]
fn test_extract_history_params_with_project() {
    let mut values = bound_history_values();
    values.insert("project".to_string(), Value::Item("otto".to_string()));
    let params = extract_history_params(&values);
    assert_eq!(params.project, Some("otto".to_string()));
}

#[test]
fn test_extract_history_params_all_fields() {
    let params = extract_history_params(&values(&[
        ("task-name", "test"),
        ("limit", "100"),
        ("status", "failed"),
        ("project", "my-proj"),
        ("json", "true"),
    ]));
    assert_eq!(params.task_name, Some("test".to_string()));
    assert_eq!(params.limit, 100);
    assert_eq!(params.status, Some("failed".to_string()));
    assert_eq!(params.project, Some("my-proj".to_string()));
    assert!(params.json);
}

// =========================================================================
// StatsParams Tests
// =========================================================================

#[test]
fn test_extract_stats_params_from_a_bound_task_with_no_flags() {
    let params = extract_stats_params(&bound_stats_values());
    assert_eq!(params.task_name, None);
    assert_eq!(params.limit, 10);
    assert!(!params.json);
}

#[test]
fn test_extract_stats_params_with_task() {
    let mut values = bound_stats_values();
    values.insert("task-name".to_string(), Value::Item("lint".to_string()));
    let params = extract_stats_params(&values);
    assert_eq!(params.task_name, Some("lint".to_string()));
}

#[test]
fn test_extract_stats_params_with_limit() {
    let mut values = bound_stats_values();
    values.insert("limit".to_string(), Value::Item("25".to_string()));
    let params = extract_stats_params(&values);
    assert_eq!(params.limit, 25);
}

#[test]
#[should_panic(expected = "Stats' --limit is derived from StatsCommand's usize arg")]
fn test_extract_stats_params_refuses_an_unparseable_limit() {
    let mut values = bound_stats_values();
    values.insert("limit".to_string(), Value::Item("xyz".to_string()));
    let _ = extract_stats_params(&values);
}

#[test]
#[should_panic(expected = "Stats's --limit is derived from its clap Command")]
fn test_extract_stats_params_refuses_an_unbound_task() {
    let _ = extract_stats_params(&HashMap::new());
}

#[test]
fn test_extract_stats_params_with_json() {
    let mut values = bound_stats_values();
    values.insert("json".to_string(), Value::Item("true".to_string()));
    let params = extract_stats_params(&values);
    assert!(params.json);
}

#[test]
fn test_extract_stats_params_all_fields() {
    let params = extract_stats_params(&values(&[("task-name", "deploy"), ("limit", "5"), ("json", "true")]));
    assert_eq!(params.task_name, Some("deploy".to_string()));
    assert_eq!(params.limit, 5);
    assert!(params.json);
}

// =========================================================================
// GraphParams Tests
// =========================================================================

#[test]
fn test_extract_graph_params_defaults_to_ascii() {
    let params = extract_graph_params(&values(&[("format", "ascii")]));
    assert_eq!(params.format, GraphFormatArg::Ascii);
    assert_eq!(params.output, None);
}

#[test]
fn test_extract_graph_params_reads_format_and_output() {
    let params = extract_graph_params(&values(&[("format", "dot"), ("output", "dag.dot")]));
    assert_eq!(params.format, GraphFormatArg::Dot);
    assert_eq!(params.output, Some(PathBuf::from("dag.dot")));
}

/// otto binds `--format` with `ignore_case(true)`, so the value handed back is
/// the spelling the user typed.
#[test]
fn test_extract_graph_params_format_ignores_case() {
    let params = extract_graph_params(&values(&[("format", "PDF")]));
    assert_eq!(params.format, GraphFormatArg::Pdf);
}

/// The inverse of the old `_ => GraphFormat::Ascii` arm in the visualizer: an
/// unknown format was silently drawn as ascii.
#[test]
#[should_panic(expected = "Graph's --format is bound against GraphCommand's choices")]
fn test_extract_graph_params_refuses_an_unknown_format() {
    let _ = extract_graph_params(&values(&[("format", "mermaid")]));
}

#[test]
#[should_panic(expected = "Graph's --format is derived from its clap Command")]
fn test_extract_graph_params_refuses_an_unbound_task() {
    let _ = extract_graph_params(&HashMap::new());
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
fn a_builtin_is_found_by_position_not_by_being_alone() {
    // The lookup is positional, which is why a mixed list is rejected one
    // layer up in `Parser::parse` (Phase 5): here, `build` is simply dropped.
    // Pinned so the rejection is not quietly removed and this silent-drop
    // becomes reachable again.
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
    let a = extract_clean_params(&bound_clean_values());
    let b = extract_clean_params(&bound_clean_values());
    assert_eq!(a, b);
}

#[test]
fn test_history_params_equality() {
    let mut values = bound_history_values();
    values.insert("task-name".to_string(), Value::Item("test".to_string()));
    assert_eq!(extract_history_params(&values), extract_history_params(&values));
}

#[test]
fn test_params_clone() {
    let params = extract_clean_params(&values(&[
        ("keep-days", "7"),
        ("dry-run", "true"),
        ("project-filter", "proj"),
    ]));
    let cloned = params.clone();
    assert_eq!(params, cloned);
}

#[test]
fn test_params_debug() {
    let params = extract_stats_params(&bound_stats_values());
    let debug = format!("{:?}", params);
    assert!(debug.contains("StatsParams"));
    assert!(debug.contains("limit"));
}
