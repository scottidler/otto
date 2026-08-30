#![cfg(test)]

use super::*;

#[test]
fn test_ast_creation() {
    let ast = MakefileAst::new();
    assert!(ast.variables.is_empty());
    assert!(ast.default_goal.is_none());
    assert!(ast.phony_targets.is_empty());
    assert!(ast.targets.is_empty());
}

#[test]
fn test_variable_creation() {
    let var = Variable {
        name: "VAR".to_string(),
        value: "value".to_string(),
        assignment_type: AssignmentType::Simple,
        line: 1,
    };
    assert_eq!(var.name, "VAR");
    assert_eq!(var.value, "value");
    assert_eq!(var.assignment_type, AssignmentType::Simple);
}

#[test]
fn test_target_creation() {
    let target = Target {
        name: "build".to_string(),
        dependencies: vec!["test".to_string()],
        commands: vec!["echo Building".to_string()],
        comment: Some("Build the project".to_string()),
        is_phony: true,
        line: 3,
    };
    assert_eq!(target.name, "build");
    assert_eq!(target.dependencies.len(), 1);
    assert_eq!(target.commands.len(), 1);
    assert!(target.comment.is_some());
    assert!(target.is_phony);
}
