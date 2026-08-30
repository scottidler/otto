impl Parser {
    fn resolve_default_tasks(&self) -> Result<Vec<String>> {
        let default_tasks = &self.config_spec.otto.tasks;

        if default_tasks.is_empty() {
            return Ok(vec![]); // No default tasks defined
        }

        let mut resolved_tasks = Vec::new();

        for task_pattern in default_tasks {
            if task_pattern == "*" {
                // "*" means all tasks
                // Built-ins are injected into `tasks` before this runs, so an
                // unfiltered `*` used to expand to them: a bare `otto` in a
                // project defining only build/test executed the `Clean`
                // builtin and exited 0. The stale lowercase `"graph"` filter
                // this replaces never matched, because BUILTIN_COMMANDS are
                // capitalized.
                resolved_tasks.extend(self.config_spec.tasks.keys().filter(|name| !is_builtin(name)).cloned());
            } else {
                // Specific task name
                if self.config_spec.tasks.contains_key(task_pattern) {
                    resolved_tasks.push(task_pattern.clone());
                } else {
                    eprintln!("Warning: Default task '{task_pattern}' not found");
                }
            }
        }

        resolved_tasks.sort();
        resolved_tasks.dedup();

        Ok(resolved_tasks)
    }

    /// Every name `partitions()` may split the arg list on: the ottofile's
    /// tasks, the built-ins, and each foreach task's subtask ids.
    ///
    /// `requested` is the raw arg list. A command-sourced foreach resolves here
    /// only if those args mention it (by parent name or by a subtask-shaped
    /// token); otherwise its subtask ids are not needed to partition this
    /// invocation and the command must not run. When it does resolve, a failure
    /// is returned, not swallowed - the silent `let Ok(items)` that predates
    /// this phase stays only for static sources, whose failures are reported at
    /// the expansion site instead.
    fn get_task_names(&self, requested: &[String]) -> Result<Vec<String>> {
        let mut task_names: Vec<String> = self.config_spec.tasks.keys().cloned().collect();
        // Built-ins are already in `tasks` (injected before this runs). The
        // lowercase `"graph"` that used to be pushed here was dead: the builtin
        // is `Graph`, so `otto graph` partitioned on a name no task ever had and
        // died later with "Task 'graph' not found".
        task_names.push("help".to_string()); // Always include help as a special command

        // Also include expanded subtask names for foreach tasks
        for (name, spec) in &self.config_spec.tasks {
            let Some(ref foreach) = spec.foreach else {
                continue;
            };
            if foreach.is_command_source() {
                if !Self::args_mention_task(requested, name) {
                    continue;
                }
                for item in self.resolve_foreach(name, foreach)? {
                    task_names.push(format!("{}:{}", name, item.identifier));
                }
            } else if let Ok(items) = self.resolve_foreach(name, foreach) {
                for item in items {
                    task_names.push(format!("{}:{}", name, item.identifier));
                }
            }
        }

        Ok(task_names)
    }

    /// Which option tokens take a value, per task name.
    ///
    /// Built from the declared params (`OPT` only: a flag consumes nothing and
    /// a positional has no token), for every task and, so subtask arguments
    /// partition the same way, every subtask id in `task_names`.
    fn value_taking_options(&self, task_names: &[String]) -> ValueTakingOptions {
        let mut options: ValueTakingOptions = HashMap::new();
        for name in task_names {
            let spec_name = Self::parent_task_name(name);
            let Some(spec) = self.config_spec.tasks.get(spec_name) else {
                continue;
            };
            let mut tokens: HashSet<String> = HashSet::new();
            for param in spec.params.values() {
                if param.param_type != ParamType::OPT {
                    continue;
                }
                if let Some(long) = &param.long {
                    tokens.insert(format!("--{long}"));
                }
                if let Some(short) = param.short {
                    tokens.insert(format!("-{short}"));
                }
            }
            options.insert(name.clone(), tokens);
        }
        options
    }

    /// The set of ottofile task names this run can reach from `roots`.
    ///
    /// Mirrors `collect_transitive_deps` on the *unexpanded* specs, so it is a
    /// superset of what the run set can contain: upstream via `before:`,
    /// downstream via `after:`, and the inverted `after:` relation (X's
    /// `after: [Y]` makes X a dependency of Y). Subtask-shaped roots collapse
    /// to their parent. Used to decide which command-sourced foreach tasks may
    /// stay unresolved.
    fn reachable_task_names(&self, roots: &[String]) -> HashSet<String> {
        // name -> tasks that must run when `name` runs, via the inverted `after:` edge.
        let mut inverted_after: HashMap<&str, Vec<&str>> = HashMap::new();
        for (name, spec) in &self.config_spec.tasks {
            for edge in &spec.after {
                inverted_after
                    .entry(Self::parent_task_name(&edge.task))
                    .or_default()
                    .push(name.as_str());
            }
        }

        let mut reachable: HashSet<String> = HashSet::new();
        let mut queue: Vec<String> = roots
            .iter()
            .map(|root| Self::parent_task_name(root).to_string())
            .collect();

        while let Some(name) = queue.pop() {
            if !reachable.insert(name.clone()) {
                continue;
            }
            let mut push = |target: &str| queue.push(Self::parent_task_name(target).to_string());
            if let Some(spec) = self.config_spec.tasks.get(&name) {
                for edge in &spec.before {
                    push(&edge.task);
                }
                for edge in &spec.after {
                    push(&edge.task);
                }
            }
            if let Some(dependents) = inverted_after.get(name.as_str()) {
                for dependent in dependents {
                    push(dependent);
                }
            }
        }

        reachable
    }

    /// `up:gamma` -> `up`; anything else unchanged.
    fn parent_task_name(name: &str) -> &str {
        name.split_once(':').map_or(name, |(parent, _)| parent)
    }

    fn extract_task_names_from_partitions(&self) -> Vec<String> {
        self.pargs
            .iter()
            .filter_map(|p| if p.is_empty() { None } else { Some(p[0].clone()) })
            .collect()
    }

    fn process_tasks_with_filter(&self, requested_tasks: &[String]) -> Result<Vec<Task>> {
        // Step 0: Evaluate global environment variables once
        // (memoized: a command-sourced foreach may already have forced this
        // evaluation at partition time, and both sites must see one result)
        let global_envs = self.global_envs()?.clone();

        // Step 0.4: Check which requested tasks have --Serial flag
        let serial_tasks: HashSet<String> = self.detect_serial_tasks(requested_tasks);

        // Step 0.5: Expand foreach tasks into subtasks
        let mut deferred_foreach: HashSet<String> = HashSet::new();
        let (mut expanded_tasks, serial_membership) =
            self.expand_foreach_tasks_with_serial(&serial_tasks, requested_tasks, &mut deferred_foreach)?;

        // Step 0.6: Desugar `on-failure:` into synthetic `after:` edges.
        // X with `on-failure: [Y]` becomes `X.after += [{task: Y, when: failure}]`,
        // which compute_task_deps_from_specs inverts into Y.task_deps += [{X, failure}].
        Self::apply_on_failure_sugar(&mut expanded_tasks)?;

        // Step 1: Compute all task dependencies using simple linear algorithm
        let task_deps = Self::compute_task_deps_from_specs(&expanded_tasks)?;

        let mut tasks_needed = HashSet::new();
        for task_name in requested_tasks {
            Self::collect_transitive_deps(task_name, &task_deps, &expanded_tasks, &mut tasks_needed)?;
        }

        // The reachability analysis that deferred a command source must be a
        // superset of the run set. If it ever isn't, we'd silently schedule an
        // empty parent instead of the subtasks - fail loudly instead.
        for name in &tasks_needed {
            if deferred_foreach.contains(Self::parent_task_name(name)) {
                return Err(eyre!(
                    "Internal error: task '{}' is in the run set but its command-sourced foreach was \
                     skipped as unreachable; please report this with the ottofile",
                    name
                ));
            }
        }

        // Param resolution uses a multi-phase approach to support propagation:
        //   Phase 1: Apply CLI-provided values only
        //   Phase 2: Propagate values from dependents to dependencies (by name-matching)
        //   Phase 3: Apply defaults for any still-unset params

        // Collect task data with CLI-provided tracking
        let mut task_entries: Vec<(String, Task)> = Vec::new();
        let mut cli_provided_params: HashMap<String, HashSet<String>> = HashMap::new();

        // Phase 1: Create tasks and apply CLI-provided values
        for task_name in &tasks_needed {
            let task_spec = expanded_tasks
                .get(task_name)
                .ok_or_else(|| eyre!("Task '{}' not found", task_name))?;

            // Virtual parent tasks are kept as real (executable) aggregator tasks.
            // Their action is empty; the scheduler short-circuits execution and aggregates
            // their subtasks' statuses to derive the parent's final status.
            let mut task = Task::from_task_with_cwd_and_global_envs(task_spec, &self.cwd, &global_envs);
            let mut cli_provided = HashSet::new();

            // Find the partition for this task's arguments
            let partition_index = self
                .pargs
                .iter()
                .position(|args| !args.is_empty() && args[0] == *task_name);
            let task_args = partition_index.map(|i| &self.pargs[i]);

            if let Some(args) = task_args
                && args.len() > 1
            {
                // Parse task arguments using clap. Use the original (unexpanded) task spec
                // for clap so foreach-derived flags like `--Serial` are still recognized
                // - the expanded virtual parent has `foreach: None` and would reject the flag.
                let clap_spec = self.config_spec.tasks.get(task_name).unwrap_or(task_spec);
                let task_command = self.task_to_command(clap_spec, BuildMode::Bind)?;
                // `try_`, not `get_matches_from`: the latter prints clap's usage
                // and exits 2 from library code, so `otto build --bogus` could
                // never be handled by a caller.
                let next_task = partition_index
                    .and_then(|i| self.pargs.get(i + 1))
                    .and_then(|next| next.first())
                    .map(String::as_str);
                let matches = task_command
                    .try_get_matches_from(args)
                    .map_err(|e| task_arg_error(e.render().ansi().to_string(), e.kind(), next_task))?;

                for param_spec in task_spec.params.values() {
                    match param_spec.param_type {
                        ParamType::FLG => {
                            // Boolean flag - CLI-provided if user explicitly passed it
                            let flag_value = matches.get_flag(param_spec.name.as_str());
                            if flag_value {
                                cli_provided.insert(param_spec.name.clone());
                                task.values
                                    .insert(param_spec.name.clone(), Value::Item("true".to_string()));
                                let env_name = param_spec.name.replace('-', "_");
                                task.envs.insert(env_name, "true".to_string());
                            }
                            // Don't apply default yet — deferred to Phase 3
                        }
                        ParamType::OPT | ParamType::POS => {
                            // Check if value was provided on CLI vs from clap default
                            if matches.value_source(param_spec.name.as_str())
                                != Some(clap::parser::ValueSource::CommandLine)
                            {
                                // Don't apply default yet — deferred to Phase 3
                                continue;
                            }
                            // `nargs` beyond a single value (`+`, `*`, `?`, a
                            // range) collects every space-separated value the
                            // user gave in one occurrence, not just the
                            // first - `get_one` silently dropped the rest.
                            // `ignore_case(true)` returns the spelling the
                            // user typed; the task sees the declared one.
                            let choices = self.param_choices(task_name, param_spec, BuildMode::Bind)?;
                            let canonicalize = |value: &str| -> String {
                                canonical_choice(value, &choices).unwrap_or(value).to_string()
                            };
                            if param_spec.nargs == Nargs::One {
                                if let Some(value) = matches.get_one::<String>(param_spec.name.as_str()) {
                                    let value = canonicalize(value);
                                    cli_provided.insert(param_spec.name.clone());
                                    task.values.insert(param_spec.name.clone(), Value::Item(value.clone()));
                                    let env_name = param_spec.name.replace('-', "_");
                                    task.envs.insert(env_name, value);
                                }
                            } else if let Some(values) = matches.get_many::<String>(param_spec.name.as_str()) {
                                let values: Vec<String> = values.map(|v| canonicalize(v)).collect();
                                cli_provided.insert(param_spec.name.clone());
                                let env_name = param_spec.name.replace('-', "_");
                                task.envs.insert(env_name, values.join(" "));
                                task.values.insert(param_spec.name.clone(), Value::List(values));
                            }
                        }
                    }
                }
            }
            // No args: no CLI-provided values, defaults applied in Phase 3

            // Override task_deps with computed dependencies
            task.task_deps = task_deps.get(task_name).map(|deps| deps.to_vec()).unwrap_or_default();

            // Carry serial-group membership through to the scheduler's ready loop.
            if let Some((group, index)) = serial_membership.get(task_name) {
                task.serial_group = Some(group.clone());
                task.serial_index = *index;
            }

            cli_provided_params.insert(task_name.clone(), cli_provided);
            task_entries.push((task_name.clone(), task));
        }

        // Phase 2: Propagate values from dependents to dependencies
        // (see param-propagation-design.md for full algorithm)
        self.propagate_params(&expanded_tasks, &task_deps, &mut task_entries, &cli_provided_params)?;

        // Phase 3: Apply defaults for any still-unset params
        for (task_name, task) in &mut task_entries {
            let task_spec = match expanded_tasks.get(task_name) {
                Some(spec) => spec,
                None => continue,
            };
            for param_spec in task_spec.params.values() {
                if task.values.contains_key(&param_spec.name) {
                    continue; // Already set from CLI or propagation
                }
                match param_spec.param_type {
                    ParamType::FLG => {
                        let default_value = param_spec.default.as_deref().unwrap_or("false");
                        task.values
                            .insert(param_spec.name.clone(), Value::Item(default_value.to_string()));
                        let env_name = param_spec.name.replace('-', "_");
                        task.envs.insert(env_name, default_value.to_string());
                    }
                    ParamType::OPT | ParamType::POS => {
                        if let Some(ref default) = param_spec.default {
                            task.values
                                .insert(param_spec.name.clone(), Value::Item(default.clone()));
                            let env_name = param_spec.name.replace('-', "_");
                            task.envs.insert(env_name, default.clone());
                        }
                    }
                }
            }
        }

        let tasks = task_entries.into_iter().map(|(_, task)| task).collect();
        Ok(tasks)
    }

}
