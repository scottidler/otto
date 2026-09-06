# Design Document: Shakedown Remediation

**Author:** Scott A. Idler
**Date:** 2026-09-06
**Status:** Implemented
**Review Passes Completed:** 5/5 (draft, correctness, clarity, edge cases, excellence), plus review-panel rounds 1 through 5 (Architect + Staff Engineer), closed at zero must-fix, every finding dispositioned in Resolved Decisions
**Verified against:** HEAD `96951d5`, Cargo.toml `2.3.0`, tag `v2.3.0`
**Modules touched:** cli/commands (clean, stats, upgrade), cli/parser (help, command), executor (state/manager, pruning, workspace), docs, tests

## Summary

`/cli-shakedown` on the tagged v2.3.0 binary found seven defects. Confirming
the worst of them surfaced two more that the shakedown could not reach through
the CLI. This doc turns all nine into a phased
plan, plus one Observation the shakedown recorded and did not number.

Nothing here is a new feature. Every phase either makes an existing promise true
(a doc claim, a `Usage:` line, a JSON key, a percentage, a synchronization
guarantee) or removes the promise.

## Finding to phase map

| Finding | What it is | Phase |
|---|---|---|
| F1 | `after:`/`before:` documented backwards | 1 |
| F2 | scar-tissue note printed as `--github-token` help | 2 |
| F3 | `Usage:` lines omit the `otto` prefix | 3 |
| F5 | two Success Rate denominators | 4 |
| F4 | `Stats --json` drops the per-task table | 5 |
| F7 | `--format` without `--tasks` runs the default tasks | 6 |
| F9 | two disagreeing directory-size implementations | 7 |
| F6 + F8 | `Clean` cannot see orphaned directories; the DB path creates more | 8 |
| Observation | run record says `args: ["otto"]` for every run | 9 |

Ten phases, 0 through 9. F4 lands after F5 because Phase 4 extracts the shared
percentage helper that Phase 5's regenerated `stats.md` output depends on.

## How to read this document

- **Implementing a phase?** Go straight to it. Every bullet carries a `file:line`
  anchor against `96951d5` and, where measured, an `Observed` line that is the
  failing assertion you write first.
- **Anchors expire.** Line numbers point into `96951d5`. Re-anchor before editing.
- **Bare filenames are given with their directory.** Three wrong anchors reached
  this doc during review, all three from a bare filename in a review report:
  `task_execution.rs` is `src/executor/scheduler/task_execution.rs` and
  `retention.rs` is `src/executor/state/retention.rs`. Where a name is ambiguous
  the directory is given, the same convention the 2026-09-02 remediation adopted
  after `config.rs` turned up under both `cfg/` and `cli/parser/`.
- **Cheap and mechanical first.** Phases 1-3 are docs and help strings. Phases
  4-7 are display and wiring. Phase 8 is the load-bearing one: it is the only
  phase that changes what gets deleted.
- **Model tags** pick who runs the phase: `opus` where the phase changes
  semantics or touches deletion, `sonnet` where it is mechanical.

## Problem Statement

### Background

v2.3.0 shipped, was tagged, and its release binary was validated (all four target
platforms, checksums verified, `otto v2.3.0` from the downloaded artifact). The
shakedown ran 41 invocations against that binary. `otto ci` is green: 966 unit
tests plus the integration suites, coverage 94.4% against an 87% floor.

The tool works. What it says about itself does not always match what it does.

### Problem

The nine defects fall into three classes.

**1. The docs lie about the DAG.** `docs/commands/ottofile-reference.md:91-92`
documents `after:` as "Tasks this one runs after" and `before:` as "Tasks this
one runs before". The scheduler does the opposite: `src/cli/parser/params.rs:292-305`
carries the comment "`after` on task X means: every task in X's `after` list
depends on X", and that is what runs. Measured with wall-clock timestamps, task
`later` declaring `after: [first]`, invoked as `otto first`:

```
later      1788676252.533636807
first      1788676252.578740988
first-done 1788676253.590615515
```

`later` ran 45ms before `first`, while `first` was still sleeping. The mirror
test: `earlier` declaring `before: [main-task]`, invoked as `otto main-task`, ran
`main-task` alone and never scheduled `earlier`.

This is not new. `docs/design/2026-09-02-second-code-review-remediation-implementation-notes.md:182`
found it four days ago and left it: "the fix is either a one-line doc correction
or a scheduler change, so it needs the author's call." **See Open Questions.**

`docs/commands/tasks.md:69-72` ships a worked example that is wrong under the real
semantics: `down: {help: "Stop each service", after: [up]}` makes `down` run
*before* `up`. Anyone copying it gets an inverted pipeline.

**2. Output that disagrees with itself.**

- `otto Stats` prints two tables; `otto Stats --json` returns only the first, and
  `-n/--limit` is structurally unreachable on the JSON path (`src/cli/commands/stats.rs:56-60`
  returns before the per-task fetch at `:109`). A script cannot reproduce the
  table it sees.
- "Success Rate" is computed two ways in one screen: `stats.rs:63-67` uses
  `successful / total`, the other three sites (`:130-135`, `:177-182`, `:276-281`)
  use `successful / (successful + failed)`. A healthy install reads as 41.8%.
- Every builtin and every task prints `Usage: Clean [OPTIONS]` with no `otto`
  prefix. Top-level gets it right. The usage line does not run when pasted.
- `otto Upgrade --help` renders an internal scar-tissue note as the user-facing
  long help for `--github-token` (`src/cli/commands/upgrade.rs:334-338` are `///`
  and should be `//`).
- `otto --format yaml` with no `--tasks` ignores the flag and silently runs the
  ottofile's default tasks. In this repo that is `ci`, a three-minute build. The
  shakedown hit it by accident and ended up with two CI runs racing over one
  `target/llvm-cov-target`.

**3. `Clean` cannot see most of what it claims to manage.** This is the one worth
reading twice, because the shakedown's first root cause was wrong and the
correction changes the fix.

Same criteria, opposite answers:

```
$ otto Clean --dry-run --keep-days 30
No runs matching deletion criteria found

$ otto Clean --dry-run --keep-days 30 --no-db
Found 1993 runs to delete by keeping everything for 30 days (173.4 MB total)
```

Measured on the live `~/.otto`:

| Fact | Value |
|---|---|
| `runs` rows | 1002 |
| oldest row timestamp | `1786205273` = 2026-08-08, inside the 30-day window |
| rows with `run_dir IS NULL` | 388 |
| rows with `status='running'` | 414 |
| run directories on disk | 5090 |
| directories older than the oldest row | **1993**, the exact count `--no-db` reports |
| `DELETE FROM projects` / `delete_project` sites in the tree | **zero** |

Nothing in otto can delete a project row, so the orphans' projects were never in
this database file: the DB they belonged to is gone. The rows were not pruned out
from under their directories.

> **The filesystem is the only complete record of run directories. The database is
> a partial, resettable index.** `execute_with_database` (`src/cli/commands/clean.rs:158-166`)
> enumerates rows, so a directory with no row is permanently invisible to it, and
> nothing reconciles the two. `auto_prune` hardcodes `no_db: false`
> (`src/executor/pruning.rs:80-88`), so the automatic path is DB-only forever.

`docs/commands/clean.md:192` ("Database and filesystem stay synchronized") and
`clean.md:221` ("**Atomic Deletion**: Both database and filesystem cleaned
together") are false as written.

Scale, so the number is not overstated: the 1993 orphans are **375 MB** (`du`) of
an 8.4 GB `~/.otto`. The rest is recent runs inside the retention window that
neither mode would delete. The defect is that 375 MB is unreachable by the default
path, not that 8.4 GB is leaking.

Two numbers appear for that same set, and they are reconciled here rather than left
looking like a contradiction: otto reports **173.4 MB**, `du` reports **375 MB**.
Measured: `du --apparent-size` over the identical 1993 directories returns
**174 MB**. The gap is 4 KB block rounding over many tiny files, not a
size-computation defect, so it is **not** an instance of F9. Note for the
implementer: Phase 7 changes symlink handling, which moves AC1's recorded
`173.4 MB` baseline, which is why AC1 asserts equal directory *sets* rather than
equal sizes.

Confirming that turned up two more defects, neither reachable from the CLI:

- **The DB path will orphan directories going forward.** `resolve_run_directory`
  (`src/executor/state/manager.rs:847-878`) reconstructs a missing `run_dir` as
  `run_root(...).join(run.timestamp.to_string())` at `:855-858`. That never
  matches a same-second run, whose directory is `<timestamp>-<seq>`
  (`src/executor/layout.rs:96-98`); 39 such directories exist today. It also
  re-derives `project_name` from the *current* ottofile parent directory, so
  moving or renaming an ottofile breaks every old NULL-`run_dir` row of that
  project. On a miss the function `log::warn!`s and returns `Ok(None)` (`:860-868`),
  and `delete_run` commits the row deletion with no directory to remove: row
  gone, directory orphaned. **388 of 1002 live rows have `run_dir IS NULL`**, so
  this is armed now. The warning goes to the log file, so nothing reaches the
  terminal.
- **Two implementations of "size of a directory" that disagree.**
  `CleanCommand::calculate_dir_size` (`clean.rs:541-567`) skips symlinks;
  `Workspace::calculate_directory_size` (`workspace.rs:436-450`) follows them
  (both `is_dir()` and `metadata()`). Every one of the 24217 symlinks under
  `~/.otto` is a `script.sh` or `input.<dep>.json` pointing back into `.cache/`,
  so following them counts shared blobs once per run that references them.

### Goals

- Every shakedown finding has a phase, a bullet, and a success criterion, or a
  recorded reason it is not here (Addendum).
- Every reproduced defect gets a regression test that fails on `96951d5`.
- After Phase 9: no doc page describes an ordering the scheduler does not
  implement; no `Usage:` line is unpasteable; no two code paths compute the same
  quantity two ways; `Clean`'s default mode can reach every run directory under
  `~/.otto`.
- Acceptance criteria that were run against `main` before this doc was called
  ready, with their output recorded.

### Non-Goals

- **New features.** No new commands and no new flags. An earlier draft of this
  doc proposed `Clean --orphans-only`; it is cut. Nothing in the shakedown asked
  for it, the default path sweeping orphans is what fixes F6, and a convenience
  flag on top of that is scope nobody requested. Recorded in the Addendum.
- **Renaming `before:` / `after:`.** The names are confusing and the confusion is
  what produced this finding, but renaming them breaks every ottofile in the
  fleet. Parked, revisit condition: the next time the schema is opened for a
  breaking change.
- **Reconciling the 414 stuck `Running` rows in Scott's live `~/.otto`.** That is
  a one-time operator action against data, not phase work. A run killed by
  SIGKILL legitimately leaves a `Running` row; Phase 4 removes their distorting
  effect on the reported rate rather than pretending they can be eliminated.
- **Reclaiming Scott's live 1993 orphans.** Also an operator action. Phase 8
  makes the mechanism exist; running it is his call.
- **`otto help <NAME>` vs `otto <NAME> --help` rendering from two different
  sources.** Real (TaskSpec-derived vs the clap derive, so `--keep-days <keep-days>`
  vs `--keep-days <KEEP_DAYS>`), found during this dig, and NOT a shakedown
  finding. Unrequested scope. Recorded in the Addendum.
- **Concurrent otto runs in one repo corrupting each other's cargo target
  directory.** The F7 incident produced two observations; Phase 6 takes the
  `--format`/`--tasks` half. This half is real (the shakedown's two `otto ci` runs
  clobbered a test binary under one `target/llvm-cov-target`) but it is cargo's
  target-directory locking, not otto's: otto ran both invocations correctly. Parked,
  revisit condition: it bites a run that was not self-inflicted.
- **Release-artifact signature verification.** Parked 2026-06-10, re-parked
  2026-09-02. Untouched.

## Proposed Solution

### Overview

Nine defects, ten phases, one commit per phase. Phases 1-3 change only strings.
Phases 4-7 change display and one clap attribute. Phase 8 is the only phase that
changes what gets deleted, and it depends on Phase 7's single size function.

### Architecture

The only structural change is in `Clean`. Today:

```
Clean (default)  -> rows -> delete row + (maybe) its directory
Clean --no-db    -> directories -> delete directory
auto_prune       -> rows only, forever
```

After Phase 8:

```
Clean (default)  -> rows -> delete row + its directory
                 -> then: directories with no row -> apply the same Retention -> delete
Clean --no-db    -> directories -> delete directory        (unchanged)
auto_prune       -> both passes                            (via the default path)
```

`Retention::expired` (`clean.rs:322-335`) is already a shared pure function used
by the filesystem path. The orphan sweep reuses it, so the two modes cannot drift
on policy.

### Data Model

No schema change, and **`OverallStats` is not modified**. It is a port-level type
(declared `src/executor/state/manager.rs:84`, constructed a second time at
`src/ports/db.rs:338`), so a new
field would force every store implementation to populate it. Phase 5 instead adds a
serialize-only view struct in `stats.rs` that `#[serde(flatten)]`s the `OverallStats`
it already has and appends `tasks: Vec<TaskStats>`. The seven existing keys keep
their names, order, and number formatting, so any `jq '.total_runs'` in the wild
keeps working. The seven pairs serialize byte-for-byte identically and in the same
order, followed by the new `tasks` key; the object as a whole is not byte-identical
to today's, and is not a prefix of it either, since today's output ends in `}`.

### API Design

| Surface | Before | After |
|---|---|---|
| `otto Stats --json` | 7-key flat object | same 7 keys plus `tasks: [...]`, honoring `--limit` |
| `otto Clean --help` | `Usage: Clean [OPTIONS]` | `Usage: otto Clean [OPTIONS]` |
| `otto lint --help` | `Usage: lint` | `Usage: otto lint` |
| `otto --format yaml` (no `--tasks`) | runs default tasks | usage error, exit 1 |
| `otto Clean` (default) | rows only | rows plus orphaned directories |

### Implementation Plan

#### Phase 0: Measure the orphan scan before designing around it
**Model:** sonnet
- Zero code. Time `otto Clean --dry-run --no-db` against the live `~/.otto`
  (`calculate_dir_size` walks every file under each selected run). The doc drafted
  this against 5090 directories; the tree measured 7072 on the day, which only
  strengthens the result below.
- Record wall-clock time and whether the scan is bounded by inode count or by
  size summation.
- If the scan exceeds a few seconds, Phase 8's sweep computes sizes only for the
  directories it actually selects, and `--dry-run` output says how many it
  scanned.
- **Measured 2026-09-06, and the conditional did not fire.** 1.85s over 7072 run
  directories / 102708 files, 1993 selected. Under "a few seconds", so Phase 8
  carries neither the size-only-for-selected optimization nor a scanned count in
  `--dry-run`. The cost is size summation, not directory traversal: pure traversal
  0.46s, `du -s` 1.10s, full scan 1.85s.
- **Success criteria:** a recorded wall-clock number for a full `--no-db` dry run
  on the live tree, in the phase's implementation notes.

#### Phase 1: Correct the before/after documentation
**Model:** sonnet
- **Ruled 2026-09-06: correct the docs.** See Resolved Decisions.
- `docs/commands/ottofile-reference.md:91` -> `after` documents "Tasks that run
  after this one (this task becomes their dependency)".
- `docs/commands/ottofile-reference.md:92` -> `before` documents "Tasks that run
  before this one (they become this task's dependencies)".
- `docs/commands/ottofile-reference.md:101` -> the `on-failure` mechanism clause
  says the synthetic edge is pushed onto **this** task's `after:` list pointing at
  the named tasks, not "on the named tasks". Verified at
  `src/cli/parser/params.rs:250-273`: `host` is the declaring task, `target` is the
  named one, and the edge lands on `host_spec.after`.
- `src/cfg/task.rs:696-698` -> same ambiguity in rustdoc, same correction.
- `docs/commands/tasks.md:69-72` -> the `down: after: [up]` example is inverted.
  Replace with an example that is correct under the real semantics and says which
  runs first.
- Add a worked two-task ordering example to `ottofile-reference.md` showing the
  invocation and the resulting order, so the next reader does not have to run the
  timestamp experiment.
- **Success criteria:** `grep -rn 'Tasks this one runs' docs/commands/` returns
  zero lines; the four rustdoc sites at `discovery.rs:115-118`, `params.rs:231-239`,
  `params.rs:293-295`, `foreach.rs:28` are unchanged (they were already correct).

#### Phase 2: Stop printing the scar tissue
**Model:** sonnet
- `src/cli/commands/upgrade.rs:334-338`: change the `///` continuation block to
  `//`. Line 333 stays `///`.
- **Success criteria:** `otto Upgrade --help | grep -c hide_env_values` returns 0;
  `otto Upgrade --help | grep -c 'GITHUB_TOKEN'` still returns 1 (the env marker
  is still shown, still without a value).

#### Phase 3: Make every `Usage:` line pasteable
**Model:** sonnet
- Six builtin structs get `bin_name` alongside the existing `#[command(name = ...)]`:
  `clean.rs:58`, `convert.rs:10`, `graph.rs:42`, `history.rs:80`, `stats.rs:12`,
  `upgrade.rs:303`.
- Tasks: set `.bin_name(format!("otto {task_name}"))` on the `task_cmd` local at
  `src/cli/parser/help.rs:192-193`, immediately before `print_help()`.
- **Not at `src/cli/parser/command.rs:12`.** That line is inside `task_to_command`
  (fn at `command.rs:11`), the shared builder, whose other caller is
  `discovery.rs:303` with `BuildMode::Bind`: the actual argument-binding path.
  Setting `bin_name` there leaks a display attribute into parsing, not just into
  help composition. That is the refutation, and it is stronger than the
  caller-count argument an earlier draft gave.
- **Not inside the `task_to_command_for_help` wrapper (`command.rs:83`) either.**
  It has five call sites: `command.rs:213` and `:226` embed tasks and builtins as
  subcommands of the root help command, where clap composes the bin_name itself,
  and `parser_tests_a.rs:1190` / `:1213` call `render_long_help()`, which renders a
  Usage line and would land in the blast radius. At the `help.rs:192` call site all
  four are untouched.
- Update the four assertions that pin the old string: `tests/help_behavior_test.rs:302`,
  `:333`, `:368`, `:403`.
- Foreach subtask names contain a colon, so the prefixed form is
  `Usage: otto health-check:database`. That is correct and pasteable; no quoting
  is needed because the colon is not shell-special.
- Add builtin `Usage:` assertions to `tests/builtin_commands_test.rs:76-140`,
  which already shells out `<Builtin> --help` and today asserts only `about` text.
- **Success criteria:** `otto Clean --help` first usage line is
  `Usage: otto Clean [OPTIONS]`; `otto lint --help` first usage line is
  `Usage: otto lint`; `otto --help` still prints `Usage: otto [OPTIONS] [COMMAND]`.

#### Phase 4: One denominator for Success Rate
**Model:** opus
- `src/cli/commands/stats.rs:63-67` currently divides by `total_runs`, which
  includes the 414 rows that are still `running` and can never reach the
  numerator. Change to `successful / (successful + failed)`, matching `:130-135`,
  `:177-182`, `:276-281`.
- Extract the computation into one function next to `format_percentage`
  (`stats.rs:305-307`) and route all four sites through it, so a fifth site cannot
  reintroduce the divergence.
- The overall table already prints `Running` as its own row; keep it, so the
  in-flight population stays visible rather than hidden by the new denominator.
- Define the zero case once, in the shared function: when
  `successful + failed == 0` the rate is not `0.0%` (which reads as "everything
  failed") but `n/a`. All four sites inherit it.
- Update `docs/commands/stats.md:70` and `stats.md:111` to define the rate
  explicitly as "of runs that reached a terminal state".
- Regression test: a store with 1 success, 1 failure, 8 running asserts the
  rendered overall rate is `50.0%`, not `10.0%`. No existing test asserts a
  percentage string (`stats_tests.rs` exercises these paths but asserts on
  execution), so this is net-new.
- **Success criteria:** the new test fails on `96951d5` with `10.0%` and passes
  after, and all four call sites render through one helper. The grep
  (`grep -c '\* 100.0' src/cli/commands/stats.rs`, 4 on main) is a signal, not a
  gate: a correct implementation may put the helper in another module.

#### Phase 5: `Stats --json` returns what `Stats` prints
**Model:** opus
- `src/cli/commands/stats.rs:56-60` returns before the per-task fetch at `:109`.
  Move the JSON emit after both payloads are gathered.
- **Do not add a field to `OverallStats`.** It is a port-level type
  (declared `src/executor/state/manager.rs:84`, constructed at `src/ports/db.rs:338`
  by a second store impl), so a new field forces every implementation to populate it. Instead
  declare a serialize-only view struct in `stats.rs` that `#[serde(flatten)]`s the
  `OverallStats` it already has and adds `tasks: Vec<TaskStats>` from
  `store.get_all_task_stats(Some(self.limit))`, so `--limit` finally applies to
  JSON.
- `#[serde(flatten)]` is what keeps the seven keys at the top level and in place;
  an envelope would move all of them. Verified against the pinned `serde 1.0.228` /
  `serde_json 1.0.146` with the real seven fields: declaration order preserved,
  number formatting identical, the seven pairs byte-for-byte identical and in the
  same order, `tasks` appended eighth.
- Additive on purpose: the seven existing top-level keys keep their names, so
  `docs/commands/stats.md:129`'s promise ("no `successful_executions` keys here")
  stays true at the top level, with those keys living under `tasks[]` where they
  already do for `Stats <task> --json`.
- `docs/commands/stats.md:115-129` prints the schema verbatim and `stats.md:7`
  says the page is regenerated from observed output. Regenerate it.
- Update `src/cli/commands/stats_tests.rs:136` `test_execute_overall_stats_json`,
  which covers exactly this branch.
- **Success criteria:** `otto Stats --json | jq -e '.tasks | length > 0'` exits 0;
  `otto Stats --json -n 3 | jq '.tasks | length'` returns at most 3;
  `otto Stats --json | jq -e '.total_runs'` still resolves.

#### Phase 6: `--format` requires `--tasks`
**Model:** sonnet
- `src/cli/parser/help.rs:33-38`: add `.requires("tasks")` to the `--format` arg.
  The `tasks` flag is two entries up at `help.rs:29-32`.
- `global_args()` is shared with two help builders (`command.rs:194-196`,
  `command.rs:245-248`) that never parse, so `requires` is inert there.
- Behavior change to state plainly: `otto --format json Graph` (global flag
  *before* a builtin name) becomes a usage error. `otto Graph --format dot` is
  unaffected: Graph is deliberately excluded from the early route
  (`src/main.rs:328-334`) and reads the value from its own bound param
  (`src/app.rs:119`).
- No existing test uses global `--format` without `--tasks`
  (`tests/cli_surface_test.rs:121` is a task's own param;
  `tests/tasks_flag_test.rs:89` and `tests/makefile_converter_test.rs:335` both
  pair it with `--tasks`).
- Regression test: `otto --format yaml` in a fixture repo exits non-zero and runs
  no task.
- **Success criteria:** `otto --format yaml` exits non-zero with a usage error
  naming `--tasks`, and no task output appears; `otto --tasks --format yaml` is
  unchanged. (Usage errors in this binary exit **1**, not clap's default 2:
  observed on main, `otto <task> --no-prefix` -> exit 1.)

#### Phase 7: One implementation of "size of a directory"
**Model:** opus
- `CleanCommand::calculate_dir_size` (`clean.rs:541-567`) and
  `Workspace::calculate_directory_size` (`workspace.rs:436-450`) compute the same
  quantity and disagree on symlinks.
- Keep one, in one place, that does **not** follow symlinks. Following them counts
  the shared `.cache/` blob once per referencing run, which is not the size of the
  run directory.
- Route both call sites through it: `clean.rs:343` and `workspace.rs:424`.
- Note the consequence honestly in `docs/commands/stats.md`: `total_disk_usage`
  will report slightly less than it did, because it stops double-counting.
- **Success criteria:** a fixture run directory containing a symlink to a 1 MB file
  outside it reports the same byte count from both callers, **and that count
  excludes the symlink target** (two callers agreeing on a wrong number would
  satisfy "the same count"). The grep (`grep -rn 'fn calculate_dir' src/`, 2 on
  main) is a signal, not a gate: the surviving function may be renamed or moved.

#### Phase 8: `Clean` can see every run directory
**Model:** opus
- Add an orphan sweep to the DB path (`clean.rs:158-289`): **after** the
  row-driven pass completes, scan the run roots for timestamp-named directories
  with no corresponding row, and apply the **same** `Retention::expired`
  (`clean.rs:322-335`) the filesystem path uses. Ordering the passes this way is
  harmless but is **not** load-bearing: under the union construction below, a
  NULL-`run_dir` row's directory is already an orphan by path and does not need the
  row deleted first to become one. An earlier draft gave that as the reason;
  implementing to it produces a dynamic two-pass design instead of the union.
- **`--dry-run` try-locks too.** AC1 compares two dry-run selections, so a dry run
  that skipped the lock filter would select a set a real invocation would not
  delete, and AC1 would stop measuring the thing it exists to measure. Try-locking
  is side-effect-free under the absent-lock rule above.
- **Delete the early return first.** `clean.rs:168-171` returns as soon as the row
  selection is empty, printing `No runs matching deletion criteria found`. That is
  the exact string AC1 records as today's baseline, so a sweep placed literally
  "after the row-driven pass" would never execute in the one case F6 is about.
  Restructure so the sweep runs on an empty row selection, and report "nothing to
  do" only once both passes have found nothing.
- **Mirror the filesystem path's statusless `--keep-failed` handling.** A directory
  scan cannot tell a failed run from a successful one; `clean.rs:300-324` already
  widens `keep_days` to the larger cutoff, warns, and sets `keep_failed_days: None`.
  The sweep is equally statusless, so "the same `Retention::expired`" is not
  sufficient: it must reuse that widening, or AC1 fails under `--keep-failed`.
- Report the two populations separately so the output is honest:
  `N rows from the database, M orphaned directories`. The first number counts
  **rows**, the second counts **directories**, and for a NULL-`run_dir` row whose
  directory the sweep takes, the same logical run is counted once in each. That is
  correct rather than a double-delete: the row pass removes a row it can identify
  and no directory it cannot, and the sweep removes a directory by path. The
  wording says `rows` for exactly this reason.
- **Apply `--keep-last` once, over the union.** `state/retention.rs:46-64` does
  `.skip(keep_count)` per population, so running it separately over rows and over
  orphans keeps N of each, up to 2N, where `--no-db` keeps N total. Order the union
  by timestamp, apply `--keep-last` to that, then split into the two passes.
- The union is over **present, unlocked directories**, not over all rows. A row
  whose directory is gone has no `--no-db` counterpart, so letting it consume a
  `keep_last` slot makes DB mode delete a directory `--no-db` keeps.
- **A row whose directory is already gone is still deleted, by the row pass.** It
  is excluded from the retention *union* and from the directory set AC1 compares,
  not from deletion, and it is counted in the row pass's reporting.
- **Which filters reach those rows:** `--keep-days`, and `--keep-failed` since the
  row pass is status-aware. **Not `--keep-last`**, which applies only to the
  directory union. So the row pass draws from two sources with two policies: rows
  whose directory is present, retained via the union, and rows whose directory is
  gone, age-filtered only. Without this, "still deleted, by the row pass" read
  literally deletes a NULL-`run_dir` row belonging to a run from two days ago. Saying only
  "excluded from the union" invites an implementer to populate the row pass *from*
  the union, at which point those rows are never deleted, the 388 NULL-`run_dir`
  rows grow without bound, and F8's disposition ("delete the row and let the sweep
  reclaim the directory by path") stops being true for every row whose directory is
  already gone. Silent and permanent.
- **Do not build it by calling `find_old_runs` with `keep_last` and then applying
  the policy again.** `find_old_runs` already applies `Retention::expired`
  internally (`manager.rs:728-736`), so that is the second application: exactly the
  bug this bullet exists to prevent. The obvious implementation is the wrong one.
- **Build the union by scanning, unfiltered, and never source it from
  `find_old_runs` at all**, not even with `keep_last: None`: it still age-filters
  (`manager.rs:720-740` builds a `Retention` from all three params and returns only
  expired rows). Worked example: 100 runs, 90 past cutoff, `--keep-last 5`.
  `--no-db` skips the 5 newest overall and deletes 90. A union pre-filtered to the
  90 skips the 5 newest *of those* and deletes 85.
- Use the same scan and name-parsing path as `--no-db` (`clean.rs:432`, `:459`,
  `:509`) rather than a parallel implementation, or the two modes drift again on
  which directory names count as a run.
- **A run root is `~/.otto/<project-name>-<project-hash>/`;** its children are the
  timestamp-named run directories (`src/executor/layout.rs:96-98`). The sweep
  enumerates those children, not the whole tree.
- **The sweep can reach a live run's directory, and needs a liveness signal.** An
  earlier draft of this doc claimed the phase "adds no new sharp edge". That was
  wrong. The corrected exposure is also narrower than the review panel's: the run
  row is INSERTed at `src/app.rs:427` (`save_execution_context` ->
  `record_run_start_in_db`, `workspace.rs:351`), **not** after `execute_all()`. A
  run is therefore row-less only between `workspace.init()` at `app.rs:420` and
  `app.rs:427`, which is milliseconds, not its whole duration. Two cases widen it:
  - `record_run_start_in_db` degrades silently: `workspace.rs:357-359` returns when
    there is no store, and `workspace.rs:388-390` only `log::warn!`s on insert
    failure. A run whose database is unavailable is row-less for its entire
    duration.
  - `--keep-last` has no CLI default (`clean.rs:64-66`) and `--keep-days 0` puts the
    cutoff at now, so nothing else holds a seconds-old directory back.

  Today the default path structurally *cannot* reach a row-less directory; this
  phase hands it one, so the exposure is new even though it is small.
  **Mitigation: an advisory lock, not an mtime grace.** An earlier draft borrowed
  `CACHE_PRUNE_GRACE` (`pruning.rs:11-15`). That does not work here, and the reason
  is worth writing down because the analogy is seductive. A directory's mtime moves
  only when its own immediate entries are created, removed, or renamed. A run
  directory gets both of its entries (`run.yaml` and `tasks/`) at
  `workspace.rs:241-251`, and every subsequent write lands in `tasks/<name>/`, two
  levels down. Measured on this filesystem with nanosecond stamps:

  ```
  after init:                 1788681195.6457345
  after write 2 levels down:  1788681195.6457345  changed=NO
  after append to own file:   1788681195.6457345  changed=NO
  after NEW entry in dir:     1788681195.663735   changed=YES
  ```

  So the run directory's mtime is frozen at run start and never moves again. A
  grace keyed to it is a 15-minute floor on the *start* time, which is the same
  signal `Retention::expired` already compares (`state/retention.rs:62`, the timestamp
  being the directory name). It protects a run shorter than the floor and nothing
  else. `otto ci` in this repo is a three-minute build; at minute 16 with an
  unavailable database the directory is selected anyway. In the cache case the
  mtime IS the signal of interest, because `written_recently()` (`pruning.rs:268`)
  tests a file that was just written; here it tests a directory nothing touches
  again.

  Instead: **the run holds an advisory `flock` on a `.lock` file in its own run
  directory for the lifetime of the process, and any sweep skips a directory whose
  lock is held.**

  Acquired **immediately after the successful exclusive mkdir**, inside the
  `RUN_DIR_ATTEMPTS` loop at `workspace.rs:179-186`, before `Workspace::new`
  returns. Not in `init()`: production calls `Workspace::new` at `app.rs:419` and
  `init()` at `app.rs:420`, so a lock taken in `init()` leaves the directory
  existing and unlocked across that gap. The comment already at that site says
  reserving a name and creating it "have to be one step" because same-second runs
  raced each other; the lock belongs in that same step for the same reason.
  - **`Workspace` gains a field to hold the `File`, and that field is what makes
    "for the lifetime of the process" true.** `flock` lives on the open file
    description and releases when the last descriptor closes, which in Rust means
    when the `File` drops, not at process exit. `Workspace`
    (`workspace.rs:84-101`) has fields for home, root, hash, time, project, cache,
    run, db_run_id, state_store and fs, and **no** field for a lock handle. Taken
    as a local inside the `RUN_DIR_ATTEMPTS` loop, the `File` drops when
    `new_with_hash_and_fs` returns, before the first task starts, and the
    protection is silently absent: it compiles, the lock is taken, and nothing is
    guarded. The field closes that. The resulting lifetime reaches far enough:
    `TaskScheduler` owns `Arc<Workspace>` (`scheduler.rs:1236`, init `:1286`), task
    bodies clone it (`scheduler/task_execution.rs:36`), and both entry paths keep
    it alive through `record_run_complete_in_db` (`app.rs:419`/`:451` terminal,
    `app.rs:575`/`:721` TUI).
  - **Acquisition failure at run start aborts the run.** If creating, opening or
    flocking the run's own `.lock` fails (read-only filesystem, NFS without write
    access, fd exhaustion), abort before task execution rather than warning and
    proceeding. Proceeding unprotected is the exact failure the mechanism exists to
    prevent and it would be invisible. This keeps both sides symmetric with the
    sweep's fail-closed rule below.
  - **The one window this leaves is bounded and not worth machinery.** The `.lock`
    lives inside the run directory, so it cannot exist before the directory does,
    and between `create_dir_exclusive` succeeding (`workspace.rs:182`) and the
    flock a concurrent sweep could see the directory, get `ENOENT`, and apply the
    absent-lock rule. Run directory names carry whole-second timestamps
    (`layout.rs:96-98`) and `Retention::expired` filters on
    `run.timestamp < cutoff` with `cutoff = now - keep_days*86400`
    (`state/retention.rs:51`, `:62`), so at `--keep-days 0` the comparison is
    `timestamp < now`, false for anything created in the current second. The sweep
    cannot select the directory until the next second boundary, while the gap being
    raced is one open plus one flock. Strictly narrower than the window that
    existed before acquisition moved out of `init()`. Do not build for it.
  - **An absent `.lock` means not live, therefore deletable.** This has to be
    stated, because every one of the 1993 orphans lacks one, as does every run
    created before this ships. Reading "cannot open, so cannot acquire, so skip"
    would make Phase 8 reclaim nothing and leave F6 unfixed.
  - **A lock error that is not `EWOULDBLOCK` fails closed and is reported.** Skip
    the directory and say so. `~/.otto` on an NFS mount is the case that matters:
    Linux emulates `flock` there via whole-file `fcntl` and an exclusive lock needs
    write access, so the failure mode degrades to "reclaims nothing, loudly" rather
    than "deletes a live run, silently".
  - **How the sweep opens it: existing file, read-write, no `O_CREAT`.** `ENOENT`
    is the absent-therefore-deletable case above. `O_RDONLY` is not sufficient,
    because the NFS branch named just above needs write access for an exclusive
    lock; `O_CREAT` would write into a directory the sweep is about to delete and
    would break `--dry-run`'s side-effect-free property.
  - **Hold the lock through `remove_dir_all`, not just for the test.** A try-lock
    that releases before deleting lets two concurrent `Clean` runs each acquire and
    each delete.
  - Correct for a run of any length, which the floor is not.
  - Fails safe in both directions: a live run is not swept outside a
    sub-millisecond window between the exclusive mkdir and the flock, and a run
    killed by SIGKILL releases immediately, so there is no dead zone where a
    crashed run's
    directory is unreclaimable.
  - One signal for one meaning. No grace constant, so no second knob that has to be
    kept consistent with `CACHE_PRUNE_GRACE`.
  - **The `--no-db` path honors it too.** `otto Clean --no-db --keep-days 0` can
    already delete a live run's directory today; this is the same defect, and the
    lock is the one chokepoint that closes it for both modes rather than only for
    the path this phase adds.
  - **Liveness is the otto process, not its task children.** Rust's
    `OpenOptions::open` sets `FD_CLOEXEC`, and otto execs task bodies through
    `tokio::process::Command` (`scheduler/task_execution.rs:219`, spawn at `:296`
    and `:324`),
    so the fd closes at exec and no task child holds the lock. That is the behavior
    this design wants: a task that daemonizes something would otherwise pin its run
    directory's lock forever, making that directory permanently unreclaimable,
    which is a new permanent leak of exactly the kind Phase 8 exists to close.
    Close-on-exec is the default, so this costs no code. A task that detaches a
    daemon is outside the run once the task process exits.

  Measured with a Rust program using `OpenOptions` + `flock`, **not** shell
  `flock`. An earlier draft of this doc measured the shell, whose fd is not
  close-on-exec, and drew the opposite and wrong conclusion:

  ```
  PARENT fd=4 FD_CLOEXEC set by Rust std: true
  PARENT flock rc=0
  PARENT exiting while grandchild sleeps
  --- grandchild sleeps alive: 4 ---
  THIRD-PARTY-ACQUIRED     try-exit=0
  ```

  and, separately, the exclusion and release properties:

  ```
  try-lock while another process holds it   -> exit 1 (not acquired)
  same try-lock after SIGKILL of the holder -> ACQUIRED, exit 0
  ```

  That second pair is why there is no dead zone: a SIGKILLed run releases
  immediately and its directory is reclaimable on the next sweep.

  `ensure_deletable_under_root` (`pruning.rs:29-57`) still fences every deletion.
- `auto_prune` is not exposed **by default**: `pruning.rs:80-84` passes
  `keep_days` / `keep_last` / `keep_failed` straight from the ottofile's
  `otto.retention` spec, which defaults to 30 / 10 / 60
  (`docs/commands/ottofile-reference.md:73-77`) but is user-settable. A user with
  `otto.retention.keep_days: 0` gets auto-prune at 0. The defaults are what make it
  safe today; the lock is what actually protects it.
- `auto_prune` (`src/executor/pruning.rs:63-146`) inherits the sweep by using the
  default path; leave `no_db: false` alone.
- Fix `resolve_run_directory` (`src/executor/state/manager.rs:847-878`): stop
  reconstructing a `<timestamp>` path that cannot match a `<timestamp>-<seq>`
  directory (`src/executor/layout.rs:96-98`) and cannot survive a moved ottofile.
  When `run_dir IS NULL`, do not guess. Delete the row and let the orphan sweep
  reclaim the directory by path, which it now can.
- Make the miss visible: today it is `log::warn!` only, which `setup_logging`
  sends to the log file. Count unresolvable rows and print the count in non-quiet
  mode.
- Correct `docs/commands/clean.md:192` and `clean.md:221`, which claim database
  and filesystem stay synchronized and are deleted atomically. After this phase
  the first is true for the default path; the second is not, and should say what
  actually happens.
- Extend `tests/cleanup_integration_test.rs:225`
  (`test_clean_database_mode_vs_filesystem_mode`), which is the existing seam.
- Regression test: a fixture `~/.otto` with one row-backed run and one
  directory-only run, both past retention, has both removed by the default path.
- **Success criteria:** on a fixture tree, `Clean --dry-run` and
  `Clean --dry-run --no-db` select the same set of runs; the NULL-`run_dir`
  fixture leaves no orphaned directory behind.
- **Upgrade window, added 2026-09-06 by the implementation audit.** Only a build
  carrying this phase writes the `.lock`, so a run already in flight when that
  build is installed has no lock and the sweep will delete its directory out from
  under it. Measured across all three combinations: a pre-lock run swept by a
  pre-lock `Clean` is deleted, a pre-lock run swept by a lock-aware `Clean` is
  deleted, and a lock-aware run is skipped with `a run is still using it`. This is
  not a regression, since the pre-lock build had the identical defect on every
  run, and it is bounded by retention: reaching it needs `--keep-days 0` or a run
  outstanding past the 30 day default. It closes itself once every in-flight run
  postdates the upgrade.

#### Phase 9: The run record says what was asked for
**Model:** sonnet
- `src/executor/workspace.rs:78` hardcodes `args: vec!["otto"]` in
  `ExecutionContext::new()`, so `otto lint` records `args: ["otto"]` and `History`
  cannot say what a run was for.
- **Do not record `std::env::args()` verbatim.** Task params are ordinary command
  line flags (`otto deploy --token abc`), and the run record is persisted twice:
  into `runs.args` in the DB (`src/executor/state/manager.rs:171-186`) and into
  `run.yaml` on disk. Recording raw argv turns every run directory into a
  credential sink. Today the hardcoded `["otto"]` leaks nothing, and a fix that
  introduces a secret-disclosure surface is worse than the defect it closes.
- Record `argv[0]` plus the **requested task and subtask names only**, no flag
  values. That is what "what was this run for" actually means, and it cannot
  carry a secret.
- Not the resolved run set either: `tasks` at `src/app.rs:422-424` and `:578-580`
  is the transitive closure including every pulled-in dependency, which is not
  what the user asked for.
- Document the field's meaning in `docs/commands/history.md` so the next reader
  does not expect a full command line.
- Propagation path, since the roots are not carried anywhere near the writer today:
  `ExecutionContext::new()` (`workspace.rs:78`) is built identically at `app.rs:422`
  and `app.rs:578`, and `RunPlan` (`parser.rs:133-141`) carries only the resolved
  closure. Threading the requested root names from the parser to both construction
  sites is the whole of this phase's wiring.
- **A bare `otto` run has roots too.** It resolves to the ottofile's `otto.tasks:`
  default list, so a bare `otto` in this repo records `["otto", "ci"]`. That is the
  requested set, not the closure: asking for nothing is asking for the default.
- **Success criteria:** after `otto lint` in this repo,
  `otto History -n 1 --json | jq -r '.[0].args | join(" ")'` returns `otto lint`
  (observed on main: `otto`). The `-n 1` window is global across projects, so run
  the check immediately after the `lint` invocation. A second assertion: running a
  task with a param whose value is `SHOULD-NOT-APPEAR` leaves that string absent
  from both the DB row and `run.yaml`.

## Acceptance Criteria

Every criterion below was executed against `main` at `96951d5` on 2026-09-06 and
its output recorded. Each is FALSE today and TRUE when the work is done.

- [x] **AC1** `otto Clean --dry-run --keep-days 30` and
      `otto Clean --dry-run --keep-days 30 --no-db` select the same **set of run
      directories** (not the same summed counts, and not under `--keep-failed`:
      see below).
      `Observed on main:` DB mode `No runs matching deletion criteria found`;
      `--no-db` `Found 1993 runs to delete by keeping everything for 30 days (173.4 MB total)`.
      Directory **sets**, not counts, because a NULL-`run_dir` row contributes a row
      deletion with no directory analogue in `--no-db` mode. Scoped to invocations
      without `--keep-failed`: the row pass is status-aware
      (`clean.rs:161-165` passes it into `find_old_runs`) while the filesystem path
      widens statuslessly for everything (`clean.rs:300-324`), so under that flag
      the two modes differ **by design**, because run status exists only in the
      database. Phase 8 does not try to make them agree there, and says so.
- [x] **AC2** `grep -rn 'Tasks this one runs' docs/commands/` returns exactly zero
      lines.
      `Observed on main:` 2 lines (`ottofile-reference.md:91`, `:92`). The phrase
      also appears outside `docs/commands/`, in `docs/design/` point-in-time
      records (the 2026-09-02 implementation notes and this document); those are
      out of the criterion's scope and stay as written.
- [x] **AC3** `otto Clean --help` first usage line is `Usage: otto Clean [OPTIONS]`.
      `Observed on main:` `Usage: Clean [OPTIONS]`.
- [x] **AC4** `otto Stats --json | jq -e 'has("tasks")'` exits 0.
      `Observed on main:` prints `false`, exit 1.
- [x] **AC5** `otto --format yaml` with no `--tasks` exits non-zero and runs no
      task.
      `Observed on main:` exit 0, and the fixture ottofile's default task ran
      (2 lines written to the task's order file). Usage errors in this binary exit
      1, so the criterion is non-zero rather than a specific code.
- [x] **AC6** `otto Upgrade --help | grep -c 'hide_env_values'` returns 0.
      `Observed on main:` 1.
- [x] **AC7** (F5) A store holding 1 success, 1 failure and 8 running renders an
      overall Success Rate of `50.0%`, and the per-task table for the same data
      renders `50.0%` too: one denominator, asserted through the rendered output.
      `Observed on main:` the fixture does not exist yet, so the criterion's own
      command cannot be run until Phase 4 writes it. The divergence it asserts
      against **is** observed, on the live store: `otto Stats` renders
      `Successful 432 (42.7%)` over `Total Runs 1011` in the overall table while
      the per-task table on the same invocation renders `otto / lint / 93 / 93 /
      0 / 100.0%`. Two denominators, one screen. Secondary, non-gating signal:
      `grep -c '\* 100.0' src/cli/commands/stats.rs` goes 4 -> 1.
- [x] **AC8** (F9) A fixture run directory containing a symlink to a 1 MB file
      outside it reports the **same** byte count from the `Clean` caller and the
      `Workspace` caller, **and that count excludes the 1 MB target**.
      `Observed on main:` the fixture does not exist yet, so this criterion's
      command cannot be run until Phase 7 writes it. What is observed on main is
      the divergence it asserts against, read from the two implementations:
      `clean.rs:541-567` skips symlinks, `workspace.rs:436-450` follows them via
      both `is_dir()` and `metadata()`.
      **Amended 2026-09-06 during Phase 7, twice.** (a) A link pointing straight at
      the 1 MB *file* does not exercise the divergence: `DirEntry::metadata()` does
      not traverse links, so the old implementation counted such a link as its own
      28-byte path string and the 1 MB was never summed either way (102524 vs
      102552). Only the `entry_path.is_dir()` branch follows. The fixture therefore
      links at both the file, which is this criterion's literal wording, and the
      directory containing it, which is the real `.cache` shape and the case that
      actually diverged: 102524 vs 1151128 before the change, 102524 vs 102524
      after. (b) The secondary signal `grep -rn 'fn calculate_dir' src/` goes
      2 -> **0**, not 2 -> 1, because the survivor is named `directory_size`. This
      criterion's own closing paragraph allows the rename; the equivalent signal is
      `grep -rn 'fn directory_size' src/` == 1 definition.

AC7 and AC8 are deliberately behavioral rather than greps. A grep over source is
satisfied by renaming a function or writing `100f64`, which is the failure mode the
ready-to-build gate exists to catch; the greps are kept only as a secondary signal
that the consolidation actually happened.
- [x] **AC9** (Phase 9) after `otto lint` in this repo,
      `otto History -n 1 --json | jq -r '.[0].args | join(" ")'` returns
      `otto lint`.
      `Observed on main:` `otto`.

F8 has no AC of its own: it has no CLI-observable symptom on `main` (it is a
latent orphaning path armed on the 388 NULL-`run_dir` rows), so it is verified by
Phase 8's fixture criterion instead of a repo-level assertion.

## Resolved Decisions

- **2026-09-06, F1 ruling: correct the docs, not the scheduler.** The author's
  call, which closes the question `docs/design/2026-09-02-second-code-review-remediation-implementation-notes.md:182`
  left open four days earlier. `after: [Y]` continues to mean "Y runs after this
  task"; Phase 1 makes `docs/commands/ottofile-reference.md` and
  `docs/commands/tasks.md` say so. Alternative 1 (inverting the scheduler) is
  rejected: it would silently re-order every ottofile already written against the
  current semantics, this repo's `.otto.yml` among them. Phase 1 is ungated.
- **2026-09-06, the shakedown report is archived, not carried.** The author
  archived `docs/shakedown-v2.3.0-postmerge.md` with `rkvr rmrf` (recoverable via
  `rkvr rcvr`). Every finding it recorded is restated in this document, so the
  three references to it are removed rather than repointed, and Phase 1 loses its
  bullet correcting that file's "leaf tasks" jq recipe: the recipe went with the
  file. Nothing in `bin/check-doc-links` breaks, because all three references were
  code spans, not markdown links.
- **2026-09-06, F4 envelope shape.** `Stats --json` gains an additive `tasks` key
  rather than an `{overall, tasks}` envelope. An envelope moves all seven existing
  keys one level down and breaks every consumer; the additive key breaks none and
  keeps `stats.md:129`'s promise true at the top level. Closed by the author
  against the research brief's open question.
- **2026-09-06, F5 denominator.** All four sites use
  `successful / (successful + failed)`. The alternative (make the three per-task
  sites match the overall one) would spread the 414-stuck-rows distortion to every
  row instead of removing it. Closed by the author.
- **2026-09-06, F8 NULL `run_dir` handling.** Do not repair the reconstruction;
  delete the row and let Phase 8's orphan sweep reclaim the directory by path.
  Repairing it means teaching `resolve_run_directory` about `-<seq>` suffixes and
  historical project names, which is synchronization logic for a derived value.
  The sweep makes the derivation unnecessary. Closed by the author.
- **2026-09-06, F9 symlink policy.** The single size function does not follow
  symlinks. Every symlink under `~/.otto` points into the shared `.cache/`, so
  following them counts one blob once per referencing run. Closed by the author.

- **2026-09-06, review panel round 1.** Four must-fix items, all folded in: the
  Data Model section contradicted Phase 5 on `OverallStats` (M1); the sweep as
  written would never run in the case F6 is about, because `clean.rs:168-171`
  returns first (M2); the sweep can reach a live run's directory and needs the
  `CACHE_PRUNE_GRACE` treatment (M3); the sweep must reuse the statusless
  `--keep-failed` widening (M4). Two panel claims were checked and corrected rather
  than accepted: the run row is written at `app.rs:427`, before execution, so
  in-flight runs are row-less for milliseconds and not for their whole duration
  (M3 as filed overstated it), and the seam refutation for Phase 3 is that
  `command.rs:12` sits in `task_to_command`, which the `BuildMode::Bind` parse path
  also uses, not that a wrapper has three callers.
- **2026-09-06, `args` for a bare `otto`.** The panel proposed opening this as a
  question. Closed instead: a bare `otto` resolves to the ottofile's `otto.tasks:`
  default list, so it records `["otto", "ci"]` in this repo. Asking for nothing is
  asking for the default, and the default is a requested root, not part of the
  resolved closure.
- **2026-09-06, the 173.4 MB / 375 MB gap.** Not an instance of F9. `du
  --apparent-size` over the same 1993 directories returns 174 MB; the difference is
  4 KB block rounding over many tiny files. Recorded in the Problem Statement so it
  is not re-raised as a size-computation defect.

- **2026-09-06, review panel round 2.** Two must-fix, both in Phase 8, both
  folded. M5: the `CACHE_PRUNE_GRACE` mitigation I took from round 1 does not work,
  because a run directory's mtime is frozen at init. Verified with nanosecond
  stamps on this filesystem; the measurement is in Phase 8. Neither of the panel's
  two offered options was taken (touch a marker per task, or accept the residual
  risk): the doc specifies an advisory `flock` held for the run's lifetime instead,
  which is correct for a run of any length, needs no constant, releases on SIGKILL,
  and closes the same pre-existing hazard on the `--no-db` path. M6: AC1 now
  compares the SET of run directories, and the reporting line says `rows` rather
  than `runs` so a NULL-`run_dir` row counted in both passes is not read as a
  double delete. Cheap-wins C6 (apply `--keep-last` once over the union), C7
  (`--keep-failed` parity is out of scope by design and AC1 says so), C8 (the
  flatten wording was backwards and is corrected in both places) and C9 (the greps
  are non-gating in the phases too) are in.
- **2026-09-06, mtime as a liveness signal.** Rejected. A directory's mtime moves
  only on changes to its own immediate entries, and a run directory's two entries
  are both created at init, so nothing a running task does touches it. Recorded so
  the grace idea is not reintroduced.

- **2026-09-06, review panel round 3.** Four must-fix, all folded, all spec gaps in
  the lock rather than problems with the lock. M7: the doc's claim that task
  children inherit the lock was **false**, and my own measurement supporting it was
  invalid because it measured shell `flock`, whose fd is not close-on-exec. Rust's
  `OpenOptions::open` sets `FD_CLOEXEC`; re-measured with an actual Rust program
  (`FD_CLOEXEC set by Rust std: true`, third party acquired the lock after the
  parent exited with a task child still alive). The corrected behavior is the one
  the design wants, because an inherited lock would let a daemonizing task pin its
  run directory forever. M8: an absent `.lock` means deletable, stated explicitly,
  because every one of the 1993 orphans lacks one. M9: acquire inside the
  `RUN_DIR_ATTEMPTS` loop at `workspace.rs:179-186`, not in `init()`, because
  `app.rs:419-420` leaves the directory existing and unlocked between the two
  calls. M10: `--dry-run` try-locks, or AC1 stops measuring what it exists to
  measure. Cheap-wins C10 through C15 all in.
- **2026-09-06, fd inheritance as a liveness extension.** Rejected on evidence. A
  lock surviving into task children would make any run that daemonizes a process
  leak its run directory permanently. Close-on-exec is the default and is correct;
  liveness is the otto process only.

- **2026-09-06, review panel round 4.** Three must-fix, all folded, all inside the
  lock spec. M12 is the one that would have shipped broken: `flock` releases when
  the `File` drops, not at process exit, and `Workspace` (`workspace.rs:84-101`)
  has no field to hold it, so the lock taken inside the `RUN_DIR_ATTEMPTS` loop
  would have dropped before the first task started. It compiles, the lock is taken,
  and nothing is guarded. Verified: the struct has ten fields and none is a lock
  handle. M11: acquisition failure at run start aborts the run rather than
  proceeding unprotected. M13: a row whose directory is already gone is excluded
  from the retention union but still deleted by the row pass, said explicitly so an
  implementer does not populate the row pass from the union and leak the 388
  NULL-`run_dir` rows forever. Cheap-wins C16 (open existing, read-write, no
  `O_CREAT`), C17 (hold the lock through `remove_dir_all`), C18 (build the union by
  unfiltered scan, never from `find_old_runs` even with `keep_last: None`) and C19
  (the "ordering is load-bearing" rationale is obsolete under the union) are in.
  Also corrected a path this doc inherited: `task_execution.rs` is at
  `src/executor/scheduler/task_execution.rs`.

- **2026-09-06, review panel round 5 (final).** Zero must-fix. Two one-sentence
  items folded: directory-less rows are age-filtered by `--keep-days` and
  `--keep-failed` in the row pass and are not subject to `--keep-last`, which
  applies only to the directory union; and "a live run is never swept" was too
  strong, since the `.lock` cannot exist before the directory does. That window is
  now stated and bounded rather than papered over: whole-second directory names
  plus `timestamp < cutoff` mean the sweep cannot select a directory created in the
  current second, so the race is one open plus one flock against a second boundary.
  Verified at `state/retention.rs:51`/`:62`. No machinery for it. Panel closed after
  five rounds: 4/2/4/3/0 must-fix, rounds 2 through 5 entirely Phase 8 and entirely
  one question, what the sweep is allowed to touch.
- **2026-09-06, path anchors.** `retention.rs` is `src/executor/state/retention.rs`
  and `task_execution.rs` is `src/executor/scheduler/task_execution.rs`. Both bare
  filenames were inherited from review reports and both are corrected; the house
  convention is to give the directory when a bare filename is ambiguous.

## Alternatives Considered

### Alternative 1: Flip the scheduler instead of the docs (F1)
- **Description:** Make `after: [Y]` mean "this task runs after Y", matching the
  documentation and the plain-English reading.
- **Pros:** The names would stop being confusing. The docs would already be right.
- **Cons:** Silently inverts every ottofile in the fleet. This repo's own
  `.otto.yml:59-61, 93, 216-218` uses the current semantics, as do
  `examples/file-dependencies/otto.yml` and `examples/conditional-deps/otto.yaml`;
  `~/.otto` holds 333 project directories. Four rustdoc sites already document the
  current behavior correctly. `on-failure` sugar (`params.rs:250-273`) is built on
  it.
- **Why not chosen:** Ruled out by the author on 2026-09-06. Correcting the docs
  is the fix; inverting the scheduler would silently re-order every ottofile that
  already runs against the current semantics.

### Alternative 2: Document `--no-db` as the answer for orphans (F6)
- **Description:** Leave `Clean` alone; document that reclaiming orphaned
  directories requires `--no-db`.
- **Pros:** Zero code.
- **Cons:** Leaves `auto_prune` permanently unable to reclaim, keeps the two modes
  disagreeing, and makes the correct behavior a thing you have to already know.
- **Why not chosen:** A fix that documents the workaround instead of removing the
  defect is not a fix.

### Alternative 3: Touch a marker in the run directory on every task transition (F8/M5)
- **Description:** Make the mtime signal real by writing a marker into the run
  directory each time a task starts or finishes, so a grace period keyed to mtime
  means what it says.
- **Pros:** Keeps the `CACHE_PRUNE_GRACE` analogy intact and needs no locking.
- **Cons:** Adds a filesystem write to every task transition on the hot path to
  serve a cleanup path, and still only narrows the window rather than closing it: a
  run stalled longer than the grace between transitions is unprotected again.
- **Why not chosen:** The lock closes the window outright at lower cost. Recorded
  because the review panel offered it as one of two options.

### Alternative 4: Accept the residual risk and keep the floor (F8/M5)
- **Description:** Keep an mtime grace as a floor, state plainly that a run longer
  than the floor with an unavailable database is not protected.
- **Pros:** Zero new mechanism.
- **Cons:** Documents a sharp edge on a deletion path instead of removing it, and
  the failure it leaves open is a corrupted concurrent build.
- **Why not chosen:** Same reason Alternative 2 was rejected: documenting a
  workaround is not a fix. Recorded because it was the panel's other option.

### Alternative 5: Teach `resolve_run_directory` the `-<seq>` suffix (F8)
- **Description:** Glob `<timestamp>*` and disambiguate.
- **Pros:** Local, no new sweep.
- **Cons:** Still cannot survive a moved or renamed ottofile, because
  `project_name` is re-derived from the current parent directory. Synchronization
  logic for a value that can be looked up by path instead.
- **Why not chosen:** Phase 8's sweep makes the derivation unnecessary.

## Technical Considerations

### Dependencies
No new crates. No schema migration.

### Performance
Phase 8 adds a directory scan to the default `Clean` path, which `auto_prune`
runs. Phase 0 measures it before the design is committed to. If the scan is
expensive, sizes are computed only for selected directories.

### Security
Two phases touch a disclosure surface and both close it rather than open it.

- Phase 2 removes text from a help page. The `hide_env_values(true)` protection
  the text explains stays in force and is asserted by Phase 2's own criterion, so
  the note can go without the guard going with it.
- Phase 9 is the one to watch. The obvious implementation (record `std::env::args()`)
  would persist task param values, which are ordinary flags and can be secrets,
  into both the database and `run.yaml`. The phase records requested task names
  only, and carries a negative assertion proving a param value never lands in
  either store.

Nothing else here touches auth or the network.

### Testing Strategy
Every phase that changes behavior gets a test that fails on `96951d5` first.
Phases 4, 5, 8 and 9 are net-new assertions; Phase 3 updates four existing ones
and adds builtin coverage. `tests/help_behavior_test.rs` is the prior art for
help assertions (`assert_cmd` + `predicates` through `common::otto_cmd`).

### Rollout Plan
One commit per phase on a working branch, `otto ci` green at each. The repo is
gated, so the branch lands through a PR and `bump` cuts the tag afterward.

## Risks and Mitigations

| Risk | Likelihood | Impact | Mitigation |
|------|------------|--------|------------|
| Phase 8's sweep deletes something it should not | Low | High | Reuses `ensure_deletable_under_root` (`pruning.rs:29-57`); `--dry-run` first; fixture test with a symlinked run directory |
| Phase 0 shows the scan is slow enough to hurt `auto_prune` | Medium | Medium | Compute sizes only for selected dirs; the scan is already what `--no-db` does today |
| Phase 5's additive key still surprises a consumer | Low | Low | Existing keys unmoved; `stats.md` regenerated from observed output |
| Phase 6 breaks someone's `otto --format json Graph` | Low | Low | Stated in the phase; `otto Graph --format dot` (the documented form) is unaffected |
| Phase 3's bin_name leaks into embedded subcommand help | Low | Medium | Seam chosen at `help.rs:192-193` specifically to avoid the two embedding call sites |

## Open Questions

F1, the one question this document carried into implementation, was answered by
the author on 2026-09-06: correct the docs. Recorded in Resolved Decisions.

One question was opened by the work itself and is still open, carried in the
implementation notes and repeated here so this section does not read as "None"
while the notes say otherwise:

- [ ] **The two deletion paths fence against different roots.** Phase 8's sweep
      fences against the home `Clean` was given; `StateManager::delete_run` fences
      against `resolve_otto_home()`, which it reads from the environment itself.
      Not reachable in production: both `auto_prune` call sites (`src/app.rs:472-474`,
      `:745-747`) pass `resolve_otto_home()`, the exact value `resolve_run_directory`
      re-resolves, and the mismatched case fails closed (`Refusing to delete run
      directory`, exit 1, nothing deleted). It is a trap for the next test author
      rather than a live defect. Worth deciding before something else takes a home
      as an argument; threading a root through `StateStore::delete_run` was out of
      scope for Phase 8.

## Addendum: cut, parked, and recorded

Kept so none of it is re-litigated.

- **`Clean --orphans-only`** (proposed in this doc's first draft, cut in Pass 4).
  Unrequested scope: the shakedown asked for the two modes to agree, and Phase 8
  delivers that on the default path. Revisit condition: an operator actually needs
  to reclaim orphans without touching rows, and says so.
- **Renaming `before:` / `after:`.** The names are the root of F1's confusion.
  Renaming breaks every ottofile in the fleet. Revisit condition: the next
  breaking schema change.
- **`otto help <NAME>` and `otto <NAME> --help` render from different sources.**
  Found during this dig, not by the shakedown. The TaskSpec path prints
  `--keep-days <keep-days>`, the clap derive prints `--keep-days <KEEP_DAYS>`.
  Phase 3's `bin_name` fix does not unify them. Not in scope: nobody asked, and
  it is a second defect wearing the first one's clothes.
- **Reconciling the 414 stuck `Running` rows and reclaiming the live 1993
  orphans.** Operator actions against Scott's `~/.otto`, not phase work. Phase 4
  removes the rows' effect on the reported rate; Phase 8 makes the reclaim
  possible. Running it is his call.
- **Release-artifact signature verification.** Parked 2026-06-10, re-parked
  2026-09-02. Untouched here.

## References

- The v2.3.0 shakedown report that produced F1-F7, and its Addendum recording
  the corrections to F6: archived 2026-09-06 with `rkvr rmrf`, recoverable via
  `rkvr rcvr`. Its findings are carried in full by this document.
- `docs/design/2026-09-02-second-code-review-remediation.md`: the prior
  remediation; its implementation notes at `:182` first raised F1
- `docs/commands/ottofile-reference.md`, `docs/commands/stats.md`,
  `docs/commands/clean.md`, `docs/commands/tasks.md`: the doc pages this changes
