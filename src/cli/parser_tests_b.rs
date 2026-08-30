#![cfg(test)]

use super::*;

#[test]
fn test_foreach_subtasks_not_chained_when_parallel_true() {
    use std::fs;
    use tempfile::TempDir;

    let temp_dir = TempDir::new().unwrap();
    let ottofile_path = temp_dir.path().join("otto.yml");

    // Create an ottofile with parallel: true (explicit, same as default)
    let config = r#"
tasks:
  install:
    foreach:
      items: [a, b, c]
      as: pkg
      parallel: true
    bash: echo ${pkg}
"#;
    fs::write(&ottofile_path, config).unwrap();

    let args = vec![
        "otto".to_string(),
        "--ottofile".to_string(),
        ottofile_path.to_string_lossy().to_string(),
        "install".to_string(),
    ];

    let mut parser = Parser::new(args).unwrap();
    let result = parser.parse().unwrap();
    let (tasks, _, _, _, _, _) = result.into_run().unwrap().into_parts();

    // Find the subtasks
    let subtask_a = tasks.iter().find(|t| t.name == "install:a");
    let subtask_b = tasks.iter().find(|t| t.name == "install:b");
    let subtask_c = tasks.iter().find(|t| t.name == "install:c");

    assert!(subtask_a.is_some(), "subtask install:a should exist");
    assert!(subtask_b.is_some(), "subtask install:b should exist");
    assert!(subtask_c.is_some(), "subtask install:c should exist");

    // With parallel: true, subtasks should NOT be chained
    let b = subtask_b.unwrap();
    let c = subtask_c.unwrap();

    // b should NOT depend on a, c should NOT depend on b
    assert!(
        !b.task_deps.iter().any(|d| d.task == "install:a"),
        "install:b should NOT depend on install:a when parallel: true, got: {:?}",
        b.task_deps
    );
    assert!(
        !c.task_deps.iter().any(|d| d.task == "install:b"),
        "install:c should NOT depend on install:b when parallel: true, got: {:?}",
        c.task_deps
    );

    // ...and they join no serial group, so the scheduler's ordering gate is inert.
    assert_eq!(b.serial_group, None);
    assert_eq!(c.serial_group, None);
}

#[test]
fn test_foreach_subtasks_parallel_by_default() {
    use std::fs;
    use tempfile::TempDir;

    let temp_dir = TempDir::new().unwrap();
    let ottofile_path = temp_dir.path().join("otto.yml");

    // Create an ottofile WITHOUT specifying parallel (should default to true)
    let config = r#"
tasks:
  install:
    foreach:
      items: [a, b, c]
      as: pkg
    bash: echo ${pkg}
"#;
    fs::write(&ottofile_path, config).unwrap();

    let args = vec![
        "otto".to_string(),
        "--ottofile".to_string(),
        ottofile_path.to_string_lossy().to_string(),
        "install".to_string(),
    ];

    let mut parser = Parser::new(args).unwrap();
    let result = parser.parse().unwrap();
    let (tasks, _, _, _, _, _) = result.into_run().unwrap().into_parts();

    // Find subtask b
    let subtask_b = tasks.iter().find(|t| t.name == "install:b").unwrap();

    // Default (parallel: true) means b should NOT depend on a
    assert!(
        !subtask_b.task_deps.iter().any(|d| d.task == "install:a"),
        "By default, install:b should NOT depend on install:a, got: {:?}",
        subtask_b.task_deps
    );
}

// Tests for subtask targeting (task:subtask notation)

#[test]
fn test_collect_transitive_deps_parent_expands_subtasks() {
    // When running parent task "install", all subtasks should be collected

    let task_deps = HashMap::new();

    let mut task_specs = TaskSpecs::new();

    // Virtual parent task
    let parent_spec = TaskSpec {
        name: "install".to_string(),
        action: String::new(), // Virtual parent has no action
        ..Default::default()
    };
    task_specs.insert("install".to_string(), parent_spec);

    // Subtasks
    let subtask_td = TaskSpec {
        name: "install:td".to_string(),
        action: "echo td".to_string(),
        ..Default::default()
    };
    task_specs.insert("install:td".to_string(), subtask_td);

    let subtask_ts = TaskSpec {
        name: "install:ts".to_string(),
        action: "echo ts".to_string(),
        ..Default::default()
    };
    task_specs.insert("install:ts".to_string(), subtask_ts);

    let subtask_cs = TaskSpec {
        name: "install:cs".to_string(),
        action: "echo cs".to_string(),
        ..Default::default()
    };
    task_specs.insert("install:cs".to_string(), subtask_cs);

    let mut collected = HashSet::new();

    // Running "install" (parent) should expand to all subtasks
    Parser::collect_transitive_deps("install", &task_deps, &task_specs, &mut collected).unwrap();

    assert!(collected.contains("install"), "parent should be collected");
    assert!(collected.contains("install:td"), "subtask td should be collected");
    assert!(collected.contains("install:ts"), "subtask ts should be collected");
    assert!(collected.contains("install:cs"), "subtask cs should be collected");
    assert_eq!(collected.len(), 4);
}

#[test]
fn test_collect_transitive_deps_subtask_does_not_expand_siblings() {
    // When running a specific subtask "install:td", should NOT collect siblings
    let task_deps = HashMap::new();

    let mut task_specs = TaskSpecs::new();

    // Virtual parent task
    let parent_spec = TaskSpec {
        name: "install".to_string(),
        action: String::new(),
        ..Default::default()
    };
    task_specs.insert("install".to_string(), parent_spec);

    // Subtasks
    let subtask_td = TaskSpec {
        name: "install:td".to_string(),
        action: "echo td".to_string(),
        ..Default::default()
    };
    task_specs.insert("install:td".to_string(), subtask_td);

    let subtask_ts = TaskSpec {
        name: "install:ts".to_string(),
        action: "echo ts".to_string(),
        ..Default::default()
    };
    task_specs.insert("install:ts".to_string(), subtask_ts);

    let subtask_cs = TaskSpec {
        name: "install:cs".to_string(),
        action: "echo cs".to_string(),
        ..Default::default()
    };
    task_specs.insert("install:cs".to_string(), subtask_cs);

    let mut collected = HashSet::new();

    // Running "install:td" should NOT expand to sibling subtasks
    Parser::collect_transitive_deps("install:td", &task_deps, &task_specs, &mut collected).unwrap();

    assert!(
        collected.contains("install:td"),
        "requested subtask should be collected"
    );
    assert!(
        !collected.contains("install:ts"),
        "sibling subtask ts should NOT be collected"
    );
    assert!(
        !collected.contains("install:cs"),
        "sibling subtask cs should NOT be collected"
    );
    assert!(!collected.contains("install"), "parent should NOT be collected");
    assert_eq!(collected.len(), 1);
}

#[test]
fn test_collect_transitive_deps_subtask_with_deps() {
    // Subtask with its own dependencies should still collect those
    let mut task_deps = HashMap::new();
    task_deps.insert("install:td".to_string(), vec![TaskEdge::success("setup")]);
    task_deps.insert("setup".to_string(), vec![]);

    let mut task_specs = TaskSpecs::new();

    let parent_spec = TaskSpec {
        name: "install".to_string(),
        action: String::new(),
        ..Default::default()
    };
    task_specs.insert("install".to_string(), parent_spec);

    let subtask_td = TaskSpec {
        name: "install:td".to_string(),
        action: "echo td".to_string(),
        ..Default::default()
    };
    task_specs.insert("install:td".to_string(), subtask_td);

    let subtask_ts = TaskSpec {
        name: "install:ts".to_string(),
        action: "echo ts".to_string(),
        ..Default::default()
    };
    task_specs.insert("install:ts".to_string(), subtask_ts);

    let setup_spec = TaskSpec {
        name: "setup".to_string(),
        action: "echo setup".to_string(),
        ..Default::default()
    };
    task_specs.insert("setup".to_string(), setup_spec);

    let mut collected = HashSet::new();

    // Running "install:td" should collect its dependency "setup" but NOT sibling subtasks
    Parser::collect_transitive_deps("install:td", &task_deps, &task_specs, &mut collected).unwrap();

    assert!(
        collected.contains("install:td"),
        "requested subtask should be collected"
    );
    assert!(collected.contains("setup"), "dependency should be collected");
    assert!(
        !collected.contains("install:ts"),
        "sibling subtask should NOT be collected"
    );
    assert_eq!(collected.len(), 2);
}

#[test]
fn test_collect_transitive_deps_multiple_subtasks_requested() {
    // Test requesting multiple specific subtasks (e.g., install:td install:cs)
    let task_deps = HashMap::new();

    let mut task_specs = TaskSpecs::new();

    let parent_spec = TaskSpec {
        name: "install".to_string(),
        action: String::new(),
        ..Default::default()
    };
    task_specs.insert("install".to_string(), parent_spec);

    let subtask_td = TaskSpec {
        name: "install:td".to_string(),
        action: "echo td".to_string(),
        ..Default::default()
    };
    task_specs.insert("install:td".to_string(), subtask_td);

    let subtask_ts = TaskSpec {
        name: "install:ts".to_string(),
        action: "echo ts".to_string(),
        ..Default::default()
    };
    task_specs.insert("install:ts".to_string(), subtask_ts);

    let subtask_cs = TaskSpec {
        name: "install:cs".to_string(),
        action: "echo cs".to_string(),
        ..Default::default()
    };
    task_specs.insert("install:cs".to_string(), subtask_cs);

    let mut collected = HashSet::new();

    // Collect install:td
    Parser::collect_transitive_deps("install:td", &task_deps, &task_specs, &mut collected).unwrap();
    // Collect install:cs
    Parser::collect_transitive_deps("install:cs", &task_deps, &task_specs, &mut collected).unwrap();

    assert!(
        collected.contains("install:td"),
        "first requested subtask should be collected"
    );
    assert!(
        collected.contains("install:cs"),
        "second requested subtask should be collected"
    );
    assert!(
        !collected.contains("install:ts"),
        "unrequested sibling should NOT be collected"
    );
    assert!(!collected.contains("install"), "parent should NOT be collected");
    assert_eq!(collected.len(), 2);
}

#[test]
fn test_collect_transitive_deps_nested_colon_names() {
    // Test task names with multiple colons (e.g., "group:subgroup:item")
    let task_deps = HashMap::new();

    let mut task_specs = TaskSpecs::new();

    // Even with nested colons, contains(':') returns true, so no expansion
    let nested_task = TaskSpec {
        name: "group:sub:item".to_string(),
        action: "echo nested".to_string(),
        ..Default::default()
    };
    task_specs.insert("group:sub:item".to_string(), nested_task);

    // Another nested task that shouldn't be collected
    let other_nested = TaskSpec {
        name: "group:sub:other".to_string(),
        action: "echo other".to_string(),
        ..Default::default()
    };
    task_specs.insert("group:sub:other".to_string(), other_nested);

    let mut collected = HashSet::new();

    Parser::collect_transitive_deps("group:sub:item", &task_deps, &task_specs, &mut collected).unwrap();

    assert!(collected.contains("group:sub:item"));
    assert!(
        !collected.contains("group:sub:other"),
        "nested sibling should NOT be collected"
    );
    assert_eq!(collected.len(), 1);
}

#[test]
fn test_collect_transitive_deps_subtask_with_after() {
    // Test that subtasks can use 'after' and it still works correctly
    let task_deps = HashMap::new();

    let mut task_specs = TaskSpecs::new();

    let parent_spec = TaskSpec {
        name: "install".to_string(),
        action: String::new(),
        ..Default::default()
    };
    task_specs.insert("install".to_string(), parent_spec);

    // install:td has an 'after' that should trigger report
    let subtask_td = TaskSpec {
        name: "install:td".to_string(),
        action: "echo td".to_string(),
        after: vec![crate::cfg::edge::EdgeSpec::sugar("report")],
        ..Default::default()
    };
    task_specs.insert("install:td".to_string(), subtask_td);

    let subtask_ts = TaskSpec {
        name: "install:ts".to_string(),
        action: "echo ts".to_string(),
        ..Default::default()
    };
    task_specs.insert("install:ts".to_string(), subtask_ts);

    let report_spec = TaskSpec {
        name: "report".to_string(),
        action: "echo report".to_string(),
        ..Default::default()
    };
    task_specs.insert("report".to_string(), report_spec);

    let mut collected = HashSet::new();

    Parser::collect_transitive_deps("install:td", &task_deps, &task_specs, &mut collected).unwrap();

    assert!(
        collected.contains("install:td"),
        "requested subtask should be collected"
    );
    assert!(collected.contains("report"), "'after' task should be collected");
    assert!(!collected.contains("install:ts"), "sibling should NOT be collected");
    assert_eq!(collected.len(), 2);
}

#[test]
fn test_collect_transitive_deps_dependency_on_specific_subtask() {
    // Test: task 'deploy' depends on a specific subtask 'install:td'
    // Running 'deploy' should collect install:td but NOT other install subtasks
    let mut task_deps = HashMap::new();
    task_deps.insert("deploy".to_string(), vec![TaskEdge::success("install:td")]);

    let mut task_specs = TaskSpecs::new();

    let parent_spec = TaskSpec {
        name: "install".to_string(),
        action: String::new(),
        ..Default::default()
    };
    task_specs.insert("install".to_string(), parent_spec);

    let subtask_td = TaskSpec {
        name: "install:td".to_string(),
        action: "echo td".to_string(),
        ..Default::default()
    };
    task_specs.insert("install:td".to_string(), subtask_td);

    let subtask_ts = TaskSpec {
        name: "install:ts".to_string(),
        action: "echo ts".to_string(),
        ..Default::default()
    };
    task_specs.insert("install:ts".to_string(), subtask_ts);

    let deploy_spec = TaskSpec {
        name: "deploy".to_string(),
        action: "echo deploy".to_string(),
        ..Default::default()
    };
    task_specs.insert("deploy".to_string(), deploy_spec);

    let mut collected = HashSet::new();

    Parser::collect_transitive_deps("deploy", &task_deps, &task_specs, &mut collected).unwrap();

    assert!(collected.contains("deploy"), "requested task should be collected");
    assert!(
        collected.contains("install:td"),
        "dependency subtask should be collected"
    );
    assert!(
        !collected.contains("install:ts"),
        "other subtask should NOT be collected"
    );
    assert!(!collected.contains("install"), "parent should NOT be collected");
    assert_eq!(collected.len(), 2);
}

fn edge_spec(task: &str, when: When) -> crate::cfg::edge::EdgeSpec {
    crate::cfg::edge::EdgeSpec {
        task: task.to_string(),
        when,
        from_sugar: false,
        is_injected_sugar: false,
    }
}

/// Paired `when: success` + `when: failure` on the same source can never both be
/// satisfied, so the dependent could never run. It used to be accepted and skipped
/// silently at exit 0; it is now rejected where dependencies are computed.
#[test]
fn test_paired_success_and_failure_edges_are_rejected() {
    let mut task_specs = TaskSpecs::new();
    task_specs.insert(
        "src".to_string(),
        TaskSpec {
            name: "src".to_string(),
            action: "echo src".to_string(),
            ..Default::default()
        },
    );
    task_specs.insert(
        "dep".to_string(),
        TaskSpec {
            name: "dep".to_string(),
            action: "echo dep".to_string(),
            before: vec![edge_spec("src", When::Success), edge_spec("src", When::Failure)],
            ..Default::default()
        },
    );

    let err = Parser::compute_task_deps_from_specs(&task_specs)
        .expect_err("a dependent gated on both outcomes of one source must be rejected");
    let message = format!("{err:#}");
    assert!(message.contains("'dep'"), "must name the dependent: {message}");
    assert!(message.contains("'src'"), "must name the source: {message}");
    assert!(message.contains("when: always"), "must offer the fix: {message}");
}

/// The paired-edge check must not fire on the shapes that are legal: two edges to
/// different sources, or a `when: always` edge alongside a conditional one.
#[test]
fn test_distinct_sources_with_opposite_conditions_are_accepted() {
    let mut task_specs = TaskSpecs::new();
    for name in ["one", "two"] {
        task_specs.insert(
            name.to_string(),
            TaskSpec {
                name: name.to_string(),
                action: format!("echo {name}"),
                ..Default::default()
            },
        );
    }
    task_specs.insert(
        "dep".to_string(),
        TaskSpec {
            name: "dep".to_string(),
            action: "echo dep".to_string(),
            before: vec![
                edge_spec("one", When::Success),
                edge_spec("two", When::Failure),
                edge_spec("one", When::Always),
            ],
            ..Default::default()
        },
    );

    let deps = Parser::compute_task_deps_from_specs(&task_specs).expect("distinct sources are legal");
    assert_eq!(deps["dep"].len(), 3);
}

#[test]
fn test_collect_transitive_deps_regular_task_no_expansion() {
    // Regular tasks (no colons) that have no subtasks should not try to expand
    let task_deps = HashMap::new();

    let mut task_specs = TaskSpecs::new();

    let build_spec = TaskSpec {
        name: "build".to_string(),
        action: "echo build".to_string(),
        ..Default::default()
    };
    task_specs.insert("build".to_string(), build_spec);

    let test_spec = TaskSpec {
        name: "test".to_string(),
        action: "echo test".to_string(),
        ..Default::default()
    };
    task_specs.insert("test".to_string(), test_spec);

    let mut collected = HashSet::new();

    Parser::collect_transitive_deps("build", &task_deps, &task_specs, &mut collected).unwrap();

    assert!(collected.contains("build"));
    assert!(!collected.contains("test"));
    assert_eq!(collected.len(), 1);
}

#[test]
fn test_subtask_targeting_integration() {
    // Integration test: parse an ottofile with foreach and request specific subtask
    use std::fs;
    use tempfile::TempDir;

    let temp_dir = TempDir::new().unwrap();
    let ottofile_path = temp_dir.path().join("otto.yml");

    let config = r#"
tasks:
  install:
    foreach:
      items: [td, ts, cs]
      as: pkg
    bash: echo "Installing ${pkg}"
"#;
    fs::write(&ottofile_path, config).unwrap();

    let args = vec![
        "otto".to_string(),
        "--ottofile".to_string(),
        ottofile_path.to_string_lossy().to_string(),
        "install:td".to_string(), // Request ONLY this subtask
    ];

    let mut parser = Parser::new(args).unwrap();
    let result = parser.parse().unwrap();
    let (tasks, _, _, _, _, _) = result.into_run().unwrap().into_parts();

    // Should only have install:td, NOT install:ts or install:cs
    let task_names: Vec<&str> = tasks.iter().map(|t| t.name.as_str()).collect();
    assert!(
        task_names.contains(&"install:td"),
        "requested subtask should be present"
    );
    assert!(
        !task_names.contains(&"install:ts"),
        "sibling subtask should NOT be present"
    );
    assert!(
        !task_names.contains(&"install:cs"),
        "sibling subtask should NOT be present"
    );
    assert_eq!(tasks.len(), 1);
}

/// End-to-end: a param with `nargs: "+"` collects every space-separated
/// value from one CLI occurrence into `task.values` as a `Value::List`
/// and into `task.envs` as a space-joined string, not just the first
/// value (the pre-fix behavior via `get_one`).
#[test]
fn test_nargs_one_or_more_param_collects_every_value_end_to_end() {
    use std::fs;
    use tempfile::TempDir;

    let temp_dir = TempDir::new().unwrap();
    let ottofile_path = temp_dir.path().join("otto.yml");

    let config = r#"
tasks:
  build:
    params:
      --files:
        nargs: "+"
        help: files to build
    bash: echo "${files}"
"#;
    fs::write(&ottofile_path, config).unwrap();

    let args = vec![
        "otto".to_string(),
        "--ottofile".to_string(),
        ottofile_path.to_string_lossy().to_string(),
        "build".to_string(),
        "--files".to_string(),
        "a.txt".to_string(),
        "b.txt".to_string(),
    ];

    let mut parser = Parser::new(args).unwrap();
    let result = parser.parse().unwrap();
    let (tasks, _, _, _, _, _) = result.into_run().unwrap().into_parts();

    let build = tasks.iter().find(|t| t.name == "build").expect("build task present");
    assert_eq!(
        build.values.get("files"),
        Some(&Value::List(vec!["a.txt".to_string(), "b.txt".to_string()]))
    );
    assert_eq!(build.envs.get("files"), Some(&"a.txt b.txt".to_string()));
}

#[test]
fn test_parent_task_runs_all_subtasks_integration() {
    // Integration test: requesting parent task should run all subtasks
    use std::fs;
    use tempfile::TempDir;

    let temp_dir = TempDir::new().unwrap();
    let ottofile_path = temp_dir.path().join("otto.yml");

    let config = r#"
tasks:
  install:
    foreach:
      items: [td, ts, cs]
      as: pkg
    bash: echo "Installing ${pkg}"
"#;
    fs::write(&ottofile_path, config).unwrap();

    let args = vec![
        "otto".to_string(),
        "--ottofile".to_string(),
        ottofile_path.to_string_lossy().to_string(),
        "install".to_string(), // Request parent
    ];

    let mut parser = Parser::new(args).unwrap();
    let result = parser.parse().unwrap();
    let (tasks, _, _, _, _, _) = result.into_run().unwrap().into_parts();

    // Should have all subtasks plus the (now executable) virtual parent
    let task_names: Vec<&str> = tasks.iter().map(|t| t.name.as_str()).collect();
    assert!(task_names.contains(&"install:td"));
    assert!(task_names.contains(&"install:ts"));
    assert!(task_names.contains(&"install:cs"));
    assert!(task_names.contains(&"install"));
    assert_eq!(tasks.len(), 4);
}

#[test]
fn test_dependency_on_subtask_integration() {
    // Integration test: task depending on specific subtask
    use std::fs;
    use tempfile::TempDir;

    let temp_dir = TempDir::new().unwrap();
    let ottofile_path = temp_dir.path().join("otto.yml");

    let config = r#"
tasks:
  install:
    foreach:
      items: [td, ts, cs]
      as: pkg
    bash: echo "Installing ${pkg}"

  deploy:
    before: ["install:td"]
    bash: echo "Deploying"
"#;
    fs::write(&ottofile_path, config).unwrap();

    let args = vec![
        "otto".to_string(),
        "--ottofile".to_string(),
        ottofile_path.to_string_lossy().to_string(),
        "deploy".to_string(),
    ];

    let mut parser = Parser::new(args).unwrap();
    let result = parser.parse().unwrap();
    let (tasks, _, _, _, _, _) = result.into_run().unwrap().into_parts();

    let task_names: Vec<&str> = tasks.iter().map(|t| t.name.as_str()).collect();
    assert!(task_names.contains(&"deploy"), "deploy should be present");
    assert!(
        task_names.contains(&"install:td"),
        "dependency subtask should be present"
    );
    assert!(
        !task_names.contains(&"install:ts"),
        "other subtask should NOT be present"
    );
    assert!(
        !task_names.contains(&"install:cs"),
        "other subtask should NOT be present"
    );
    assert_eq!(tasks.len(), 2);
}

#[test]
fn test_unknown_dependency_errors() {
    // Test that referencing an unknown dependency produces an error
    use std::fs;
    use tempfile::TempDir;

    let temp_dir = TempDir::new().unwrap();
    let ottofile_path = temp_dir.path().join("otto.yml");

    let config = r#"
tasks:
  build:
    before: ["nonexistent_task"]
    bash: echo "Building"
"#;
    fs::write(&ottofile_path, config).unwrap();

    let args = vec![
        "otto".to_string(),
        "--ottofile".to_string(),
        ottofile_path.to_string_lossy().to_string(),
        "build".to_string(),
    ];

    let mut parser = Parser::new(args).unwrap();
    let result = parser.parse();

    assert!(result.is_err());
    let err = result.unwrap_err();
    assert!(
        err.to_string().contains("unknown dependency"),
        "Error should mention unknown dependency: {}",
        err
    );
    assert!(
        err.to_string().contains("nonexistent_task"),
        "Error should mention the dependency name: {}",
        err
    );
}

#[test]
fn test_unknown_subtask_dependency_errors() {
    // Test that referencing a typo'd subtask produces an error
    use std::fs;
    use tempfile::TempDir;

    let temp_dir = TempDir::new().unwrap();
    let ottofile_path = temp_dir.path().join("otto.yml");

    let config = r#"
tasks:
  install:
    foreach:
      items: [td, ts, cs]
    bash: echo "Installing ${item}"

  deploy:
    before: ["install:tx"]  # Typo: should be "install:td"
    bash: echo "Deploying"
"#;
    fs::write(&ottofile_path, config).unwrap();

    let args = vec![
        "otto".to_string(),
        "--ottofile".to_string(),
        ottofile_path.to_string_lossy().to_string(),
        "deploy".to_string(),
    ];

    let mut parser = Parser::new(args).unwrap();
    let result = parser.parse();

    assert!(result.is_err());
    let err = result.unwrap_err();
    assert!(
        err.to_string().contains("unknown dependency"),
        "Error should mention unknown dependency: {}",
        err
    );
    assert!(
        err.to_string().contains("install:tx"),
        "Error should mention the typo'd subtask: {}",
        err
    );
}

#[test]
fn test_valid_dependencies_succeed() {
    // Test that valid dependencies don't trigger the error
    use std::fs;
    use tempfile::TempDir;

    let temp_dir = TempDir::new().unwrap();
    let ottofile_path = temp_dir.path().join("otto.yml");

    let config = r#"
tasks:
  setup:
    bash: echo "Setting up"

  build:
    before: ["setup"]
    bash: echo "Building"
"#;
    fs::write(&ottofile_path, config).unwrap();

    let args = vec![
        "otto".to_string(),
        "--ottofile".to_string(),
        ottofile_path.to_string_lossy().to_string(),
        "build".to_string(),
    ];

    let mut parser = Parser::new(args).unwrap();
    let result = parser.parse();

    assert!(result.is_ok(), "Valid dependencies should succeed: {:?}", result.err());
}

// =========================================================================
// help drift regression (docs/design/2026-08-28-boundary-fixes-and-dynamic-foreach.md Phase 1)
// =========================================================================

/// The exact `Options:` block otto's global flags must render as, in every
/// builder. Pinned so a builder that stops calling `global_args()` (or a
/// change to `global_args()` that isn't propagated) fails loudly instead
/// of silently dropping flags from `--help` again.
///
/// `{JOBS}` stands in for `-j/--jobs`'s default, which is `DEFAULT_JOBS`
/// (`num_cpus::get()` on the machine that renders the help text, not a
/// fixed number). A literal `32` here pinned this test to the developing
/// machine's core count: green locally, red on any runner with a
/// different core count (`docs/design/2026-06-10-code-review-remediation.md`
/// Phase 0). `expected_global_options_help()` below substitutes the real
/// default at test time so the anti-drift check still holds everywhere
/// else in the string.
const EXPECTED_GLOBAL_OPTIONS_HELP_TEMPLATE: &str = "Options:\n  -C, --cwd <DIR>\n          Change to DIR before doing anything\n\n  -o, --ottofile <PATH>\n          path to the ottofile\n          \n          [env: OTTOFILE=]\n          [default: .]\n\n      --list-subtasks\n          List all foreach subtasks and exit\n\n      --tasks\n          Print the machine-readable task list and exit\n\n      --format <FORMAT>\n          Output format for --tasks (yaml or json); default: yaml on a tty, json when piped\n          \n          [possible values: yaml, json]\n\n  -j, --jobs <N>\n          Number of parallel jobs\n          \n          [default: {JOBS}]\n\n  -t, --tui\n          Enable interactive TUI dashboard for task monitoring\n\n      --no-prefix\n          Suppress the [task] prefix on task output\n\n      --log-level <LEVEL>\n          Verbosity of otto's own log file, under $XDG_DATA_HOME/otto/logs\n          \n          [possible values: off, error, warn, info, debug, trace]\n\n  -h, --help\n          Print help\n\n  -V, --version\n          Print version";

/// Renders `EXPECTED_GLOBAL_OPTIONS_HELP_TEMPLATE` against this
/// machine's actual `-j/--jobs` default, so the comparison is exact
/// everywhere except the one value that is legitimately
/// machine-dependent.
fn expected_global_options_help() -> String {
    EXPECTED_GLOBAL_OPTIONS_HELP_TEMPLATE.replace("{JOBS}", &DEFAULT_JOBS)
}

/// Extracts the `Options:` section, from the `Options:` heading through
/// the auto-appended `-V, --version` entry (always the last flag clap
/// renders). Builders may append more after that (subcommand-derived
/// `after_help` error text, in `build_help_command_with_error()`'s case)
/// which is irrelevant to this test and must not pollute the comparison.
fn options_section(help: &str) -> &str {
    let start = help
        .find("Options:")
        .expect("help output must contain an Options: section");
    let rest = &help[start..];
    let anchor = "Print version";
    let end = rest.find(anchor).expect("help output must contain -V, --version") + anchor.len();
    &rest[..end]
}

#[test]
fn test_help_global_flags_no_drift() {
    // Parser::new() doesn't load the ottofile (that happens in parse()),
    // so build_help_command() sees an empty config_spec and takes its
    // after_help branch here. That only affects content appended after
    // the Options: section, which options_section() strips off below -
    // irrelevant to this test's concern (global flag parity).
    let args = vec!["otto".to_string()];
    let parser = Parser::new(args).unwrap();

    let otto_cmd_help = Parser::otto_command().render_long_help().to_string();
    let help_cmd_help = parser.build_help_command().render_long_help().to_string();
    let help_cmd_error_help = Parser::build_help_command_with_error().render_long_help().to_string();

    let expected = expected_global_options_help();
    assert_eq!(
        options_section(&otto_cmd_help),
        expected,
        "otto_command() global flags drifted from the pinned snapshot"
    );
    assert_eq!(
        options_section(&help_cmd_help),
        expected,
        "build_help_command() global flags drifted from the pinned snapshot"
    );
    assert_eq!(
        options_section(&help_cmd_error_help),
        expected,
        "build_help_command_with_error() global flags drifted from the pinned snapshot"
    );
}

/// `expected_global_options_help()` must reflect this machine's actual
/// `-j/--jobs` default rather than a value baked in at write time - the
/// exact bug this fix closes. Locks the substitution itself, not just
/// the drift check that depends on it.
#[test]
fn test_expected_global_options_help_substitutes_actual_jobs_default() {
    let expected = expected_global_options_help();
    assert!(
        expected.contains(&format!("[default: {}]", num_cpus::get())),
        "expected help must reflect this machine's num_cpus::get(), got: {expected}"
    );
    assert!(
        !expected.contains("{JOBS}"),
        "template placeholder must be fully substituted"
    );
}

// =========================================================================
// otto.api version gate (design doc 2026-08-29-strict-ottofile-schema)
// =========================================================================

/// Write `content` as an ottofile in a fresh temp dir and load it through
/// the real config path.
fn load_ottofile(content: &str) -> (tempfile::TempDir, Result<(ConfigSpec, String, Option<PathBuf>)>) {
    let temp_dir = tempfile::TempDir::new().unwrap();
    let ottofile_path = temp_dir.path().join("otto.yml");
    std::fs::write(&ottofile_path, content).unwrap();
    let result = Parser::load_config_from_path(Some(ottofile_path));
    // The TempDir rides along so it outlives the load.
    (temp_dir, result)
}

#[test]
fn test_load_config_rejects_an_unsupported_api_version() {
    let (_dir, result) = load_ottofile("otto:\n  api: 2\ntasks:\n  up:\n    bash: echo hi\n");
    let err = result.expect_err("api: 2 must not load").to_string();
    assert!(err.contains("unsupported api version '2'"), "{err}");
    assert!(err.contains("this otto supports: 1"), "{err}");
    assert!(err.contains("upgrade otto"), "{err}");
}

#[test]
fn test_load_config_accepts_a_supported_and_an_absent_api_version() {
    let (_dir, declared) = load_ottofile("otto:\n  api: 1\ntasks:\n  up:\n    bash: echo hi\n");
    let (config, ..) = declared.expect("api: 1 must load");
    assert_eq!(config.otto.api, "1");
    assert!(config.tasks.contains_key("up"));

    let (_dir, absent) = load_ottofile("tasks:\n  up:\n    bash: echo hi\n");
    let (config, ..) = absent.expect("an absent api: must load");
    assert_eq!(config.otto.api, "1");
    assert!(config.tasks.contains_key("up"));
}

/// THE ORDERING ASSERT. The api gate runs BEFORE the typed parse, so a file
/// that is both too new AND unparseable by this otto reports "upgrade otto"
/// rather than a complaint about whichever key it could not read. Reverse
/// the two statements in `load_config_from_path` and this test fails on the
/// second assert: serde reports `tasks.up.before: invalid type: map,
/// expected a sequence`, which tells the operator nothing useful.
#[test]
fn test_load_config_reports_the_api_error_before_the_parse_error() {
    let (_dir, result) =
        load_ottofile("otto:\n  api: 2\ntasks:\n  up:\n    before:\n      some: map\n    bash: echo hi\n");
    let err = result.expect_err("a file that is too new must not load").to_string();
    assert!(
        err.contains("unsupported api version '2'"),
        "the version error wins: {err}"
    );
    assert!(
        !err.contains("invalid type"),
        "the parse error must not win over the version error: {err}"
    );
}
