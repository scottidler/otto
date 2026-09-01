impl Parser {
    fn inject_graph_meta_task(&mut self) {
        use crate::cfg::param::{Nargs, ParamType};

        let graph_task = TaskSpec {
            name: "Graph".to_string(),
            help: Some("[built-in] Visualize the task dependency graph".to_string()),
            after: vec![],
            before: vec![],
            input: vec![],
            output: vec![],
            envs: HashMap::new(),
            params: {
                let mut params = crate::cfg::param::ParamSpecs::new();

                params.insert(
                    "format".to_string(),
                    ParamSpec {
                        name: "format".to_string(),
                        short: Some('f'),
                        long: Some("format".to_string()),
                        param_type: ParamType::OPT,
                        metavar: None,
                        default: Some("ascii".to_string()),
                        choices_command: None,
                        choices: vec![
                            "ascii".to_string(),
                            "dot".to_string(),
                            "svg".to_string(),
                            "png".to_string(),
                            "pdf".to_string(),
                        ],
                        nargs: Nargs::One,
                        help: Some("Output format".to_string()),
                        value: Value::Empty,
                        required: false,
                    },
                );

                params.insert(
                    "output".to_string(),
                    ParamSpec {
                        name: "output".to_string(),
                        short: None,
                        long: Some("output".to_string()),
                        param_type: ParamType::OPT,
                        metavar: None,
                        default: None,
                        choices_command: None,
                        choices: vec![],
                        nargs: Nargs::One,
                        help: Some("Output file path".to_string()),
                        value: Value::Empty,
                        required: false,
                    },
                );

                params
            },
            action: "# Built-in graph command".to_string(),
            foreach: None,
            virtual_parent: false,
            tty: None,
            on_failure: vec![],
        };

        self.config_spec.tasks.insert("Graph".to_string(), graph_task);
    }

    fn inject_clean_meta_task(&mut self) {
        use crate::cfg::param::{Nargs, ParamType};

        let clean_task = TaskSpec {
            name: "Clean".to_string(),
            help: Some("[built-in] Clean old runs from ~/.otto/".to_string()),
            after: vec![],
            before: vec![],
            input: vec![],
            output: vec![],
            envs: HashMap::new(),
            params: {
                let mut params = crate::cfg::param::ParamSpecs::new();

                params.insert(
                    "keep".to_string(),
                    ParamSpec {
                        name: "keep".to_string(),
                        short: None,
                        long: Some("keep".to_string()),
                        param_type: ParamType::OPT,
                        metavar: Some("DAYS".to_string()),
                        default: Some("30".to_string()),
                        choices_command: None,
                        choices: vec![],
                        nargs: Nargs::One,
                        help: Some("Keep runs from the last N days".to_string()),
                        value: Value::Empty,
                        required: false,
                    },
                );

                params.insert(
                    "dry-run".to_string(),
                    ParamSpec {
                        name: "dry-run".to_string(),
                        short: None,
                        long: Some("dry-run".to_string()),
                        param_type: ParamType::FLG,
                        metavar: None,
                        default: None,
                        choices_command: None,
                        choices: vec![],
                        nargs: Nargs::Zero,
                        help: Some("Show what would be deleted without actually deleting".to_string()),
                        value: Value::Empty,
                        required: false,
                    },
                );

                params.insert(
                    "project".to_string(),
                    ParamSpec {
                        name: "project".to_string(),
                        short: None,
                        long: Some("project".to_string()),
                        param_type: ParamType::OPT,
                        metavar: Some("HASH".to_string()),
                        default: None,
                        choices_command: None,
                        choices: vec![],
                        nargs: Nargs::One,
                        help: Some("Only clean runs for a specific project".to_string()),
                        value: Value::Empty,
                        required: false,
                    },
                );

                params
            },
            action: "# Built-in clean command".to_string(),
            foreach: None,
            virtual_parent: false,
            tty: None,
            on_failure: vec![],
        };

        self.config_spec.tasks.insert("Clean".to_string(), clean_task);
    }

    fn inject_history_meta_task(&mut self) {
        use crate::cfg::param::{Nargs, ParamType};

        let history_task = TaskSpec {
            name: "History".to_string(),
            help: Some("[built-in] View execution history".to_string()),
            after: vec![],
            before: vec![],
            input: vec![],
            output: vec![],
            envs: HashMap::new(),
            params: {
                let mut params = crate::cfg::param::ParamSpecs::new();

                params.insert(
                    "task".to_string(),
                    ParamSpec {
                        name: "task".to_string(),
                        short: Some('t'),
                        long: Some("task".to_string()),
                        param_type: ParamType::OPT,
                        metavar: Some("TASK".to_string()),
                        default: None,
                        choices_command: None,
                        choices: vec![],
                        nargs: Nargs::One,
                        help: Some("Show history for a specific task".to_string()),
                        value: Value::Empty,
                        required: false,
                    },
                );

                params.insert(
                    "limit".to_string(),
                    ParamSpec {
                        name: "limit".to_string(),
                        short: Some('n'),
                        long: Some("limit".to_string()),
                        param_type: ParamType::OPT,
                        metavar: Some("N".to_string()),
                        default: Some("20".to_string()),
                        choices_command: None,
                        choices: vec![],
                        nargs: Nargs::One,
                        help: Some("Limit number of results".to_string()),
                        value: Value::Empty,
                        required: false,
                    },
                );

                params.insert(
                    "status".to_string(),
                    ParamSpec {
                        name: "status".to_string(),
                        short: Some('s'),
                        long: Some("status".to_string()),
                        param_type: ParamType::OPT,
                        metavar: Some("STATUS".to_string()),
                        default: None,
                        choices_command: None,
                        choices: vec!["success".to_string(), "failed".to_string(), "running".to_string()],
                        nargs: Nargs::One,
                        help: Some("Filter by status".to_string()),
                        value: Value::Empty,
                        required: false,
                    },
                );

                params.insert(
                    "project".to_string(),
                    ParamSpec {
                        name: "project".to_string(),
                        short: Some('p'),
                        long: Some("project".to_string()),
                        param_type: ParamType::OPT,
                        metavar: Some("HASH".to_string()),
                        default: None,
                        choices_command: None,
                        choices: vec![],
                        nargs: Nargs::One,
                        help: Some("Filter by project hash".to_string()),
                        value: Value::Empty,
                        required: false,
                    },
                );

                params.insert(
                    "json".to_string(),
                    ParamSpec {
                        name: "json".to_string(),
                        short: None,
                        long: Some("json".to_string()),
                        param_type: ParamType::FLG,
                        metavar: None,
                        default: None,
                        choices_command: None,
                        choices: vec![],
                        nargs: Nargs::Zero,
                        help: Some("Output as JSON".to_string()),
                        value: Value::Empty,
                        required: false,
                    },
                );

                params
            },
            action: "# Built-in history command".to_string(),
            foreach: None,
            virtual_parent: false,
            tty: None,
            on_failure: vec![],
        };

        self.config_spec.tasks.insert("History".to_string(), history_task);
    }

    fn inject_stats_meta_task(&mut self) {
        use crate::cfg::param::{Nargs, ParamType};

        let stats_task = TaskSpec {
            name: "Stats".to_string(),
            help: Some("[built-in] View execution statistics".to_string()),
            after: vec![],
            before: vec![],
            input: vec![],
            output: vec![],
            envs: HashMap::new(),
            params: {
                let mut params = crate::cfg::param::ParamSpecs::new();

                params.insert(
                    "task".to_string(),
                    ParamSpec {
                        name: "task".to_string(),
                        short: Some('t'),
                        long: Some("task".to_string()),
                        param_type: ParamType::OPT,
                        metavar: Some("TASK".to_string()),
                        default: None,
                        choices_command: None,
                        choices: vec![],
                        nargs: Nargs::One,
                        help: Some("Show stats for a specific task".to_string()),
                        value: Value::Empty,
                        required: false,
                    },
                );

                params.insert(
                    "limit".to_string(),
                    ParamSpec {
                        name: "limit".to_string(),
                        short: Some('n'),
                        long: Some("limit".to_string()),
                        param_type: ParamType::OPT,
                        metavar: Some("N".to_string()),
                        default: Some("10".to_string()),
                        choices_command: None,
                        choices: vec![],
                        nargs: Nargs::One,
                        help: Some("Limit number of tasks shown".to_string()),
                        value: Value::Empty,
                        required: false,
                    },
                );

                params.insert(
                    "json".to_string(),
                    ParamSpec {
                        name: "json".to_string(),
                        short: None,
                        long: Some("json".to_string()),
                        param_type: ParamType::FLG,
                        metavar: None,
                        default: None,
                        choices_command: None,
                        choices: vec![],
                        nargs: Nargs::Zero,
                        help: Some("Output as JSON".to_string()),
                        value: Value::Empty,
                        required: false,
                    },
                );

                params
            },
            action: "# Built-in stats command".to_string(),
            foreach: None,
            virtual_parent: false,
            tty: None,
            on_failure: vec![],
        };

        self.config_spec.tasks.insert("Stats".to_string(), stats_task);
    }

    fn inject_convert_meta_task(&mut self) {
        use crate::cfg::param::{Nargs, ParamType};

        let convert_task = TaskSpec {
            name: "Convert".to_string(),
            help: Some("[built-in] Convert Makefile to Otto YAML format".to_string()),
            after: vec![],
            before: vec![],
            input: vec![],
            output: vec![],
            envs: HashMap::new(),
            params: {
                let mut params = crate::cfg::param::ParamSpecs::new();

                params.insert(
                    "strict".to_string(),
                    ParamSpec {
                        name: "strict".to_string(),
                        short: None,
                        long: Some("strict".to_string()),
                        param_type: ParamType::FLG,
                        metavar: None,
                        default: None,
                        choices_command: None,
                        choices: vec![],
                        nargs: Nargs::Zero,
                        help: Some("Treat warnings as errors".to_string()),
                        value: Value::Empty,
                        required: false,
                    },
                );

                params.insert(
                    "output".to_string(),
                    ParamSpec {
                        name: "output".to_string(),
                        short: Some('o'),
                        long: Some("output".to_string()),
                        param_type: ParamType::OPT,
                        metavar: Some("FILE".to_string()),
                        default: None,
                        choices_command: None,
                        choices: vec![],
                        nargs: Nargs::One,
                        help: Some("Output file (default: stdout)".to_string()),
                        value: Value::Empty,
                        required: false,
                    },
                );

                params
            },
            action: "# Built-in convert command".to_string(),
            foreach: None,
            virtual_parent: false,
            tty: None,
            on_failure: vec![],
        };

        self.config_spec.tasks.insert("Convert".to_string(), convert_task);
    }

    fn inject_upgrade_meta_task(&mut self) {
        use crate::cfg::param::{Nargs, ParamType};

        let upgrade_task = TaskSpec {
            name: "Upgrade".to_string(),
            help: Some("[built-in] Upgrade Otto to a newer version".to_string()),
            after: vec![],
            before: vec![],
            input: vec![],
            output: vec![],
            envs: HashMap::new(),
            params: {
                let mut params = crate::cfg::param::ParamSpecs::new();

                params.insert(
                    "dry-run".to_string(),
                    ParamSpec {
                        name: "dry-run".to_string(),
                        short: None,
                        long: Some("dry-run".to_string()),
                        param_type: ParamType::FLG,
                        metavar: None,
                        default: None,
                        choices_command: None,
                        choices: vec![],
                        nargs: Nargs::Zero,
                        help: Some("Show what would be done without doing it".to_string()),
                        value: Value::Empty,
                        required: false,
                    },
                );

                params.insert(
                    "version".to_string(),
                    ParamSpec {
                        name: "version".to_string(),
                        short: Some('v'),
                        long: Some("version".to_string()),
                        param_type: ParamType::OPT,
                        metavar: Some("VERSION".to_string()),
                        default: None,
                        choices_command: None,
                        choices: vec![],
                        nargs: Nargs::One,
                        help: Some("Specific version to upgrade to".to_string()),
                        value: Value::Empty,
                        required: false,
                    },
                );

                params.insert(
                    "list-versions".to_string(),
                    ParamSpec {
                        name: "list-versions".to_string(),
                        short: None,
                        long: Some("list-versions".to_string()),
                        param_type: ParamType::FLG,
                        metavar: None,
                        default: None,
                        choices_command: None,
                        choices: vec![],
                        nargs: Nargs::Zero,
                        help: Some("List available versions".to_string()),
                        value: Value::Empty,
                        required: false,
                    },
                );

                params.insert(
                    "rollback".to_string(),
                    ParamSpec {
                        name: "rollback".to_string(),
                        short: None,
                        long: Some("rollback".to_string()),
                        param_type: ParamType::FLG,
                        metavar: None,
                        default: None,
                        choices_command: None,
                        choices: vec![],
                        nargs: Nargs::Zero,
                        help: Some("Rollback to previous version".to_string()),
                        value: Value::Empty,
                        required: false,
                    },
                );

                params.insert(
                    "force".to_string(),
                    ParamSpec {
                        name: "force".to_string(),
                        short: None,
                        long: Some("force".to_string()),
                        param_type: ParamType::FLG,
                        metavar: None,
                        default: None,
                        choices_command: None,
                        choices: vec![],
                        nargs: Nargs::Zero,
                        help: Some("Force upgrade even if already on target version".to_string()),
                        value: Value::Empty,
                        required: false,
                    },
                );

                params.insert(
                    "no-backup".to_string(),
                    ParamSpec {
                        name: "no-backup".to_string(),
                        short: None,
                        long: Some("no-backup".to_string()),
                        param_type: ParamType::FLG,
                        metavar: None,
                        default: None,
                        choices_command: None,
                        choices: vec![],
                        nargs: Nargs::Zero,
                        help: Some("Skip creating backup".to_string()),
                        value: Value::Empty,
                        required: false,
                    },
                );

                params
            },
            action: "# Built-in upgrade command".to_string(),
            foreach: None,
            virtual_parent: false,
            tty: None,
            on_failure: vec![],
        };

        self.config_spec.tasks.insert("Upgrade".to_string(), upgrade_task);
    }

    fn inject_builtin_commands(&mut self) {
        self.inject_clean_meta_task();
        self.inject_convert_meta_task();
        self.inject_graph_meta_task();
        self.inject_history_meta_task();
        self.inject_stats_meta_task();
        self.inject_upgrade_meta_task();
    }

}
