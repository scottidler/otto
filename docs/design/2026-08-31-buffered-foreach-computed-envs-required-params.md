# Design Document: Buffered Foreach, Computed Envs, Required Params

**Author:** Scott A. Idler
**Date:** 2026-08-31
**Status:** Implemented
**Review Passes Completed:** 5/5

## Summary

Three additive ottofile keys that close the last otto-side gaps found reviewing `tatari-tv/otto-dev` PR #4 (merged, adopted otto v2.0.5): `foreach.buffer` (run subtasks concurrently, emit each one's output as one contiguous block in item order), `otto.envs-command` (a command whose `KEY=VALUE` stdout becomes global envs), and `params.<title>.required`. Together they retire the last ~180 lines of shell in otto-dev: the hand-rolled buffered scheduler in `stack.sh`, `gen-otto.sh` plus its drift gate and generated markers, and the `${svc:?usage: ...}` guards in every task body. Everything lands in otto; otto-dev adopts on its own schedule.

## Problem Statement

### Background

- otto-dev is otto's most demanding consumer. PR #4 adopted every feature the v2.0.3 and v2.0.5 releases shipped for it: `foreach: command:`, serial ordering without requiring, positional params, `choices-command`, `tty: true`, `-C`, `--no-prefix`, `--tasks`, `otto.jobs`, and the env self-reference fix. Net -483 lines there, 66 tasks -> 21, the 409-line generated region gone.
- Reviewing that merge (published: see References) found three otto gaps that block further deletion. Every one is a place where otto-dev still carries shell that exists only because otto cannot express something.
- Requirement source: Scott, 2026-08-31, in the `/create-design-doc` invocation, quoting the review's "Blocked on otto, and worth me building" section. All three trace to that message. Nothing else is in scope.
- Every finding below was reproduced at a terminal on 2026-08-31 against installed `otto v2.0.5` at `4d9ca4e`.

### Problem

**1. No per-task output grouping, so a parallel foreach cannot print readable per-item blocks.**

Verified on main (`otto -C fx/f1 say`, three items, `parallel: true`, three lines each):

```console
[say:gamma] gamma line 1
[say:beta] beta line 1
[say:alpha] alpha line 1
[say:gamma] gamma line 2
[say:beta] beta line 2
[say:alpha] alpha line 2
```

Lines interleave, and the first block is not even the first item. A `docker compose ps` table is unreadable this way. otto-dev keeps `status` and `logs` out of the task graph entirely because of it, at `scripts/stack.sh:296-378` (about 80 lines): `mktemp -d` buffer dir, pid list, per-pid `wait`, INT/TERM/EXIT traps, ordered replay by item order, exit aggregation. Its own comment states the reason:

```sh
  # status and logs stay here rather than becoming foreach subtasks: each
  # service's output must print as one block, in order, and otto's parallel
  # foreach interleaves lines, which shreds a compose table.
```

That is the last scheduler duty left in otto-dev.

**2. `otto.envs` is a static map, so a computed env SET cannot be expressed.**

Verified on main:

```console
$ otto -C fx/f2 show
otto: unknown field `envs-command`, expected one of `name`, `about`, `api`, `jobs`, `tasks`, `envs`, `retention` at line 3 column 3
```

otto-dev therefore generates eleven `envs:` lines with `scripts/gen-otto.sh` (100 lines) spliced between BEGIN/END GENERATED markers in `.otto.yml`, guarded by a `scripts/check.sh` drift gate. The generator, the markers, and the gate exist only to write:

```yaml
    WEB_ROOT: '$(scripts/svc.sh root web)'
    WEB_FE_ROOT: '$(scripts/svc.sh root web-fe)'
    DATA_API_ROOT: '$(scripts/svc.sh root data-api)'
```

`foreach.command` and `choices-command` both solved the same shape (a command supplies a list); the env map is the one place that still cannot.

**3. Params cannot be marked required.**

Verified on main:

```console
$ otto -C fx/f3 sw
tasks.sw.params.svc: unknown field `required`, expected one of `metavar`, `default`, `choices`, `choices-command`, `nargs`, `help` at line 7 column 9

$ otto -o fx/f3/noreq.yml sw          # control: param omitted, no required key
[sw] svc=[UNSET]
[sw] finished successfully
```

A missing value is silently empty, so every otto-dev task body carries its own guard:

```yaml
      scripts/svc.sh run "${svc:?usage: otto switch <service> <branch>}" \
        switch "${branch:?usage: otto switch <service> <branch>}"
```

clap already does this; otto never asks it to.

### Verified facts the design rests on

Reproduced on 2026-08-31 at `4d9ca4e`, not inferred:

- **Every task's output is already captured to files.** `TaskStreams.stdout_file` / `stderr_file` (`src/executor/output.rs:156-158`), paths computed in `TaskStreams::new` (`:170-171`), written by `TeeWriter` (`:82-96`) and read back by `TaskStreams::read_output` (`:249-275`). Buffering needs no new capture mechanism and no new file, only a new emission policy and an existing reader.
- **Terminal emission is already suppressible.** `TeeWriter` carries `suppress_terminal` (used by TUI mode) and `no_prefix` (`src/executor/output.rs:89-95`); the terminal bytes are built by `format_terminal_output` (`src/executor/output.rs:102-109`).
- **stdout and stderr are separate files, and their relative order is ALREADY not preserved.** Two independent tokio drains per task (`src/executor/scheduler/task_execution.rs:252-273`), joined with a timeout at `:282-310`. Nothing serializes them against each other, so "arrival order at otto" is racy today. A merged capture would therefore buy no ordering guarantee that does not already exist.
- **otto-dev merges at the source anyway.** `svc.sh` runs `otto -C "$root" --no-prefix "$task" 2>&1 | filter_noise`, so for the actual consumer everything arrives on the subtask's stdout and `stderr.log` is empty.
- **The foreach item index already exists.** `expand_foreach_with_items` enumerates every expansion (`src/cfg/task.rs:793-797`) and injects `OTTO_FOREACH_INDEX` into each subtask's env (`:808`). The index is computed for parallel and serial alike; it is simply not carried onto the executor `Task`.
- **Foreach order is carried only for serial groups.** `serial_group` / `serial_index` on the parsed task (`src/cli/parser.rs:432-435`), assigned at `src/cli/parser/discovery.rs:319-322`. The doc comment says `serial_index` is "Meaningless when `serial_group` is `None`", so a parallel foreach expansion has no index today.
- **The TUI already keeps a per-task ring buffer**: `output_buffer: VecDeque<String>` at `src/tui/pane.rs:173`, capped at `max_buffer_lines` (`src/tui/pane.rs:215-217`). Precedent for per-task grouping exists; it is not reusable as-is (RAM-bounded, TUI-owned).
- **Global envs are already lazy and resolved at most once**: `global_envs()` at `src/cli/parser.rs:652-660`, behind `self.resolver`, delegating to `evaluate_envs`. `foreach.command` is handed them (`src/cli/parser.rs:669-671`), so anything `envs-command` produces is visible to a foreach command.
- **`--help` never evaluates envs. `--tasks` and `--list-subtasks` do, but only when a command-source `foreach` forces it.** Marker-file fixture, measured both ways: with no `foreach: command:` in the file, the marker is absent after `--help`, `--tasks`, and `--list-subtasks`, present after a real run. Add a `foreach: command:` to the same file and the marker appears after `--tasks` and `--list-subtasks` (both resolve foreach items, and item resolution is handed `global_envs()` at `src/cli/parser.rs:669-671`), while `--help` still leaves it absent. So the rule `envs-command` inherits is "never for `--help`, otherwise whenever something needs the env map", not "never for enumeration surfaces".
- **`tty: true` on a `foreach` task is legal today** and prints unprefixed blocks, because a tty task runs exclusively so its subtasks never overlap. Verified: `tty: true` + `items: [alpha, beta]` + `parallel: true` printed `beta` then `alpha`, each contiguous, no prefixes. That combination must keep working; only `buffer` + `tty` together is rejected.
- **`required` will only ever fire for a task named on the command line.** A dependency-only task's params are not bound from the CLI today: fixture `dep` with a `choices`-validated `-m|--mode`, pulled in by `main`, ran as `dep mode=[UNSET]` with no error. clap validates a task's args only when that task is matched, so `required` cannot break dependency chains. This is a property to state, not a risk to mitigate.
- **`param_to_arg` is the single clap construction site** (`src/cli/parser/command.rs:68-136`), and already sets `num_args` (`:106`), `default_value` (`:108-110`), and `PossibleValuesParser` (`:118-121`). `grep -rn '\.required(' src/` returns zero hits.
- **BLOCKER, found during research: clap never runs for a task named with no arguments.** `src/cli/parser/discovery.rs:235-237` gates the whole clap bind on `if let Some(args) = task_args && args.len() > 1`. The partition's first element is the task name, so `len() == 1` means "named, no args" and skips clap entirely. Verified: a task whose only param carries `choices: [alpha, beta]` runs as `[sw] svc=[UNSET]`, exit 0, on bare `otto sw`; supply any value and clap fires (`error: invalid value 'nope' for '[svc]'`). `required: true` alone therefore would NOT catch the exact case otto-dev needs. Phase 1 changes this gate.
- **SEPARATE DEFECT, found during research: `envs:` `$(...)` runs in the process cwd, not the ottofile's directory.** `global_envs()` evaluates with `Some(&self.cwd)` (`src/cli/parser.rs:657`) while `foreach.command` and `choices-command` run in `base_dir()` (`:634-636`). Verified against otto-dev's own file: `cd ~ && otto -o <otto-dev>/.otto.yml profiles` fails with `Command 'scripts/svc.sh root auth' failed with exit code 127: sh: 1: scripts/svc.sh: not found`. It works only because otto-dev is always driven with `-C` pointed at the ottofile's own directory. The defect fires on ANY invocation whose process cwd differs from the ottofile's directory: plain `otto <task>` from a subdirectory, `-C` aimed anywhere but the ottofile's own directory, `-o`, and `$OTTOFILE` (all four verified at a terminal, exit 127).

### Goals

- `foreach.buffer: true` runs subtasks concurrently and prints each subtask's output as one contiguous block, in foreach item order.
- `otto.envs-command: <cmd>` LAYERS `KEY=VALUE` lines from a command's stdout under the declared `otto.envs`, lazily and at most once, with the same execution contract `foreach.command` follows.
- `params.<title>.required: true` makes clap enforce the value, with a usage error instead of an empty variable.
- otto-dev can delete `stack.sh` `main()`, `gen-otto.sh`, `check.sh`'s drift gate, both marker pairs, and the `:?` guards. otto-dev is not modified by this doc.
- Every new key is additive: an ottofile that does not set them behaves byte-identically.

### Non-Goals

- **Per-task `buffer:` outside `foreach`.** Excluded. A single task's output is already contiguous; there is nothing to group.
- **Emitting blocks in completion order.** Excluded: item order is the property `stack.sh` implements and the reason it exists ("the same output as running them serially, minus the wait").
- **Replacing `stdout.log` / `stderr.log` with a single log.** Excluded. The split files are the run-dir contract exposed by `Workspace::stdout()` / `stderr()` (`src/executor/workspace.rs:283-290`) and asserted in `src/executor/workspace_tests.rs:140-141,260-261` and `src/executor/scheduler_tests_b.rs:730,786,793`; buffering reads the two that already exist and adds no file at all.
- **Task-scoped `envs-command`.** Parked. Revisit condition: a consumer needs per-task computed envs that `otto.envs-command` plus task `envs:` cannot express. Nobody has asked.
- **`required` on flags (`FLG`).** Excluded as meaningless: a required boolean that must always be passed is a constant.
- **Interpolating `$(...)` inside `envs-command` output.** Excluded on injection grounds (v2.0.0 containment precedent): command output is data, never re-evaluated as an expression.
- **Changing `--tui` behavior.** Under `--tui` the terminal leg is already suppressed; `buffer` is inert there.

## Relationship to In-Flight Docs (ship order)

- `docs/design/2026-08-30-audit-batch-handoff.md` is **In Progress** (batches 1-7 done, 8-14 TODO). Batch 9 audits "TUI and CLI surface" and collides with Phases 1-2 as well as Phase 4: its open items live in `src/cli/parser.rs:2463` and its shipped fix in the `discovery.rs:228-245` binding loop, while Phase 1's preflight lands at the top of that same function and Phase 2 edits `src/cli/parser.rs:657`. Batch 11 audits "Doc truth", which Phase 5 moves.
- **Ship order: finish batches 8-14 first.** Auditing a surface while this doc rewrites it produces findings against code that no longer exists. If that ordering slips, batches 9, 11, and 14 are re-run after this doc lands (batch 14 audits the Resolved Decisions log, whose code assertions Phase 1's preflight and Phase 2's `base_dir()` change can expire), and this doc's phases are treated as new audit scope.
- Everything else in `docs/design/` is `Implemented`. This doc extends `2026-08-28-boundary-fixes-and-dynamic-foreach.md` (same consumer, same lazy-once resolution rules) and supersedes none of it.

### Cross-repo blast radius

- **otto**: three additive schema keys, no new run-dir file, no change to scheduler execution structures, one docs page, `docs/commands/ottofile-reference.md` key inventory updated. One disclosed behavior change to a shipped surface: `envs:` substitutions resolve against the ottofile directory instead of the process cwd. The clap bind gate is NOT changed; required-param enforcement is a preflight ahead of it. Minor version bump.
- **tatari-tv/otto-dev**: no forced change. On adoption it deletes `scripts/stack.sh:296-378`, `scripts/gen-otto.sh` (100 lines), the `check.sh` drift gate, both BEGIN/END marker pairs in `.otto.yml`, and the `:?` guards. That is otto-dev's PR, on otto-dev's schedule.
- **Other consumers**: the three new keys are inert unless set, but the `base_dir()` cwd fix reaches ANY ottofile that uses `$(...)` in `otto.envs` whenever the process cwd differs from the ottofile's directory: plain `otto <task>` from a subdirectory, `-C` aimed anywhere but the ottofile's own directory, `-o`, and `$OTTOFILE` (all four verified). Measured against all 125 ottofiles under `~/repos` (panel round 5): 138 `otto.envs` `$(...)` substitutions across 113 files, 130 cwd-insensitive (`git describe`, `git rev-parse`, `date`), 8 cwd-sensitive, 7 of those at `tatari-tv/otto-dev/.otto.yml:76-82`, the exact lines the fix repairs. Zero measured breaks: for every surveyed consumer the change is a no-op or a repair.

## Proposed Solution

### Overview

| Feature | Key | Mechanism |
|---|---|---|
| Buffered foreach | `tasks.<n>.foreach.buffer: bool` | suppress the terminal leg per subtask, replay the existing `stdout.log` then `stderr.log` in item order under an output lock |
| Computed envs | `otto.envs-command: string` | resolve inside `global_envs()`, LAYER onto the inherited env so literal `envs:` still wins |
| Required params | `tasks.<n>.params.<t>.required: bool` | `arg.required(true)` in `param_to_arg`, plus a preflight so a bare `otto <task>` errors without resolving dynamic config |

### Architecture

#### 1. `foreach.buffer`

Three pieces:

- **A display-order map, built where the order already exists. The scheduler's execution structures are not touched.** `expand_foreach_with_items` already enumerates every expansion and injects `OTTO_FOREACH_INDEX` (`src/cfg/task.rs:793-797,808`). That same enumeration emits a `HashMap<String, Vec<String>>` of parent task name -> subtask names in item order, threaded to the scheduler and read ONLY by the replay cursor. `serial_group` / `serial_index` (`src/cli/parser.rs:432-435`) are left exactly as they are.

  This reverses an earlier draft of this doc, which renamed those two fields to `foreach_group`/`foreach_index` and added `serial: bool` across 63 references in 13 files (counts confirmed by the review panel). The named flaw: rewriting the dependency scheduler's state to obtain a display-ordering index is an architectural boundary violation, and it is scope Scott did not ask for. The map gets the same ordering with an additive structure the executor can ignore. The redundancy objection (two things encode subtask order) is real but small: `serial_index` orders execution, the map orders display, and they are read by different code for different reasons.
- **No new capture. Replay the two logs that already exist**, `stdout.log` then `stderr.log`, streamed with `std::io::BufReader` in bounded chunks. NOT via `TaskStreams::read_output` (`src/executor/output.rs:249-275`): that is `async fn -> Result<Vec<String>>`, built on `tokio::fs` with `.await` inside its read loop, and it collects the whole log into memory. It is async where replay must be blocking, and unbounded where replay must be bounded. It stays as it is for its one existing caller. A merged `output.log` was designed first and cut: the two streams are drained by independent tokio tasks (`src/executor/scheduler/task_execution.rs:252-273`), so otto's own arrival order is already racy and a merged file would promise an ordering it cannot keep. For otto-dev it changes nothing: `svc.sh` already runs the inner otto with `2>&1`, so a subtask's stderr log is empty.
- **An ordered replay gate in the scheduler.** Buffered subtasks run with `suppress_terminal: true`. A per-parent replay cursor starts at index 0 and, whenever the subtask at the cursor reaches a terminal state, streams its logs to the terminal and advances, draining any already-finished successors. Blocks therefore emit in item order, and a completed block is never held for longer than the slowest earlier item.

  **The cursor must be driven from all four terminal-transition sites, not two.** Only the success and failure arms go through the `mpsc` report channel; both skip paths mutate the terminal-state sets inline in the launch loop and send no `TaskReport`. A gate hung on the report arms alone would never emit a block for a skipped subtask and would stall every later item behind it. The cursor is therefore a plain local beside `completed_set` / `failed_set` / `skipped_set` on the `execute_all` frame, advanced by one helper called from all four sites: no `Arc<Mutex>`, no lock ordering. The helper lives in a new `scheduler/` sibling module, because `scheduler.rs` is 1288 lines against the 1500-line cap, which is why `support.rs` and `task_execution.rs` are already `include!`d siblings.

Per-line prefixes are preserved inside a block. Contiguity is what was missing, not attribution; `--no-prefix` still strips them, one rule for both modes.

**Hook points**, so the phase is not a search:

| What | Where |
|---|---|
| Subtask -> parent link | `Task.parent: Option<String>` (`src/executor/task.rs:40`), `Task.is_virtual_parent` (`:51`) |
| Ready loop and terminal-state sets | `completed_set` `src/executor/scheduler.rs:936`, `failed_set` `:937`, `skipped_set` `:938`, all locals on the single `execute_all` frame |
| Terminal transition 1: success arm | `src/executor/scheduler.rs:1041-1173` (through the `mpsc` report channel, created `:898`, received `:1029-1039`) |
| Terminal transition 2: failure arm | `src/executor/scheduler.rs:1174-1242` (through the same channel) |
| Terminal transition 3: gated-out skip | `mark_skipped` (`src/executor/scheduler/support.rs:9-29`), called from `scheduler.rs:959, 1016, 1170, 1234, 1255`. **No report is sent.** |
| Terminal transition 4: up-to-date skip | `src/executor/scheduler/support.rs:60-117`, sets mutated `:91-93`. **No report is sent.** |

Two paths that look like a fifth and sixth site are not: a task body that ends without reporting is reaped and synthesized into a `TaskReport::failure` (`src/executor/scheduler.rs:1029-1035`), so it arrives through the failure arm; and the post-loop reconciliation of still-blocked tasks (`:1250-1256`) calls `mark_skipped`, which is already one of the five call sites above. Four sites cover every terminal transition.

| End-of-group flush | parent's success arm `src/executor/scheduler.rs:1063-1100`, reachable because the virtual parent carries `When::Always` edges to every subtask (`src/cli/parser/foreach.rs:74-86`) |
| Per-task log paths | `Workspace::stdout()` / `stderr()` (`src/executor/workspace.rs:283-290`). No new file. Replay opens them with `std::fs` and streams; `TaskStreams::read_output` (`src/executor/output.rs:249-275`) is async and reads whole-file, so it is not the replay path. |
| Interrupt path | `abandon_run` (`src/executor/scheduler/support.rs:145-155`) |
| Terminal write | `TeeWriter::write` (`src/executor/output.rs:122-146`) |

**A block must not be split by anything else that writes to the terminal.** There is no shared lock today, and `TeeWriter` is not the only writer. Seven unlocked sites exist, and all seven take the lock:

| Site | What it writes |
|---|---|
| `src/executor/output.rs:131` | task stderr line |
| `src/executor/output.rs:133` | task stdout line |
| `src/executor/scheduler.rs:1112` | task success status line |
| `src/executor/scheduler.rs:1193` | task failure message |
| `src/executor/scheduler/support.rs:14` | `mark_skipped` message |
| `src/executor/scheduler/support.rs:72` | up-to-date skip message |
| `src/executor/scheduler/support.rs:150` | run-cancelled message |

Naming only `TeeWriter` would have left five of them free to split a replayed block, including the status and skip lines that fire most often during a parallel group. Replay holds the lock for one whole block; every site above takes it per write.

**Replay runs inside `tokio::task::spawn_blocking`, and the scheduler awaits the handle.** Reading a log and writing it under a held lock is blocking work; doing it inline in the `execute_all` ready loop would stall a tokio worker and starve every concurrent task, and the block cannot be assembled with `await` points inside the lock either. `spawn_blocking` resolves both: the lock is taken and released entirely inside one blocking closure, using `std::fs` and a locked `io::stdout()`, and the async side only awaits the join. The review panel raised the starvation risk against an earlier draft that tried to do this inline.

#### Edge cases

- **The subtask's status line travels with its block.** `[say:alpha] finished successfully` is emitted by the scheduler, not by the task, so under buffering it is appended to the block at replay time rather than printed when the subtask finishes. Otherwise the status lines arrive in completion order while the blocks arrive in item order.
- **A skipped or failed subtask still occupies its slot.** The cursor advances on any terminal state. A skipped item replays its (possibly empty) log plus its skip reason, in position; a failed item replays its block and the parent still fails.
- **The cursor cannot outlive the group.** The virtual parent carries `When::Always` edges to every subtask (`src/cli/parser/foreach.rs:74-86`), so it is queued only after all subtasks are terminal. Its success arm is a guaranteed end-of-group flush point for anything the four-site cursor did not already emit: a backstop, not the mechanism.
- **Aggregation order is not disturbed.** The parent's status override must run before `completed_set.insert` and before the blocked-tasks sweep (`src/executor/scheduler.rs:1058-1062`, stated in the code). The flush hooks after that override, never before it.
- **Replay does not collide with the one production log reader.** The only place log CONTENTS are read today is the failure preview at `src/executor/scheduler/task_execution.rs:316-324` (last 20 lines of `stderr.log`); everything else records paths only.
- **Empty output is a no-op, not a header.** A subtask that printed nothing contributes nothing but its status line.
- **Terminal state is not the same as output complete.** `src/executor/scheduler/task_execution.rs:282-310` only `error!`-logs a drain failure or a 5s drain timeout, then falls through to `if status.success() { Ok(()) }` at `:312`. A subtask can therefore report success with a partially written `stdout.log`, and a naive replay would print a truncated block and append "finished successfully". Buffered replay records whether the drain completed and, when it did not, ends the block with a loud marker naming the condition and the log path. **The carrier is named, so Phase 4 does not start with a search:** `TaskReport` (`src/executor/scheduler.rs:57-65`) carries only `name`, `exit_code`, and `error` today, and gains `drain: Vec<DrainIssue>` where `DrainIssue { stream: OutputType, condition: DrainCondition }` and `DrainCondition` is one of `ProcessingError`, `JoinError`, `Timeout`. A bool will not do: `task_execution.rs:282-310` distinguishes six outcomes (three conditions per stream), and the doc promises a marker "naming the condition and the log path", which a bool cannot express. Empty vec means a clean drain. Skip paths send no report and ran no process, so they carry nothing. Visible degradation, never a silent truncation. The underlying silent-success in the drain path is a shipped defect wider than this feature; it is named in Risks and left for its own targeted fix rather than widened into this doc.
- **Stream order inside a block is stdout then stderr**, documented rather than promised as emission order. Nothing is lost: the two drains are already unsynchronized, so no ordering guarantee exists today to preserve.
- **Interrupt, with a named mechanism.** `abandon_run` (`src/executor/scheduler/support.rs:145-155`) aborts the in-flight children, persists skip records, and returns `Err`. It never marks in-flight tasks terminal and never touches a log, so an unmodified `abandon_run` would drop every buffered block that had completed but not yet replayed. It is called from `execute_all`, so it takes `&mut cursor` and, before returning `Err`, flushes the blocks of subtasks that ARE terminal. Killed in-flight subtasks are NOT replayed: their logs are partial by construction. Their run-dir paths are printed instead, so nothing is silently discarded and nothing is silently truncated.
- **`-j 1` or `parallel: false`.** Subtasks cannot overlap, so buffering is inert; the output is identical either way.

#### 2. `otto.envs-command`

`global_envs()` (`src/cli/parser.rs:652-660`) becomes:

0. **First, fix the cwd disagreement.** `envs:` `$(...)` evaluates with the process cwd (`src/cli/parser.rs:657`); every other command source uses `base_dir()` (`:634-636`). That is the defect verified above, reachable through `-o` and `$OTTOFILE`. `global_envs()` switches to `base_dir()`, so `envs:`, `envs-command`, `foreach.command`, and `choices-command` all resolve relative paths against the ottofile. This is a behavior change to a shipped surface, called out here rather than buried: a file that relies on the process cwd for an `envs:` substitution changes meaning. No committed example or consumer does, and `-C` users see no difference because the two paths coincide there.
1. If `envs-command` is set, run it: cwd = the ottofile's directory (`base_dir()`, now the one contract for all four), a scrubbed environment plus the inherited process env, non-zero exit is a loud error naming the command and its stderr.
2. Parse stdout as `KEY=VALUE`, one per line. Split on the FIRST `=`. Skip blank lines and lines whose first non-space character is `#`. A key that is not `[A-Za-z_][A-Za-z0-9_]*` is a loud error naming the line number and the line. A line with no `=` is the same error. Values are taken literally: no unquoting, no `$(...)` re-evaluation, no `${VAR}` expansion.
3. Layer, do not merge. The command's output is applied as an override on the INHERITED environment that `evaluate_envs` builds from `env::vars()` (`src/cfg/env.rs:32`), and literal `envs:` are then evaluated against that layered base. **This requires a signature change**: `evaluate_envs(envs, working_dir)` (`src/cfg/env.rs:7-10`) builds `inherited` internally with no way to seed it, so it gains a third parameter carrying the base overrides. Existing callers pass an empty map and behave identically. Naming it here because "layer onto inherited" is not expressible against the function as it ships. Two consequences, both wanted:
   - An explicit `envs:` entry still wins the final value for the same key.
   - A shadowing literal key that self-references resolves to the COMPUTED value, because `evaluation_context` seeds the key's inherited value (`src/cfg/env.rs:130-131`) and the inherited layer now carries it. Merging the command's output into the declared map instead, as an earlier draft did, would discard the computed value and silently resolve `FOO: '$(echo "${FOO:-x}")'` to the OS value. The review panel found that; it is the difference between layering and merging.
4. Resolve INSIDE the existing `global_envs` init closure (`src/cfg/resolver.rs:71-79`), ahead of `evaluate_envs`, so it runs at most once per invocation and never for `--help`. It must not go through `DynamicResolver`'s own caches the way `foreach.command` and `choices-command` do: those sit downstream of `global_envs`, and re-entering the `OnceCell` from inside its own initializer panics. What is reused is the execution contract, not the cache cell.
5. It DOES run on `--tasks` / `--list-subtasks` when a command-source `foreach` is present, because item resolution needs the env map: the same rule literal `envs:` already follows (see Verified facts).

Recursion guard: `ENVS_GUARD_VAR = "OTTO_ENVS_COMMAND"`, declared beside `FOREACH_GUARD_VAR` (`src/cfg/resolver.rs:33`) and `CHOICES_GUARD_VAR` (`:37`). The guard is a comma-separated CHAIN, not a boolean: `run_lines_command` reads it (`:142`), errors if the key is already present naming the closing cycle (`:143-149`), and extends it for the child (`:150-154,161`). `envs-command` has one resolution per invocation, so its guard key is a fixed literal rather than a task name.

**The execution contract is shared, not re-implemented.** `run_lines_command` (`src/cfg/resolver.rs:140-191`) already does the guard chain, `sh -c`, `.current_dir(cwd)`, `.envs(envs)` with no `env_clear`, the exit-code error format including a trimmed stderr detail (`:166-179`), and stderr passthrough on success (`:181-184`). Two adjustments:

- **Split off the raw form.** `run_lines_command` finishes with `.lines().map(str::trim).filter(...)` (`:186-191`), so `KEY=  spaced value  ` would silently lose its whitespace. Extract everything above that into `run_command_stdout` returning raw stdout, and leave `run_lines_command` as the trim-and-filter wrapper over it. Zero behavior change for foreach and choices, both of which want trimmed lines; `envs-command` calls the raw form so a value survives byte-for-byte.
- **No global envs to pass.** The existing callers hand `.envs()` the resolved global env map. `envs-command` runs while that map is being computed, so it gets the inherited process environment only. Stated because it is the one place the contract cannot be identical.

Empty stdout is legal and means "no variables", unlike `choices-command` where empty is a misconfiguration. An env set can legitimately be empty on a machine with nothing cloned; a validation set cannot.

#### Edge cases

- **Duplicate key in the output:** last wins, matching `env` and `export` semantics. Not an error.
- **CRLF:** a trailing `\r` is stripped from each line. A generator run on a Windows-ish toolchain otherwise produces values with an invisible carriage return.
- **A value cannot contain a newline**, by construction of the line format. Multi-line values stay in literal `envs:`, and the docs page says so.
- **The command's stderr passes through to otto's stderr** unchanged, so a generator's own warnings are visible; only stdout is parsed (`src/cfg/resolver.rs:181-184`).
- **Whitespace in a value survives.** `KEY=  spaced  ` keeps its spaces, which is why `envs-command` uses the raw-stdout form rather than the line-trimming one. Leading whitespace before the KEY is still skipped, and a blank line is still ignored.
- **A key that collides with an essential variable** (`PATH`, `HOME`) is accepted and applied, exactly as a literal `envs:` entry would be. `envs-command` grants no capability that `envs:` did not already have.

#### 3. `params.<title>.required`

`ParamSpec` gains `required: bool` (default `false`, `#[serde(default)]`). `param_to_arg` (`src/cli/parser/command.rs:68-125`) sets `arg.required(true)` for `OPT` and `POS`. Load-time errors:

- `required: true` on a `FLG` (a title with no value, e.g. `-v|--verbose`): rejected, named.
- `required: true` together with `default:`: rejected, named. A default makes required unreachable, and two keys encoding opposite intent is the cognitive-dissonance case.
- `required: true` with `nargs` `"0"`, `"?"`, or `"*"`: rejected, named. Those spellings all mean "may appear zero times", which is the negation of required. Allowed with `"1"`, `"+"`, a bare `"N"`, and `"N:M"` where the minimum is at least 1.
- **A required positional after an optional positional: rejected at load, named.** clap panics when a required positional follows an optional one, and a panic from a config file is never an acceptable error path. The check runs over the task's positionals in declaration order.

**A preflight, NOT a widened clap gate.** `src/cli/parser/discovery.rs:235-237` runs clap only when a task's CLI partition has more than one element, so `otto switch` with no arguments never reaches clap and a required param would not fire. An earlier draft widened that gate to `args.len() > 1 || task_has_required_param(...)`. The review panel killed it, and the chain it traced is verified: the widened gate reaches `task_to_command(clap_spec, BuildMode::Bind)` (`discovery.rs:242`) -> `param_to_arg` -> `param_choices` (`command.rs:112`) -> `resolve_choices_command` (`command.rs:147-157`), which also calls `self.global_envs()`. Bare `otto switch` would go from running zero subprocesses to running `scripts/svc.sh exposing switch` AND resolving the whole env map (and, with Phase 2, the `envs-command` subprocess) purely to print a missing-argument error. That lands on otto-dev's `switch` task specifically, the task this feature exists for.

The gate is therefore left exactly as it is. Instead, the preflight goes at the TOP of `process_tasks_with_filter`, **before its Step 0 `global_envs()` call** (`src/cli/parser/discovery.rs:167-171`), not merely ahead of the clap gate at `:235`. Placing it at the gate would be too late: globals resolve at `:171` and task envs at `:225`, both before it, so after Phase 2 the `envs-command` subprocess would already have run. The preflight needs only `self.pargs` and `self.config_spec.tasks`, neither of which requires an env map.

```rust
fn process_tasks_with_filter(&self, requested_tasks: &[String]) -> Result<Vec<Task>> {
    // BEFORE Step 0. Reads partitions and ParamSpecs only: no global_envs(),
    // no task env evaluation, no clap Command, no dynamic choices.
    self.preflight_required_params(requested_tasks)?;

    // Step 0: Evaluate global environment variables once  (discovery.rs:171)
    let global_envs = self.global_envs()?.clone();
```

`preflight_required_params` walks each requested task's partition; where the partition length is 1 (task named, zero arguments) and the spec declares required params, it returns the usage error naming them.

Properties, all of which the widened gate lacked:

- **Zero NEW subprocesses on any path.** The bare-invocation error is produced from the spec alone: no `choices-command`, no `envs-command`, no `global_envs()`.
- **Every other path is unchanged.** A task with no required params never reaches the preflight's body; a task invoked WITH arguments takes today's gate.
- **Defaults are untouched.** They are applied at `discovery.rs:331+`, outside the gate, which the panel verified.

**The boundary, stated rather than glossed:** "an error path has no side effects" holds for the zero-argument case only. `otto sw --verbose` has `args.len() == 2`, skips the preflight, enters the gate, and resolves `choices-command` plus `global_envs()` before clap reports the missing required value (verified chain: `discovery.rs:235,242` -> `command.rs:112,147-157`). That is exactly what happens today for any task invoked with arguments, so this doc neither creates nor removes it; the preflight covers the case the feature creates, which is the bare invocation that runs nothing at all today. Removing it from the N>0 path means making dynamic-choice resolution lazy inside clap, a separate change nobody asked for.

The cost of the preflight is that the missing-argument message is otto's rather than clap's for that one case. Right trade: the case the feature introduces should not gain side effects.

**`required` is a CLI-surface constraint, not a data constraint.** `propagate_params` (`src/cli/parser/params.rs:43-98`) resolves values from dependents and re-validates `choices`, but it does not check `required`: a param with no inherited value is skipped outright. So a task that declares a required param and is ALSO reachable as a dependency runs with that param unset when nothing supplies it. That is exactly how `choices` behaves today (verified: a dependency-only task with `choices: [alpha, beta]` ran unset, exit 0), so `required` matches its sibling rather than inventing a second enforcement model. Making it a data constraint means validating propagated values, which is a different feature nobody asked for. The review panel raised this as a correctness hole; it is a documented boundary.

Enforcement scope, stated because it is load-bearing: `required` fires only when the task is NAMED on the command line. A task pulled in as a dependency has no CLI partition at all, so it never reaches clap (verified: a dependency-only task with a `choices`-validated param ran with it unset, exit 0). Tasks selected by `otto.tasks:` defaults have no partitions either, and are likewise unaffected.

### Data Model

```yaml
otto:
  api: 1
  envs-command: "scripts/svc.sh roots"     # stdout: KEY=VALUE lines
  envs:
    WEB_ROOT: /explicit/wins             # beats the command's WEB_ROOT

tasks:
  status:
    foreach:
      command: "scripts/stack.sh scope status"
      as: svc
      parallel: true
      buffer: true                          # NEW
    bash: |
      scripts/svc.sh run "${svc}" status

  switch:
    params:
      svc:
        choices-command: "scripts/svc.sh exposing switch"
        required: true                      # NEW
      branch:
        required: true                      # NEW
    bash: |
      scripts/svc.sh run "${svc}" switch "${branch}"
```

No run-dir change. Buffered replay reads the two per-task logs that already exist:

```
<run>/tasks/status:web/stdout.log     unchanged, replayed first
<run>/tasks/status:web/stderr.log     unchanged, replayed second
```

### API Design

- `ForeachSpec` gains `buffer: bool` (`#[serde(default)]`), joining `glob`/`items`/`range`/`command`/`as`/`parallel`/`max_items`. 7 keys -> 8.
- `OttoSpec` gains `envs_command: Option<String>`, serde-renamed to `envs-command` (kebab, matching `choices-command` and `on-failure`). 7 keys -> 8.
- `ParamSpec` gains `required: bool` (`#[serde(default)]`). 6 keys -> 7.
- `evaluate_envs` (`src/cfg/env.rs:7-10`) gains a base-override map parameter. Internal Rust API, not an ottofile key; existing callers pass an empty map.
- `docs/commands/ottofile-reference.md` total moves from 42 fixed keys to 45.
- `--tasks` output carries `required` **only when it is true**, via `#[serde(skip_serializing_if = "is_false")] pub required: bool` on `ParamView` (`src/cli/commands/tasks.rs:39-46`), the pattern that struct already uses for `choices` and `choices-command`. Emitting `required: false` for every plain param would change `--tasks` output for every existing ottofile that has params and sets none of the new keys, breaking this doc's own additivity goal. A doctor-style consumer still learns whether a delegated task can be invoked without arguments.
- No CLI flag is added by this doc.

### Implementation Plan

#### Phase 1: `params.<title>.required`
**Model:** sonnet
- `required: bool` on `ParamSpec` with `#[serde(default)]`; `arg.required(true)` in `param_to_arg` (`src/cli/parser/command.rs:68-136`) for `OPT`/`POS`.
- Preflight at the TOP of `process_tasks_with_filter`, before its Step 0 `global_envs()` (`src/cli/parser/discovery.rs:167-171`), NOT at the clap gate and NOT a widened gate: a task named with zero arguments that declares required params errors from partitions and `ParamSpec` alone. The gate is unchanged, so no path gains a `choices-command` subprocess, and the placement is what keeps `envs-command` off this path too. Without the preflight the key is inert on exactly the case it was asked for; with a widened gate it would resolve dynamic config on an error path.
- Load-time rejection of: `required` on `FLG`; `required` + `default:`; `required` with `nargs` `0`/`?`/`*`; a required positional declared after an optional one. Each names the param path. The last one exists because clap panics on that shape.
- `required` in the `--tasks` param entries as `#[serde(skip_serializing_if = "is_false")] pub required: bool` on `ParamView`, so a plain param emits nothing new; roundtrip test updated.
- **Success criteria:** (a) bare `otto sw` on a fixture with `required: true` exits non-zero with OTTO's usage error naming `svc` (not clap's: the preflight answers before clap is built, which is the whole point); (b) `otto sw alpha` runs and binds it; (c) a task carrying a required param, pulled in only as a dependency, still runs with the param unset, exit 0, matching `choices` behavior; (d) each of the four rejected combinations (`FLG`, `default:`, `nargs` in {`0`,`?`,`*`}, required positional after optional positional) fails the load with an error naming the param path, and none of them panics; (e) for every example ottofile under `examples/` plus the otto-dev tree at `659c0ef`, `otto --tasks --format json` is byte-identical before and after, and for the subset `tests/examples_integration_test.rs:125-139` already executes as safe, `otto <task>` stdout+stderr+exit is byte-identical too. NOT `otto <each task>` across all examples: those bodies compile, delete, package, and deploy into `$HOME` (`examples/build-pipeline/otto.yml:13-37`, `examples/interactive-demo/otto.yml:4-51`), which is why the suite parses most and runs few; (f) bare `otto switch` on a fixture that sets `otto.envs-command` AND a param carrying `required: true` with a `choices-command`, where BOTH commands touch distinct marker files, leaves BOTH markers absent. Asserting only the choices-command marker would go green even with the preflight placed after Step 0, since `global_envs()` resolves at `discovery.rs:171` roughly 64 lines before the clap gate; (g) `otto --tasks --format json` reports `required: true` for the required param and omits the key entirely for a plain one, and the full `--tasks` output for an ottofile that sets none of the three new keys is byte-identical to the pre-change output, diffed against a locally captured baseline of otto-dev's real `--tasks --format json` output (21 tasks, param keys today: `choices`, `choices-command`, `default`, `flags`, `name`, `positional`; captured during Phase 1, deliberately NOT committed — this repo is public and the dump names internal services). Every phase additionally ends `otto ci` green.

#### Phase 2: `otto.envs-command`
**Model:** opus
- **First:** switch `global_envs()` from `Some(&self.cwd)` to `base_dir()` (`src/cli/parser.rs:657`), closing the verified cwd defect (plain subdirectory invocation, `-C` elsewhere, `-o`, `$OTTOFILE`) and giving all four command surfaces one cwd contract. Disclosed behavior change, own commit inside the phase.
- `envs_command: Option<String>` on `OttoSpec`, kebab-renamed; resolved inside the `global_envs` init closure (`src/cfg/resolver.rs:71-79`) ahead of `evaluate_envs`. Not through `DynamicResolver`'s `RefCell` caches (`:55-57, 85-95, 109-119`): those are downstream of `global_envs`, and re-entering the `OnceCell` from its own initializer panics. The `OnceCell` already gives once-per-invocation.
- Extract `run_command_stdout` out of `run_lines_command` (`src/cfg/resolver.rs:140-191`), keeping the latter as the trim-and-filter wrapper, so `envs-command` shares the guard/exec/exit/stderr contract without inheriting the per-line `str::trim` that would eat whitespace in a value. Add `ENVS_GUARD_VAR` beside the other two constants.
- `KEY=VALUE` parser: first-`=` split, trailing `\r` stripped, blank and `#` lines skipped, invalid key or missing `=` is a loud error naming line number and content, values taken literally, duplicate keys last-wins.
- Precedence by LAYERING, not merging, which means `evaluate_envs` (`src/cfg/env.rs:7-10`) gains a base-override parameter; existing callers pass an empty map. The command's output overrides the inherited map built from `env::vars()` (`src/cfg/env.rs:32`); literal `envs:` are evaluated against that base and still win the final value. A shadowing literal that self-references therefore sees the computed value via `evaluation_context` (`src/cfg/env.rs:130-131`). `OTTO_ENVS_COMMAND` recursion guard, as a constant beside `FOREACH_GUARD_VAR` (`src/cfg/resolver.rs:33`).
- **Success criteria:** (a) fixture `envs-command: "printf 'FOO=bar\nBAZ=qux\n'"` makes both visible to a task body and to a `foreach.command`, and `tests/foreach_command_test.rs` plus `tests/dynamic_choices_test.rs` pass untouched across the `run_command_stdout` extraction; (b) a literal `envs: {FOO: explicit}` beats the command's `FOO`; (c) `otto --help` on that fixture leaves untouched a marker file the command creates, and a real run creates it (`--tasks` is NOT asserted absent: a command-source foreach legitimately forces env resolution there, see Verified facts); (d) `printf 'not-a-kv\n'` fails the load naming line 1; (e) `cd / && otto -o <otto-dev>/.otto.yml profiles` resolves `WEB_ROOT` instead of failing with `sh: 1: scripts/svc.sh: not found`, and plain `otto <task>` from a SUBDIRECTORY of a fixture whose `envs:` runs `$(scripts/...)` resolves too (the no-flag shape, verified failing with exit 127 today); (f) with `envs-command` emitting `FOO=computed` and literal `envs: {FOO: '$(echo "${FOO:-fallback}")'}`, the task sees `FOO=[computed]`, not `[fallback]` and not the OS value; (g) a `choices-command` reads a variable produced by `envs-command`, covering the third consumer of `global_envs()` alongside task bodies and `foreach.command`. Every phase additionally ends `otto ci` green.

#### Phase 3: Display-order map and the `buffer` key
**Model:** sonnet
- Emit a `HashMap<String, Vec<String>>` (parent task -> subtask names in item order) from the enumeration that already produces `OTTO_FOREACH_INDEX` (`src/cfg/task.rs:793-797,808`), and thread it to the scheduler. Additive; read only by the replay cursor in Phase 4.
- `serial_group` / `serial_index` are NOT touched, and no scheduler execution structure changes.
- `buffer: bool` added to `ForeachSpec`. Validation: `buffer: true` with `tty: true` on the same task is a load error (a tty task owns the terminal; there is nothing to buffer). `tty: true` on a foreach task WITHOUT `buffer` keeps working, as verified on main. `buffer` with `parallel: false` is accepted and inert. `buffer` under `--tui` is accepted and inert.
- No behavior change yet: this phase is the index plus the schema key.
- **Success criteria:** (a) `tests/serial_foreach_test.rs` and `tests/flag_integration_test.rs` pass completely untouched, and `rg 'serial_group|serial_index' src/ tests/ | wc -l` still returns 63 (`rg -c` over two paths prints per-file counts, not a total); (b) a `parallel: true` expansion's map entry lists subtask names in declared item order, matching each subtask's own `OTTO_FOREACH_INDEX`; (c) the `buffer` + `tty` fixture fails to load naming both keys, while the `tty` + foreach fixture without `buffer` still runs. Every phase additionally ends `otto ci` green.

#### Phase 4: Buffered capture and ordered replay
**Model:** opus
- `suppress_terminal: true` for buffered subtasks. Replay opens `stdout.log` then `stderr.log` with `std::fs` and streams them in bounded chunks. `TaskStreams::read_output` is deliberately not used: it is async and whole-file.
- Process-wide output lock taken by all seven terminal-writing sites (`output.rs:131,133`; `scheduler.rs:1112,1193`; `scheduler/support.rs:14,72,150`), held for one whole block during replay.
- `TaskReport` gains `drain: Vec<DrainIssue>` (`{stream, condition}`, condition one of processing-error / join-error / timeout), populated from the six arms at `task_execution.rs:282-310`.
- Per-parent replay cursor as a local on the `execute_all` frame beside `completed_set` / `failed_set` / `skipped_set` (`src/executor/scheduler.rs:936-938`), advanced by one helper in a new `scheduler/` sibling module and called from all four terminal-transition sites: the success arm (`:1041-1173`), the failure arm (`:1174-1242`), `mark_skipped` (`scheduler/support.rs:9-29`, five call sites), and the up-to-date skip (`scheduler/support.rs:60-117`).
- Each advance streams the cursor item's two logs, appends its status line, and drains finished successors, all inside one `tokio::task::spawn_blocking` closure that takes the output lock, uses `std::fs`, and releases before returning. Item order comes from the Phase 3 map.
- A block whose drain did not complete (`task_execution.rs:282-310`) ends with a loud marker naming the condition and the log path, never a silent truncation.
- End-of-group flush in the parent's success arm (`:1063-1100`), hooked AFTER the aggregation override so the ordering constraint at `:1058-1062` is preserved.
- `abandon_run` (`scheduler/support.rs:145-155`) takes `&mut cursor` and flushes before returning `Err`.
- **The cancellation flush is ordered but never stops early.** This is the one sentence the design needs and the review panel was right to demand it: on cancellation the flush walks the parent's whole item list in order and emits exactly one thing per item, never halting at the first non-terminal one:

  | Item state at cancellation | What is emitted |
  |---|---|
  | Terminal (completed, failed, skipped) | its block, as in a normal run |
  | Report sent but not yet consumed (`task_execution.rs:422` vs `scheduler.rs:1029-1038`) | its block: the logs are complete, only the scheduler had not read the report |
  | Active body, child launched and killed (`task_execution.rs:233-249`) | its run-dir log paths, never a partial block |
  | Active body spawned, child not yet launched (`task_execution.rs:30` vs `:233-237`) | a did-not-start line; there is no log to point at |
  | Ready-queued, never started (`scheduler.rs:950-951`) | a did-not-start line |
  | Blocked, never started | a did-not-start line |

  Six states, not three. `ActiveTasks` tracks spawned BODIES, not child or log state (`scheduler.rs:166-230`), so "active" and "has a killed child with logs" are not the same thing, and a body that finished and sent its report is neither killed nor unstarted. Collapsing these would either print a path that does not exist or discard a complete block.

  A strictly-ordered flush that stopped at the first non-terminal item would lose every later completed block behind a killed or unstarted earlier one. That is the exact stall class the four-site cursor exists to prevent, reappearing on the one path the four sites do not drive: cancellation returns from the `is_cancelled` check at `scheduler.rs:944-945` and the `select!` cancel arm at `:1036-1037`, both BEFORE the post-loop reconciliation at `:1253-1256` that would otherwise mark blocked tasks skipped. Neither funnel runs on this path, so the flush cannot rely on either.
- Exit aggregation unchanged: buffering changes when bytes print, never what the parent's result is.
- **Success criteria:** (a) the three-item fixture under `parallel: true, buffer: true` prints all of alpha's lines, then all of beta's, then all of gamma's, with zero interleaving; (b) concurrency is proved by a barrier, not a stopwatch: each item writes to and then reads from a shared FIFO, so the run completes only if all three overlap and times out otherwise; (c) a failing middle item still prints its block and the parent still exits non-zero; (d) with a chatty unbuffered task AND a task that emits skip/status lines running alongside the buffered group, no replayed block contains a line from either (this covers the five non-`TeeWriter` write sites, not just task output); (e) a group whose FIRST item is skipped by a gate still emits the later items' blocks, in order, and does not stall (the case a report-channel-only cursor would hang); (f) SIGINT mid-group with a group wide enough to hold every state in the cancellation table at once prints, in item order and with no early stop: blocks for terminal and report-unread items, log paths for the killed-child item, did-not-start lines for the body-spawned-no-child, ready-queued, and blocked items, and a LATER item's completed block behind all of them, with no partial block anywhere; (g) a subtask that leaves a background child holding the stdout pipe open (`bash -c 'sleep 30 & echo hi'`) trips the drain timeout for real, and replays with the truncation marker present, never a clean "finished successfully" over a short block. `OUTPUT_PROCESSING_TIMEOUT_SECS` is a hard-coded const (`src/executor/scheduler.rs:34`) with no injection point, so the test provokes the condition rather than shortening the timeout. Every phase additionally ends `otto ci` green.

#### Phase 5: Docs and examples
**Model:** sonnet
- `docs/commands/ottofile-reference.md`: three new key rows, the 42 -> 45 total, and the kebab-key list updated (`envs-command` joins `on-failure` and `choices-command`).
- A `docs/commands/buffered-foreach.md` page covering the item-order guarantee, the stdout-then-stderr replay order, the `tty` conflict, and the `--tui` inertness.
- One example ottofile exercising all three keys, picked up by `tests/examples_integration_test.rs`.
- **Success criteria:** (a) `otto examples` passes; (b) `docs/commands/ottofile-reference.md` contains a row for each of `otto.envs-command`, `tasks.<name>.foreach.buffer`, and `params.<title>.required`, and its stated total reads 45, not 42; (c) `otto ci` green.

## Acceptance Criteria

Fixtures live under `/tmp/claude-1000/fx`. Every `Observed on main` line was run on 2026-08-31 against installed `otto v2.0.5` at `4d9ca4e`.

- [ ] **Buffered foreach:** fixture with three items, `parallel: true, buffer: true`, each printing three lines; `otto say` prints alpha's three lines contiguously, then beta's, then gamma's, in item order. Asserted as a property (no line of one item appears between two lines of another, and blocks appear in item order), not as a fixed transcript.
  `Observed on main:` the property fails on every run: output is fully interleaved and block order is nondeterministic. One sample run gave `[say:gamma] gamma line 1` / `[say:beta] beta line 1` / `[say:alpha] alpha line 1` / `[say:gamma] gamma line 2`; the review panel's independent re-run gave beta/alpha/gamma. The transcript varies; the property failure does not.
- [ ] **Computed envs:** fixture `otto: {envs-command: "printf 'FOO=bar\nBAZ=qux\n'"}`; a task body prints `FOO=[bar] BAZ=[qux]`.
  `Observed on main:` ``otto: unknown field `envs-command`, expected one of `name`, `about`, `api`, `jobs`, `tasks`, `envs`, `retention` at line 3 column 3``, exit 1
- [ ] **Envs precedence and laziness:** with both `envs-command` and a literal `envs: {FOO: explicit}`, the task sees `FOO=[explicit]`; and a marker file the command would create is absent after `otto --help`, present after a real run.
  `Observed on main:` precedence half cannot run (the key does not load). Laziness half already holds for literal `envs:`: marker absent after `--help` both with and without a command-source foreach in the file; absent after `--tasks` and `--list-subtasks` without one; PRESENT after both of those with one; present after `otto show`.
- [ ] **Required params, on the case that matters:** fixture param with `required: true`; BARE `otto sw` (task named, zero arguments) exits non-zero with a usage error naming `svc`, and `otto sw alpha` binds it.
  `Observed on main:` two separate failures. The key does not load: ``tasks.sw.params.svc: unknown field `required`, expected one of `metavar`, `default`, `choices`, `choices-command`, `nargs`, `help` at line 7 column 9``, exit 1. And clap does not even run for a bare named task: with a `choices: [alpha, beta]` param and no `required` key, `otto sw` gives `[sw] svc=[UNSET]`, exit 0, while `otto sw nope` gives `error: invalid value 'nope' for '[svc]'  [possible values: alpha, beta]`. The gate is `src/cli/parser/discovery.rs:235-237`. On the real consumer the shape is worse than the synthetic fixture shows: bare `otto switch` on the otto-dev tree at `659c0ef` EXECUTES the task and is stopped only by bash, `script.sh: line 36: svc: usage: otto switch <service> <branch>`, exit 1, after otto has created a run directory, resolved the global env map, and written the task script. The preflight replaces all of that with an error raised before any of it happens, which is also why its placement before Step 0 of `process_tasks_with_filter` is load-bearing rather than cosmetic.
- [ ] **Command sources share one cwd:** `cd / && otto -o <otto-dev>/.otto.yml profiles` resolves the relative `$(scripts/svc.sh root ...)` envs instead of failing.
  `Observed on main:` `Failed to evaluate global environment variables: Failed to resolve environment variable 'AUTH_ROOT': Command 'scripts/svc.sh root auth' failed with exit code 127: sh: 1: scripts/svc.sh: not found` (run from `/home/saidler` against the `659c0ef` extract).
- [ ] **Additive, not breaking:** `otto ci` green, and the merged `tatari-tv/otto-dev` `.otto.yml` at `659c0ef` still loads with exactly 21 tasks.
  `Observed on main:` 21, against a clean extract of the `659c0ef` tarball: `otto --tasks --format json | python3 -c "import json,sys; print(len(json.load(sys.stdin)))"` -> `21`, task names `auth-bypass auth-local auth-staging convert-check data doctor down init logs nuke preflight profiles pull seed start start-docker status stop switch up use`. Exact count is safe here: this doc changes no otto-dev file, so the number must not move.

## Resolved Decisions

- 2026-08-31: **Blocks emit in foreach item order, not completion order.** Item order is what `stack.sh` implements and the stated reason it exists. Cost is display-side head-of-line blocking only; execution stays fully concurrent.
- 2026-08-31: **Per-line prefixes are preserved inside a buffered block.** Contiguity was the missing property, not attribution. `--no-prefix` remains the single way to drop prefixes, in both modes.
- 2026-08-31: **No merged capture; replay `stdout.log` then `stderr.log`.** A combined `output.log` was designed first and cut on evidence: the two streams are drained by independent tokio tasks (`src/executor/scheduler/task_execution.rs:252-273`), so otto's arrival order is already racy and a merged file would promise an ordering it cannot keep. It also costs otto-dev nothing, which runs the inner otto with `2>&1`. This deletes the file, the shared writer, and the spike phase that existed to de-risk it.
- 2026-08-31: **Literal `envs:` wins over `envs-command` output.** The more specific declaration wins, and it gives a consumer a one-line override without editing the command.
- 2026-08-31: **`envs-command` empty output is legal**, unlike `choices-command` where empty is a loud error. An env set can legitimately be empty (nothing cloned); an empty validation set is always a misconfiguration.
- 2026-08-31: **`envs-command` laziness is "never for `--help`", not "never for enumeration".** Measured: `--tasks` and `--list-subtasks` already resolve `otto.envs` when a command-source foreach is present, because item resolution is handed the env map. Stating the weaker guarantee is the truthful one.
- 2026-08-31: **The replay cursor is driven from all four terminal-transition sites, not from the report channel.** Only the success and failure arms report; `mark_skipped` and the up-to-date skip mutate the sets inline and send nothing. A channel-only cursor would stall behind any skipped item. The cursor is a local on the `execute_all` frame, so no shared-state locking is introduced.
- 2026-08-31: **A buffered subtask's scheduler status line travels with its block**, not with its completion. Otherwise status lines arrive in completion order while blocks arrive in item order, which is a worse read than today.
- 2026-08-31: **Replay takes a process-wide output lock for one whole block.** `TeeWriter::write` has no shared lock today, so a concurrently running unbuffered task would split a block: the exact defect being fixed. Live writers take the same lock per line.
- 2026-08-31 (superseded same day, panel round 2): ~~the clap bind gate widens for tasks that declare a required param~~ -> **a preflight ahead of the gate; the gate is untouched.** Verified chain: the widened gate reaches `BuildMode::Bind` (`discovery.rs:242`) -> `param_choices` (`command.rs:112`) -> `resolve_choices_command` (`command.rs:147-157`) plus `global_envs()`, so bare `otto switch` would run `scripts/svc.sh exposing switch` and resolve the env map just to print a missing-argument error, on the exact task the feature exists for. An error path must not have side effects. The preflight answers from the spec alone: zero subprocesses, every other path byte-identical.
- 2026-08-31 (superseded same day, panel round 4): ~~`TaskReport` gains `output_complete: bool`~~ -> **`drain: Vec<DrainIssue>`**, see the round-4 entry below. The round-2 finding was that the truncation marker had no carrier at all; naming a bool closed that, and round 4 showed the bool cannot carry what the marker promises.
- 2026-08-31 (panel round 3): **`--tasks` emits `required` only when true.** Emitting `required: false` for every plain param would change `--tasks` output for every existing ottofile with params that sets none of the new keys, which contradicts this doc's own additivity goal. `ParamView` already uses `skip_serializing_if` for `choices` and `choices-command`; `required` follows it.
- 2026-08-31 (panel round 4): **Six cancellation states, not three.** `ActiveTasks` tracks spawned bodies, not child or log state (`scheduler.rs:166-230`), so a body whose child never launched has no log to point at, and a body that finished and sent an unread report has a COMPLETE block that must still be printed. Collapsing them would print paths that do not exist or discard finished output.
- 2026-08-31 (panel round 4): **`TaskReport` carries `drain: Vec<DrainIssue>`, not `output_complete: bool`.** The drain path distinguishes six outcomes (three conditions per stream); a bool cannot name the condition the marker promises to name.
- 2026-08-31 (panel round 4): **The preflight sits before Step 0 of `process_tasks_with_filter`, not at the clap gate.** Globals resolve at `discovery.rs:171` and task envs at `:225`, both before the gate, so a gate-adjacent preflight would still have run `envs-command`. The claim was false as originally placed.
- 2026-08-31 (panel round 3): **The cancellation flush is ordered but never stops early.** The panel posed it as a choice between strictly-ordered-and-stop and flush-everything-regardless; neither is right. Ordered preserves the reading, never-stopping preserves the output. Cancellation returns at `scheduler.rs:944-945` and `:1036-1037`, before the post-loop reconciliation at `:1253-1256`, so neither terminal-transition funnel runs on this path.
- 2026-08-31 (panel round 3): **The side-effect-free property is scoped to the zero-argument case, in the doc's own words.** `otto sw --verbose` still resolves `choices-command` and `global_envs()` before clap reports the missing value. That is today's behavior for any task invoked with arguments; this doc neither creates nor removes it, and says so rather than implying a broader guarantee.
- 2026-08-31 (panel round 2): **Phase 2 ships as two commits**, the `base_dir()` cwd fix first, `envs-command` second, so the disclosed behavior change is reviewable and revertable on its own.
- 2026-08-31 (blast radius corrected panel round 5): **`global_envs()` moves from process cwd to `base_dir()`.** `envs:` `$(...)` resolved relative paths against the caller's directory while every other command source used the ottofile's; the defect fires on any invocation whose cwd is not the ottofile's directory, including plain subdirectory invocation with no flags at all. `envs-command` COULD be specified coherently without the fix (documented as another `base_dir()` source, tolerating the sibling disagreement), so the fix is not forced by Phase 2; it is taken because the round-5 survey measured zero consumers that break and seven otto-dev substitutions it repairs, and one cwd contract for all four command sources is the simpler spec.
- 2026-08-31: **`envs-command` reuses the execution contract of the other command sources, not their cache cell.** `DynamicResolver`'s caches sit downstream of `global_envs`; re-entering its `OnceCell` from inside its own initializer panics.
- 2026-08-31: **Command output is data.** No `$(...)`, no `${VAR}`, no unquoting applied to `envs-command` values. Follows the v2.0.0 injection-containment rule.
- 2026-08-31: **`required` + `default:` is a load error**, not a silent precedence rule. Two keys encoding opposite intent is the naming-tells-the-truth case.
- 2026-08-31 (superseded same day, panel round 1): ~~`serial_index` is generalized to `foreach_index` plus `serial: bool`, 63 refs across 13 files~~ -> **a `parent -> ordered-subtask-names` map, built at expansion and read only by the replay cursor.** The panel named the flaw (rewriting the dependency scheduler's state for a display-ordering index is an architectural boundary violation, and it is scope Scott did not ask for) and confirmed the 63/13 counts. `serial_group` / `serial_index` are untouched. The redundancy this accepts is small and honest: `serial_index` orders execution, the map orders display.
- 2026-08-31 (panel round 1): **`envs-command` output LAYERS onto the inherited environment; it is not merged into the declared map.** Merging discards the computed value, so a shadowing literal key that self-references (`FOO: '$(echo "${FOO:-x}")'`) would resolve to the OS value instead of the computed one. Layering makes `evaluation_context`'s inherited-value seeding (`src/cfg/env.rs:130-131`) do the right thing, and literal `envs:` still wins the final value.
- 2026-08-31 (panel round 1): **Replay streams with `std::fs` + `BufReader`, NOT `TaskStreams::read_output`.** That function is `async fn -> Result<Vec<String>>` on `tokio::fs` and collects the whole log into memory: async where replay must be blocking, unbounded where it must be bounded. Naming it as the reader contradicted the `spawn_blocking` bullet and the Performance claim in the same doc.
- 2026-08-31 (panel round 1): **The interrupt drain has a named mechanism or it does not exist.** `abandon_run` takes `&mut cursor` and flushes terminal subtasks' blocks before returning `Err`; killed in-flight subtasks get their log paths printed, never a partial replay.
- 2026-08-31 (panel round 1): **All seven terminal-writing sites take the output lock**, not just `TeeWriter`. The five others (success status, failure message, two skip messages, run-cancelled) fire constantly during a parallel group and would have split blocks.
- 2026-08-31 (panel round 1): **A block whose drain failed or timed out ends with a loud marker.** `task_execution.rs:282-310` only `error!`-logs a drain failure and falls through to success at `:312`, so terminal state does not imply complete output. Replay degrades visibly rather than printing a short block under a success line.
- 2026-08-31 (panel round 1): **Replay runs in `tokio::task::spawn_blocking`.** Holding an output lock while reading a log inline in the `execute_all` ready loop would stall a tokio worker and starve concurrent tasks, and the block cannot be assembled with `await` points inside the lock. One blocking closure takes the lock, does `std::fs` reads and locked stdout writes, and returns.
- 2026-08-31 (panel round 1): **`required` is a CLI-surface constraint, not a data constraint.** `propagate_params` does not check it, matching how `choices` already behaves for propagated values. Validating propagated values is a separate feature nobody requested.

## Alternatives Considered

### A merged `output.log` written through one shared writer
- **Description:** buffered subtasks tee into a third per-task file so stdout and stderr land in arrival order, replayed as one stream.
- **Pros:** a single block reads the way a serial run would.
- **Cons:** the two streams are drained by independent tokio tasks (`src/executor/scheduler/task_execution.rs:252-273`), so arrival order at otto is already racy; the file would promise an ordering nothing can keep. It also adds a shared writer on the hot path and a spike phase to de-risk it.
- **Why not chosen:** it buys no guarantee that does not already exist, and buys nothing at all for the consumer that asked, which runs the inner otto with `2>&1`.

### One OS pipe as both child stdout and stderr
- **Description:** pass a single pipe to the child for both streams; one reader, natural `2>&1` ordering.
- **Pros:** genuine emission order, unlike the merged-file option above.
- **Cons:** `stderr.log` would be empty for buffered subtasks, breaking the run-dir contract for one mode.
- **Why not chosen:** the contract break is not worth an ordering guarantee no consumer has asked for. Revisit only if someone needs true interleaved capture.

### Widening the clap bind gate, either unconditionally or for tasks declaring `required` (BOTH REJECTED)
- **Description:** change `discovery.rs:235-237` from `args.len() > 1` to `>= 1`, or to `> 1 || task_has_required_param(...)`, so a bare `otto <task>` reaches clap and clap reports the missing value.
- **Pros:** clap owns the message; no new predicate on the error path.
- **Cons:** the unconditional form pushes every ottofile through a path it does not take today, changing when `default:` values are applied. The scoped form avoids that but still enters `BuildMode::Bind`, which resolves `choices-command` and `global_envs()` (`command.rs:112,147-157`): bare `otto switch` would run a registry subprocess and the whole env map to print a usage error.
- **Why not chosen:** an error path must not have side effects, and the side effect lands on precisely the task this feature was requested for. The preflight gets the same error from the spec with zero subprocesses. Recorded so neither form is proposed again.

### Buffer in memory instead of on disk
- **Description:** accumulate each subtask's lines in a `VecDeque`, like the TUI pane does.
- **Pros:** no new file; the TUI precedent already exists.
- **Cons:** a `logs` subtask is unbounded; the TUI caps its buffer and drops lines, which is right for a live view and wrong for a replay that claims to be the whole output.
- **Why not chosen:** silently truncating output is the silent-success class v2.0.0 spent 88 commits removing.

### A per-task `buffer:` key instead of `foreach.buffer:`
- **Description:** put `buffer` at the task level.
- **Pros:** one key covers more shapes.
- **Cons:** a non-foreach task's output is already contiguous, so the key would do nothing almost everywhere; and `parallel:` living one level up from `foreach` is the exact bug the strict-schema doc was written to catch.
- **Why not chosen:** the property is about a GROUP of subtasks, so it belongs on the group.

### `envs-command` emitting JSON or YAML instead of `KEY=VALUE`
- **Description:** parse structured output into the env map.
- **Pros:** unambiguous quoting; nested types possible.
- **Cons:** env values are strings; a structured format invites values that cannot be represented, and every producer would need a serializer where `printf` suffices.
- **Why not chosen:** `KEY=VALUE` is what `env`, `docker`, and every shell already emit.

### Generalizing `serial_index` to `foreach_index` plus a `serial: bool` (REJECTED after review)
- **Description:** rename `serial_group`/`serial_index` and populate them for every expansion, so one field pair carries subtask position.
- **Pros:** one truth for subtask order; removes a field documented as "Meaningless when `serial_group` is `None`".
- **Cons:** 63 references across 13 files (count confirmed by the panel), all inside the dependency scheduler's execution path, in service of a display feature. Unrequested scope.
- **Why not chosen:** the panel named it an architectural boundary violation and it was not asked for. Reversed in favor of the display-order map. Recorded here so it is not proposed again without a new reason.

### Task-scoped `envs-command`
- **Description:** allow the key under `tasks.<name>:` as well.
- **Pros:** per-task computed environments.
- **Cons:** unrequested; multiplies resolution points and cache keys.
- **Why not chosen:** parked with a revisit condition (see Non-Goals).

## Technical Considerations

### Dependencies
- No new crates. tokio, clap, serde as already used.

### Performance
- Buffered mode preserves concurrency; only display is serialized. Replay streams with a fixed-size `BufReader`, so peak memory is the read buffer and not the output size. This is why `TaskStreams::read_output` (whole log into a `Vec<String>`) is not the replay path.
- `envs-command` adds at most one subprocess per invocation, already the cost profile of `foreach.command` and `choices-command`, and skipped entirely on `--help`.
- `required` is a clap builder call: zero runtime cost.

### Security
- `envs-command` output is never re-evaluated, so a value containing `$(rm -rf ...)` is an inert string. This is the same containment the four script generators got in v2.0.0.
- The command itself runs with the ottofile's directory as cwd, from the ottofile, which is already a trusted-input surface (`bash:` bodies run arbitrary code).
- No new file is written, so retention, pruning, and permissions are unchanged.

### Testing Strategy
- New integration tests beside the existing ones: `tests/foreach_buffer_test.rs`, `tests/envs_command_test.rs`, `tests/required_param_test.rs`, matching the naming of `tests/foreach_command_test.rs`, `tests/env_self_reference_test.rs`, `tests/dynamic_choices_test.rs`.
- Negative cases are mandatory, not optional: interleaving asserted absent (not merely "blocks present"), `buffer` + `tty` rejected, invalid `KEY=VALUE` rejected by line number, `required` + `default` rejected, required-omitted exits non-zero.
- Break-the-code checks, so the tests bite: reverting the replay cursor to completion order must fail `foreach_buffer_test`; removing the lock from `scheduler.rs:1112` alone must fail criterion (d); short-circuiting the drain-completion check must fail criterion (g); reverting the clap gate to `args.len() > 1` must fail the bare-`otto sw` case in `required_param_test`; reverting `global_envs()` to `self.cwd` must fail the `-o`-from-elsewhere case in `envs_command_test`.
- `tests/serial_foreach_test.rs`, `tests/flag_integration_test.rs`, and `tests/roundtrip.rs` must pass completely untouched; Phase 3 is additive and asserts `rg 'serial_group|serial_index' src/ tests/ | wc -l` still returns 63 (`rg -c` over two paths prints per-file counts, never the total). Any change to those files is a disclosed test change with its reason in the phase.

### Rollout Plan
- One phase, one commit, `otto ci` green at each, with one exception stated in the phase itself: Phase 2 splits into two commits, the `base_dir()` cwd fix first and `envs-command` second, so the disclosed behavior change is reviewable and revertable on its own. Minor version bump after Phase 5.
- otto-dev adopts separately and its own PR carries the deletions.

## Risks and Mitigations

| Risk | Likelihood | Impact | Mitigation |
|---|---|---|---|
| The preflight changes behavior for a task that has no required param | Low | High | The gate is untouched; the preflight body is unreachable without a declared required param. Phase 1 criterion (e) diffs captured stdout+stderr+exit before and after |
| A missing-required error path runs `choices-command` and `envs-command` subprocesses | **High if built as a widened gate** | Med | Preflight answers from the spec alone; Phase 1 criterion (f) asserts a `choices-command` marker file stays absent on the bare-invocation error path |
| The `base_dir()` cwd change breaks an ottofile that relied on process cwd for an `envs:` substitution | Low | Med | Disclosed behavior change in its own commit; round-5 survey of 138 substitutions across 125 ottofiles found zero that depend on process cwd (the 8 cwd-sensitive ones are repaired, 7 in otto-dev itself) |
| Replay cursor stalls on a skipped subtask | **High if built naively** | High | Both skip paths bypass the report channel entirely (`mark_skipped`, up-to-date skip). The cursor is driven from all four terminal sites, and the parent's `When::Always` edges guarantee an end-of-group flush. Phase 4 criterion (e) pins the skipped-first-item case |
| Blocking replay stalls the tokio runtime | Med | High | Replay is confined to `spawn_blocking`; the scheduler only awaits the join handle |
| A buffered block is split by a status, skip, or failure line | **High if built naively** | High | All seven terminal-writing sites take the lock; Phase 4 criterion (d) runs a skip/status emitter alongside the group |
| A subtask reports success with a truncated log, and replay prints it under a success line | Med | High | Drain completion is recorded and a failed drain ends the block with a loud marker; Phase 4 criterion (g) forces the timeout. The underlying silent-success in `task_execution.rs:282-312` is a shipped defect wider than this feature and is left for its own targeted fix |
| Buffered blocks are lost on SIGINT | Med | Med | `abandon_run` flushes terminal blocks and prints paths for killed ones; Phase 4 criterion (f) |
| A shadowing literal `envs:` key loses the computed value it self-references | Med | Med | Layering rather than merging; Phase 2 criterion (f) pins `FOO=[computed]` |
| `envs-command` output shadows something a task depends on | Low | Med | Literal `envs:` wins; precedence pinned by test |
| `required` breaks an existing ottofile | Low | Low | Additive with `#[serde(default)] false`; unset means today's behavior |
| Audit batches 8-14 land on code this doc moves | Med | Med | Ship order stated above: finish the batches, or re-run 9 and 11 after |
| The new output lock serializes live task output and slows a wide parallel run | Med | Med | Lock is per line for live writers, exactly the granularity `print!` already takes internally; Phase 4 criterion (d) pins correctness, and the barrier fixture pins that concurrency survives |
| A required positional declared after an optional one panics clap at build time | Med | High | Load-time guard in Phase 1, with criterion (d) asserting no panic on all four rejected combinations |

## Open Questions

- None.

## References

- otto-dev PR #4: https://github.com/tatari-tv/otto-dev/pull/4
- otto-dev merged tree: https://github.com/tatari-tv/otto-dev/tree/659c0efd9f0d06012fbc40ae30ccd5c6da2d3fc0
- Review that produced these three items: https://marquee.internal.tatari.dev/p/~scott-idler/otto-dev-on-v2-0-5-what-you-took-and-what-is-still-on-the-bone/
- Predecessor doc: `docs/design/2026-08-28-boundary-fixes-and-dynamic-foreach.md`
- Schema inventory to update: `docs/commands/ottofile-reference.md`
- In-flight audit: `docs/design/2026-08-30-audit-batch-handoff.md`
