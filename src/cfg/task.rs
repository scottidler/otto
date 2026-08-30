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
/// under. Only `var_name` resolves; any other `${...}`/`$...` reference in a
/// path is an unexpandable variable and, per the design doc's fail-closed
/// direction, a loud config-load error naming the task rather than a warning
/// or a silent pass-through.
fn interpolate_foreach_paths(task_name: &str, paths: &[String], var_name: &str, value: &str) -> Result<Vec<String>> {
    paths
        .iter()
        .map(|path| {
            crate::cfg::env::expand_var_refs(path, |name| (name == var_name).then(|| value.to_string()))
                .map_err(|e| eyre!("Task '{task_name}' foreach path '{path}': {e}"))
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
                subtask.name = format!("{}:{}", self.name, identifier);
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

    // ------------------------------------------------------------------
    // Command source (design doc 2026-08-28, Phase 6)
    // ------------------------------------------------------------------

    fn command_foreach(command: &str) -> ForeachSpec {
        ForeachSpec {
            command: Some(command.to_string()),
            var_name: "svc".to_string(),
            ..Default::default()
        }
    }

    #[test]
    fn test_foreach_command_items_are_trimmed_non_empty_lines() {
        let foreach = command_foreach("printf 'alpha\n\n  beta  \n'");
        let items = foreach
            .resolve_command_items("up", &PathBuf::from("."), &HashMap::new())
            .unwrap();

        assert_eq!(items.len(), 2);
        assert_eq!(items[0].identifier, "alpha");
        assert_eq!(items[1].identifier, "beta");
        assert_eq!(items[1].value, "beta");
    }

    #[test]
    fn test_foreach_command_sanitizes_identifiers_like_glob_does() {
        let foreach = command_foreach("printf 'two words\n'");
        let items = foreach
            .resolve_command_items("up", &PathBuf::from("."), &HashMap::new())
            .unwrap();

        assert_eq!(items[0].identifier, "two_words");
        assert_eq!(items[0].value, "two words", "the value keeps the original spacing");
    }

    #[test]
    fn test_foreach_command_nonzero_exit_names_task_and_command() {
        let foreach = command_foreach("exit 7");
        let err = foreach
            .resolve_command_items("up", &PathBuf::from("."), &HashMap::new())
            .unwrap_err()
            .to_string();

        assert!(err.contains("up"), "{err}");
        assert!(err.contains("exit 7"), "{err}");
        assert!(err.contains("exit code 7"), "{err}");
    }

    #[test]
    fn test_foreach_command_zero_lines_is_an_empty_expansion_not_an_error() {
        let foreach = command_foreach("true");
        let items = foreach
            .resolve_command_items("up", &PathBuf::from("."), &HashMap::new())
            .unwrap();

        assert!(items.is_empty());
    }

    #[test]
    fn test_foreach_command_respects_max_items() {
        let foreach = ForeachSpec {
            max_items: 2,
            ..command_foreach("printf 'a\nb\nc\n'")
        };
        let err = foreach
            .resolve_command_items("up", &PathBuf::from("."), &HashMap::new())
            .unwrap_err()
            .to_string();

        assert!(err.contains("max_items"), "{err}");
    }

    #[test]
    fn test_foreach_command_is_exclusive_with_static_sources() {
        let foreach = ForeachSpec {
            items: vec!["x".to_string()],
            range: Some("1-2".to_string()),
            ..command_foreach("printf 'a\n'")
        };

        let err = foreach.validate_sources("up").unwrap_err().to_string();
        assert!(err.contains("Task 'up'"), "{err}");
        assert!(err.contains("items"), "{err}");
        assert!(err.contains("range"), "{err}");

        // and the same error blocks resolution, not just load-time validation
        assert!(
            foreach
                .resolve_command_items("up", &PathBuf::from("."), &HashMap::new())
                .is_err()
        );
    }

    #[test]
    fn test_foreach_static_sources_still_validate_clean() {
        let foreach = ForeachSpec {
            items: vec!["x".to_string()],
            glob: Some("*.sh".to_string()),
            ..Default::default()
        };
        // Only a `command:` source is exclusive; the pre-existing glob/items
        // precedence is untouched by this phase.
        assert!(foreach.validate_sources("up").is_ok());
    }

    #[test]
    fn test_resolve_items_refuses_a_command_source() {
        let foreach = command_foreach("printf 'a\n'");
        let err = foreach.resolve_items(&PathBuf::from(".")).unwrap_err().to_string();

        assert!(
            err.contains("must be resolved through otto's foreach resolver"),
            "{err}"
        );
    }

    #[test]
    fn test_expand_foreach_with_items_names_subtasks_and_injects_vars() {
        let mut task = TaskSpec::new(
            "up".to_string(),
            None,
            vec![],
            vec![],
            vec![],
            vec![],
            HashMap::new(),
            ParamSpecs::new(),
            "echo ${svc}".to_string(),
        );
        task.foreach = Some(command_foreach("unused: items are supplied"));

        let items = vec![
            ForeachItem {
                identifier: "alpha".to_string(),
                value: "alpha".to_string(),
            },
            ForeachItem {
                identifier: "beta".to_string(),
                value: "beta".to_string(),
            },
        ];
        let subtasks = task.expand_foreach_with_items(&items).unwrap();

        assert_eq!(subtasks.len(), 2);
        assert_eq!(subtasks[0].name, "up:alpha");
        assert_eq!(subtasks[1].name, "up:beta");
        assert_eq!(subtasks[1].envs.get("svc"), Some(&"beta".to_string()));
        assert_eq!(subtasks[1].envs.get("OTTO_FOREACH_INDEX"), Some(&"1".to_string()));
        assert!(subtasks[0].foreach.is_none(), "subtasks must not re-expand");
    }

    #[test]
    fn test_expand_foreach_with_items_interpolates_input_and_output_per_item() {
        let mut task = TaskSpec::new(
            "build".to_string(),
            None,
            vec![],
            vec![],
            vec!["src/${item}.txt".to_string()],
            vec!["out/${item}.o".to_string()],
            HashMap::new(),
            ParamSpecs::new(),
            "echo ${item}".to_string(),
        );
        task.foreach = Some(command_foreach("unused: items are supplied"));
        task.foreach.as_mut().unwrap().var_name = "item".to_string();

        let items = vec![
            ForeachItem {
                identifier: "a".to_string(),
                value: "a".to_string(),
            },
            ForeachItem {
                identifier: "b".to_string(),
                value: "b".to_string(),
            },
        ];
        let subtasks = task.expand_foreach_with_items(&items).unwrap();

        assert_eq!(subtasks[0].input, vec!["src/a.txt".to_string()]);
        assert_eq!(subtasks[0].output, vec!["out/a.o".to_string()]);
        assert_eq!(subtasks[1].input, vec!["src/b.txt".to_string()]);
        assert_eq!(subtasks[1].output, vec!["out/b.o".to_string()]);
    }

    #[test]
    fn test_expand_foreach_with_items_rejects_an_unexpandable_path_variable() {
        let mut task = TaskSpec::new(
            "build".to_string(),
            None,
            vec![],
            vec![],
            vec!["src/${bogus}.txt".to_string()],
            vec![],
            HashMap::new(),
            ParamSpecs::new(),
            "echo hi".to_string(),
        );
        task.foreach = Some(command_foreach("unused: items are supplied"));

        let items = vec![ForeachItem {
            identifier: "a".to_string(),
            value: "a".to_string(),
        }];
        let err = task.expand_foreach_with_items(&items).unwrap_err().to_string();
        assert!(err.contains("build"), "{err}");
        assert!(err.contains("bogus"), "{err}");
    }

    #[test]
    fn test_expand_foreach_with_items_rejects_duplicates() {
        let mut task = TaskSpec::new(
            "up".to_string(),
            None,
            vec![],
            vec![],
            vec![],
            vec![],
            HashMap::new(),
            ParamSpecs::new(),
            "echo ${svc}".to_string(),
        );
        task.foreach = Some(command_foreach("printf 'a\na\n'"));

        let dup = ForeachItem {
            identifier: "a".to_string(),
            value: "a".to_string(),
        };
        let err = task
            .expand_foreach_with_items(&[dup.clone(), dup])
            .unwrap_err()
            .to_string();

        assert!(err.contains("duplicate subtask name 'up:a'"), "{err}");
    }

    #[test]
    fn sanitize_identifier_keeps_an_item_to_one_path_component() {
        // The reproduced traversal: this identifier became the directory
        // `tasks/build:../../../ESCAPED`, i.e. a write outside the run's tasks/.
        assert_eq!(sanitize_identifier("../../../ESCAPED"), ".._.._.._ESCAPED");
        assert_eq!(sanitize_identifier("pkg/sub/name"), "pkg_sub_name");
        assert_eq!(sanitize_identifier(r"win\path"), "win_path");
        assert_eq!(sanitize_identifier("two words"), "two_words");
        assert_eq!(sanitize_identifier(".."), "_");
        assert_eq!(sanitize_identifier("."), "_");
        assert_eq!(sanitize_identifier(""), "_");
        // Ordinary identifiers are untouched.
        assert_eq!(sanitize_identifier("01-basic.sh"), "01-basic.sh");
        assert_eq!(sanitize_identifier("us-east-1"), "us-east-1");
    }

    #[test]
    fn expand_foreach_cannot_name_a_subtask_outside_its_tasks_dir() {
        let mut task = TaskSpec::new(
            "build".to_string(),
            None,
            vec![],
            vec![],
            vec![],
            vec![],
            HashMap::new(),
            ParamSpecs::new(),
            "echo ${pkg}".to_string(),
        );
        task.foreach = Some(command_foreach("printf '../../../ESCAPED\n'"));

        let items = vec![ForeachItem {
            identifier: "../../../ESCAPED".to_string(),
            value: "../../../ESCAPED".to_string(),
        }];
        let subtasks = task.expand_foreach_with_items(&items).unwrap();

        assert_eq!(subtasks[0].name, "build:.._.._.._ESCAPED");
        assert!(
            !subtasks[0].name.contains('/'),
            "a subtask name is one path component: {}",
            subtasks[0].name
        );
        // The value the script sees is untouched data.
        assert_eq!(
            subtasks[0].envs.get("OTTO_FOREACH_ITEM"),
            Some(&"../../../ESCAPED".to_string())
        );
    }

    #[test]
    fn foreach_item_values_are_escaped_against_the_env_evaluator() {
        let mut task = TaskSpec::new(
            "build".to_string(),
            None,
            vec![],
            vec![],
            vec![],
            vec![],
            HashMap::new(),
            ParamSpecs::new(),
            "echo ${pkg}".to_string(),
        );
        task.foreach = Some(command_foreach("printf 'a\n'"));

        let items = vec![ForeachItem {
            identifier: "item".to_string(),
            value: "${IFS}-$(touch /tmp/OTTO_PWNED)".to_string(),
        }];
        let subtasks = task.expand_foreach_with_items(&items).unwrap();

        // `$$` is the evaluator's literal-dollar escape; it evaluates back to the
        // item verbatim instead of aborting the task env with
        // `Environment variable 'IFS' not found` or running the command.
        let injected = subtasks[0].envs.get("OTTO_FOREACH_ITEM").unwrap();
        assert_eq!(injected, "$${IFS}-$$(touch /tmp/OTTO_PWNED)");

        let evaluated = crate::cfg::env::evaluate_envs(&subtasks[0].envs, None).unwrap();
        assert_eq!(
            evaluated.get("OTTO_FOREACH_ITEM").map(String::as_str),
            Some("${IFS}-$(touch /tmp/OTTO_PWNED)")
        );
        assert!(
            !std::path::Path::new("/tmp/OTTO_PWNED").exists(),
            "the item must never have executed"
        );
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

    /// Phase 4 negative test (design doc 2026-08-29, Phase 4 table). Parsed
    /// through `ConfigSpec` so the error carries the full nesting path
    /// (`tasks.up.foreach`).
    #[test]
    fn deny_unknown_fields_names_a_misspelled_foreach_items_key() {
        use crate::cfg::config::ConfigSpec;
        let yaml = "tasks:\n  up:\n    foreach:\n      itmes: [a, b]\n    bash: echo hi\n";
        let err = serde_yaml::from_str::<ConfigSpec>(yaml).unwrap_err().to_string();
        assert!(err.contains("itmes"), "must name the field: {err}");
        assert!(err.contains("tasks.up.foreach"), "must name the path: {err}");
    }

    /// Phase 4 negative test (design doc 2026-08-29, Phase 4 table). This is
    /// the doc's motivating incident, verbatim: `parallel:` belongs under
    /// `foreach:`, not beside it on the task. Before this phase it was
    /// silently dropped and all subtasks ran concurrently; now it is a loud
    /// config error naming the field and the path (`tasks.up`).
    #[test]
    fn deny_unknown_fields_names_a_wrong_level_parallel_key() {
        use crate::cfg::config::ConfigSpec;
        let yaml = "tasks:\n  up:\n    parallel: false\n    foreach: {items: [alpha, beta, gamma], as: svc}\n    bash: |\n      echo \"start ${svc}\"; sleep 0.3; echo \"end ${svc}\"\n";
        let err = serde_yaml::from_str::<ConfigSpec>(yaml).unwrap_err().to_string();
        assert!(err.contains("parallel"), "must name the field: {err}");
        assert!(err.contains("tasks.up"), "must name the path: {err}");
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

    // ------------------------------------------------------------------
    // Phase 7: tty: true
    // ------------------------------------------------------------------

    #[test]
    fn test_tty_defaults_to_none_when_absent() {
        let yaml = "action: echo hi";
        let spec: TaskSpec = serde_yaml::from_str(yaml).expect("parse failed");
        assert_eq!(spec.tty, None, "an ottofile without tty: must not gain one");
    }

    #[test]
    fn test_tty_parses_both_values() {
        let on: TaskSpec = serde_yaml::from_str("action: aws sso login\ntty: true").expect("parse failed");
        assert_eq!(on.tty, Some(true));
        let off: TaskSpec = serde_yaml::from_str("action: echo hi\ntty: false").expect("parse failed");
        assert_eq!(off.tty, Some(false));
    }

    #[test]
    fn test_tty_serializes_only_when_set() {
        let mut spec = TaskSpec::new(
            "login".to_string(),
            None,
            vec![],
            vec![],
            vec![],
            vec![],
            HashMap::new(),
            ParamSpecs::new(),
            "#!/bin/bash\naws sso login".to_string(),
        );
        assert!(!serde_yaml::to_string(&spec).unwrap().contains("tty"));
        spec.tty = Some(true);
        assert!(serde_yaml::to_string(&spec).unwrap().contains("tty: true"));
    }

    /// A task naming both `bash:` and `python:` used to silently run bash and
    /// drop python with no warning. Now it is a loud config-load error naming
    /// every source present.
    #[test]
    fn bash_and_python_together_is_a_loud_config_error() {
        let yaml = "bash: echo FROM_BASH\npython: print('FROM_PYTHON')\n";
        let err = serde_yaml::from_str::<TaskSpec>(yaml).unwrap_err().to_string();
        assert!(err.contains("bash"), "{err}");
        assert!(err.contains("python"), "{err}");
    }

    /// Dedent used to compute indentation in bytes (two ascii spaces = 2
    /// bytes) while slicing every line at that same byte offset, including a
    /// sibling line indented by one U+2002 (EN SPACE, 3 bytes): byte offset 2
    /// lands mid-character there and panicked, "byte index 2 is not a char
    /// boundary; it is inside '\u{2002}'". Char-counted indent sidesteps it.
    #[test]
    fn deserialize_script_string_dedents_multibyte_whitespace_without_panicking() {
        let script = "  a\n\u{2002}b";
        assert_eq!(deserialize_script_string(script), "a\nb");
    }

    /// `tty:` on a foreach task means "give each of these the terminal", so every
    /// generated subtask carries it. The exclusivity gate then serializes them
    /// even under `parallel: true`; that is documented behavior, not an error.
    #[test]
    fn test_foreach_subtasks_inherit_tty() {
        let mut task = TaskSpec::new(
            "login".to_string(),
            None,
            vec![],
            vec![],
            vec![],
            vec![],
            HashMap::new(),
            ParamSpecs::new(),
            "#!/bin/bash\necho ${item}".to_string(),
        );
        task.tty = Some(true);
        task.foreach = Some(ForeachSpec {
            items: vec!["a".to_string(), "b".to_string()],
            ..Default::default()
        });

        let subtasks = task.expand_foreach(Path::new(".")).expect("expansion failed");

        assert_eq!(subtasks.len(), 2);
        for subtask in &subtasks {
            assert_eq!(subtask.tty, Some(true), "{} lost tty", subtask.name);
        }
    }

    /// The virtual parent runs no script, so it must not claim the terminal - it
    /// would take the exclusive permit to do nothing.
    #[test]
    fn test_virtual_parent_drops_tty() {
        let mut task = TaskSpec::new(
            "login".to_string(),
            None,
            vec![],
            vec![],
            vec![],
            vec![],
            HashMap::new(),
            ParamSpecs::new(),
            "#!/bin/bash\necho hi".to_string(),
        );
        task.tty = Some(true);

        assert_eq!(task.as_virtual_parent().tty, None);
    }

    // ========================================================================
    // Phase 5 drift test (design doc 2026-08-29, Phase 5(b) / Resolved
    // Decisions "panel round 2"): docs/commands/ottofile-reference.md must
    // name every key of every deny_unknown_fields struct, PLUS EdgeSpec
    // (src/cfg/edge.rs), whose hand-written `visit_map` enforces the same
    // "unknown field" contract without the derive macro (round-4 audit
    // cheap-win: EdgeSpec's `task`/`when` keys had no reference-doc rows and
    // tripped none of the checks below). Two zero-new-crate techniques, used
    // together, so the reference cannot drift silently:
    //
    // 1. Exhaustive destructuring below is the compile-time TRIGGER: if any
    //    of the seven structs gains or loses a field, the destructuring
    //    pattern stops matching and the BUILD breaks, before any test even
    //    runs. This is what reaches private `TaskSpecHelper` from inside this
    //    file's own `#[cfg(test)]` module.
    // 2. `expected_keys_from_deny_unknown_fields` recovers each struct's real
    //    on-disk key list (renames already applied, e.g. `as` not `var_name`)
    //    straight out of serde's own "unknown field" error message, by
    //    feeding it a single bogus key. Works identically against EdgeSpec's
    //    hand-written `visit_map`: it returns `Error::unknown_field(other,
    //    &["task", "when"])`, which stringifies to the exact same "unknown
    //    field `x`, expected `task` or `when`" shape serde's derive macro
    //    produces for a two-field struct (verified directly: no location
    //    suffix, same phrasing). Nothing here hand-copies a key list that
    //    could go stale on its own.
    // ========================================================================

    use crate::cfg::config::ConfigSpec;
    use crate::cfg::otto::{OttoSpec, RetentionSpec};
    use crate::cfg::param::ParamSpec;

    /// Every field on the six `deny_unknown_fields` structs has a default (per
    /// the design doc's Data Model), so a document containing nothing but one
    /// bogus key is enough to provoke the "unknown field" error and never a
    /// "missing field" one instead.
    const BOGUS_KEY_PROBE: &str = "__ottofile_reference_drift_probe__: true\n";

    /// Parse `T` from [`BOGUS_KEY_PROBE`] and read its real on-disk field names
    /// back out of the `deny_unknown_fields` error text, rather than hand-
    /// copying them. Handles all three of serde's phrasings: "expected `a`",
    /// "expected `a` or `b`", and "expected one of `a`, `b`, ..., `z`".
    fn expected_keys_from_deny_unknown_fields<T: serde::de::DeserializeOwned>() -> Vec<String> {
        let err = serde_yaml::from_str::<T>(BOGUS_KEY_PROBE)
            .err()
            .expect("bogus key must be rejected")
            .to_string();
        let after_expected = err
            .split("expected")
            .nth(1)
            .unwrap_or_else(|| panic!("deny_unknown_fields error names no expected set: {err}"));
        // A location suffix (" at line N column M") is present except for the
        // root ConfigSpec case, which has no parent path (Phase 4 table).
        let before_location = after_expected.split(" at line").next().unwrap();
        before_location
            .split('`')
            .skip(1)
            .step_by(2)
            .map(str::to_string)
            .collect()
    }

    /// True when `expected_path` appears in the reference doc as the ENTIRE
    /// value of some backtick-quoted span (e.g. `otto.retention.keep_days`
    /// matches the doc's `` `otto.retention.keep_days` `` row exactly).
    ///
    /// This is a full-path match, not a trailing-dot-segment match. Three
    /// on-disk key names are reused at more than one level (`tasks`: root
    /// `ConfigSpec.tasks` vs. `otto.tasks`; `envs`: `otto.envs` vs.
    /// `tasks.<name>.envs`; `help`: `tasks.<name>.help` vs.
    /// `...params.<title>.help`), so a bare trailing-segment match (matching
    /// `tasks` against ANY span ending in `.tasks`) would let one level's row
    /// silently vouch for a different level's key, going undetected if that
    /// key's own row were deleted. Requiring the full path per level closes
    /// that hole (round-4 implementation audit, cheap-win 3).
    fn reference_doc_mentions_key_at(doc: &str, expected_path: &str) -> bool {
        doc.split('`').skip(1).step_by(2).any(|token| token == expected_path)
    }

    /// Doc-path builders, one per struct, mirroring the reference doc's own
    /// level headings (`## otto:`, `## otto.retention:`, `## tasks.<name>:`,
    /// `## tasks.<name>.foreach:`, `## tasks.<name>.params.<title>:`) plus one
    /// for `EdgeSpec`'s `tasks.<name>.after[]`/`before[]` object. Applying the
    /// right prefix to a recovered on-disk key name is what lets the exact
    /// match above tell same-named keys at different levels apart.
    fn root_path(key: &str) -> String {
        key.to_string()
    }
    fn otto_path(key: &str) -> String {
        format!("otto.{key}")
    }
    fn retention_path(key: &str) -> String {
        format!("otto.retention.{key}")
    }
    fn task_path(key: &str) -> String {
        format!("tasks.<name>.{key}")
    }
    fn foreach_path(key: &str) -> String {
        format!("tasks.<name>.foreach.{key}")
    }
    fn param_path(key: &str) -> String {
        format!("...params.<title>.{key}")
    }
    fn edge_path(key: &str) -> String {
        format!("tasks.<name>.after[].{key}")
    }

    #[test]
    fn ottofile_reference_key_inventory_is_exhaustive() {
        // --- Compile-time trigger: exhaustive destructuring of all six ---
        // Every field is bound to `_`; the build breaks the moment any struct
        // gains or loses a field, forcing whoever changed the schema to touch
        // this test (and, from there, the reference doc) immediately.
        let ConfigSpec { otto: _, tasks: _ } = ConfigSpec::default();
        let OttoSpec {
            name: _,
            about: _,
            api: _,
            jobs: _,
            home: _,
            tasks: _,
            verbosity: _,
            envs: _,
            retention: _,
        } = OttoSpec::default();
        let RetentionSpec {
            keep_days: _,
            keep_last: _,
            keep_failed: _,
            auto_prune: _,
            prune_interval_hours: _,
        } = RetentionSpec::default();
        let ForeachSpec {
            glob: _,
            items: _,
            range: _,
            command: _,
            var_name: _,
            parallel: _,
            max_items: _,
        } = ForeachSpec::default();
        let TaskSpecHelper {
            help: _,
            after: _,
            before: _,
            input: _,
            output: _,
            envs: _,
            params: _,
            bash: _,
            python: _,
            action: _,
            foreach: _,
            on_failure: _,
            tty: _,
        } = serde_yaml::from_str::<TaskSpecHelper>("{}\n").unwrap();
        let ParamSpec {
            name: _,
            short: _,
            long: _,
            param_type: _,
            metavar: _,
            default: _,
            choices: _,
            choices_command: _,
            nargs: _,
            help: _,
            value: _,
        } = serde_yaml::from_str::<ParamSpec>("{}\n").unwrap();
        // EdgeSpec has two bookkeeping fields (`from_sugar`, `is_injected_sugar`)
        // alongside its two on-disk keys (`task`, `when`); all four are bound
        // here so the destructuring still breaks on ANY field added or removed,
        // but only `task`/`when` are on-disk keys checked against the doc below.
        let EdgeSpec {
            task: _,
            when: _,
            from_sugar: _,
            is_injected_sugar: _,
        } = EdgeSpec::sugar("probe");

        // --- Runtime check: recovered on-disk keys vs. the reference doc ---
        let doc = include_str!("../../docs/commands/ottofile-reference.md");

        type PathBuilder = fn(&str) -> String;
        type KeyProbe = (&'static str, usize, fn() -> Vec<String>, PathBuilder);
        let expectations: &[KeyProbe] = &[
            (
                "ConfigSpec",
                2,
                expected_keys_from_deny_unknown_fields::<ConfigSpec>,
                root_path,
            ),
            (
                "OttoSpec",
                9,
                expected_keys_from_deny_unknown_fields::<OttoSpec>,
                otto_path,
            ),
            (
                "RetentionSpec",
                5,
                expected_keys_from_deny_unknown_fields::<RetentionSpec>,
                retention_path,
            ),
            (
                "ForeachSpec",
                7,
                expected_keys_from_deny_unknown_fields::<ForeachSpec>,
                foreach_path,
            ),
            (
                "TaskSpecHelper",
                13,
                expected_keys_from_deny_unknown_fields::<TaskSpecHelper>,
                task_path,
            ),
            (
                "ParamSpec",
                6,
                expected_keys_from_deny_unknown_fields::<ParamSpec>,
                param_path,
            ),
            (
                "EdgeSpec",
                2,
                expected_keys_from_deny_unknown_fields::<EdgeSpec>,
                edge_path,
            ),
        ];

        let mut total = 0;
        for (struct_name, expected_count, probe, path_for) in expectations {
            let keys = probe();
            assert_eq!(
                keys.len(),
                *expected_count,
                "{struct_name}: expected {expected_count} on-disk keys, serde reports {}: {keys:?}",
                keys.len()
            );
            for key in &keys {
                let expected_path = path_for(key);
                assert!(
                    reference_doc_mentions_key_at(doc, &expected_path),
                    "{struct_name}'s on-disk key `{key}` (expected doc path \
                     `{expected_path}`) is not mentioned in \
                     docs/commands/ottofile-reference.md"
                );
            }
            total += keys.len();
        }
        assert_eq!(
            total, 44,
            "total on-disk key count drifted from the design doc's count of 44"
        );
    }
}
