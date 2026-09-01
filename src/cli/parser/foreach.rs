impl Parser {
    /// Detect which requested tasks have --Serial flag in their arguments
    fn detect_serial_tasks(&self, requested_tasks: &[String]) -> HashSet<String> {
        let mut serial_tasks = HashSet::new();
        let value_options = self.value_taking_options(requested_tasks);

        for task_name in requested_tasks {
            // Find partition for this task
            if let Some(args) = self.pargs.iter().find(|args| !args.is_empty() && args[0] == *task_name) {
                // Check if --Serial is present as a flag, not as a value
                if contains_flag(args, "--Serial", value_options.get(task_name)) {
                    serial_tasks.insert(task_name.clone());
                }
            }
        }

        serial_tasks
    }

    /// Expand all foreach tasks into their subtasks.
    ///
    /// Returns a new task specs map with:
    /// - Non-foreach tasks unchanged
    /// - Foreach tasks replaced by: virtual parent + N subtasks
    ///
    /// When a task is in serial_tasks, its subtasks join a serial group: the returned
    /// membership map records `subtask name -> (group name, order index)`. Serial
    /// ordering is NOT expressed as `before:` edges - "runs after" and "requires" are
    /// different things, and edges are the latter (see the Phase 4 design bullet in
    /// docs/design/2026-08-28-boundary-fixes-and-dynamic-foreach.md).
    ///
    /// A third map, `DisplayOrderMap`, records `parent task name -> subtask names`
    /// in declared foreach item order, for every foreach expansion (parallel and
    /// serial alike, mirroring `OTTO_FOREACH_INDEX`). It is additive and carries no
    /// scheduling meaning of its own; the buffered-foreach replay cursor (design
    /// doc `2026-08-31-buffered-foreach-computed-envs-required-params.md`,
    /// Phase 4) is its only reader.
    ///
    /// `requested_tasks` gates command-sourced foreach: a task the run can't
    /// reach is left unexpanded so its command never runs (`otto build` must
    /// not execute an unrelated `up` task's command source). `deferred`
    /// collects the names left unexpanded, so the caller can prove none of them
    /// ended up in the run set.
    fn expand_foreach_tasks_with_serial(
        &self,
        serial_tasks: &HashSet<String>,
        requested_tasks: &[String],
        deferred: &mut HashSet<String>,
    ) -> Result<ForeachExpansion> {
        let mut expanded: TaskSpecs = TaskSpecs::new();
        let mut membership: SerialMembership = HashMap::new();
        let mut display_order: DisplayOrderMap = HashMap::new();
        let mut buffered: HashSet<String> = HashSet::new();
        let reachable = self.reachable_task_names(requested_tasks);

        for (name, spec) in &self.config_spec.tasks {
            if spec.has_foreach() {
                let foreach = spec.foreach.as_ref().expect("has_foreach() implies Some");

                // Lazy trigger: a command source resolves only for a task this
                // run can reach. Everything else keeps its virtual parent as a
                // placeholder and never executes anything.
                if foreach.is_command_source() && !reachable.contains(name) {
                    deferred.insert(name.clone());
                    let mut parent = spec.as_virtual_parent();
                    parent.before = Vec::new();
                    expanded.insert(name.clone(), parent);
                    continue;
                }

                // Expand foreach task into subtasks (resolve relative to ottofile dir)
                let items = self.resolve_foreach(name, foreach)?;
                let subtasks = spec.expand_foreach_with_items(&items)?;

                if subtasks.is_empty() {
                    // Zero matches - just keep the virtual parent for dependency tracking
                    log::warn!("foreach task '{}' expanded to 0 subtasks", name);
                }

                // Record display order before `subtasks` is consumed below.
                // Declared item order here is the same order `expand_foreach_with_items`
                // assigned each subtask's `OTTO_FOREACH_INDEX` in, so the two never drift.
                display_order.insert(name.clone(), subtasks.iter().map(|st| st.name.clone()).collect());

                // `buffer: true` does not survive the expansion on its own: the
                // subtasks are clones with `foreach = None` and the virtual parent
                // is built by `as_virtual_parent()`, which drops `foreach` too. The
                // parent name is recorded here, where the spec is still in hand, and
                // the caller stamps `Task::buffered` onto the parent and every
                // subtask (design doc Phase 4).
                if foreach.buffer {
                    buffered.insert(name.clone());
                }

                // Check if this task should run serially (CLI --Serial flag OR config parallel: false)
                let run_serial = serial_tasks.contains(name) || spec.foreach.as_ref().is_some_and(|f| !f.parallel);

                // Build virtual parent. Its `before:` is replaced with When::Always edges
                // to every subtask, so the scheduler queues the parent only after all
                // subtasks reach a terminal state (Completed | Failed | Skipped).
                let mut parent = spec.as_virtual_parent();
                parent.before = subtasks
                    .iter()
                    .map(|st| crate::cfg::edge::EdgeSpec {
                        task: st.name.clone(),
                        when: crate::cfg::edge::When::Always,
                        from_sugar: false,
                        is_injected_sugar: false,
                    })
                    .collect();

                // Add all subtasks. Each subtask inherits the *original* parent's
                // before dependencies (its prerequisites), not the rewritten one.
                // Subtasks do NOT inherit the parent's `after:` edges - only the
                // parent triggers downstreams; otherwise every subtask would
                // double-fire the downstream task.
                for (index, mut subtask) in subtasks.into_iter().enumerate() {
                    // Subtask inherits parent's original before dependencies
                    subtask.before = spec.before.clone();
                    // Subtasks do not inherit the parent's downstream edges. `on-failure:`
                    // is sugar for an `after:` edge and desugars *after* this expansion,
                    // so it has to be cleared here too - otherwise every subtask grows its
                    // own `when: failure` edge to the fixer, and the first subtask that
                    // succeeds makes the fixer Unreachable. Only the parent, which
                    // aggregates the group's outcome, triggers downstreams.
                    subtask.after = Vec::new();
                    subtask.on_failure = Vec::new();

                    // Serial ordering is recorded as group membership, not as an edge.
                    if run_serial {
                        membership.insert(subtask.name.clone(), (name.clone(), index));
                    }

                    expanded.insert(subtask.name.clone(), subtask);
                }

                expanded.insert(name.clone(), parent);
            } else {
                // Non-foreach task - keep as-is
                expanded.insert(name.clone(), spec.clone());
            }
        }

        Ok(ForeachExpansion {
            specs: expanded,
            membership,
            display_order,
            buffered,
        })
    }

    /// Collect all tasks needed to run a given task, including:
    /// - Transitive dependencies (before/upstream tasks)
    /// - After tasks (downstream tasks that should auto-run)
    /// - Subtasks (for foreach parent tasks)
    fn collect_transitive_deps(
        task_name: &str,
        task_deps: &HashMap<String, Vec<TaskEdge>>,
        task_specs: &TaskSpecs,
        collected: &mut HashSet<String>,
    ) -> Result<()> {
        if collected.contains(task_name) {
            return Ok(());
        }

        collected.insert(task_name.to_string());

        // Collect upstream dependencies (before)
        if let Some(deps) = task_deps.get(task_name) {
            for dep in deps {
                Self::collect_transitive_deps(&dep.task, task_deps, task_specs, collected)?;
            }
        }

        // Collect downstream tasks (after) - these auto-run when this task is requested
        if let Some(spec) = task_specs.get(task_name) {
            for after_edge in &spec.after {
                Self::collect_transitive_deps(&after_edge.task, task_deps, task_specs, collected)?;
            }
        }

        // Collect subtasks for foreach parent tasks
        // Only expand subtasks if this is a parent task (no colon in name)
        // If user requests "install:td", don't also collect install:ts, install:cs
        if !crate::naming::is_subtask(task_name) {
            for subtask_name in task_specs.keys() {
                if crate::naming::is_subtask_of(subtask_name, task_name) {
                    Self::collect_transitive_deps(subtask_name, task_deps, task_specs, collected)?;
                }
            }
        }

        Ok(())
    }
}
