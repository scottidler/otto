#![cfg(test)]

use super::*;

fn parse(content: &str) -> (MakefileAst, Vec<Diagnostic>) {
    let mut parser = MakefileParser::new(content.to_string());
    let ast = parser.parse().expect("parse failed");
    let diagnostics = parser.diagnostics().to_vec();
    (ast, diagnostics)
}

fn messages(diagnostics: &[Diagnostic]) -> String {
    diagnostics.iter().map(|d| d.to_string()).collect::<Vec<_>>().join("\n")
}

#[test]
fn test_parse_simple_variable() {
    let (ast, diagnostics) = parse("VAR := value");

    assert_eq!(ast.variables.len(), 1);
    assert_eq!(ast.variables[0].name, "VAR");
    assert_eq!(ast.variables[0].value, "value");
    assert_eq!(ast.variables[0].assignment_type, AssignmentType::Simple);
    assert_eq!(ast.variables[0].line, 1);
    assert!(diagnostics.is_empty(), "{}", messages(&diagnostics));
}

#[test]
fn test_parse_recursive_variable() {
    let (ast, _) = parse("VAR = value");

    assert_eq!(ast.variables.len(), 1);
    assert_eq!(ast.variables[0].assignment_type, AssignmentType::Recursive);
}

#[test]
fn test_parse_conditional_variable() {
    let (ast, _) = parse("VAR ?= default");

    assert_eq!(ast.variables.len(), 1);
    assert_eq!(ast.variables[0].name, "VAR");
    assert_eq!(ast.variables[0].value, "default");
    assert_eq!(ast.variables[0].assignment_type, AssignmentType::Conditional);
}

#[test]
fn test_parse_shell_variable() {
    let (ast, _) = parse("VERSION := $(shell git describe --tags)");

    assert_eq!(ast.variables.len(), 1);
    assert_eq!(ast.variables[0].assignment_type, AssignmentType::ShellExecution);
    assert!(ast.variables[0].value.contains("$(shell"));
}

#[test]
fn test_parse_simple_target() {
    let (ast, _) = parse("build:\n\techo Building");

    assert_eq!(ast.targets.len(), 1);
    assert_eq!(ast.targets[0].name, "build");
    assert_eq!(ast.targets[0].commands, vec!["echo Building".to_string()]);
    assert_eq!(ast.targets[0].line, 1);
}

#[test]
fn test_parse_target_with_dependencies() {
    let (ast, _) = parse("build: test clean\n\techo Building");

    assert_eq!(
        ast.targets[0].dependencies,
        vec!["test".to_string(), "clean".to_string()]
    );
}

#[test]
fn test_parse_target_with_comment() {
    let (ast, _) = parse("# Build the project\nbuild:\n\techo Building");

    assert_eq!(ast.targets[0].comment, Some("Build the project".to_string()));
}

#[test]
fn test_parse_phony_declaration() {
    let (ast, _) = parse(".PHONY: build clean test");

    assert_eq!(ast.phony_targets.len(), 3);
    assert!(ast.phony_targets.contains("build"));
    assert!(ast.phony_targets.contains("clean"));
    assert!(ast.phony_targets.contains("test"));
}

#[test]
fn test_phony_targets_mark_their_rules() {
    let (ast, _) = parse(".PHONY: build\n\nbuild:\n\techo hi\n\napp.bin:\n\techo bin");

    let build = ast.targets.iter().find(|t| t.name == "build").unwrap();
    let app = ast.targets.iter().find(|t| t.name == "app.bin").unwrap();
    assert!(build.is_phony, "a .PHONY target must be marked phony");
    assert!(!app.is_phony);
}

#[test]
fn test_parse_default_goal() {
    let (ast, _) = parse(".DEFAULT_GOAL := build");

    assert_eq!(ast.default_goal, Some("build".to_string()));
}

#[test]
fn test_parse_multiline_command() {
    let (ast, _) = parse("build:\n\tmkdir -p dist && \\\n\techo Done");

    assert_eq!(ast.targets[0].commands.len(), 1);
    assert!(ast.targets[0].commands[0].contains("mkdir -p dist"));
    assert!(ast.targets[0].commands[0].contains("echo Done"));
}

#[test]
fn test_parse_multiple_targets() {
    let (ast, _) = parse("build:\n\techo Building\n\ntest:\n\techo Testing");

    assert_eq!(ast.targets.len(), 2);
    assert_eq!(ast.targets[0].name, "build");
    assert_eq!(ast.targets[1].name, "test");
}

#[test]
fn test_parse_target_with_multiple_commands() {
    let (ast, _) = parse("build:\n\techo Starting\n\tmkdir -p dist\n\techo Done");

    assert_eq!(ast.targets[0].commands.len(), 3);
}

#[test]
fn test_parse_empty_makefile() {
    let (ast, _) = parse("");

    assert_eq!(ast.variables.len(), 0);
    assert_eq!(ast.targets.len(), 0);
}

#[test]
fn test_parse_comments_only() {
    let (ast, _) = parse("# This is a comment\n# Another comment");

    assert_eq!(ast.variables.len(), 0);
    assert_eq!(ast.targets.len(), 0);
}

#[test]
fn test_parse_complex_makefile() {
    let content = r#".DEFAULT_GOAL := build

VAR1 := value1
VAR2 = value2
VERSION := $(shell git describe --tags)

.PHONY: build clean test

# Build the project
build: test
	echo "Building version $(VERSION)"
	mkdir -p dist

# Run tests
test:
	go test ./...

clean:
	rm -rf dist
"#;

    let (ast, diagnostics) = parse(content);

    assert_eq!(ast.default_goal, Some("build".to_string()));
    assert_eq!(ast.variables.len(), 3);
    assert_eq!(ast.phony_targets.len(), 3);
    assert_eq!(ast.targets.len(), 3);
    assert!(diagnostics.is_empty(), "{}", messages(&diagnostics));
}

// --- continuations -----------------------------------------------------

#[test]
fn test_variable_continuation_is_joined() {
    let (ast, _) = parse("SOURCES := a.c \\\n  b.c \\\n  c.c\n");

    assert_eq!(ast.variables.len(), 1);
    assert_eq!(ast.variables[0].value, "a.c b.c c.c");
}

#[test]
fn test_space_indented_recipe_continuation_is_joined_not_invented() {
    let content = "build:\n\tdocker run --rm \\\n\t  -v /a:/b \\\n\t  --label foo\n";

    let (ast, _) = parse(content);

    assert_eq!(ast.targets.len(), 1, "the continuation must not become a second target");
    assert_eq!(
        ast.targets[0].commands,
        vec!["docker run --rm -v /a:/b --label foo".to_string()]
    );
}

#[test]
fn test_dependency_list_continuation_is_joined() {
    let (ast, _) = parse("all: build \\\n     test\n\techo done\n");

    assert_eq!(ast.targets.len(), 1);
    assert_eq!(
        ast.targets[0].dependencies,
        vec!["build".to_string(), "test".to_string()]
    );
}

#[test]
fn test_escaped_backslash_does_not_continue() {
    let (ast, _) = parse("VAR := back\\\\\nOTHER := two\n");

    assert_eq!(ast.variables.len(), 2);
    assert_eq!(ast.variables[1].name, "OTHER");
}

// --- errors ------------------------------------------------------------

#[test]
fn test_space_indented_recipe_is_rejected() {
    let mut parser = MakefileParser::new("build:\n    docker run -v /a:/b\n".to_string());

    let err = parser.parse().expect_err("a space-indented recipe must not convert");

    let message = err.to_string();
    assert!(message.contains("indented with spaces"), "{message}");
    assert!(message.contains("Makefile:2"), "{message}");
}

#[test]
fn test_space_indented_recipe_without_colon_is_rejected() {
    let mut parser = MakefileParser::new("build:\n    echo hi\n".to_string());

    let err = parser.parse().expect_err("a space-indented recipe must not convert");

    assert!(err.to_string().contains("indented with spaces"), "{err}");
}

#[test]
fn test_space_indented_assignment_is_not_an_error() {
    let (ast, _) = parse("build:\n\techo hi\n\n  INDENTED := ok\n");

    assert_eq!(ast.variables.len(), 1);
    assert_eq!(ast.variables[0].name, "INDENTED");
}

// --- constructs that used to corrupt silently --------------------------

#[test]
fn test_export_is_stripped_and_reported() {
    let (ast, diagnostics) = parse("export PATH := /usr/bin\n");

    assert_eq!(ast.variables.len(), 1);
    assert_eq!(
        ast.variables[0].name, "PATH",
        "`export` must not become part of the name"
    );
    assert!(
        messages(&diagnostics).contains("`export` on `PATH` dropped"),
        "{}",
        messages(&diagnostics)
    );
}

#[test]
fn test_target_specific_variable_is_not_a_dependency() {
    let (ast, diagnostics) = parse("build: CFLAGS=-g\n");

    assert!(ast.targets.is_empty(), "a target-specific variable is not a rule");
    assert!(
        messages(&diagnostics).contains("target-specific variable"),
        "{}",
        messages(&diagnostics)
    );
}

#[test]
fn test_multi_target_rule_becomes_one_task_each() {
    let (ast, diagnostics) = parse("test check: build\n\techo hi\n");

    assert_eq!(ast.targets.len(), 2);
    assert_eq!(ast.targets[0].name, "test");
    assert_eq!(ast.targets[1].name, "check");
    assert_eq!(ast.targets[1].dependencies, vec!["build".to_string()]);
    assert!(
        messages(&diagnostics).contains("declares 2 targets"),
        "{}",
        messages(&diagnostics)
    );
}

#[test]
fn test_double_colon_rule_keeps_its_dependencies() {
    let (ast, diagnostics) = parse("install:: build\n\techo hi\n");

    assert_eq!(ast.targets.len(), 1);
    assert_eq!(ast.targets[0].name, "install");
    assert_eq!(
        ast.targets[0].dependencies,
        vec!["build".to_string()],
        "`:` must not become a dependency"
    );
    assert!(
        messages(&diagnostics).contains("double-colon"),
        "{}",
        messages(&diagnostics)
    );
}

#[test]
fn test_pattern_rule_is_skipped_with_a_warning() {
    let (ast, diagnostics) = parse("%.o: %.c\n\tgcc -c $< -o $@\n\nbuild:\n\techo hi\n");

    assert_eq!(ast.targets.len(), 1, "the pattern rule must not become a task");
    assert_eq!(ast.targets[0].name, "build");
    assert!(
        messages(&diagnostics).contains("pattern rule"),
        "{}",
        messages(&diagnostics)
    );
}

#[test]
fn test_static_pattern_rule_is_skipped_with_a_warning() {
    let (ast, diagnostics) = parse("objs: %.o: %.c\n\tgcc -c $< -o $@\n");

    assert!(ast.targets.is_empty());
    assert!(
        messages(&diagnostics).contains("static pattern rule"),
        "{}",
        messages(&diagnostics)
    );
}

#[test]
fn test_inline_comment_is_not_a_dependency() {
    let (ast, _) = parse("help: ## Help me.\n\techo help\n");

    assert_eq!(ast.targets.len(), 1);
    assert!(
        ast.targets[0].dependencies.is_empty(),
        "`## Help me.` is a comment, not three deps"
    );
    assert_eq!(ast.targets[0].comment, Some("Help me.".to_string()));
}

#[test]
fn test_inline_comment_is_not_part_of_a_value() {
    let (ast, _) = parse("VERSION := 1.0 # bump me\n");

    assert_eq!(ast.variables[0].value, "1.0");
}

#[test]
fn test_blank_line_does_not_end_a_recipe() {
    let (ast, diagnostics) =
        parse("keygen:\n\tmkdir -p certs\n\n\topenssl genpkey -out certs/a.pem -pkeyopt bits:2048\n");

    assert_eq!(
        ast.targets.len(),
        1,
        "the second half of the recipe must not become a target"
    );
    assert_eq!(ast.targets[0].commands.len(), 2);
    assert!(diagnostics.is_empty(), "{}", messages(&diagnostics));
}

#[test]
fn test_make_function_variable_is_reported_not_silently_dropped() {
    let (ast, diagnostics) = parse("OBJS := $(wildcard *.c)\n");

    assert!(ast.variables.is_empty());
    assert!(
        messages(&diagnostics).contains("make function"),
        "{}",
        messages(&diagnostics)
    );
}

#[test]
fn test_include_is_reported() {
    let (_, diagnostics) = parse("include other.mk\n");

    assert!(
        messages(&diagnostics).contains("`include` is not supported"),
        "{}",
        messages(&diagnostics)
    );
}

#[test]
fn test_conditional_block_is_reported_once() {
    let (ast, diagnostics) = parse("ifeq ($(OS),Linux)\nCC := gcc\nendif\n");

    assert!(ast.variables.is_empty());
    assert_eq!(diagnostics.len(), 1, "{}", messages(&diagnostics));
    assert!(messages(&diagnostics).contains("conditional block skipped"));
}

#[test]
fn test_define_block_is_skipped_whole() {
    let (ast, diagnostics) = parse("define greet\nfoo: bar\nendef\n\nbuild:\n\techo hi\n");

    assert_eq!(
        ast.targets.len(),
        1,
        "the define body must not be parsed as makefile syntax"
    );
    assert_eq!(ast.targets[0].name, "build");
    assert!(messages(&diagnostics).contains("define"), "{}", messages(&diagnostics));
}
