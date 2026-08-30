#![cfg(test)]

use super::*;
use std::collections::HashMap;

#[test]
fn test_validate_acyclic_accepts_a_dag() {
    let tasks = vec![
        create_test_task("a", vec![]),
        create_test_task("b", vec!["a"]),
        create_test_task("c", vec!["b"]),
    ];
    assert!(DagVisualizer::validate_acyclic(&tasks).is_ok());
}

#[test]
fn test_validate_acyclic_rejects_a_two_cycle_and_names_it() {
    let tasks = vec![create_test_task("a", vec!["b"]), create_test_task("b", vec!["a"])];
    let err = DagVisualizer::validate_acyclic(&tasks).expect_err("a -> b -> a is not a DAG");
    let msg = err.to_string();
    assert!(msg.contains("dependency cycle detected"), "{msg}");
    assert!(
        msg.contains("a -> b") || msg.contains("b -> a"),
        "the path must be named: {msg}"
    );
}

#[test]
fn test_validate_acyclic_rejects_a_self_dependency() {
    let tasks = vec![create_test_task("a", vec!["a"])];
    let err = DagVisualizer::validate_acyclic(&tasks).expect_err("a depending on itself is a cycle");
    assert!(err.to_string().contains("a -> a"), "{err}");
}

#[test]
fn test_validate_acyclic_ignores_dependencies_outside_the_task_set() {
    // `collect_transitive_deps` can hand the scheduler a task whose dep is
    // not in the run set; that is not a cycle.
    let tasks = vec![create_test_task("b", vec!["a"])];
    assert!(DagVisualizer::validate_acyclic(&tasks).is_ok());
}

fn create_test_task(name: &str, deps: Vec<&str>) -> Task {
    let parent = Task::derive_parent(name);
    Task::new(
        name.to_string(),
        parent,
        deps.into_iter().map(crate::executor::task::TaskEdge::success).collect(),
        vec![],
        vec![],
        HashMap::new(),
        HashMap::new(),
        format!("echo 'Running {name}'"),
    )
}

#[test]
fn test_dot_generation_simple() -> Result<()> {
    let mut dag = DAG::new();

    let task1 = create_test_task("build", vec![]);
    let task2 = create_test_task("test", vec!["build"]);

    dag.add_node(task1);
    dag.add_node(task2);

    let visualizer = DagVisualizer::with_defaults();
    let dot = visualizer.generate_dot(&dag)?;

    assert!(dot.contains("digraph otto_dag"));
    assert!(dot.contains("task_0"));
    assert!(dot.contains("task_1"));
    assert!(dot.contains("Otto Task DAG"));

    Ok(())
}

fn create_serial_member(name: &str, group: &str, index: usize) -> Task {
    let mut task = create_test_task(name, vec![]);
    task.serial_group = Some(group.to_string());
    task.serial_index = index;
    task
}

/// Serial ordering has no `task_deps` entry, so DOT renders it from the group index
/// as a dashed `order` edge - visually distinct from a `depends` edge, because the
/// two mean different things.
#[test]
fn test_dot_renders_serial_group_as_order_edges() -> Result<()> {
    let mut dag = DAG::new();
    dag.add_node(create_serial_member("up:alpha", "up", 0));
    dag.add_node(create_serial_member("up:beta", "up", 1));
    dag.add_node(create_serial_member("up:gamma", "up", 2));

    let visualizer = DagVisualizer::with_defaults();
    let dot = visualizer.generate_dot(&dag)?;

    assert!(
        dot.contains(r#"task_0 -> task_1 [label="order", color="gray40", style="dashed"];"#),
        "expected an order edge alpha -> beta, got:\n{dot}"
    );
    assert!(
        dot.contains(r#"task_1 -> task_2 [label="order", color="gray40", style="dashed"];"#),
        "expected an order edge beta -> gamma, got:\n{dot}"
    );
    assert!(
        !dot.contains(r#"label="depends""#),
        "serial ordering must not be rendered as a depends edge, got:\n{dot}"
    );
    Ok(())
}

/// Only members actually in the graph are chained: a targeted run of one member
/// renders no ordering edge at all.
#[test]
fn test_dot_emits_no_order_edge_for_a_lone_group_member() -> Result<()> {
    let mut dag = DAG::new();
    dag.add_node(create_serial_member("up:gamma", "up", 2));

    let visualizer = DagVisualizer::with_defaults();
    let dot = visualizer.generate_dot(&dag)?;

    assert!(!dot.contains(r#"label="order""#), "got:\n{dot}");
    Ok(())
}

#[test]
fn test_ascii_generation() -> Result<()> {
    let mut dag = DAG::new();

    let task1 = create_test_task("setup", vec![]);
    let task2 = create_test_task("build", vec!["setup"]);
    let task3 = create_test_task("test", vec!["build"]);

    dag.add_node(task1);
    dag.add_node(task2);
    dag.add_node(task3);

    let visualizer = DagVisualizer::with_defaults();
    let empty_specs = TaskSpecs::new();
    let ascii = visualizer.generate_ascii(&dag, &empty_specs)?;

    assert!(ascii.contains("Otto Task DAG"));
    assert!(ascii.contains("setup"));
    assert!(ascii.contains("build"));
    assert!(ascii.contains("test"));
    assert!(ascii.contains("Legend"));

    Ok(())
}

#[test]
fn test_graphviz_detection() {
    let visualizer = DagVisualizer::with_defaults();
    // This test will pass regardless of whether graphviz is installed
    let _has_graphviz = visualizer.is_graphviz_available();
}

#[test]
fn test_dot_string_escaping() {
    let visualizer = DagVisualizer::with_defaults();
    assert_eq!(visualizer.escape_dot_string("hello"), "hello");
    assert_eq!(visualizer.escape_dot_string("hello\nworld"), "hello\\nworld");
    assert_eq!(visualizer.escape_dot_string("say \"hello\""), "say \\\"hello\\\"");
    assert_eq!(visualizer.escape_dot_string("path\\to\\file"), "path\\\\to\\\\file");
}

#[test]
fn test_infer_subtask_pattern_rs_extension() {
    let subtasks: Vec<Task> = vec![
        create_test_task("examples:04_task_manager_api.rs", vec![]),
        create_test_task("examples:05_scheduler_api.rs", vec![]),
        create_test_task("examples:06_event_bus.rs", vec![]),
    ];
    let refs: Vec<&Task> = subtasks.iter().collect();
    let pattern = DagVisualizer::infer_subtask_pattern(&refs);
    assert_eq!(pattern, "*.rs");
}

#[test]
fn test_infer_subtask_pattern_sh_extension() {
    let subtasks: Vec<Task> = vec![
        create_test_task("scripts:build.sh", vec![]),
        create_test_task("scripts:deploy.sh", vec![]),
    ];
    let refs: Vec<&Task> = subtasks.iter().collect();
    let pattern = DagVisualizer::infer_subtask_pattern(&refs);
    assert_eq!(pattern, "*.sh");
}

#[test]
fn test_infer_subtask_pattern_mixed_extensions() {
    let subtasks: Vec<Task> = vec![
        create_test_task("examples:basic.rs", vec![]),
        create_test_task("examples:script.sh", vec![]),
    ];
    let refs: Vec<&Task> = subtasks.iter().collect();
    let pattern = DagVisualizer::infer_subtask_pattern(&refs);
    assert_eq!(pattern, "*"); // Mixed extensions fall back to *
}

#[test]
fn test_infer_subtask_pattern_no_extension() {
    let subtasks: Vec<Task> = vec![
        create_test_task("deploy:dev", vec![]),
        create_test_task("deploy:staging", vec![]),
        create_test_task("deploy:prod", vec![]),
    ];
    let refs: Vec<&Task> = subtasks.iter().collect();
    let pattern = DagVisualizer::infer_subtask_pattern(&refs);
    assert_eq!(pattern, "*"); // No common extension
}

#[test]
fn test_ascii_collapses_foreach_subtasks() -> Result<()> {
    let mut dag = DAG::new();

    let task1 = create_test_task("examples:04_task_manager_api.rs", vec![]);
    let task2 = create_test_task("examples:05_scheduler_api.rs", vec![]);
    let task3 = create_test_task("examples:06_event_bus.rs", vec![]);

    dag.add_node(task1);
    dag.add_node(task2);
    dag.add_node(task3);

    let visualizer = DagVisualizer::with_defaults();
    // Empty specs falls back to pattern inference (*.rs)
    let empty_specs = TaskSpecs::new();
    let ascii = visualizer.generate_ascii(&dag, &empty_specs)?;

    // Should show collapsed pattern inferred from extensions, not individual subtasks
    assert!(ascii.contains("examples:*.rs [3 items]"));
    assert!(!ascii.contains("04_task_manager_api.rs"));

    Ok(())
}

#[test]
fn test_format_foreach_display_items_small() {
    let foreach = ForeachSpec {
        items: vec!["td".to_string(), "ts".to_string(), "cs".to_string()],
        ..Default::default()
    };
    let display = DagVisualizer::format_foreach_display("install", &foreach, 3);
    assert_eq!(display, "install:{td,ts,cs}");
}

#[test]
fn test_format_foreach_display_items_large() {
    let foreach = ForeachSpec {
        items: vec![
            "a".to_string(),
            "b".to_string(),
            "c".to_string(),
            "d".to_string(),
            "e".to_string(),
            "f".to_string(),
            "g".to_string(),
        ],
        ..Default::default()
    };
    let display = DagVisualizer::format_foreach_display("batch", &foreach, 7);
    assert_eq!(display, "batch:{...} [7 items]");
}

#[test]
fn test_format_foreach_display_glob() {
    let foreach = ForeachSpec {
        glob: Some("*.sh".to_string()),
        ..Default::default()
    };
    let display = DagVisualizer::format_foreach_display("scripts", &foreach, 8);
    assert_eq!(display, "scripts:*.sh [8 items]");
}

#[test]
fn test_format_foreach_display_range() {
    let foreach = ForeachSpec {
        range: Some("1-10".to_string()),
        ..Default::default()
    };
    let display = DagVisualizer::format_foreach_display("batch", &foreach, 10);
    assert_eq!(display, "batch:1-10");
}

#[test]
fn test_format_foreach_display_items_with_special_chars() {
    // Items with commas should fall back to {...} notation
    let foreach = ForeachSpec {
        items: vec!["a,b".to_string(), "c,d".to_string()],
        ..Default::default()
    };
    let display = DagVisualizer::format_foreach_display("special", &foreach, 2);
    assert_eq!(display, "special:{...} [2 items]");
}

#[test]
fn test_collapse_with_original_specs() -> Result<()> {
    use crate::cfg::param::ParamSpecs;
    use crate::cfg::task::TaskSpec;

    let mut dag = DAG::new();

    let task1 = create_test_task("install:td", vec![]);
    let task2 = create_test_task("install:ts", vec![]);
    let task3 = create_test_task("install:cs", vec![]);

    dag.add_node(task1);
    dag.add_node(task2);
    dag.add_node(task3);

    // Create original specs with ForeachSpec
    let mut original_specs = TaskSpecs::new();
    original_specs.insert(
        "install".to_string(),
        TaskSpec {
            name: "install".to_string(),
            help: None,
            before: vec![],
            after: vec![],
            input: vec![],
            output: vec![],
            envs: HashMap::new(),
            params: ParamSpecs::default(),
            action: "echo test".to_string(),
            foreach: Some(ForeachSpec {
                items: vec!["td".to_string(), "ts".to_string(), "cs".to_string()],
                ..Default::default()
            }),
            virtual_parent: false,
            tty: None,
            on_failure: vec![],
        },
    );

    let visualizer = DagVisualizer::with_defaults();
    let ascii = visualizer.generate_ascii(&dag, &original_specs)?;

    // Should use brace notation from ForeachSpec
    assert!(ascii.contains("install:{td,ts,cs}"));
    assert!(!ascii.contains("install:* [3 items]")); // Should NOT fall back to inference

    Ok(())
}
