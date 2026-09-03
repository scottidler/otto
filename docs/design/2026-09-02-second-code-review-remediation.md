# Design Document: Second Code Review Remediation

**Author:** Scott A. Idler
**Date:** 2026-09-02
**Status:** Implemented
**Review Passes Completed:** 5/5 (draft, correctness, clarity, edge cases, excellence), plus review-panel rounds 1 through 5 (Architect + Staff Engineer), every finding dispositioned in Resolved Decisions, Open Questions empty
**Verified against:** HEAD `f9882ed`, Cargo.toml `2.2.1`, tag `v2.2.1`
**Modules touched:** executor, executor/scheduler, executor/state, ports, cfg, cli/parser, cli/commands, makefile, tui, app/main, Cargo.toml, repo/CI, docs, examples

## Summary

A repo-wide review at v2.2.1 (six parallel reviewers by area, ~110 findings, every bug reproduced against `target/debug/otto` or confirmed by reading the code) found 14 bugs, a tail of load-time validation holes, dead code and duplication, one deprecated dependency on the hot path, and a set of docs describing code that does not exist. This doc converts every finding into a phased plan. One doc, 17 phases, one commit per phase: the 2026-06-10 remediation already litigated one-doc-vs-many (its Alternative 4) and this doc inherits that decision rather than reopening it.

Nothing here is a new feature. Every phase either makes an existing promise true (a doc claim, a comment, a column, a flag), or removes the promise.

## How to read this document

- **Implementing a phase?** Go straight to it. Each bullet carries a `file:line` anchor against `f9882ed` and, where reproduced, an `Observed` line that is the failing test you write first.
- **Anchors expire, and some were born wrong.** Line numbers are pointers into `f9882ed`. Re-anchor before editing; the 2026-06-10 doc records nine times an anchor went stale under its own phases. Panel round 1 swept all 173 anchors in this doc and found seven past end-of-file (the reviewer reports mixed vintages); those are corrected inline. Where a bare filename is ambiguous (`config.rs` exists under both `cfg/` and `cli/parser/`), the directory is now given.
- **Bugs first, then validation, then dead code, then docs.** Phases 1-2 bite `tatari-tv/otto-dev` today. Phases 16-17 describe the tree as it stands after 1-15, so they go last.
- **Model tags** pick who runs the phase: `opus` where the phase changes semantics or touches the scheduler, `sonnet` where it is mechanical.

## Problem Statement

### Background

v2.0 through v2.2.1 landed in six days: strict ottofile schema, buffered foreach, computed envs, required params, process-group reaping on cancel, `foreach.jobs`, Ctrl+C on the non-TUI path, and a 14-batch implementation audit. Each change was reviewed against its own design doc. Nobody had yet read the whole tree once, cold, after all of them.

The 2026-06-10 remediation (`docs/design/2026-06-10-code-review-remediation.md`) did that for v1.x and closed ~98 items. This is the same exercise for the v2.x tree.

### Problem

Four of the bugs sit exactly where the recent work moved a boundary:

1. **Process groups broke stdin and SIGTERM.** Putting every non-`tty` child in its own process group (`task_execution.rs:230`) was right for reaping. But stdin is still inherited, so a child that reads the terminal is a background group reading its controlling tty: the kernel stops it with SIGTTIN (the code comment at `:236-238` names this for the `tty` case and then leaves stdin inherited anyway) and otto waits forever, silently. Measured 2026-09-02 under `script`: a task body `head -c1` never returns and otto runs until `timeout` kills it (rc 124); the same task with `</dev/null` finishes in under a second. There is a second shape (panel round 1): a program that opens `/dev/tty` itself, which is the controlling terminal regardless of fd 0. Measured: child in its own group, fd 0 = `/dev/null`, `head -c1 </dev/tty` -> stopped, state `T`; under otto the same task hangs to the timeout, rc 124. `sudo` prompts on `/dev/tty`, so `otto-dev`'s docker-start task (`sudo systemctl start docker`, `otto-dev/.otto.yml:124`) hangs today whenever the sudo timestamp has expired, and nulling fd 0 alone would not change that. Phase 1 handles both shapes. And only SIGINT is handled (`app.rs:522-528`): on SIGTERM otto dies with default disposition, `abandon_run` never runs, and every child (now in its own group) survives a CI runner's group kill.
2. **Subtask names contain `:` and the env writer does not fold it.** `json_to_env` folds `-` and `.` only, so a consumer of `up:alpha`'s output sources a file containing `OTTO_INPUT_UP:ALPHA_K='v-alpha'`, which bash executes as a command. Reproduced: exit 127, `command not found`.
3. **`buffer: true` assumes the parent is in the run.** `otto say:alpha` on a buffered foreach suppresses the live leg (the parser flag says buffered) but `ReplayCursor::new` builds no group (the parent is not in the run set), so the item's output goes to its log and nowhere else. Reproduced: `[say:alpha] finished successfully`, no `HELLO alpha`.
4. **The env evaluator defers on references but runs commands first.** `A: "$(echo $B)"`, `B: hello` resolves `A` to `got:` because the `$(...)` executes with `B` stripped from the context before the deferral check sees the reference. Reversing the declaration order gives the right answer. Reproduced both ways.

The rest is the tail every codebase grows between whole-tree reads: builtins reachable two ways with two behaviors, load-time checks that only fire for one of four cases, a `--rollback` that can only go back one step, a converter that inverts `@-`, a TUI pane that hides the newest lines, dead exit-code fields, 15 dead macros, three copies of `format_size`, five copies of the blocked-task sweep, a deprecated YAML crate under every ottofile, a `baseline` task that `cd`s into a deleted directory, and command pages documenting flags that are positional.

### Goals

- Every reviewer finding has a phase, a bullet, and a success criterion, or a recorded reason it is not here (Addendum).
- Every reproduced bug gets a regression test that fails on `f9882ed`.
- The tree after Phase 17 has: no doc page describing a nonexistent flag, file, or symbol; no `pub` item whose only callers are tests (or it is `#[cfg(test)]`); no dependency in `[dependencies]` used only by tests; no builtin reachable by two paths with two behaviors.
- Anchored, runnable acceptance criteria, observed on `main` before and after.

### Non-Goals

- **New features.** No new flags, keys, or commands. Where a fix needs a new key (`End`/`G` in the TUI) it is the minimum to make an existing promise ("follow mode") reachable.
- **Release-artifact signature verification.** Parked with a revisit condition in the 2026-06-10 Addendum; the review re-raised it (`upgrade.rs:632-634` "verify" wording). This doc fixes the wording only and does not reopen the decision.
- **Rewriting the Makefile converter on a real grammar.** Rejected 2026-06-10 (Alternative 2). This doc fixes the three heuristics that are wrong.
- **The kebab-case key flip.** Parked 2026-08-29 with a revisit condition ("next time the schema is opened"). Phase 4 opens the schema for `jobs: Option<usize>` and `ForeachSpec` serialization. **Decision below:** those are field-shape changes, not key renames, and the parked item stays parked; its revisit condition is about renaming keys.
- **History rewrite, `otto-dev` changes, or any other repo's fixes.** Blast radius is stated; the work stays here.

## Proposed Solution

### Overview

Seventeen phases, each one commit, each `otto ci` green before commit, in the order listed. Phases 1-5 fix the fourteen bugs and the validation holes around them. Phases 6-13 collapse the duplication that produced the bugs (two builtin paths, three formatters, five sweeps, hand-copied meta tasks) and trim what nothing calls. Phase 14 is repo and dependency hygiene, Phase 15 the YAML crate swap. Phases 16-17 regenerate the docs from the running binary.

### Architecture

No new components. Four structural changes, each of which removes a class rather than a symptom:

- **One builtin path.** Today a builtin is reached either by `main.rs:314-321` (clap route, first arg only) or by the parser's injected fake task (`meta_tasks.rs`). Phase 5 rejects a mixed list in the parser and reserves builtin names at config load; Phase 6 derives the fake task's params from the clap struct, so the two help surfaces cannot drift because there is one source.
- **One identifier predicate.** `env.rs:177 is_valid_env_key` and `action.rs:45 validate_identifier` are the same function twice; Phase 2 makes them one (`naming::is_identifier`, a whole-name rule: leading `[A-Za-z_]`, then `[A-Za-z0-9_]*`); Phase 4 validates `foreach.as` with it. The Phase 2 env-name fold is a separate per-byte rule (`[A-Za-z0-9_]` kept, everything else `_`) and is not derived from the whole-name predicate, because applying a leading-digit rule per byte would fold `2024` to `____` (panel round 1, S1).
- **One blocked-task sweep.** `scheduler.rs` has three identical `retain` blocks and `support.rs` a fourth that ignores `when:`. Phase 11 makes it one function.
- **One `TransactionGuard`.** The cold-start path already has an immediate-mode guard; Phase 9 moves it where `migrations.rs` and `manager.rs` can both use it, and every read-then-write goes through it.

### Data Model

- `OttoSpec.jobs: usize` -> `Option<usize>`, `None` means "CPU count, decided at parse". Today `is_default_jobs` skips serializing an explicit `jobs: 8` on an 8-core host and the next read invents a different number on a different host (`otto.rs:259-261`).
- `ForeachSpec` gains `skip_serializing_if` on every optional field, matching `ParamSpec` and `OttoSpec` (`task.rs:159-213`).
- `Nargs::Range(min, max)` stores the real minimum. `"N"` deserializes to exactly N (see Resolved Decisions).
- `runs.hostname` and `tasks.script_hash` become populated columns; no schema change.
- `TaskReport.exit_code` and `TaskFailure.exit_code` are deleted; the drain loop reads the code from the report it already has.

### API Design

- **Ottofile:** no key added or removed. `nargs: "N"` changes meaning (exactly N). `otto.jobs` and every optional `foreach` field (`glob`, `range`, `command`, `as`, `jobs`, `parallel`, `max_items`, `buffer`) re-emit only when set. Numeric and boolean scalars are accepted as edge targets the same way they are as task keys.
- **CLI:** `otto <task> <Builtin>` and `otto <Builtin> <task>` are errors naming both. A user task named like a builtin is a config-load error. `-t` works everywhere `--tui` works. An unknown default task is the same error as an unknown CLI task. `Clean` exits non-zero when a delete failed. `Upgrade --rollback` steps back through distinct versions. `Upgrade` "latest" is GitHub's `/releases/latest`.
- **Runtime:** a non-`tty` task never reads the terminal. Its stdin is `/dev/null` when otto's own stdin is a terminal (a pipe or file stdin is still inherited), and it runs in its own session (`setsid`), so it has no controlling terminal and a program that opens `/dev/tty` gets `ENXIO` and fails at once instead of stopping silently. `tty: true` is the only way a task owns the terminal. SIGTERM and SIGHUP cancel the run like SIGINT; a second signal exits 128 plus the signal number (130, 143, 129). `OTTO_INPUT_<TASK>_<KEY>` folds every non-identifier byte to `_`.
- **Bash helpers, env vars, builtin flags:** unchanged in behavior, newly documented (Phase 17).

### Implementation Plan

#### Phase 1: Executor process boundary: stdin and SIGTERM
**Model:** opus

- `task_execution.rs:203-231`: on the non-`tty` path, when `std::io::stdin().is_terminal()`, add `.stdin(Stdio::null())` before spawn. A pipe or file stdin stays inherited (SIGTTIN only arises from reading the controlling terminal). Update the comment at `:236-238`, which today says "stdin is already inherited - otto never redirects it". This rule stays necessary alongside `setsid` below: a pty fd inherited into a child in another session is not that child's controlling terminal, so a read on it neither stops nor fails, it blocks waiting for keystrokes (measured: `setsid` child, fd 0 = the pty, `head -c1` still alive after 3s in state `S`). Nulling fd 0 gives EOF instead. Nothing in `src/` relies on the child sharing otto's session: the only terminal checks are otto's own `stdout().is_terminal()` at `app.rs:404` and `cli/parser.rs:969`.
  `Observed:` under `script -qec "timeout 6 otto ask" /dev/null`, a task body `echo before-read; head -c1; echo after-read` printed nothing after `before-read` and otto ran until `timeout` killed it, rc 124, no message; identical with `timeout --foreground`. The same task with `</dev/null` printed `after-read` and finished in 0s. Note for the test: bash's `read -t N` polls before it reads and times out cleanly, so it does not reproduce this; use a real `read(2)` such as `head -c1` or `cat`.
- Same site, the `/dev/tty` shape (panel rounds 1 and 2): in the non-`tty` branch, **replace** `cmd.process_group(0)` (`task_execution.rs:230`) with `setsid()` in `CommandExt::pre_exec` (the one `unsafe` block, commented). The child becomes a session leader with no controlling terminal, so `open("/dev/tty")` fails with `ENXIO` for every program in the body, whatever it does with signal dispositions. One precondition (panel round 3): a session leader *acquires* a controlling terminal by opening a terminal device, and bash does that at startup when a terminal is on fd 0 (measured: `bash -c` with the pty on fd 0 under `setsid` acquires it and then blocks; `head`, `cat`, and dash do not, a dup'd fd is not an open). So the fd-0 rule above is what makes the `setsid` guarantee hold for bash bodies, not an independent half; the two go together and the phase must not land one without the other. `setsid` yields `pgid == pid`, so `abandon_run`'s `killpg` and the grandchild-reachability comment at `:219-227` are unchanged. Trap, verified: `setsid()` returns `EPERM` if the caller is already a group leader, so it replaces `process_group(0)` rather than following it. The comment at `:274` ("`own_group` mirrors the `process_group(0)` above exactly") is rewritten to name `setsid`. The `tty: true` branch keeps its current path (otto's group, inherited fds). This is the fail-loud half; nulling fd 0 is the fail-fast half for programs that read stdin.
  `Observed:` pty session, child with fd 0 = `/dev/null`, body `head -c1 </dev/tty`:

  | child setup | `bash -c` | `bash -ic` |
  |---|---|---|
  | own process group (today) | stopped, state `T` | stopped, state `T` |
  | own group + SIGTTIN/SIGTTOU ignored (round-1 draft) | rc 1, `EIO`, 0.0s | **stopped, state `T`** |
  | `setsid` | rc 1, `/dev/tty: No such device or address`, 0.0s | rc 1, same, 1.8s |

  Under otto today the same body hangs: rc 124 at the 6s timeout. The round-1 draft chose `SIG_IGN`; round 2 showed two holes: an interactive shell resets job-control signals to default (row 2, right column), and `sudo` installs its own SIGTTIN handler (`tgetpass.c:218`), restores the ignore, then self-signals and `goto restart`s, so it would spin re-printing the prompt instead of stopping. `setsid` is not defeatable by the child.
- Policy, stated once here and in the reference page: a non-`tty` task cannot read the terminal, by either route; `tty: true` is how a task gets one. A prompt in a non-`tty` body is a task failure with the program's own error text, not a hang. For `sudo` that text is `a terminal is required to read the password` (the `ttyfd == -1` branch, `tgetpass.c:139-146`, reached only when `/dev/tty` cannot be opened, which `setsid` guarantees; with no askpass configured it exits there). For a bash body it is `/dev/tty: No such device or address`.
- **Blast radius, `otto-dev`:** its scripts gate every `/dev/tty` prompt on `has_terminal() { [ -t 0 ] && ... }` (`otto-dev/scripts/lib.sh:142`; readers at `bootstrap.sh:589,820,1006`, `init_dev.sh:300,374`). Today that gate passes under otto (stdin inherited) and the script then hangs on SIGTTIN; after this phase the gate is false and those scripts take their noninteractive paths, which is the behavior their own comment at `otto-dev/.otto.yml` `init` already expects. The docker-start `sudo` fails loudly if its timestamp has expired, instead of hanging. `otto-dev` uses `tty:` nowhere; marking a task `tty: true` to get prompts back is their call, not this doc's.
- `app.rs:520-533 install_interrupt_handler`: `select!` over `ctrl_c()`, `signal(SignalKind::terminate())`, and `signal(SignalKind::hangup())`; any one calls `cancel.cancel()` and logs which. A second signal of any kind exits 128 plus its number (130, 143, 129). The doc comment gains the SIGTERM and SIGHUP sentences. SIGHUP is in because a terminal hangup is the same bug: it kills otto (default disposition), `abandon_run` never runs, and the children, in their own groups or sessions, are not reached by the hangup and are orphaned (panel round 3 measured a pty hangup killing the session leader and otto's group while a child in its own group survived). One extra `select!` arm, same mechanism, same test shape.
  `Observed:` task body `sleep 333 & wait`; `kill -TERM <otto pid>` 1.5s in: otto exited 143 and `pgrep -x sleep -a | grep -c ' 333$'` read `1` before and `1` after. The child survived.
- Scope of the reaping, stated so the criterion below is true: a `tty: true` task is registered `own_group: false` (`task_execution.rs:249`) and cancelled by `kill(pid)` (`scheduler.rs:612-615`), because it shares otto's group and a `killpg` would hit otto (`cancel_reaping_test.rs:346` asserts that must never happen). Its grandchildren therefore survive any cancel, SIGINT included; the 2026-09-01 doc records that carve-out. This phase does not widen it: SIGTERM and SIGHUP reap exactly what SIGINT reaps today, the descendants of every non-`tty` task.
- `app.rs:632-641` (TUI path) has its own `ctrl_c()` task that only sets a shutdown flag, which `tui/app.rs:103` reads once per loop after drawing. That is not enough for a hangup (panel round 4): after SIGHUP every terminal write fails `EIO`, the draw at `tui/app.rs:83` returns `Err` through `?`, `run()` returns, and `app.rs:655` propagates `tui_result` with `?` *before* `cancel.cancel()` at `:662`. The flag is never read, the run is never cancelled, the children are orphaned: the exact bug on the exact path where a closing terminal is the motivation. Three changes here, one behavior: (a) the TUI signal task `select!`s over the same three signals and calls `cancel.cancel()` directly on the first one, in addition to setting the flag; (b) `app.rs:655` becomes cancel-then-propagate: if the scheduler is not finished, `cancel.cancel()` and await the scheduler handle, then the `?` as today. The "dashboard closed, cancelling the run" message and its still-running list stay in the normal-quit branch at `:660-673` only; they are not printed on the error path, because "dashboard closed" is false for a hangup (`CancelSignal::cancel` is idempotent, `scheduler.rs:210-214`, so that branch cancelling again is harmless); (c) the second-signal escape hatch in TUI mode restores the terminal first (`terminal.restore()`, the same claim mechanism Phase 13 fixes) and then exits 128 plus the signal number, so a wedged run never leaves the user in the alternate screen. `--tui` plus any `tty: true` task is already rejected before execution (`app.rs:390-397`, pinned by `tests/tty_task_test.rs:275`), so no `tty` interaction exists on this path. Side effect worth naming: with stdin nulled, a non-`tty` child no longer competes with the TUI for keystrokes on the same terminal.
- Tests: `tests/sigint_cancel_test.rs` gains SIGTERM and SIGHUP siblings using the marker-file pattern from `tests/cancel_reaping_test.rs`, each asserting on a non-`tty` task's grandchild, and a `--tui` SIGHUP case under a pty (`tests/tui_panic_test.rs` already drives a pty) asserting the grandchild dies and the terminal is restored; `tests/tty_task_test.rs` gains "a non-tty task running `head -c1` under a pty exits within the timeout with stdin at EOF", "a non-tty task running `head -c1 </dev/tty` under a pty fails within one second", the same with the body run through `bash -ic` (the interactive-shell hole; assert on the `/dev/tty` error substring, since `bash -ic` without a terminal also prints a job-control warning on stderr), "a `tty: true` task running `head -c1 </dev/tty` under a pty reads the byte written to the pty", and "a non-tty task with piped stdin reads the pipe" (`echo hi | otto t` where the body is `cat`). `tests/cancel_reaping_test.rs` keeps passing unchanged, which is what proves `setsid`'s `pgid == pid` preserved the reaping.
- Docs touched here, not in Phase 17, because the behavior is new: `docs/commands/ottofile-reference.md`'s `tty` row gains "a non-`tty` task cannot read the terminal: stdin is `/dev/null` when otto's is a terminal, and it has no controlling terminal, so `/dev/tty` cannot be opened. Set `tty: true` for anything that prompts."
- **Success criteria:** `grep -c 'SignalKind::hangup()' src/app.rs` prints at least 1 (a count, not a shape: factoring both paths through one helper is fine and would make an "exactly 2" fail with the work done); the SIGTERM and SIGHUP tests leave zero descendants of the non-`tty` task on both the plain and the `--tui` path; `grep -c 'cmd.process_group(0)' src/executor/scheduler/task_execution.rs` prints 0 (the call; today 1) and `grep -c 'setsid' src/executor/scheduler/task_execution.rs` prints at least 1; the three non-tty pty tests complete in under their timeouts with the read failing; the `tty: true` pty test reads the byte; `cancel_reaping_test` passes unchanged.

#### Phase 2: Executor data passing and buffered replay
**Model:** opus

- Shared identifier predicate, created here because this phase needs it first: `env.rs:177 is_valid_env_key` and `action.rs:45 validate_identifier` are the same function written twice. One `pub(crate) fn is_identifier(&str) -> bool` in `naming.rs` (it already owns name rules); both call sites use it. Phase 4 reuses it for `foreach.as`.
- `action.rs:454-457` (bash `task_upper` fold) and `scheduler.rs:725-748` (`json_to_env`): fold every byte outside `[A-Za-z0-9_]` to `_`, both sides, one rule. This is a per-byte class, deliberately not the whole-name predicate above (which rejects a leading digit and would turn `OTTO_INPUT_UP_2024` into `OTTO_INPUT_UP____`). Rust: `c.is_ascii_alphanumeric() || c == '_'`; bash: extend the two `${var//x/_}` lines to `tr -c '[:alnum:]_' '_'` under the `LC_ALL=C` the surrounding code already sets. `OTTO_INPUTKEY_`/`OTTO_INPUTTASK_` companions already carry the real names, and collision suffixing already exists (`scheduler.rs:758`), so nothing is lost.
  `Observed:` `before: ["up:alpha"]` consumer failed with `input.up:alpha.env: line 4: OTTO_INPUT_UP:ALPHA_K=v-alpha: command not found`.
- `task_execution.rs:26` `suppress_terminal = tui_mode || task.buffered`: replace `task.buffered` with "the replay cursor owns this task" (`cursor.parent_of(name).is_some()`), computed in `execute_all` where the cursor is built (`replay.rs:171`) and passed into `execute_task`. The parser flag stays as the cursor's input; it stops being the suppression decision.
  `Observed:` `otto say:alpha` on a `buffer: true` foreach printed only `[say:alpha] finished successfully`; `otto say` printed `HELLO alpha`.
- `scheduler.rs:758`: the warn literal contains 22 literal spaces inside the string; join the line.
- Tests: `tests/foreach_aggregation_test.rs` (or a new `subtask_output_test.rs`) with a consumer of `up:alpha`'s output asserting `got=[v-alpha]` and exit 0; `tests/foreach_buffer_test.rs` gains "requesting one item of a buffered foreach prints its output".
- **Success criteria:** `otto use` in the fixture above exits 0 and prints `got=[v-alpha]`; `otto say:alpha` prints `HELLO alpha`; `grep -c '                      {base}' src/executor/scheduler.rs` prints 0.

#### Phase 3: cfg env evaluator
**Model:** opus

- `env.rs:76-88`: before `evaluate_single_env_value`, run `referenced_vars(raw_value)` over the whole raw value (it already scans `$(...)` bodies, `env.rs:551-559`); if any referenced name is a declared key, other than the key being evaluated, and not yet in `evaluated`, push to `still_pending` without executing. Only then evaluate. The self-exclusion matters: a key referencing its own name reads the inherited seed (`env.rs:200-203`, the documented self-reference rule) and must not defer on itself forever. A two-key cycle through `$(...)` bodies reaches the existing no-progress branch, and `find_reference_cycle` already uses `referenced_vars`, so it is reported as a cycle, not as "not found".
  `Observed:` `A: "$(echo \"got:$B\")"`, `B: hello` -> `A=[got:] B=[hello]`; swapping names -> `B=[got:hello]`. Order-dependent.
- Same loop: classify errors. A command that exits non-zero, or a reference to a name that is neither declared nor inherited, returns immediately with the existing message. Only "declared and not yet resolved" defers. Today `Err(_) => still_pending.push(...)` treats every error as "wait", so a failing `$(cmd)` re-runs once per pass and a `"$(cmd) ${LATER}"` value runs `cmd` on every pass (`env.rs:79-88`, `:104-114`).
- `env.rs:440-452` comment: "controlled environment to prevent parent process pollution" is not what the code does. `env_clear()` plus seven essentials is immediately followed by `cmd.envs(env_overrides)`, which is the whole inherited environment minus declared keys. Say that: the essential list matters only when one of those seven is itself a declared key.
- **Blast radius check, in this phase:** `otto-dev/.otto.yml:93-105` uses `$(echo "${OTTO_DEV_AUTH_PROFILE:-...}")`, references to *inherited* names inside `$(...)`. Those must still resolve from the inherited environment. The deferral rule keys on declared names only, so they do; the test below pins it.
- Tests in `env_tests.rs`: sibling-in-command both orders; inherited-name-in-command resolves; self-reference inside `$(...)` reads the inherited value and terminates; two keys referencing each other inside `$(...)` report a cycle; failing command runs exactly once (count via a marker file); `"$(cmd) ${LATER}"` runs `cmd` exactly once.
- **Success criteria:** the fixture prints `A=[got:hello]` in both declaration orders; a `$(touch $MARK.$RANDOM; false)` value leaves exactly one marker file after a failed load.

#### Phase 4: cfg load-time validation and schema truth
**Model:** opus

- `task.rs:552-558 resolve_range`: compute `end.checked_sub(start).and_then(|n| n.checked_add(1))` and compare to `max_items` before iterating. `range: "0-18446744073709551615"` must be a config error, not an allocation.
- `task.rs:340-364 validate_sources`: count all four sources (`command`, `glob`, `items`, `range`) and error unless exactly one is set. Today it returns `Ok` when `command` is `None`, so `glob:` + `items:` loads and `resolve_items`'s `else if` drops `items`; `foreach: {}` loads and fails at expansion.
  `Observed:` `glob: "*.yml"` + `items: [x, y]` -> `otto --help` shows `multi [1 items]`, no error.
- `task.rs:180-182` `foreach.as`: validate at load with Phase 2's `naming::is_identifier`, error naming `foreach.as`. Today `as: "my item"` loads and fails at `action.rs:303` naming "environment variable name", not the field.
- `param.rs:374-378`: `"N"` -> `Range(N, N)`, exactly N (decision below). `Range` stores the real min; `Serialize` (`param.rs:318`) emits `"N"` for `min == max` and `"min:max"` otherwise, and `nargs_to_num_args` (`cli/parser.rs:370`) stops adding 1. `ottofile-reference.md:162` changes from "(max count, min 0)" to "exactly N", and gains one sentence (panel round 1, S3): a bounded zero-to-N is not expressible (`"0:N"` is rejected today and stays rejected: `min must be at least 1`); use `"?"` for zero-or-one or `"*"` for zero-or-more. Nothing is lost at runtime: bare `N` never meant zero-to-N in clap, only the reference page said so. Round-trip test at the text level: `nargs: "3"` re-emits `nargs: "3"`.
- `otto.rs:259-261 is_default_jobs`: `jobs: Option<usize>`, `None` = CPU count resolved at parse in the one place `default_jobs()` is read (`cli/parser.rs:118,701,859`). `deserialize_jobs` (`otto.rs:234`) keeps rejecting `Some(0)` at load; that guard was reinstated once already (2026-06-10 doc, Phase 1) and does not move. `tests/roundtrip.rs:27 config_otto_only_roundtrips` currently passes on a 4-core host for the wrong reason (`jobs: 4` skipped then re-defaulted to 4); after this it passes for the right one.
- `cfg/edge.rs:47-88 EdgeVisitor` implements only `visit_str`, `visit_string`, `visit_map` (`:53,61,69`): add `visit_u64`/`visit_i64`/`visit_bool` that stringify, so `after: [2024]` is accepted the same way the `2024:` task key is (siblings behave identically). Traces to the cfg reviewer's finding 8.
  `Observed:` a task keyed `2024:` loads; a sibling with `after: [2024]` fails `tasks.b.after[0]: invalid type: integer `2024`, expected a task name string or a {task, when} object at line 6 column 13`; `after: ["2024"]` runs `[2024] y`.
- `task.rs:159-213 ForeachSpec`: `skip_serializing_if` on `glob`, `range`, `command`, `as`, `jobs`, `parallel`, `max_items`, `buffer`, matching `ParamSpec` (`param.rs:82-112`). Text-level round-trip test: a minimal `foreach: {items: [a]}` re-emits without `glob: null`.
- Dead: `param.rs:232 Values`, `param.rs:238 Value::Dict` (never constructed; the only mention is a match arm at `action.rs:64`), `task.rs:829-857 TaskSpec::new` (test-only, nine positional args under `#[allow(too_many_arguments)]`; tests use a struct literal with `..Default::default()`), `resolver.rs:109-113,133-137 has_foreach/has_choices` (`#[cfg(test)]` or delete).
- Docs in code: `cfg/config.rs:11-19` has two doc blocks merged onto `otto_is_default` (move the `deny_unknown_fields` paragraph onto `ConfigSpec`); `cfg/config.rs:12` cites `borg/src/config.rs:281-285`, a private repo path in a public tree after the internal-name sweep: state the rule, drop the citation. `env.rs:12` and `otto.rs:58` link private items in public docs (the two `cargo doc` warnings); use plain backticks.
- **Success criteria:** `cargo doc --no-deps` emits zero warnings; the `glob`+`items` fixture fails at load naming both sources; `foreach: {items: [a], as: "my item"}` fails at load naming `foreach.as`; `after: [2024]` loads and runs the `2024` task; `range: "0-18446744073709551615"` fails at load in under one second; `nargs: "3"` round-trips byte-identical; `cargo test roundtrip` passes with `jobs` explicitly set to a value other than the host's CPU count; `grep -n 'Value::Dict\|pub type Values' src/cfg/param.rs` prints nothing; `grep -n 'fn has_foreach\|fn has_choices' src/cfg/resolver.rs` prints nothing outside `#[cfg(test)]`; `grep -n 'pub fn new(' src/cfg/task.rs` prints nothing. (Named per file on purpose: `DynamicResolver::new` at `resolver.rs:74` and `TaskSpec::has_foreach` at `task.rs:861` are production and stay; a combined pattern would flag them.)

#### Phase 5: CLI builtin routing
**Model:** opus

- `app.rs:433-434`, `:553-554`: `find_builtin` early-returns, so `otto build Clean` runs only `Clean` and `filter_execution_tasks` plus the second "No tasks to execute" check (`:437-443`, `:557-563`) are dead. Fix in `Parser::parse`: a task list containing both a builtin and a user task is an error naming both; delete `filter_execution_tasks`.
  `Observed:` `otto build Clean --dry-run` -> `Querying database for old runs... / No runs matching deletion criteria found`, exit 0, `build` never mentioned.
- `meta_tasks.rs:67,147,263,343,405,539`: `tasks.insert("Clean", ...)` on an `IndexMap` silently replaces a user task of that name. `cli/parser/config.rs:203-219 validate_no_builtin_params` validates reserved *param* names; add the same for task names against `BUILTIN_COMMANDS`, same error shape.
  `Observed:` an ottofile with `tasks: {Clean: {bash: echo USER-CLEAN-RAN}}`, `otto Clean --dry-run` -> the builtin ran.
- `parser.rs:253-270 take_tui_flag`: strip `-t` as well as `--tui` (`help.rs:46-51` declares `.short('t').global(true)`).
  `Observed:` `otto build -t` -> `unexpected argument '-t'`; `otto build --tui` works.
- `discovery.rs:26`: an unknown default task is `Err(unknown_task_error(...))` with the `nearest_task_name` suggestion, not a warning and exit 0.
  `Observed:` `otto.tasks: [bild]`, bare `otto` -> `Warning: Default task 'bild' not found / No tasks to execute`, exit 0.
- `parser.rs:848-861`: the `--help` path builds `temp_parser` with `ottofile: None`, so `base_dir()` is the cwd and foreach globs resolve there. Pass `Some(path)`.
- `help.rs:117-118`, `command.rs:183,209-211`: `tasks.is_empty()` after builtin injection can never be true; use `tasks.keys().any(|n| !is_builtin(n))`. Consequence today: `otto.tasks: ["*"]` with no user tasks prints "No tasks to execute" instead of help.
- `help.rs:107`: `-h` is intercepted for every task, so a param declared `-h|--host` can never be given. Treat `-h` as help only when the task declares no `-h` short.
- `cli/builtins.rs:8-13` checklist (the file is 33 lines): names `inject_NAME_meta_task() in parser.rs` (now `parser/meta_tasks.rs`) and omits the `Builtin` enum (`app.rs:227-259`) and the `main.rs:314-321` match. Rewrite the checklist; add a test that every `Builtin::all()` entry is handled by `handle_subcommand` except `Graph`, which is deliberately absent.
- **Blast radius check:** run the new binary's `otto --help` and `otto --tasks` in `~/repos/tatari-tv/otto-dev`; its ottofile has no builtin-named task and its default `tasks:` all exist (verify by command, record in the commit message).
- **Success criteria:** `otto build Clean` exits non-zero and the message names both `build` and `Clean`; a task named `Clean` fails at load; `otto build -t` reaches the TUI (or fails only for lack of a tty); bare `otto` with `tasks: [bild]` exits non-zero naming `bild` and suggesting `build`.

#### Phase 6: CLI meta tasks from clap, and dead CLI code
**Model:** opus

- `meta_tasks.rs` (551 lines, six copy-pasted `TaskSpec` literals): derive each builtin's `TaskSpec` from its clap `Command` (`CleanCommand::command().get_arguments()`, etc.). One `builtin_task(cmd: &clap::Command, help: &str) -> TaskSpec` maps `Arg` -> `ParamSpec` with the clap 4 reflection API (the repo is on clap 4.6.6; `takes_value` is clap 3 and gone): `get_long`/`get_short`, `get_help`, `get_default_values`, `get_action` + `get_num_args` -> `nargs` (`SetTrue` -> flag), `is_positional`, `get_possible_values` -> `choices`, `is_hide_set` -> skipped. The `TaskSpec` fields clap cannot express (`action` placeholder, `foreach: None`, `virtual_parent: false`, `tty: None`, `on_failure: []`) are identical static defaults across all six and live in `builtin_task`. No builtin uses `choices_command` (21 `None`, 0 `Some` in `meta_tasks.rs`), the one field with no clap equivalent. This is what makes `otto help Clean` and `otto Clean --help` one surface. Today they disagree: meta `--keep DAYS` vs clap `--keep-days`; meta lacks `--keep-last/--keep-failed/--no-db`; History/Stats meta declare `-t/--task TASK` while clap takes `TASK` positionally; Upgrade meta lacks `--backup-dir/--github-token`.
- `Graph` has no clap struct today (`meta_tasks.rs:6-67` is its only definition; it is reached by the task route alone and is deliberately absent from `main.rs:314-321`). Give it a `GraphCommand` derive in `cli/commands/` with the params the meta task declares, so it is derived like the other five and the `app.rs` extractor reads typed values instead of strings. `format` defaults to `ascii`, matching `meta_tasks.rs:24` and `graph.rs:97`'s `unwrap_or("ascii")`; do not reach for `GraphOptions::default()` (`graph.rs:52`), which is `Svg` and has only test callers (panel round 1, N1). No new route: `Graph` stays task-route only.
- The task-route extractors in `app.rs:47,98,151` (`unwrap_or(30)`, `unwrap_or(20)`, `unwrap_or(10)`) fall back silently when the meta param is missing or unparseable; after derivation the param always exists, and the fallback becomes an `expect` naming the derivation.
- Delete `src/cli/macros.rs` (15 macros, zero callers in `src/` or `tests/`, hidden by `#![allow(unused_macros)]`) and `#[macro_use] pub mod macros;` in `cli/mod.rs:1-2`.
- `parser.rs`: delete `OttofileNotFound` (`:421-430`, never constructed), `Parser.prog`/`Parser.user` (`:671-672`, written, never read); replace `calculate_hash` (`:381-386`) and `cli/parser/config.rs:43-46` (same sha256-to-hex[..8]) with one function; `nearest_task_name` (`:332`) filters lowercase `"graph"`, which never appears in production lists: delete the filter and the synthetic test at `parser_tests_a.rs:49-52`.
- `parser.rs:1049-1074` and `:1081-1105 parse_all_tasks`: both branches load config then run the identical "all non-builtin names -> `process_tasks_with_filter`" tail. Compute `ottofile_value` in the match and fall through once.
- **Success criteria:** `grep -cE 'name: "(Clean|Convert|Graph|History|Stats|Upgrade)"' src/cli/parser/meta_tasks.rs` prints 0 (no hand-written builtin literals remain); a test asserts for every builtin that the meta task's param set equals its clap `Command`'s visible arg set; `ls src/cli/macros.rs` fails; `cargo clippy -- -D warnings` clean with no new `#[allow]`.

#### Phase 7: Clean, History, and shared formatters
**Model:** sonnet

- `clean.rs:184-196`: the DB path prints `Warning`/`Error` per failed delete and returns `Ok(())`. The filesystem path (`:282-329`) counts `refused`/`failed` and returns `Err`, with a comment on why exit codes matter to scripts. Same command, one contract: count and return `Err`.
- `clean.rs:474-477 calculate_dir_size`: `entry_path.is_dir()` and `entry.metadata()` follow symlinks; a link inside a run dir to a large or unreadable tree makes the scan slow or aborts `Clean` via `?`. Use `entry.file_type()?` and skip symlinks, as `scan_runs` does at `:349`.
- `clean.rs:207` says "No runs older than N days found" when `scan_runs` returned every run regardless of age (its own doc at `:335-339`); `:253-257` header says "older than {keep_days} days" even when `--keep-failed` or `--keep-last` decided. Messages name the filter that ran.
- `history.rs:152-156`: `starts_with(&home)` then `s.replace(&home, "~")` replaces every occurrence. `format!("~{}", &s[home.len()..])`.
- New `cli/commands/format.rs`: `format_size` (today `clean.rs:454`, `history.rs:335`, `stats.rs:329`, GB precision `.1` vs `.2`), `format_duration` (`history.rs:317`, `stats.rs:313`), `format_timestamp` (`clean.rs:449` UTC vs `history.rs:306`/`stats.rs:249` Local: the same run prints two different times). One implementation each; Local for all three.
- **Success criteria:** `grep -rn 'fn format_size\|fn format_duration\|fn format_timestamp' src/cli/commands/` prints exactly three hits, all in `format.rs`; a DB-path `Clean` with one refused directory exits non-zero; `otto History` renders `/home/u/proj/home/u/x` as `~/proj/home/u/x`.

#### Phase 8: Upgrade
**Model:** opus

- `upgrade.rs:523-527 --rollback`: the safety backup of the current binary makes the next rollback reinstall what was just replaced. Choose the newest backup whose version differs from `current_version()`; skip the safety backup when one for the current version already exists.
- `upgrade.rs:437-443`: "latest" is `releases.first()` from `/releases` page 1 (created order, includes prereleases; `GitHubRelease` at `:347-354` has no `prerelease`/`draft` field). `install.sh:121` uses `/releases/latest`. Use `/releases/latest` for the default and `/releases/tags/v{X}` for `--version X`, which also fixes "Release vX not found" for a release beyond page 1.
- `upgrade.rs:930-931 get_backup_dir`: rebuilds `$HOME/.otto/backups`; use `layout::resolve_otto_home()?.join("backups")` like `clean.rs:486`.
- `upgrade.rs:152-162 stage_beside`: copy succeeds, `set_permissions` fails with `?`, `.otto.upgrade-<pid>` is stranded until the next upgrade's reaper. `remove_file` on that path.
- `upgrade.rs:562` dry-run prints `otto-{ver}-{platform}.tar.gz`; `find_asset` (`:615`) and the release workflow use `otto-v{ver}-...`. One `asset_name(version, platform)` used by both. Step numbering skips "2." under `--no-backup`.
- `upgrade.rs:632-634` and its messages say "verify"; say "checksum matches the release's published `.sha256`" so nobody reads it as signature verification (parked, see Non-Goals).
- `upgrade.rs:318-319 PlatformInfo::_os/_arch`, `:350-351 GitHubRelease::_name`: delete (serde ignores unknown fields by default).
- `upgrade.rs:383-391`: the `with_fixture` doc paragraph is attached to `tap_no_backup`; move it.
- `upgrade_tests.rs:632 test_version_parsing` asserts `semver` orders 0.5.5 < 0.5.6: tests the crate. Delete.
- Tests: rollback twice across three fixture versions lands on the oldest; "latest" resolves through `/releases/latest` (fixture server); dry-run asset name equals `find_asset`'s.
- **Success criteria:** `grep -n 'releases/latest' src/cli/commands/upgrade.rs` prints one hit; the two-step rollback test passes; `grep -cE '^\s+_(os|arch|name):' src/cli/commands/upgrade.rs` prints 0 (underscore-prefixed fields only; a bare `_name` pattern would match `tag_name`).

#### Phase 9: State correctness
**Model:** opus

- `migrations.rs:126-155`: every upgrade step uses `conn.unchecked_transaction()` (deferred) and reads (`column_exists`) before writing. The v0 branch at `:115` uses `TransactionGuard::immediate` and documents why: a deferred read-then-write gets `SQLITE_BUSY` immediately and `busy_timeout` cannot help. Use the guard for each step and re-read the version inside it.
- Move `TransactionGuard` to `state/mod.rs` (or `db.rs`) so `manager.rs` can use it.
- `manager.rs:154-194 record_run_start`: upsert project, `SELECT id`, `INSERT run`, `UPDATE run_count` as four autocommit statements; a crash between the last two drifts `run_count` permanently (the `MAX(run_count - 1, 0)` guard on delete exists because of this). One immediate transaction.
- `manager.rs:276-288 record_task_complete`: `SELECT started_at` then `UPDATE`, duration in Rust with no clamp; `record_run_complete` (`:210-216`) does it in one statement with `MAX(?3 - timestamp, 0)`. Same shape: one `UPDATE ... SET duration_seconds = CASE WHEN started_at IS NULL THEN NULL ELSE MAX(?3 - started_at, 0) END`.
- `manager.rs:951-991 delete_run`: rows and `run_count` are committed, then `delete_run_directory` runs `ensure_deletable_under_root` (`:1032`). A refusal leaves the directory orphaned with no row. Resolve and validate the path before `BEGIN`; only `remove_dir_all` after commit. `manager_tests.rs:36 delete_run_never_deletes_through_a_symlinked_run_directory` asserts the victim survives; add "and the row survives".
- `manager.rs:462`: unknown `skip_kind` is `.and_then(SkipKind::parse)` -> `None`; `row_to_run_record` (`:407-411`) rejects unknown status with `bad_column`. Same rule: `bad_column(13, ...)`.
- `workspace.rs:375` passes `None // hostname not in ExecutionContext yet`; `RunMetadata::current_system_info` (`metadata.rs:85`) exists for this and has no production caller. Call it. `history.md`'s JSON example already promises `hostname`.
- `task_execution.rs:183` passes `None // TODO` for `script_hash` while `ProcessedAction::Bash { hash, .. }` (`action.rs:86-91`) carries it in the same match. Pass it.
- `ports/db.rs:255-262 MemoryStateStore` completes a missing task with `Ok(())` (SQLite errors), `:397-399` never computes avg/min/max durations (SQLite does), `:260` `ended_at - started_at` is unchecked u64. Align, and add a parity test on the model of `memory_and_sqlite_stores_agree_about_retention` covering `get_task_stats` durations. `stats_tests.rs` uses only the fake today, so `otto Stats` duration columns are never exercised end to end.
- Tests: `db_tests.rs:8-14 clear_path_env` removes `OTTO_HOME`/`OTTO_DB_PATH` and never restores (use `manager_tests.rs:16-29`'s `with_otto_home`); `migrations_tests.rs:330` runs with `foreign_keys` OFF so the `PRAGMA foreign_keys=OFF` toggle at `migrations.rs:148` is unguarded (turn FKs ON in the test); `manager_tests.rs:872` asserts `<= 7` where the fixture's answer is 3; `manager_tests.rs:788 test_delete_run_updates_project_count` never reads `run_count`.
- **Success criteria:** `grep -c 'conn.unchecked_transaction()' src/executor/state/migrations.rs` prints 0 (call sites; the explanatory comment at `:12` that names the API stays); two processes opening a v4 fixture DB concurrently both succeed (extend `tests/concurrent_cold_start_test.rs` with the upgrade path); `otto History --json` shows a non-null `hostname`; a `manager_tests.rs` test asserts `script_hash` is `Some` on the row `record_task_start` writes for a bash task, and `grep -c 'TODO' src/executor/scheduler/task_execution.rs` prints 0.

#### Phase 10: State and ports trim
**Model:** sonnet

- `ports/fs.rs:18-55 FileSystem` declares 25 methods (16 async, 9 sync); 13 have no production caller (the round-1 text said 14 and listed `write` among them; `write` is called through `self.fs` at `workspace.rs:352` and `:457`, reached from `app.rs:455` and `:575`, panel round 2). Three of the 13 (`exists`, `is_dir`, `read_to_string`) are used through `MemFs` by `src/executor/workspace_tests.rs` (`grep -rnE '\bfs\.(exists|is_dir|read_to_string)\(' src --include='*_tests.rs'`: 8 hits there, 20 in `fs_tests.rs`, none in `cli/commands/`; the round-1 text named the wrong directory) and stay. The removal set is exactly these ten: async `is_file`, `metadata`, `remove_file`, `remove_dir_all`, `copy`, `read_dir`, `read_link`, `symlink`, `set_permissions`, and sync `metadata_sync`. Kept, 15: async `exists`, `is_dir`, `read_to_string`, `write`, `create_dir_all`, `create_dir_exclusive`, `canonicalize`; sync `exists_sync`, `read_sync`, `write_sync`, `create_dir_all_sync`, `remove_file_sync`, `copy_sync`, `symlink_sync`, `set_permissions_sync`. `RealFs`, `MemFs`, and `fs_tests.rs` shrink with them. `Workspace::script_cache` (`workspace.rs:263`) encodes the old `.cache/<task>/<hash>` layout and has no callers: delete.
- `ports/db.rs:52-53`: `get_recent_runs(n, p)` is `get_runs_with_filters(None, p, n)`; it and `get_run_tasks` have only test callers. Delete `get_recent_runs`, redirect the tests; delete `get_run_tasks`.
- `manager.rs:541-842 get_task_stats/get_all_task_stats`: ~120 duplicated lines, 8 queries per row. One `GROUP BY` query with `COUNT(*)`, `SUM(status='completed')`, `SUM(status='failed')`, `SUM(status='skipped')`, `AVG/MIN/MAX(duration_seconds)`; `LIMIT ?` bound to `-1` for no limit.
- Project-name derivation copied at `manager.rs:1051-1060`, `schema.rs:264-270` (inside `migrate_v1_to_v2`), `ports/db.rs:125-135`: one `project_name_from(ottofile: Option<&Path>, hash: &str)` in `naming.rs`.
- `schema.rs:118 RUNS_TABLE_DDL` says it is shared by `init_schema` and the v4-to-v5 rebuild; `migrate_v4_to_v5` (`schema.rs:306-328`) has its own inline copy under `runs_v5`. Derive the `runs_v5` DDL from the constant.
- `db.rs:190-241 health_check`, `stats`, `DatabaseStats`, `path()`: test-only; delete. `db.rs:40 conn: Arc<Mutex<Connection>>` is never cloned: `Mutex<Connection>`. `metadata.rs:44 RunMetadata::minimal` test-only (the file is 95 lines); its doc and the struct doc ("stored in run.yaml") are stale: `run.yaml` is a serialized `ExecutionContext` (`workspace.rs:344-354`).
- `db.rs:135-165`: the "Where the database lives" doc block is attached to the `#[cfg(test)]` guard; move it onto `default_db_path` (`:182`).
- **Success criteria:** `awk '/pub trait FileSystem/,/^}/' src/ports/fs.rs | grep -c 'fn '` prints 15 (was 25); `grep -c 'fn ' src/ports/fs.rs` drops by at least 20 (ten methods, two impls; record the before count in the commit); `grep -rn 'fn get_recent_runs(\|fn health_check(\|fn script_cache(' src/` prints nothing (with the paren: two test names at `manager_tests.rs:891,923` start with `get_recent_runs_` and would match the bare pattern); `otto Stats` and `otto Stats --json` against a fixture DB (a scratch `OTTO_HOME` after running `examples/hello-world` twice and `examples/parallel-tasks` once) are byte-identical before and after the query rewrite (capture first).

#### Phase 11: Executor cleanup
**Model:** opus

- `scheduler.rs:111-122 TaskReport.exit_code`: written, never read (both drain-loop arms match with `..` at `:1594-1599`, `:1735-1740`). `TaskFailure.exit_code` (`:168-195`) is `None` in all three `From` impls, so `code.or(exit_code)` at `task_execution.rs:469` is always `exit_code`. Delete both fields and the doc sentence about a consumer "that used to re-parse it"; the drain loop reads the code from the local it already has.
- `support.rs:313-340 get_file_timestamps` swallows every error into `None` and always returns `Ok`, so `needs_rebuild` cannot fail and the `Err` arm at `:127-144` duplicates the `Ok(true)` arm. Return `bool`; delete the arm; delete or rename `scheduler_tests_b.rs:565-597 test_file_dependency_check_error_handling`, which passes via the early return at `:276-283`.
- The blocked-task sweep: `scheduler.rs:1546-1566`, `:1714-1733`, `:1778-1797` (three identical `retain` blocks over `classify_gates`) and `support.rs:104-125` (tests only `completed_set.contains`, ignores `when:`, never detects newly-unreachable dependents). One `sweep_blocked(...) -> Vec<Task>` used by all four. `classify_gates` has a fifth caller that is not a sweep and stays as it is: the dispatch gate at `scheduler.rs:1475`, which classifies one ready task before launch. `classify_source`'s own doc (`scheduler.rs:979-986`) records that this exact drift already happened once.
- Cancel tests fixing a sleep: `scheduler_tests_a.rs:113` (200 ms then `cancel()`; passes even if the child never spawned) and `tests/foreach_buffer_cancel_test.rs:149` (1500 ms). Use the marker-file pattern from `tests/cancel_reaping_test.rs`.
- **Success criteria:** `grep -cE 'pub exit_code|exit_code: Option<i32>' src/executor/scheduler.rs` prints 0; `grep -h 'classify_gates(' src/executor/scheduler.rs src/executor/scheduler/support.rs | wc -l` prints 3 (the definition, the dispatch gate at `:1475`, and the single call inside `sweep_blocked`; today it prints 5: definition plus four call sites at `:1475,1547,1715,1779`); `grep -rn 'sleep(Duration::from_millis' src/executor/scheduler_tests_a.rs tests/foreach_buffer_cancel_test.rs` prints nothing before a `cancel()`.

#### Phase 12: Makefile converter truth
**Model:** sonnet

- `converter.rs:284-297`: only the first prefix character is handled. Loop over leading `@`, `-`, `+` (any order, repeated); set ignore-errors if `-` appeared; strip all three; emit `|| true` once.
  `Observed:` `@-rm -rf dist` -> `-rm -rf dist`; `-@echo hi` -> `@echo hi || true`; `+make -C sub` -> verbatim.
- `parser.rs:515`: `strip_prefix(".PHONY:")` misses `.PHONY :`. Match `.PHONY`, optional whitespace, `:`.
  `Observed:` `.PHONY : clean` -> "special target `.PHONY` is not converted" and a false "not `.PHONY`" warning on `clean`.
- `parser.rs:452-465`: a target containing `$(` or `${` becomes a literal task named `$(TARGETS)` with no warning about the name (`converter.rs:228` warns for `$` in dependencies only). Treat like a pattern rule: warn and skip the recipe.
  `Observed:` `$(TARGETS): dep` -> `tasks: - $(TARGETS)` as the default task.
- Delete `makefiles/{docker-compose-service,go-build-project,makefile-example,python-poetry-service,python-pre-commit}/otto.yml`: hand-written, not converter output, drifting from `expected.yml` (`jobs: 4`, invented `help:`, different edges). `expected.yml` is asserted (`tests/makefile_converter_test.rs:231-249`); `otto.yml` is only touched by `tests/examples_integration_test.rs:163-205`, which runs `otto --help` in the directory and proves nothing about conversion. Delete those five tests with the files.
- Tests: the three cases above as golden fixtures under `makefiles/` with `expected.yml`.
- **Success criteria:** `printf '.PHONY : c\nc:\n\t@-false\n' | otto Convert` emits `bash: false || true` and no warning; `ls makefiles/*/otto.yml` prints nothing; `otto Convert --strict` on the `$(TARGETS)` fixture exits non-zero naming the target.

#### Phase 13: TUI
**Model:** opus

- `pane.rs:311-326`: the window `[start_line, end_line)` is `visible_height` *unwrapped* lines, each then wrapped; the resulting rows exceed the inner height and `Paragraph` clips the bottom, so in follow mode a wrapped line hides the newest output. Wrap first, then take the last `visible_height` wrapped rows (scroll state in wrapped rows).
- `mod.rs:105-112 TuiTerminal::restore`: calls `claim_terminal_restore()` and discards the bool, then restores unconditionally; `mod.rs:33-41` says the claim must be exclusive because a second `LeaveAlternateScreen` is harmful. `if !claim_terminal_restore() { return Ok(()); }`. `tests/tui_panic_test.rs` `mem::forget`s the guard so it never sees the double restore; add the in-scope case.
- `tui/app.rs:349-384`: `Home` exists, nothing resumes following once scrolled (`ScrollState::down` re-enables follow only at the bottom, `pane.rs:122`). Bind `End` and `G` to `ScrollState::bottom()` (sets follow); add to the status bar text and `examples/tui-demo/README.md`.
- **Success criteria:** a pane test with one 3x-width line and `visible_height` rows shows the last line of the buffer; a restore-twice test performs exactly one `LeaveAlternateScreen`; `End` after `k` returns `follow == true`.

#### Phase 14: Dependencies and repo hygiene
**Model:** sonnet

- `Cargo.toml`: `regex` is used only by `tests/history_table_alignment_test.rs`: move to `[dev-dependencies]`. `tempfile` is in both sections (`Cargo.toml:37`, `:64`): keep `[dependencies]` only. `log = { features = ["std", "serde"] }`: nothing serializes a `Level`; drop `serde`. Delete the empty `[build-dependencies]` header.
- `expanduser` (3 sites: `cli/parser/config.rs:26`, `upgrade.rs:927`, `workspace.rs:115`) drags in `dirs 1.0.5`, `redox_users 0.3.5`, `winapi`, `pwd`, `lazy_static`. One `expand_tilde()` on `std::env::home_dir()`: checked 2026-09-02, `rustup run 1.96.0 rustc -D warnings` compiles a `home_dir()` call clean, so no deprecation warning on the CI toolchain. `dirs = "6.0"` (one call, `layout.rs:42`) goes with it. `num_cpus` (`otto.rs:134`, `cli/parser.rs:118,701,859`) -> `std::thread::available_parallelism()`. `cargo tree -i` confirms otto is the only parent of all three, so they leave the tree entirely.
- `.github/workflows/release-and-publish.yml:62-65`, `:120-123`: the "Set GIT_DESCRIBE" steps are dead; `build.rs:19` always emits `cargo:rustc-env=GIT_DESCRIBE`, which overrides the ambient variable. Delete the steps.
- `.otto.yml:296` `baseline` task `cd examples/ex1` (deleted; only `ex2` exists): delete the task, its TUI-phase narrative is finished work. `.otto.yml:328-329` `clean` removes `.otto-run/`, which nothing writes: drop the two lines. `.vscode/launch.json:61,79,97` point at `examples/ex1/` with tasks `hello`/`world`: retarget to `examples/hello-world`.
- `.github/workflows/test-setup-otto.yml:32` runs `otto dev`, a task removed from `.otto.yml` in `f77fbce` (2025-12-24, "removed jq dependency"); the workflow only triggers on edits to itself, so it has failed silently since. Use `otto --help`. `checks.yml:135` says the floor is "currently 85"; `.otto.yml:108` is 87: say "see `cov-report --fail-under`'s default".
- `.pre-commit-config.yaml`: a third definition of "did the suite pass" (fmt, clippy, `cargo test --locked`, `cargo check --locked`) that nothing references. Reduce to one local hook running `otto quick`.
- `.rustfmt.toml`: 292 lines, two live (`max_width = 120`, `single_line_if_else_max_width = 80`), the rest a 2017 rustfmt template of options that no longer exist. Keep the two lines; `cargo fmt --all --check` must stay clean with no reformat, which proves the deleted lines were inert.
- **Success criteria:** `cargo tree -e normal --depth 1 | grep -c ' regex v'` prints 0 (`regex` stays in the tree transitively via `env_logger -> env_filter`, so the criterion is the direct edge, not the crate's presence); `cargo tree -e normal | grep -c 'expanduser\|num_cpus\|dirs v'` prints 0; `grep -c 'ex1' .otto.yml .vscode/launch.json` prints `.otto.yml:0` and `.vscode/launch.json:0`; `grep -c 'otto dev' .github/workflows/test-setup-otto.yml` prints 0; `otto ci` green.

#### Phase 15: YAML crate
**Model:** sonnet

- `serde_yaml = "0.9.34"` resolves to `0.9.34+deprecated` (`Cargo.lock:2890`); upstream is archived (RUSTSEC-2024-0320) and it loads every ottofile. otto uses four symbols: `from_str`, `to_string`, `Value`, `Error` (21 files). `serde_yaml_ng` 0.10 is the maintained API-compatible fork (decision below).
- Step 1, spike inside the phase: swap the dependency and `use` paths, run `cargo test`. The strict-schema tests pin error text (`unknown field`, `line N column M`), so the suite is the compatibility check; record what changed.
- Step 2: fix whatever the suite reports. If the error-location format changed, update the pins and `docs/design/2026-08-29-strict-ottofile-schema.md`'s quoted messages, and say so in the commit.
- Exit (panel round 1, S5): if the suite reports a parsing difference the phase cannot absorb by re-pinning text (a value that parses differently, an anchor or alias handled differently), the swap is reverted and the phase ends as a recorded null result here, with the failing case quoted; the phase is the whole commit, so reverting is `git revert` of one commit. Alternative 4 (`serde-saphyr`) becomes the next candidate.
- **Success criteria:** `cargo tree -e normal | grep -c 'serde_yaml v0.9'` prints 0; `otto ci` green; `otto -o examples/foreach-glob --help` output byte-identical before and after; the unknown-field error for `foreach.bogus` still names the field and the upgrade hint.

#### Phase 16: Docs: command and architecture pages, regenerated from the binary
**Model:** sonnet

- `docs/commands/history.md`: `--task <NAME>` (`:18,57,60,145`) is positional `[TASK]` (`history.rs:83-84`); `-n/-s/-p` shorts undocumented; JSON example shows `"status": "success"` (emitted `"Success"`) and omits `run_dir`; `hostname` becomes real in Phase 9. Regenerate from `otto History --help` and `otto History --json`.
- `docs/commands/stats.md`: `--task <NAME>` (`:17,47,71`) is positional; `-n/--limit` and the Top-N table (`stats.rs:19-20,116`) undocumented; "Average Run Duration" row and JSON keys `successful_executions/failed_executions/skipped_executions` (`:40,89,119-121`) are not emitted (actual: `total_runs`, `successful_runs`, `failed_runs`, `running_runs`, `total_tasks`, `total_disk_usage`, `total_duration_seconds`); task JSON is an array. Regenerate.
- `docs/commands/upgrade.md`: is the pre-implementation plan ("Implementation Plan" title, checklists, "Timeline Estimate" at `:557`, a `--base-url` flag at `:76` deliberately never built per `upgrade.rs:299-303`), linked from README as the user doc. Move to `docs/archive/upgrade-implementation-plan.md`; write the user page from `otto Upgrade --help` (`--dry-run`, `-v/--version`, `--list-versions`, `--rollback`, `--force`, `--no-backup`, `--backup-dir`, `--github-token`).
- `docs/commands/clean.md`: `:155,163` `otto-<project-hash>/` and `output.json` (real: `<name>-<hash>/`, `output.<task>.json`); `:1,8,28` lowercase `otto clean`, which in this repo runs the user task that does `cargo clean`.
- `docs/directory-layout.md`: describes `.cache/task1/<hash>`, `output.json`, `input-task1.json`, `artifacts/`, `env.yaml`, `metadata.yaml`, `cmdline.yaml`. A real run writes `<name>-<hash>/.cache/<hash>.sh` (flat), `<ts>/run.yaml` (serialized `ExecutionContext`), `tasks/<t>/{script.sh -> ../../../.cache/<hash>.sh, builtins.sh, stdout.log, stderr.log, output.<t>.json, output.<t>.env}`, `input.<dep>.{json,env}` (`workspace.rs:483-525`). Regenerate from `find` on a scratch `OTTO_HOME`.
- `docs/architecture/sqlite-integration.md`: schema section is v1 (`projects` lacks `name`; `runs` shows the dropped `UNIQUE(timestamp)` and lacks `run_dir`; `tasks` lacks `skip_reason`/`skip_kind`); "relative path from ~/.otto/" for `stdout_path` etc. (absolute, `task_execution.rs:171-172`); "Connection Pooling", "Health Checks", "Rollback support", "fall back to in-memory database" describe nothing that exists; history flow cites `get_recent_runs` (deleted in Phase 10); layout shows `otto-<hash>/` and a top-level `.cache/`. Regenerate the schema from `schema.rs` and delete the feature claims.
- `docs/grammar.md:463-490`: "Implementation Notes" name `parse_global_options_only()`, `parse_tasks_only()`, nom combinators, `enum ParseError`, `enum ValidatedValue` (`:274`); none exist, nom is not a dependency. Delete the section; point at `src/cli/parser.rs partitions()`. Add the per-foreach `--Serial` flag to the option table.
- `docs/migration-guide.md`: historical SQLite note promising `otto import --scan` (`:82`) and PostgreSQL (`:265`), recommending `otto -v build` (`:302,325`; `-v` does not exist), lowercase builtins throughout. Move to `docs/archive/`.
- `docs/schedulers.md`: a pasted ChatGPT transcript ("You said: what does AirFlow use?") with no otto content. Delete.
- **Success criteria:** `grep -rncE 'otto (clean|history|stats|upgrade)\b' docs/commands docs/*.md` prints 0 for every file; `grep -c -- '--task' docs/commands/history.md docs/commands/stats.md` prints 0 for both; `ls docs/schedulers.md docs/migration-guide.md` fails; `grep -c 'base-url\|Timeline' docs/commands/upgrade.md` prints 0; for each of History, Stats, Clean, Upgrade, every long flag in `otto <B> --help | grep -oE -- '--[a-z-]+' | sort -u` appears in its page (a `for` loop over the four, zero misses), which is the regeneration check the grep-for-stale-strings criteria do not give.

#### Phase 17: Docs: README, reference surface, examples, links
**Model:** sonnet

- `README.md:131` "supports `--version` and `-v`": clap defines `-V` only. Drop `-v`. `:142-149` steps 5-6 describe building and uploading a release by hand; `release-and-publish.yml` does that on tag push: replace with "push the tag". `:23,26,32` `scottidler/setup-otto@v1` redirects to `otto-rs/setup-otto` (`gh api repos/scottidler/setup-otto`); `test-setup-otto.yml:21` already uses the new name. `:101` lists three ottofile names; `parser.rs:84-90 OTTOFILES` accepts six (`.otto.yaml`, `Ottofile`, `OTTOFILE` too), same for `ottofile-reference.md:3`.
- `docs/commands/ottofile-reference.md`: new section "Environment and shell helpers": `OTTO_TASK`, `OTTO_TASK_DIR`, `OTTO_WORKSPACE`, `OTTO_TASKS_DIR`, `OTTO_USER` (`task_execution.rs:208-212`, `action.rs:370`), `OTTO_FOREACH_INDEX/ITEM`, `OTTO_HOME`, `OTTO_DB_PATH`, `OTTO_MAX_LOG_BYTES` (`main.rs:16-18`), `OTTOFILE`; bash color vars `RED..NC`; `otto_set_output`, `otto_get_input`, `otto_serialize_output`, `otto_deserialize_input` (`action.rs:416-429,526`) and the Python equivalents (`action.rs:783-786`); the per-foreach-task `--Serial` flag (`builtins.rs:25`). Plus the Phase 1 stdin sentence and Phase 4's `nargs` row if not already landed.
- Broken relative links into `docs/archive/`: `positional-parameters.md:83` -> `../flag-support.md`; `tasks.md:153,160` -> `../foreach-subtasks.md`; `upgrade.md:537` -> `../capitalized-builtins-design.md`; `history.md`/`stats.md` -> `graph.md` (never existed); `migration-guide.md` -> `sqlite-implementation-plan.md`. Repoint or remove.
- Recurrence guard: `bin/check-doc-links`, a short shell script that resolves every relative `.md` link under `docs/`, `examples/`, and `README.md` against the tree and exits non-zero on a miss, wired into `.otto.yml`'s `docs` task so `otto all` runs it. Links moved into `docs/archive/` broke seven times without anyone noticing; a guard is the structural fix, not a sweep.
- `examples/README.md` is 0 bytes: one line per example (22). `examples/data-passing-demo/README.md` documents tasks `bash_producer`..`final_report`; the ottofile has `task_a`..`report`; references `~/.otto/ex14-*/latest/tasks` (no `latest` symlink exists) and `ex8`/`ex10`/`ex11`; `otto.yml:234` prints the same bogus path. Rewrite against the actual tasks. `examples/file-dependencies/README.md:21-22` says `examples/ex8/otto.yml`. `examples/dependency-ordering/otto.yml:5` defaults to task `bob`, which does not exist: point at `two`.
- `convert.rs:11` about text and the meta help never say "reads stdin"; bare `otto Convert` in a terminal blocks. Say "Convert a Makefile on stdin to an ottofile".
- **Success criteria:** `grep -c '`-v`' README.md` prints 0; `grep -c 'scottidler/setup-otto' README.md` prints 0; `grep -c '^- ' examples/README.md` is at least 22; `otto -o examples/dependency-ordering` runs without "not found"; `bin/check-doc-links` exits 0, and exits non-zero when one link is deliberately broken (the break-the-test proof for the guard).

### Cross-repo blast radius and ship order

- **`tatari-tv/otto-dev`** is the only known consumer with a v2 ottofile. Checked by grep on 2026-09-02: no builtin-named task, no `nargs`, no `tty: true`, no subtask output consumer, no `otto_get_input`/`OTTO_INPUT` use. Phase 1 changes its visible behavior in two ways, both stated in Phase 1: `/dev/tty` prompts gated on `[ -t 0 ]` go noninteractive instead of hanging, and an expired-timestamp `sudo` fails loudly instead of hanging. It must keep working after Phase 3 (its `$(echo "${VAR:-...}")` values read inherited names inside `$(...)`; the Phase 3 test pins that). Its comment at `.otto.yml:60` ("otto's `$(...)` parser truncates at the first unbalanced `)`") is already false since v2.1 and is an otto-dev edit, not this doc's.
- **`otto-rs/setup-otto`**: README link target only (Phase 17). No change there.
- **Ship order:** phases in listed order, one commit each on `main`, `otto ci` green before each. One `bump` after Phase 17. If a release is needed earlier, cut it after Phase 5 (all fourteen bugs closed); the release notes then say the stdin, `/dev/tty`, SIGTERM, and SIGHUP behavior changed.

## Acceptance Criteria

Each criterion's literal command is run against `main` at `f9882ed` and the output recorded under it before this doc is called ready. Probes run with `OTTO_HOME` set to a scratch directory and `OTTO_DB_PATH` unset (v2 derives the DB path from `OTTO_HOME`).

- [ ] **A non-`tty` task that reads stdin under a terminal completes; it does not hang.**
  task `bash: echo before-read; head -c1 >/dev/null; echo after-read`; `script -qec "timeout 6 otto ask; echo rc=\$?" /dev/null`
  `Observed on main:` `rc=124` after 6s, `after-read` never printed; same with `timeout --foreground`. Control: `otto ask </dev/null` printed `[ask] after-read` in 0s. Expected after Phase 1: `after-read` printed, rc 0, under 1s.

- [ ] **A non-`tty` task that opens `/dev/tty` fails at once; it does not stop.**
  task body (otto's prologue is `set -euo pipefail`, `action.rs:364`, so the rc must be captured under `set +e` or the failing redirect aborts the script before the echo, panel round 4):
  ```
  echo before-read
  set +e; head -c1 </dev/tty >/dev/null; rc=$?; set -e
  echo "tty-read-rc=$rc"; exit "$rc"
  ```
  `script -qec "timeout 6 otto ask; echo rc=\$?" /dev/null`
  `Observed on main:` `rc=124` after 6s; `tty-read-rc` never printed (the read never returns). Expected after Phase 1: `tty-read-rc=1` printed, then the task fails, otto rc 1, under 2s. The two outcomes differ on the presence of the `tty-read-rc` line and on 124 vs 1, so the criterion separates before from after. Mechanism isolated outside otto: a child in its own process group with fd 0 = `/dev/null` reading `/dev/tty` sits in state `T` (SIGTTIN), from `bash -c` and `bash -ic` alike; the same child under `setsid` gets `/dev/tty: No such device or address`, rc 1, in 0.0s from both.

- [ ] **SIGTERM (and SIGHUP) to otto leaves no descendant of a non-`tty` task alive.**
  task `bash: sleep 333 & wait` (non-`tty`); `otto t & pid=$!; sleep 1.5; kill -TERM $pid; sleep 2; pgrep -x sleep -a | grep -c ' 333$'`; repeat with `-HUP`. Scoped to non-`tty` tasks deliberately (panel round 3): a `tty: true` task shares otto's group and is cancelled by `kill(pid)`, so its grandchildren survive every signal, SIGINT included; that carve-out is the 2026-09-01 doc's and this phase does not widen or narrow it.
  `Observed on main:` `1` before the kill, `1` after; otto exited 143. (Use `pgrep -x sleep -a`, not `pgrep -f`: the latter matches the probing shell's own command line.)

- [ ] **A consumer of a foreach subtask's output exits 0 and reads the value.**
  `otto use` on the `up:alpha` fixture; expect `got=[v-alpha]`, rc 0
  `Observed on main:` `input.up:alpha.env: line 4: OTTO_INPUT_UP:ALPHA_K=v-alpha: command not found`, task failed.

- [ ] **`otto say:alpha` on a `buffer: true` foreach prints the item's output.**
  `Observed on main:` `[say:alpha] finished successfully` only; `otto say` prints `HELLO alpha`.

- [ ] **An env value referencing a later-declared sibling inside `$(...)` resolves to the sibling's value.**
  fixture: `otto: {api: 1, envs: {A: "$(echo \"got:$B\")", B: hello}}`, `tasks: {show: {bash: 'echo "A=[$A] B=[$B]"'}}`; `otto show` -> expect `A=[got:hello]`
  `Observed on main:` `A=[got:] B=[hello]`. Swapping the names gives `B=[got:hello]`.

- [ ] **A mixed builtin/task list is an error naming both.**
  `otto build Clean --dry-run; echo rc=$?`
  `Observed on main:` `Querying database for old runs... / No runs matching deletion criteria found`, rc 0; `build` never ran.

- [ ] **`grep -c 'conn.unchecked_transaction()' src/executor/state/migrations.rs` prints 0.**
  `Observed on main:` `4` (call sites at `:126,132,138,151`). The first draft counted the bare word and recorded 4; the real count of that pattern is 5, because the doc comment at `:12` names the API while explaining why the v0 branch avoids it. The criterion now counts call sites so the correct comment can stay (panel round 1, M3).

- [ ] **No test-only crate as a direct `[dependencies]` edge; no deprecated YAML crate anywhere in the normal tree.**
  `cargo tree -e normal --depth 1 | grep -cE ' regex v| serde_yaml v0\.9'`
  `Observed on main:` `2`. (`regex` also appears at depth 3 via `env_logger -> env_filter`; that edge is not otto's and stays.)

- [ ] **No doc page tells the reader to run a lowercase builtin or a flag that does not exist.**
  `grep -rnE 'otto (clean|history|stats|upgrade)\b' docs/commands docs/*.md README.md | wc -l; grep -c '`-v`' README.md`
  `Observed on main:` `62` and `1`.

- [ ] **REGRESSION GUARD: `otto --tasks --format json` on `examples/hello-world` is byte-identical before and after all phases.**
  `otto -o examples/hello-world --tasks --format json | sha256sum`
  `Observed on main:` 895 bytes, sha256 prefix `3eef1056c76f`. This guards the `--tasks` view against an unintended change; no phase here touches `TaskView`, so it verifies nothing about any phase and is kept as a guard only.

## Resolved Decisions

- **2026-09-02: one doc, seventeen phases.** The 2026-06-10 doc's Alternative 4 rejected splitting a remediation into per-phase docs ("Twelve separate docs would re-litigate ordering twelve times") and its author decision later that day removed even the extraction requirement. Same call here for the same reason; the cross-phase view is what makes the two builtin paths, the three formatters, and the five sweeps each one fix.
- **2026-09-02: `nargs: "N"` means exactly N.** Code says 1..=N, the reference says 0..=N, the serializer emits `"1:N"`. None of the three is what a bare integer means to clap (`num_args(N)`) or argparse (`nargs=N`). No ottofile in this repo, its examples, or `otto-dev` uses a bare integer, so the change breaks nothing known. The other two spellings (`"?"`, `"1:N"`) already exist for the other meanings.
- **2026-09-02: stdin policy, amended after panel rounds 1 and 2.** A non-`tty` task's stdin is `/dev/null` only when otto's own stdin is a terminal. A pipe or file is still inherited, so `echo x | otto task` keeps working. **And** the non-`tty` child runs in its own session (`setsid` in `pre_exec`, replacing `process_group(0)`), so it has no controlling terminal and any `open("/dev/tty")` fails with `ENXIO`: `sudo`, `ssh`, `read </dev/tty`, an interactive shell, all fail at once instead of stopping. The policy (a non-`tty` task cannot read the terminal; `tty: true` is how a task gets one) has not changed since the first draft; the mechanism did, twice. Draft 1 nulled fd 0 only; round 1 showed `/dev/tty` bypasses fd 0. Draft 2 ignored SIGTTIN/SIGTTOU; round 2 showed two holes, both re-measured here: `bash -ic` resets job-control signals and still stops (state `T`), and `sudo` installs its own SIGTTIN handler, restores the ignore, self-signals, and `goto restart`s, so it would spin instead of stop (`tgetpass.c:218,252,269-284`, read by the panel from upstream source; neither seat nor this session can run `sudo` in the sandbox, `NO_NEW_PRIVS`). `setsid` is the one mechanism in the comparison the child cannot defeat, and it also makes sudo's "a terminal is required" branch reachable, which the draft-2 text had promised without it being so. Measured: `bash -c` and `bash -ic` both fail in 0.0s and 1.8s under `setsid`. Alternatives recorded below as 3b (detect the stop), 3c (`SIG_IGN`), 3d (`sigprocmask` block).
- **2026-09-02: SIGTERM and SIGHUP join SIGINT on the cancel path.** Same `cancel()`; a second signal exits 128 plus its number. **Corrected after panel round 3:** the first draft excluded SIGHUP with the claim that "a terminal hangup already reaches the children as their own groups' controlling-terminal loss". That is false and contradicted the doc's own SIGTERM argument: a hangup signals the session leader and the foreground group, and a child in its own group (today) or its own session (after Phase 1) survives it, measured on a pty by the panel. Closing a terminal therefore kills otto without `abandon_run` and orphans every non-`tty` child, the same bug as SIGTERM with the same one-arm fix, so it is in. The author's decision, prompted by the panel's correction; not a reviewer finding in the original six reports.
- **2026-09-02: `serde_yaml_ng`, not `serde-saphyr`.** otto uses four `serde_yaml` symbols. `serde_yaml_ng` is the API-compatible fork of the same code; `serde-saphyr` is a rewrite with its own error model, which would invalidate the strict-schema error-message pins for no behavioral gain. Revisit if `serde_yaml_ng` is itself archived.
- **2026-09-02: meta tasks are derived, not tested for parity.** A parity test catches drift after the fact; derivation makes drift impossible. Rule: a field derived from another never diverges.
- **2026-09-02: `jobs` becomes `Option<usize>`.** Alternative was "never skip `jobs` on serialize", which re-emits `jobs: <host count>` into a file that never wrote it. `None` is the honest representation of "not set".
- **2026-09-02: `runs.hostname` and `tasks.script_hash` are populated, not dropped.** Both have a documented consumer promise (`history.md` JSON, `sqlite-integration.md`) and the producing code already exists one call away.
- **2026-09-02: numeric and boolean edge targets are accepted.** `2024:` as a task key already works; `after: [2024]` failing is the two-signals-one-meaning defect. Accepting is smaller than documenting a quoting rule.
- **2026-09-02: `Convert` stays stdin-only.** Adding a path argument is a feature nobody asked for; the help text says stdin.
- **2026-09-02: signature verification stays parked.** Reaffirms the 2026-06-10 Addendum; Phase 8 changes the word "verify" so the checksum is not mistaken for it.
- **2026-09-02: `.pre-commit-config.yaml` reduces to one hook running `otto quick`** rather than being deleted. Anyone with pre-commit installed keeps a gate; the duplicate definition dies.
- **2026-09-02: the five stale `makefiles/*/otto.yml` are deleted, not regenerated.** Regenerating them from `otto Convert` and asserting equality with `expected.yml` is the existing golden test twice.
- **2026-09-02: the kebab-case key flip stays parked.** Phase 4 changes field shapes (`Option`, `skip_serializing_if`), not key names; the 2026-08-29 revisit condition is about renaming.
- **2026-09-02: broken doc links get a guard, not just a sweep.** `bin/check-doc-links` in the `docs` task (Phase 17). Seven links broke when files moved to `docs/archive/`; a sweep fixes seven, a guard fixes the class.
- **2026-09-02, panel round 1 (Architect + Staff Engineer), accepted in full: five must-fix, five should-fix, four nits.** M1 `/dev/tty` (decision above). M2 seven anchors past end-of-file (`builtins.rs`, `edge.rs`, `metadata.rs`, `pane.rs`, `schema.rs` x2, `Cargo.toml`), corrected inline; the reviewer reports carried mixed-vintage line numbers and the doc claimed they were verified. M3 `unchecked_transaction` count was 5 not 4, criterion now counts call sites. M4 the env criterion invoked `otto show` without defining the fixture; fixture stated. M5 `FileSystem` is 25 methods, 14 unused, 3 kept, 11 removed, set pinned. S1 fold rule split from the whole-name predicate. S2 clap 4 reflection API named. S3 bounded zero-to-N sentence added to the reference row. S4 `otto-dev`'s `[ -t 0 ]` gate flip stated in Phase 1 and blast radius. S5 Phase 15 exit stated. N1 Graph `ascii` default pinned. N2 `grep -c` two-file output wording. N3 numeric-edge `Observed` added with the cfg-reviewer trace. N4 `End`/`G` kept as completing an existing pair.
- **2026-09-02, panel round 1, four seat findings rejected against the code, recorded so they are not re-raised.** R1 Architect: "immediate transactions risk SQLITE_BUSY": backwards; `migrations.rs:110-115` documents that the deferred read-then-write is what fails at once, `busy_timeout` is set (`db.rs:58`), and `manager_tests.rs:985` already asserts `record_run_start` is BUSY-free under WAL. R2 Architect: "SIGTTOU/TOSTOP undecided": unreachable for stdout/stderr, which are piped for non-`tty` children (`task_execution.rs:270-271`). **Corrected in round 2:** the round-1 text added "Phase 1 ignores SIGTTOU anyway as a free symmetric guard"; that premise was wrong. Ignoring SIGTTOU is not free: POSIX lets a background `tcsetattr` on the controlling terminal complete when SIGTTOU is ignored, so a child could put the user's terminal into cbreak mode (`sudo` does this once per prompt pass), which the kernel blocks today. Under `setsid` the question is moot (no controlling terminal to `tcsetattr`), and SIGTTOU is left alone. R3 Architect: "the YAML swap is unrequested scope": an unmaintained crate with a RUSTSEC advisory on the ottofile load path was raised by the review and is in scope. R4 Staff: "`otto-dev` `init` prompts break": overstated; they are gated on `[ -t 0 ]` and go noninteractive, which is the fix working (S4).
- **2026-09-02, panel round 3, accepted in full: two must-fix, two should-fix, two nits; no design change to the mechanism.** M1 the SIGTERM acceptance criterion said "no task descendant" and is false for `tty: true` tasks (`kill(pid)`, `scheduler.rs:612-615`); scoped to non-`tty` descendants, matching the 2026-09-01 carve-out, and the scope is now stated in Phase 1. M2 `grep -c 'process_group(0)'` counts the comment at `:274` too; criterion anchored on `cmd.process_group(0)` and the `:274` comment added to the phase. S1 the SIGHUP rationale was factually wrong; withdrawn, and SIGHUP added (decision above). S2 `setsid`'s guarantee depends on fd 0 not being a terminal for bash bodies; stated as a precondition, not a parallel half. N1 the `bash -ic` test asserts a substring. N2 recorded for the record: the Architect's round-3 claim that the `unchecked_transaction` narrative is "backwards" is itself wrong (bare word 5, call sites 4, as the doc says). The Architect seat returned "READY TO BUILD" for the third consecutive round without a probe; its one novel finding was wrong. The panel confirmed all three round-2 must-fix closed and every count in the doc it could re-run.
- **2026-09-02, panel round 4, accepted in full: two must-fix, one cheap win; one non-finding recorded.** M1 (both seats): the TUI path's flag-only handler and the `?` at `app.rs:655` before `cancel()` at `:662` meant a hangup during `--tui` would still orphan children; Phase 1 now cancels directly from the signal task, cancels before propagating a draw error, and restores the terminal before a second-signal exit. M2 (Staff, verified): the `/dev/tty` acceptance fixture could not print its rc under `set -euo pipefail`; rewritten with `set +e` capture, and its before/after outcomes now differ. Cheap win: the `SignalKind` criteria counted a shape ("exactly 2"); now "at least 1" plus behavior tests on both paths. Non-finding, kept so it is not re-raised: first-signal cancellation exits 1 via `main.rs:234-237`, and the doc scopes 128 plus n to the second signal (`app.rs:515-519` records that the ordinary path keeps the normal failure exit). The Architect seat found the TUI area this round but declared it safe; the Staff seat's probe is what made the finding real.
- **2026-09-02, panel round 5: converged.** Zero must-fix; both seats said nothing blocks. Three nits applied: the `cancel.cancel()` anchor is `app.rs:662` (not `:661`) in two places; the dashboard-closed branch is `:660-673`; and (b) of the TUI change now names its ordering (keep the `?`, cancel and await before it, no "dashboard closed" message on the error path). The panel confirmed from the code that `CancelSignal::cancel` is idempotent, that the `/dev/tty` fixture prints `tty-read-rc=1` under `set +e` and skips the echo without it, that `grep -c 'SignalKind::hangup()' src/app.rs` is 0 today, and that the pty harness the new `--tui` test needs already exists (`tests/common/mod.rs:92 pty_cmd`, markers in `tui_panic_test.rs:23-24`).
- **2026-09-02: `FileSystem` keeps the three test-used methods.** `exists`, `is_dir`, `read_to_string` have callers in `src/executor/workspace_tests.rs` via `MemFs` (round 1 said "command tests"; round 2 corrected the file); deleting them would mean rewriting those tests against `tempfile` for no behavior gain. `write` stays because production calls it (`workspace.rs:352,457`). The ten with zero callers outside `ports/` go.
- **2026-09-02, panel round 2, accepted in full: three must-fix, four should-fix.** M1 `SIG_IGN` replaced by `setsid` (decision above; the round-1 mechanism was measured against `bash -c` only and the claim "measured to work" did not cover interactive shells or `sudo`'s handler). M2 `FileSystem::write` moved to the keep set; 10 removed, 15 kept; both Phase 10 numbers corrected. M3 the Phase 10 justification grep named `cli/commands/*_tests.rs`, where every hit is `std::path::Path::exists`; the trait callers are in `workspace_tests.rs`. S1 (their numbering) the R2 rejection's "free symmetric guard" premise withdrawn, above. S2 Phase 4's dead-code criterion would have flagged `DynamicResolver::new` and `TaskSpec::has_foreach`; now per-file and exact. S3 Phase 11's `classify_gates` count: four call sites today, not three; the dispatch gate at `:1475` is named and stays; criterion restated as a single number. S4 Phase 10's dead-symbol grep matched two test names by substring; now anchored on `(`. The Architect seat returned "READY TO BUILD, nothing blocks" this round with no runtime probe and no source citation, and was wrong on all three must-fix; recorded so its bare verdicts are weighted accordingly, as the 2026-06-10 doc already had to record once.

## Alternatives Considered

### Alternative 1: Fix the fourteen bugs, skip the tail
- **Description:** Phases 1-5, 8, 9, 12, 13; drop dead-code, dedup, deps, docs.
- **Pros:** Fewer commits.
- **Cons:** The tail is where the bugs came from: two builtin paths (Phase 5's bugs), three formatters drifting, five sweeps one of which already drifted once, hand-copied meta tasks whose drift is Phase 6's bug. The 2026-06-10 doc made the same argument and it held.
- **Why not chosen:** Every finding gets a phase; that is the operating rule.

### Alternative 2: Per-phase design docs
- **Description:** Seventeen small docs on the `2026-08-28`/`2026-08-29` model.
- **Pros:** Small docs demonstrably ship fast.
- **Why not chosen:** Decided 2026-08-29 (Alternative 4 there) and inherited here; see Resolved Decisions. Recorded so it is not re-proposed.

### Alternative 3: Always `Stdio::null()` for non-`tty` tasks
- **Description:** Unconditionally detach stdin from every non-`tty` child.
- **Pros:** Simpler predicate.
- **Cons:** Breaks `echo x | otto task` and any CI that feeds a task on stdin, for no gain: a pipe never produces SIGTTIN.
- **Why not chosen:** The terminal-only rule is the same fix without the regression.

### Alternative 3b: Detect the stop instead of preventing it
- **Description:** Leave the child with a controlling terminal; poll each child with `waitid(P_PID, WUNTRACED|WNOHANG)` (or a SIGCHLD handler) and fail the task with "tried to read the terminal; set `tty: true`" when it reports stopped.
- **Pros:** otto's own message, not the program's.
- **Cons:** More code on the scheduler's hot loop; the program is left stopped until otto kills the group; races between the stop and the poll; tokio's `Child` exposes no stop notification; and a program that handles SIGTTIN itself (sudo) never stops, so there is nothing to detect.
- **Why not chosen:** `setsid` in `pre_exec` is one line, removes the terminal rather than reacting to its use, and the program's own error text names the cause.

### Alternative 3c: `SIG_IGN` on SIGTTIN and SIGTTOU in `pre_exec` (the round-1 draft)
- **Description:** Keep the process group, ignore the job-control signals so a `/dev/tty` read returns `EIO`.
- **Pros:** Two lines; works for non-interactive bash bodies (measured).
- **Cons:** Defeatable by the child. An interactive shell resets the dispositions and still stops (measured, `bash -ic`, state `T`). `sudo` installs its own SIGTTIN handler, restores the ignore afterwards, then self-signals and restarts the prompt, so it spins instead of stopping (panel round 2, from `tgetpass.c`). Ignoring SIGTTOU also lets a background `tcsetattr` change the user's terminal.
- **Why not chosen:** Both holes are the silent-hang class this phase exists to remove.

### Alternative 3d: block SIGTTIN and SIGTTOU with `sigprocmask`
- **Description:** Same as 3c but blocked, not ignored; a blocked job-control signal makes the read return `EIO` and a handler install does not unblock it.
- **Pros:** Survives `sudo`'s handler (it never touches the mask); measured clean for `bash -c` and `bash -ic`.
- **Cons:** A child may clear its own mask (any program calling `sigprocmask(SIG_SETMASK, empty)` at startup, which some runtimes do) and is then back to 3c.
- **Why not chosen:** `setsid` cannot be undone by the child; there is no reason to pick the weaker one.

### Alternative 4: `serde-saphyr`
- **Description:** Replace `serde_yaml` with the panic-free rewrite.
- **Pros:** Active maintenance, better error reports.
- **Cons:** Different error model and text; the strict-schema suite pins error text, and the `deny_unknown_fields` upgrade-hint wrapping (`cfg/config.rs`) matches on it. A behavior change disguised as a dependency swap.
- **Why not chosen:** `serde_yaml_ng` is the drop-in. Saphyr is the revisit if the fork stalls.

### Alternative 5: Parity test for meta tasks vs clap
- **Description:** Keep `meta_tasks.rs`, add a test that the two param sets agree.
- **Pros:** Smaller diff.
- **Cons:** Keeps 551 lines of copies and a second source of truth; the test fires after drift, derivation prevents it.
- **Why not chosen:** Derived fields never diverge.

## Technical Considerations

### Dependencies
- Removed from `[dependencies]`: `regex` (to dev), `expanduser`, `dirs`, `num_cpus`, `serde_yaml`. Added: `serde_yaml_ng`. Net: four fewer crates in the normal tree, five fewer transitive (`dirs 1.0.5`, `redox_users 0.3.5`, `winapi`, `pwd`, `lazy_static`).
- Toolchain stays 1.96.0 (all three workflows); `std::env::home_dir` compiles under `-D warnings` on 1.96.0 (checked) and `available_parallelism` has been stable since 1.59.

### Performance
- Phase 4's range check is O(1) before an O(n) allocation. Phase 10's stats query is one statement per project instead of 8 per row. Nothing else touches a hot path.

### Security
- Phase 8 narrows nothing and widens nothing about what `Upgrade` trusts; it corrects the wording so the checksum is not read as signature verification (still parked).
- Phase 2's fold removes a data-as-code path: a producer key containing `$(...)` would today land in command position when the consumer sources the `.env` file.
- Phase 7's `calculate_dir_size` symlink skip removes a way for a run directory to point `Clean` at an unreadable tree.

### Testing Strategy
- Every reproduced bug: a test that fails on `f9882ed`, named for the behavior, in the file that owns the surface.
- Every deleted-duplicate: the surviving implementation's tests cover the callers the deleted copies had.
- Doc phases: the success criteria are `grep` and link-resolution commands, run in the phase.
- Break-the-test proof per phase: revert the fix, confirm the new test goes red, restore. Recorded in the implementation notes.

### Rollout Plan
- Per-phase commits on `main`, `otto ci` green before each, per the house flow. One bump after Phase 17 (or after Phase 5 if a release is needed sooner). Release notes name the four runtime behavior changes: the stdin and `/dev/tty` policy for non-`tty` tasks, SIGTERM and SIGHUP cancelling the run (reaping non-`tty` descendants, as SIGINT does), `nargs: "N"`, and the builtin/task mixed-list error.

## Risks and Mitigations

| Risk | Likelihood | Impact | Mitigation |
|------|------------|--------|------------|
| Phase 3's deferral rule changes resolution order for an existing ottofile that accidentally relied on the empty read | Low | Med | Only declared-name references defer; inherited-name reads are pinned by test against the `otto-dev` shape. |
| Phase 5's mixed-list error breaks a script doing `otto build Clean` on purpose | Low | Low | Today that script silently skips `build`; the error is the fix. Release notes name it. |
| Phase 6's derivation misses a clap arg attribute (hidden, env, value parser) | Med | Low | Parity test on every builtin runs inside the phase; `hidden` args are excluded explicitly. |
| Phase 9's immediate transactions serialize concurrent cold starts more than today | Low | Low | Same guard the v0 path already uses; `concurrent_cold_start_test` extended to the upgrade path. |
| Phase 11's sweep unification changes skip provenance for `when:` dependents of up-to-date tasks | Med | Med | It is the intended correction (the divergent copy ignored `when:`); the nine-cell `SkipKind` x `when` matrix in `scheduler_tests_a.rs` is the guard. |
| Phase 15's fork changes an error string a test pins | Med | Low | That is the phase's spike step; pins are updated with the change named in the commit. |
| Anchors go stale between doc and implementation | High | Low | Every bullet is re-anchored by the implementer; the doc says so up front. |

## Open Questions

- [ ] None. The five questions put to panel round 1 were all answered (stdin: the `/dev/tty` shape, now decided; fold: no consumer on the old rule; `nargs`: no reason to keep either meaning; clap derivation: nothing essential lost; ordering: no violation) and folded above. Round 2 recommended listing the Phase 1 mechanism (setsid vs sigmask vs keep `SIG_IGN`) as an open question for the owner; it is instead decided above on measured evidence (`setsid` is the only one of the four the child cannot defeat), with the three rejected mechanisms recorded as Alternatives 3b-3d so the owner can overrule on the record rather than re-derive.

## References

- Review source: six-agent parallel review, 2026-09-02, against HEAD `f9882ed`; per-area reports `review-{cfg,cli,exec,state,misc,docs}.md` in the session scratchpad (line anchors in this doc come from them, verified by command where marked `Observed`).
- `docs/design/2026-06-10-code-review-remediation.md` (predecessor; Alternative 4 and the signature-verification Addendum are inherited)
- `docs/design/2026-09-01-cancellation-reaping-and-foreach-concurrency.md` (introduced the process group Phase 1 corrects the stdin consequence of)
- `docs/design/2026-08-31-buffered-foreach-computed-envs-required-params.md` (introduced `buffer:` and `envs-command`; Phases 2 and 3)
- `docs/design/2026-08-29-strict-ottofile-schema.md` (error-message pins Phase 15 must keep)
- `docs/design/2026-05-24-conditional-task-dependencies.md` (`SkipKind` x `when` contract Phase 11 must keep)
- `docs/design/2026-05-24-paramspec-roundtrip.md` (round-trip invariant Phase 4 extends to `ForeachSpec`)
- RUSTSEC-2024-0320 (`serde_yaml` unmaintained)
- `~/repos/tatari-tv/otto-dev/.otto.yml` (blast-radius check, 2026-09-02)

## Addendum: Parked and Removed Items

- **Release-artifact signature verification.** Parked 2026-06-10 with a revisit condition; re-raised by this review as wording. Wording fixed in Phase 8; the decision is not reopened.
- **Kebab-case key flip.** Parked 2026-08-29; Phase 4's field-shape changes do not trigger its revisit condition.
- **A path argument for `Convert`.** Not requested; help text fix only.
- **`SIGHUP` handling** was parked here in the first draft on a false premise; after panel round 3 it is in Phase 1 (see the SIGTERM and SIGHUP decision). Kept in this list only so the reversal is visible.
- **Makefile converter grammar rewrite.** Rejected 2026-06-10; three heuristics fixed in Phase 12 instead.
