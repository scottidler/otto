use eyre::Result;
use std::collections::HashMap;

use crate::cfg::config::ConfigSpec;
use crate::cfg::otto::{OttoSpec, RetentionSpec};
use crate::cfg::task::{TaskSpec, TaskSpecs};

use super::ast::{AssignmentType, MakefileAst, Target};

pub struct OttoConverter {
    ast: MakefileAst,
}

impl OttoConverter {
    pub fn new(ast: MakefileAst) -> Self {
        Self { ast }
    }

    pub fn convert(&self) -> Result<ConfigSpec> {
        let otto_spec = self.convert_otto_spec()?;
        let tasks = self.convert_targets()?;

        Ok(ConfigSpec { otto: otto_spec, tasks })
    }

    fn convert_otto_spec(&self) -> Result<OttoSpec> {
        let envs = self.convert_variables();
        let tasks = self.determine_default_tasks();

        Ok(OttoSpec {
            name: "otto".to_string(),
            about: "Converted from Makefile".to_string(),
            api: "1".to_string(),
            jobs: num_cpus::get(),
            home: "~/.otto".to_string(),
            tasks,
            verbosity: 1,
            envs,
            retention: RetentionSpec::default(),
        })
    }

    fn convert_variables(&self) -> HashMap<String, String> {
        let mut envs = HashMap::new();

        for var in &self.ast.variables {
            // For shell executions, preserve the $(shell ...) syntax
            // Otto will evaluate these at runtime
            let value = match var.assignment_type {
                AssignmentType::ShellExecution => {
                    // Keep the shell command syntax
                    var.value.clone()
                }
                AssignmentType::Simple | AssignmentType::Recursive | AssignmentType::Conditional => var.value.clone(),
                AssignmentType::Append => {
                    // For append, we need to reference the existing value
                    // Otto doesn't support this directly, so we just use the appended value
                    var.value.clone()
                }
            };

            envs.insert(var.name.clone(), value);
        }

        envs
    }

    fn convert_targets(&self) -> Result<TaskSpecs> {
        let mut tasks = HashMap::new();

        for target in &self.ast.targets {
            let task = self.convert_target_to_task(target)?;
            tasks.insert(target.name.clone(), task);
        }

        Ok(tasks)
    }

    fn convert_target_to_task(&self, target: &Target) -> Result<TaskSpec> {
        // Build the bash script from commands
        let action = self.build_bash_action(&target.commands);

        // Dependencies in Make become "before" in Otto
        // (task X depends on Y means Y must run before X)
        // Emit as sugar (bare-string) edges so converted ottofiles read naturally.
        let before: Vec<crate::cfg::edge::EdgeSpec> = target
            .dependencies
            .iter()
            .map(crate::cfg::edge::EdgeSpec::sugar)
            .collect();

        Ok(TaskSpec {
            name: target.name.clone(),
            help: target.comment.clone(),
            after: Vec::new(),
            before,
            input: Vec::new(),
            output: Vec::new(),
            envs: HashMap::new(),
            params: HashMap::new(),
            action,
            foreach: None,
            virtual_parent: false,
            tty: None,
            on_failure: Vec::new(),
        })
    }

    fn build_bash_action(&self, commands: &[String]) -> String {
        if commands.is_empty() {
            return "#!/bin/bash\n".to_string();
        }

        let mut script = String::from("#!/bin/bash\n");

        for cmd in commands {
            let trimmed = cmd.trim();

            // Remove Make-specific prefixes
            let cleaned_cmd = if let Some(cmd_without_at) = trimmed.strip_prefix('@') {
                // @ suppresses echo in Make, not needed in Otto
                cmd_without_at.trim_start()
            } else if let Some(cmd_without_dash) = trimmed.strip_prefix('-') {
                // - ignores errors in Make
                // In Otto/bash, we can use `|| true` for this
                let cmd_without_prefix = cmd_without_dash.trim_start();
                script.push_str(cmd_without_prefix);
                script.push_str(" || true\n");
                continue;
            } else {
                trimmed
            };

            script.push_str(cleaned_cmd);
            script.push('\n');
        }

        script
    }

    fn determine_default_tasks(&self) -> Vec<String> {
        if let Some(ref default_goal) = self.ast.default_goal {
            vec![default_goal.clone()]
        } else if !self.ast.targets.is_empty() {
            // If no default goal, use the first target
            vec![self.ast.targets[0].name.clone()]
        } else {
            vec!["*".to_string()]
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::makefile::ast::{Target, Variable};

    #[test]
    fn test_convert_simple_variable() {
        let mut ast = MakefileAst::new();
        ast.variables.push(Variable {
            name: "VAR".to_string(),
            value: "value".to_string(),
            assignment_type: AssignmentType::Simple,
        });

        let converter = OttoConverter::new(ast);
        let config = converter.convert().unwrap();

        assert_eq!(config.otto.envs.get("VAR"), Some(&"value".to_string()));
    }

    #[test]
    fn test_convert_shell_variable() {
        let mut ast = MakefileAst::new();
        ast.variables.push(Variable {
            name: "VERSION".to_string(),
            value: "$(shell git describe --tags)".to_string(),
            assignment_type: AssignmentType::ShellExecution,
        });

        let converter = OttoConverter::new(ast);
        let config = converter.convert().unwrap();

        let version_value = config.otto.envs.get("VERSION").unwrap();
        assert!(version_value.contains("$(shell"));
    }

    #[test]
    fn test_convert_simple_target() {
        let mut ast = MakefileAst::new();
        ast.targets.push(Target {
            name: "build".to_string(),
            dependencies: Vec::new(),
            commands: vec!["echo Building".to_string()],
            comment: None,
            is_phony: false,
        });

        let converter = OttoConverter::new(ast);
        let config = converter.convert().unwrap();

        assert!(config.tasks.contains_key("build"));
        let task = config.tasks.get("build").unwrap();
        assert_eq!(task.name, "build");
        assert!(task.action.contains("echo Building"));
        assert!(task.action.starts_with("#!/bin/bash"));
    }

    #[test]
    fn test_convert_target_with_dependencies() {
        let mut ast = MakefileAst::new();
        ast.targets.push(Target {
            name: "build".to_string(),
            dependencies: vec!["test".to_string(), "clean".to_string()],
            commands: vec!["echo Building".to_string()],
            comment: None,
            is_phony: false,
        });

        let converter = OttoConverter::new(ast);
        let config = converter.convert().unwrap();

        let task = config.tasks.get("build").unwrap();
        assert_eq!(task.before.len(), 2);
        assert!(task.before.iter().any(|e| e.task == "test"));
        assert!(task.before.iter().any(|e| e.task == "clean"));
    }

    #[test]
    fn test_convert_target_with_comment() {
        let mut ast = MakefileAst::new();
        ast.targets.push(Target {
            name: "build".to_string(),
            dependencies: Vec::new(),
            commands: vec!["echo Building".to_string()],
            comment: Some("Build the project".to_string()),
            is_phony: false,
        });

        let converter = OttoConverter::new(ast);
        let config = converter.convert().unwrap();

        let task = config.tasks.get("build").unwrap();
        assert_eq!(task.help, Some("Build the project".to_string()));
    }

    #[test]
    fn test_convert_multiple_commands() {
        let mut ast = MakefileAst::new();
        ast.targets.push(Target {
            name: "build".to_string(),
            dependencies: Vec::new(),
            commands: vec![
                "mkdir -p dist".to_string(),
                "echo Building".to_string(),
                "echo Done".to_string(),
            ],
            comment: None,
            is_phony: false,
        });

        let converter = OttoConverter::new(ast);
        let config = converter.convert().unwrap();

        let task = config.tasks.get("build").unwrap();
        assert!(task.action.contains("mkdir -p dist"));
        assert!(task.action.contains("echo Building"));
        assert!(task.action.contains("echo Done"));
    }

    #[test]
    fn test_default_goal_conversion() {
        let mut ast = MakefileAst::new();
        ast.default_goal = Some("build".to_string());
        ast.targets.push(Target {
            name: "build".to_string(),
            dependencies: Vec::new(),
            commands: vec!["echo Building".to_string()],
            comment: None,
            is_phony: false,
        });

        let converter = OttoConverter::new(ast);
        let config = converter.convert().unwrap();

        assert_eq!(config.otto.tasks, vec!["build".to_string()]);
    }

    #[test]
    fn test_no_default_goal_uses_first_target() {
        let mut ast = MakefileAst::new();
        ast.targets.push(Target {
            name: "build".to_string(),
            dependencies: Vec::new(),
            commands: vec!["echo Building".to_string()],
            comment: None,
            is_phony: false,
        });
        ast.targets.push(Target {
            name: "test".to_string(),
            dependencies: Vec::new(),
            commands: vec!["echo Testing".to_string()],
            comment: None,
            is_phony: false,
        });

        let converter = OttoConverter::new(ast);
        let config = converter.convert().unwrap();

        assert_eq!(config.otto.tasks, vec!["build".to_string()]);
    }

    #[test]
    fn test_command_prefix_handling() {
        let mut ast = MakefileAst::new();
        ast.targets.push(Target {
            name: "build".to_string(),
            dependencies: Vec::new(),
            commands: vec![
                "@echo Hidden".to_string(),
                "-mkdir -p dist".to_string(),
                "echo Visible".to_string(),
            ],
            comment: None,
            is_phony: false,
        });

        let converter = OttoConverter::new(ast);
        let config = converter.convert().unwrap();

        let task = config.tasks.get("build").unwrap();
        // @ prefix should be removed
        assert!(task.action.contains("echo Hidden"));
        assert!(!task.action.contains("@echo"));
        // - prefix should be converted to || true
        assert!(task.action.contains("mkdir -p dist || true"));
        // Normal command should remain
        assert!(task.action.contains("echo Visible"));
    }

    #[test]
    fn test_empty_makefile() {
        let ast = MakefileAst::new();
        let converter = OttoConverter::new(ast);
        let config = converter.convert().unwrap();

        assert!(config.tasks.is_empty());
        assert_eq!(config.otto.tasks, vec!["*".to_string()]);
    }
}
