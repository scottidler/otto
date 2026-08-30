#![cfg(test)]

use super::*;
use crate::cfg::param::ParamSpecs;
use serial_test::serial;
use tempfile::TempDir;

/// One rule for deriving a foreach subtask's parent, and it holds for the shapes
/// the three former copies each had to get right independently.
#[test]
fn test_derive_parent() {
    assert_eq!(Task::derive_parent("install:td"), Some("install".to_string()));
    assert_eq!(Task::derive_parent("build"), None);
    // Only the first colon splits: an item containing a colon stays with the item.
    assert_eq!(Task::derive_parent("deploy:us:east"), Some("deploy".to_string()));
    // A leading colon yields an empty parent rather than panicking or silently
    // treating the name as parentless.
    assert_eq!(Task::derive_parent(":orphan"), Some(String::new()));
    assert_eq!(Task::derive_parent(""), None);
}

fn make_task_spec(name: &str, before: Vec<String>, action: &str) -> TaskSpec {
    use crate::cfg::edge::EdgeSpec;
    TaskSpec {
        name: name.to_string(),
        help: None,
        before: before.into_iter().map(EdgeSpec::sugar).collect(),
        after: vec![],
        input: vec![],
        output: vec![],
        envs: HashMap::new(),
        params: ParamSpecs::default(),
        action: action.to_string(),
        foreach: None,
        virtual_parent: false,
        on_failure: vec![],
        tty: None,
    }
}

#[test]
fn test_calculate_hash() {
    let action = "echo hello".to_string();
    let hash = calculate_hash(&action);

    // Hash should be 8 characters
    assert_eq!(hash.len(), 8);
    // Hash should be hexadecimal
    assert!(hash.chars().all(|c| c.is_ascii_hexdigit()));

    // Same action should produce same hash
    let hash2 = calculate_hash(&action);
    assert_eq!(hash, hash2);

    // Different action should produce different hash
    let action2 = "echo world".to_string();
    let hash3 = calculate_hash(&action2);
    assert_ne!(hash, hash3);
}

#[test]
fn test_task_new() {
    let task = Task::new(
        "build".to_string(),
        None,
        vec![TaskEdge::success("test")],
        vec!["src/main.rs".to_string()],
        vec!["target/app".to_string()],
        HashMap::new(),
        HashMap::new(),
        "cargo build".to_string(),
    );

    assert_eq!(task.name, "build");
    assert_eq!(task.parent, None);
    assert_eq!(task.task_deps.len(), 1);
    assert_eq!(task.task_deps[0].task, "test");
    assert_eq!(task.file_deps, vec!["src/main.rs"]);
    assert_eq!(task.output_deps, vec!["target/app"]);
    assert_eq!(task.action, "cargo build");
    assert_eq!(task.hash.len(), 8);
}

#[test]
fn test_task_with_envs_and_values() {
    let mut envs = HashMap::new();
    envs.insert("FOO".to_string(), "bar".to_string());

    let mut values = HashMap::new();
    values.insert("name".to_string(), Value::Item("test".to_string()));

    let task = Task::new(
        "test".to_string(),
        None,
        vec![],
        vec![],
        vec![],
        envs.clone(),
        values.clone(),
        "echo $FOO".to_string(),
    );

    assert_eq!(task.envs, envs);
    assert_eq!(task.values, values);
}

#[test]
fn test_task_equality() {
    let task1 = Task::new(
        "build".to_string(),
        None,
        vec![],
        vec![],
        vec![],
        HashMap::new(),
        HashMap::new(),
        "cargo build".to_string(),
    );

    let task2 = Task::new(
        "build".to_string(),
        None,
        vec![],
        vec![],
        vec![],
        HashMap::new(),
        HashMap::new(),
        "cargo build".to_string(),
    );

    assert_eq!(task1, task2);
}

#[test]
fn test_resolve_file_globs_absolute_path() {
    let temp_dir = TempDir::new().unwrap();
    let file_path = temp_dir.path().join("test.txt");
    std::fs::write(&file_path, "test content").unwrap();

    let patterns = vec![file_path.to_string_lossy().to_string()];
    let resolved = Task::resolve_file_globs(&patterns, temp_dir.path());

    assert_eq!(resolved.len(), 1);
    assert!(resolved[0].contains("test.txt"));
}

#[test]
fn test_resolve_file_globs_relative_path() {
    let temp_dir = TempDir::new().unwrap();
    let file_path = temp_dir.path().join("test.txt");
    std::fs::write(&file_path, "test content").unwrap();

    let patterns = vec!["test.txt".to_string()];
    let resolved = Task::resolve_file_globs(&patterns, temp_dir.path());

    assert_eq!(resolved.len(), 1);
    assert!(resolved[0].contains("test.txt"));
}

#[test]
fn test_resolve_file_globs_with_glob_pattern() {
    let temp_dir = TempDir::new().unwrap();
    std::fs::write(temp_dir.path().join("file1.rs"), "").unwrap();
    std::fs::write(temp_dir.path().join("file2.rs"), "").unwrap();
    std::fs::write(temp_dir.path().join("file3.txt"), "").unwrap();

    let patterns = vec!["*.rs".to_string()];
    let resolved = Task::resolve_file_globs(&patterns, temp_dir.path());

    // Should find both .rs files
    assert_eq!(resolved.len(), 2);
    assert!(resolved.iter().all(|p| p.ends_with(".rs")));
}

#[test]
fn test_resolve_file_globs_nonexistent() {
    let temp_dir = TempDir::new().unwrap();

    let patterns = vec!["nonexistent.txt".to_string()];
    let resolved = Task::resolve_file_globs(&patterns, temp_dir.path());

    // Should still return the path even if it doesn't exist
    assert_eq!(resolved.len(), 1);
    assert!(resolved[0].contains("nonexistent.txt"));
}

#[test]
fn test_evaluate_merged_envs_empty() {
    let temp_dir = TempDir::new().unwrap();
    let result = Task::evaluate_merged_envs(&HashMap::new(), &HashMap::new(), temp_dir.path());

    assert!(result.is_ok());
    assert!(result.unwrap().is_empty());
}

#[test]
fn test_evaluate_merged_envs_global_only() {
    let temp_dir = TempDir::new().unwrap();
    let mut global_envs = HashMap::new();
    global_envs.insert("GLOBAL_VAR".to_string(), "global_value".to_string());

    let result = Task::evaluate_merged_envs(&global_envs, &HashMap::new(), temp_dir.path());

    assert!(result.is_ok());
    let evaluated = result.unwrap();
    assert_eq!(evaluated.get("GLOBAL_VAR"), Some(&"global_value".to_string()));
}

#[test]
fn test_evaluate_merged_envs_task_overrides_global() {
    let temp_dir = TempDir::new().unwrap();
    let mut global_envs = HashMap::new();
    global_envs.insert("VAR".to_string(), "global".to_string());

    let mut task_envs = HashMap::new();
    task_envs.insert("VAR".to_string(), "task".to_string());

    let result = Task::evaluate_merged_envs(&global_envs, &task_envs, temp_dir.path());

    assert!(result.is_ok());
    let evaluated = result.unwrap();
    // Task-level should override global
    assert_eq!(evaluated.get("VAR"), Some(&"task".to_string()));
}

/// The merged (global + task) evaluation routes through the same evaluator, so a task env
/// can read its own inherited value here too.
#[test]
#[serial]
fn test_evaluate_merged_envs_self_reference_reads_inherited_value() {
    let temp_dir = TempDir::new().unwrap();
    let key = "OTTO_TEST_MERGED_SELF";
    unsafe {
        std::env::set_var(key, "from-shell");
    }

    let mut task_envs = HashMap::new();
    task_envs.insert(key.to_string(), format!("$(echo \"${{{key}:-fallback}}\")"));

    let result = Task::evaluate_merged_envs(&HashMap::new(), &task_envs, temp_dir.path());

    unsafe {
        std::env::remove_var(key);
    }
    assert_eq!(result.unwrap().get(key), Some(&"from-shell".to_string()));
}

#[test]
fn test_from_task_spec() {
    let task_spec = make_task_spec("test", vec!["build".to_string()], "echo test");

    let task = Task::from_task(&task_spec);

    assert_eq!(task.name, "test");
    assert_eq!(task.task_deps.len(), 1);
    assert_eq!(task.task_deps[0].task, "build");
    assert_eq!(task.action, "echo test");
}

#[test]
fn test_from_task_with_cwd() {
    let temp_dir = TempDir::new().unwrap();

    let mut task_spec = make_task_spec("test", vec![], "cat input.txt > output.txt");
    task_spec.input = vec!["input.txt".to_string()];
    task_spec.output = vec!["output.txt".to_string()];

    let task = Task::from_task_with_cwd(&task_spec, temp_dir.path());

    // File paths should be resolved relative to cwd
    assert!(task.file_deps[0].contains("input.txt"));
    assert!(task.output_deps[0].contains("output.txt"));
}

#[test]
fn test_from_task_with_global_envs() {
    let temp_dir = TempDir::new().unwrap();

    let mut global_envs = HashMap::new();
    global_envs.insert("GLOBAL_VAR".to_string(), "global_value".to_string());

    let task_spec = make_task_spec("test", vec![], "echo $GLOBAL_VAR");

    let task = Task::from_task_with_cwd_and_global_envs(&task_spec, temp_dir.path(), &global_envs);

    assert_eq!(task.envs.get("GLOBAL_VAR"), Some(&"global_value".to_string()));
}

#[test]
fn test_task_action_trimmed() {
    let task_spec = make_task_spec("test", vec![], "  \n  echo test  \n  ");

    let task = Task::from_task(&task_spec);

    // Action should be trimmed
    assert_eq!(task.action, "echo test");
}

#[test]
fn test_subtask_has_parent_field() {
    // Test that subtasks (names with colons) get parent field set
    let task_spec = make_task_spec("install:td", vec![], "echo test");
    let task = Task::from_task(&task_spec);

    assert_eq!(task.parent, Some("install".to_string()));
}

#[test]
fn test_regular_task_has_no_parent() {
    // Test that regular tasks (no colons) have parent = None
    let task_spec = make_task_spec("build", vec![], "echo build");
    let task = Task::from_task(&task_spec);

    assert_eq!(task.parent, None);
}

#[test]
fn test_nested_colon_parent() {
    // Test that nested colon names extract first segment as parent
    let task_spec = make_task_spec("group:sub:item", vec![], "echo nested");
    let task = Task::from_task(&task_spec);

    assert_eq!(task.parent, Some("group".to_string()));
}

/// The parser -> executor conversion is the single funnel every runtime path
/// goes through, so a field it drops is a field the scheduler never sees.
#[test]
fn test_from_parser_task_carries_tty() {
    let mut parser_task = crate::cli::parser::Task::new(
        "login".to_string(),
        vec![],
        vec![],
        vec![],
        HashMap::new(),
        HashMap::new(),
        "echo hi".to_string(),
    );
    parser_task.tty = true;

    let task: Task = parser_task.into();

    assert!(task.tty, "tty must survive the parser -> executor conversion");
}

/// And the spec -> executor path used by the non-parser entry points.
#[test]
fn test_from_task_spec_carries_tty() {
    let mut spec = make_task_spec("login", vec![], "echo hi");
    spec.tty = Some(true);
    assert!(Task::from_task(&spec).tty);

    let off = make_task_spec("plain", vec![], "echo hi");
    assert!(!Task::from_task(&off).tty, "an absent tty: must mean false");
}
