# Design Document: Cancellation reaping, foreach concurrency, and the upgrade cliff

**Author:** Scott A. Idler
**Date:** 2026-09-01
**Status:** Draft
**Review Passes Completed:** 4/5 (draft, correctness, clarity, edge cases; excellence pass not run)

## Summary

Reviewing `tatari-tv/otto-dev#15` (the v2.1.0 adoption) turned up three otto defects and two otto-dev items. The headline: otto's cancellation kills the direct task child and orphans everything below it, which the code comment at `task_execution.rs:200-204` already claims to have solved. This doc fixes that, adds a per-group concurrency override so a group of never-exiting tasks cannot starve itself against the global `-j` cap, and makes the pre-2.1.0 upgrade cliff say "upgrade otto" instead of a serde error.

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

`SUPPORTED_API_VERSIONS` has only ever contained `"1"`, so no ottofile can declare a newer schema generation, so the good message is unreachable.

**4. otto-dev has no floor check on the path everyone uses.** `bin/ttv` validates that `.otto.yml` exists and that `otto` is on PATH, then `exec otto -C "$ROOT" "$@"`. It does not compare `otto --version` against `OTTO_DEV_MIN_OTTO`, and `otto --version` needs no ottofile. That check is the one thing that could turn finding 3 into words today, for people already on an old binary.

**5. A stale premise in otto-dev's own comment.** `scripts/lib.sh:130-135`, unchanged by #15, says an old otto "doesn't reject keys it predates, it ignores them and runs something else entirely." That was true before v1.4.0. It is the source of #15's risk-section claim that the new keys are "silently ignored by an old binary", which is the opposite of measured behavior.

### Goals

- Cancellation reaps the whole task subtree, not just the direct child.
- An ottofile can declare that a foreach group needs one permit per item.
- A binary too old for an ottofile says "upgrade otto", not a serde field error.
- otto-dev's `logs` survives Ctrl+C with no orphans and no starved services.
- Every nit from the #15 review is dispositioned in this doc: fixed, or recorded as no-action with the reason.

### Non-Goals

- **A general job-control or process-supervision layer.** One grace period, one group signal. Excluded, not parked.
- **Changing what `-j`/`otto.jobs` mean globally.** The new key is an override at one seam, not a redefinition.
- **Retrofitting the good upgrade message onto already-released binaries.** Impossible: v2.0.5 is published. Parked with a revisit condition: none, it is physics.
- **Windows.** otto's process-group handling is already `#[cfg(unix)]`. Excluded.
- **`--tasks` laziness.** See Alternatives; the resolution is contractual, not waste.

## Proposed Solution

### Overview

Three otto changes and two otto-dev changes, in that dependency order.

| # | Repo | Change |
|---|---|---|
| 1 | otto | Reap the task's process group on cancel: SIGTERM to `-pgid`, grace, SIGKILL to `-pgid` |
| 2 | otto | `tasks.<name>.foreach.jobs: all \| <N>` |
| 3 | otto | `SUPPORTED_API_VERSIONS` gains `"2"`; the unknown-field error names the upgrade |
| 4 | otto-dev | Version floor check in `bin/ttv` before `exec`; `otto.api: 2`; `foreach.jobs: all` on `logs`; `lib.sh` comment corrected |
| 5 | both | Correct the release post's teardown sentence |

### Architecture

**Group reaping.** The child handles live inside the spawned task bodies, not in the scheduler, so the scheduler cannot reach them today. Add a live-child registry the body writes into:

- `Arc<Mutex<HashMap<String, ChildHandle>>>` on the scheduler, where `ChildHandle { pid: u32, own_group: bool }`.
- The body inserts after spawn and removes on exit, in both the success and failure paths.
- `abandon_run` walks the registry before `abort_all`: for each `own_group` entry, `killpg(pid, SIGTERM)`; sleep `CANCEL_GRACE`; `killpg(pid, SIGKILL)` for anything still present. Entries with `own_group: false` (a `tty: true` task, which deliberately stays in otto's group) get a direct `kill(pid, ...)`, never a group signal: otto is in that group and would signal itself.
- `abort_all` still runs after, so `kill_on_drop` remains the backstop rather than the mechanism.

`CANCEL_GRACE` is a hard-coded const beside `OUTPUT_PROCESSING_TIMEOUT_SECS` (`scheduler.rs:35`), matching that precedent. It is teardown timing, not a user tunable.

**Per-group concurrency.** The scheduler gates on one `Semaphore::new(max_parallel)` (`scheduler.rs:884`), and a `tty: true` task acquires all `max_parallel` permits to make itself exclusive (`permits_for`, `:326-337`). A group override must not break that exclusivity, so:

- A foreach parent carrying `jobs` acquires **one** permit from the shared semaphore for the group as a whole.
- Its items acquire from a per-group semaphore instead: `Semaphore::new(N)`, or `Semaphore::new(item_count)` for `all`.
- Net effect: a `tty` task is still exclusive against the group (the group holds a shared permit), and the group's items are bounded by their own number rather than by `-j`.

**API generation 2.** `SUPPORTED_API_VERSIONS` becomes `["1", "2"]`. Generation 2 means "may use the key set introduced in v2.1.0" (`otto.envs-command`, `foreach.buffer`, `params.<name>.required`) plus `foreach.jobs` from this doc. `api: 1` files keep working unchanged on every binary. This buys nothing for v2.0.5 users pulling otto-dev today, and everything for the next time otto adds a key.

Separately, the strict-parse failure path gets a wrapper: an `unknown field` serde error is re-emitted with a trailing line naming the likely cause and the fix. That helps from this release forward.

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
/// global cap the items past the cap never start, silently. The group as a
/// whole still holds one global permit, so a `tty: true` task stays exclusive.
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
- Live-child registry on the scheduler; body inserts after spawn, removes on both exit paths.
- `abandon_run` walks it: `killpg` SIGTERM -> `CANCEL_GRACE` -> `killpg` SIGKILL, direct `kill` for the `tty` case.
- `CANCEL_GRACE` const beside `OUTPUT_PROCESSING_TIMEOUT_SECS`, with the rationale in a doc comment.
- Update the `task_execution.rs:200-204` comment: it currently describes a reachability the code does not use, which is what hid this for two releases.
- **Success criteria:** (a) a pty test spawns a parallel foreach whose bodies each fork a `sleep 600` grandchild, interrupts with a literal `0x03`, and asserts every recorded grandchild pid is gone (checked by `/proc/<pid>/cmdline`, not pid liveness alone, since pids recycle); (b) break-the-code: with the `killpg` call removed the same test fails, and the grandchildren survive; (c) a `tty: true` task's cancellation does not signal otto itself, asserted by otto reaching its normal non-zero exit rather than dying of its own SIGTERM.

#### Phase 2: `foreach.jobs` schema and validation
**Model:** sonnet
- `ForeachJobs` enum, `ForeachSpec::jobs`, `deny_unknown_fields` intact.
- The four load-time rejections above, each naming the task path.
- `docs/commands/ottofile-reference.md` key inventory updated; `ottofile_reference_key_inventory_is_exhaustive` goes 45 -> 46.
- **Success criteria:** (a) each of the four rejected shapes fails the load with an error naming the param path, and none panics; (b) `otto --tasks --format json` on an ottofile setting none of the new keys is byte-identical to its pre-change output; (c) the inventory test passes at 46 without hand-editing the count into the assertion.

#### Phase 3: Scheduler honors `foreach.jobs`
**Model:** opus
- Group acquires one shared permit; items acquire from a per-group semaphore.
- `tty` exclusivity preserved against an overridden group.
- **Success criteria:** (a) 10 items, `parallel: true`, `jobs: all`, global `-j 2`, each body blocking on a fifo: all 10 write their start marker (today 2 do); (b) the same fixture with `jobs: 4` starts exactly 4; (c) a `tty: true` task and a `jobs: all` group in one run never overlap, proved by a barrier file rather than timing.

#### Phase 4: API generation 2 and an actionable unknown-field error
**Model:** sonnet
- `SUPPORTED_API_VERSIONS = ["1", "2"]`, with a doc comment defining what generation 2 means.
- Wrap the strict-parse `unknown field` error with a line naming the likely cause and `otto Upgrade`.
- `docs/commands/ottofile-reference.md` documents both generations and when to declare 2.
- **Success criteria:** (a) an `api: 2` ottofile loads and runs on this binary; (b) the v2.0.5 binary on that same file still prints `unsupported api version '2' ... upgrade otto.` and exits non-zero, which is the whole point and is verified against the built v2.0.5 worktree, not asserted; (c) an ottofile with a genuinely misspelled key still fails, and its message names both the misspelling and the upgrade possibility.

#### Phase 5: Docs, example, and the release-post correction
**Model:** sonnet
- `docs/commands/buffered-foreach.md` gains a `jobs` section; `examples/foreach-buffer/otto.yml` gains a commented `jobs:` line.
- Correct the v2.1.0 marquee post's teardown sentence: descendants were not reaped before Phase 1.
- **Success criteria:** (a) `otto examples` passes; (b) the reference page's key count matches the schema, enforced by the existing inventory test rather than by reading.

### Cross-repo blast radius and ship order

- **otto**: four additive keys/values, one behavior change to cancellation (descendants now die, which is the fix). No change to `-j` semantics for any ottofile that does not set `foreach.jobs`. Minor version bump.
- **tatari-tv/otto-dev**: their PR, their schedule, but two halves with different urgency.
  - **Unblocked now, and the most urgent thing in this document:** a version floor check in `bin/ttv` before `exec otto`, plus the `scripts/lib.sh:130-135` comment correction. Anyone who pulls `otto-dev@main` today on v2.0.5 gets a serde error naming a YAML line. This does not wait on otto.
  - **Blocked on otto:** `foreach.jobs: all` on `logs`, and `otto.api: 2`.
- Ship order: otto-dev's floor check -> otto Phases 1-5 -> otto-dev adopts `jobs: all` and `api: 2`. Phase 1 should land before otto-dev is told `logs` is safe, since today every Ctrl+C on it leaks an inner otto and a compose tail per service.

## Acceptance Criteria

Each criterion's literal command was run against current `main` (`a6cd589`, v2.1.0) on 2026-09-01 and the output recorded.

- [ ] **A cancelled run leaves no descendant of any task body alive.**
  `Observed on main:` a parallel foreach whose bodies fork `sleep 600`, interrupted by a literal `0x03` through a pty, left both grandchildren alive: `pid 57 AFTER Ctrl+C -> cmdline='sleep 600'`, same for 58. Criterion currently FALSE, which is the defect.

- [ ] **`grep -rn 'killpg\|Pid::from_raw(-' src/` returns at least one hit in the cancellation path.**
  `Observed on main:` zero hits. The only `process_group` reference in `src/` outside a comment is `task_execution.rs:207`, which creates the group nothing signals.

- [ ] **A `parallel: true` foreach of N never-exiting items with `jobs: all` starts all N under a global cap below N.**
  `Observed on main:` cannot run, `jobs` does not exist yet. The unguarded case measured 8 of 10 started at `otto.jobs: 8`, silently.

- [ ] **An ottofile declaring `api: 2` loads on this binary and is refused with an upgrade message by v2.0.5.**
  `Observed on main:` the v2.0.5 half already passes: `otto: unsupported api version '2' (this otto supports: 1). upgrade otto.` The v2.1.0 half fails: `SUPPORTED_API_VERSIONS` is `["1"]`, so current otto refuses its own generation.

- [ ] **`otto --tasks --format json` on an ottofile setting none of the new keys is byte-identical before and after all five phases.**
  `Observed on main:` baseline captured, 20 top-level keys on `otto-dev@main`, `subtasks` present per task. Not yet comparable; the after-side does not exist.

## Resolved Decisions

- **2026-09-01: `jobs: all`, not `jobs: 0`.** A magic zero meaning unbounded is the naming-tells-the-truth violation. `all` is the literal thing meant, and `0` becomes a load error that names the replacement.
- **2026-09-01: the overridden group holds one shared permit rather than bypassing the semaphore.** Bypassing it entirely would silently break `tty: true` exclusivity, which is enforced today by a tty task taking every permit. Holding one keeps that invariant true with no special case in `permits_for`.
- **2026-09-01: `--tasks` is not made lazy.** Checked rather than assumed: `--tasks --format json` emits a `subtasks` array per task (`src/cli/commands/tasks.rs:22-72` documents it as part of the view), so resolving a `command:` source there is contractual, not waste. The #15 nit about 10 subprocesses instead of 8 is the direct cost of adding two foreach tasks. No action.
- **2026-09-01: the `[status:<svc>]` prefix replacing otto-dev's `=== <name> ===` headers stays.** Within one block every row carries the same prefix, so a compose table stays column-aligned; only the left margin differs between services. Cosmetic, and the prefix buys attribution that the header did not (it survives interleaving on `logs`). No action.
- **2026-09-01: no per-item concurrency for `status`.** It is buffered and its commands exit, so permits recycle and the global cap only affects wall time. Only `logs` needs the override.

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

### Alternative 5: warn when a parallel group cannot get a permit per item
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
- The v2.0.5 half of the API criterion runs against the built `v2.0.5` worktree, so "an old binary refuses this" is measured rather than reasoned.

### Rollout Plan

Minor bump. `foreach.jobs` and `api: 2` are opt-in; the cancellation fix is not, and is the reason for the bump rather than a patch.

## Risks and Mitigations

| Risk | Likelihood | Impact | Mitigation |
|------|------------|--------|------------|
| `killpg` reaches something otto did not start | Low | High | The pgid is one otto created via `process_group(0)` and recorded at spawn; the `tty` branch never takes a group signal; non-positive pids refused before the syscall |
| The grace period makes Ctrl+C feel slow | Med | Low | SIGTERM first, and the second Ctrl+C escape hatch (exit 130) already shipped in v2.1.0 bypasses the wait entirely |
| A `jobs: all` group starves the rest of the run | Med | Med | The group holds one shared permit, so the scheduler still counts it; documented as the explicit tradeoff of asking for it |
| `api: 2` splits the ecosystem | Low | Med | Generation 1 files keep working on every binary forever; declaring 2 is opt-in and only needed by files using the v2.1.0+ key set |
| otto-dev ships `jobs: all` before Phase 1 lands | Med | High | Ship order above: Phase 1 first. Until then every Ctrl+C on `otto logs` leaks an inner otto and a compose tail per service |

## Open Questions

None.

## References

- `tatari-tv/otto-dev#15`, merged 2026-09-01
- v2.1.0 release post: `otto v2.1.0: I built all three, the bone is clean`
- `docs/design/2026-08-31-buffered-foreach-computed-envs-required-params.md` (the three features #15 adopts)
- `docs/design/2026-09-01-post-audit-finalization-handoff.md`
