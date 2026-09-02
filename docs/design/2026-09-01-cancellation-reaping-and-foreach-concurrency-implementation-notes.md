# Implementation notes: cancellation reaping, foreach concurrency, and the upgrade cliff

Design doc: `docs/design/2026-09-01-cancellation-reaping-and-foreach-concurrency.md`

Append-only. One section per phase, written as the phase lands.

## Phase 1: Reap the process group on cancel

### Design decisions

- **`CANCEL_GRACE = 500ms`** — `src/executor/scheduler.rs`, beside `OUTPUT_PROCESSING_TIMEOUT_SECS` — the doc fixed the location and the rationale but not the number. Chosen small because the grace exists to let a shell run its `trap`, not to let a child finish work: a cancelled run is already abandoned, otto drains the logs and flushes the buffered blocks itself, so nothing downstream waits on the child. A teardown that outlasts it has the second Ctrl+C (exit 130) as its escape hatch. The alternative considered was the docker-ish 2-10s, which buys nothing here and makes every Ctrl+C feel wedged.
- **Deregistration happens the moment `wait()` returns, not when the body ends** — `src/executor/scheduler/task_execution.rs`, both spawn branches — the two are not the same instant: a grandchild holding the pipe open keeps the output drains waiting up to `OUTPUT_PROCESSING_TIMEOUT_SECS` (5s) after the direct child is reaped. An entry left behind for those seconds names a pid the kernel may have reissued, and it would also have made the Phase 1 (d) scenario unreachable: with a 500ms grace, the body would never have deregistered inside the window the criterion is about. A third, catch-all `deregister_child` after the run block covers the paths that fail before reaching a `wait()`.
- **The signal policy is a value, not a log line** — `signal_child` returns `Result<(), SignalFailure>` and `volume_for(failure) -> SignalVolume`, so "ESRCH is silent, EPERM is loud" is a pure function a unit test asserts on, and the logging sits at the one call site.
- **`own_group` is `cfg!(unix)` in the non-tty branch**, mirroring the `#[cfg(unix)] if !tty { cmd.process_group(0) }` above it exactly. Recording `true` on a platform where otto never created a group would be a registry that lies, even though nothing reads it there.
- **`abort_all` clears the registry and became `async`** — the bodies that would remove their own entries are gone once it runs, so the entries have to go with them.

### Deviations

- **A third stale comment was corrected beyond the two the phase names.** `src/app.rs` (`install_interrupt_handler`'s doc) asserted "They die from `kill_on_drop(true)` when `abandon_run` aborts them, which is the same contract cancellation has always had" — the same false guarantee as `support.rs`, in the third place it was written down. It is the exact sentence Phase 5 corrects in the release post, so leaving it in the source would have re-hidden the bug by the doc's own reasoning ("either one alone re-hides the bug"). Found by grepping the DEAD phrase (`kill_on_drop`) rather than the new one, per the design doc's excellence-pass guard.
- **`flush_cancelled_groups` now takes the task-status map as a parameter, read at the moment of cancellation** — `src/executor/scheduler/replay.rs`, called from `abandon_run`. Not in the phase spec, and it is what keeps the phase behavior-preserving. `plan_cancelled_group` classifies an item as `KilledChild` (the "was killed mid-run, here are its log paths" line) only while its status is `Running`. The reaping now sits between the drain and `abort_all`, and during its grace period a body whose child took the SIGTERM sets its own status to `Failed` on the way out. Reading `task_statuses` inside the flush therefore saw "not Running" and printed `did not start` for subtasks whose children otto had just killed. Caught by `tests/foreach_buffer_cancel_test.rs`, which failed with `otto: say:alpha did not start` where it demands `say:alpha was killed mid-run`. Freezing the map at cancellation time is also the more honest question to ask: the flush describes the run as it was when the user hit Ctrl+C.
- **Ordering inside `abandon_run`: reap AFTER the report drain**, which the doc does not specify. The first attempt put it before, so the reports of the children dying during the grace were drained and replayed a killed subtask as an ordinary failure over its half-written log — the exact thing `BlockKind::KilledPaths` exists to prevent. `abort_all` still runs after the reaping, as the doc requires, so `kill_on_drop` remains the backstop and not the mechanism.

### Tradeoffs

- **Snapshot vs live registry on the SIGKILL pass** — the doc's call, implemented as written, and both break-the-code checks below confirm the live-registry version fails exactly one test and passes the other, which is what makes criterion (d) worth having separately from (a).
- **`tokio::sync::Mutex` for the registry vs `std::sync::Mutex`** — tokio's, matching `task_statuses` in the same file. The std one would have allowed a `Drop`-guard registration, but a `Drop` guard fires at body end, which is the timing already rejected above.
- **The tty case leaks its own grandchildren, and this phase does not change that.** A `tty: true` task stays in otto's process group by design, so cancellation signals its pid alone; anything it forked is unreachable without signalling otto. Accepted: it is the cost of the terminal-ownership carve-out, and the alternative (a group signal) is the failure criterion (c) tests for.
- **Three integration tests are Linux-only** (`/proc/<pid>/cmdline`), while the mechanism is `#[cfg(unix)]`. The unit tests in `src/executor/scheduler_tests_b.rs` cover the mechanism on any unix, including the grace-window property, by asserting the child died of signal 9 rather than by reading `/proc`.

### Verification

Success criteria from the design doc, Phase 1:

- **(a) PASS** — `tests/cancel_reaping_test.rs::a_cancelled_run_reaps_every_task_bodys_grandchildren`: parallel foreach, two bodies each forking `sleep 601`, literal `0x03` through a real `script` pty, both grandchild pids checked by `/proc/<pid>/cmdline` content before and after.
- **(b) PASS (break-the-code, run 2026-09-01, code restored)** — with both `signal_snapshot` calls removed from `reap_live_children`, that test failed with `cancellation must reap the whole task subtree, but these grandchildren outlived it: ["1187398 -> Some(\"sleep 601\")", "1187401 -> Some(\"sleep 601\")"]`, matching the design doc's measured baseline. `a_grandchild_that_ignores_sigterm_is_reaped_after_the_grace_window` failed at the same time with `saw Some("sleep 602")`.
- **(c) PASS** — `cancelling_a_tty_task_signals_the_task_and_not_otto_itself`: the tty task ignores SIGINT so it is still registered when `abandon_run` runs; otto reaches its ordinary cancelled exit and prints `run cancelled`, rather than exiting `-1` (killed by a signal) or `143`.
- **(d) PASS, and pinned twice** — `a_grandchild_that_ignores_sigterm_is_reaped_after_the_grace_window` end-to-end, plus `cancellation_reaping::the_sigkill_pass_reaps_a_group_whose_registry_entry_vanished_mid_grace`, which empties the registry partway through the grace and asserts the child died of `SIGKILL`. **Second break-the-code check, run 2026-09-01, code restored:** with the SIGKILL pass re-reading the live registry instead of the snapshot, `a_cancelled_run_reaps_every_task_bodys_grandchildren` still PASSED and only the grace-window test failed (`saw Some("sleep 602")`) — which is precisely why the doc asked for (d) as its own criterion.

`otto ci` green, twice: `✅ All CI checks passed!` with coverage 92.0% (22490/24448 lines) against the 87% threshold.

### Open questions

- **A pre-existing test-isolation flake in `src/cfg/`, not caused by this phase and not fixed by it.** One `otto ci` run failed with `cfg::param::tests::resolve_choices_command_returns_trimmed_non_empty_lines` and `..._zero_lines_is_an_error_unlike_foreach` reporting `'switch:svc' is already being resolved by an outer otto invocation (OTTO_CHOICES_COMMAND=switch:svc)`. Root cause: `src/cfg/resolver_tests.rs:136` (`run_lines_command_refuses_to_recurse_on_the_same_choices_key`) sets that variable process-wide with `std::env::set_var` and is not `#[serial]`, so under `llvm-cov`'s slower instrumented run it overlaps the two tests that read it. The fix is one attribute in a file this phase does not touch; flagging rather than taking it. Two subsequent `otto ci` runs were green.
- **The tty grandchild leak named under Tradeoffs** is a real remaining hole in "a cancelled run leaves no descendant alive" (the doc's first acceptance criterion), scoped out by the terminal-ownership carve-out. Worth confirming that is intended to stay open.

## Phase 2: `foreach.jobs` schema and validation

### Design decisions

- **`ForeachJobs` gets a hand-written `Deserialize`/`Serialize` pair, not a derive with an untagged enum** — `src/cfg/task.rs` — the same shape `Nargs` already uses one file over (`src/cfg/param.rs`) for a keyword-or-value field. A `Visitor::deserialize_any` lets `jobs: all` and `jobs: 4` share one field with no wrapper syntax, and overriding only `visit_str`/`visit_u64` and leaving every other `visit_*` at its trait default is what makes the negative/float/bool rejections "already loud" for free: the default impl builds its "invalid type" message straight from `expecting()`, so there is no bespoke error text to maintain for shapes the doc explicitly does not want bespoke text for.
- **Only one custom load-time validator was needed: `Parser::validate_foreach_jobs`** (`src/cli/parser/config.rs`), checking `jobs.is_some() && !parallel`. The API Design table lists four rejected shapes, but the other three turn out to need zero custom code, verified empirically before writing anything (see Deviations): `jobs: 0` is rejected inside `ForeachJobs::deserialize` itself with the "write `all`" message; negative/non-integer is the visitor-default "invalid type" error; and `jobs` written as a sibling of `foreach:` (not nested in it) is already rejected by `TaskSpecHelper`'s existing `deny_unknown_fields` - it has no `jobs` field, and never gains one, so this shape can't reach a post-parse validator at all.
- **The task path is free from `serde_yaml`, not synthesized.** Empirically confirmed (three throwaway probe tests, since deleted): `serde_yaml` prefixes an error raised from any depth of a nested struct with the dotted path to that struct, e.g. `tasks.logs.foreach.jobs: foreach jobs: 0 is not a valid count...`. Every one of the three deserialize-time rejections therefore already names the task path with no wrapping needed; only the cross-field `validate_foreach_jobs` check needed an explicit `Task '{task_name}':` prefix, matching `validate_foreach_buffer`'s shape.
- **`ForeachSpec::jobs` doc comment is copied verbatim from the design doc's Data Model section**, per the task brief - it deliberately does not claim the group holds a shared permit (that idea was killed in panel round 1).

### Deviations

- **Empirically verified, not assumed, that "jobs with no foreach" needs no code.** The design doc's API Design table lists it as one of four "load-time rejections... in the shape `validate_foreach_buffer` already uses," which reads as if it wants a custom validator. Written as a throwaway integration test first (`tasks.hi: unknown field 'jobs', expected one of ... at line N column M`) before concluding the existing `deny_unknown_fields` on `TaskSpecHelper` already produces this, naming the task path, with no `jobs` field ever added there. Adding one would have been the wrong seam anyway: it would have grown `TaskSpecHelper`'s own key count and forced a THIRD inventory-test literal, which the task brief said does not exist on `main` (verified: only `src/cfg/task_tests.rs:992` and `:1057` needed edits).
- **Same-effect, correct seam: relied on `serde_yaml`'s built-in path-prefixing for three of the four rejections' "names the task path" requirement**, rather than wrapping every `ForeachJobs` parse error in a second layer that re-adds a task name the error already carries. Confirmed by direct probe (see Design decisions) rather than assumed from the doc's prose.

### Tradeoffs

- **`ForeachJobs::Fixed` wraps `NonZeroUsize` rather than `usize` with a runtime zero-check** — matches the design doc's literal signature and makes "zero" structurally unrepresentable in the success path, so every call site that pattern-matches `Fixed(n)` gets a value already known positive, with the "0 means write `all`" message living in exactly one place (the deserializer).
- **`visit_i64` was deliberately left un-overridden** rather than added for a friendlier negative-number message, since the design doc calls the negative/non-integer case "serde type error, already loud" - writing custom text there would have been unrequested scope beyond what criterion (a) needs (fails to load, names the task path, does not panic - all satisfied by the default).

### Verification

Success criteria from the design doc, Phase 2:

- **(a) PASS** — `tests/foreach_jobs_test.rs`: all four rejected shapes (`jobs` + `parallel: false`, `jobs` outside `foreach:`, `jobs: 0`, and `jobs` as `-3`/`1.5`/`sometimes`) fail to load with a non-zero exit, name the task and/or key, and none panics (checked directly: asserted stderr never contains `"panicked"`). `jobs: all` + `buffer: true` and a bare `jobs: 4` both load cleanly, pinning the two legal shapes beside the four illegal ones.
- **(b) PASS** — built the release binary at this phase's tip and at the unstashed prior commit (`b5eea2b`), ran both against an identical ottofile (foreach + params + a dependent task, no new keys) through `--tasks --format json`, and diffed by SHA-256: `f12926a4ea413b3caa9027d80f8e16422a35992ed3d550c3cb802ea2cb334ad8` on both sides, byte-identical.
- **(c) PASS** — `ottofile_reference_key_inventory_is_exhaustive` (`src/cfg/task_tests.rs`) passes with the `ForeachSpec` per-struct count at 9 (`:1013`) and the total at 46 (`:1058`), both edited in this phase's commit; `docs/commands/ottofile-reference.md` gained the `tasks.<name>.foreach.jobs` row, its section header, and its `## Total:` line in the same commit.

`otto ci` green, run twice independently (parallel background runs, no shared state): both printed `✅ All CI checks passed!`, coverage 92.0% (22562-22574 / 24532 lines, threshold 87%), `cargo fmt --check` clean after `fmt-fix` auto-applied one reflow to the new test file.

### Open questions

None.
