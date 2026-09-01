impl<F: FileSystem + 'static> TaskScheduler<F> {
    /// Record a task as Skipped with its provenance and a user-visible reason.
    ///
    /// Every skip that is not an up-to-date skip goes through here: unreachable
    /// dependency edges and serial-group cascades alike. The record lands in
    /// `skip_records`, the name and kind land in `skipped_set` so downstream gates
    /// classify against the kind, and the terminal prints the detail so a skipped
    /// task is never silent.
    ///
    /// Terminal transition 3 of the four the replay cursor is driven from: this
    /// path sends no `TaskReport`, so a cursor hung on the report channel would
    /// never emit a block for a skipped subtask and would stall every later
    /// item behind it.
    async fn mark_skipped(
        &self,
        task: &Task,
        record: SkipRecord,
        skipped_set: &mut SkippedSet,
        cursor: &mut ReplayCursor,
    ) {
        let SkipRecord { kind, detail } = &record;
        info!("Skipping task {} ({detail})", task.name);
        let msg = format!("{} skipped ({detail})\n", self.status_label(&task.name));
        self.report_status_line(cursor, &task.name, msg, false, Vec::new()).await;
        skipped_set.insert(task.name.clone(), *kind);
        {
            let mut statuses = self.task_statuses.lock().await;
            statuses.insert(task.name.clone(), TaskStatus::Skipped(*kind));
        }
        self.skip_records.lock().await.insert(task.name.clone(), record);
        self.broadcast_message(TaskMessage::Finished {
            task_name: task.name.clone(),
            status: TuiTaskStatus::Skipped,
            timestamp: std::time::SystemTime::now(),
            duration_ms: 0,
        });
    }

    /// Try to start a ready task, handling skipping and errors
    #[allow(clippy::too_many_arguments)]
    async fn try_start_ready_task(
        &self,
        task: Task,
        tx: mpsc::Sender<TaskReport>,
        active_tasks: &mut ActiveTasks,
        completed_set: &mut std::collections::HashSet<String>,
        blocked_tasks: &mut Vec<Task>,
        ready_queue: &mut std::collections::VecDeque<Task>,
        completed_tasks: &mut usize,
        total_tasks: usize,
        serial_groups: &SerialGroups,
        failed_set: &std::collections::HashSet<String>,
        skipped_set: &mut SkippedSet,
        cursor: &mut ReplayCursor,
    ) -> Result<()> {
        match self.needs_rebuild(&task).await {
            Ok(true) => {
                // Task needs to run
                info!("Starting task {} ({}/{})", task.name, *completed_tasks + 1, total_tasks);

                // Broadcast task started to TUI
                self.broadcast_message(TaskMessage::Started {
                    task_name: task.name.clone(),
                    timestamp: std::time::SystemTime::now(),
                });

                self.execute_task(task.clone(), tx.clone(), active_tasks).await?;
            }
            Ok(false) => {
                // Task can be skipped - outputs are up to date
                info!(
                    "Skipping task {} - outputs are up to date ({}/{})",
                    task.name,
                    *completed_tasks + 1,
                    total_tasks
                );

                // Terminal transition 4 of four. Like `mark_skipped`, this path
                // mutates the terminal-state sets inline and sends no report, so
                // the cursor has to be advanced from here too.
                let skipped_msg = format!("{} skipped (up to date)\n", self.status_label(&task.name));
                self.report_status_line(cursor, &task.name, skipped_msg, false, Vec::new())
                    .await;

                // Broadcast task skipped to TUI
                self.broadcast_message(TaskMessage::StatusChange {
                    task_name: task.name.clone(),
                    status: TuiTaskStatus::Skipped,
                    timestamp: std::time::SystemTime::now(),
                });

                {
                    let mut statuses = self.task_statuses.lock().await;
                    statuses.insert(task.name.clone(), TaskStatus::Skipped(SkipKind::UpToDate));
                }
                // An up-to-date skip is success-like, so it lands in `completed_set`
                // like any other success. It is also recorded in `skipped_set` with
                // its kind, because it is still terminal-Skipped and the gates must
                // be able to tell it apart from a gated-out skip.
                completed_set.insert(task.name.clone());
                skipped_set.insert(task.name.clone(), SkipKind::UpToDate);
                *completed_tasks += 1;

                blocked_tasks.retain(|blocked_task| {
                    let task_deps_completed = blocked_task
                        .task_deps
                        .iter()
                        .all(|task_dep| completed_set.contains(&task_dep.task));
                    if !task_deps_completed {
                        return true; // Keep the task in blocked list
                    }

                    // The serial gate composes with dependency readiness: a member whose
                    // predecessor has not finished stays blocked.
                    if !matches!(
                        serial_groups.classify(blocked_task, completed_set, failed_set, skipped_set),
                        EdgeState::Satisfied
                    ) {
                        return true;
                    }

                    // All dependencies are completed, move to ready queue
                    ready_queue.push_back(blocked_task.clone());
                    false // Remove from blocked list
                });
            }
            Err(e) => {
                error!("Error checking file dependencies for task {}: {}", task.name, e);
                // On error, default to running the task
                info!(
                    "Starting task {} (file check failed, defaulting to run) ({}/{})",
                    task.name,
                    *completed_tasks + 1,
                    total_tasks
                );

                // Broadcast task started to TUI
                self.broadcast_message(TaskMessage::Started {
                    task_name: task.name.clone(),
                    timestamp: std::time::SystemTime::now(),
                });

                self.execute_task(task.clone(), tx.clone(), active_tasks).await?;
            }
        }
        Ok(())
    }

    /// Stop the run: kill the in-flight children and report the cancellation.
    ///
    /// The children are killed rather than waited on, which is the whole point of a
    /// cancel: `kill_on_drop(true)` on every spawned command means aborting the task
    /// bodies takes the processes with them.
    ///
    /// Buffered blocks are not dropped on the floor here. `abandon_run` never
    /// marks in-flight tasks terminal and never touches a log, so without the
    /// flush every completed-but-not-yet-replayed block would vanish. The
    /// report channel is drained first, which is what turns the "report sent
    /// but not yet consumed" state into a complete block rather than a
    /// did-not-start line.
    async fn abandon_run(
        &self,
        active_tasks: &mut ActiveTasks,
        cursor: &mut ReplayCursor,
        rx: &mut mpsc::Receiver<TaskReport>,
    ) -> Result<()> {
        let abandoned = active_tasks.len();
        info!("Run cancelled; killing {abandoned} in-flight task(s)");
        if !cursor.is_empty() {
            while let Ok(report) = rx.try_recv() {
                self.record_report_for_replay(cursor, &report).await;
            }
        }
        active_tasks.abort_all();
        let notice = format!("otto: run cancelled; {abandoned} running task(s) killed\n");
        if self.tui_mode {
            self.persist_skip_records().await;
            return Err(eyre!("run cancelled; {abandoned} running task(s) were killed"));
        }
        self.flush_cancelled_groups(cursor, notice).await;
        self.persist_skip_records().await;
        Err(eyre!("run cancelled; {abandoned} running task(s) were killed"))
    }


    /// Write each skipped task and why it was skipped into the run record, so
    /// `otto History` can say why a task did not run.
    ///
    /// Both halves of the record are persisted: `skip_kind` for anything that
    /// wants to filter by reason class, `skip_reason` for the operator.
    async fn persist_skip_records(&self) {
        let records = self.get_skip_records().await;
        if records.is_empty() {
            return;
        }
        let (Some(run_id), Some(store)) = (self.workspace.db_run_id(), self.workspace.state_store()) else {
            return;
        };
        for (task_name, record) in records {
            let name = task_name.clone();
            let detail = record.detail.clone();
            let kind = record.kind;
            let recorded = crate::ports::record_blocking(store, move |store| {
                store.record_task_skipped(run_id, &name, None, Some(&detail), Some(kind))
            })
            .await;
            if let Err(e) = recorded {
                log::warn!("Failed to record skipped task {task_name} in database: {e}");
            }
        }
    }


    pub async fn get_task_statuses(&self) -> HashMap<String, TaskStatus> {
        self.task_statuses.lock().await.clone()
    }

    /// Why each task skipped by an unreachable dependency edge or a serial-group
    /// cascade was skipped, keyed by task name. Up-to-date skips are absent by
    /// design: they are successes, not gated-out tasks.
    pub async fn get_skip_records(&self) -> HashMap<String, SkipRecord> {
        self.skip_records.lock().await.clone()
    }

    pub async fn get_task_status(&self, task_name: &str) -> TaskStatus {
        let statuses = self.task_statuses.lock().await;
        statuses.get(task_name).cloned().unwrap_or(TaskStatus::Pending)
    }

    pub async fn needs_rebuild(&self, task: &Task) -> Result<bool> {
        // If no file dependencies, always run (traditional task-only mode)
        if task.file_deps.is_empty() {
            debug!("Task {} has no file dependencies, will run", task.name);
            return Ok(true);
        }

        let output_files = &task.output_deps;

        // If no output files exist, need to run
        if output_files.is_empty() {
            debug!("Task {} has no output files defined, will run", task.name);
            return Ok(true);
        }

        for output_path in output_files {
            if !Path::new(output_path).exists() {
                debug!(
                    "Output file {} does not exist, task {} needs to run",
                    output_path, task.name
                );
                return Ok(true);
            }
        }

        let input_timestamps = self.get_file_timestamps(&task.file_deps).await?;
        let output_timestamps = self.get_file_timestamps(output_files).await?;

        // Find the newest input and oldest output
        let newest_input = input_timestamps.iter().filter_map(|(_, time)| *time).max();
        let oldest_output = output_timestamps.iter().filter_map(|(_, time)| *time).min();

        match (newest_input, oldest_output) {
            (Some(input_time), Some(output_time)) => {
                let needs_rebuild = input_time > output_time;
                if needs_rebuild {
                    debug!("Input files newer than outputs, task {} needs to run", task.name);
                } else {
                    debug!("Outputs up to date, task {} can be skipped", task.name);
                }
                Ok(needs_rebuild)
            }
            (None, _) => {
                debug!("No input files found, task {} will run", task.name);
                Ok(true) // No inputs found, run the task
            }
            (_, None) => {
                debug!("No output files found, task {} needs to run", task.name);
                Ok(true) // No outputs found, need to run
            }
        }
    }

    async fn get_file_timestamps(&self, file_paths: &[String]) -> Result<Vec<(String, Option<std::time::SystemTime>)>> {
        let mut timestamps = Vec::new();

        for file_path in file_paths {
            let path = Path::new(file_path);
            let timestamp = if path.exists() {
                match tokio::fs::metadata(path).await {
                    Ok(metadata) => match metadata.modified() {
                        Ok(time) => Some(time),
                        Err(e) => {
                            debug!("Could not get modification time for {file_path}: {e}");
                            None
                        }
                    },
                    Err(e) => {
                        debug!("Could not get metadata for {file_path}: {e}");
                        None
                    }
                }
            } else {
                debug!("File {file_path} does not exist");
                None
            };
            timestamps.push((file_path.clone(), timestamp));
        }

        Ok(timestamps)
    }
}
