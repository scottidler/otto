# Implementation notes: second code review remediation

Companion to `docs/design/2026-09-02-second-code-review-remediation.md`. One
section per phase, appended as each phase lands. Append-only.

## Phase 1: Executor process boundary: stdin and SIGTERM

### Design decisions
- Both stdin routes live in ONE `if !tty` block — `src/executor/scheduler/task_execution.rs`, `execute_task` — because the doc's round-3 S2 finding is that they are not two parallel halves: `setsid`'s guarantee only holds for a bash body if fd 0 is not a terminal, since a session leader acquires a controlling terminal by opening one at startup. Keeping them adjacent under one comment is what stops a later edit from deleting one and leaving the other looking sufficient.
- `setsid` goes through `tokio::process::Command::pre_exec` rather than `std::os::unix::process::CommandExt` — `task_execution.rs`, `execute_task` — tokio's `Command` exposes `pre_exec` as an inherent unsafe method, so no extension trait import is needed and the `#[cfg(unix)]` sits on the one `unsafe` block.
- One helper, two call sites — `src/app.rs`, `next_stop_signal` and `install_stop_handler` — the plain path and the `--tui` path had two hand-written signal tasks that already disagreed (one cancelled, one only set a flag). They now differ only in two closures: what to do after the first cancel (`--tui` sets its quit flag) and what to undo before the second-signal exit (`--tui` hands the terminal back). The doc's success criterion is a count of at least 1, precisely so this factoring is allowed.
- Signal numbers come from `libc::SIGINT` / `SIGTERM` / `SIGHUP` rather than literals — `app.rs`, `next_stop_signal` — the number is only ever used to build the 128 + n exit and `libc` is already a direct dependency.
- The three plain/TUI signal tests read liveness from `/proc/<pid>/cmdline` content, not pid existence — `tests/sigint_cancel_test.rs`, `still_running` — same rule `cancel_reaping_test.rs` states: pids recycle and a zombie still has a `/proc` entry. That makes those three tests `#[cfg(target_os = "linux")]`, matching the existing reaping suite.
- The grandchild mark in the new signal fixture is `sleep 603` — `tests/sigint_cancel_test.rs`, `GRANDCHILD_MARK` — distinct from `cancel_reaping_test.rs`'s 601 and 602 so the two binaries cannot read each other's processes when cargo runs them concurrently.
- The pty read tests set `HOME` to the fixture directory as well as `OTTO_HOME` — `tests/tty_task_test.rs`, `PtyRun::start` — the `bash -ic` case sources the developer's own startup files, which on this machine printed seven lines of zsh-only syntax errors. Pointing `HOME` at the fixture makes the case read the same nothing everywhere.
- `tests/cancel_reaping_test.rs` was left byte-for-byte untouched, including its module comment's historical reference to `cmd.process_group(0)`, which reads as narrative about v2.1.0 and is still true as history. `git diff --name-only tests/cancel_reaping_test.rs` is empty.

### Deviations
- **Renamed `restore_terminal_on_panic` to `restore_terminal_best_effort`** (`src/tui/mod.rs`, plus its two hook call sites and one test). The doc says the second-signal escape hatch "restores the terminal first (`terminal.restore()`)", but the signal task is a detached `tokio::spawn` that cannot own the `TerminalGuard`; the correct seam is the existing free function that goes through the same `claim_terminal_restore` mechanism. Its old name would then have been a lie about who calls it. Same effect, correct seam.
- **Renamed `install_interrupt_handler` to `install_stop_handler`** (`src/app.rs`, and the one prose reference at `src/executor/scheduler.rs`'s `CANCEL_GRACE`). It handles three signals now; "interrupt" named one of them.
- **Timeouts in the new pty read tests are 30s, not the doc's "within one second".** The behavior being excluded is an *unbounded* hang (before this phase the same fixtures ran until an external `timeout` killed them), so the bound only has to be finite; one second would flake on a loaded machine on otto's startup alone. Measured locally the whole `tty_task_test` binary, all eleven tests, finishes in 1.80s.
- **Four present-tense comments outside the doc's list were corrected** because `process_group(0)` is no longer what happens: `scheduler.rs` (`ChildHandle::pid`'s doc and `signal_child`'s SAFETY note), `scheduler/support.rs` (`abandon_run`'s doc), and `app.rs` (the stop handler's doc). The doc names only `task_execution.rs:274`; leaving the other four would have left the dead mechanism written down in four places.
- **Test placement follows the doc, including the file name.** The SIGTERM, SIGHUP and `--tui` SIGHUP tests are in `tests/sigint_cancel_test.rs`, whose name now covers only one of the four signals it tests. Renaming the file was not in scope for this phase; the module comment was rewritten to say what the file actually covers.

### Tradeoffs
- **`next_stop_signal` re-registers its `Signal` streams on the second call, rather than a struct holding them across both awaits.** A signal arriving in the microsecond gap between the two calls is absorbed by tokio's process-level handler (which is installed once and never uninstalled) rather than killing otto, so the gap cannot resurrect the default disposition; a struct would have added a type for no behavior change. The pre-existing `ctrl_c()`-twice code had exactly this shape.
- **`bash -ic` is asserted on the substring `/dev/tty`, not on the whole stream** (`a_non_tty_task_opening_dev_tty_from_an_interactive_shell_fails_too`), because an interactive bash with no controlling terminal also prints its own job-control warning. The doc's round-3 N1 nit says the same.
- **Change (b) of the TUI fix — cancel-then-propagate at the draw-error path — has no isolating regression test, and I could not build one.** Measured: with (b) reverted but (a) in place, killing the pty master under `--tui` still reaped the grandchild in 3 runs of 3, because the signal task's `cancel()` wins the race against process exit. (b) converts that race into a guarantee by awaiting the scheduler handle before propagating; it is determinism, not mechanism, so nothing observable goes red when only it is removed. Recorded rather than papered over.

### Break-the-test proofs
Per the doc's Testing Strategy. Each fix reverted alone, the suite run, the fix restored.

- **Remove `cmd.stdin(Stdio::null())`, keep `setsid`.** `a_non_tty_task_reading_stdin_under_a_pty_gets_eof` FAILED: `otto did not exit within 30s; the task is still blocked on its read`. The other ten tests in the binary passed, which is the doc's measured result: a pty fd inherited into another session blocks rather than stopping.
- **Revert `setsid` to `cmd.process_group(0)`, keep the stdin nulling.** Two tests FAILED, both on the same timeout: `a_non_tty_task_opening_dev_tty_fails_instead_of_hanging` and `a_non_tty_task_opening_dev_tty_from_an_interactive_shell_fails_too`. `a_non_tty_task_reading_stdin_under_a_pty_gets_eof` still passed, which is the two-halves point: neither half covers the other.
- **Revert the stop handler to `ctrl_c()` only (the pre-phase shape).** Three tests FAILED and the pre-existing Ctrl+C test still passed:
  - `a_sigterm_on_a_plain_run_reaps_the_task_subtree`: `SIGTERM must reach abandon_run, which announces the cancellation:` with empty output — otto died on the default disposition and printed nothing.
  - `a_sighup_on_a_plain_run_reaps_the_task_subtree`: same, empty output.
  - `a_sighup_during_a_tui_run_reaps_the_task_subtree_and_restores_the_terminal`: `this grandchild outlived it: Some("1642719 -> sleep 603")`, and the captured stream contained the alternate-screen ENTER with no LEAVE — the orphaned subtree and the wedged terminal, both, on the path the doc's round-4 finding named.

### Success criteria, as run
- `grep -c 'SignalKind::hangup()' src/app.rs` -> `1` (>= 1 required). PASS.
- SIGTERM and SIGHUP leave zero descendants of the non-`tty` task, plain and `--tui`: `a_sigterm_on_a_plain_run_reaps_the_task_subtree`, `a_sighup_on_a_plain_run_reaps_the_task_subtree`, `a_sighup_during_a_tui_run_reaps_the_task_subtree_and_restores_the_terminal` all pass. PASS.
- `grep -c 'cmd.process_group(0)' src/executor/scheduler/task_execution.rs` -> `0`. PASS.
- `grep -c 'setsid' src/executor/scheduler/task_execution.rs` -> `5` (>= 1 required). PASS.
- The three non-tty pty tests complete under their timeouts with the read failing: all three pass, binary finishes in 1.80s against a 30s per-run bound. PASS.
- The `tty: true` pty test reads the byte: `a_tty_task_reads_the_byte_typed_at_the_terminal` asserts `read=[x]` and passes. PASS.
- `cancel_reaping_test` passes unchanged: 3 passed, and `git diff --name-only tests/cancel_reaping_test.rs` is empty. PASS.

### Open questions
- **`otto-dev` is a cross-repo consequence of this phase and was NOT touched.** The doc's blast-radius bullet says its `has_terminal() { [ -t 0 ] && ... }` gate at `otto-dev/scripts/lib.sh:142` now evaluates false under otto, so `bootstrap.sh` and `init_dev.sh` take their noninteractive paths and the docker-start `sudo` fails loudly rather than hanging when its timestamp has expired. The doc calls marking those tasks `tty: true` "their call, not this doc's". Nothing here verified that repo; someone should confirm before this ships.
- **`tests/sigint_cancel_test.rs` now covers four signals and two paths under a name that says SIGINT.** Renaming it to something like `stop_signal_test.rs` is a rename this phase did not take. Worth doing in a later phase or not at all.
