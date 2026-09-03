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

## Phase 2: Executor data passing and buffered replay

### Design decisions
- **`is_identifier` lives in `naming.rs` as `pub(crate)`, exactly as the doc specifies** — `src/naming.rs:is_identifier` — Phase 4 reuses it for `foreach.as`, so the signature is fixed now. The two copies it replaces are gone: `cfg/env.rs`'s `is_valid_env_key` was deleted and its one call site rewritten, `executor/action.rs`'s `validate_identifier` kept its error message (which is the caller's context: kind, task name, name) and now asks `naming::is_identifier` for the verdict.
- **The per-byte fold is a separate function with its own name, `fold_to_var_name`** — `src/executor/scheduler.rs:fold_to_var_name` — panel round 1 finding S1. Its doc comment says in so many words why it is not `is_identifier`: the whole-name rule rejects a leading digit, and applying it byte by byte would fold `OTTO_INPUT_UP_2024` to `OTTO_INPUT_UP____`. `naming_tests.rs:is_identifier_is_a_whole_name_rule_not_a_per_byte_class` asserts the distinction rather than leaving it to the comment.
- **The fold is per BYTE, not per `char`** — `src/executor/scheduler.rs:fold_to_var_name` — the reader is `LC_ALL=C tr` in `builtins.sh`, which folds bytes. `é` is two bytes, so `tr` emits two underscores; a `chars()` fold would emit one, the prefixes would differ, and the reader would match nothing at exit 0 — the same silent-empty failure the `-`-only fold had. Uppercasing is `to_ascii_uppercase` for the same reason (`ß` uppercases to `SS` in Rust and to itself under `LC_ALL=C tr`).
- **The bash side complements the class instead of enumerating it** — `src/executor/action.rs`, `otto_deserialize_input` — `LC_ALL=C tr -c '[:alnum:]_' '_'` replaces the two `${task_upper//x/_}` expansions. Enumerating was the bug: `:` was simply not on the list. `tr -c` is POSIX and bash-3.2-safe, so the macOS constraint the surrounding comment guards is unaffected.
- **`suppress_terminal` is computed in `try_start_ready_task`** — `src/executor/scheduler/support.rs` — see Deviations; it is the frame that already holds `&mut ReplayCursor` and is the sole caller of `execute_task` (both arms).
- **The parity test now pins itself to the shell that ships** — `src/executor/action_tests.rs:builtins_input_fold_matches_the_writer_fold` — it holds the fold pipeline as a `SHELL_FOLD` const, asserts the generated `builtins.sh` still contains that exact text, and only then runs it against `fold_to_var_name`. Before, the test held a private copy of the shell fold with nothing tying it to the real one, so reverting `builtins.sh` alone could have left the test green.

### Deviations
- **The doc says compute `suppress_terminal` "in `execute_all` where the cursor is built"; it is computed in `try_start_ready_task`.** Same effect, correct seam: `execute_all` builds the cursor (`scheduler.rs:1445`) and hands it down, but it never calls `execute_task` — `try_start_ready_task` is the only caller, in both its `Ok(true)` and its `Err(_)` arm, and it already takes `cursor: &mut ReplayCursor`. Computing it in `execute_all` would mean computing it for a task that has not been selected to start yet.
- **The doc's Rust spelling is `c.is_ascii_alphanumeric() || c == '_'` over chars; the implementation is the same predicate over bytes.** Reason under Design decisions: a char-wise fold does not agree with `LC_ALL=C tr` for multibyte input, and byte-exact agreement with the reader is the whole requirement.
- **The doc lists the parity fix as two call sites; a third file changed.** `src/executor/action_tests.rs` held a hand-copied duplicate of both folds and pinned the old rule, so it would have gone red on the correct behavior. Inverted by name rather than deleted: it still tests fold parity, now against the real code on both sides, with `up:alpha` and `up_2024` added as cases.
- **Two tests were added to `tests/foreach_buffer_test.rs`, not one.** The doc asks for "requesting one item of a buffered foreach prints its output". `test_asking_for_the_parent_still_buffers_the_whole_group` is its complement: without it, deleting suppression outright would also pass.

### Tradeoffs
- **`fold_to_var_name` lives in `scheduler.rs` beside its only caller, not in `naming.rs` beside `is_identifier`** vs putting both name rules in one module — the doc is explicit that the two must not be merged, and `naming.rs` is about *subtask* names, while this is the writer's half of a wire format shared with `builtins.sh`. Keeping it next to `json_to_env` keeps the two halves of that format one screen apart. It is `pub(crate)` only so the parity test can call the real function instead of a copy.
- **The parity test asserts `builtins.sh` contains `SHELL_FOLD` verbatim, indentation and all** vs extracting the pipeline by parsing the file — a literal-text assertion is brittle to reindentation, but it fails loudly with a message that says exactly what to do, whereas a parser would silently test a fragment it no longer found. The `#[tokio::test] #[serial]` cost of generating a real `builtins.sh` is the price of not testing a copy.
- **`tests/subtask_output_test.rs` is a new file rather than a case in `tests/foreach_aggregation_test.rs`** (the doc allows either) — the bug is a foreach subtask *feeding a consumer*, which is the input/output surface, not the aggregation surface. It carries a second test on the generated `.env` file itself, so the fold is pinned at the file as well as at the consumer.

### Break-the-test proofs
Per the doc's Testing Strategy. Each fix reverted alone, the test run, the fix restored.

- **Revert `fold_to_var_name` in `json_to_env` to `to_uppercase().replace(['-', '.'], "_")`.** Both tests in `tests/subtask_output_test.rs` FAILED, with the doc's `Observed:` line verbatim: `input.up:alpha.env: line 4: OTTO_INPUT_UP:ALPHA_K=v-alpha: command not found`, task `use` exiting 127.
- **Revert `suppress_terminal` to `self.tui_mode || task.buffered`.** `test_requesting_one_item_of_a_buffered_foreach_prints_its_output` FAILED with the captured stdout being exactly `[say:alpha] finished successfully` and nothing else — again the doc's `Observed:` line. The other 13 tests in the binary passed.
- **Revert the `builtins.sh` fold to the two `${task_upper//x/_}` expansions.** `builtins_input_fold_matches_the_writer_fold` FAILED on the drift guard: `the shell fold this test runs is no longer the one builtins.sh ships`. Reverting the test's `SHELL_FOLD` copy to match (i.e. reverting both shell copies together) then FAILED on the comparison itself: `reader and writer folds disagree for task name "up:alpha"`, left `OTTO_INPUT_UP:ALPHA_`, right `OTTO_INPUT_UP_ALPHA_`.

### Success criteria, as run
- `otto use` on the `up:alpha` fixture exits 0 and prints `got=[v-alpha]`: observed `[up:alpha] finished successfully` / `[use] got=[v-alpha]` / `[use] finished successfully`, `EXIT=0`. PASS.
- `otto say:alpha` on the buffered-foreach fixture prints `HELLO alpha`: observed `[say:alpha] HELLO alpha` / `[say:alpha] finished successfully`, `EXIT=0`. PASS.
- `grep -c '                      {base}' src/executor/scheduler.rs` -> `0`. PASS.
- `otto ci`: `✅ All CI checks passed!`, coverage 93.1% lines against an 87% threshold.

### Open questions
- None.

## Phase 3: cfg env evaluator

### Design decisions
- **The deferral check runs on the raw value, before `evaluate_single_env_value`** — `src/cfg/env.rs:evaluate_envs` — `referenced_vars(raw_value)` scans `$(...)` bodies under the same rules as the expander, so a declared sibling named inside a command body is visible before anything executes. The old order executed first and let the empty read stand as the answer, which is why the result depended on which key the sorted pass reached first.
- **Only a DECLARED name defers; the key's own name never does** — same loop — the predicate is `name != var_name && envs.contains_key(name) && !evaluated.contains_key(name)`. An inherited name resolves inside the command from the inherited environment (the `$(echo "${SOME_PROFILE:-default}")` idiom), so it must not wait; a self-reference reads the inherited seed `evaluation_context` plants, which is the only value it will ever get, so deferring on it would spin to the no-progress branch and be misreported as a cycle.
- **An evaluation error is terminal, not "wait"** — same loop, the `Err` arm — once the check above has passed, every declared reference the value makes is resolved, so the only remaining errors are a command that exited non-zero and a reference to a name that is neither declared nor inherited. Both return immediately, with the message the partial-resolution fallback already used (`Failed to resolve environment variable '<key>': <cause>`), so no pinned string changed.
- **The comment on the command environment now describes the environment the command gets** — `src/cfg/env.rs:execute_shell_command_with_env` — `env_clear()` plus seven essentials is immediately overwritten by `cmd.envs(env_overrides)`, which is the whole inherited environment minus the declared keys plus the values resolved so far. The essential list is a floor for exactly one case: when one of those seven names is itself a declared key, and is therefore stripped from the context until it resolves.

### Deviations
- **The comment fix also touched the neighbouring claim in `evaluate_single_env_value`.** Its parenthetical called the same environment "the controlled command environment [that] exists to prevent" a parent-env leak, which is the identical false statement one screen up; the doc bullet only names `:440-452`. Rewritten to say what actually makes the leak reachable (the context this resolves against carries the inherited environment) and the same sentence in the test's doc comment (`src/cfg/env_tests.rs:command_output_is_not_rescanned_for_variable_references`) with it. Behavior unchanged.
- **The failing-command tests count executions with `$$`, not `$RANDOM`.** `/bin/sh` here is dash, where `$RANDOM` expands to the empty string, so every touch would land on one filename and the count could not distinguish one execution from three. `$$` is the shell's PID, unique per `sh -c`, so the marker count *is* the execution count. The doc's literal `$RANDOM` shape was also run against the binary (see below); it passes, it just cannot fail.
- **`a_command_referencing_an_earlier_sibling_still_resolves` and `a_command_referencing_an_inherited_name_resolves_from_the_environment` are green before and after by design.** The first is the declaration order that already worked by luck (the doc asks for both orders, so both are pinned); the second is the `otto-dev` blast-radius pin, whose whole point is that the new rule does *not* change it.

### Tradeoffs
- **The now-unreachable partial-resolution fallback (`env.rs`, after the cycle check) was left in place** vs replacing it with a hard error — every entry in `still_pending` now got there through the deferral rule, so each has a reference to another unresolved declared key, so `find_reference_cycle` cannot return `None` and the fallback cannot run. Deleting it is a behavior claim about a branch nothing exercises; it stays as the fail-closed backstop the comment above `max_iterations` already describes, and the phase does not widen to it.
- **`find_reference_cycle` still follows a self-edge when a value names both itself and a pending sibling** (`A: "$A-$B"`, `B: "$A"` reports the path `A -> A` rather than `A -> B -> A`) vs skipping self-edges there — the verdict is right (it is a real deadlock and it is reported as a cycle) and only the path is imprecise. Out of this phase's bullets; a self-reference alone no longer reaches that branch at all, because it evaluates instead of deferring.

### Break-the-test proofs
Per the doc's Testing Strategy. The deferral check and the `Err` arm reverted together to the `f9882ed` shape, the tests run, the fix restored.

- `a_command_referencing_a_later_sibling_waits_for_it` FAILED: `left: Some("got:")`, `right: Some("got:hello")` — the doc's `Observed:` line.
- `a_self_reference_inside_a_command_reads_the_inherited_value_and_terminates` FAILED: `left: Some("from-shell-")`, `right: Some("from-shell-sib")`.
- `two_keys_referencing_each_other_inside_commands_report_a_cycle` FAILED: no error at all; both commands ran with the other key stripped and the load succeeded with two empty values.
- `a_failing_command_runs_exactly_once` FAILED: `left: 3`, `right: 1` marker files (once per pass, plus once in the partial-resolution fallback).
- `a_command_beside_a_later_reference_runs_exactly_once` FAILED: `left: 2`, `right: 1` marker files.
- The reverted source was also built and run as a binary: the acceptance fixture printed `[show] A=[got:] B=[hello]` and the `$(touch $MARK.$$; false)` fixture left 3 marker files, both matching the doc's `Observed on main`.

### Success criteria, as run
- **The fixture prints `A=[got:hello]` in both declaration orders.** `A: '$(echo "got:$B")'`, `B: hello` -> `[show] A=[got:hello] B=[hello]`, rc 0. Names swapped (`B: '$(echo "got:$A")'`, `A: hello`) -> `[show] A=[hello] B=[got:hello]`, rc 0. PASS.
- **A `$(touch $MARK.$RANDOM; false)` value leaves exactly one marker file after a failed load.** Load failed with `Failed to evaluate global environment variables: Failed to resolve environment variable 'A_BAD': Command 'touch $MARK.$RANDOM; false' failed with exit code 1`, rc 1, one marker file (`mark.`). Repeated with `$$` for a per-execution-unique name: one marker file (`mark.1900199`), against 3 from the reverted build. PASS.
- `otto ci`: `✅ All CI checks passed!` — lint, fmt-check, check, compile, clippy, test, cov, cov-report all `finished successfully`; coverage 93.1% lines against an 87% threshold.

### Open questions
- None.
