#![cfg(test)]

use super::*;
use crate::makefile::ast::{Target, Variable};

fn variable(name: &str, value: &str, assignment_type: AssignmentType) -> Variable {
    Variable {
        name: name.to_string(),
        value: value.to_string(),
        assignment_type,
        line: 1,
    }
}

fn target(name: &str, commands: &[&str]) -> Target {
    Target {
        name: name.to_string(),
        dependencies: Vec::new(),
        commands: commands.iter().map(|c| c.to_string()).collect(),
        comment: None,
        is_phony: false,
        line: 1,
    }
}

fn convert(ast: MakefileAst) -> (ConfigSpec, Vec<Diagnostic>) {
    let mut converter = OttoConverter::new(ast);
    let config = converter.convert().expect("convert failed");
    let diagnostics = converter.diagnostics().to_vec();
    (config, diagnostics)
}

fn messages(diagnostics: &[Diagnostic]) -> String {
    diagnostics.iter().map(|d| d.to_string()).collect::<Vec<_>>().join("\n")
}

/// Diagnostics other than the "no targets were converted" notice.
///
/// The variables-only tests below build an AST with no targets on purpose -
/// they are about variable conversion - and a conversion with no tasks is
/// legitimately reported. Filtering it keeps those assertions about the thing
/// they test, without weakening them to "some diagnostics are fine".
fn diagnostics_about_variables(diagnostics: &[Diagnostic]) -> Vec<&Diagnostic> {
    diagnostics
        .iter()
        .filter(|d| !d.to_string().contains("no targets were converted"))
        .collect()
}

#[test]
fn test_convert_simple_variable() {
    let mut ast = MakefileAst::new();
    ast.variables.push(variable("VAR", "value", AssignmentType::Simple));

    let (config, diagnostics) = convert(ast);

    assert_eq!(config.otto.envs.get("VAR"), Some(&"value".to_string()));
    let others = diagnostics_about_variables(&diagnostics);
    assert!(others.is_empty(), "{}", messages(&diagnostics));
}

#[test]
fn test_shell_variable_loses_the_shell_prefix() {
    let mut ast = MakefileAst::new();
    ast.variables.push(variable(
        "VERSION",
        "$(shell git describe --tags)",
        AssignmentType::ShellExecution,
    ));

    let (config, _) = convert(ast);

    assert_eq!(
        config.otto.envs.get("VERSION"),
        Some(&"$(git describe --tags)".to_string())
    );
}

#[test]
fn test_variable_reference_becomes_a_shell_reference() {
    let mut ast = MakefileAst::new();
    ast.variables.push(variable("NAME", "example", AssignmentType::Simple));
    ast.variables
        .push(variable("IMAGE", "$(NAME):latest", AssignmentType::Simple));

    let (config, diagnostics) = convert(ast);

    assert_eq!(config.otto.envs.get("IMAGE"), Some(&"${NAME}:latest".to_string()));
    let others = diagnostics_about_variables(&diagnostics);
    assert!(others.is_empty(), "{}", messages(&diagnostics));
}

#[test]
fn test_append_assignment_is_reported() {
    let mut ast = MakefileAst::new();
    ast.variables.push(variable("FLAGS", "-g", AssignmentType::Append));

    let (_, diagnostics) = convert(ast);

    assert!(
        messages(&diagnostics).contains("cannot append"),
        "{}",
        messages(&diagnostics)
    );
}

#[test]
fn test_convert_simple_target() {
    let mut ast = MakefileAst::new();
    ast.targets.push(target("build", &["echo Building"]));

    let (config, _) = convert(ast);

    let task = config.tasks.get("build").unwrap();
    assert_eq!(task.name, "build");
    assert!(task.action.contains("echo Building"));
    assert!(task.action.starts_with("#!/bin/bash"));
}

#[test]
fn test_convert_target_with_dependencies() {
    let mut ast = MakefileAst::new();
    let mut build = target("build", &["echo Building"]);
    build.dependencies = vec!["test".to_string(), "clean".to_string()];
    ast.targets.push(build);

    let (config, _) = convert(ast);

    let task = config.tasks.get("build").unwrap();
    assert_eq!(task.before.len(), 2);
    assert!(task.before.iter().any(|e| e.task == "test"));
    assert!(task.before.iter().any(|e| e.task == "clean"));
}

#[test]
fn test_convert_target_with_comment() {
    let mut ast = MakefileAst::new();
    let mut build = target("build", &["echo Building"]);
    build.comment = Some("Build the project".to_string());
    ast.targets.push(build);

    let (config, _) = convert(ast);

    assert_eq!(
        config.tasks.get("build").unwrap().help,
        Some("Build the project".to_string())
    );
}

#[test]
fn test_convert_multiple_commands() {
    let mut ast = MakefileAst::new();
    ast.targets
        .push(target("build", &["mkdir -p dist", "echo Building", "echo Done"]));

    let (config, _) = convert(ast);

    let task = config.tasks.get("build").unwrap();
    assert!(task.action.contains("mkdir -p dist"));
    assert!(task.action.contains("echo Building"));
    assert!(task.action.contains("echo Done"));
}

#[test]
fn test_recipe_variable_becomes_bash_and_never_stays_make() {
    let mut ast = MakefileAst::new();
    ast.variables.push(variable("NAME", "example", AssignmentType::Simple));
    ast.targets
        .push(target("build", &["echo Building $(NAME) version ${NAME}"]));

    let (config, diagnostics) = convert(ast);

    let action = &config.tasks.get("build").unwrap().action;
    assert!(action.contains("echo Building ${NAME} version ${NAME}"), "{action}");
    assert!(!action.contains("$(NAME)"), "{action}");
    assert!(diagnostics.is_empty(), "{}", messages(&diagnostics));
}

#[test]
fn test_dollar_dollar_becomes_one_dollar() {
    let mut ast = MakefileAst::new();
    ast.targets.push(target("show", &["awk '{print $$1}'"]));

    let (config, _) = convert(ast);

    let action = &config.tasks.get("show").unwrap().action;
    assert!(action.contains("awk '{print $1}'"), "{action}");
}

#[test]
fn test_undefined_variable_is_reported() {
    let mut ast = MakefileAst::new();
    ast.targets.push(target("build", &["echo $(NOWHERE)"]));

    let (config, diagnostics) = convert(ast);

    assert!(config.tasks.get("build").unwrap().action.contains("${NOWHERE}"));
    assert!(
        messages(&diagnostics).contains("`NOWHERE` is not defined in the Makefile"),
        "{}",
        messages(&diagnostics)
    );
}

#[test]
fn test_automatic_variable_is_reported() {
    let mut ast = MakefileAst::new();
    ast.targets.push(target("build", &["gcc -c $< -o $@"]));

    let (config, diagnostics) = convert(ast);

    let action = &config.tasks.get("build").unwrap().action;
    assert!(action.contains("$<") && action.contains("$@"), "{action}");
    assert_eq!(
        diagnostics
            .iter()
            .filter(|d| d.message.contains("automatic variable"))
            .count(),
        2,
        "{}",
        messages(&diagnostics)
    );
}

#[test]
fn test_make_function_in_a_recipe_is_reported_not_translated() {
    let mut ast = MakefileAst::new();
    ast.targets.push(target("list", &["echo $(wildcard *.c)"]));

    let (config, diagnostics) = convert(ast);

    assert!(config.tasks.get("list").unwrap().action.contains("$(wildcard *.c)"));
    assert!(
        messages(&diagnostics).contains("make expression otto cannot translate"),
        "{}",
        messages(&diagnostics)
    );
}

#[test]
fn test_special_make_variable_is_reported() {
    let mut ast = MakefileAst::new();
    ast.targets.push(target("help", &["awk -f help.awk $(MAKEFILE_LIST)"]));

    let (_, diagnostics) = convert(ast);

    assert!(
        messages(&diagnostics).contains("`MAKEFILE_LIST` is a make-internal variable"),
        "{}",
        messages(&diagnostics)
    );
}

#[test]
fn test_unbalanced_expansion_is_reported() {
    let mut ast = MakefileAst::new();
    ast.targets.push(target("build", &["echo $(NAME"]));

    let (_, diagnostics) = convert(ast);

    assert!(
        messages(&diagnostics).contains("unbalanced"),
        "{}",
        messages(&diagnostics)
    );
}

#[test]
fn test_non_shell_name_is_left_alone() {
    let mut ast = MakefileAst::new();
    ast.targets.push(target("build", &["echo $(BINARY-NAME)"]));

    let (config, diagnostics) = convert(ast);

    // `${BINARY-NAME}` means "BINARY, or NAME if unset" in bash: rewriting
    // it would be a silent behavior change.
    assert!(config.tasks.get("build").unwrap().action.contains("$(BINARY-NAME)"));
    assert!(
        messages(&diagnostics).contains("cannot translate"),
        "{}",
        messages(&diagnostics)
    );
}

#[test]
fn test_duplicate_target_is_reported() {
    let mut ast = MakefileAst::new();
    ast.targets.push(target("build", &["echo first"]));
    let mut second = target("build", &["echo second"]);
    second.line = 10;
    ast.targets.push(second);

    let (config, diagnostics) = convert(ast);

    assert_eq!(config.tasks.len(), 1);
    assert!(config.tasks.get("build").unwrap().action.contains("echo second"));
    assert!(
        messages(&diagnostics).contains("duplicate target `build` overrides the rule at line 1, discarding"),
        "{}",
        messages(&diagnostics)
    );
}

#[test]
fn test_default_goal_conversion() {
    let mut ast = MakefileAst::new();
    ast.default_goal = Some("build".to_string());
    ast.targets.push(target("build", &["echo Building"]));

    let (config, diagnostics) = convert(ast);

    assert_eq!(config.otto.tasks, vec!["build".to_string()]);
    assert!(diagnostics.is_empty(), "{}", messages(&diagnostics));
}

#[test]
fn test_default_goal_without_a_task_is_reported() {
    let mut ast = MakefileAst::new();
    ast.default_goal = Some("all".to_string());
    ast.targets.push(target("build", &["echo Building"]));

    let (_, diagnostics) = convert(ast);

    assert!(
        messages(&diagnostics).contains("default goal `all` has no converted task"),
        "{}",
        messages(&diagnostics)
    );
}

#[test]
fn test_no_default_goal_uses_first_target() {
    let mut ast = MakefileAst::new();
    ast.targets.push(target("build", &["echo Building"]));
    ast.targets.push(target("test", &["echo Testing"]));

    let (config, _) = convert(ast);

    assert_eq!(config.otto.tasks, vec!["build".to_string()]);
}

#[test]
fn test_command_prefix_handling() {
    let mut ast = MakefileAst::new();
    ast.targets
        .push(target("build", &["@echo Hidden", "-mkdir -p dist", "echo Visible"]));

    let (config, _) = convert(ast);

    let task = config.tasks.get("build").unwrap();
    // @ prefix should be removed
    assert!(task.action.contains("echo Hidden"));
    assert!(!task.action.contains("@echo"));
    // - prefix should be converted to || true
    assert!(task.action.contains("mkdir -p dist || true"));
    // Normal command should remain
    assert!(task.action.contains("echo Visible"));
}

/// Make allows `@`, `-`, and `+` combined, in either order, and repeated. The
/// old code peeled at most one leading character, so `@-rm -rf dist` kept a
/// literal `-` in the command (no `|| true`), and `-@echo hi` left a literal
/// `@` for bash to choke on instead of suppressing it.
#[test]
fn test_combined_prefixes_are_all_stripped_in_any_order() {
    let mut ast = MakefileAst::new();
    ast.targets
        .push(target("build", &["@-rm -rf dist", "-@echo hi", "+make -C sub"]));

    let (config, _) = convert(ast);

    let task = config.tasks.get("build").unwrap();
    assert!(task.action.contains("rm -rf dist || true"), "{}", task.action);
    assert!(task.action.contains("echo hi || true"), "{}", task.action);
    assert!(!task.action.contains("@echo hi"), "{}", task.action);
    // `+` has no otto equivalent (there is no `-n` dry run to override) and
    // is just stripped, with no `|| true` since `-` never appeared.
    assert!(task.action.contains("make -C sub"), "{}", task.action);
    assert!(!task.action.contains("+make"), "{}", task.action);
}

/// A repeated prefix character (`--rm`, legal but pointless make) must not
/// double the `|| true`.
#[test]
fn test_a_repeated_dash_prefix_still_adds_or_true_once() {
    let mut ast = MakefileAst::new();
    ast.targets.push(target("clean", &["--rm -rf dist"]));

    let (config, _) = convert(ast);

    let task = config.tasks.get("clean").unwrap();
    assert_eq!(task.action.matches("|| true").count(), 1, "{}", task.action);
    assert!(
        task.action.contains("\nrm -rf dist || true"),
        "a leftover literal `-` would make bash read this as an option: {}",
        task.action
    );
}

#[test]
fn test_empty_makefile() {
    let (config, _) = convert(MakefileAst::new());

    assert!(config.tasks.is_empty());
    assert_eq!(config.otto.tasks, vec!["*".to_string()]);
}

#[test]
fn test_converted_spec_carries_no_machine_specific_jobs() {
    let mut ast = MakefileAst::new();
    ast.targets.push(target("build", &["echo Building"]));

    let (config, _) = convert(ast);

    // `jobs` at its default is omitted on serialize, so the converting
    // machine's CPU count never reaches the ottofile.
    let yaml = yaml_serde::to_string(&config).unwrap();
    assert!(!yaml.contains("jobs:"), "{yaml}");
}
