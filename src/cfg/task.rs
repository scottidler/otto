//#![allow(unused_imports, unused_variables, dead_code)]

use eyre::{Result, eyre};
use serde::de::{Deserializer, Error as DeError, MapAccess, Visitor};
use serde::ser::{SerializeMap, Serializer};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fmt;
use std::path::Path;
use std::vec::Vec;

use indexmap::IndexMap;

use crate::cfg::edge::EdgeSpec;
use crate::cfg::param::{ParamMapSerializer, ParamSpecs, deserialize_param_map};

// IndexMap, not HashMap: preserves author order, so serializing one config
// twice emits tasks in the same order both times. A HashMap here made five
// serializes of one 5-task config produce five distinct orders (reproduced),
// which is both a noisy diff on every re-emit and a determinism gap in any
// test asserting on serialized output.
pub type TaskSpecs = IndexMap<String, TaskSpec>;

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
            command: None,
            var_name: default_as(),
            parallel: default_parallel(),
            max_items: default_max_items(),
            buffer: false,
        }
    }
}

/// Configuration for foreach-based subtask generation
///
/// `deny_unknown_fields` turns a stale or misplaced `foreach:` key (e.g.
/// `parallel:` written here instead of one level up, under the task) into a
/// loud config-load error naming the field, rather than a silently-ignored
/// no-op. Per `borg/src/config.rs:281-285`.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
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

    /// Shell command whose stdout lines are the items. Mutually exclusive with
    /// `glob`/`items`/`range`. Unlike the other three sources this one executes,
    /// so it resolves lazily and at most once per invocation - never for `--help`
    /// (see docs/design/2026-08-28-boundary-fixes-and-dynamic-foreach.md, Phase 6).
    #[serde(default)]
    pub command: Option<String>,

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

    /// Run subtasks concurrently but print each subtask's output as one
    /// contiguous block, in foreach item order (design doc
    /// `2026-08-31-buffered-foreach-computed-envs-required-params.md`, Phase 3).
    /// This phase is schema-only: the key loads and is validated, but nothing
    /// yet changes emission order (Phase 4). `buffer: true` combined with
    /// `tty: true` on the same task is rejected at load
    /// (`Parser::validate_foreach_buffer`): a tty task owns the terminal
    /// exclusively, so there is nothing to buffer.
    #[serde(default)]
    pub buffer: bool,
}

/// Represents a single item from foreach expansion
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ForeachItem {
    /// The identifier used in subtask naming (e.g., "01-basic.sh")
    pub identifier: String,
    /// The full value passed to the script (e.g., "examples/01-basic.sh")
    pub value: String,
}

/// Reduce a foreach item to something safe to use as one path component.
///
/// A subtask is named `<parent>:<identifier>` and that name becomes a directory
/// under the run's `tasks/`. The only sanitizing that existed was space to
/// underscore, so an item of `../../../ESCAPED` produced the directory
/// `tasks/build:../../../ESCAPED`, which the kernel resolves - the run wrote
/// outside its own `tasks/` tree. Separators and whitespace collapse to `_`
/// (slugified rather than rejected, because a command source listing paths is a
/// legitimate foreach; duplicate identifiers are already a loud error), and an
/// identifier made only of dots would still be a traversal or a self-reference,
/// so it is replaced outright.
#[must_use]
pub fn sanitize_identifier(raw: &str) -> String {
    let slug: String = raw
        .chars()
        .map(|c| {
            if c == '/' || c == '\\' || c.is_whitespace() || c.is_control() {
                '_'
            } else {
                c
            }
        })
        .collect();

    if slug.is_empty() || slug.chars().all(|c| c == '.') {
        "_".to_string()
    } else {
        slug
    }
}

/// Escape a value that must survive `cfg::env` evaluation unchanged.
///
/// A foreach item is data produced by a glob, a list, a range or a command - it
/// is not an expression the ottofile author wrote. Injected raw, an item
/// containing `${IFS}` aborted the whole task's environment with `Environment
/// variable 'IFS' not found`, and one containing `$(...)` would have run. `$$`
/// is the evaluator's literal-dollar escape, honored by both its stages.
fn escape_literal_env_value(value: &str) -> String {
    value.replace('$', "$$")
}

/// Interpolate a foreach item's value into a subtask's `input`/`output` paths,
/// the same `${var_name}` / `$var_name` form the item is injected into `envs`
/// under.
///
/// Only `var_name` resolves here. Every other reference is left standing, in
/// `${NAME}` form, for the environment pass in `Task::from_task_*` to resolve
/// against the task's evaluated envs - `examples/environment-variables/otto.yml`
/// ships `output: ["${BUILD_DIR}/${PROJECT_NAME}"]`, so global variables in
/// paths are a documented feature and this function must not consume them.
/// Erroring on them here made a foreach task reject what a plain task accepted.
fn interpolate_foreach_paths(task_name: &str, paths: &[String], var_name: &str, value: &str) -> Result<Vec<String>> {
    paths
        .iter()
        .map(|path| {
            crate::cfg::env::expand_var_refs(path, |name| {
                if name == var_name {
                    Some(value.to_string())
                } else {
                    // Re-emitted braced so the next pass sees the same reference,
                    // whichever form it was written in.
                    Some(format!("${{{name}}}"))
                }
            })
            .map_err(|e| eyre!("Task '{task_name}' foreach path '{path}': {e}"))
        })
        .collect()
}

/// Expand a task's evaluated environment into its `input`/`output` paths.
///
/// Paths used to expand nothing, so `input: ["${SRCDIR}/a.txt"]` matched no file,
/// the task tracked no inputs and re-ran on every invocation while reporting
/// success - a silent failure of the up-to-date check, in a form the shipped
/// environment-variables example demonstrates as a feature. An unresolvable
/// reference is an error here, per Phase 6 bullet 10's recorded decision to
/// prefer the error over a warning.
///
/// It happens at task construction, not config load, and that is deliberate: a
/// path may legitimately reference a variable from the task's OWN `envs:`, which
/// are not merged with the globals until this point. Validating earlier would
/// have to guess and would reject valid ottofiles. The cost is that `otto --help`
/// and `otto --list-subtasks` do not build tasks and so do not report it.
pub(crate) fn expand_env_in_paths(
    task_name: &str,
    field: &str,
    paths: &[String],
    envs: &HashMap<String, String>,
) -> Result<Vec<String>> {
    paths
        .iter()
        .map(|path| {
            crate::cfg::env::expand_var_refs(path, |name| envs.get(name).cloned()).map_err(|e| {
                eyre!(
                    "Task '{task_name}' {field} path '{path}': {e}\n\
                     Hint: input/output paths expand otto.envs and task envs. \
                     Define the variable, or use a literal path."
                )
            })
        })
        .collect()
}

impl ForeachSpec {
    /// True when the items come from a command's stdout rather than from the
    /// ottofile itself. Command sources execute, so they resolve lazily and
    /// only through the parser's `DynamicResolver`.
    #[must_use]
    pub fn is_command_source(&self) -> bool {
        self.command.is_some()
    }

    /// Reject a `command:` source combined with any static source. Loud config
    /// error naming the task, checked at load time so `otto --help` reports it
    /// too (validating the shape executes nothing).
    pub fn validate_sources(&self, task_name: &str) -> Result<()> {
        let Some(command) = &self.command else {
            return Ok(());
        };
        let mut combined: Vec<&str> = Vec::new();
        if self.glob.is_some() {
            combined.push("glob");
        }
        if !self.items.is_empty() {
            combined.push("items");
        }
        if self.range.is_some() {
            combined.push("range");
        }
        if !combined.is_empty() {
            return Err(eyre!(
                "Task '{}': foreach command '{}' cannot be combined with {}; \
                 foreach takes exactly one source (command, glob, items, or range)",
                task_name,
                command,
                combined.join(", ")
            ));
        }
        Ok(())
    }

    /// Resolve a static (glob / items / range) foreach source into a list of items.
    ///
    /// A command source is NOT resolvable here: it needs a task name (for the
    /// recursion guard and error messages) and the resolved global envs, which
    /// only the parser's resolver has. See `resolve_command_items`.
    pub fn resolve_items(&self, cwd: &Path) -> Result<Vec<ForeachItem>> {
        let items = if let Some(command) = &self.command {
            return Err(eyre!(
                "foreach command '{}' must be resolved through otto's foreach resolver \
                 (internal error: reached a call site with no command context)",
                command
            ));
        } else if let Some(glob_pattern) = &self.glob {
            self.resolve_glob(glob_pattern, cwd)?
        } else if !self.items.is_empty() {
            self.resolve_list()
        } else if let Some(range) = &self.range {
            self.resolve_range(range)?
        } else {
            return Err(eyre!("foreach requires glob, items, or range"));
        };

        self.check_max_items(&items)?;

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

    /// Resolve a `command:` foreach source: run the command and turn its
    /// non-empty stdout lines into items.
    ///
    /// Contract (design doc Phase 6): `sh -c`, cwd = the ottofile's directory,
    /// env = inherited environment + `envs` (the resolved global `envs:`); task
    /// params are NOT available, because params resolve after expansion. A
    /// non-zero exit is a loud error naming the task and the command; zero
    /// lines with exit 0 is a legitimate empty scope that prints one notice on
    /// stderr and expands to zero subtasks.
    pub fn resolve_command_items(
        &self,
        task_name: &str,
        cwd: &Path,
        envs: &HashMap<String, String>,
    ) -> Result<Vec<ForeachItem>> {
        self.validate_sources(task_name)?;
        let command = self
            .command
            .as_ref()
            .ok_or_else(|| eyre!("Task '{}': foreach has no command source", task_name))?;

        let context = format!("Task '{task_name}' foreach");
        let lines = crate::cfg::resolver::run_lines_command(
            command,
            cwd,
            envs,
            crate::cfg::resolver::FOREACH_GUARD_VAR,
            task_name,
            &context,
        )?;

        let items: Vec<ForeachItem> = lines
            .into_iter()
            .map(|line| ForeachItem {
                // Sanitized exactly as glob identifiers are.
                identifier: sanitize_identifier(&line),
                value: line,
            })
            .collect();

        self.check_max_items(&items)?;

        if items.is_empty() {
            eprintln!("Notice: task '{task_name}' foreach command '{command}' produced no items");
        }

        Ok(items)
    }

    fn check_max_items(&self, items: &[ForeachItem]) -> Result<()> {
        if items.len() > self.max_items {
            return Err(eyre!(
                "foreach matched {} items, exceeding max_items limit ({})",
                items.len(),
                self.max_items
            ));
        }
        Ok(())
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

                    // Sanitize identifier: it becomes one path component
                    let identifier = sanitize_identifier(&identifier);

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
                identifier: sanitize_identifier(item),
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
    /// Give this task the terminal: inherit stdout/stderr instead of capturing
    /// them, drop the `[task]` prefix, and run it exclusively (no other task runs
    /// alongside it). `None` and `Some(false)` are the same behavior; the option
    /// exists so an absent `tty:` key round-trips as absent.
    pub tty: Option<bool>,
}

// Helper struct for deserialization that accepts bash:, python:, or action: fields
//
// `deny_unknown_fields` turns a stale or misplaced task-level key (e.g.
// `parallel:` written beside `foreach:` instead of inside it) into a loud
// config-load error naming the field, rather than a silently-ignored no-op.
// Per `borg/src/config.rs:281-285`. `TaskSpec`'s hand-written `Deserialize`
// impl below does nothing but delegate to this helper, so the attribute
// lives here rather than on `TaskSpec` itself. Does not reach `envs`' or
// `params`' free-form keys: the attribute governs the helper's own field
// names, not the contents of the maps those fields hold.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
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

    // Opt-in interactivity: hand this task the terminal.
    #[serde(default)]
    tty: Option<bool>,
}

impl<'de> Deserialize<'de> for TaskSpec {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let helper = TaskSpecHelper::deserialize(deserializer)?;

        // A task naming more than one action source silently picked
        // `bash:` and ignored the rest (`if let Some(bash) ... else if let
        // Some(python) ...`), so a task with both `bash:` and `python:` ran
        // the bash script with no warning that the python one was dead.
        // Fail-closed: name every source that was present.
        let action_sources: Vec<&str> = [
            helper.bash.is_some().then_some("bash"),
            helper.python.is_some().then_some("python"),
            helper.action.is_some().then_some("action"),
        ]
        .into_iter()
        .flatten()
        .collect();
        if action_sources.len() > 1 {
            return Err(DeError::custom(format!(
                "task declares more than one action source ({}); a task takes exactly one of \
                 `bash:`, `python:`, or `action:`",
                action_sources.join(", ")
            )));
        }

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
            tty: helper.tty,
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

        // Emitted only when the key was present: `None` is "not written", not
        // "false", so an ottofile without `tty:` round-trips without gaining one.
        if let Some(tty) = self.tty {
            map.serialize_entry("tty", &tty)?;
        }

        // Serialize action as "bash:"/"python:" only when the auto-added bare
        // shebang (no args) is the whole first line. A prefix match here used
        // to also match a shebang WITH ARGS (e.g. "#!/bin/bash -euo pipefail"),
        // stripping only the "#!/bin/bash" substring and leaving the args
        // stranded as a mangled first line ("bash: |2-" / " -euo pipefail" /
        // "  echo hello") that reparses to a different action. A shebang line
        // carrying anything beyond the bare interpreter is user content, not
        // sugar this serializer added, so it stays put and falls through to
        // the verbatim "action:" key, which round-trips byte-identically.
        if !self.action.is_empty() {
            let trimmed = self.action.trim_start();
            if let Some(bash_script) = trimmed.strip_prefix("#!/bin/bash\n") {
                map.serialize_entry("bash", bash_script)?;
            } else if trimmed == "#!/bin/bash" {
                map.serialize_entry("bash", "")?;
            } else if let Some(python_script) = trimmed.strip_prefix("#!/usr/bin/env python3\n") {
                map.serialize_entry("python", python_script)?;
            } else if trimmed == "#!/usr/bin/env python3" {
                map.serialize_entry("python", "")?;
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

    // Minimum indentation in *characters*, not bytes. A byte offset derived
    // from one line's leading whitespace is not guaranteed to be a char
    // boundary in another line's: U+2002 (EN SPACE) alone is 3 bytes, so a
    // byte-indent of 2 (from an ascii-space-indented sibling line) sliced
    // into the middle of it and panicked ("byte index 2 is not a char
    // boundary"). Counting and skipping by chars sidesteps that entirely.
    let min_indent = lines
        .iter()
        .filter(|line| !line.trim().is_empty())
        .map(|line| line.chars().take_while(|c| c.is_whitespace()).count())
        .min()
        .unwrap_or(0);

    let dedented: Vec<String> = lines
        .iter()
        .map(|line| {
            if line.chars().count() > min_indent {
                line.chars().skip(min_indent).collect()
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
            tty: None,
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
        log::debug!("cfg::expand_foreach: task={} cwd={cwd:?}", self.name);
        let foreach = match &self.foreach {
            Some(f) => f,
            None => return Ok(vec![self.clone()]),
        };

        let items = foreach.resolve_items(cwd)?;
        self.expand_foreach_with_items(&items)
    }

    /// Expand a foreach task over already-resolved items.
    ///
    /// The parser uses this so a command source resolves exactly once per
    /// invocation (through the resolver cache) instead of once per call site;
    /// the guards after this point - duplicate identifiers, variable injection -
    /// are identical for every source.
    pub fn expand_foreach_with_items(&self, items: &[ForeachItem]) -> Result<Vec<TaskSpec>> {
        log::debug!(
            "cfg::expand_foreach_with_items: task={} items={}",
            self.name,
            items.len()
        );
        let foreach = match &self.foreach {
            Some(f) => f,
            None => return Ok(vec![self.clone()]),
        };

        // Sanitize here as well as at each source: this is the site that turns an
        // identifier into a task name, and a `ForeachItem` built anywhere else
        // (a test fixture, a future source) must not be able to skip the rule.
        let identifiers: Vec<String> = items.iter().map(|item| sanitize_identifier(&item.identifier)).collect();

        // Check for duplicate identifiers
        let mut seen_identifiers = std::collections::HashSet::new();
        for identifier in &identifiers {
            if !seen_identifiers.insert(identifier) {
                return Err(eyre!(
                    "foreach produced duplicate subtask name '{}:{}'",
                    self.name,
                    identifier
                ));
            }
        }

        items
            .iter()
            .zip(identifiers)
            .enumerate()
            .map(|(index, (item, identifier))| {
                let mut subtask = self.clone();
                subtask.name = crate::naming::subtask_name(&self.name, &identifier);
                subtask.foreach = None; // Prevent recursive expansion

                // Inject foreach variables into environment. The item is data, so
                // it is escaped against the env evaluator rather than handed to it
                // as an expression.
                let literal_value = escape_literal_env_value(&item.value);
                subtask.envs.insert(foreach.var_name.clone(), literal_value.clone());
                subtask.envs.insert("OTTO_FOREACH_ITEM".to_string(), literal_value);
                subtask.envs.insert("OTTO_FOREACH_INDEX".to_string(), index.to_string());

                // `input`/`output` are cloned by `self.clone()` above and, unlike
                // `envs`, never rewritten per item - so a pattern like
                // `input: ["src/${item}.txt"]` reached the up-to-date check as the
                // literal string `src/${item}.txt`, matching no file, and a static
                // shared path made every subtask's cache entry all-or-nothing.
                // Interpolate the same `var_name` into both lists, same as it is
                // injected into `envs`. An unexpandable reference in a path is a
                // config-load error (fail-closed, consistent with
                // `deny_unknown_fields`), not a silent no-op or a warning.
                subtask.input = interpolate_foreach_paths(&self.name, &subtask.input, &foreach.var_name, &item.value)?;
                subtask.output =
                    interpolate_foreach_paths(&self.name, &subtask.output, &foreach.var_name, &item.value)?;

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
            // The parent runs no script, so it wants no terminal. `tty:` on a
            // foreach task is inherited by each subtask (they are clones of the
            // spec); the aggregating parent must not hold the exclusive permit
            // for a task that does nothing.
            tty: None,
        }
    }
}

/// Derive a task's display name from its params-map-style key (e.g.
/// `-g|--greeting`), the same rich-key idea `divine()` uses for params: the
/// long form wins if present, else the first segment with its leading
/// dashes stripped.
///
/// Has exactly one call site (`deserialize_task_map` below), and stays a
/// named function rather than being inlined there: `task_spec.name =
/// namify(&name)` reads better at the call site than the
/// `split('|')`/`map_or_else` chain would inline to, and keeping it named
/// keeps its own unit test (`test_namify`) attached to the behavior instead
/// of losing coverage to a config-level test that would only exercise it
/// indirectly.
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

#[path = "task_tests.rs"]
mod foreach_tests;
