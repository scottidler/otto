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

## Phase 3: Scheduler honors `foreach.jobs`

### Design decisions

- **Admission is a three-arm enum stored ON the in-flight set, not recomputed** — `Admission::{Capped, Tty, Exempt}` in `src/executor/scheduler.rs`, with `ActiveTasks::running` going from `HashSet<String>` to `HashMap<String, Admission>` — the loop's view of what is in flight is then literally the decision it made when it admitted each task, rather than a re-derivation from `Task` fields that a later mutation could make disagree with it. Everything the two rules read (`capped_len`, `in_flight`) is derived from that one map, so there is no second piece of state to keep in sync — which is the failure mode the killed `AtomicUsize` design had.
- **`may_admit(class, in_flight)` is a pure function over a value** (`InFlight { tty, exempt }`), not a method that reads `self` — that is what let the whole rule table be pinned by a unit test with no scheduler, no runtime and no timing (`the_admission_rules_are_symmetric_and_bind_only_tty_against_exempt`), and it is why criterion (e)'s break-the-code is a one-line edit with an unambiguous blast radius.
- **`jobs: all` is resolved to a number at expansion time, not carried as a keyword** — `ForeachJobs::permits(item_count)` (`src/cfg/task.rs`), called from `expand_foreach_tasks_with_serial` (`src/cli/parser/foreach.rs`) — that is the only place where both the keyword and the item list exist at once; the scheduler sees subtasks and never sees the `foreach:` block that produced them. Every item of the group is stamped with the same `NonZeroUsize`, so the group's size is decided once rather than per body.
- **The stamp lands on the ITEMS and deliberately not on the virtual parent** (`src/cli/parser/discovery.rs`, keyed by `naming::parent_of`, which returns `None` for a parent). The parent is queued only once its items are terminal (`foreach.rs`'s `When::Always` edges), so it never runs beside them; exempting it would be a concurrency carve-out for a task that cannot use one. Pinned by `foreach_jobs_is_stamped_on_every_item_and_never_on_the_virtual_parent`.
- **Per-group semaphores are built once in `TaskScheduler::new`, never lazily in a body** — a body that had to create the semaphore it then acquires from would race every sibling item, and the group's bound would be whichever body won. `group_semaphore()` fails closed (an `Err`, not a fallback to the shared semaphore): borrowing a global permit for an item classified exempt would quietly reinstate the starvation this key exists to remove.
- **The launch cap moved from the loop's condition into a per-task check, guarded by a `has_exempt_items` bool computed once.** An exempt item has to be admitted past a full cap, which the old loop condition cannot express; but a run with no `foreach.jobs` still breaks out of the pass at exactly the same point, on exactly the same condition, so those runs keep today's shape and today's cost.
- **A task the rules will not admit yet goes back to the HEAD of the ready queue, not into `blocked_tasks`.** The blocked sweep is gate-driven and would never look at an admission-deferred task again. Head, not back: a deferral is "not yet", not a demotion behind tasks that became ready later.

### Deviations

- **Same effect, correct seam: `ActiveTasks::len()` was split rather than changed.** The doc names the cap as `active_tasks.len() < max_concurrent`, but one number can no longer answer both questions truthfully once items stop counting against the cap: `abandon_run` uses the same call to say how many tasks it is killing. It is now `capped_len()` for the cap and `in_flight_len()` for cancellation, with the doc comments saying which is which.
- **The per-group semaphore is selected in `execute_task`, not inside the body.** The doc says items "acquire from a per-group `Semaphore::new(N)`"; the acquire at `task_execution.rs`'s `semaphore.acquire_many(permits)` is untouched and simply receives whichever semaphore the task's class selected, with `permits = 1` for an exempt item. `permits_for` and the shared semaphore's tty handling are unchanged, so the FIFO sentence above that line still describes what happens there.
- **`tty: true` together with `foreach.jobs` is expressible and the doc does not say what it means.** Subtasks are clones of the task spec, so a foreach task carrying `tty: true` hands every item both requests, which are opposite: exclusive ownership of the terminal cannot be shared out one permit per item. `admission_for` resolves it as tty-wins (the conservative half - it never puts two writers on one terminal) and `warn_on_tty_with_foreach_jobs` says so once per run rather than resolving it silently. A load-time rejection would be the better answer and it belongs at Phase 2's seam, not this one; see Open questions.
- **One test beyond the six criteria: `without_jobs_the_same_fixture_starts_only_the_cap`.** Criterion (a) alone would pass on a build where the fixture never blocked in the first place; the control runs the identical fixture with the `jobs:` line deleted and asserts exactly two items start and no third ever does. It is the in-tree version of the design doc's `Observed on main: started_count=2`.

### Tradeoffs

- **Head-of-line skipping vs strict FIFO admission.** A deferred task does not block the ones behind it, so an exempt group still starts while a tty task waits at the head. The cost is that a tty task can starve behind a continuous stream of exempt items - which the doc already accepts and states ("the cost of asking for the exemption", and `logs` is terminal in a run by construction). The alternative, stopping the pass at the first deferral, would have made criterion (a) fail as soon as a tty task shared the run.
- **Queue scan cost in runs that use the key.** With an exempt item present and the cap full, each pass now pops the whole ready queue, classifies gates, and puts the deferred tasks back, instead of stopping at the cap. Bounded to runs that actually declare `foreach.jobs` by `has_exempt_items`; the alternative (an index or a per-class queue) is more state to keep correct for a case the profile does not yet justify.
- **`HashMap<String, Admission>` with derived counts vs two maintained counters.** Counters would make `in_flight()` O(1) instead of O(tasks in flight), at the price of two more things that can disagree with the map. The map is the single truth and the counts are read off it; the set it walks is bounded by `-j` plus the exempt groups' sizes.
- **The concurrency tests drive real otto processes over fifos rather than exercising the scheduler in-process.** Slower and they need cleanup discipline (every fifo is released `O_NONBLOCK` on drop, or a failed assertion would leave shells blocked forever), but an in-process test would have proved the rules and not the wiring - and the wiring (two gates, not one) is exactly where the first two versions of this design were wrong.

### Verification

Success criteria from the design doc, Phase 3. End-to-end tests are in `tests/foreach_jobs_concurrency_test.rs`; unit tests in `src/executor/scheduler_tests_b.rs::foreach_jobs_admission`.

- **(a) PASS** — `jobs_all_starts_every_item_past_the_global_launch_cap`: 10 items, `parallel: true`, `jobs: all`, `otto -j 2`, every body blocked on its own fifo; all 10 write their start marker. The control (`without_jobs_the_same_fixture_starts_only_the_cap`) reproduces the defect on the same fixture: exactly 2 start and no third appears while those two block.
- **(b) PASS** — `jobs_fixed_starts_exactly_n_and_the_next_only_after_one_exits`: with `jobs: 4`, four items start, a fifth never appears while all four block, and the fifth appears after exactly one item's fifo is released. The fifth starting is a permit moving, not time passing.
- **(c) PASS** — `a_tty_task_and_an_exempt_group_never_overlap_at_one_job`, run at `-j 1`: whichever side the loop admits first, the other writes no marker while the first is in flight, and starts as soon as the first is released. Barrier files throughout; the only duration is the 750ms window each negative claim is polled over.
- **(d) PASS, both halves.** `a_tty_task_becoming_ready_during_an_exempt_group_waits_for_it`: a capped gate task returns only once all three exempt items are running (it can run beside them at `-j 1` precisely because they are exempt), so the tty task behind it becomes ready mid-group; it starts only after the group drains. `an_exempt_group_becoming_ready_during_a_tty_task_waits_for_it`: the mirror, sequenced structurally rather than by timing - `pre` reporting is what readies the tty task, and the launch pass that admits it runs before the loop can consume the next report, so releasing `hold` afterwards readies the group with the tty task already counted in flight (and still queuing for its permits). The group starts only after the tty task is released.
- **(e) PASS, break-the-code, run 2026-09-01, code restored after each.**
  - Rule 1 deleted (`Admission::Tty => true`): `a_tty_task_and_an_exempt_group_never_overlap_at_one_job` failed with `no exempt item may start while the tty task owns the terminal: ["s1", "s2", "s3"]`, and `a_tty_task_becoming_ready_during_an_exempt_group_waits_for_it` failed with `a tty task that became ready during an exempt group must wait for it`. The other four passed.
  - Rule 2 deleted (`Admission::Exempt => true`): `a_tty_task_and_an_exempt_group_never_overlap_at_one_job` failed the same way and `an_exempt_group_becoming_ready_during_a_tty_task_waits_for_it` failed with `the group became ready during the tty task and must wait for it: ["s1", "s2", "s3"]`. The other four passed.
  - Extra, for the half of the fix the doc says is easy to miss: `Admission::is_capped` forced to `true` (the launch-cap exemption deleted, per-group semaphore left in place) failed criterion (a) with `jobs: all must start every item regardless of the global cap; started 2 of 10: ["s06", "s10"]` - the design doc's measured `started_count=2`, reproduced in-tree. Fixing only the semaphore really does change nothing.
- **(f) PASS** — `spawn_counts_a_task_in_flight_before_its_body_acquires_a_permit`: a body is spawned against a `Semaphore::new(0)` it can never acquire from, the test yields until the body has parked on that acquire, and then asserts the body has NOT acquired, that `in_flight()` already reports `tty: 1`, and that `may_admit(Admission::Exempt, ...)` is therefore false. That is the property the doc names as load-bearing, asserted through its consequence rather than through the field.

`otto ci` green, run twice: `✅ All CI checks passed!`, coverage 92.0% (22805-22812 / 24784 lines across the two runs) against the 87% threshold. The first run failed `fmt-check` only (three `wait_for` closures wanted reflowing); `cargo fmt` applied, no other change.

### Open questions

- **Should `tty: true` combined with `foreach.jobs` be a load error?** It is expressible today, the two keys ask for opposite things, and this phase resolves it as tty-wins plus a `warn!`. A fifth entry in Phase 2's rejection table would be the honest answer (`fail loudly`), and it is a one-validator change beside `validate_foreach_jobs` - but adding it here would be reaching into a phase that is already committed, so it is surfaced rather than taken.
- **The starvation the doc accepts is now real and observable:** a `jobs: all` group of never-exiting items blocks every later `tty: true` task for as long as the group runs, and head-of-line skipping means later exempt items keep starting past the waiting tty task. The doc calls this "not a regression" and the cost of the exemption; worth confirming that is still intended now that it is code rather than prose.

## Phase 4: An actionable unknown-field error

### Design decisions

- **`wrap_unknown_field_error` lives in `src/cfg/otto.rs`, beside `check_api_version`** (`src/cfg/otto.rs:97-127`) — both are "make an ottofile-load failure name the upgrade" mechanisms, and both are read together by anyone auditing `SUPPORTED_API_VERSIONS` policy, so the doc comment for one can point at the other without a module hop.
- **The wrapper is a plain string match on `"unknown field"`, not a `serde_yaml::Error` downcast** — `serde_yaml::Error` exposes no structured "this was a `deny_unknown_fields` rejection" variant to match on; the message text is the only signal serde gives, and it is the same text `deny_unknown_fields_names_a_misspelled_otto_task_key` and its siblings already assert on, so the repo already treats that substring as stable.
- **Wired at the single call site, `src/cli/parser/config.rs`'s `load_config_from_path`** — `serde_yaml::from_str::<ConfigSpec>(&content).map_err(wrap_unknown_field_error)?` replaces the bare `?`. This is the only place an ottofile's typed parse happens, so every load path (`--tasks`, a real run, `otto doctor`) gets the wrapper for free rather than needing its own call.
- **The trailing line hedges both explanations by construction, not by inspecting the key** — otto cannot tell "new key from a newer otto" from "typo" (see doc's Phase 4 rationale), so the wrapper does not try; it states both possibilities and names the one fix that helps if the first is true (`otto Upgrade`), satisfying success criterion (b) without a heuristic that could be wrong out loud.

### Deviations

None. The change is exactly what the doc specifies: wrap the strict-parse `unknown field` error with a trailing line, no api bump.

### Tradeoffs

- **String-match on `"unknown field"` vs a custom `Deserializer` wrapper that tags the failure kind at the point of rejection.** The custom-deserializer route would require re-deriving `deny_unknown_fields`' rejection by hand for both `ConfigSpec` and every nested `#[serde(deny_unknown_fields)]` struct, to attach a typed marker serde does not provide. The string match costs a substring check on an already-formatted error and nothing else; it is wrong only if serde ever changes its wording, which the existing negative tests would also break on first.
- **Passing every other `serde_yaml::Error` through unchanged, rather than also softening type-mismatch or missing-field messages.** The doc's Phase 4 scope is the unknown-field path specifically ("An actionable unknown-field error"); widening it to every load failure would be scope creep this phase does not need, and `wrap_unknown_field_error_is_a_noop_for_other_serde_failures` pins that boundary.

### Verification

Success criteria from the design doc, Phase 4. Unit tests in `src/cfg/otto_tests.rs`; manual confirmation against the built binary below.

- **(a) PASS** — `wrap_unknown_field_error_names_the_key_and_the_upgrade_fix` asserts the wrapped message contains both the offending key and `otto Upgrade`. Confirmed against the real binary: `otto -o <fixture with foreach.totally-fake-key> --tasks` prints `tasks.hi.foreach: unknown field \`totally-fake-key\`, expected one of ...` followed by `this key is either new to a newer otto than this binary, or simply misspelled in the ottofile; if the ottofile targets a newer otto, run \`otto Upgrade\` to update this binary`, `rc=1`.
- **(b) PASS** — `wrap_unknown_field_error_hedges_a_genuine_misspelling` uses `tsaks:` (never a real otto key) and asserts the message contains both "misspelled" and "newer", i.e. it does not assert out-of-date as the only explanation. Confirmed against the real binary: same trailing line, same hedge, `rc=1`.
- **(c) PASS** — `supported_api_versions_stays_exactly_one` asserts `SUPPORTED_API_VERSIONS == &["1"]`. `src/cfg/otto.rs:27` is untouched: `pub const SUPPORTED_API_VERSIONS: &[&str] = &[CURRENT_API_VERSION];`.
- Extra test beyond the three criteria: `wrap_unknown_field_error_is_a_noop_for_other_serde_failures`, pinning that a type-mismatch error (`otto.jobs: not-a-number`) passes through byte-identical, so the wrapper's reach is bounded to unknown-field rejections.

`cargo test --workspace --all-features`: 864 unit tests + all integration binaries green (44 test binaries, 0 failed). `cargo fmt --all` and `cargo clippy --workspace --all-features --all-targets`: both clean, no warnings.

**Reduced gate, by explicit user decision for Phases 4 and 5 (overriding the doc's Testing Strategy line "Every phase carries a break-the-code check"):** no break-the-code check was run for this phase, and coverage was not measured. The gate used was `cargo test --workspace --all-features` plus `cargo fmt`/`cargo clippy`, not `otto ci`. The orchestrator runs one full `otto ci` before finalization.

### Open questions

None.

## Phase 5: Docs, example, and the release-post correction

### Design decisions

- **The `jobs` section in `docs/commands/buffered-foreach.md` documents behavior, not schema.** `docs/commands/ottofile-reference.md` already carries the one-line schema row (added in Phase 2); this section's job is the same one `buffer`'s section already does above it in the file — worked example, the accepted consequence stated rather than left for the reader to discover, and the exact rejection error text quoted so a load failure is recognizable. Quoted directly from `src/cli/parser/config.rs::validate_foreach_jobs` and `src/cfg/task.rs`'s `ForeachJobs::deserialize`, read at their current lines, rather than paraphrased, so the doc cannot drift from the message a user actually sees.
- **The `tty: true` + `foreach.jobs` resolution is documented as Phase 3 actually built it (tty wins, warned once per run), not as the design doc's Architecture section describes it (silence).** Read `admission_for` and `warn_on_tty_with_foreach_jobs` (`src/executor/scheduler.rs`) directly and quoted the real `log::warn!` text, since the Phase 3 notes record this resolution as a deviation the design doc's prose doesn't cover.
- **The example's `jobs:` line is commented out, not live**, matching the brief ("gains a commented `jobs:` line") and because `foreach-buffer`'s items are static strings, not never-exiting bodies — turning `jobs: all` on for real would change nothing observable and would misrepresent the feature's actual use case (a log tail, a watcher, a dev server).

### Deviations

- **Criterion (c) is not satisfied and cannot be, this phase.** The design doc's Phase 5 success criterion (c) is `marquee read <URL> | grep -c 'They die from'` returning 0, with the replacement naming the process group. Publishing to the marquee post is outward-facing and the task brief explicitly withholds that authority from this phase ("DO NOT publish or modify the marquee release post... The user must approve that separately"). Reported as DEFERRED, not PASS or FAIL: the work (finding the exact current sentence, drafting the replacement) is done and staged in `docs/design/2026-09-01-marquee-post-correction.md`; only the `marquee update` call is withheld pending approval.
- **`marquee read` could not be run against the live post to re-verify the current sentence.** It failed: `Error: authentication failed: Okta token is missing or expired and no controlling terminal is available (non-interactive session)`. No interactive terminal is available in this session to complete the device-grant login. The correction file uses the sentence as quoted, with citation, in the design doc's own Problem Statement (finding 1) — measured against the post before that doc was written — and states plainly that it was not independently re-fetched this phase. Reported honestly per the task brief ("If `marquee read` fails, say so... rather than guessing the wording") rather than treated as a completed verification.
- **Reduced gate, by explicit user decision for Phases 4 and 5:** no break-the-code check was run (there is no code in this phase to break — docs, an example comment, and a staged correction file) and coverage was not measured. The gate used was `cargo test --workspace --all-features` (864 unit tests + all integration binaries, 0 failed) plus `cargo fmt --all` (no changes needed) and `cargo clippy --workspace --all-features --all-targets` (clean, no warnings). The orchestrator runs one full `otto ci` before finalization.

### Tradeoffs

- **Quoting the exact validator error text in the doc vs. paraphrasing it.** Quoting risks the doc going stale if the message wording changes later with no matching doc update; paraphrasing risks describing a message the user never actually sees. Chose quoting: `buffered-foreach.md`'s `buffer` section above it already sets this precedent (it quotes the exact truncation-warning line verbatim), so this keeps the file's existing convention rather than starting a new one.
- **A separate `marquee-post-correction.md` file vs. inlining the correction into these implementation notes.** A standalone file is the artifact Scott approves and then acts on directly (`marquee update` takes post text, not a notes excerpt buried in a phase section); these notes instead point at it. The implementation notes are an append-only project record, not a staging area for outward-facing content that will itself be edited before use.

### Open questions

- **The proposed replacement sentence for the marquee post needs Scott's approval before publishing**, since it is outward-facing. Staged verbatim, with the current sentence, citation, and reasoning, in `docs/design/2026-09-01-marquee-post-correction.md`. It also needs a final `marquee read` re-fetch (after `marquee login`, since this session's Okta token was unavailable non-interactively) to confirm section 5 still contains the sentence exactly as recorded, before publishing.
