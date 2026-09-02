use daggy::Dag;
use eyre::Result;
use hex;
use sha2::{Digest, Sha256};
use std::collections::HashMap;

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
    /// Name of the serial foreach group this task belongs to (the parent task name),
    /// or `None` when the task carries no ordering constraint. The scheduler's ready
    /// loop gates each member on the nearest preceding member that is in the run set.
    pub serial_group: Option<String>,
    /// Position of this task within `serial_group`, in declared foreach order.
    /// Meaningless when `serial_group` is `None`.
    pub serial_index: usize,
    /// Give this task the terminal: stdout/stderr inherited rather than captured,
    /// no `[task]` prefix, and the whole semaphore held for its duration so no
    /// other task runs alongside it.
    pub tty: bool,
    /// For a foreach virtual parent (buffered or not): its subtask names, in
    /// declared item order. `None` for every non-foreach task. Additive;
    /// carried from `cli::parser::Task::foreach_display_order` and read only
    /// by the Phase 4 replay cursor (design doc
    /// `2026-08-31-buffered-foreach-computed-envs-required-params.md`, Phase 3).
    pub foreach_display_order: Option<Vec<String>>,
    /// True for a `foreach.buffer: true` parent and every one of its subtasks.
    /// On a subtask it suppresses the live terminal leg so the bytes reach only
    /// `stdout.log`/`stderr.log`; on the parent it marks the group the replay
    /// cursor owns (design doc
    /// `2026-08-31-buffered-foreach-computed-envs-required-params.md`, Phase 4).
    pub buffered: bool,
    /// For an ITEM of a foreach group that declared `foreach.jobs`: the permit
    /// count that group's own semaphore is built with (one per item under
    /// `jobs: all`). Its presence is what makes the item exempt from the
    /// scheduler's launch cap and from the shared semaphore; its value is what
    /// bounds the group instead. `None` for every other task, including the
    /// group's virtual parent (design doc
    /// `2026-09-01-cancellation-reaping-and-foreach-concurrency.md`, Phase 3).
    pub foreach_jobs: Option<std::num::NonZeroUsize>,
}

impl Task {
    /// The parent task a foreach subtask belongs to, derived from its name.
    ///
    /// Owned-`String` convenience over [`crate::naming::parent_of`], which is the
    /// single definition of the rule. This function used to *be* that definition
    /// and said so, and six open-coded copies grew anyway; it now delegates so
    /// there is nothing here to drift from.
    #[must_use]
    pub fn derive_parent(name: &str) -> Option<String> {
        crate::naming::parent_of(name).map(str::to_string)
    }

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
            serial_group: None,
            serial_index: 0,
            tty: false,
            foreach_display_order: None,
            buffered: false,
            foreach_jobs: None,
        }
    }

    pub fn from_task(task_spec: &TaskSpec) -> Result<Self> {
        // Was `unwrap_or_else(|_| PathBuf::from("."))`: the error was discarded
        // and a task silently ran against a relative path instead of the
        // directory the caller meant. The function already returns `Result`.
        let cwd = crate::executor::workspace::current_dir()?;

        Self::from_task_with_cwd_and_global_envs(task_spec, &cwd, &HashMap::new())
    }

    pub fn from_task_with_cwd(task_spec: &TaskSpec, cwd: &std::path::Path) -> Result<Self> {
        Self::from_task_with_cwd_and_global_envs(task_spec, cwd, &HashMap::new())
    }

    /// Fails closed on an unresolvable task environment, matching
    /// `cli::parser::Task::from_task_with_cwd_and_global_envs`. Dropping the map
    /// and running anyway let one cyclic key take every other key with it and
    /// still exit 0.
    pub fn from_task_with_cwd_and_global_envs(
        task_spec: &TaskSpec,
        cwd: &std::path::Path,
        global_envs: &HashMap<String, String>,
    ) -> Result<Self> {
        let name = task_spec.name.clone();
        let task_deps: Vec<TaskEdge> = task_spec
            .before
            .iter()
            .map(|e| TaskEdge::new(e.task.clone(), e.when))
            .collect();

        let parent = Self::derive_parent(&name);

        let evaluated_envs = Self::evaluate_merged_envs(global_envs, &task_spec.envs, cwd)
            .map_err(|e| eyre::eyre!("Failed to evaluate environment variables for task '{name}': {e}"))?;

        // Paths expand the task's evaluated environment before globbing: a
        // reference in an input/output path used to expand to nothing, so the
        // glob matched no file and the task silently never went up to date.
        let input_paths = crate::cfg::task::expand_env_in_paths(&name, "input", &task_spec.input, &evaluated_envs)?;
        let output_paths = crate::cfg::task::expand_env_in_paths(&name, "output", &task_spec.output, &evaluated_envs)?;
        let file_deps = Self::resolve_file_globs(&input_paths, cwd);
        let output_deps = Self::resolve_file_globs(&output_paths, cwd);

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
        task.tty = task_spec.tty.unwrap_or(false);
        Ok(task)
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
            env_eval::evaluate_envs(&merged_envs, Some(working_dir), &HashMap::new())?
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

/// Single source of truth for parser task -> executor task conversion. Every runtime
/// path (plain execution, TUI, graph) goes through here so a field added to one struct
/// can't be silently dropped on one of the paths.
impl From<crate::cli::parser::Task> for Task {
    fn from(parser_task: crate::cli::parser::Task) -> Self {
        let parent = Task::derive_parent(&parser_task.name);
        let mut task = Self::new(
            parser_task.name,
            parent,
            parser_task.task_deps,
            parser_task.file_deps,
            parser_task.output_deps,
            parser_task.envs,
            parser_task.values,
            parser_task.action,
        );
        task.is_virtual_parent = parser_task.is_virtual_parent;
        task.serial_group = parser_task.serial_group;
        task.serial_index = parser_task.serial_index;
        task.tty = parser_task.tty;
        task.foreach_display_order = parser_task.foreach_display_order;
        task.buffered = parser_task.buffered;
        task.foreach_jobs = parser_task.foreach_jobs;
        task
    }
}

fn calculate_hash(action: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(action.as_bytes());
    let result = hasher.finalize();
    hex::encode(result)[..8].to_string()
}

#[path = "task_tests.rs"]
mod tests;
