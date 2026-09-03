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

## Phase 4: cfg load-time validation and schema truth

### Design decisions
- **All four `foreach:` load checks are reached through one `ForeachSpec::validate(task_name)`** — `src/cfg/task.rs:ForeachSpec::validate`, called from `src/cli/parser/config.rs:validate_foreach_specs` — the parser's load path should have one foreach seam, not three; `validate_sources`, `validate_var_name` and `validate_range` stay separate functions (and `validate_sources` stays public, because `resolve_command_items` calls it on its own).
- **The source counter reports what it found, not what it forbade** — `src/cfg/task.rs:validate_sources` — `Task 'multi': foreach declares glob and items; foreach takes exactly one source (command, glob, items, or range)`. Zero sources gets its own arm (`foreach declares no source`), which is the `foreach: {}` case that used to load and fail at expansion.
- **The range is counted at load through the same parse the expansion uses** — `src/cfg/task.rs:parse_range`, called by both `validate_range` and `resolve_range` — the doc's bullet put the checked arithmetic in `resolve_range`, but `resolve_range` is not on the load path (the only `--help` caller, `cli/parser/command.rs:23`, swallows its error with `map_or(0, ..)`), so a fix there alone could not satisfy the "fails at load" criterion. `resolve_range` keeps its own copy of the guard for the run path.
- **The overflow arm names `max_items` too** — `src/cfg/task.rs:validate_range` — `checked_add` is the only thing that separates `0-18446744073709551615` from `0-18446744073709551614`, and an author who hits either wants the same sentence: the limit they blew past.
- **`Nargs::Range` stores the counts the ottofile wrote** — `src/cfg/param.rs` `Deserialize`/`Serialize`/`Display`, `src/cli/parser.rs:nargs_to_num_args` — `"3"` is `Range(3, 3)` and `"2:5"` is `Range(2, 5)`. The offset `min` existed only so the serializer could add it back; three readers had to know about it and one of them (the reference page) got it wrong.
- **`otto.jobs` is `Option<usize>` with `skip_serializing_if = "Option::is_none"`** — `src/cfg/otto.rs:OttoSpec` — `default_jobs()` and `is_default_jobs()` are gone; the CPU-count default lives in `cli/parser.rs` (`DEFAULT_JOBS`, clap's default) and the ottofile value now only overrides it when present (`cli/parser.rs`, the `!jobs_explicit` branch). `deserialize_jobs` keeps rejecting `Some(0)`, unchanged in behavior.
- **The three new edge visitors stringify through `EdgeSpec::sugar`** — `src/cfg/edge.rs:EdgeVisitor` — `visit_u64`/`visit_i64`/`visit_bool` produce `from_sugar: true`, so `after: [2024]` re-emits as the bare `2024` it was written as rather than gaining a `{task, when}` map.

### Deviations
- **The load-time range check lives in `validate_range`, not only in `resolve_range` (same effect, correct seam).** Reason above; both call `parse_range`, so there is one parser and two callers, not two copies.
- **The private-repo citation was dropped from all six sites in `src/cfg/`, not just `config.rs:12`.** `otto.rs` x2, `task.rs` x2, `param.rs` and `config.rs` all carried the same `Per borg/src/config.rs:281-285` sentence; the doc's stated reason (a private repo path in a public tree) is a property of the string, not of the one file it happened to name. The rule each one states is kept verbatim.
- **`ottofile-reference.md` gained three sentences beyond the `nargs` row the bullet names**: the `foreach` section now states the exactly-one-source rule, the `as` row states the identifier rule, and the `range` row states that the count is checked at load. All three are load errors this phase introduced; leaving them undocumented would have made the page wrong in the same way the `nargs` row was.
- **`tests/config_validation_test.rs` is a new integration file** rather than additions to an existing one: "at load" is the claim under test, and the existing foreach files are organized by feature (`foreach_command_test.rs`, `foreach_jobs_test.rs`), not by lifecycle stage.
- **Two tests were inverted rather than deleted.** `test_foreach_static_sources_still_validate_clean` (`src/cfg/task_tests.rs`) pinned the old "only a `command:` source is exclusive" behavior by name and is now `two_static_foreach_sources_are_rejected_naming_both`, with `exactly_one_static_foreach_source_validates_clean` keeping the positive case. `command_combined_with_a_static_source_is_a_config_error` (`tests/foreach_command_test.rs`) asserted the old message fragment `cannot be combined with items` and now asserts `declares command and items`.
- **`Nargs::Range(0, 3)` in `test_nargs_roundtrip_all_variants` became `Range(3, 3)`.** `Range(0, N)` is no longer a value any ottofile can produce: it serialized to `"0:3"`, which the deserializer rejects (`min must be at least 1`), so the case was asserting a round-trip through a spelling the schema does not accept.

### Tradeoffs
- **`foreach.items` did NOT get a `skip_serializing_if`** vs adding `Vec::is_empty` alongside the other seven — the doc lists the eight fields by name and `items` is not among them; an explicitly written `items: []` is also the one empty source that says something (it is now a load error, and dropping the key on re-emit would change which error the author sees). The minimal-foreach round-trip test passes without it.
- **`resolve_range` keeps its own count guard** vs relying on the load-time check — `resolve_items` is reachable from `TaskSpec::expand_foreach`, which does not go through the parser, so removing it would leave one caller unguarded for the sake of not repeating four lines.
- **`DynamicResolver::has_foreach`/`has_choices` are `#[cfg(test)]`, not deleted** — the doc allows either; they are the only way the memoization tests can distinguish "cached" from "resolved again cheaply", and the production surface they would otherwise need (`foreach_items` with a counting closure) is what those tests already do one level up.

### Break-the-test proofs
Per the doc's Testing Strategy. Each fix reverted to its `f9882ed` shape, the tests run, the fix restored; `cargo test --lib` green again afterwards (887 passed).

- Source counting reverted to the `command`-only early return: `two_static_foreach_sources_are_rejected_naming_both` FAILED and `a_glob_and_items_foreach_fails_at_load_naming_both_sources` FAILED (the config loads).
- `validate_var_name` call removed: `a_non_identifier_foreach_as_is_rejected_naming_the_field` FAILED and `a_non_identifier_foreach_as_fails_at_load_naming_the_field` FAILED.
- `validate_range` call and the `resolve_range` pre-count removed: `a_range_wider_than_max_items_is_rejected_at_validation`, `a_range_spanning_the_whole_usize_space_is_rejected_without_counting` and `a_range_spanning_the_usize_space_fails_at_load_without_allocating` all FAILED.
- `visit_u64`/`visit_i64`/`visit_bool` removed: 3 unit tests FAILED (`a_numeric_edge_target_deserializes_as_the_stringified_name`, `a_negative_...`, `a_boolean_...`) and 2 integration tests FAILED (`a_numeric_edge_target_loads_and_runs_the_numerically_named_task`, `a_boolean_...`).
- `Nargs` reverted to the offset `min`: `a_bare_nargs_count_means_exactly_that_many` FAILED and `a_bare_nargs_count_round_trips_byte_identical` FAILED. `an_nargs_span_re_emits_the_counts_that_were_written` stayed green, as expected: `"2:5"` round-tripped under the old offset too, because the serializer added the one back. It is a companion assertion, not a regression pin.
- The eight `ForeachSpec` `skip_serializing_if`s removed: `a_minimal_foreach_round_trips_without_gaining_keys` FAILED, emitting `glob: null`, `range: null`, `command: null`, `as: item`, `parallel: true`, `max_items: 1000`, `buffer: false`.
- `otto.jobs`' skip predicate changed back to "equals the host CPU count": `jobs_equal_to_the_host_cpu_count_survives_the_round_trip` FAILED (`jobs: Some(32)` in, `jobs: None` back). This is the 4-core-host bug the doc describes, reproduced on a 32-core host by pinning the test to the host's own count.

### Success criteria, as run
- **`cargo doc --no-deps` emits zero warnings.** `Documenting otto v2.2.1 ... Finished dev profile ... Generated target/doc/otto/index.html`, no warning lines. Both private-item intra-doc links (`env.rs`'s `evaluation_context`, `otto.rs`'s `ApiHeader`) are now plain backticks. PASS.
- **The `glob`+`items` fixture fails at load naming both sources.** `otto -o <fixture> --help` -> rc 2, `ERROR: failed to parse ottofile: ... Task 'multi': foreach declares glob and items; foreach takes exactly one source (command, glob, items, or range)`, and no `[N items]` count is rendered. PASS.
- **`foreach: {items: [a], as: "my item"}` fails at load naming `foreach.as`.** rc 2, `Task 'up': foreach.as 'my item' is not a valid identifier (letters, digits and underscore only, not starting with a digit); it becomes a shell variable in every subtask`. PASS.
- **`after: [2024]` loads and runs the `2024` task.** `otto -o <fixture> report` -> rc 0, `[report] ran-report` / `[2024] ran-2024`, byte-for-byte the same output as the `after: ["alpha"]` control fixture. PASS.
- **`range: "0-18446744073709551615"` fails at load in under one second.** rc 2, `Task 'huge': foreach range '0-18446744073709551615' spans more items than this platform can count, far exceeding max_items (1000); narrow the range`, `0.009 total` under `time`. PASS.
- **`nargs: "3"` round-trips byte-identical.** `a_bare_nargs_count_round_trips_byte_identical`: in and out are both `nargs: '3'`. PASS.
- **`cargo test roundtrip` passes with `jobs` explicitly set to a value other than the host's CPU count.** 21 passed, 0 failed. `config_otto_only_roundtrips` writes `jobs: 4`; `nproc` here is 32. The companion `jobs_equal_to_the_host_cpu_count_survives_the_round_trip` covers the equal case that used to pass for the wrong reason. PASS.
- **`grep -n 'Value::Dict\|pub type Values' src/cfg/param.rs`** -> no output. PASS.
- **`grep -n 'fn has_foreach\|fn has_choices' src/cfg/resolver.rs`** -> `115: pub fn has_foreach(&self, task: &str) -> bool {` and `141: pub fn has_choices(&self, key: &str) -> bool {`, both immediately under `#[cfg(test)]` (`:113` and `:139`). Nothing outside `#[cfg(test)]`, which is the criterion. PASS.
- **`grep -n 'pub fn new(' src/cfg/task.rs`** -> no output; `TaskSpec::new` is gone and its 14 call sites (12 in `src/cfg/task_tests.rs`, 2 in `src/cli/parser_tests_a.rs`) are struct literals with `..Default::default()`. `DynamicResolver::new` (`resolver.rs`) and `TaskSpec::has_foreach` (`task.rs`) are untouched, per panel round 2's S2. PASS.
- `otto ci`: `✅ All CI checks passed!` — lint, fmt-check, check, compile, clippy, test, cov, cov-report all `finished successfully`; coverage 93.2% lines (23387/25092) against an 87% threshold.

### Open questions
- **`after:` is documented backwards.** `docs/commands/ottofile-reference.md:90` says "Tasks this one runs after", but the scheduler treats the edge the other way: with `report: {after: [alpha]}`, `report` runs FIRST and `alpha` is reported as depending on it (`[alpha] skipped (dep report failed; ...)` when `report` fails). Found while verifying the numeric-edge criterion, which is unaffected (the numeric and string forms behave identically). Not this phase's bullet, and the fix is either a one-line doc correction or a scheduler change, so it needs the author's call. Phases 16 and 17 are the doc phases if it is the former.

## Phase 5: CLI builtin routing

### Design decisions
- **A mixed builtin/user task list is rejected in one pure function, not in the dispatcher** — `src/cli/parser.rs:reject_mixed_task_list`, called from `Parser::parse` right before `process_tasks_with_filter` — the doc puts the fix in `parse` because that is where both task-source paths (explicit args and `otto.tasks` defaults) have converged into one resolved name list, so one call covers both. The function partitions and names every offender on each side, which is what makes the message say `'Clean'` *and* `'build'` rather than one of them.
- **`filter_execution_tasks` deleted rather than kept as a no-op** — `src/app.rs` — with `parse` rejecting the mixed shape, the only task lists that reach `execute_with_terminal_output`/`execute_with_tui` are all-builtin (dispatched by `find_builtin` above it) or all-user, so the filter could only ever be an identity function and the "No tasks to execute" check behind it could only ever be false. Both call sites now pass `tasks` straight into `build_executor_tasks`.
- **Builtin task names are reserved at load, next to the param check** — `src/cli/parser/config.rs:validate_no_builtin_tasks`, called from `load_config_from_path` immediately before `validate_no_builtin_params` — same shape and same wording as the param rejection ("Capitalized names are reserved for otto builtins"), because it is the same class of mistake. Load time, so every surface that reads the ottofile reports it, including `--help` and `--tasks`.
- **`-h` and `-t` interception is gated on a shared "does a task claim this short?" query** — `src/cli/parser/help.rs:args_claim_short`, used by `help_requested_in` (`-h`) and by `Parser::parse`'s `take_tui_flag` call (`-t`) — every single-letter token otto intercepts is a name taken away from the ottofile author, so both interceptions ask the same question in the same way: a task in the arg list that declares the short owns it. Reads declarations only; resolves nothing and runs nothing, so it is safe on the help path.
- **`has_user_tasks()` replaces four `tasks.is_empty()` checks** — `src/cli/parser/help.rs:has_user_tasks`, used by `should_show_help` and `build_help_command` — `inject_builtin_commands` runs before every help decision, so the task map always holds at least the six builtins and `is_empty()` was a question with a constant answer.
- **`EarlyCommand` in `main.rs` makes the "all builtins except Graph" set a type instead of a comment** — `src/main.rs` — `handle_subcommand` matched five string literals and a comment claimed `Graph`'s absence was deliberate. A five-variant enum with a pure `from_name` lets a test assert the difference against `Builtin::all()` without executing any builtin (dispatch stays an exhaustive match, so a new variant still fails to compile until it is handled).
- **The `builtins.rs` checklist now names the five real lists** — `src/cli/builtins.rs` — it pointed at `inject_NAME_meta_task() in parser.rs` (that file is now `parser/meta_tasks.rs`), omitted the `Builtin` enum and the `main.rs` route entirely, and told the reader to "add an execution filter", which is exactly the mechanism this phase deleted.

### Deviations
- **The `-h` bullet also needed a change in `task_to_command`, not just in the help gate** — `src/cli/parser/command.rs` — with only the help-gate fix, a task declaring `-h|--host` still could not be given a host: clap asserts on the duplicate short outright (`Command build: Short option names must be unique for each argument, but '-h' is in use by both 'host' and 'help'`), which panicked the binary on *every* invocation of that task, `--host` included. Reproduced on `f9882ed` before touching anything. The task's command now disables clap's auto help flag and re-adds a long-only `--help` when a param claims `-h`, so `otto build -h example.com` binds and `otto build --help` still renders help. Same effect the bullet asks for, at the seam that makes it reachable.
- **`-t` is stripped only when no task in the arg list declares `-t` itself** — `src/cli/parser.rs:take_tui_flag` now takes a `take_short` argument. The bullet says "strip `-t` as well as `--tui`" unconditionally; done unconditionally it would create the exact bug the `-h` bullet two lines below it exists to fix, and would break `otto History -t <task>`, whose meta task declares `-t/--task` today (the doc's own Phase 6 text says so). `--tui` is stripped unconditionally, as before: a task cannot declare a long that collides with a global.
- **The "no ottofile found" epilogue is now gated on `self.ottofile.is_none()`, not on the task map being empty** — `src/cli/parser/command.rs:build_help_command` — swapping `tasks.is_empty()` for `has_user_tasks()` as written would have printed "ERROR: No ottofile found in this directory or any parent directory!" for an ottofile that exists and declares no tasks of its own, which is the case the bullet is about. The task/builtin subcommand loops are now unconditional (they iterate an empty map harmlessly) and only the epilogue is conditional.
- **The mixed-list guard does not reject two builtins named together** — `otto Clean Stats` still runs `Clean` and drops `Stats`, because the doc's rule is specifically "a builtin and a user task". Left as specified; listed under open questions.

### Tradeoffs
- **Rejecting a mixed list in `parse` vs making `find_builtin` return an error** — the error belongs where the user's words are still visible: `parse` holds the resolved *name* list, so the message quotes what was typed. `find_builtin` sees `Vec<Task>` after DAG construction, and it is also the wrong layer to fail from, since both output modes call it after they have already printed nothing.
- **Applying the mixed-list guard to `otto.tasks` defaults too** vs only to explicitly typed args — one call site, one rule, and an ottofile whose defaults are `[Clean, build]` is unrunnable for the same reason `otto Clean build` is. Cost: that ottofile now fails at run rather than silently running `Clean`. `*` expansion already excludes builtins, so the common shape is unaffected.
- **A separate `EarlyCommand` enum in `main.rs`** vs a method on `otto::app::Builtin` — the early-route set is deliberately not the builtin set, and a `Builtin::early_route()` returning `Option` would put "except Graph" back into a comment inside the library, away from the `main.rs` match it constrains.
- **`args_claim_short` scans the arg list for known task names** vs partitioning first — partitioning needs `get_task_names`, which can resolve a command-sourced `foreach`, i.e. run user code. The help path must execute nothing, so this reads the params map directly and accepts being coarse: a task declaring `-t` anywhere in the invocation suppresses otto's `-t` for the whole invocation.
- **`disable_help_flag` + a long-only `--help`** vs `mut_arg`-ing clap's help short away — `disable_help_flag` is the API clap's own assertion message recommends, and it is skipped entirely when a param already declares a long `--help`, so the re-add can never itself collide.

### Break-the-test proofs
Per the doc's Testing Strategy. Each fix reverted to its `f9882ed` shape one at a time, the test run, the fix restored; all 8 tests in `tests/builtin_routing_test.rs` green again afterwards and `otto ci` rc 0.

- `reject_mixed_task_list` call removed from `Parser::parse`: `a_builtin_named_with_a_user_task_fails_naming_both` FAILED (exit 0, `Clean` ran).
- `validate_no_builtin_tasks` call removed from `load_config_from_path`: `a_task_named_like_a_builtin_fails_at_load` FAILED (`--tasks` succeeded on an ottofile whose only task can never run).
- `take_tui_flag` reverted to `arg == "--tui"` only: `the_tui_short_flag_reaches_the_tui` FAILED (`unexpected argument '-t'`).
- `discovery.rs` reverted to `eprintln!("Warning: Default task ... not found")`: `an_unknown_default_task_fails_with_a_suggestion` FAILED (exit 0).
- The `-h` fix reverted in two independent halves, each FAILING `a_task_can_declare_the_h_short_for_itself` on its own: (a) `task_to_command`'s `disable_help_flag` branch disabled — clap's duplicate-short assertion panics; (b) `help_requested_in` reverted to an unconditional `-h` check — otto answers with task help instead of binding the host.
- `should_show_help` reverted to `self.config_spec.tasks.is_empty()`: `an_ottofile_with_no_user_tasks_prints_help` FAILED (`No tasks to execute`).
- The `--help` path's `temp_parser` reverted to `ottofile: None`: `help_from_a_subdirectory_counts_foreach_items_from_the_ottofile_dir` FAILED (no `[3 items]`; the glob resolved against the cwd).

### Success criteria, as run
Debug binary at `target/debug/otto`, `OTTO_HOME` pinned to a temp dir, run in a fresh temp project.

- **`otto build Clean` exits non-zero and the message names both `build` and `Clean`.** `otto build Clean --dry-run` -> rc 1, `cannot run builtin command(s) 'Clean' together with task(s) 'build': a builtin command runs on its own`, and nothing on stdout (the builtin did not run on the way to the error). Observed on `f9882ed`: rc 0, `Querying database for old runs... / No runs matching deletion criteria found`, `build` never mentioned. PASS.
- **A task named `Clean` fails at load.** `otto --tasks` on `tasks: {Clean: {action: echo USER-CLEAN-RAN}}` -> rc 1, `Task 'Clean' defines reserved builtin command name 'Clean'. Capitalized names are reserved for otto builtins.` Observed on `f9882ed`: `otto Clean --dry-run` ran the builtin at rc 0. PASS.
- **`otto build -t` reaches the TUI (or fails only for lack of a tty).** Without a tty: rc 0, `Warning: --tui requires a TTY, falling back to standard output` then `[build] BUILD-RAN`. Under a pty (`script -qec "otto build -t"`): the alternate screen is entered (`ESC[?1049h` and the TUI's redraw stream), so it reaches the TUI itself, not just the flag parser. Observed on `f9882ed`: `error: unexpected argument '-t' found`. PASS.
- **Bare `otto` with `tasks: [bild]` exits non-zero naming `bild` and suggesting `build`.** rc 1, `unknown task 'bild'; did you mean 'build'?`. Observed on `f9882ed`: `Warning: Default task 'bild' not found / No tasks to execute`, rc 0. PASS.
- **`Builtin::all()` coverage test exists.** `every_builtin_but_graph_is_early_routed` (`src/main_tests.rs`) asserts `EarlyCommand::from_name` is `Some` for all five and `None` for `Graph`. PASS.
- **Blast radius: the new binary in `~/repos/tatari-tv/otto-dev`.** `otto --help` -> rc 0, full command list rendered (`auth-bypass` ... `fe-up` ...), empty stderr. `otto --tasks` -> rc 0, valid JSON on stdout, empty stderr. Verified by reading its `.otto.yml`: `otto.tasks` is `[]` (so no default-task resolution to break), zero tasks named after a builtin, and zero tasks declaring a `-h` or `-t` short. Nothing under `otto-dev` was modified. PASS.
- `otto ci`: rc 0, `✅ All CI checks passed!` — lint, compile, clippy, fmt-check, check, test, cov, cov-report all `finished successfully`; 93 `test result: ok` lines, zero `FAILED`; coverage 93.2% lines (23474/25176) against the 87% threshold.

### Open questions
- **Two builtins named together still silently drops all but the first.** `otto Clean Stats` runs `Clean` at rc 0 and says nothing about `Stats`, because `find_builtin` returns on the first match and this phase's guard only covers "a builtin plus a user task", which is what the doc specifies. The same one-line partition in `reject_mixed_task_list` could reject it; not done, because it is beyond the bullet.
- **`otto Clean` on an ottofile that declares a task named `Clean` still runs the builtin without complaint.** `main`'s early route reaches `CleanCommand`'s own clap parser before any ottofile is read, so the new load-time rejection is not consulted on that one invocation. Every other surface (`otto`, `otto --help`, `otto --tasks`, `otto <any-task>`) rejects the file. Closing it would mean loading the ottofile before early routing, which would give `otto Clean` an ottofile requirement it deliberately does not have.

## Phase 6: CLI meta tasks from clap, and dead CLI code

### Design decisions
- `GraphFormatArg` is a new CLI-side `clap::ValueEnum` in `src/cli/commands/graph.rs` rather than a `ValueEnum` derive on the executor's `GraphFormat` — `GraphFormat` also carries `Auto`, which is not a format a user can ask for, so deriving there would advertise a sixth choice that means nothing on the command line. Mirrors the existing `StatusFilter` -> `RunStatus` arrangement in `history.rs`.
- `GraphCommand` gets the derive but no early route in `main` — `Graph` needs the parsed ottofile's task specs, so it stays task-route only, as the doc specifies. The derive exists solely as the single declaration the meta task is built from.
- The parity test lives in `src/cli/parser_tests_b.rs` as `every_builtin_meta_task_matches_its_clap_command`, alongside three companions: `the_derived_meta_tasks_are_exactly_the_reserved_builtins`, `the_graph_meta_task_defaults_to_ascii_with_the_five_declared_formats`, and `a_builtin_meta_task_carries_no_executable_action`.
- The three task-route extractors' silent fallbacks became `expect`s naming the derivation (`src/app.rs:48,82,104`), per the doc.

### Deviations
- Phase 6's implementing agent wedged after completing the code, tests, break-the-test proofs, and a full CI run, but before appending these notes or committing. The orchestrator verified every success criterion independently, re-ran `otto ci` to green on the same tree, wrote this section, and made the commit. No code was changed after the agent's last edit.
- `src/executor/graph.rs` shrank by 41 lines: `DagVisualizer::render_ottofile_graph` and `execute_command` were the string-parsing entry points the task-route extractor called with `unwrap_or("ascii")`. With typed values arriving from the derive, the callers construct `GraphOptions` directly.
- Four `cli/commands/*.rs` files changed by one line each (`clean.rs`, `convert.rs`, `history.rs`, `stats.rs`) — help-text wording that the derivation now surfaces in the meta task as well, so the two help surfaces agree verbatim.
- `src/app_tests.rs` churned heavily (295 lines) because `CleanParams`/`HistoryParams`/`StatsParams` gained the fields the meta tasks previously omitted (`keep_last`, `keep_failed`, `no_db`, `backup_dir`, `github_token`), which removed their `Default` impls' usefulness in those tests.

### Tradeoffs
- Derivation over a parity test alone: the doc's Alternative 5 rejected keeping `meta_tasks.rs` plus a parity test, because a test detects drift after the fact. `meta_tasks.rs` dropped from 715 lines of changes to a mapper. The parity test is kept anyway as verification that the mapper is faithful, not as the drift guard.
- `GraphFormatArg` duplicates five variant names that `GraphFormat` already has. The alternative (deriving on `GraphFormat`) leaks `Auto` into the CLI surface. Five names is the cheaper cost.

### Open questions
- None from the implementation. Two carried from Phase 5 remain open for the author: whether `otto Clean Stats` (two builtins together) should also be rejected, and that `main`'s early route runs a builtin before any ottofile read, so a file declaring a task named `Clean` is not rejected on that one invocation.

## Phase 7: Clean, History, and shared formatters

This phase arrived partly edited by a previous session plus the orchestrator:
`format.rs`/`format_tests.rs` created and wired into `stats.rs`, most of
`clean.rs`'s DB-path Err contract and `calculate_dir_size`'s symlink skip
already in place, `history.rs`'s `abbreviate_home` already correct. This
session finished the remaining gaps: a leftover unused-import warning, a
duplicate variable declaration, a non-existent dev-dependency in a test, and
stale tests calling deleted methods.

### Design decisions
- **`format_tests.rs` needed `#![cfg(test)]`** — `src/cli/commands/format_tests.rs:1` — without it the module (and its `use super::*`) compiled even outside `cfg(test)`, where `#[test]`-attributed functions are stripped, leaving the import genuinely unused in a non-test build. `history_tests.rs` and `clean_tests.rs` already carry this guard; `format_tests.rs` was the one file missing it.
- **The tilde test mutates `HOME` in place under `#[serial_test::serial]`, restoring it afterward** — `src/cli/commands/history_tests.rs:only_the_leading_home_prefix_becomes_a_tilde` — this is the pattern `clean_tests.rs` already uses for `OTTO_HOME` (`test_get_otto_home_honors_otto_home_env`, `test_execute_with_database_ignores_missing_otto_home`), and `serial_test` is already a dev-dependency; `temp_env` was not.
- **A new end-to-end test drives the DB-path refusal through the real `StateManager`, not `MemoryStateStore`** — `src/cli/commands/clean_tests.rs:a_db_path_clean_with_one_refused_directory_exits_non_zero` — `MemoryStateStore::delete_run` never refuses (it has no filesystem to protect), so verifying the phase's second success criterion end to end needed a real `StateManager` whose `delete_run` calls `ensure_deletable_under_root` on a symlinked run directory, the same defect `manager_tests.rs`'s `delete_run_never_deletes_through_a_symlinked_run_directory` covers one layer down.

### Deviations
- None beyond what the assigning message already flagged as unfinished. Everything closed matches the bullet it was assigned to.

### Tradeoffs
- **The stale `test_format_timestamp`/`test_format_size_*` tests in `clean_tests.rs` were deleted, not converted to call `format::format_size`/`format::format_timestamp` directly** — `format_tests.rs` already asserts the same input/output pairs (bytes-to-KB/MB/GB boundaries, the known-timestamp case) at the seam that owns them now; keeping a second copy in `clean_tests.rs` would be the exact duplication this phase exists to remove, just moved one file over.
- **The DB-path refusal test constructs its own `StateManager` inline** rather than factoring a shared `create_test_manager` helper with `manager_tests.rs` — the two files are in different modules (`cli::commands::clean` vs `executor::state::manager`) and `manager_tests.rs`'s helper is private to that file; duplicating four lines was cheaper than exporting a test-only constructor across a module boundary for one caller.

### Break-the-test proofs
- Reverted the DB-path `Err` return in `execute_with_database` back to always `Ok(())`: `a_db_path_clean_with_one_refused_directory_exits_non_zero` FAILED (`result.unwrap_err()` panicked on an `Ok`). Restored.
- Reverted `abbreviate_home` to `s.replace(&home, "~")`: `only_the_leading_home_prefix_becomes_a_tilde` FAILED (`~/proj~/x` instead of `~/proj/home/u/x`). Restored.

### Success criteria, as run
- **`grep -rn 'fn format_size\|fn format_duration\|fn format_timestamp' src/cli/commands/` prints exactly three hits, all in `format.rs`.** Confirmed: three hits, `format.rs:13`, `:26`, `:47`. PASS.
- **A DB-path `Clean` with one refused directory exits non-zero.** `a_db_path_clean_with_one_refused_directory_exits_non_zero`: a run recorded against a symlinked run directory, `Clean --no-db=false` (DB path) returns `Err` containing `"failed"`, and the symlink's target survives. PASS.
- **`otto History` renders `/home/u/proj/home/u/x` as `~/proj/home/u/x`.** `only_the_leading_home_prefix_becomes_a_tilde` asserts exactly this input/output pair plus the bare-home and non-home-prefixed cases. PASS.
- `otto ci`: `✅ All CI checks passed!` — lint, compile, clippy, fmt-check, check, test, cov, cov-report all `finished successfully`; coverage 93.2% lines (23150/24836) against the 87% threshold.

### Open questions
- None.

## Phase 8: Upgrade

### Design decisions
- **Rollback picks the newest backup whose version is strictly OLDER than the version running now** — `src/cli/commands/upgrade.rs:select_rollback_target`, a free function so it is unit-testable without a filesystem — the safety backup a rollback writes is always the newest file in the directory when the next rollback runs, so "newest wins" made two rollbacks undo each other and no run of rollbacks ever reached the version before last. See Deviations: "differs from `current_version()`", the doc's literal rule, does not achieve this.
- **A backup whose version is not a semver is skipped, not guessed at** — same function — `~/.otto/backups` (or `--backup-dir`) is a directory anything can write into, and a version that cannot be ordered cannot be shown to be older than this one. `list_backups_tolerates_adversarial_filenames` already establishes that the parser accepts odd names; this decides what selection does with them.
- **`current_version` joined `api_base` and `install_target` as an `#[arg(skip)]` field with a `#[cfg(test)]` writer** — `src/cli/commands/upgrade.rs:UpgradeCommand`, written only by `with_current_version` — a process's own version is `GIT_DESCRIBE`, fixed at build time, so "roll back twice" (two processes at two versions) cannot be expressed any other way. The field carries the same "no `env =`, no user-settable writer" doc as the other two.
- **The releases override became an API base, not a releases URL** — `api_base` plus `releases_url()`, `latest_release_url()`, `tagged_release_url()` — one string now yields all three endpoints, so a fixture server and production compose the same paths. This is also what makes the phase's grep criterion land on real code rather than a comment.
- **One `get_json<T>` behind `fetch_release` and `fetch_releases`** — same file — the status check and the `Authorization` header live in one place, so a 404 from `/releases/tags/vX` reports the same way as one from `/releases` instead of being parsed as a release.
- **`dry_run_steps` and `version_lines` return data; the printers print it** — same file — the phase's criteria are about what those two surfaces *say* (the asset name, the step numbers, which release is marked latest), and asserting on stdout in-process is not a thing this repo does. The numbering is now the position in a `Vec`, so `--no-backup` cannot reintroduce a gap.
- **`make_executable` is its own function and `stage_beside_with` injects it** — same file — the discard-on-failure path needs a chmod that fails on a file this process just created, which no test can arrange from the outside; the file already establishes the injected-parameter pattern with `verify_binary_within` and `download_with_progress_within`, both documented the same way.
- **`get_backup_dir` goes through `layout::resolve_otto_home()`** — same file — `$OTTO_HOME` moves every other piece of otto's state and backups were the one directory that rebuilt `$HOME/.otto` itself. Verified against the real binary: `OTTO_HOME=/tmp/.../ottohome otto Upgrade --dry-run` plans its backup under `/tmp/.../ottohome/backups`.

### Deviations
- **The rollback rule is "strictly older", not the doc's "differs from `current_version()`" (same intent, correct rule).** Walked through: start on 3.0.0 with backups 2.0.0 and 1.0.0. Rollback 1 installs 2.0.0 and writes a safety backup of 3.0.0, now the newest file. Rollback 2 runs as 2.0.0, and the newest backup whose version *differs* from 2.0.0 is that 3.0.0 safety backup: the doc's rule rolls forward onto the version just left and fails the doc's own test bullet ("rollback twice across three fixture versions lands on the oldest"). "Older than the version running now" satisfies both, subsumes "differs", and makes repeated rollback monotonic.
- **`download_and_verify` was renamed to `download_and_check_checksum`.** The bullet says the *messages* must not read as signature verification; a function name is the message a reader hits first, and this one claimed the stronger thing. Its doc now states in so many words that a checksum match says nothing about who produced either file. Three test names renamed with it.
- **`--list-versions` was fixed too, which the bullet does not name.** Its "(latest)" marker was `i == 0` over `/releases`, i.e. the same "newest created is latest" assumption the bullet removes from the upgrade path; leaving it would have left the file asserting two different meanings of "latest" one screen apart. It now marks the tag `/releases/latest` returns, and warns and drops the marker if that lookup fails rather than failing a listing.
- **The `--dry-run` plan gained a step it always performed.** The checksum check happens between download and extract and the plan never mentioned it; with the numbering rebuilt anyway, omitting it would have been a second wrong plan.
- **`test_platform_detection` was deleted, not rewritten.** It existed to read `platform._os`, one of the fields this phase deletes; `platform_strings_match_the_published_asset_suffixes` already asserts the detected suffix is one `install.sh` publishes, which is strictly stronger.
- **`test_backup_dir_default` was rewritten rather than left alone.** It asserted `.otto/backups` is a substring of the default, which stays true under a rebuilt `$HOME/.otto` and so could not see the fix; it now pins both branches (`$OTTO_HOME` set, and unset) and is `#[serial]` because the environment is process-global and other tests in this binary write `OTTO_HOME`.
- **`grep -n 'releases/latest'` prints one hit only because two doc comments were reworded** to name the accessor functions instead of respelling the path. The remaining hit is the production `format!`.

### Tradeoffs
- **`select_rollback_target` fails loudly when nothing older exists** vs falling back to the newest backup — a rollback that reinstalls the version already running is a no-op reported as success. The error names the current version and lists the versions present, so the answer to "why did it refuse" is in the message.
- **The safety backup is skipped when a backup of the current version already exists** vs always writing one — a second copy of the same bytes under a newer timestamp is what created the trap in the first place, and it is not additional safety. Cost: if the installed binary at that version has been modified since its backup was taken, the modification is not preserved. Rollback restores released binaries, so that case is a user who overwrote `otto` by hand.
- **`--version X` now resolves before the already-on-target and downgrade short-circuits.** `otto Upgrade --version <nonexistent>` used to print "Use --force to downgrade" and exit 0 when the current version was newer; it now fails naming the version. Same number of HTTP requests, and the honest answer is that the release does not exist.
- **`stage_beside_with` takes a closure that production never varies** vs leaving the discard path untested — the alternative was a `#[cfg(test)]` fault-injection global, which is worse, or no test, which is how the stranding shipped.

### Break-the-test proofs
Per the doc's Testing Strategy. Each fix reverted alone from the green tree, the named test run, the fix restored; the file was byte-compared against the kept copy after each round and `otto ci` is green on the restored tree.

- `select_rollback_target(...)` reverted to `&backups[0]`: `two_rollbacks_walk_down_to_the_oldest_backup` FAILED — `the second rollback must go back another version, not undo the first`, left `"otto 3.0.0"`, right `"otto 1.0.0"`. That left value is the bug verbatim: the second rollback reinstalled the version the first one replaced.
- The safety-backup skip reverted to an unconditional `create_backup`: `a_rollback_does_not_stack_a_second_backup_of_the_version_it_is_leaving` FAILED (three backups where two belong).
- `latest_release_url()` reverted to `fetch_releases(...).first()`: `the_default_target_comes_from_the_latest_endpoint_not_the_listing` FAILED — left `"otto 9.9.10"`, right `"otto 9.9.9"`: it installed the prerelease that was created most recently.
- `tagged_release_url()` reverted to a `find` over the listing: `an_explicit_version_is_fetched_by_tag_even_when_the_listing_omits_it` FAILED (the off-page release is "not found").
- The staged-file discard reverted to `set_executable(&staged)?`: `a_staged_copy_is_removed_when_it_cannot_be_made_executable` FAILED — `staging debris left behind: [".otto.upgrade-1310"]`.
- `asset_name(...)` reverted to the plan's own spelling: `the_dry_run_plan_names_the_asset_find_asset_looks_for` FAILED — `step 1 must name the published asset otto-v9.9.9-linux-amd64.tar.gz, got: Download otto-9.9.9-linux-amd64.tar.gz`.
- `get_backup_dir` reverted to `$HOME/.otto/backups`: `test_backup_dir_default` FAILED — left `"/home/saidler/.otto/backups"`, right `"/tmp/otto-home-fixture/backups"`.
- The "(latest)" marker reverted to listing order: `the_latest_marker_follows_the_latest_endpoint_not_the_listing_order` FAILED.

### Success criteria, as run
- **`grep -n 'releases/latest' src/cli/commands/upgrade.rs` prints one hit.** `475:        format!("{}/releases/latest", self.api_base())`. PASS.
- **The two-step rollback test passes.** `two_rollbacks_walk_down_to_the_oldest_backup ... ok`, and it FAILS with the message above when the selection is reverted. PASS.
- **`grep -cE '^\s+_(os|arch|name):' src/cli/commands/upgrade.rs` prints 0.** Observed `0`; `PlatformInfo::_os`/`_arch` and `GitHubRelease::_name` are gone and `tag_name` is untouched. PASS.
- **Against the live GitHub API, debug binary, `OTTO_HOME` pinned to a scratch dir.** `otto Upgrade --dry-run` -> `Target version:  v2.2.1` resolved through `/releases/latest`, and `1. Download otto-v2.2.1-linux-amd64.tar.gz` / `2. Create backup: /tmp/.../ottohome/backups/otto-<current>-<timestamp>.backup`. With `--no-backup --force --version 2.2.1` the plan is numbered `1.` through `5.` with no gap: download, checksum, extract, run `--version`, replace. `otto Upgrade --list-versions` prints `v2.2.1 (latest)` with the marker coming from the latest endpoint rather than the listing's first row.
- `otto ci`: `✅ All CI checks passed!` — lint, compile, clippy, fmt-check, check, test, cov, cov-report all `finished successfully`; coverage 93.4% lines (23499/25169) against the 87% threshold.

### Open questions
- **Nothing has exercised the real `--rollback` end to end on a machine with a populated `~/.otto/backups`.** The new selection rule is covered by fixtures and by two unit tests, but the first real rollback after this ships will meet whatever version strings are already in that directory, including any written before backups moved under `$OTTO_HOME`. A backup left in `$HOME/.otto/backups` while `$OTTO_HOME` points elsewhere is now invisible to `--rollback`; that is the intended meaning of `$OTTO_HOME`, and worth a line in the release notes.
- **Signature verification stays parked** (the doc's Non-Goals). The command's wording now says "checksum" everywhere, so the gap is at least not misdescribed.
