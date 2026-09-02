# Design Document: Cancellation reaping, foreach concurrency, and the upgrade cliff

**Author:** Scott A. Idler
**Date:** 2026-09-01
**Status:** Implemented
**Review Passes Completed:** 5/5 (draft, correctness, clarity, edge cases, excellence), plus review-panel round 1 (Architect + Staff Engineer) and its post-round synthesis, every finding dispositioned below

## Summary

Reviewing `tatari-tv/otto-dev#15` (the v2.1.0 adoption) turned up three otto defects and two otto-dev items. The headline: otto's cancellation kills the direct task child and orphans everything below it, which the code comment at `task_execution.rs:200-204` already claims to have solved. This doc fixes that, adds a per-group concurrency override so a group of never-exiting tasks cannot starve itself against the global `-j` cap, and makes otto's unknown-key error name the upgrade from this release forward. It does NOT rescue anyone already on v2.0.5: no otto change reaches a published binary, so that gap is closed on the otto-dev side by a version check in `bin/ttv`.

## Problem Statement

### Background

otto v2.1.0 shipped `foreach.buffer`, `otto.envs-command`, and `params.<name>.required` to unblock `tatari-tv/otto-dev`. Their adoption PR (#15, merged) deleted a ~90-line hand-rolled scheduler from `scripts/stack.sh`, a 100-line generator, a drift gate, two marker pairs, and every `:?` usage guard. Every deletion is correct.

Two of those deletions handed otto duties otto does not actually discharge, and a third invocation shape now hits an upgrade cliff with no guidance. All five findings below were measured on 2026-09-01 against the merged `otto-dev@main` tree with both a v2.0.5 and a v2.1.0 binary, not inferred.

### Problem

**1. Cancellation orphans grandchildren.** `abandon_run` calls `ActiveTasks::abort_all` (`scheduler.rs:292-296`), which aborts the tokio join handles. Dropping each `Child` fires `kill_on_drop(true)`, which is a SIGKILL to the **direct** child only. Every task child is already made a process-group leader (`task_execution.rs:206-208`), and the comment above it says the reason is "so the child's own children are reachable as a group". Nothing ever reaches them: `grep -rn 'killpg|Pid::from_raw(-' src/` returns zero hits.

Measured, parallel foreach, task body spawns a grandchild, real pty Ctrl+C:

```console
grandchild pids: ['57', '58'] cmdlines BEFORE: ['sleep 600', 'sleep 600']
  pid 58 AFTER Ctrl+C -> cmdline='sleep 600'
  pid 57 AFTER Ctrl+C -> cmdline='sleep 600'
```

This is not academic for otto-dev: their `logs` task body runs `otto -C "$root" --no-prefix logs | filter_noise`, so each item is bash -> inner otto -> `docker compose logs`. Ctrl+C leaves the inner ottos and the compose tails running, once per service, every time. The `trap 'kill ${pids}' INT TERM EXIT` they deleted was doing real work.

It also falsifies a sentence in the v2.1.0 release post: "They die from `kill_on_drop(true)` when `abandon_run` aborts them." True for the direct child, false below it.

**2. A parallel foreach of never-exiting tasks starves against `-j`.** otto-dev sets `otto.jobs: 8`; the registry has 10 services; `status|logs` deliberately skip the per-verb registry filter (`stack.sh:259-263`), so all 10 participate. A `docker compose logs` tail never exits, so the queued items never get a permit.

```console
$ otto logs        # 10 items, parallel, jobs: 8, each body blocks forever
tails that actually started: 8 of 10
s1 s2 s3 s5 s6 s7 s8 s9
```

Silent. No error, no notice, no missing-item line. The developer watches 8 streams and does not learn that 2 are absent. `status` is unaffected because those commands exit and permits recycle.

There is no way in an ottofile today to say "every item in this group must run at once, the global cap does not apply here." The global `-j` is the only knob and it is repo-wide.

**3. The pre-2.1.0 upgrade cliff has no guidance.** Strict ottofile parsing (`deny_unknown_fields`, shipped v1.4.0) means a v2.0.5 binary rejects the whole file on the first new key:

```console
$ otto --tasks          # v2.0.5, against otto-dev@main
otto: unknown field `envs-command`, expected one of `name`, `about`, `api`, `jobs`, `tasks`, `envs`, `retention` at line 59 column 3
$ echo $?
1
```

Every task dies the same way, including `otto doctor`, which is the task that wraps both of otto-dev's version-floor checks (`.otto.yml:130-131` calls `scripts/bootstrap.sh --check-only` and `scripts/doctor.sh`). The friendly "your otto predates the floor" message cannot reach the person who needs it through the entry point they would reach for.

otto already ships the correct mechanism and never uses it. `check_api_version` (`src/cfg/otto.rs:60-77`) runs **before** the typed parse against a loose `ApiHeader`, so unknown sibling keys do not break it:

```console
$ otto hi               # v2.0.5, fixture declaring api: 2
otto: unsupported api version '2' (this otto supports: 1). upgrade otto.
```

`SUPPORTED_API_VERSIONS` has only ever contained `"1"`, so no ottofile can declare a newer generation and reach that message. **That is a deliberate policy, not an oversight,** and this doc leaves it alone: `src/cfg/otto.rs:20-26` says a new api version is for a change a prior otto would MIS-EXECUTE, not for an added optional key, and every key at issue is additive. The gap is closed on otto-dev's side instead. See Alternative 5.

**4. otto-dev has no floor check on the path everyone uses.** `bin/ttv` validates that `.otto.yml` exists and that `otto` is on PATH, then `exec otto -C "$ROOT" "$@"`. It does not compare `otto --version` against `OTTO_DEV_MIN_OTTO`, and `otto --version` needs no ottofile. That check is the one thing that could turn finding 3 into words today, for people already on an old binary.

**5. A stale premise in otto-dev's own comment.** `scripts/lib.sh:130-135`, unchanged by #15, says an old otto "doesn't reject keys it predates, it ignores them and runs something else entirely." That was true before v1.4.0. It is the source of #15's risk-section claim that the new keys are "silently ignored by an old binary", which is the opposite of measured behavior.

### Goals

- Cancellation reaps the whole task subtree, not just the direct child.
- An ottofile can declare that a foreach group needs one permit per item.
- From this release forward, otto's unknown-key error names the upgrade rather than only the key. For binaries already published, otto-dev's `bin/ttv` check is what says it.
- otto-dev's `logs` survives Ctrl+C with no orphans and no starved services.
- Every nit from the #15 review is dispositioned in this doc: fixed, or recorded as no-action with the reason.

### Non-Goals

- **A general job-control or process-supervision layer.** One grace period, one group signal. Excluded, not parked.
- **Changing what `-j`/`otto.jobs` mean globally.** The new key is an override at one seam, not a redefinition.
- **Retrofitting the good upgrade message onto already-released binaries.** Impossible: v2.0.5 is published, and no otto change reaches it. That gap is covered on the otto-dev side by the `bin/ttv` floor check. Excluded, not parked.
- **Windows.** otto's process-group handling is already `#[cfg(unix)]`. Excluded.
- **`--tasks` laziness.** See Alternatives; the resolution is contractual, not waste.

## Proposed Solution

### Overview

Three otto changes, one otto-dev change, and one correction to a published post, in that dependency order.

| # | Repo | Change |
|---|---|---|
| 1 | otto | Reap the task's process group on cancel: SIGTERM to `-pgid`, grace, SIGKILL to `-pgid` |
| 2 | otto | `tasks.<name>.foreach.jobs: all \| <N>` |
| 3 | otto | The unknown-field parse error names the upgrade (NOT an api bump: see Resolved Decisions) |
| 4 | otto-dev | Version floor check in `bin/ttv` before `exec`; `foreach.jobs: all` on `logs`; the stale comment corrected in BOTH `scripts/lib.sh:132-135` and `CLAUDE.md:49-52` |
| 5 | both | Correct the release post's teardown sentence |

### Architecture

**Group reaping.** The child handles live inside the spawned task bodies, not in the scheduler, so the scheduler cannot reach them today. Add a live-child registry the body writes into:

- `Arc<Mutex<HashMap<String, ChildHandle>>>` hung off `ActiveTasks`, where `ChildHandle { pid: u32, own_group: bool }`. Conceptually it belongs there rather than as disconnected bookkeeping: `ActiveTasks` is already the answer to "what is in flight".
- The body inserts after spawn and removes on exit, in both the success and failure paths.
- **`abandon_run` takes a SNAPSHOT of the registry before signalling anything, and both passes iterate the snapshot.** For each `own_group` entry: `killpg(pid, SIGTERM)`; sleep `CANCEL_GRACE`; `killpg(pid, SIGKILL)` over **the same snapshot**, not over whatever is still in the live registry. Entries with `own_group: false` (a `tty: true` task, which deliberately stays in otto's group) get a direct `kill(pid, ...)`, never a group signal: otto is in that group and would signal itself.
- `abort_all` still runs after, so `kill_on_drop` remains the backstop rather than the mechanism.

**Why the snapshot, spelled out, because the obvious version is wrong.** Panel round 1, both seats independently: if the SIGKILL pass asks the live registry for "anything still present", it misses the exact case the pass exists for. The `CANCEL_GRACE` sleep yields to the executor; the direct child (which took the SIGTERM) exits; its body wakes and removes its own registry entry; a SIGTERM-ignoring grandchild is still alive in a process group that is still valid. The second pass then finds an empty registry and signals nothing. The fix is free: capture the pgid list once, up front, and use it twice.

A process group outliving its leader is fine and was measured by the staff seat: leader 9, child 10, `pgid 9`; after the leader exits the child is still in group 9; `kill -TERM -9` then reaps it. So the only ordering hazard is the registry one above, not a stale-pgid one. Pid reuse inside `CANCEL_GRACE` remains theoretically possible and is accepted: the window is one grace period on a pid otto itself created and has not reaped.

Signal errors: `ESRCH` on the second pass is the expected success case (the group already died) and is ignored. `EPERM` is loud, since it means otto is signalling something it does not own.

`CANCEL_GRACE` is a hard-coded const beside `OUTPUT_PROCESSING_TIMEOUT_SECS` (`scheduler.rs:35`), matching that precedent. It is teardown timing, not a user tunable.

**Per-group concurrency.** There are TWO gates, not one, and the first draft of this doc addressed only the second. Panel round 1, staff seat, confirmed at file:line:

1. An outer launch cap: `while active_tasks.len() < max_concurrent` (`scheduler.rs:1021`). Nothing past `-j` is ever spawned.
2. A `Semaphore::new(max_parallel)` (`scheduler.rs:884`) acquired inside each task body (`task_execution.rs:54`), where a `tty: true` task takes all `max_parallel` permits to make itself exclusive (`permits_for`, `:326-337`).

Moving foreach items onto a per-group semaphore fixes gate 2 and leaves gate 1 in place, so 10 items under `-j 2` would still spawn 2. Measured by the seat: `started_count=2`.

**The "group holds one shared permit" idea from the first draft is dead, and it was false.** Both seats killed it the same way: the virtual parent is not concurrent with its items. `foreach.rs:98` replaces the parent's `before:` with `When::Always` edges to every subtask, so the scheduler queues the parent only after all subtasks are terminal. There is no running group owner to hold anything.

What replaces it:

- Items of a group carrying `jobs` are **exempt from both gates**: they do not count against `active_tasks.len() < max_concurrent`, and they acquire from a per-group `Semaphore::new(N)` (or `Semaphore::new(item_count)` for `all`) instead of the shared one.
- **Exclusivity is decided in the launch loop, which is already single-threaded, not by a second runtime predicate.** `execute_all`'s `while active_tasks.len() < max_concurrent && !ready_queue.is_empty()` (`scheduler.rs:1021`) is the one admission point in the program: it runs in one task, and `active_tasks` is a local it alone mutates. Two rules go there, and only there:
  - do not admit a `tty: true` task while any exempt item is in flight;
  - do not admit an exempt item while a `tty: true` task is in flight.

  **The load-bearing property, named because a refactor could silently remove it:** `ActiveTasks::spawn` (`scheduler.rs:248-255`) inserts into `running` at SPAWN time, before the body reaches `semaphore.acquire_many` at `task_execution.rs:54`, and an entry only leaves via `reported()` (`:258-261`) or `reap_unreported()`, both driven from the same loop. So a task the loop admitted counts as in flight for the entire time its body sits queuing on the semaphore, not just while it holds permits. Without that, admission could be decided on a stale view: the loop lets a tty task through, the body is still queuing, and the loop admits an exempt group in the next iteration because the tty task "is not running yet". If anyone ever moves that insert to permit-acquisition time, this design breaks and Phase 3 criterion (f) is what catches it.

  Everything else is untouched. `permits_for` and the shared semaphore keep handling tty-versus-normal exactly as today, so the FIFO guarantee documented at `task_execution.rs:52-53` ("tokio's semaphore is FIFO, so this wait cannot be starved by later single-permit acquires") still holds: the tty path never leaves the queue to wait on anything else.

  **This replaces a mechanism that was wrong.** The first version of this fix used an `AtomicUsize` of in-flight exempt items, with the tty path waiting for zero and exempt items checking that no tty task held the semaphore. Panel round 1 (synthesis, not a seat: the text postdated the seats' snapshot) found the TOCTOU: two unsynchronized predicates over two pieces of state, so an exempt item can read "no tty holds the semaphore" while the tty path concurrently reads "counter == 0", and both proceed. A barrier test would pass most runs. It also cost the tty path its FIFO position. Deciding both rules in the serialized loop removes the second predicate rather than trying to order two of them.
- **Consequence, stated rather than discovered later:** a `jobs: all` group of never-exiting items blocks a later `tty: true` task for as long as the group runs, which for `otto logs` is forever. This is not a regression. A single never-exiting task holds a shared permit forever today and starves a later tty task identically. It is the cost of asking for the exemption, and `logs` is the last task in a run by construction.

**An actionable unknown-field error, and NO api bump.** The strict-parse failure path gets a wrapper: an `unknown field` serde error is re-emitted with a trailing line naming the likely cause and the fix. That is the whole of the otto-side change, and it helps from this release forward.

The first draft also proposed `SUPPORTED_API_VERSIONS = ["1", "2"]`. Dropped, see Resolved Decisions: this repo already has a written policy forbidding it, at `src/cfg/otto.rs:20-26`.

### Data Model

```yaml
tasks:
  logs:
    foreach:
      command: "scripts/stack.sh scope logs"
      as: svc
      parallel: true
      jobs: all          # NEW: all | <positive integer>
```

`ForeachSpec` gains:

```rust
/// Concurrency for THIS group's items, overriding the global `-j`/`otto.jobs`.
/// `all` gives one permit per item, which is what a group of tasks that never
/// exit on their own (a log tail, a watcher, a dev server) requires: under the
/// global cap the items past the cap never start, silently. Items carrying this
/// key are exempt from the global launch cap and the shared semaphore; `tty:`
/// exclusivity against them is enforced by the scheduler's admission loop, not
/// by a permit (see the design doc: the virtual parent runs AFTER its items and
/// cannot hold one).
#[serde(default, skip_serializing_if = "Option::is_none")]
pub jobs: Option<ForeachJobs>,
```

```rust
pub enum ForeachJobs { All, Fixed(NonZeroUsize) }
```

`all` is a literal, not a magic `0`. `jobs: 0` is a load error telling you to write `all`.

### API Design

Load-time rejections, each naming both keys and the task path, in the shape `validate_foreach_buffer` already uses:

| rejected | why |
|---|---|
| `jobs` with `parallel: false` | serial means one at a time; a concurrency override is incoherent |
| `jobs` with no `foreach` | it is a foreach key |
| `jobs: 0` | write `all` |
| `jobs: <negative or non-integer>` | serde type error, already loud |

`jobs: all` with `buffer: true` is legal and expected: buffering is a display policy, concurrency is a scheduling policy.

### Implementation Plan

No Phase 0. The one environmental assumption this design rests on (that a Ctrl+C leaves grandchildren alive, and that the global cap starves a blocking group) was measured on 2026-09-01 and is recorded above with output. A spike would re-run what is already in the doc.

#### Phase 1: Reap the process group on cancel
**Model:** opus
- Live-child registry hung off `ActiveTasks`; body inserts after spawn, removes on both exit paths.
- `abandon_run` walks it: `killpg` SIGTERM -> `CANCEL_GRACE` -> `killpg` SIGKILL, direct `kill` for the `tty` case.
- `CANCEL_GRACE` const beside `OUTPUT_PROCESSING_TIMEOUT_SECS`, with the rationale in a doc comment.
- Update BOTH stale comments, since either one alone re-hides the bug: `task_execution.rs:200-204` describes a group reachability the code never uses, and `support.rs:150-153` (`abandon_run`'s own doc comment) asserts outright that "`kill_on_drop(true)` on every spawned command means aborting the task bodies takes the processes with them." That second one is the more dangerous of the two: it sits on the function being changed and states the false guarantee as the reason the function is correct.
- Snapshot the pgid list before the first signal; both passes iterate the snapshot. `ESRCH` ignored on the second pass, `EPERM` loud.
- **Success criteria:** (a) a pty test spawns a parallel foreach whose bodies each fork a `sleep 600` grandchild, interrupts with a literal `0x03`, and asserts every recorded grandchild pid is gone (checked by `/proc/<pid>/cmdline`, not pid liveness alone, since pids recycle); (b) break-the-code: with the `killpg` call removed the same test fails, and the grandchildren survive; (c) a `tty: true` task's cancellation does not signal otto itself, asserted by otto reaching its normal non-zero exit rather than dying of its own SIGTERM; (d) **the grace-window case**: the direct child exits on SIGTERM while a grandchild traps and ignores it, so the body deregisters during `CANCEL_GRACE`; the grandchild is still reaped, which fails if the SIGKILL pass reads the live registry instead of the snapshot.

#### Phase 2: `foreach.jobs` schema and validation
**Model:** sonnet
- `ForeachJobs` enum, `ForeachSpec::jobs`, `deny_unknown_fields` intact.
- The four load-time rejections above, each naming the task path.
- `docs/commands/ottofile-reference.md` key inventory updated; `ottofile_reference_key_inventory_is_exhaustive` goes 45 -> 46. **Two literals must be edited**, not one: the per-struct expected count (`src/cfg/task_tests.rs:992`) and the total (`:1057`, currently `assert_eq!(total, 45, ...)`). The first draft of this criterion claimed the test passes "without hand-editing the count", which is false; the test is deliberately built so adding a key fails the build until both the page and the counts are updated.
- **Success criteria:** (a) each of the four rejected shapes fails the load with an error naming the task path, and none panics; (b) `otto --tasks --format json` on an ottofile setting none of the new keys is byte-identical to its pre-change output, which holds because `jobs` carries `skip_serializing_if = "Option::is_none"`; (c) the inventory test passes at 46, with both literals updated in the same commit as the schema change.

#### Phase 3: Scheduler honors `foreach.jobs`
**Model:** opus
- Exempt the group's items from BOTH gates: the outer `active_tasks.len() < max_concurrent` launch cap (`scheduler.rs:1021`) and the shared semaphore acquire (`task_execution.rs:54`). Fixing only the semaphore leaves the launch cap in force and changes nothing.
- Items acquire from a per-group `Semaphore::new(N)` / `Semaphore::new(item_count)`.
- `tty` exclusivity as two admission rules in the single-threaded launch loop, not as a runtime predicate. No new atomic, no change to `permits_for`, FIFO preserved.
- **Success criteria:** (a) 10 items, `parallel: true`, `jobs: all`, global `-j 2`, each body blocking on a fifo: all 10 write their start marker. `Observed on main` for the unguarded shape: `started_count=2`; (b) the same fixture with `jobs: 4` starts exactly 4 and the fifth only after one exits; (c) a `tty: true` task and a `jobs: all` group in one run never overlap, proved by a barrier file rather than timing, **with `max_parallel = 1`**, which is the case where a fake permit would have looked correct; (d) a `tty: true` task that becomes ready WHILE an exempt group is in flight waits for it, rather than starting alongside it, and the mirror case (an exempt group becoming ready while a tty task runs) also waits; (e) break-the-code: deleting either admission rule fails (c) or (d), which is what pins that the check lives in the loop and not in a racing predicate; (f) a unit test asserts `ActiveTasks::spawn` marks a task running BEFORE its body acquires a permit, so the admission view is never stale. This is the property the whole mechanism rests on and it is currently incidental rather than pinned.

#### Phase 4: An actionable unknown-field error
**Model:** sonnet
- Wrap the strict-parse `unknown field` error with a line naming the likely cause and `otto Upgrade`. No api bump: see Resolved Decisions.
- **Success criteria:** (a) an ottofile using a key this binary does not know fails with a message that names the key AND names upgrading, and still exits non-zero; (b) an ottofile with a genuinely misspelled key (`tsaks:`) gets the same treatment without the wrapper claiming the user is out of date as the only possibility; (c) `SUPPORTED_API_VERSIONS` is unchanged, asserted by a test so a later phase cannot quietly grow it against the policy at `src/cfg/otto.rs:20-26`.

#### Phase 5: Docs, example, and the release-post correction
**Model:** sonnet
- `docs/commands/buffered-foreach.md` gains a `jobs` section; `examples/foreach-buffer/otto.yml` gains a commented `jobs:` line.
- Correct the v2.1.0 marquee post's teardown sentence: descendants were not reaped before Phase 1. Post: `https://marquee.internal.tatari.dev/p/~scott-idler/otto-v2-1-0-i-built-all-three-the-bone-is-clean/`, section 5, the bullet reading "They die from `kill_on_drop(true)` when `abandon_run` aborts them."
- **Success criteria:** (a) `otto examples` passes; (b) the reference page's key count matches the schema, enforced by the existing inventory test rather than by reading; (c) `marquee read <the URL above> | grep -c 'They die from'` returns 0 and the replacement sentence names the process group.

### Cross-repo blast radius and ship order

- **otto**: one additive key (`tasks.<name>.foreach.jobs`), one behavior change to cancellation (descendants now die, which is the fix). No change to `-j` semantics for any ottofile that does not set `foreach.jobs`. Minor version bump.
- **tatari-tv/otto-dev**: their PR, their schedule, but two halves with different urgency.
  - **Unblocked now, and the most urgent thing in this document:** a version floor check in `bin/ttv` before `exec otto`, plus the `scripts/lib.sh:130-135` comment correction. Anyone who pulls `otto-dev@main` today on v2.0.5 gets a serde error naming a YAML line. This does not wait on otto.
  - **Blocked on otto:** `foreach.jobs: all` on `logs`. Nothing else; the api-generation idea that used to sit here is dropped.
- Ship order: otto-dev's floor check -> otto Phases 1-5 -> otto-dev adopts `jobs: all`. Phase 1 should land before otto-dev is told `logs` is safe, since today every Ctrl+C on it leaks an inner otto and a compose tail per service.

## Acceptance Criteria

Each criterion's literal command was run against current `main` (`a6cd589`, v2.1.0) on 2026-09-01 and the output recorded.

- [ ] **A cancelled run leaves no descendant of any task body alive.**
  `Observed on main:` a parallel foreach whose bodies fork `sleep 600`, interrupted by a literal `0x03` through a pty, left both grandchildren alive: `pid 57 AFTER Ctrl+C -> cmdline='sleep 600'`, same for 58. Criterion currently FALSE, which is the defect.

- [ ] **`grep -rn 'killpg\|Pid::from_raw(-' src/` returns at least one hit in the cancellation path.**
  `Observed on main:` zero hits. The only `process_group` reference in `src/` outside a comment is `task_execution.rs:207`, which creates the group nothing signals.

- [ ] **A `parallel: true` foreach of N never-exiting items with `jobs: all` starts all N under a global cap below N.**
  `Observed on main:` cannot run, `jobs` does not exist yet. The unguarded case measured 8 of 10 started at `otto.jobs: 8`, silently.

- [ ] **An ottofile using a key this binary does not know fails with a message that names the key AND names upgrading.**
  `Observed on main:` names the key only. `tasks.hi.foreach: unknown field 'jobs', expected one of 'glob', 'items', 'range', 'command', 'as', 'parallel', 'max_items', 'buffer'`, rc=1. Nothing tells the reader their otto might be too old. Criterion currently FALSE.

- [ ] **`SUPPORTED_API_VERSIONS` still contains exactly `["1"]` after every phase.**
  `Observed on main:` `pub const SUPPORTED_API_VERSIONS: &[&str] = &[CURRENT_API_VERSION];`, one entry. This criterion guards a decision, not a feature: the doc's first draft proposed adding `"2"` and the policy at `src/cfg/otto.rs:20-26` forbids it for additive keys.

- [ ] **REGRESSION GUARD, not phase verification: `otto --tasks --format json` on an ottofile setting none of the new keys is byte-identical before and after all five phases.**
  `Observed on main:` baseline captured, 20 top-level keys on `otto-dev@main`, `subtasks` present per task.
  **Labelled honestly after panel round 1, because as originally written it was vacuous.** `TaskView` (`src/cli/commands/tasks.rs:25-30`) has exactly four fields: `help`, `params`, `edges`, `subtasks`. No `ForeachSpec` field reaches that serializer, so adding `jobs` cannot change this output whether Phase 2 is right or wrong. It is worth keeping as a guard against an unintended change to the view, and it verifies nothing about the phase it sat under. The architect seat reached the right verdict here through the wrong mechanism (crediting `skip_serializing_if`), which is why the reasoning is recorded rather than just the verdict.

- [ ] **Every ottofile under `examples/` still loads, and `otto examples` passes, after the schema change.**
  `Observed on main:` `otto ci` green at `a6cd589` includes `examples_integration_test`. This is the criterion that actually bites when a `ForeachSpec` field is added wrong, since `deny_unknown_fields` and the typed parse are what Phase 2 touches.

## Resolved Decisions

- **2026-09-01: `jobs: all`, not `jobs: 0`.** A magic zero meaning unbounded is the naming-tells-the-truth violation. `all` is the literal thing meant, and `0` becomes a load error that names the replacement.
- **2026-09-01, SUPERSEDED same day by panel round 1: the overridden group holds one shared permit.** Recorded because it was decided and then killed, not because it stands. It rested on the virtual parent being concurrent with its items, which `foreach.rs:98` disproves. See the round-1 entries below for what replaced it.
- **2026-09-01: `--tasks` is not made lazy.** Checked rather than assumed: `--tasks --format json` emits a `subtasks` array per task (`src/cli/commands/tasks.rs:22-72` documents it as part of the view), so resolving a `command:` source there is contractual, not waste. The #15 nit about 10 subprocesses instead of 8 is the direct cost of adding two foreach tasks. No action.
- **2026-09-01: the `[status:<svc>]` prefix replacing otto-dev's `=== <name> ===` headers stays.** Within one block every row carries the same prefix, so a compose table stays column-aligned; only the left margin differs between services. Cosmetic, and the prefix buys attribution that the header did not (it survives interleaving on `logs`). No action.
- **2026-09-01: no per-item concurrency for `status`.** It is buffered and its commands exit, so permits recycle and the global cap only affects wall time. Only `logs` needs the override.

Panel round 1 (Architect + Staff Engineer), 2026-09-01. Every finding below was re-verified against the code before folding in.

- **`api: 2` is dropped, both seats, and the repo already said so.** `src/cfg/otto.rs:20-26` carries a written policy: "A new version is added when, and only when, otto makes a change that a prior otto would MIS-EXECUTE rather than merely fail to understand. Adding an optional field does NOT bump it." Every key in v2.1.0 and in this doc is additive. The proposed generation 2 would have gated nothing mechanically, which is the names-tell-the-truth violation, and the unknown-field wrapper covers every future addition natively. The one thing `api: 2` bought (a clean message from the already-shipped v2.0.5) is bought instead by otto-dev's `bin/ttv` floor check, which is in this doc anyway and needs no otto release.
- **The SIGKILL pass reads a snapshot, not the live registry.** Both seats, independently. Folded into Architecture with the reasoning, and Phase 1 gains criterion (d) for the exact grace-window case.
- **"The group holds one shared permit" was FALSE and is removed.** Both seats: `foreach.rs:98` gives the virtual parent `When::Always` edges to every subtask, so it is queued only after they are all terminal. It cannot hold anything while they run. Replaced first with an in-flight counter, which was itself racy, and finally with two admission rules in the launch loop; see the TOCTOU entry below for that second correction.
- **Phase 3 must move BOTH gates.** Staff seat, measured: the outer `active_tasks.len() < max_concurrent` at `scheduler.rs:1021` alone holds the run to `started_count=2` under `-j 2`, so a per-group semaphore by itself changes nothing.
- **The Phase 2 inventory criterion was FALSE as written.** Staff seat: `src/cfg/task_tests.rs:1057` is a literal `assert_eq!(total, 45, ...)` with per-struct counts at `:992`. Both get edited; the criterion now says so.
- **`bin/ttv` cheap-win widened.** Staff seat found a second copy of the stale premise at otto-dev `CLAUDE.md:49-52`, beyond the `scripts/lib.sh` one. Both are in the otto-dev item now.
- **Held against both seats: nothing.** No pushbacks. Every must-fix from both seats is folded in above.
- **The Phase 3 TOCTOU, found by the synthesis rather than a seat**, in text written after the seats' snapshot. The `AtomicUsize` replacement had two unsynchronized predicates over two pieces of state and cost the tty path its FIFO position. Solved rather than logged as an open question: both admission rules move into `execute_all`'s launch loop, which is single-threaded and already owns `active_tasks`, so there is no second predicate to race. Open Questions stays empty.
- **Criterion 5 was vacuous and is relabelled**, also from the synthesis: `TaskView` has four fields and none derive from `ForeachSpec`, so a `--tasks` byte-identity check cannot fail because of Phase 2. Kept as a regression guard, with a real Phase 2 criterion (the `examples/` load) added beside it.
- **The admission-versus-acquisition seam was attacked and holds.** Raised by the panel synthesis after round 1 closed: the loop decides admission, but permits are acquired inside the body at `task_execution.rs:54`, so a rule true at admission could be false while the body is still queuing. Checked rather than assumed: `ActiveTasks::spawn` inserts into `running` at spawn, and removal only happens via `reported()` or `reap_unreported()`, both loop-driven. An admitted task is therefore in flight for the whole queuing window. Recorded as a named property with criterion (f) pinning it, since today it is incidental.
- **2026-09-01: no round 2.** The reason to run one was that the Phase 3 exclusivity mechanism had been wrong twice (a permit the virtual parent structurally cannot hold, then two racing predicates). Version three was then attacked at its weakest seam by the panel synthesis (admission is decided in the loop, but permits are acquired later inside the body) and held, with the load-bearing property verified at `scheduler.rs:248-255` and pinned by Phase 3 criterion (f). A seat would be re-deriving a single ownership fact that is already read and cited. The remaining risk is implementation, and the implementation audit walks the plan bullet by bullet.
- **Excellence pass, 2026-09-01, run after the panel rather than before it, and it caught real breakage.** The dead "group holds one shared permit" design had survived in four places the round-1 edits missed, including the `ForeachSpec` Rust doc comment, which would have been copied verbatim into the code and re-asserted the false invariant at the exact site that implements it. Three `api: 2` references also outlived the decision to drop it, in the ship order, the rollout plan, and the blast radius. Cause: several of those edits were applied without asserting the anchor matched, so they printed success and changed nothing. Every edit in this pass asserts. **The guard that actually catches this, and the one to use when a design changes mid-doc: after editing, grep for the DEAD phrase, not for the new one.** A search for "shared permit" and for "api: 2" would have found all seven survivors in one command, including the `ForeachSpec` doc comment. Trusting the edit tool's own success report does not, and neither does a frozen-doc review round, because the panel reads the file rather than the intent.
- **Process note, recorded because it cost the seats accuracy:** I told the panel the doc was frozen and then edited it while the seats were running, 56 diff lines against their snapshot. Their findings still landed, but every one had to be re-read against text they had not seen. The doc is frozen for any future round, or the round is not dispatched.

## Alternatives Considered

### Alternative 1: otto-dev raises `otto.jobs` to 10 or more
- **Description:** No otto change. Set the global cap above the service count.
- **Pros:** Zero code. Available today.
- **Cons:** Couples one repo-wide tuning knob to one group's item count; breaks silently again when service 11 is added; makes every other verb run 10-wide when 8 was the considered choice.
- **Why not chosen:** It encodes a correctness requirement ("all tails must run") as a tuning value, and nothing keeps the two in sync.

### Alternative 2: `PR_SET_PDEATHSIG` on the child
- **Description:** Ask the kernel to signal the child when otto dies.
- **Pros:** No registry, no grace period.
- **Cons:** Linux only, and it covers the direct child, which `kill_on_drop` already covers. Grandchildren, the actual problem, are untouched. It also fires on otto's normal exit, not only on cancel.
- **Why not chosen:** Solves the half that is not broken.

### Alternative 3: one process group for the entire run
- **Description:** otto puts itself in a new group at start; every child inherits; teardown is one `killpg`.
- **Pros:** Simplest possible reaping, no registry.
- **Cons:** Breaks `tty: true` outright (a background group reading the terminal takes SIGTTIN and stops, which is the exact reason `process_group(0)` is skipped for tty today), and it removes otto from the terminal's foreground group, so the terminal's own Ctrl+C stops reaching otto at all.
- **Why not chosen:** It breaks the feature the current group layout exists to protect.

### Alternative 4: a `foreach.unbounded: true` boolean
- **Description:** A boolean escape hatch instead of a value.
- **Pros:** Smaller surface than an enum.
- **Cons:** Cannot express "cap this group at 4", which is the symmetric and equally reasonable case; and a second boolean would then be needed beside it.
- **Why not chosen:** Write as if more are coming. `jobs: all | <N>` covers both with one key and reuses the `-j`/`otto.jobs` vocabulary already in the file.

### Alternative 5: `SUPPORTED_API_VERSIONS` gains `"2"`
- **Description:** Let an ottofile declare schema generation 2, so an old binary refuses it with `check_api_version`'s existing clean upgrade message before the typed parse.
- **Pros:** The message already exists and already works: v2.0.5 prints `unsupported api version '2' (this otto supports: 1). upgrade otto.` It is the one mechanism that reaches an already-shipped binary.
- **Cons:** `src/cfg/otto.rs:20-26` forbids it for additive keys, in writing, and every key here is additive. A generation that gates nothing mechanically is a version number that lies. It also cannot be enforced: a file can declare `2` and use no new keys, or declare `1` and use them all.
- **Why not chosen:** Rejected by both panel seats and by the repo's own policy. Carried in this doc's first draft as Phase 4's first half; removed after round 1 so it is not re-litigated.

### Alternative 6: warn when a parallel group cannot get a permit per item
- **Description:** Leave the cap alone, print a notice at expansion time.
- **Pros:** No scheduler change.
- **Cons:** otto cannot distinguish "these items never exit" from "these items are slow", so the notice fires on every large parallel foreach, which is the normal and correct case. A warning that is usually wrong gets filtered out, and then it is not there when it matters.
- **Why not chosen:** Noise, and it does not fix anything.

## Technical Considerations

### Dependencies

`libc` is already a direct dependency (`Cargo.toml:56`), used by the upgrade reaper's `kill(pid, 0)`. `killpg` needs nothing new.

### Performance

One `HashMap` insert and remove per task, on paths that already spawn a process. The grace period is paid only on cancel.

### Security

Signalling a process group is signalling processes otto did not directly spawn. The pgid is one otto created itself via `process_group(0)` and recorded at spawn time, never a pid read from anywhere else, so the blast radius is exactly the subtree otto started. The `tty` case, where the group is otto's own, is the one that must never take a group signal and is handled as its own branch rather than by an `if` inside the loop.

Negative-pid handling gets the same treatment the upgrade reaper already learned: anything that does not fit a positive `pid_t` is refused before reaching the syscall, rather than being reinterpreted as a process group.

### Testing Strategy

- Phase 1 extends the pty pattern in `tests/sigint_cancel_test.rs`: real `script` pty, literal `0x03`, every step gated on a marker file the body wrote. Grandchild liveness is checked by `/proc/<pid>/cmdline` content, never by pid existence alone.
- Phase 3 proves concurrency with a barrier, not a stopwatch: each item writes to and then reads from a shared fifo, so the run only completes if the items actually overlap.
- Every phase carries a break-the-code check: revert the change, watch the test go red.
- Claims about old-binary behavior are measured against the built `v2.0.5` worktree rather than reasoned about, which is how the api-generation idea was tested and then rejected.

### Rollout Plan

Minor bump. `foreach.jobs` is opt-in; the cancellation fix is not, and is the reason for the bump rather than a patch.

## Risks and Mitigations

| Risk | Likelihood | Impact | Mitigation |
|------|------------|--------|------------|
| `killpg` reaches something otto did not start | Low | High | The pgid is one otto created via `process_group(0)` and recorded at spawn; the `tty` branch never takes a group signal; non-positive pids refused before the syscall |
| The grace period makes Ctrl+C feel slow | Med | Low | SIGTERM first, and the second Ctrl+C escape hatch (exit 130) already shipped in v2.1.0 bypasses the wait entirely |
| A `jobs: all` group of never-exiting items blocks every later task, including a `tty:` one | Med | Med | Not a regression: one never-exiting task starves a later tty task identically today. Stated in Architecture as the cost of asking for the exemption, and `logs` is terminal in a run by construction |
| A later change grows `SUPPORTED_API_VERSIONS` against the policy | Low | Med | Phase 4 criterion (c) asserts the set stays `["1"]`, so growing it fails a test rather than passing review |
| otto-dev ships `jobs: all` before Phase 1 lands | Med | High | Ship order above: Phase 1 first. Until then every Ctrl+C on `otto logs` leaks an inner otto and a compose tail per service |

## Open Questions

None.

## References

- `tatari-tv/otto-dev#15`, merged 2026-09-01
- v2.1.0 release post: `otto v2.1.0: I built all three, the bone is clean`
- `docs/design/2026-08-31-buffered-foreach-computed-envs-required-params.md` (the three features #15 adopts)
- `docs/design/2026-09-01-post-audit-finalization-handoff.md`
