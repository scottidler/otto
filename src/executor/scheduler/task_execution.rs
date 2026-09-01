impl<F: FileSystem + 'static> TaskScheduler<F> {
    /// Spawn `task`'s body into `active`, reporting exactly once when it ends.
    ///
    /// The body is one fallible expression: every early exit inside it - the
    /// semaphore, the dependency double-check, `create_dir_all`, the symlinks,
    /// the action processor - lands in the same place, and that place is the
    /// only sender. Before this, a `?` on any of those abandoned the task with
    /// nothing sent, and the scheduler waited on it forever.
    async fn execute_task(&self, task: Task, tx: mpsc::Sender<TaskReport>, active: &mut ActiveTasks) -> Result<()> {
        debug!("execute_task: task={} tty={}", task.name, task.tty);
        let semaphore = self.semaphore.clone();

        let task_name = task.name.clone();
        let task_dir = self.workspace.task(&task_name);
        let task_statuses = self.task_statuses.clone();
        let task_deps = task.task_deps.clone();
        let workspace = self.workspace.clone();
        let envs = task.envs.clone();
        let tasks_dir = self.workspace.run().join("tasks");
        let execution_context = self.execution_context.clone();
        // A buffered foreach subtask's bytes reach only its two logs; ordered
        // replay is the sole path to the terminal for them (design doc Phase 4).
        let suppress_terminal = self.tui_mode || task.buffered;
        let no_prefix = self.no_prefix;
        let task_streams = self.task_streams.clone();
        let is_virtual_parent = task.is_virtual_parent;
        let action_is_empty = task.action.is_empty();
        let tty = task.tty;
        let permits = permits_for(tty, self.max_parallel)?;

        let spawn_name = task_name.clone();
        active.spawn(spawn_name, async move {
            // Set by the run block when a process actually exited, so the
            // database records the real code instead of re-parsing it back out
            // of the error message and defaulting to 1.
            let mut exit_code: Option<i32> = None;
            // Assigned once the task has a database row; the completion write
            // happens after the body, on the single report path.
            let mut db_task_id: Option<i64> = None;
            // A virtual parent has no script, no output files and no database
            // row: it only aggregates its subtasks' statuses.
            let mut aggregator_only = false;
            // Streams whose drain did not finish. Terminal state is not the
            // same as output complete: the six arms below only `error!`-log and
            // fall through to the exit status, so a task can report success over
            // a short log. Buffered replay reads this to end the block with a
            // marker naming the condition instead of a silent truncation.
            let mut drain_issues: Vec<DrainIssue> = Vec::new();

            let outcome: std::result::Result<(), TaskFailure> = async {
                // Acquire semaphore permit. A tty task takes every permit, so nothing
                // else runs while it owns the terminal; tokio's semaphore is FIFO, so
                // this wait cannot be starved by later single-permit acquires.
                let _permit = semaphore.acquire_many(permits).await?;

                // Empty-action fast path (used by virtual parent tasks). The parent
                // task has no script to run - it exists only to aggregate subtask
                // statuses. We mark it Running briefly so the scheduler's success
                // arm picks it up and applies the aggregation override.
                if is_virtual_parent && action_is_empty {
                    {
                        let mut statuses = task_statuses.lock().await;
                        statuses.insert(task_name.clone(), TaskStatus::Running);
                    }
                    aggregator_only = true;
                    return Ok(());
                }

                {
                    let statuses = task_statuses.lock().await;
                    for dep in &task_deps {
                        let status = statuses.get(&dep.task);
                        // Second gate on the same edge, and it must answer exactly what
                        // `classify_edge` answered: same nine cells, asserted together by
                        // `classify_edge_skip_provenance_matrix`. A disagreement here
                        // aborts at spawn time a task the scheduler just admitted, so the
                        // table lives in one function that both this gate and that test
                        // call rather than being transcribed into either.
                        let satisfied = edge_satisfied_by_status(dep.when, status);
                        if !satisfied {
                            return Err(eyre!(
                                "Dependency {} not satisfied (when: {:?}, status: {:?}) for task {}",
                                dep.task,
                                dep.when,
                                status,
                                task_name
                            )
                            .into());
                        }
                    }
                }

                // Update task status to Running ONLY after dependency check
                {
                    let mut statuses = task_statuses.lock().await;
                    statuses.insert(task_name.clone(), TaskStatus::Running);
                }

                info!("Starting task {task_name}");

                tokio::fs::create_dir_all(&task_dir).await?;

                // Setup dependency input files (symlink outputs from dependencies)
                for dep_edge in &task_deps {
                    let dep_name = &dep_edge.task;
                    let dep_output_file = workspace.task_output_file(dep_name);
                    let current_input_file = workspace.task_input_file(&task_name, dep_name);
                    let current_input_env_file = workspace.task_input_env_file(&task_name, dep_name);

                    // Only create symlink if dependency output exists
                    if dep_output_file.exists() {
                        if current_input_file.exists() {
                            tokio::fs::remove_file(&current_input_file).await.ok();
                        }

                        // Create symlink from dependency output to current task input
                        #[cfg(unix)]
                        {
                            use std::os::unix::fs;
                            // Use relative path for portability
                            let relative_dep_path = workspace.relative_task_dependency_path(dep_name);
                            fs::symlink(&relative_dep_path, &current_input_file)?;
                        }
                        #[cfg(not(unix))]
                        {
                            // Fallback: copy file on non-Unix systems
                            tokio::fs::copy(&dep_output_file, &current_input_file).await?;
                        }

                        // Generate .env file from JSON for jq-free bash deserialization
                        // This allows bash to source the .env file instead of parsing JSON with jq
                        if let Ok(json_content) = tokio::fs::read_to_string(&dep_output_file).await
                            && let Ok(json_data) = serde_json::from_str::<serde_json::Value>(&json_content)
                        {
                            let env_content = json_to_env(&json_data, dep_name);
                            tokio::fs::write(&current_input_env_file, env_content).await.ok();
                        }
                    }
                }

                // Process the user's action script with Otto enhancements
                let action_processor = ActionProcessor::new(workspace.clone(), &task_name)?;
                let processed_action = action_processor.process(&task.action, &task)?;

                // Extract script path and determine interpreter
                let (script_path, interpreter) = match processed_action {
                    ProcessedAction::Bash { path, .. } => (path, "bash"),
                    ProcessedAction::Python3 { path, .. } => (path, "python3"),
                };

                // Record task start in database with paths (graceful degradation)
                db_task_id = if let (Some(run_id), Some(store)) = (workspace.db_run_id(), workspace.state_store()) {
                    let stdout_path = tasks_dir.join(&task_name).join("stdout.log");
                    let stderr_path = tasks_dir.join(&task_name).join("stderr.log");
                    let name = task_name.clone();
                    let script = script_path.clone();

                    // rusqlite is synchronous and holds a mutex across the
                    // write, so it runs on the blocking pool rather than
                    // parking a tokio worker mid-task.
                    let recorded = crate::ports::record_blocking(store, move |store| {
                        store.record_task_start(
                            run_id,
                            &name,
                            None, // TODO: Compute script hash in future phase
                            Some(&stdout_path),
                            Some(&stderr_path),
                            Some(&script),
                        )
                    })
                    .await;

                    match recorded {
                        Ok(task_id) => Some(task_id),
                        Err(e) => {
                            log::warn!("Failed to record task start in database: {}", e);
                            None
                        }
                    }
                } else {
                    None
                };

                // Setup command environment
                let mut cmd = Command::new(interpreter);
                cmd.arg(&script_path)
                    .current_dir(workspace.root())
                    // Inherit current environment by default (no env_clear())
                    .envs(&envs) // Override with user-specified env vars
                    .env("OTTO_TASK", &task_name)
                    .env("OTTO_TASK_DIR", task_dir.to_string_lossy().to_string())
                    .env("OTTO_WORKSPACE", workspace.root().to_string_lossy().to_string())
                    .env("OTTO_TASKS_DIR", tasks_dir.to_string_lossy().to_string())
                    .env("OTTO_USER", &execution_context.user)
                    // The child dies with its task body. Without this, cancelling or
                    // dropping the scheduler left orphaned processes holding the
                    // workspace open.
                    .kill_on_drop(true);

                // Its own process group, so a signal aimed at otto does not race
                // ahead of otto's own teardown and so the child's own children are
                // reachable as a group. NOT for a `tty: true` task: that task owns
                // the terminal, and a background process group reading from it gets
                // SIGTTIN and stops.
                #[cfg(unix)]
                if !tty {
                    cmd.process_group(0);
                }

                // Execute without timeout - runs until completion or failure
                let result = async {
                    if tty {
                        // The task owns the terminal: inherit stdout/stderr (stdin is
                        // already inherited - otto never redirects it) and skip
                        // TaskStreams entirely, so there is no capture and no [task]
                        // prefix. The logs still exist, carrying the marker line.
                        write_tty_log_markers(&tasks_dir, &task_name).await?;
                        let mut child = cmd
                            .stdout(std::process::Stdio::inherit())
                            .stderr(std::process::Stdio::inherit())
                            .spawn()
                            .map_err(|e| eyre!("Task {task_name} could not start {interpreter}: {e}"))?;
                        let status = child.wait().await?;
                        exit_code = status.code();
                        if status.success() {
                            return Ok(());
                        }
                        let stdout_log = tasks_dir.join(&task_name).join("stdout.log");
                        let stderr_log = tasks_dir.join(&task_name).join("stderr.log");
                        // No stderr preview: nothing was captured, and echoing the
                        // marker line back as "error output" would be a lie.
                        return Err(eyre!(
                            "Task {} failed with exit code {:?}\n\nLogs:\n  stdout: {}\n  stderr: {}",
                            task_name,
                            status.code(),
                            stdout_log.display(),
                            stderr_log.display()
                        ));
                    }

                    let mut child = cmd
                        .stdout(std::process::Stdio::piped())
                        .stderr(std::process::Stdio::piped())
                        .spawn()
                        .map_err(|e| eyre!("Task {task_name} could not start {interpreter}: {e}"))?;

                    // Setup output streams
                    let stdout = child.stdout.take().ok_or_else(|| eyre!("Failed to capture stdout"))?;
                    let stderr = child.stderr.take().ok_or_else(|| eyre!("Failed to capture stderr"))?;

                    let streams = if let Some(streams_map) = &task_streams {
                        streams_map
                            .get(&task_name)
                            .ok_or_else(|| eyre!("TaskStreams not found for task {}", task_name))?
                            .clone()
                    } else {
                        TaskStreams::new(&task_name, &tasks_dir).await?
                    };

                    // Start output handling
                    let stdout_handle = {
                        let streams = streams.clone();
                        let task_name = task_name.clone();
                        tokio::spawn(async move {
                            let reader = BufReader::new(stdout);
                            streams
                                .process_output(task_name, OutputType::Stdout, reader, suppress_terminal, no_prefix)
                                .await
                        })
                    };

                    let stderr_handle = {
                        let streams = streams.clone();
                        let task_name = task_name.clone();
                        tokio::spawn(async move {
                            let reader = BufReader::new(stderr);
                            streams
                                .process_output(task_name, OutputType::Stderr, reader, suppress_terminal, no_prefix)
                                .await
                        })
                    };

                    // Wait for process to complete
                    let status = child.wait().await?;
                    exit_code = status.code();

                    // Wait for output handling to complete with timeout (only for output processing)
                    let output_timeout = Duration::from_secs(OUTPUT_PROCESSING_TIMEOUT_SECS);

                    match timeout(output_timeout, stdout_handle).await {
                        Ok(Ok(Ok(()))) => {
                            // Stdout processing completed successfully
                        }
                        Ok(Ok(Err(e))) => {
                            error!("Stdout processing failed for task {task_name}: {e}");
                            drain_issues.push(DrainIssue {
                                stream: OutputType::Stdout,
                                condition: DrainCondition::ProcessingError,
                            });
                        }
                        Ok(Err(e)) => {
                            error!("Stdout processing join failed for task {task_name}: {e}");
                            drain_issues.push(DrainIssue {
                                stream: OutputType::Stdout,
                                condition: DrainCondition::JoinError,
                            });
                        }
                        Err(_) => {
                            error!("Stdout processing timed out for task {task_name}");
                            drain_issues.push(DrainIssue {
                                stream: OutputType::Stdout,
                                condition: DrainCondition::Timeout,
                            });
                        }
                    }

                    match timeout(output_timeout, stderr_handle).await {
                        Ok(Ok(Ok(()))) => {
                            // Stderr processing completed successfully
                        }
                        Ok(Ok(Err(e))) => {
                            error!("Stderr processing failed for task {task_name}: {e}");
                            drain_issues.push(DrainIssue {
                                stream: OutputType::Stderr,
                                condition: DrainCondition::ProcessingError,
                            });
                        }
                        Ok(Err(e)) => {
                            error!("Stderr processing join failed for task {task_name}: {e}");
                            drain_issues.push(DrainIssue {
                                stream: OutputType::Stderr,
                                condition: DrainCondition::JoinError,
                            });
                        }
                        Err(_) => {
                            error!("Stderr processing timed out for task {task_name}");
                            drain_issues.push(DrainIssue {
                                stream: OutputType::Stderr,
                                condition: DrainCondition::Timeout,
                            });
                        }
                    }

                    if status.success() {
                        Ok(())
                    } else {
                        // Get fully qualified log file paths
                        let stdout_log = tasks_dir.join(&task_name).join("stdout.log");
                        let stderr_log = tasks_dir.join(&task_name).join("stderr.log");

                        // Read stderr content to include in error message
                        let stderr_content = tokio::fs::read_to_string(&stderr_log).await.unwrap_or_default();
                        let stderr_preview = if !stderr_content.trim().is_empty() {
                            let lines: Vec<&str> = stderr_content.lines().collect();
                            let preview_lines = if lines.len() > 20 { &lines[lines.len() - 20..] } else { &lines[..] };
                            format!(
                                "\n\nError output (last {} lines):\n{}",
                                preview_lines.len(),
                                preview_lines.join("\n")
                            )
                        } else {
                            String::new()
                        };

                        Err(eyre!(
                            "Task {} failed with exit code {:?}{}\n\nLogs:\n  stdout: {}\n  stderr: {}",
                            task_name,
                            status.code(),
                            stderr_preview,
                            stdout_log.canonicalize().unwrap_or(stdout_log).display(),
                            stderr_log.canonicalize().unwrap_or(stderr_log).display()
                        ))
                    }
                }
                .await;

                result.map_err(TaskFailure::from)
            }
            .await;

            // Exactly one report per started task, whichever way the body ended.
            let report = match outcome {
                Ok(()) if aggregator_only => TaskReport::success(task_name.clone()),
                Ok(()) => {
                    info!("Task {task_name} completed successfully");

                    // Convert .env output to JSON for downstream tasks
                    // This allows bash to write simple key=value format while maintaining JSON compatibility
                    let env_output_file = workspace.task_output_env_file(&task_name);
                    let json_output_file = workspace.task_output_file(&task_name);

                    if env_output_file.exists() {
                        // Read .env file and convert to JSON
                        if let Ok(env_content) = tokio::fs::read_to_string(&env_output_file).await
                            && let Ok(json_str) = serde_json::to_string_pretty(&env_to_json(&env_content))
                            && let Err(e) = tokio::fs::write(&json_output_file, json_str).await
                        {
                            log::warn!("Failed to write JSON output for task {task_name}: {e}");
                        }
                    } else if !json_output_file.exists() {
                        // If no output was written, create empty JSON
                        if let Err(e) = tokio::fs::write(&json_output_file, "{}").await {
                            log::warn!("Failed to write empty JSON output for task {task_name}: {e}");
                        }
                    }

                    // Record task completion in database (graceful degradation)
                    if let Some(task_id) = db_task_id
                        && let Some(store) = workspace.state_store()
                    {
                        let code = exit_code.unwrap_or(0);
                        let recorded = crate::ports::record_blocking(store, move |store| {
                            store.record_task_complete(task_id, code, super::state::TaskStatus::Completed)
                        })
                        .await;
                        if let Err(e) = recorded {
                            log::warn!("Failed to record task completion in database: {}", e);
                        }
                    }

                    TaskReport::success(task_name.clone()).with_drain(drain_issues)
                }
                Err(TaskFailure { error, exit_code: code }) => {
                    error!("Task {task_name} failed: {error}");
                    // The body's own observation wins; `code` only carries a
                    // value on paths that set it before failing.
                    let recorded = code.or(exit_code);

                    // Record task failure in database (graceful degradation)
                    if let Some(task_id) = db_task_id
                        && let Some(store) = workspace.state_store()
                    {
                        let code = recorded.unwrap_or(1);
                        // Degrading gracefully is not the same as saying
                        // nothing: this used to be `let _ =`, so a failure to
                        // record a failure vanished entirely.
                        let stored = crate::ports::record_blocking(store, move |store| {
                            store.record_task_complete(task_id, code, super::state::TaskStatus::Failed)
                        })
                        .await;
                        if let Err(e) = stored {
                            log::warn!("Failed to record task failure in database: {}", e);
                        }
                    }

                    {
                        let mut statuses = task_statuses.lock().await;
                        statuses.insert(task_name.clone(), TaskStatus::Failed(error.to_string()));
                    }
                    TaskReport::failure(task_name.clone(), error, recorded).with_drain(drain_issues)
                }
            };

            if let Err(e) = tx.send(report).await {
                error!("Failed to report outcome for task {task_name}: {e}");
            }
        });

        Ok(())
    }

}
