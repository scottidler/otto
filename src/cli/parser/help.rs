impl Parser {
    /// Single source of truth for otto's global flags: cwd, ottofile,
    /// list-subtasks, jobs, tui, no-prefix. `otto_command()`, `build_help_command()`,
    /// and `build_help_command_with_error()` all consume this so the
    /// rendered `--help` output can never drift from what's actually parsed
    /// again (see docs/design/2026-08-28-boundary-fixes-and-dynamic-foreach.md
    /// Phase 1 - the prior drift: the two help builders re-declared only
    /// `jobs` and `tui`, silently omitting `-C/--cwd`, `-o/--ottofile`, and
    /// `--list-subtasks` from `otto --help`).
    fn global_args() -> Vec<Arg> {
        vec![
            Arg::new("cwd")
                .short('C')
                .long("cwd")
                .value_name("DIR")
                .help("Change to DIR before doing anything")
                .value_parser(value_parser!(String)),
            Arg::new("ottofile")
                .short('o')
                .long("ottofile")
                .value_name("PATH")
                .help("path to the ottofile (default: search upward from the current directory)")
                .env("OTTOFILE")
                .value_parser(value_parser!(String)),
            Arg::new("list-subtasks")
                .long("list-subtasks")
                .help("List all foreach subtasks and exit")
                .action(clap::ArgAction::SetTrue),
            Arg::new("tasks")
                .long("tasks")
                .help("Print the machine-readable task list and exit")
                .action(clap::ArgAction::SetTrue),
            Arg::new("format")
                .long("format")
                .value_name("FORMAT")
                .help("Output format for --tasks (yaml or json); default: yaml on a tty, json when piped")
                .value_parser(["yaml", "json"])
                .ignore_case(true)
                // Without --tasks this flag has nothing to format, and silently
                // fell through to running the ottofile's default tasks instead
                // of erroring (docs/design/2026-09-06-shakedown-remediation.md
                // Phase 6, F7). global_args() is also consumed by the two help
                // builders in command.rs, which only render a Command for
                // --help and never call try_get_matches_from, so `requires` is
                // inert there and only bites the real parse path in
                // Parser::parse() (parser.rs).
                .requires("tasks"),
            Arg::new("jobs")
                .short('j')
                .long("jobs")
                .value_name("N")
                .help("Number of parallel jobs")
                .default_value(DEFAULT_JOBS.as_str())
                .value_parser(value_parser!(u64).range(1..)),
            Arg::new("tui")
                .short('t')
                .long("tui")
                .help("Enable interactive TUI dashboard for task monitoring")
                .action(clap::ArgAction::SetTrue)
                .global(true),
            Arg::new("no-prefix")
                .long("no-prefix")
                .help("Suppress the [task] prefix on task output")
                .action(clap::ArgAction::SetTrue),
            // Declared here so `--help` lists it; `main` strips it from the
            // args before this command ever parses them, because logging is
            // configured before the parser exists. Same arrangement as -C/--cwd.
            Arg::new("log-level")
                .long("log-level")
                .value_name("LEVEL")
                .help("Verbosity of otto's own log file, under $XDG_DATA_HOME/otto/logs")
                .value_parser(clap::builder::PossibleValuesParser::new(LOG_LEVELS))
                .ignore_case(true),
        ]
    }

    fn otto_command() -> Command {
        let mut cmd = Command::new("otto")
            .version(env!("GIT_DESCRIBE"))
            .about("A task runner");
        for arg in Self::global_args() {
            cmd = cmd.arg(arg);
        }
        cmd.allow_external_subcommands(true)
    }

    fn extract_remaining_args(&self, matches: &ArgMatches) -> Vec<String> {
        // Handle external subcommands properly
        if let Some((subcommand_name, sub_matches)) = matches.subcommand() {
            let mut args = vec![subcommand_name.to_string()];

            // For external subcommands, collect all the trailing arguments
            // The key for external subcommand arguments is usually "" (empty string)
            // Note: external subcommands store args as OsString, not String
            if let Some(trailing_args) = sub_matches.get_many::<std::ffi::OsString>("") {
                args.extend(trailing_args.map(|s| s.to_string_lossy().to_string()));
            }

            args
        } else {
            // No subcommand found, return empty
            vec![]
        }
    }

    /// True when some task named in `args` declares `-<short>` as one of its own
    /// params.
    ///
    /// otto intercepts a handful of single-letter tokens (`-h` for help, `-t`
    /// for the TUI) before a task's arguments are bound. Every interception is
    /// also a name otto takes away from the ottofile author, so each one asks
    /// this first: a task that declares the short owns it, and otto keeps its
    /// hands off. Reads declarations only - resolves nothing, runs nothing.
    ///
    /// A subtask id is looked up through its parent: `up:gamma` is not a key of
    /// the task map, and asking for it directly answered "no task claims this"
    /// for every foreach subtask, so a parent that declares `-t` or `-h` lost it
    /// the moment the user addressed one of its items. `value_taking_options`
    /// (`discovery.rs`) already partitions subtask arguments through the same
    /// mapping.
    fn args_claim_short(&self, args: &[String], short: char) -> bool {
        args.iter().any(|arg| {
            self.config_spec
                .tasks
                .get(crate::naming::parent_or_self(arg))
                .is_some_and(|spec| spec.params.values().any(|param| param.short == Some(short)))
        })
    }

    /// True when `args` asks for help: `--help` always, `-h` only when no task
    /// in the list declared `-h` itself (a task with `-h|--host` could never be
    /// given a host, because otto answered with help every time).
    fn help_requested_in(&self, args: &[String]) -> bool {
        args.iter().any(|arg| arg == "--help") || (args.iter().any(|arg| arg == "-h") && !self.args_claim_short(args, 'h'))
    }

    /// Every task name in this ottofile that the user wrote, built-ins excluded.
    ///
    /// `inject_builtin_commands` runs before every help decision, so
    /// `self.config_spec.tasks` is never empty by the time anything asks - the
    /// six builtins are always in it. Asking `is_empty()` therefore always
    /// answered "no", which is why an ottofile with no tasks of its own printed
    /// "No tasks to execute" instead of help.
    fn has_user_tasks(&self) -> bool {
        self.config_spec.tasks.keys().any(|name| !is_builtin(name))
    }

    fn should_show_help(&self, args: &[String]) -> bool {
        // Show help if:
        // 1. Explicit help command: "otto help" or "otto help <task>"
        // 2. Task with --help or -h flag: "otto <task> --help"
        // 3. No args AND no default tasks defined
        if !args.is_empty() {
            if args[0] == "help" {
                return true;
            }
            // Check if --help or -h is present (subcommand help)
            if self.help_requested_in(args) {
                return true;
            }
            // An explicit task request is never a help request. Falling through
            // to the default-tasks check below is what made `otto build` with
            // `otto: tasks: []` print nothing and exit 0.
            return false;
        }

        let default_tasks = &self.config_spec.otto.tasks;
        default_tasks.is_empty() || (default_tasks.len() == 1 && default_tasks[0] == "*" && !self.has_user_tasks())
    }

    fn show_help(&self, args: &[String]) -> Result<()> {
        if args.is_empty() {
            // Show general help (no default tasks case)
            let mut help_cmd = self.build_help_command();
            help_cmd.print_help()?;
        } else if args.len() == 1 && args[0] == "help" {
            // "otto help" - show general help
            let mut help_cmd = self.build_help_command();
            help_cmd.print_help()?;
        } else if args.len() == 2 && args[0] == "help" {
            // "otto help <task>" - show task-specific help
            let task_name = &args[1];
            self.show_task_help(task_name)?;
        } else if self.help_requested_in(args) {
            // "otto <task> --help" or "otto <task> -h" - show task-specific help
            let task_name = &args[0];
            self.show_task_help(task_name)?;
        } else {
            // Exhaustive on purpose: the missing else used to swallow
            // `otto help build extra` into a silent exit 0.
            return Err(eyre!(
                "unrecognized help request: {}; usage: otto help [TASK]",
                args.join(" ")
            ));
        }
        Ok(())
    }

    fn show_task_help(&self, task_name: &str) -> Result<()> {
        if let Some(task) = self.config_spec.tasks.get(task_name) {
            let mut task_cmd = self.task_to_command_for_help(task).bin_name(format!("otto {task_name}"));
            task_cmd.print_help()?;
        } else {
            return Err(match nearest_task_name(task_name, &self.known_task_names()) {
                Some(suggestion) => eyre!("Task '{task_name}' not found; did you mean '{suggestion}'?"),
                None => eyre!("Task '{task_name}' not found"),
            });
        }
        Ok(())
    }

    /// Every task name this ottofile defines, built-ins included. The
    /// suggestion set for an unknown name typed at the top level.
    fn known_task_names(&self) -> Vec<String> {
        self.config_spec.tasks.keys().cloned().collect()
    }

    /// Print all foreach subtasks and their parent tasks.
    ///
    /// `--list-subtasks` is an enumeration surface: giving the real list is its
    /// job, so it DOES resolve command sources, and it propagates a resolution
    /// failure as its own loud error instead of printing a partial list.
    fn print_subtasks(&self) -> Result<()> {
        let mut has_foreach = false;

        // Sort tasks by name for consistent output
        let mut task_names: Vec<_> = self.config_spec.tasks.keys().collect();
        task_names.sort();

        for task_name in task_names {
            let task_spec = &self.config_spec.tasks[task_name];
            if let Some(ref foreach) = task_spec.foreach {
                has_foreach = true;

                // Get the items for this foreach (resolve relative to ottofile dir)
                let items = match self.resolve_foreach(task_name, foreach) {
                    Ok(items) => items,
                    Err(e) => {
                        if foreach.is_command_source() {
                            return Err(e);
                        }
                        eprintln!("{task_name}: Error resolving items: {e}");
                        continue;
                    }
                };

                println!("{task_name} ({} items):", items.len());
                for item in &items {
                    println!("  {}", crate::naming::subtask_name(task_name, &item.identifier));
                }
                println!();
            }
        }

        if !has_foreach {
            println!("No tasks with foreach directive found.");
        }

        Ok(())
    }

}
