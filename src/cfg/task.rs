//#![allow(unused_imports, unused_variables, dead_code)]

use eyre::{Result, eyre};
use serde::de::{Deserializer, MapAccess, Visitor};
use serde::ser::{SerializeMap, Serializer};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fmt;
use std::path::Path;
use std::vec::Vec;

use crate::cfg::edge::EdgeSpec;
use crate::cfg::param::{ParamMapSerializer, ParamSpecs, deserialize_param_map};

pub type TaskSpecs = HashMap<String, TaskSpec>;

// ============================================================================
// ForeachSpec - Configuration for dynamic subtask generation
// ============================================================================

fn default_as() -> String {
    "item".to_string()
}

fn default_parallel() -> bool {
    true
}

fn default_max_items() -> usize {
    1000
}

impl Default for ForeachSpec {
    fn default() -> Self {
        Self {
            glob: None,
            items: Vec::new(),
            range: None,
            var_name: default_as(),
            parallel: default_parallel(),
            max_items: default_max_items(),
        }
    }
}

/// Configuration for foreach-based subtask generation
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub struct ForeachSpec {
    /// Glob pattern to match files
    #[serde(default)]
    pub glob: Option<String>,

    /// Explicit list of items
    #[serde(default)]
    pub items: Vec<String>,

    /// Numeric range (e.g., "1-10" for 1 through 10 inclusive)
    #[serde(default)]
    pub range: Option<String>,

    /// Variable name for the current item (default: "item")
    #[serde(default = "default_as")]
    #[serde(rename = "as")]
    pub var_name: String,

    /// Whether subtasks run in parallel (default: true)
    #[serde(default = "default_parallel")]
    pub parallel: bool,

    /// Maximum number of items before erroring (default: 1000)
    #[serde(default = "default_max_items")]
    pub max_items: usize,
}

/// Represents a single item from foreach expansion
#[derive(Clone, Debug)]
pub struct ForeachItem {
    /// The identifier used in subtask naming (e.g., "01-basic.sh")
    pub identifier: String,
    /// The full value passed to the script (e.g., "examples/01-basic.sh")
    pub value: String,
}

impl ForeachSpec {
    /// Resolve the foreach source into a list of items
    pub fn resolve_items(&self, cwd: &Path) -> Result<Vec<ForeachItem>> {
        let items = if let Some(glob_pattern) = &self.glob {
            self.resolve_glob(glob_pattern, cwd)?
        } else if !self.items.is_empty() {
            self.resolve_list()
        } else if let Some(range) = &self.range {
            self.resolve_range(range)?
        } else {
            return Err(eyre!("foreach requires glob, items, or range"));
        };

        // Check max_items limit
        if items.len() > self.max_items {
            return Err(eyre!(
                "foreach matched {} items, exceeding max_items limit ({})",
                items.len(),
                self.max_items
            ));
        }

        // Warn if zero items
        if items.is_empty() {
            log::warn!(
                "foreach {} matched 0 items",
                self.glob
                    .as_ref()
                    .or(self.range.as_ref())
                    .unwrap_or(&"items".to_string())
            );
        }

        Ok(items)
    }

    fn resolve_glob(&self, pattern: &str, cwd: &Path) -> Result<Vec<ForeachItem>> {
        let full_pattern = if Path::new(pattern).is_absolute() {
            pattern.to_string()
        } else {
            cwd.join(pattern).to_string_lossy().to_string()
        };

        let mut items: Vec<ForeachItem> = Vec::new();

        for entry in glob::glob(&full_pattern).map_err(|e| eyre!("Invalid glob pattern '{}': {}", pattern, e))? {
            match entry {
                Ok(path) => {
                    // Use filename as identifier, full path as value
                    let identifier = path
                        .file_name()
                        .map(|s| s.to_string_lossy().to_string())
                        .unwrap_or_else(|| path.to_string_lossy().to_string());

                    // Sanitize identifier: replace whitespace with underscore
                    let identifier = identifier.replace(' ', "_");

                    let value = path.to_string_lossy().to_string();

                    items.push(ForeachItem { identifier, value });
                }
                Err(e) => {
                    log::warn!("Failed to resolve glob entry: {}", e);
                }
            }
        }

        // Sort alphabetically for deterministic ordering
        items.sort_by(|a, b| a.identifier.cmp(&b.identifier));

        Ok(items)
    }

    fn resolve_list(&self) -> Vec<ForeachItem> {
        self.items
            .iter()
            .filter(|item| !item.trim().is_empty())
            .map(|item| ForeachItem {
                identifier: item.clone(),
                value: item.clone(),
            })
            .collect()
    }

    fn resolve_range(&self, range: &str) -> Result<Vec<ForeachItem>> {
        // Parse range format: supports "start..end" (Rust-like) or "start-end" (inclusive)
        let (start_str, end_str) = if range.contains("..") {
            // Rust-like format: "1..10"
            let parts: Vec<&str> = range.split("..").collect();
            if parts.len() != 2 {
                return Err(eyre!(
                    "Invalid range format '{}'. Expected 'start..end' (e.g., '1..10')",
                    range
                ));
            }
            (parts[0], parts[1])
        } else {
            // Hyphen format: "1-10"
            let parts: Vec<&str> = range.split('-').collect();
            if parts.len() != 2 {
                return Err(eyre!(
                    "Invalid range format '{}'. Expected 'start..end' or 'start-end' (e.g., '1..10' or '1-10')",
                    range
                ));
            }
            (parts[0], parts[1])
        };

        let start: usize = start_str
            .trim()
            .parse()
            .map_err(|_| eyre!("Invalid range start: '{}'", start_str))?;
        let end: usize = end_str
            .trim()
            .parse()
            .map_err(|_| eyre!("Invalid range end: '{}'", end_str))?;

        if start > end {
            return Err(eyre!("Invalid range: start ({}) > end ({})", start, end));
        }

        // Calculate padding width for zero-padding
        let width = end.to_string().len();

        Ok((start..=end)
            .map(|n| {
                let identifier = format!("{:0width$}", n, width = width);
                let value = n.to_string();
                ForeachItem { identifier, value }
            })
            .collect())
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TaskSpec {
    pub name: String,
    pub help: Option<String>,
    pub after: Vec<EdgeSpec>,
    pub before: Vec<EdgeSpec>,
    pub input: Vec<String>,
    pub output: Vec<String>,
    pub envs: HashMap<String, String>,
    pub params: ParamSpecs,
    pub action: String,
    /// Optional foreach configuration for subtask generation
    pub foreach: Option<ForeachSpec>,
    /// True for foreach-created virtual parent tasks (no action, just dependency tracking)
    pub virtual_parent: bool,
    /// Tasks named here fire when this task fails (parse-time sugar that desugars
    /// into `after:` edges with `when: failure` on the named target tasks).
    /// Preserved verbatim for round-trip serialization.
    pub on_failure: Vec<String>,
}

// Helper struct for deserialization that accepts bash:, python:, or action: fields
#[derive(Debug, Deserialize)]
struct TaskSpecHelper {
    #[serde(default)]
    help: Option<String>,

    #[serde(default)]
    after: Vec<EdgeSpec>,

    #[serde(default)]
    before: Vec<EdgeSpec>,

    #[serde(default)]
    input: Vec<String>,

    #[serde(default)]
    output: Vec<String>,

    #[serde(default)]
    envs: HashMap<String, String>,

    #[serde(default, deserialize_with = "deserialize_param_map")]
    params: ParamSpecs,

    // Support for new bash: field
    #[serde(default)]
    bash: Option<String>,

    // Support for new python: field
    #[serde(default)]
    python: Option<String>,

    // Legacy support for action: field (deprecated)
    #[serde(default)]
    action: Option<String>,

    // Support for foreach subtask generation
    #[serde(default)]
    foreach: Option<ForeachSpec>,

    // Sugar surface: tasks named here will be wired with when: failure edges
    // pointing back at this host task. Desugared by the parser before scheduling.
    #[serde(default, rename = "on-failure")]
    on_failure: Vec<String>,
}

impl<'de> Deserialize<'de> for TaskSpec {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let helper = TaskSpecHelper::deserialize(deserializer)?;

        let action = if let Some(bash_script) = helper.bash {
            let bash_script = deserialize_script_string(&bash_script);
            if bash_script.trim_start().starts_with("#!") {
                bash_script
            } else {
                format!("#!/bin/bash\n{bash_script}")
            }
        } else if let Some(python_script) = helper.python {
            let python_script = deserialize_script_string(&python_script);
            if python_script.trim_start().starts_with("#!") {
                python_script
            } else {
                format!("#!/usr/bin/env python3\n{python_script}")
            }
        } else if let Some(action_script) = helper.action {
            deserialize_script_string(&action_script)
        } else {
            String::new()
        };

        Ok(TaskSpec {
            name: String::new(), // Will be set by deserialize_task_map
            help: helper.help,
            after: helper.after,
            before: helper.before,
            input: helper.input,
            output: helper.output,
            envs: helper.envs,
            params: helper.params,
            action,
            foreach: helper.foreach,
            virtual_parent: false,
            on_failure: helper.on_failure,
        })
    }
}

impl Serialize for TaskSpec {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut map = serializer.serialize_map(None)?;

        if let Some(ref help) = self.help {
            map.serialize_entry("help", help)?;
        }

        // Filter out injected sugar edges - those are serialized via the host's
        // `on-failure:` field, not the target's `after:` list.
        let visible_after: Vec<&EdgeSpec> = self.after.iter().filter(|e| !e.is_injected_sugar).collect();
        if !visible_after.is_empty() {
            map.serialize_entry("after", &visible_after)?;
        }

        let visible_before: Vec<&EdgeSpec> = self.before.iter().filter(|e| !e.is_injected_sugar).collect();
        if !visible_before.is_empty() {
            map.serialize_entry("before", &visible_before)?;
        }

        if !self.on_failure.is_empty() {
            map.serialize_entry("on-failure", &self.on_failure)?;
        }

        if !self.input.is_empty() {
            map.serialize_entry("input", &self.input)?;
        }

        if !self.output.is_empty() {
            map.serialize_entry("output", &self.output)?;
        }

        if !self.envs.is_empty() {
            map.serialize_entry("envs", &self.envs)?;
        }

        if !self.params.is_empty() {
            map.serialize_entry("params", &ParamMapSerializer(&self.params))?;
        }

        if let Some(ref foreach) = self.foreach {
            map.serialize_entry("foreach", foreach)?;
        }

        // Serialize action as "bash:" if it starts with #!/bin/bash
        if !self.action.is_empty() {
            if self.action.trim_start().starts_with("#!/bin/bash") {
                let bash_script = self
                    .action
                    .trim_start()
                    .strip_prefix("#!/bin/bash\n")
                    .or_else(|| self.action.trim_start().strip_prefix("#!/bin/bash"))
                    .unwrap_or(&self.action);
                map.serialize_entry("bash", bash_script)?;
            } else if self.action.trim_start().starts_with("#!/usr/bin/env python3") {
                let python_script = self
                    .action
                    .trim_start()
                    .strip_prefix("#!/usr/bin/env python3\n")
                    .or_else(|| self.action.trim_start().strip_prefix("#!/usr/bin/env python3"))
                    .unwrap_or(&self.action);
                map.serialize_entry("python", python_script)?;
            } else {
                map.serialize_entry("action", &self.action)?;
            }
        }

        map.end()
    }
}

fn deserialize_script_string(s: &str) -> String {
    // For block scalars, preserve the exact content but trim any common indentation
    let lines: Vec<&str> = s.lines().collect();

    // Find minimum indentation (ignoring empty lines)
    let min_indent = lines
        .iter()
        .filter(|line| !line.trim().is_empty())
        .map(|line| line.len() - line.trim_start().len())
        .min()
        .unwrap_or(0);

    let dedented: Vec<String> = lines
        .iter()
        .map(|line| {
            if line.len() > min_indent {
                line[min_indent..].to_string()
            } else {
                line.to_string()
            }
        })
        .collect();

    // Join lines and trim any leading/trailing empty lines
    let result = dedented.join("\n");
    result.trim_start().trim_end().to_string()
}

impl TaskSpec {
    #[must_use]
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        name: String,
        help: Option<String>,
        after: Vec<EdgeSpec>,
        before: Vec<EdgeSpec>,
        input: Vec<String>,
        output: Vec<String>,
        envs: HashMap<String, String>,
        params: ParamSpecs,
        action: String,
    ) -> Self {
        Self {
            name,
            help,
            after,
            before,
            input,
            output,
            envs,
            params,
            action,
            foreach: None,
            virtual_parent: false,
            on_failure: Vec::new(),
        }
    }

    /// Check if this task has a foreach configuration
    #[must_use]
    pub fn has_foreach(&self) -> bool {
        self.foreach.is_some()
    }

    /// Expand a foreach task into multiple concrete subtasks.
    /// Returns the original task in a vec if there's no foreach configuration.
    pub fn expand_foreach(&self, cwd: &Path) -> Result<Vec<TaskSpec>> {
        let foreach = match &self.foreach {
            Some(f) => f,
            None => return Ok(vec![self.clone()]),
        };

        let items = foreach.resolve_items(cwd)?;

        // Check for duplicate identifiers
        let mut seen_identifiers = std::collections::HashSet::new();
        for item in &items {
            if !seen_identifiers.insert(&item.identifier) {
                return Err(eyre!(
                    "foreach produced duplicate subtask name '{}:{}'",
                    self.name,
                    item.identifier
                ));
            }
        }

        items
            .iter()
            .enumerate()
            .map(|(index, item)| {
                let mut subtask = self.clone();
                subtask.name = format!("{}:{}", self.name, item.identifier);
                subtask.foreach = None; // Prevent recursive expansion

                // Inject foreach variables into environment
                subtask.envs.insert(foreach.var_name.clone(), item.value.clone());
                subtask.envs.insert("OTTO_FOREACH_ITEM".to_string(), item.value.clone());
                subtask.envs.insert("OTTO_FOREACH_INDEX".to_string(), index.to_string());

                Ok(subtask)
            })
            .collect()
    }

    /// Create a virtual parent task (no action, just for dependency tracking)
    #[must_use]
    pub fn as_virtual_parent(&self) -> TaskSpec {
        TaskSpec {
            name: self.name.clone(),
            help: self.help.clone(),
            after: self.after.clone(),
            before: self.before.clone(),
            input: vec![],
            output: vec![],
            envs: HashMap::new(),
            params: ParamSpecs::new(),
            action: String::new(), // No action - virtual task
            foreach: None,
            virtual_parent: true,
            on_failure: self.on_failure.clone(),
        }
    }
}

fn namify(name: &str) -> String {
    name.split('|').find(|&part| part.starts_with("--")).map_or_else(
        || name.split('|').next().unwrap().trim_start_matches('-').to_string(),
        |s| s.trim_start_matches("--").to_string(),
    )
}

#[test]
fn test_namify() {
    assert_eq!(namify("-g|--greeting"), "greeting".to_string());
    assert_eq!(namify("-k"), "k".to_string());
    assert_eq!(namify("--name"), "name".to_string());
}

pub fn deserialize_task_map<'de, D>(deserializer: D) -> Result<TaskSpecs, D::Error>
where
    D: Deserializer<'de>,
{
    struct TaskMap;

    impl<'de> Visitor<'de> for TaskMap {
        type Value = TaskSpecs;

        fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
            formatter.write_str("a map of name to Task")
        }

        fn visit_map<M>(self, mut map: M) -> Result<Self::Value, M::Error>
        where
            M: MapAccess<'de>,
        {
            let mut tasks = TaskSpecs::new();
            while let Some((name, mut task_spec)) = map.next_entry::<String, TaskSpec>()? {
                task_spec.name = namify(&name);
                tasks.insert(name.clone(), task_spec);
            }
            Ok(tasks)
        }
    }
    deserializer.deserialize_map(TaskMap)
}

// ============================================================================
// Unit tests for foreach functionality
// ============================================================================

#[cfg(test)]
mod foreach_tests {
    use super::*;
    use std::path::PathBuf;
    use tempfile::TempDir;

    #[test]
    fn test_foreach_resolve_items_list() {
        let foreach = ForeachSpec {
            items: vec!["dev".to_string(), "staging".to_string(), "prod".to_string()],
            ..Default::default()
        };

        let cwd = PathBuf::from("/tmp");
        let items = foreach.resolve_items(&cwd).unwrap();

        assert_eq!(items.len(), 3);
        assert_eq!(items[0].identifier, "dev");
        assert_eq!(items[0].value, "dev");
        assert_eq!(items[1].identifier, "staging");
        assert_eq!(items[2].identifier, "prod");
    }

    #[test]
    fn test_foreach_resolve_items_range() {
        let foreach = ForeachSpec {
            range: Some("1-5".to_string()),
            ..Default::default()
        };

        let cwd = PathBuf::from("/tmp");
        let items = foreach.resolve_items(&cwd).unwrap();

        assert_eq!(items.len(), 5);
        assert_eq!(items[0].identifier, "1");
        assert_eq!(items[0].value, "1");
        assert_eq!(items[4].identifier, "5");
        assert_eq!(items[4].value, "5");
    }

    #[test]
    fn test_foreach_resolve_items_range_zero_padded() {
        let foreach = ForeachSpec {
            range: Some("1-12".to_string()),
            ..Default::default()
        };

        let cwd = PathBuf::from("/tmp");
        let items = foreach.resolve_items(&cwd).unwrap();

        assert_eq!(items.len(), 12);
        assert_eq!(items[0].identifier, "01"); // Zero-padded to match width of "12"
        assert_eq!(items[0].value, "1");
        assert_eq!(items[9].identifier, "10");
        assert_eq!(items[11].identifier, "12");
    }

    #[test]
    fn test_foreach_resolve_items_glob() {
        let temp_dir = TempDir::new().unwrap();
        let dir = temp_dir.path();

        // Create test files
        std::fs::write(dir.join("a.txt"), "").unwrap();
        std::fs::write(dir.join("b.txt"), "").unwrap();
        std::fs::write(dir.join("c.txt"), "").unwrap();
        std::fs::write(dir.join("skip.md"), "").unwrap(); // Should not match

        let foreach = ForeachSpec {
            glob: Some("*.txt".to_string()),
            ..Default::default()
        };

        let items = foreach.resolve_items(dir).unwrap();

        assert_eq!(items.len(), 3);
        // Should be sorted alphabetically
        assert_eq!(items[0].identifier, "a.txt");
        assert_eq!(items[1].identifier, "b.txt");
        assert_eq!(items[2].identifier, "c.txt");
    }

    #[test]
    fn test_foreach_max_items_limit() {
        let foreach = ForeachSpec {
            range: Some("1-100".to_string()),
            max_items: 10,
            ..Default::default()
        };

        let cwd = PathBuf::from("/tmp");
        let result = foreach.resolve_items(&cwd);

        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("exceeding max_items"));
    }

    #[test]
    fn test_foreach_empty_items_filtered() {
        let foreach = ForeachSpec {
            items: vec!["a".to_string(), "".to_string(), "  ".to_string(), "b".to_string()],
            ..Default::default()
        };

        let cwd = PathBuf::from("/tmp");
        let items = foreach.resolve_items(&cwd).unwrap();

        assert_eq!(items.len(), 2);
        assert_eq!(items[0].identifier, "a");
        assert_eq!(items[1].identifier, "b");
    }

    #[test]
    fn test_foreach_invalid_range_format() {
        let foreach = ForeachSpec {
            range: Some("invalid".to_string()),
            ..Default::default()
        };

        let cwd = PathBuf::from("/tmp");
        let result = foreach.resolve_items(&cwd);

        assert!(result.is_err());
    }

    #[test]
    fn test_foreach_range_start_greater_than_end() {
        let foreach = ForeachSpec {
            range: Some("10-5".to_string()),
            ..Default::default()
        };

        let cwd = PathBuf::from("/tmp");
        let result = foreach.resolve_items(&cwd);

        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("start (10) > end (5)"));
    }

    #[test]
    fn test_foreach_requires_source() {
        let foreach = ForeachSpec::default();

        let cwd = PathBuf::from("/tmp");
        let result = foreach.resolve_items(&cwd);

        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("foreach requires glob, items, or range"));
    }

    #[test]
    fn test_taskspec_expand_foreach_with_list() {
        let mut task = TaskSpec::new(
            "deploy".to_string(),
            Some("Deploy to environment".to_string()),
            vec![],
            vec![],
            vec![],
            vec![],
            HashMap::new(),
            ParamSpecs::new(),
            "#!/bin/bash\necho deploy".to_string(),
        );
        task.foreach = Some(ForeachSpec {
            items: vec!["dev".to_string(), "staging".to_string(), "prod".to_string()],
            var_name: "env".to_string(),
            ..Default::default()
        });

        let cwd = PathBuf::from("/tmp");
        let subtasks = task.expand_foreach(&cwd).unwrap();

        assert_eq!(subtasks.len(), 3);
        assert_eq!(subtasks[0].name, "deploy:dev");
        assert_eq!(subtasks[1].name, "deploy:staging");
        assert_eq!(subtasks[2].name, "deploy:prod");

        // Check environment variables
        assert_eq!(subtasks[0].envs.get("env"), Some(&"dev".to_string()));
        assert_eq!(subtasks[0].envs.get("OTTO_FOREACH_ITEM"), Some(&"dev".to_string()));
        assert_eq!(subtasks[0].envs.get("OTTO_FOREACH_INDEX"), Some(&"0".to_string()));

        assert_eq!(subtasks[2].envs.get("OTTO_FOREACH_INDEX"), Some(&"2".to_string()));

        // Subtasks should not have foreach
        assert!(subtasks[0].foreach.is_none());
    }

    #[test]
    fn test_taskspec_expand_foreach_none() {
        let task = TaskSpec::new(
            "build".to_string(),
            None,
            vec![],
            vec![],
            vec![],
            vec![],
            HashMap::new(),
            ParamSpecs::new(),
            "#!/bin/bash\necho build".to_string(),
        );

        let cwd = PathBuf::from("/tmp");
        let subtasks = task.expand_foreach(&cwd).unwrap();

        assert_eq!(subtasks.len(), 1);
        assert_eq!(subtasks[0].name, "build");
    }

    #[test]
    fn test_taskspec_as_virtual_parent() {
        let mut task = TaskSpec::new(
            "examples".to_string(),
            Some("Run examples".to_string()),
            vec![EdgeSpec::sugar("cleanup")],
            vec![EdgeSpec::sugar("build")],
            vec!["input.txt".to_string()],
            vec!["output.txt".to_string()],
            HashMap::from([("KEY".to_string(), "value".to_string())]),
            ParamSpecs::new(),
            "#!/bin/bash\necho hello".to_string(),
        );
        task.foreach = Some(ForeachSpec::default());

        let parent = task.as_virtual_parent();

        assert_eq!(parent.name, "examples");
        assert_eq!(parent.help, Some("Run examples".to_string()));
        assert_eq!(parent.after, vec![EdgeSpec::sugar("cleanup")]);
        assert_eq!(parent.before, vec![EdgeSpec::sugar("build")]);
        assert!(parent.input.is_empty());
        assert!(parent.output.is_empty());
        assert!(parent.envs.is_empty());
        assert!(parent.action.is_empty());
        assert!(parent.foreach.is_none());
        assert!(parent.virtual_parent);
    }

    #[test]
    fn test_foreach_yaml_deserialization() {
        let yaml = r#"
            help: "Run all examples"
            foreach:
              items: [a, b, c]
              as: example
              parallel: true
            bash: |
              echo ${example}
        "#;

        let task: TaskSpec = serde_yaml::from_str(yaml).unwrap();

        assert!(task.foreach.is_some());
        let foreach = task.foreach.unwrap();
        assert_eq!(foreach.items, vec!["a", "b", "c"]);
        assert_eq!(foreach.var_name, "example");
        assert!(foreach.parallel);
    }

    #[test]
    fn test_foreach_yaml_deserialization_with_glob() {
        let yaml = r#"
            foreach:
              glob: "examples/*.sh"
            bash: echo test
        "#;

        let task: TaskSpec = serde_yaml::from_str(yaml).unwrap();

        assert!(task.foreach.is_some());
        let foreach = task.foreach.unwrap();
        assert_eq!(foreach.glob, Some("examples/*.sh".to_string()));
        assert_eq!(foreach.var_name, "item"); // default
    }

    #[test]
    fn test_foreach_yaml_deserialization_with_range() {
        let yaml = r#"
            foreach:
              range: "1-10"
              as: num
            bash: echo ${num}
        "#;

        let task: TaskSpec = serde_yaml::from_str(yaml).unwrap();

        assert!(task.foreach.is_some());
        let foreach = task.foreach.unwrap();
        assert_eq!(foreach.range, Some("1-10".to_string()));
        assert_eq!(foreach.var_name, "num");
    }
}
