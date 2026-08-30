use std::collections::HashSet;

/// Abstract Syntax Tree representation of a Makefile
#[derive(Debug, Clone, PartialEq)]
pub struct MakefileAst {
    pub variables: Vec<Variable>,
    pub default_goal: Option<String>,
    pub phony_targets: HashSet<String>,
    pub targets: Vec<Target>,
}

impl MakefileAst {
    pub fn new() -> Self {
        Self {
            variables: Vec::new(),
            default_goal: None,
            phony_targets: HashSet::new(),
            targets: Vec::new(),
        }
    }
}

impl Default for MakefileAst {
    fn default() -> Self {
        Self::new()
    }
}

/// Represents a variable assignment in the Makefile
#[derive(Debug, Clone, PartialEq)]
pub struct Variable {
    pub name: String,
    pub value: String,
    pub assignment_type: AssignmentType,
    /// 1-based physical line the assignment starts on, so a converter warning
    /// about this variable can name where to go fix it.
    pub line: usize,
}

/// Types of variable assignments in Make
#[derive(Debug, Clone, PartialEq)]
pub enum AssignmentType {
    Simple,         // :=
    Recursive,      // =
    Conditional,    // ?=
    Append,         // +=
    ShellExecution, // $(shell ...)
}

/// Represents a target (rule) in the Makefile
#[derive(Debug, Clone, PartialEq)]
pub struct Target {
    pub name: String,
    pub dependencies: Vec<String>,
    pub commands: Vec<String>,
    pub comment: Option<String>,
    pub is_phony: bool,
    /// 1-based physical line the rule starts on. Also what tells a duplicate
    /// target apart from the rule it overrides.
    pub line: usize,
}

#[path = "ast_tests.rs"]
mod tests;
