#![cfg(test)]

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
        tty: None,
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
            metavar: None,
            default: Some("web".to_string()),
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
