// Ordered replay for `foreach.buffer: true` (design doc
// `docs/design/2026-08-31-buffered-foreach-computed-envs-required-params.md`,
// Phase 4). Included into `scheduler.rs` like `support.rs` and
// `task_execution.rs`, because that file is already 1288 lines against the
// 1500-line cap.
//
// The shape, in one paragraph: a buffered subtask runs with
// `suppress_terminal: true`, so its bytes reach only `stdout.log` and
// `stderr.log`. A per-parent cursor over the Phase 3 display-order map decides
// when each subtask's slot comes up; the moment the subtask at the cursor
// reaches a terminal state its two logs are streamed to the terminal, its
// scheduler status line is appended, and any already-finished successors are
// drained behind it. Execution stays fully concurrent; only display is
// serialized.

/// Read buffer for a replayed log. Peak memory during replay is this plus one
/// chunk, never the size of the log: that is why replay does not go through
/// `TaskStreams::read_output`, which collects the whole file into a `Vec<String>`.
const REPLAY_BUFFER_BYTES: usize = 64 * 1024;

/// Largest chunk written in one go. A log line longer than this is emitted in
/// pieces (prefixed once, at its start) rather than accumulated whole, so a
/// task that prints a megabyte without a newline cannot make replay unbounded.
const REPLAY_CHUNK_BYTES: usize = 64 * 1024;

/// What replay prints for one subtask when its slot comes up.
enum BlockKind {
    /// Stream `stdout.log` then `stderr.log`, then the status line. The normal
    /// case, and the only one a non-cancelled run produces.
    Logs,
    /// The subtask's child was launched and then killed by a cancellation, so
    /// its logs are partial by construction. Its run-dir paths are printed
    /// instead: nothing silently discarded, nothing silently truncated.
    KilledPaths,
    /// The subtask never got as far as a child process. There is no log to
    /// point at, so it gets one line saying it did not start.
    DidNotStart,
}

/// One subtask's replay, fully resolved: everything the blocking writer needs,
/// owned, so the closure borrows nothing from the scheduler.
struct ReplayBlock {
    task_name: String,
    kind: BlockKind,
    stdout_log: PathBuf,
    stderr_log: PathBuf,
    /// The scheduler status line, already formatted with the `[task]`/`task`
    /// label. It travels with the block rather than printing at completion
    /// time, or status lines would arrive in completion order while blocks
    /// arrive in item order.
    status_line: Option<String>,
    /// Failure lines go to stderr, exactly as they do unbuffered.
    status_to_stderr: bool,
    /// Streams whose drain did not complete. Empty means the logs are whole.
    drain: Vec<DrainIssue>,
}

/// A terminal transition recorded for a buffered subtask whose slot has not
/// come up yet.
struct PendingBlock {
    status_line: String,
    status_to_stderr: bool,
    drain: Vec<DrainIssue>,
}

/// One buffered foreach group's replay state.
struct ReplayGroup {
    /// Subtask names in declared foreach item order, from the Phase 3
    /// display-order map, filtered to the tasks actually in this run.
    items: Vec<String>,
    /// Index of the next item whose block has not been emitted.
    cursor: usize,
    /// Terminal transitions recorded but not yet emitted, by subtask name.
    pending: HashMap<String, PendingBlock>,
    /// Subtasks whose block has already been printed, so no backstop and no
    /// cancellation flush can print one twice.
    emitted: HashSet<String>,
}

/// What a cancelled run owes one item of a buffered group.
///
/// Six states collapse onto four outcomes, and the collapse is the point: the
/// design doc's table distinguishes six because `ActiveTasks` tracks spawned
/// bodies, not child or log state, so "active" and "has a killed child with
/// logs" are not the same thing and a body that finished and sent an unread
/// report is neither killed nor unstarted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CancelledItem {
    /// Its block was already printed during the run; nothing more is owed.
    AlreadyEmitted,
    /// Terminal, or a report that was sent and never consumed. Either way the
    /// logs are complete, so the block prints in full.
    Block,
    /// A child was launched and killed. Its logs are partial by construction,
    /// so its run-dir paths print instead of a partial block.
    KilledChild,
    /// No child ever ran: the body was spawned but had not launched one, or the
    /// item was still ready-queued, or still blocked. There is no log to point
    /// at, so it gets a did-not-start line.
    NeverStarted,
}

/// Decide what a cancelled run owes each item of one group, in item order,
/// never stopping early.
///
/// Pure, and separate from the writing, because the property that matters here
/// is not the bytes: it is that a killed or unstarted item does not swallow
/// every later completed block behind it. That is the same stall class the
/// four-site cursor exists to prevent, reappearing on the one path the four
/// sites do not drive.
fn plan_cancelled_group<'a>(
    items: &'a [String],
    emitted: &HashSet<String>,
    recorded: &HashSet<String>,
    statuses: &HashMap<String, TaskStatus>,
    log_exists: &dyn Fn(&str) -> bool,
) -> Vec<(&'a str, CancelledItem)> {
    items
        .iter()
        .map(|name| {
            let state = if emitted.contains(name) {
                CancelledItem::AlreadyEmitted
            } else if recorded.contains(name) {
                CancelledItem::Block
            } else if matches!(statuses.get(name), Some(TaskStatus::Running)) && log_exists(name) {
                // The log file is the child's fingerprint: `process_output`
                // creates it the moment the child is spawned, so its presence
                // separates a killed child from a body that never launched one.
                CancelledItem::KilledChild
            } else {
                CancelledItem::NeverStarted
            };
            (name.as_str(), state)
        })
        .collect()
}

/// Ordered replay state for every buffered foreach group in the run.
///
/// A plain local on the `execute_all` frame, beside `completed_set` /
/// `failed_set` / `skipped_set`: no `Arc<Mutex>`, and so no lock ordering to
/// get wrong. It is advanced by one helper called from all four
/// terminal-transition sites, because only two of the four go through the
/// report channel; a cursor driven from the report arms alone would stall
/// forever behind a skipped subtask.
#[derive(Default)]
struct ReplayCursor {
    /// Parent task name -> group state.
    groups: HashMap<String, ReplayGroup>,
    /// Subtask name -> its parent, so a transition site can find its group
    /// from the only thing it has: the task's own name.
    parent_of: HashMap<String, String>,
    /// Parent names in run order, so a cancellation flush is deterministic.
    parents: Vec<String>,
}

impl ReplayCursor {
    /// Build the cursor from the run set.
    ///
    /// Empty under `--tui`, where the terminal leg is already suppressed and
    /// buffering has nothing to do. Each group's item list is filtered to the
    /// tasks actually in this run: an item that is not going to reach a
    /// terminal state would stall its group's cursor forever.
    fn new(tasks: &[Task], tui_mode: bool) -> Self {
        let mut cursor = Self::default();
        if tui_mode {
            return cursor;
        }
        let in_run: HashSet<&str> = tasks.iter().map(|t| t.name.as_str()).collect();
        for task in tasks {
            if !(task.buffered && task.is_virtual_parent) {
                continue;
            }
            let Some(order) = task.foreach_display_order.as_ref() else {
                continue;
            };
            let items: Vec<String> = order
                .iter()
                .filter(|name| in_run.contains(name.as_str()))
                .cloned()
                .collect();
            if items.is_empty() {
                continue;
            }
            for item in &items {
                cursor.parent_of.insert(item.clone(), task.name.clone());
            }
            cursor.parents.push(task.name.clone());
            cursor.groups.insert(
                task.name.clone(),
                ReplayGroup {
                    items,
                    cursor: 0,
                    pending: HashMap::new(),
                    emitted: HashSet::new(),
                },
            );
        }
        debug!("ReplayCursor::new: buffered_groups={}", cursor.groups.len());
        cursor
    }

    /// True when this run buffers nothing, so every hook can return early.
    fn is_empty(&self) -> bool {
        self.groups.is_empty()
    }

    /// The parent of `task_name`, when it is a buffered subtask.
    fn parent_of(&self, task_name: &str) -> Option<&str> {
        self.parent_of.get(task_name).map(String::as_str)
    }

    /// Record a buffered subtask's terminal transition, returning `false` when
    /// the task is not buffered at all (so the caller prints normally).
    fn record(&mut self, task_name: &str, block: PendingBlock) -> bool {
        let Some(parent) = self.parent_of.get(task_name).cloned() else {
            return false;
        };
        let Some(group) = self.groups.get_mut(&parent) else {
            return false;
        };
        if group.emitted.contains(task_name) {
            // Already printed: a second transition for the same subtask would
            // duplicate its block rather than correct it.
            return true;
        }
        group.pending.insert(task_name.to_string(), block);
        true
    }

    /// Take the run of consecutive recorded items starting at the cursor, and
    /// advance past them. Nothing is taken when the cursor's own item has not
    /// reached a terminal state yet, which is exactly the item-order guarantee.
    fn take_ready(&mut self, parent: &str) -> Vec<(String, PendingBlock)> {
        let Some(group) = self.groups.get_mut(parent) else {
            return Vec::new();
        };
        let mut ready = Vec::new();
        while group.cursor < group.items.len() {
            let name = group.items[group.cursor].clone();
            let Some(block) = group.pending.remove(&name) else {
                break;
            };
            group.emitted.insert(name.clone());
            group.cursor += 1;
            ready.push((name, block));
        }
        ready
    }

    /// End-of-group backstop: take every remaining recorded item in item order
    /// and run the cursor to the end. Nothing should be left here (all four
    /// terminal-transition sites record, and the virtual parent's
    /// `When::Always` edges mean it is queued only once every subtask is
    /// terminal), which is why this is a backstop and not the mechanism.
    fn take_remaining(&mut self, parent: &str) -> Vec<(String, PendingBlock)> {
        let Some(group) = self.groups.get_mut(parent) else {
            return Vec::new();
        };
        let mut ready = Vec::new();
        while group.cursor < group.items.len() {
            let name = group.items[group.cursor].clone();
            group.cursor += 1;
            if let Some(block) = group.pending.remove(&name) {
                group.emitted.insert(name.clone());
                ready.push((name, block));
            }
        }
        ready
    }
}

/// Read up to and including the next `\n`, or [`REPLAY_CHUNK_BYTES`], whichever
/// comes first. Returns `true` when the chunk ended at a newline.
///
/// `BufRead::read_until` would be simpler and unbounded: one line with no
/// newline in it would be read entirely into memory, which is the property
/// replay is specified not to have.
fn read_bounded_chunk(reader: &mut impl BufRead, chunk: &mut Vec<u8>) -> io::Result<bool> {
    loop {
        let available = match reader.fill_buf() {
            Ok(buf) => buf,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(e),
        };
        if available.is_empty() {
            return Ok(false);
        }
        let room = REPLAY_CHUNK_BYTES.saturating_sub(chunk.len());
        match available.iter().position(|b| *b == b'\n') {
            Some(index) if index < room => {
                chunk.extend_from_slice(&available[..=index]);
                reader.consume(index + 1);
                return Ok(true);
            }
            _ => {
                let take = room.min(available.len());
                chunk.extend_from_slice(&available[..take]);
                reader.consume(take);
                if chunk.len() >= REPLAY_CHUNK_BYTES {
                    return Ok(false);
                }
            }
        }
    }
}

/// Stream one log file to `out`, prefixed the same way the live terminal leg
/// would have prefixed it.
///
/// A missing log is not an error: a subtask that was skipped, or that never
/// wrote anything, contributes nothing but its status line.
fn stream_log(out: &mut impl Write, path: &Path, task_name: &str, no_prefix: bool) -> io::Result<()> {
    let file = match fs::File::open(path) {
        Ok(file) => file,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(e),
    };
    let mut reader = io::BufReader::with_capacity(REPLAY_BUFFER_BYTES, file);
    let mut chunk: Vec<u8> = Vec::with_capacity(REPLAY_CHUNK_BYTES);
    // A chunk that did not end at a newline is the middle of a long line, so
    // the next one continues it and must not be prefixed again.
    let mut mid_line = false;
    loop {
        chunk.clear();
        let complete = read_bounded_chunk(&mut reader, &mut chunk)?;
        if chunk.is_empty() {
            break;
        }
        if mid_line {
            out.write_all(&chunk)?;
        } else {
            out.write_all(format_terminal_output(task_name, &chunk, no_prefix).as_bytes())?;
        }
        mid_line = !complete;
    }
    if mid_line {
        // The log's last line had no trailing newline. Live output would have
        // left the cursor mid-line; a block must end cleanly so the status line
        // that follows it starts at column zero.
        out.write_all(b"\n")?;
    }
    Ok(())
}

/// The loud marker that ends a block whose drain did not complete.
///
/// Terminal state is not the same as output complete: `task_execution.rs` only
/// `error!`-logs a drain failure or timeout and then falls through to the
/// process's exit status. Without this a short block would print under a
/// "finished successfully" line with nothing said.
fn truncation_marker(block: &ReplayBlock, issue: &DrainIssue) -> String {
    let (stream, path) = match issue.stream {
        OutputType::Stdout => ("stdout", &block.stdout_log),
        OutputType::Stderr => ("stderr", &block.stderr_log),
    };
    format!(
        "otto: WARNING: {} {} output may be truncated: {}; full log: {}\n",
        block.task_name,
        stream,
        issue.condition.describe(),
        path.display()
    )
}

/// Write a batch of blocks, and an optional notice ahead of them, as one
/// contiguous run of terminal output.
///
/// Runs inside `spawn_blocking`: the process-wide terminal lock is taken and
/// released entirely inside this function, using `std::fs` and locked
/// `io::stdout()`/`io::stderr()`. Doing this inline in the `execute_all` ready
/// loop would stall a tokio worker and starve every concurrent task, and the
/// block cannot be assembled with `.await` points inside the lock.
fn write_replay_blocks(notice: Option<String>, blocks: Vec<ReplayBlock>, no_prefix: bool) {
    let _terminal = terminal_lock();
    let stdout = io::stdout();
    let stderr = io::stderr();
    let mut out = stdout.lock();
    let mut err = stderr.lock();

    // Every write is best-effort: a closed pipe must not take the run down from
    // the display path, and there is nowhere left to report it to.
    if let Some(notice) = notice {
        let _ = err.write_all(notice.as_bytes());
        let _ = err.flush();
    }

    for block in blocks {
        match block.kind {
            BlockKind::Logs => {
                let _ = stream_log(&mut out, &block.stdout_log, &block.task_name, no_prefix);
                let _ = out.flush();
                let _ = stream_log(&mut err, &block.stderr_log, &block.task_name, no_prefix);
                for issue in &block.drain {
                    let _ = err.write_all(truncation_marker(&block, issue).as_bytes());
                }
                let _ = err.flush();
            }
            BlockKind::KilledPaths => {
                let _ = err.write_all(
                    format!(
                        "otto: {} was killed mid-run; its logs are partial and are not replayed: {} {}\n",
                        block.task_name,
                        block.stdout_log.display(),
                        block.stderr_log.display()
                    )
                    .as_bytes(),
                );
                let _ = err.flush();
            }
            BlockKind::DidNotStart => {
                let _ = err.write_all(format!("otto: {} did not start\n", block.task_name).as_bytes());
                let _ = err.flush();
            }
        }
        if let Some(line) = block.status_line {
            if block.status_to_stderr {
                let _ = err.write_all(line.as_bytes());
                let _ = err.flush();
            } else {
                let _ = out.write_all(line.as_bytes());
                let _ = out.flush();
            }
        }
    }
}

impl<F: FileSystem + 'static> TaskScheduler<F> {
    /// The one place a task's scheduler status line reaches the terminal.
    ///
    /// Called from all four terminal-transition sites. For a buffered subtask
    /// the line is stashed and travels with the block; for everything else it
    /// prints immediately, now under the process-wide terminal lock so it can
    /// never land in the middle of someone else's replayed block.
    async fn report_status_line(
        &self,
        cursor: &mut ReplayCursor,
        task_name: &str,
        line: String,
        to_stderr: bool,
        drain: Vec<DrainIssue>,
    ) {
        if !cursor.is_empty()
            && cursor.record(
                task_name,
                PendingBlock {
                    status_line: line.clone(),
                    status_to_stderr: to_stderr,
                    drain,
                },
            )
        {
            self.replay_ready_blocks(cursor, task_name).await;
            return;
        }

        if self.tui_mode {
            return;
        }
        let _terminal = terminal_lock();
        if to_stderr {
            eprint!("{line}");
            io::stderr().flush().unwrap_or(());
        } else {
            print!("{line}");
            io::stdout().flush().unwrap_or(());
        }
    }

    /// Emit every block that is now unblocked in `task_name`'s group: the
    /// cursor item, plus each already-finished successor behind it.
    async fn replay_ready_blocks(&self, cursor: &mut ReplayCursor, task_name: &str) {
        let Some(parent) = cursor.parent_of(task_name).map(str::to_string) else {
            return;
        };
        let ready = cursor.take_ready(&parent);
        self.write_blocks(None, self.blocks_from(ready)).await;
    }

    /// End-of-group backstop, hooked into the parent's success arm AFTER the
    /// aggregation override so the override still runs before
    /// `completed_set.insert` and the blocked-tasks sweep.
    async fn replay_flush_group(&self, cursor: &mut ReplayCursor, parent: &str) {
        if cursor.is_empty() {
            return;
        }
        let remaining = cursor.take_remaining(parent);
        if remaining.is_empty() {
            return;
        }
        debug!("replay_flush_group: parent={parent} backstop_blocks={}", remaining.len());
        self.write_blocks(None, self.blocks_from(remaining)).await;
    }

    /// Turn recorded transitions into blocks, resolving each subtask's two log
    /// paths from the workspace. No new run-dir file: these are the same
    /// `stdout.log` / `stderr.log` every task already writes.
    fn blocks_from(&self, ready: Vec<(String, PendingBlock)>) -> Vec<ReplayBlock> {
        ready
            .into_iter()
            .map(|(name, pending)| ReplayBlock {
                stdout_log: self.workspace.stdout(&name),
                stderr_log: self.workspace.stderr(&name),
                task_name: name,
                kind: BlockKind::Logs,
                status_line: Some(pending.status_line),
                status_to_stderr: pending.status_to_stderr,
                drain: pending.drain,
            })
            .collect()
    }

    /// Hand a batch to the blocking pool and await only the join handle, so no
    /// tokio worker is parked on file I/O under a held lock.
    async fn write_blocks(&self, notice: Option<String>, blocks: Vec<ReplayBlock>) {
        if notice.is_none() && blocks.is_empty() {
            return;
        }
        let no_prefix = self.no_prefix;
        if let Err(e) = tokio::task::spawn_blocking(move || write_replay_blocks(notice, blocks, no_prefix)).await {
            error!("Buffered foreach replay failed to run: {e}");
        }
    }

    /// Record a report the scheduler never got to consume.
    ///
    /// On the cancel path the body may have finished and sent its report while
    /// `execute_all` was already returning. Those logs are complete, so the
    /// block is printed in full: draining the channel here is what tells that
    /// state apart from a body whose child was killed.
    async fn record_report_for_replay(&self, cursor: &mut ReplayCursor, report: &TaskReport) {
        if cursor.parent_of(&report.name).is_none() {
            return;
        }
        let (word, to_stderr) = match &report.error {
            None => (task_outcome_word(&TaskStatus::Completed), false),
            Some(_) => ("failed", true),
        };
        cursor.record(
            &report.name,
            PendingBlock {
                status_line: format!("{} {word}\n", self.status_label(&report.name)),
                status_to_stderr: to_stderr,
                drain: report.drain.clone(),
            },
        );
    }

    /// Cancellation flush: ordered, but never stopping early.
    ///
    /// Walks each buffered group's whole item list and emits exactly one thing
    /// per item, per the design doc's six-state table. Halting at the first
    /// non-terminal item would lose every later completed block behind a killed
    /// or unstarted earlier one, which is the same stall class the four-site
    /// cursor exists to prevent, reappearing on the one path the four sites do
    /// not drive: cancellation returns before both the post-loop reconciliation
    /// and the report funnel, so this flush can rely on neither.
    async fn flush_cancelled_groups(&self, cursor: &mut ReplayCursor, notice: String) {
        if cursor.is_empty() {
            self.write_blocks(Some(notice), Vec::new()).await;
            return;
        }
        let statuses = self.task_statuses.lock().await.clone();
        let workspace = self.workspace.clone();
        let log_exists = move |name: &str| workspace.stdout(name).exists();
        let mut blocks = Vec::new();
        for parent in cursor.parents.clone() {
            let Some(group) = cursor.groups.get_mut(&parent) else {
                continue;
            };
            let recorded: HashSet<String> = group.pending.keys().cloned().collect();
            let plan = plan_cancelled_group(&group.items, &group.emitted, &recorded, &statuses, &log_exists);
            let plan: Vec<(String, CancelledItem)> = plan.into_iter().map(|(n, s)| (n.to_string(), s)).collect();
            for (name, state) in plan {
                if state == CancelledItem::AlreadyEmitted {
                    continue;
                }
                group.emitted.insert(name.clone());
                let stdout_log = self.workspace.stdout(&name);
                let stderr_log = self.workspace.stderr(&name);
                let pending = group.pending.remove(&name);
                blocks.push(ReplayBlock {
                    task_name: name,
                    kind: match state {
                        CancelledItem::Block => BlockKind::Logs,
                        CancelledItem::KilledChild => BlockKind::KilledPaths,
                        _ => BlockKind::DidNotStart,
                    },
                    stdout_log,
                    stderr_log,
                    status_line: pending.as_ref().map(|p| p.status_line.clone()),
                    status_to_stderr: pending.as_ref().is_some_and(|p| p.status_to_stderr),
                    drain: pending.map(|p| p.drain).unwrap_or_default(),
                });
            }
            group.cursor = group.items.len();
        }
        self.write_blocks(Some(notice), blocks).await;
    }
}
