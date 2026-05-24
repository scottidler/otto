use daggy::Dag;
use eyre::Result;
use hex;
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::path::PathBuf;

use crate::cfg::config::Value;
use crate::cfg::edge::When;
use crate::cfg::env as env_eval;
use crate::cfg::task::TaskSpec;

pub type DAG<T> = Dag<T, (), u32>;

/// Runtime equivalent of `EdgeSpec` without the cosmetic serialization fields.
/// Carries the task name and the condition under which the edge is satisfied.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TaskEdge {
    pub task: String,
    pub when: When,
}

impl TaskEdge {
    pub fn new(task: impl Into<String>, when: When) -> Self {
        Self {
            task: task.into(),
            when,
        }
    }

    /// Construct a `TaskEdge` with default `When::Success`.
    pub fn success(task: impl Into<String>) -> Self {
        Self::new(task, When::Success)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Task {
    pub name: String,
    /// Parent task name for foreach subtasks (e.g., "install" for "install:td")
    pub parent: Option<String>,
    pub task_deps: Vec<TaskEdge>,
    pub file_deps: Vec<String>,
    pub output_deps: Vec<String>,
    pub envs: HashMap<String, String>,
    pub values: HashMap<String, Value>,
    pub action: String,
    pub hash: String,
    /// True for foreach virtual parent tasks. The scheduler short-circuits execution
    /// of these (empty action) and aggregates their subtasks' statuses to derive the
    /// parent's final status.
    pub is_virtual_parent: bool,
}

impl Task {
    #[must_use]
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        name: String,
        parent: Option<String>,
        task_deps: Vec<TaskEdge>,
        file_deps: Vec<String>,
        output_deps: Vec<String>,
        envs: HashMap<String, String>,
        values: HashMap<String, Value>,
        action: String,
    ) -> Self {
        let hash = calculate_hash(&action);
        Self {
            name,
            parent,
            task_deps,
            file_deps,
            output_deps,
            envs,
            values,
            action,
            hash,
            is_virtual_parent: false,
        }
    }

    #[must_use]
    pub fn from_task(task_spec: &TaskSpec) -> Self {
        let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));

        Self::from_task_with_cwd_and_global_envs(task_spec, &cwd, &HashMap::new())
    }

    #[must_use]
    pub fn from_task_with_cwd(task_spec: &TaskSpec, cwd: &std::path::Path) -> Self {
        Self::from_task_with_cwd_and_global_envs(task_spec, cwd, &HashMap::new())
    }

    #[must_use]
    pub fn from_task_with_cwd_and_global_envs(
        task_spec: &TaskSpec,
        cwd: &std::path::Path,
        global_envs: &HashMap<String, String>,
    ) -> Self {
        let name = task_spec.name.clone();
        let task_deps: Vec<TaskEdge> = task_spec
            .before
            .iter()
            .map(|e| TaskEdge::new(e.task.clone(), e.when))
            .collect();

        // Derive parent for subtasks (names with colons like "install:td")
        let parent = if name.contains(':') {
            name.split(':').next().map(|s| s.to_string())
        } else {
            None
        };

        // Resolve file globs from input to canonical paths using explicit cwd
        let file_deps = Self::resolve_file_globs(&task_spec.input, cwd);

        // Resolve output globs to canonical paths using explicit cwd
        let output_deps = Self::resolve_file_globs(&task_spec.output, cwd);

        let evaluated_envs = Self::evaluate_merged_envs(global_envs, &task_spec.envs, cwd).unwrap_or_else(|e| {
            eprintln!("Warning: Failed to evaluate environment variables for task '{name}': {e}");
            HashMap::new()
        });

        // Note: We do NOT add after tasks here since they depend on us, not vice versa
        // The after dependencies will be handled during DAG construction
        let values = HashMap::new();
        let action = task_spec.action.trim().to_string(); // Trim whitespace from script content
        let mut task = Self::new(
            name,
            parent,
            task_deps,
            file_deps,
            output_deps,
            evaluated_envs,
            values,
            action,
        );
        task.is_virtual_parent = task_spec.virtual_parent;
        task
    }

    /// Evaluate and merge environment variables from global and task-level sources
    fn evaluate_merged_envs(
        global_envs: &HashMap<String, String>,
        task_envs: &HashMap<String, String>,
        working_dir: &std::path::Path,
    ) -> Result<HashMap<String, String>> {
        let mut merged_envs = global_envs.clone();
        merged_envs.extend(task_envs.iter().map(|(k, v)| (k.clone(), v.clone())));

        let evaluated_merged = if merged_envs.is_empty() {
            HashMap::new()
        } else {
            env_eval::evaluate_envs(&merged_envs, Some(working_dir))?
        };

        Ok(evaluated_merged)
    }

    /// Resolve file glob patterns to canonical file paths
    fn resolve_file_globs(patterns: &[String], cwd: &std::path::Path) -> Vec<String> {
        let mut resolved_files = Vec::new();

        for pattern in patterns {
            // Convert pattern to absolute path using provided cwd
            let pattern_path = if std::path::Path::new(pattern).is_absolute() {
                pattern.clone()
            } else {
                cwd.join(pattern).to_string_lossy().to_string()
            };

            // Use glob to expand patterns
            match glob::glob(&pattern_path) {
                Ok(paths) => {
                    let mut found_files = false;
                    for path in paths.flatten() {
                        found_files = true;
                        if let Ok(canonical) = path.canonicalize() {
                            resolved_files.push(canonical.to_string_lossy().to_string());
                        } else {
                            resolved_files.push(path.to_string_lossy().to_string());
                        }
                    }

                    // If glob succeeded but found no files, convert to absolute path anyway
                    if !found_files {
                        let abs_path = if std::path::Path::new(pattern).is_absolute() {
                            pattern.clone()
                        } else {
                            cwd.join(pattern).to_string_lossy().to_string()
                        };
                        resolved_files.push(abs_path);
                    }
                }
                Err(_) => {
                    // If glob fails, convert to absolute path anyway
                    let abs_path = if std::path::Path::new(pattern).is_absolute() {
                        pattern.clone()
                    } else {
                        cwd.join(pattern).to_string_lossy().to_string()
                    };
                    resolved_files.push(abs_path);
                }
            }
        }

        resolved_files
    }
}

fn calculate_hash(action: &String) -> String {
    let mut hasher = Sha256::new();
    hasher.update(action.as_bytes());
    let result = hasher.finalize();
    hex::encode(result)[..8].to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cfg::param::ParamSpecs;
    use tempfile::TempDir;

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
}
