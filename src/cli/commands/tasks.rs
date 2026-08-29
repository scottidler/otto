//! `otto --tasks`: a machine-readable task list.
//!
//! Frozen contract (see `docs/design/2026-08-28-boundary-fixes-and-dynamic-foreach.md`,
//! Phase 5): user-defined ottofile tasks only (no injected builtins), foreach parents
//! appear once with a `subtasks` array (no separate top-level subtask entries), stdout
//! carries pure data in the selected format, all notices and errors go to stderr, and a
//! resolution failure exits non-zero with nothing on stdout.
//!
//! One logical shape in both formats: a map keyed by task name, mirroring the
//! ottofile's own `tasks:` shape. `BTreeMap` keeps key order identical (and
//! deterministic) between the YAML and JSON encodings.

use std::collections::BTreeMap;

use eyre::{Result, WrapErr};
use serde::Serialize;

use crate::cfg::param::ParamType;
use crate::cfg::task::{ForeachItem, ForeachSpec, TaskSpecs};
use crate::cli::builtins::is_builtin;

/// A dedicated serde view for `--tasks`, deliberately separate from `TaskSpec`'s
/// own `Serialize` impl (that one is shaped for YAML round-trip fidelity against
/// the ottofile's on-disk form and is the wrong contract for consumers).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct TaskView {
    pub help: Option<String>,
    pub params: Vec<ParamView>,
    pub edges: EdgesView,
    pub subtasks: Vec<String>,
}

/// `choices` and `choices-command` are mutually exclusive at the config level
/// (`deserialize_param_map` enforces it), so exactly one of them is emitted per
/// param: a static param carries its list (possibly empty), a dynamic one
/// carries the command verbatim in its place. `--tasks` reports the provenance
/// and executes nothing: the rule stays "a surface executes only what it needs",
/// and a consumer asking for the task list has no need of the values.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ParamView {
    pub name: String,
    pub flags: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub choices: Option<Vec<String>>,
    #[serde(rename = "choices-command", skip_serializing_if = "Option::is_none")]
    pub choices_command: Option<String>,
    pub default: Option<String>,
    pub positional: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct EdgesView {
    pub before: Vec<String>,
    pub after: Vec<String>,
}

pub type TasksView = BTreeMap<String, TaskView>;

/// Build the `--tasks` view from the loaded (post-builtin-injection) task specs.
///
/// `resolve` turns one task's `foreach` into its items. The parser passes its
/// per-invocation resolver, which anchors glob/range at the ottofile's
/// directory and runs a `command:` source at most once (`--tasks` is an
/// enumeration surface: reporting the real subtask ids is its job, so it does
/// resolve command sources).
///
/// Any foreach resolution failure aborts the whole view (frozen contract: a
/// resolution failure exits non-zero with nothing on stdout). A foreach that
/// legitimately resolves to zero items is not an error: it prints a one-line
/// notice to stderr and contributes an empty `subtasks` array, matching the
/// posture `foreach: command:` (Phase 6) is designed around.
pub fn build_tasks_view(
    tasks: &TaskSpecs,
    resolve: &dyn Fn(&str, &ForeachSpec) -> Result<Vec<ForeachItem>>,
) -> Result<TasksView> {
    let mut view = TasksView::new();

    let mut names: Vec<&String> = tasks.keys().collect();
    names.sort();

    for name in names {
        if is_builtin(name) {
            continue;
        }
        let spec = &tasks[name];

        let subtasks = match &spec.foreach {
            Some(foreach) => {
                let items = resolve(name, foreach)
                    .wrap_err_with(|| format!("task '{name}': failed to resolve foreach items"))?;
                if items.is_empty() && !foreach.is_command_source() {
                    // A command source prints its own empty-scope notice while resolving.
                    eprintln!("Notice: task '{name}' foreach matched 0 items");
                }
                items.iter().map(|item| format!("{name}:{}", item.identifier)).collect()
            }
            None => Vec::new(),
        };

        let mut params: Vec<ParamView> = spec.params.values().map(param_view).collect();
        params.sort_by(|a, b| a.name.cmp(&b.name));

        let edges = EdgesView {
            before: spec.before.iter().map(|e| e.task.clone()).collect(),
            after: spec.after.iter().map(|e| e.task.clone()).collect(),
        };

        view.insert(
            name.clone(),
            TaskView {
                help: spec.help.clone(),
                params,
                edges,
                subtasks,
            },
        );
    }

    Ok(view)
}

fn param_view(spec: &crate::cfg::config::ParamSpec) -> ParamView {
    let mut flags = Vec::new();
    if let Some(s) = spec.short {
        flags.push(format!("-{s}"));
    }
    if let Some(l) = &spec.long {
        flags.push(format!("--{l}"));
    }
    let (choices, choices_command) = match &spec.choices_command {
        Some(command) => (None, Some(command.clone())),
        None => (Some(spec.choices.clone()), None),
    };
    ParamView {
        name: spec.name.clone(),
        flags,
        choices,
        choices_command,
        default: spec.default.clone(),
        positional: spec.param_type == ParamType::POS,
    }
}

/// Output format for `--tasks`, resolved once per invocation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TasksFormat {
    Yaml,
    Json,
}

/// TTY-detect seam (house rule: YAML for humans, JSON when piped, one
/// `--format` override, no boolean format flags). Pure function so the tty
/// branch is unit-testable without a real terminal.
pub fn choose_format(explicit: Option<&str>, stdout_is_tty: bool) -> TasksFormat {
    match explicit {
        Some("json") => TasksFormat::Json,
        Some("yaml") => TasksFormat::Yaml,
        _ => {
            if stdout_is_tty {
                TasksFormat::Yaml
            } else {
                TasksFormat::Json
            }
        }
    }
}

/// Render the view in the chosen format. Stdout is pure data: no notices, no
/// trailing commentary.
pub fn render_tasks_view(view: &TasksView, format: TasksFormat) -> Result<String> {
    match format {
        TasksFormat::Json => serde_json::to_string_pretty(view).wrap_err("failed to render task list as JSON"),
        TasksFormat::Yaml => serde_yaml::to_string(view).wrap_err("failed to render task list as YAML"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cfg::config::{ParamSpec, TaskSpec, Value};
    use crate::cfg::edge::EdgeSpec;
    use crate::cfg::param::{Nargs, ParamType};
    use std::path::PathBuf;

    /// Stand-in for the parser's resolver: static sources only, anchored at the
    /// test's cwd, which is all these view-shape tests need.
    fn static_resolver(_name: &str, foreach: &ForeachSpec) -> Result<Vec<ForeachItem>> {
        foreach.resolve_items(&PathBuf::from("."))
    }

    fn plain_task(name: &str) -> TaskSpec {
        TaskSpec {
            name: name.to_string(),
            help: Some(format!("help for {name}")),
            after: vec![],
            before: vec![],
            input: vec![],
            output: vec![],
            envs: Default::default(),
            params: Default::default(),
            action: "echo hi".to_string(),
            foreach: None,
            virtual_parent: false,
            on_failure: vec![],
        }
    }

    #[test]
    fn choose_format_explicit_override_wins_over_tty() {
        assert_eq!(choose_format(Some("json"), true), TasksFormat::Json);
        assert_eq!(choose_format(Some("yaml"), false), TasksFormat::Yaml);
    }

    #[test]
    fn choose_format_defaults_to_tty_detection() {
        assert_eq!(choose_format(None, true), TasksFormat::Yaml);
        assert_eq!(choose_format(None, false), TasksFormat::Json);
    }

    #[test]
    fn build_tasks_view_excludes_builtins() {
        let mut tasks = TaskSpecs::new();
        tasks.insert("up".to_string(), plain_task("up"));
        tasks.insert("History".to_string(), plain_task("History"));
        tasks.insert("Clean".to_string(), plain_task("Clean"));

        let view = build_tasks_view(&tasks, &static_resolver).unwrap();

        assert_eq!(view.keys().collect::<Vec<_>>(), vec!["up"]);
    }

    #[test]
    fn build_tasks_view_reports_foreach_subtasks_once() {
        let mut parent = plain_task("up");
        parent.foreach = Some(ForeachSpec {
            items: vec!["alpha".to_string(), "beta".to_string()],
            ..Default::default()
        });

        let mut tasks = TaskSpecs::new();
        tasks.insert("up".to_string(), parent);

        let view = build_tasks_view(&tasks, &static_resolver).unwrap();

        assert_eq!(view.len(), 1, "subtasks must not appear as separate top-level entries");
        let up = &view["up"];
        assert_eq!(up.subtasks, vec!["up:alpha".to_string(), "up:beta".to_string()]);
    }

    #[test]
    fn build_tasks_view_zero_items_is_not_fatal() {
        let mut parent = plain_task("up");
        parent.foreach = Some(ForeachSpec {
            glob: Some("no-such-file-*.xyz".to_string()),
            ..Default::default()
        });

        let mut tasks = TaskSpecs::new();
        tasks.insert("up".to_string(), parent);

        let view = build_tasks_view(&tasks, &static_resolver).unwrap();
        assert!(view["up"].subtasks.is_empty());
    }

    #[test]
    fn build_tasks_view_carries_edges_and_params() {
        let mut task = plain_task("deploy");
        task.before = vec![EdgeSpec::sugar("build")];
        task.after = vec![EdgeSpec::sugar("notify")];
        task.params.insert(
            "svc".to_string(),
            ParamSpec {
                name: "svc".to_string(),
                short: Some('s'),
                long: Some("svc".to_string()),
                param_type: ParamType::OPT,
                dest: None,
                metavar: None,
                default: Some("web".to_string()),
                constant: Value::Empty,
                choices_command: None,
                choices: vec!["web".to_string(), "api".to_string()],
                nargs: Nargs::One,
                help: Some("service name".to_string()),
                value: Value::Empty,
            },
        );

        let mut tasks = TaskSpecs::new();
        tasks.insert("deploy".to_string(), task);

        let view = build_tasks_view(&tasks, &static_resolver).unwrap();
        let deploy = &view["deploy"];

        assert_eq!(deploy.edges.before, vec!["build".to_string()]);
        assert_eq!(deploy.edges.after, vec!["notify".to_string()]);
        assert_eq!(deploy.params.len(), 1);
        assert_eq!(deploy.params[0].name, "svc");
        assert_eq!(deploy.params[0].flags, vec!["-s".to_string(), "--svc".to_string()]);
        assert_eq!(
            deploy.params[0].choices,
            Some(vec!["web".to_string(), "api".to_string()])
        );
        assert_eq!(deploy.params[0].default, Some("web".to_string()));
        assert!(!deploy.params[0].positional);
    }

    #[test]
    fn render_tasks_view_json_and_yaml_share_key_set() {
        let mut tasks = TaskSpecs::new();
        tasks.insert("up".to_string(), plain_task("up"));
        tasks.insert("down".to_string(), plain_task("down"));
        let view = build_tasks_view(&tasks, &static_resolver).unwrap();

        let json = render_tasks_view(&view, TasksFormat::Json).unwrap();
        let yaml = render_tasks_view(&view, TasksFormat::Yaml).unwrap();

        let json_value: serde_json::Value = serde_json::from_str(&json).unwrap();
        let yaml_value: serde_yaml::Value = serde_yaml::from_str(&yaml).unwrap();

        let mut json_keys: Vec<String> = json_value.as_object().unwrap().keys().cloned().collect();
        let mut yaml_keys: Vec<String> = yaml_value
            .as_mapping()
            .unwrap()
            .keys()
            .map(|k| k.as_str().unwrap().to_string())
            .collect();
        json_keys.sort();
        yaml_keys.sort();

        assert_eq!(json_keys, yaml_keys);
        assert_eq!(json_keys, vec!["down".to_string(), "up".to_string()]);
    }
}
