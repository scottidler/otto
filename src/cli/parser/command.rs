impl Parser {

    /// Build a clap `Command` for a task.
    ///
    /// The mode is explicit because the two callers want opposite things from a
    /// dynamic source, and the distinction used to ride on `cwd: Option<&Path>`
    /// (help passed `Some`, bind passed `None`) - an implicit discriminator that
    /// Phase 6b inverts, since the bind path is the one that needs the ottofile
    /// directory and the resolver. Both modes now read `self`, so the mode says
    /// what it means: `Help` describes dynamic sources, `Bind` resolves them.
    fn task_to_command(&self, task_spec: &TaskSpec, mode: BuildMode) -> Result<Command> {
        let mut cmd = Command::new(task_spec.name.clone());

        // Build help text, including foreach count if available
        let help_text = if let Some(ref foreach) = task_spec.foreach {
            // A command source is never executed to render help: the item count
            // isn't knowable without running user code, so help says so and
            // stays execution-free (design doc Phase 6, and the same rule
            // Phase 6b applies to dynamic param choices).
            let foreach_indicator = if foreach.is_command_source() {
                " [dynamic]".to_string()
            } else if mode == BuildMode::Help {
                let count = foreach.resolve_items(self.base_dir()).map_or(0, |items| items.len());
                if count > 0 { format!(" [{count} items]") } else { " [foreach]".to_string() }
            } else {
                // Binding never renders this string; don't walk the filesystem for it.
                " [foreach]".to_string()
            };

            match &task_spec.help {
                Some(help) => format!("{help}{foreach_indicator}"),
                None => foreach_indicator,
            }
        } else {
            task_spec.help.clone().unwrap_or_default()
        };

        if !help_text.is_empty() {
            cmd = cmd.about(help_text);
        }

        for param_spec in task_spec.params.values() {
            let arg = self.param_to_arg(&task_spec.name, param_spec, mode)?;
            cmd = cmd.arg(arg);
        }

        // Auto-inject --Serial flag for foreach tasks
        if task_spec.has_foreach() {
            cmd = cmd.arg(
                Arg::new("Serial")
                    .long("Serial")
                    .help("[builtin] Run subtasks sequentially instead of in parallel")
                    .action(clap::ArgAction::SetTrue),
            );
        }

        Ok(cmd)
    }

    /// Help-mode wrapper: rendering help can never fail, because it never runs
    /// anything. Keeping the infallible signature makes that guarantee visible
    /// at every help call site.
    fn task_to_command_for_help(&self, task_spec: &TaskSpec) -> Command {
        self.task_to_command(task_spec, BuildMode::Help)
            .expect("help mode resolves no dynamic sources and cannot fail")
    }

    fn param_to_arg(&self, task_name: &str, param_spec: &ParamSpec, mode: BuildMode) -> Result<Arg> {
        let mut arg = Arg::new(param_spec.name.clone());

        if let Some(short) = param_spec.short {
            arg = arg.short(short);
        }

        if let Some(ref long) = param_spec.long {
            arg = arg.long(long.clone());
        }

        // A dynamic value set can't be listed in help without executing the
        // command, so help names the command instead and a human can run it.
        let help_text = match (&param_spec.help, &param_spec.choices_command) {
            (Some(help), Some(command)) => Some(format!("{help} [dynamic choices: {command}]")),
            (None, Some(command)) => Some(format!("[dynamic choices: {command}]")),
            (help, None) => help.clone(),
        };
        if let Some(help) = help_text {
            arg = arg.help(help);
        }

        // Handle different parameter types
        match param_spec.param_type {
            ParamType::FLG => {
                // Boolean flag - no value required
                arg = arg.action(clap::ArgAction::SetTrue);
            }
            ParamType::OPT | ParamType::POS => {
                // Argument with value
                arg = arg.value_parser(value_parser!(String));

                // `nargs` had zero readers outside cfg/param.rs: parsed,
                // serialized, and never consulted when building the clap
                // `Arg`, so every param accepted exactly one value no matter
                // what `nargs:` said. Space-separated only (`num_args`),
                // never `value_delimiter` (comma-splitting a value is a
                // different, unrequested feature).
                arg = arg.num_args(nargs_to_num_args(&param_spec.nargs));

                if let Some(ref default) = param_spec.default {
                    arg = arg.default_value(default.clone());
                }

                let choices = self.param_choices(task_name, param_spec, mode)?;
                if !choices.is_empty() {
                    // Case-insensitive per the CLI rule: a tool that prints
                    // `ASCII` and then rejects `ASCII` is friction with no
                    // benefit. The bound value is canonicalized back to the
                    // declared spelling at bind time.
                    arg = arg
                        .value_parser(clap::builder::PossibleValuesParser::new(choices))
                        .ignore_case(true);
                }
            }
        }

        // Handle positional arguments
        if param_spec.param_type == ParamType::POS {
            let value_name = param_spec
                .metavar
                .as_deref()
                .unwrap_or(param_spec.name.as_str())
                .to_string();
            arg = arg.value_name(value_name);
        }

        Ok(arg)
    }

    /// The allowed value set for a param, resolving a `choices-command:` source
    /// at most once per invocation.
    ///
    /// In `Help` mode a dynamic source yields no set at all: help must execute
    /// nothing, so it renders `[dynamic choices: ...]` and leaves the value
    /// unconstrained (nothing is being validated on a help path anyway). Both
    /// bind triggers - direct invocation here, propagated-value validation in
    /// `propagate_params` - come through this one function, which is why they
    /// share the cache instead of running the command twice.
    fn param_choices(&self, task_name: &str, param_spec: &ParamSpec, mode: BuildMode) -> Result<Vec<String>> {
        if !param_spec.has_dynamic_choices() {
            return Ok(param_spec.choices.clone());
        }
        if mode == BuildMode::Help {
            return Ok(Vec::new());
        }
        let key = format!("{task_name}:{}", param_spec.name);
        self.resolver.choices(&key, || {
            param_spec.resolve_choices_command(task_name, self.base_dir(), self.global_envs()?)
        })
    }

    fn build_help_command(&self) -> Command {
        let mut cmd = Command::new("otto")
            .version(env!("GIT_DESCRIBE"))
            .about("A task runner");
        for arg in Self::global_args() {
            cmd = cmd.arg(arg);
        }
        cmd = cmd.allow_external_subcommands(true);

        if !self.config_spec.tasks.is_empty() {
            // Separate regular tasks from built-in commands
            let mut regular_tasks: Vec<_> = self
                .config_spec
                .tasks
                .iter()
                .filter(|(name, _)| !BUILTIN_COMMANDS.contains(&name.as_str()))
                .collect();
            regular_tasks.sort_by_key(|(name, _)| name.as_str());

            for (_, task_spec) in regular_tasks {
                cmd = cmd.subcommand(self.task_to_command_for_help(task_spec));
            }

            // Collect and sort built-in commands
            let mut builtins: Vec<(&String, &TaskSpec)> = self
                .config_spec
                .tasks
                .iter()
                .filter(|(name, _)| BUILTIN_COMMANDS.contains(&name.as_str()))
                .collect();
            builtins.sort_by_key(|(name, _)| name.as_str());

            for (_, task_spec) in builtins {
                cmd = cmd.subcommand(self.task_to_command_for_help(task_spec));
            }
        } else {
            cmd = cmd.after_help(ottofile_not_found_message());
        }

        cmd
    }

    /// Global flags only, no epilogue: the shared base for both config-failure
    /// help fallbacks. Split out so the "found but unparseable" path can render
    /// the same flag list without inheriting the not-found claim.
    fn build_bare_help_command() -> Command {
        let mut cmd = Command::new("otto")
            .version(env!("GIT_DESCRIBE"))
            .about("A task runner");
        for arg in Self::global_args() {
            cmd = cmd.arg(arg);
        }
        cmd.allow_external_subcommands(true)
    }

    /// The fallback for the "no ottofile anywhere up the tree" state only.
    fn build_help_command_with_error() -> Command {
        Self::build_bare_help_command().after_help(ottofile_not_found_message())
    }

}
