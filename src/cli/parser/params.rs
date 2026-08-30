impl Parser {
    /// Propagate param values from dependents (parents) to their dependencies.
    ///
    /// When a parent task has a resolved param and its dependency declares a param
    /// with the same name but wasn't given an explicit CLI value, the dependency
    /// inherits the parent's value. Propagation is transitive through chains
    /// (deploy -> middle -> build) as long as each intermediate task declares the
    /// param. Diamond conflicts (two parents propagating different values) are
    /// rejected with a clear error.
    fn propagate_params(
        &self,
        expanded_tasks: &TaskSpecs,
        task_deps: &HashMap<String, Vec<TaskEdge>>,
        task_entries: &mut [(String, Task)],
        cli_provided_params: &HashMap<String, HashSet<String>>,
    ) -> Result<()> {
        let entry_names: HashSet<String> = task_entries.iter().map(|(n, _)| n.clone()).collect();

        // Build dependency graph filtered to only entry tasks, resolving virtual parents
        let filtered_deps = Self::build_filtered_deps(task_deps, &entry_names, expanded_tasks);

        // Build reverse index: dep -> list of dependents (value sources for propagation)
        let dependents_of = Self::build_reverse_index(&filtered_deps);

        // Topological sort in propagation order (dependents before deps)
        let ordered = Self::topo_sort_propagation_order(&filtered_deps, &entry_names);

        // Build name-to-index map for task_entries (owned keys to avoid borrow conflict)
        let name_to_idx: HashMap<String, usize> = task_entries
            .iter()
            .enumerate()
            .map(|(i, (name, _))| (name.clone(), i))
            .collect();

        // Pre-populate resolved values from CLI-provided params
        let mut resolved_values: HashMap<String, HashMap<String, String>> = HashMap::new();
        for (task_name, task) in task_entries.iter() {
            let mut values = HashMap::new();
            for (param_name, value) in &task.values {
                if let Value::Item(v) = value {
                    values.insert(param_name.clone(), v.clone());
                }
            }
            resolved_values.insert(task_name.clone(), values);
        }

        // Process tasks in propagation order
        let empty_set = HashSet::new();
        for task_name in &ordered {
            let Some(task_spec) = expanded_tasks.get(task_name) else {
                continue;
            };
            let cli_provided = cli_provided_params.get(task_name).unwrap_or(&empty_set);

            for param_spec in task_spec.params.values() {
                if cli_provided.contains(&param_spec.name) {
                    continue; // CLI value takes precedence
                }

                // Collect values from all dependents (parents)
                let mut inherited: Vec<(&str, &str)> = Vec::new();
                if let Some(parents) = dependents_of.get(task_name.as_str()) {
                    for parent_name in parents {
                        if let Some(parent_resolved) = resolved_values.get(*parent_name)
                            && let Some(value) = parent_resolved.get(&param_spec.name)
                        {
                            inherited.push((parent_name, value));
                        }
                    }
                }

                if inherited.is_empty() {
                    continue;
                }

                // Check all parents agree on the value
                let first_value = inherited[0].1;
                if !inherited.iter().all(|(_, v)| *v == first_value) {
                    let details: Vec<String> = inherited
                        .iter()
                        .map(|(parent, value)| format!("  {} provides '{}'", parent, value))
                        .collect();
                    return Err(eyre!(
                        "Conflicting param propagation for '{}' on task '{}':\n{}\n\
                         Hint: Use explicit CLI values to resolve the conflict, \
                         e.g., otto {} --{} <value>",
                        param_spec.name,
                        task_name,
                        details.join("\n"),
                        task_name,
                        param_spec.name,
                    ));
                }

                // Validate choices constraint on propagated value. This is the
                // second bind trigger for a dynamic set: a value the user never
                // typed at this task still has to be valid here, so invoking A
                // can legitimately run dependency B's choices-command. The
                // per-invocation cache is what keeps that at one execution.
                let choices = self.param_choices(task_name, param_spec, BuildMode::Bind)?;
                if !choices.is_empty() && !choices.contains(&first_value.to_string()) {
                    let source_task = inherited[0].0;
                    return Err(eyre!(
                        "Propagated value '{}' for param '{}' on task '{}' (from task '{}') \
                         is not in allowed choices: [{}]",
                        first_value,
                        param_spec.name,
                        task_name,
                        source_task,
                        choices.join(", "),
                    ));
                }

                // Inherit the value
                let value = first_value.to_string();
                resolved_values
                    .entry(task_name.clone())
                    .or_default()
                    .insert(param_spec.name.clone(), value.clone());

                if let Some(&idx) = name_to_idx.get(task_name.as_str()) {
                    let (_, task) = &mut task_entries[idx];
                    task.values.insert(param_spec.name.clone(), Value::Item(value.clone()));
                    let env_name = param_spec.name.replace('-', "_");
                    task.envs.insert(env_name, value);
                }
            }
        }

        Ok(())
    }

    /// Build dependency graph filtered to only include tasks in entry_names,
    /// resolving virtual parent references to their subtasks.
    fn build_filtered_deps(
        task_deps: &HashMap<String, Vec<TaskEdge>>,
        entry_names: &HashSet<String>,
        expanded_tasks: &TaskSpecs,
    ) -> HashMap<String, Vec<String>> {
        let mut filtered: HashMap<String, Vec<String>> = HashMap::new();
        for name in entry_names {
            let mut deps = Vec::new();
            if let Some(raw_deps) = task_deps.get(name) {
                for dep in raw_deps {
                    if expanded_tasks.get(&dep.task).is_some_and(|s| s.virtual_parent) {
                        // Virtual parent -> redirect to subtasks
                        let prefix = format!("{}:", dep.task);
                        for subtask in entry_names {
                            if subtask.starts_with(&prefix) {
                                deps.push(subtask.clone());
                            }
                        }
                    } else if entry_names.contains(&dep.task) {
                        deps.push(dep.task.clone());
                    }
                }
            }
            filtered.insert(name.clone(), deps);
        }
        filtered
    }

    /// Build reverse index: maps each task to the list of tasks that depend on it
    /// (i.e., its dependents/parents in propagation terminology).
    fn build_reverse_index(filtered_deps: &HashMap<String, Vec<String>>) -> HashMap<&str, Vec<&str>> {
        let mut reverse: HashMap<&str, Vec<&str>> = HashMap::new();
        for (task_name, deps) in filtered_deps {
            for dep in deps {
                reverse.entry(dep.as_str()).or_default().push(task_name.as_str());
            }
        }
        reverse
    }

    /// Topological sort in propagation order: dependents before their deps.
    ///
    /// In the depends-on graph (A -> B means A depends on B), nodes with no tasks
    /// depending on them (in-degree 0) are processed first. These are the leaf
    /// dependents that propagate values downward.
    fn topo_sort_propagation_order(
        filtered_deps: &HashMap<String, Vec<String>>,
        entry_names: &HashSet<String>,
    ) -> Vec<String> {
        use std::collections::VecDeque;

        // Compute in-degree: how many tasks list each task as a dependency
        let mut in_degree: HashMap<&str, usize> = entry_names.iter().map(|n| (n.as_str(), 0)).collect();
        for deps in filtered_deps.values() {
            for dep in deps {
                *in_degree.entry(dep.as_str()).or_default() += 1;
            }
        }

        // Start with nodes that have in-degree 0 (no task depends on them)
        let mut queue: VecDeque<String> = in_degree
            .iter()
            .filter(|(_, count)| **count == 0)
            .map(|(&name, _)| name.to_string())
            .collect();
        // Sort for deterministic ordering
        let mut sorted_queue: Vec<String> = queue.drain(..).collect();
        sorted_queue.sort();
        queue.extend(sorted_queue);

        let mut result = Vec::new();
        while let Some(node) = queue.pop_front() {
            result.push(node.clone());
            if let Some(deps) = filtered_deps.get(&node) {
                // Collect and sort for deterministic ordering
                let mut next: Vec<&str> = Vec::new();
                for dep in deps {
                    // Every dep got an in_degree entry in the counting loop above.
                    let count = in_degree
                        .get_mut(dep.as_str())
                        .expect("in_degree has an entry for every dependency it counted");
                    *count -= 1;
                    if *count == 0 {
                        next.push(dep);
                    }
                }
                next.sort();
                for dep in next {
                    queue.push_back(dep.to_string());
                }
            }
        }

        result
    }

    /// Desugar each host's `on_failure:` field into synthetic `after:` edges.
    ///
    /// For each host X with `on_failure: [Y]`:
    /// - Push `EdgeSpec { task: Y, when: Failure, is_injected_sugar: true }` onto X's
    ///   `after:` list. compute_task_deps_from_specs then inverts this to
    ///   `Y.task_deps += [{X, failure}]`, so Y depends on X with `when: failure`.
    /// - The host's `on_failure` field is preserved verbatim so the serializer
    ///   can re-emit it; `is_injected_sugar` lets the serializer filter the
    ///   synthetic `after:` entry so it doesn't show up as a duplicate.
    fn apply_on_failure_sugar(specs: &mut TaskSpecs) -> Result<()> {
        // Collect (host, target) pairs first, then mutate. We can't hold a &mut to
        // `specs` while iterating it. Sort for determinism: HashMap iteration order
        // is non-deterministic, so without sorting the synthetic-edge order would
        // sporadically reorder between runs and break byte-equivalent round-trip.
        let mut pairs: Vec<(String, String)> = specs
            .iter()
            .flat_map(|(host, spec)| spec.on_failure.iter().map(move |target| (host.clone(), target.clone())))
            .collect();
        pairs.sort();

        for (host, target) in pairs {
            if host == target {
                return Err(eyre!(
                    "on-failure on task '{}' references itself; a task cannot depend on its own failure",
                    host
                ));
            }
            if !specs.contains_key(&target) {
                return Err(eyre!(
                    "on-failure on task '{}' references unknown task '{}'",
                    host,
                    target
                ));
            }
            let host_spec = specs
                .get_mut(&host)
                .expect("host was iterated from specs and the map has not shrunk");
            host_spec.after.push(crate::cfg::edge::EdgeSpec {
                task: target.clone(),
                when: crate::cfg::edge::When::Failure,
                from_sugar: false,
                is_injected_sugar: true,
            });
        }
        Ok(())
    }

    /// Compute task dependencies from a given task specs map.
    /// Validates that all referenced dependencies exist in the task specs.
    fn compute_task_deps_from_specs(task_specs: &TaskSpecs) -> Result<HashMap<String, Vec<TaskEdge>>> {
        let mut task_deps: HashMap<String, Vec<TaskEdge>> = HashMap::new();

        // Initialize with direct dependencies from 'before' field
        for (task_name, task_spec) in task_specs {
            let edges: Vec<TaskEdge> = task_spec
                .before
                .iter()
                .map(|e| TaskEdge::new(e.task.clone(), e.when))
                .collect();
            task_deps.insert(task_name.clone(), edges);
        }

        // `after` on task X means: every task in X's `after` list depends on X.
        // The condition lives on the inverted edge: X.after = [{task: Y, when: W}]
        // means Y now has a dependency {task: X, when: W} added to Y's task_deps.
        //
        // Dedup is by (task, when) tuple - identical (task, when) pairs collapse to one edge.
        for (task_name, task_spec) in task_specs {
            for after_edge in &task_spec.after {
                let deps = task_deps.entry(after_edge.task.clone()).or_default();
                let new_edge = TaskEdge::new(task_name.clone(), after_edge.when);
                if !deps.iter().any(|d| d.task == new_edge.task && d.when == new_edge.when) {
                    deps.push(new_edge);
                }
            }
        }

        // Validate all dependencies exist
        // This catches typos like "install:tx" when only "install:td" exists
        for (task_name, deps) in &task_deps {
            for dep in deps {
                if !task_specs.contains_key(&dep.task) {
                    return Err(eyre!(
                        "Task '{}' has unknown dependency '{}'\n\
                         Hint: Check for typos in the task name.",
                        task_name,
                        dep.task
                    ));
                }
            }
        }

        // Reject a task that holds both `when: success` and `when: failure` on the
        // same source. One of the two can never be satisfied, whichever way the
        // source ends, so the dependent is permanently skipped - and it used to be
        // skipped silently, at exit 0. `when: always` is the way to express "either
        // outcome". Names are sorted so the message is stable across HashMap order.
        let mut conflicts: Vec<(String, String)> = Vec::new();
        for (task_name, deps) in &task_deps {
            for dep in deps {
                if dep.when == When::Success
                    && deps
                        .iter()
                        .any(|other| other.task == dep.task && other.when == When::Failure)
                {
                    conflicts.push((task_name.clone(), dep.task.clone()));
                }
            }
        }
        if !conflicts.is_empty() {
            conflicts.sort();
            let (task_name, dep_name) = &conflicts[0];
            return Err(eyre!(
                "Task '{}' depends on '{}' with both 'when: success' and 'when: failure'\n\
                 Hint: those cannot both be satisfied, so '{}' could never run. Use 'when: always' \
                 if it should run either way.",
                task_name,
                dep_name,
                task_name
            ));
        }

        Ok(task_deps)
    }

}
