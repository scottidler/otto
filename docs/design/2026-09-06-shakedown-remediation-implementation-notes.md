# Implementation Notes: Shakedown Remediation

Companion to `docs/design/2026-09-06-shakedown-remediation.md`. Append-only: a
later entry supersedes an earlier one, nothing is rewritten.

## Phase 0: Measure the orphan scan before designing around it

### Design decisions
- **The scan does not need Phase 8's size-only-for-selected optimization.**
  Measured on the live `~/.otto` on `desk.lan`, 2026-09-06, against the installed
  `otto v2.3.0`:

  ```
  run directories (find ~/.otto -mindepth 2 -maxdepth 2 -type d):  7072
  files under ~/.otto (find -type f):                            102708
  otto Clean --dry-run --no-db:                       wall 1.85s, maxrss 7.4 MB
  directories selected by that run:                                 1993
  ```

  The design doc's conditional was "if the scan exceeds a few seconds". 1.85s on
  a tree 39% larger than the 5090 directories the doc anticipated does not, so
  Phase 8 computes sizes on the same terms the current `--no-db` path does.
- **The scan is bounded by size summation, not by inode count.** Two baselines
  over the same tree separate the two costs:

  ```
  find ~/.otto -type f -printf ''   (pure traversal)      0.46s
  du -s ~/.otto/*                   (traversal + stat)    1.10s
  otto Clean --dry-run --no-db                            1.85s
  ```

  Directory enumeration is a quarter of the wall time; the rest is per-file
  metadata and summation. The consequence for Phase 8: a sweep that enumerates
  directories without sizing them is nearly free, and sizing is what to bound if
  the tree ever grows enough to matter.

### Deviations
None.

### Tradeoffs
- Measured the installed `v2.3.0` binary against the real `~/.otto` rather than a
  synthetic fixture. A fixture would be reproducible; the live tree is the thing
  Phase 8 has to be fast enough for, and its shape (2 of 3 directories selected,
  102708 files) is not something a fixture would have guessed.

### Open questions
None.

## Phase 1: Correct the before/after documentation

### Design decisions
- `docs/commands/ottofile-reference.md:91-92` (`tasks.<name>.after`/`before` table
  rows) now read "Tasks that run after this one (this task becomes their
  dependency)" and "Tasks that run before this one (they become this task's
  dependencies)", matching `params.rs:293-295`'s comment verbatim in plain
  English.
- `docs/commands/ottofile-reference.md:101` (`on-failure` table row) now says the
  synthetic edge is pushed onto the host's own `after:` list pointing at the
  named tasks, not "on the named tasks", matching `apply_on_failure_sugar`
  (`params.rs:250-273`): the host is X, the named tasks are the targets, the edge
  lands on `host_spec.after`.
- `src/cfg/task.rs:696-698` (`on_failure` field rustdoc) got the same correction,
  same wording as the doc row above it.
- `docs/commands/tasks.md:69-72`: the worked example's `down: after: [up]` was
  inverted (it made `down` a dependency of `up`, i.e. `down` would run *before*
  `up` starts). Changed to `down: before: [up]`, which makes `up` a dependency
  of `down`: `down` runs after `up`, the behavior the prose ("Stop each
  service") implies. Added one sentence spelling out why `after: [up]` on `down`
  would have been backwards, so a reader who reaches for `after:` first sees the
  trap named. Propagated the same swap (`before`/`after` list contents) through
  the JSON and YAML `--tasks` output blocks immediately below, since `--tasks`
  reports each task's own declared edges verbatim and both blocks are derived
  from the same source example.
- Added a "Worked example: which task runs first" subsection to
  `ottofile-reference.md` right after the Edge object key table, using the
  design doc's own measured wall-clock example (`later` declares
  `after: [first]`, invoked as `otto first`, `later` runs 45ms before `first`).
  Included both correct ways to reverse the order (`after:` on `first`, or
  `before:` on `later`), so a reader who wants "B after A" sees which field goes
  on which task without re-running the experiment.
- Left `src/cli/parser/discovery.rs:113-120`, `params.rs:231-239`,
  `params.rs:293-295`, and `src/cli/parser/foreach.rs:20-32` untouched, per the
  phase's success criteria: `git diff --stat` on those three files is empty.
- Scheduler behavior in `src/cli/parser/params.rs` (`compute_task_deps_from_specs`,
  `apply_on_failure_sugar`) is unchanged: this phase is docs-only, per the
  2026-09-06 F1 ruling in Resolved Decisions.

### Deviations
None. The phase's bullets were followed as written; the `tasks.md` example fix
used the `before:` swap (one of the two options named in the phase's own worked
example) rather than moving the edge to `up`, since it keeps the edit
minimal (one field change on one task) and keeps the example task `down`
declaring its own scheduling relationship, which is the more common ottofile
style in this repo's own `.otto.yml`.

### Tradeoffs
- Rewriting `tasks.md`'s example via `before:` instead of `after:` on `up`: both
  are correct under the real semantics; `before:` on `down` was chosen because it
  changes one token (`after` -> `before`) rather than moving the whole edge to a
  different task's spec, and keeps the JSON/YAML output diffs (this phase also
  had to fix) to a value swap rather than a restructure.

### Open questions
None.

## Phase 2: Stop printing the scar tissue

### Design decisions
- `src/cli/commands/upgrade.rs:334-338`: changed the five-line `///` continuation
  block explaining why `hide_env_values(true)` is load-bearing from doc comments
  to plain `//` comments, so clap no longer folds them into the `--github-token`
  long help. Line 333 (`/// GitHub token for API access (avoids rate limits)`)
  stays `///`: it is the actual user-facing summary line and clap still needs it.
  Re-anchored against current HEAD (`b4c0f8b`) before editing; the lines matched
  the doc's description exactly, no drift from the `96951d5` anchor.

### Deviations
None.

### Tradeoffs
None: the phase specified an exact two-character comment-marker change on five
lines, and that is what was made.

### Open questions
None.

## Phase 3: Make every `Usage:` line pasteable

### Design decisions
- Six builtin structs (`clean.rs:58`, `convert.rs:10`, `graph.rs:42`,
  `history.rs:80`, `stats.rs:12`, `upgrade.rs:303`) each gained a `bin_name` in
  their `#[command(...)]` attribute, alongside the existing `name`. clap's
  `bin_name` fully replaces the rendered Usage prefix for a top-level Command
  rather than prepending to `name`, so `bin_name = "otto"` alone printed
  `Usage: otto [OPTIONS]` with `Clean` silently dropped. Set each to the full
  invocation instead, e.g. `bin_name = "otto Clean"`, matching the pattern the
  phase specifies for tasks. Verified behaviorally on a release build of every
  builtin, not just Clean.
- `src/cli/parser/help.rs:192`: `show_task_help` now calls
  `.bin_name(format!("otto {task_name}"))` on the `task_cmd` local built by
  `task_to_command_for_help(task)`, immediately before `print_help()`. This is
  the one seam that renders help for both real ottofile tasks (`otto lint
  --help`) and builtins reached via `otto help <Name>` / `otto <task> --help`
  routed through `show_task_help` rather than the early-dispatch
  `execute_<builtin>_command` path in `app.rs`. `task_name` is the exact key
  looked up in `config_spec.tasks`, so a foreach subtask name with a colon
  (`health-check:database`) reaches this call unmangled and renders `Usage:
  otto health-check:database` as the phase specifies.
- Confirmed by direct route: builtins invoked as `otto Clean --help` go
  through `main.rs`'s early-route table into `app::execute_clean_command`,
  which calls `CleanCommand::parse_from(args)` (`app.rs:853` and siblings).
  clap's own `--help` handling there uses `CleanCommand::command()`, i.e. the
  derive's own `bin_name`, independent of the `help.rs:192` seam. Both paths
  needed the fix; the doc's "six structs" and "task_cmd local" bullets are the
  two are not redundant with each other, they cover the two different routes
  by which builtin help gets rendered.
- Did **not** touch `command.rs:12` (`task_to_command`, the `BuildMode::Bind`
  parse path shared with `discovery.rs:303`) or `command.rs:83`
  (`task_to_command_for_help`, embedded at `command.rs:213`/`:226` into the
  root `--help` subcommand listing). Verified behaviorally: `otto --help`'s
  `Commands:` listing and `otto help Clean` / `otto help clean` (both task and
  builtin) render exactly one `otto` prefix, no doubling, confirming the Risk
  row's mitigation holds.
- Updated the four pinned assertions at `tests/help_behavior_test.rs:302`,
  `:333`, `:368`, `:403` (anchors matched `96951d5` exactly, no re-anchoring
  needed) from `"Usage: build"` / `"Usage: examples"` to `"Usage: otto
  build"` / `"Usage: otto examples"`.
- Added `Usage: otto <Name>` assertions to the four existing per-builtin tests
  in `tests/builtin_commands_test.rs` that already shell out `<Builtin>
  --help` (`test_graph_command_exists`, `test_clean_command_exists`,
  `test_history_command_exists`, `test_stats_command_exists`, lines 76-140 in
  the doc's anchor), rather than adding new test functions: this file has no
  existing dedicated test for `Convert` or `Upgrade` help output, and adding
  that coverage is outside this phase's stated range.

### Deviations
- The phase's bullet ("Six builtin structs get `bin_name` alongside the
  existing `#[command(name = ...)]`") did not specify the literal value.
  `bin_name = "otto"` alone is wrong for a root Command (clap replaces the
  whole prefix, it does not append to `name`); the correct value is the full
  invocation, `"otto <Name>"`. Caught by running the built binary against the
  phase's own success criteria before finishing, not by the compiler: clap
  accepts either value silently, this is a rendering-only defect.

### Tradeoffs
None: once the `bin_name` semantics were understood correctly, the
implementation matches the doc's seam choice exactly (help.rs:192-193, not the
two embedding call sites).

### Open questions
None.

## Phase 4: One denominator for Success Rate

### Design decisions
- The shared helper is `format_success_rate(successful: u64, failed: u64) -> String`
  in `src/cli/commands/stats.rs`, sitting immediately above `format_percentage`,
  which it still calls for the `{:.1}%` shape. It takes counts, not a
  pre-computed rate, so a caller cannot pick its own denominator: that is the
  whole point of the phase, and a `fn(f64) -> String` signature would leave the
  divergence one line away. `format_percentage` survives as the single place the
  number format lives (Phase 5's regenerated `stats.md` output depends on that
  shape) and keeps its existing test.
- All four rate sites now render through it: the overall table
  (`render_overall_table`), the Top-N task table (`render_task_stats_table`),
  and both branches of `show_task_stats` (single-project and per-project).
  `grep -c '\* 100.0' src/cli/commands/stats.rs` is 1, down from 4.
- The zero case is defined once, inside the helper: `successful + failed == 0`
  renders `n/a`, not `0.0%`. All four sites inherit it.
- `src/cli/commands/history.rs:290` computes a fifth success rate. It was left
  alone: it already divides by `successful + failed`, so it agrees with the new
  denominator, and it is a different command whose zero case suppresses the line
  entirely rather than printing a rate. Routing it through a `stats.rs`-private
  function would mean making the helper `pub` and changing `History`'s output,
  neither of which this phase asks for.

### Deviations
- **AC7 needed a render seam that did not exist; same effect, correct seam.**
  AC7 is asserted through rendered output, and every table was built inline
  inside `show_overall_stats`, which only prints. The table construction moved
  verbatim into two free functions, `render_overall_table(&OverallStats) -> Table`
  and `render_task_stats_table(&[TaskStats]) -> Table`, and
  `show_overall_stats` now gathers both payloads and prints them. Printed output
  is unchanged (`println!("{}", table)` either way; verified against the live
  store). This matches the repo's existing convention at
  `src/cli/commands/tasks.rs:183` (`render_tasks_view(...) -> Result<String>`),
  whose tests assert on the returned string. The alternative was capturing
  stdout, which would have needed a new dev-dependency for a defect that is
  really "the renderer is not callable".
- The doc's `stats.md:70` / `stats.md:111` anchors are now lines 78 and 113.
  The denominator paragraph went in after the overall-tables code block, ahead
  of the "Average Run Duration" prose; the per-project sentence got the clause
  inline.

### Tradeoffs
- `render_*` returns `comfy_table::Table` rather than `String`, unlike
  `render_tasks_view`. `Table: Display`, so the call sites and the tests read the
  same, and returning `Table` keeps the option of composing tables later without
  re-parsing text. The cost is that a caller could still mutate the table before
  printing.
- The regression test asserts on `contains("1 (50.0%)")` and
  `!contains("10.0%")` rather than pinning the whole table. Pinning the full
  render would break on every unrelated column change; the negative assertion is
  what actually catches the old denominator, and it was observed doing so.

### Open questions
None.

## Phase 5: `Stats --json` returns what `Stats` prints

### Design decisions
- `src/cli/commands/stats.rs`: `show_overall_stats` now fetches both payloads
  before branching, so `get_all_task_stats(Some(self.limit))` runs on the JSON
  path too. That single move is what makes `-n/--limit` reachable there: the
  flag was parsed and bound all along, but the JSON branch returned before the
  only call that consumed it.
- The new `OverallStatsJson<'a>` view struct holds `&'a OverallStats` under
  `#[serde(flatten)]` plus `tasks: &'a [TaskStats]`. Borrowed, not owned, so the
  render seam takes the same two values the table seams take and nothing is
  cloned to serialize it. `OverallStats` itself is untouched, per the doc: it is
  built by both store implementations, and a field there would force every one
  of them to populate it.
- `render_overall_json(&OverallStats, &[TaskStats]) -> Result<String>` sits
  beside `render_overall_table` / `render_task_stats_table` and follows the shape
  Phase 4 established: the seam returns the rendered payload, the command
  prints it. Tests assert on the returned string instead of capturing stdout.
- Verified on the built binary, not just in unit tests: the seven pre-existing
  keys come out in declaration order with `total_duration_seconds` still `0.0`,
  and `tasks` is appended eighth.

### Deviations
- The phase names `src/ports/db.rs:48` as `OverallStats`'s declaration. The
  struct is declared at `src/executor/state/manager.rs:82` and re-exported; the
  `db.rs` anchors are the trait method and the `MemoryStateStore` construction.
  Same instruction either way (do not add a field), same outcome.
- `docs/commands/stats.md` was regenerated wider than the phase's
  `115-129` bullet. The page states at line 7 that every block on it is observed
  output, and one run of the freshly built binary produced all of them, so the
  overall table's `Total Disk Usage` (20.9 KB / 21443), the per-task
  `project_hash` and `last_executed`, and the `Last Executed` timestamp were
  refreshed from that same run rather than left pointing at an older one. The
  `Usage:` block in the same page still read `Usage: Stats [OPTIONS] [TASK]`,
  which Phase 3's `bin_name` change made stale; regenerating it from
  `otto Stats --help` was one line and the alternative was shipping a page that
  claims observed output and is not.
- Two `-n/--limit` scope sentences (`stats.md:32`, the Notes bullet) said the
  flag affects the Top-N table only. That is no longer true, so both now say it
  caps the JSON `tasks` array as well. Not in the phase's bullets, but leaving
  them would have made the page contradict the change on the same page.

### Tradeoffs
- The key-order assertion in `overall_json_leaves_the_seven_existing_keys_unmoved`
  reads the order off the rendered text rather than off a parsed
  `serde_json::Map`. Without the `preserve_order` feature that map is sorted, so
  a parsed comparison would pass even if serde reordered the output. The text
  scan is uglier and is the only version that can fail for the right reason.
- The same test compares against `serde_json::to_string_pretty(&stats)` computed
  in the test rather than against a checked-in golden string. A golden would pin
  the exact bytes forever; recomputing pins the property the decision actually
  requires, that flattening changes nothing about how those seven pairs
  serialize.
- `test_execute_overall_stats_json` still calls `execute_with_store` (which
  prints to stdout and can only be asserted `is_ok`) and then re-derives the
  payload through the seam to assert content. Keeping the execute call costs a
  duplicate fetch in one test and keeps coverage on the branch that actually
  runs in production.

### Open questions
None.

## Phase 6: `--format` requires `--tasks`

### Design decisions
- `src/cli/parser/help.rs:33-40` (`global_args()`): added `.requires("tasks")`
  to the `format` `Arg`, exactly as the phase specifies. `global_args()` is
  the single shared arg list consumed by three builders: `Parser::otto_command()`
  (`help.rs:68-76`, the only one that actually parses via
  `try_get_matches_from`, at `parser.rs:858`) and the two help-only builders
  (`command.rs:194-196` `build_help_command`, `command.rs:245-248`
  `build_bare_help_command`), which only call `render_long_help()` /
  `print_help()` and never produce `ArgMatches`. `requires` is a parse-time
  constraint, so it is inert on the latter two and fires only on the real
  parse path, confirmed by `test_help_global_flags_no_drift`
  (`parser_tests_b.rs`) still passing unchanged: clap's default help renderer
  does not print `requires` relationships in the Options section, so the
  pinned snapshot did not need updating.
- Verified the failure mode is a clap `MissingRequiredArgument`, which falls
  through `Parser::parse()`'s `match e.kind()` (`parser.rs:891-931`) to the
  catch-all `_ => return Err(eyre!(e))` (`DisplayVersion` and `DisplayHelp` are
  the only two kinds special-cased there). That `Err` propagates out of
  `RuntimeConfig::from_parser` to `main`'s `Err(e) => { eprintln!(...);
  std::process::exit(1); }` (`main.rs:224-228`), which is the same exit-1 path
  the phase's own `--no-prefix` observation names, not a separate one this
  phase had to build.
- Confirmed `otto Graph --format dot` is unaffected because it never reaches
  `otto_command()`'s `format` arg at all: `Graph` has no early route
  (`EarlyCommand::from_name`, `main.rs:337-346`, deliberately excludes it) and
  is reached only through the task partitioning path, where `--format dot`
  after the task name is captured as part of the external subcommand's
  trailing args and bound by `Graph`'s own clap derive
  (`src/app.rs:119`), not parsed by the top-level `Arg::new("format")` at all.
  `otto --format json Graph` (global flag *before* the name) does route
  through the top-level parse and is now a usage error, matching the phase's
  stated and accepted breakage.
- Built the binary fresh (`cargo build --bin otto`, `target/debug/otto`,
  `otto v2.3.0-5-g25ae00c`) and ran AC5's exact scenario against it, since
  the installed `~/.cargo/bin/otto` on `$PATH` is the stale tagged `v2.3.0`:
  ```
  $ target/debug/otto --format yaml
  error: the following required arguments were not provided:
    --tasks
  Usage: otto --tasks --format <FORMAT>
  exit=1
  $ target/debug/otto --tasks --format yaml -o .otto.yml   # unchanged
  exit=0
  $ target/debug/otto Graph --format dot                    # unaffected
  digraph otto_dag { ... }
  exit=0
  ```
  Also ran the fixture-repo variant with `OTTO_SENTINEL` set on a task whose
  body would `touch` it: the sentinel file was absent after
  `otto --format yaml` exited 1, confirming no task ran, and confirmed
  `otto --format json Graph` (the undocumented shape) now also errors, as the
  Risks table predicts.
- Regression tests added to `tests/tasks_flag_test.rs`:
  `format_without_tasks_is_a_usage_error_and_runs_nothing` (reuses the file's
  existing `OTTO_SENTINEL` pattern from `tasks_executes_no_task_body` to
  assert the side effect, not just the exit code, plus a stderr check for
  `--tasks`) and `tasks_and_format_together_is_unchanged` (the paired form
  still exits 0 and still runs no task).

### Deviations
None. The phase named the exact `Arg` (`help.rs:33-38`, `.requires("tasks")`)
and that is what was added; no seam correction was needed.

### Tradeoffs
- The regression test asserts `stderr.contains("--tasks") || stderr.contains("tasks")`
  rather than pinning clap's full multi-line usage error text. Clap's exact
  wording for a `requires` violation is not part of this phase's contract (the
  phase's own success criterion only says "naming `--tasks`"); pinning the
  full string would break on an unrelated clap upgrade.

### Open questions
None.


## Phase 7: One implementation of "size of a directory"

### Design decisions
- The surviving implementation is `directory_size` in
  `src/executor/layout.rs`, a free function taking `&Path`, not a method on
  either caller. `layout.rs` is already the module that decides what a run root
  and a run directory are, is already a dependency of both `clean.rs` and
  `workspace.rs`, and its own module doc records the identical failure mode
  (four call sites each building the project directory name themselves, two of
  them wrong). A free function is also the cheapest seam for Phase 8's orphan
  sweep: no `self`, no `CleanCommand`, no `Workspace`.
- Symlinks are skipped by `entry.file_type()`, not by `is_dir()` or
  `metadata()`, and the doc comment says why: both of those follow links.
- `clean.rs:511` and `workspace.rs:426` now call it; both private copies are
  deleted, so no caller can drift again without deleting the shared one.

### Deviations
- The phase said the grep `fn calculate_dir` goes 2 -> 1. It goes 2 -> 0: the
  surviving function is named `directory_size`, which the criterion explicitly
  permits ("the surviving function may be renamed or moved"). The equivalent
  signal is `grep -rn 'fn directory_size' src/` == 1.
- AC8's fixture links to the 1 MB file **and** to the directory containing it.
  A link straight at a file was not sufficient to make the criterion
  load-bearing: `DirEntry::metadata()` does not traverse symlinks, so the old
  `Workspace` implementation counted such a link as its own 28-byte path string,
  not as 1 MB. Only its `entry_path.is_dir()` branch, which does follow, pulled
  in a target. Measured against the pre-change implementation: with the file
  link alone, Clean 102524 vs Workspace 102552 (28 bytes apart, and the 1 MB
  never counted); with both links, Clean 102524 vs Workspace 1151128. After the
  change both callers report 102524.

### Tradeoffs
- AC8 is asserted through the two production callers (`CleanCommand::scan_runs`
  and `Workspace::record_run_complete_in_db` via a `MemoryStateStore`) rather
  than by calling the shared function twice. The test is slower and needs a real
  temp `OTTO_HOME` plus `#[serial]`, but calling one function twice cannot fail,
  which is exactly the "renamed a function" failure mode the criterion exists to
  catch.
- `directory_size` still returns `Ok(0)` for a path that is not a directory,
  which is what both old implementations did. Making a missing run directory an
  error would turn `Clean`'s scan into an abort on a directory deleted between
  `read_dir` and the size walk.

### Open questions
None.

## Phase 8: `Clean` can see every run directory

### Design decisions
- **The lock is `std::fs::File::try_lock`, not a hand-rolled `libc::flock`.**
  `src/executor/runlock.rs`. Stable since Rust 1.89 (this tree builds on 1.98),
  implemented with `flock` on Unix, and it carries the two properties the design
  turns on: the lock lives on the open file description, so it releases when the
  `File` drops and on SIGKILL, and `OpenOptions` sets `FD_CLOEXEC`, so a task
  child does not inherit it. No new crate, no `unsafe`, and it works on the
  non-Unix branches the repo still compiles.
- **Phase 0's measurement was relied on as the doc says.** 1.85s over 7072 run
  directories is under the "few seconds" bar, so the sweep computes sizes during
  the scan for every directory rather than only for selected ones, and
  `--dry-run` reports no scanned count. One scan path, shared with `--no-db`.
- **The union is built from the scan and the rows are joined onto it by
  canonical path** (`clean.rs`, `CleanCommand::select`). Rows come from
  `get_runs_with_filters`, never `find_old_runs`, which applies retention itself.
- **Retention is applied twice, over two disjoint populations, exactly as the
  doc specifies.** Present unlocked directories get `{keep_days, keep_last,
  keep_failed}` once, over the union. Rows the scan did not account for get
  `{keep_days, keep_last: None, keep_failed}`.
- **A row whose directory is present but not in the union is skipped only when a
  run holds its lock.** The doc's rule was stated as "gone or never recorded";
  implemented as "not matched to a scanned directory", which also covers a
  directory the scan refuses (a symlink) or cannot parse. Those rows are still
  selected by age, so a run directory replaced by a symlink is still selected and
  still loudly refused by `ensure_deletable_under_root`, which is what
  `a_db_path_clean_with_one_refused_directory_exits_non_zero` (Phase 7's
  criterion) pins. A rule of "gone" alone would have made that test pass by
  never selecting the row at all.
- **Both `--dry-run` listings print the run directory last on every line.** The
  criterion is that the two modes select the same *set of directories*, and that
  is only checkable from the outside if the output names them.
- **`CleanCommand` gained an `#[arg(skip)] otto_home: Option<PathBuf>`, set by
  `auto_prune`.** See Deviations: without it the sweep is a live re-arming of the
  bug `pruning.rs` already documents, where a function called with one home
  pruned the one the environment named.

### Deviations
- **The lock is taken through the `FileSystem` port, not with `std::fs`
  directly.** The doc says `Workspace` takes the lock inside the
  `RUN_DIR_ATTEMPTS` loop and holds the `File` in a field. It does, but through
  `fs.lock_run_dir_sync(&candidate)`, because `Workspace` is generic over
  `FileSystem` and its `MemFs` tests build workspaces under paths like
  `/otto-home` that do not exist on disk. A direct `std::fs` lock aborts every
  one of those runs (acquisition failure is fatal, by design), so the seam has to
  be the port. `RealFs` locks for real; `MemFs` returns a handle that holds
  nothing, since those directories exist only inside the test process and `Clean`
  never sees them. Same effect, correct seam.
- **`CleanCommand` takes the otto home as a field.** Not in the design. The DB
  path now walks the filesystem, and `Clean` resolved the tree to walk from
  `$OTTO_HOME` while `auto_prune(otto_home, ...)` is *given* one. The comment at
  `pruning.rs:83` records the same defect in its database half: two unit tests
  passing a `TempDir` deleted rows from the developer's real `~/.otto/otto.db`.
  Left alone, the sweep would have been the directory version of that, and every
  `MemoryStateStore` clean test in `clean_tests.rs` would have swept the
  developer's real run directories on an ordinary `cargo test`. Those tests now
  pass a `TempDir` home explicitly.
- **`--keep-failed`'s statusless widening is applied per directory, not to the
  whole selection.** The doc says the sweep must reuse the filesystem path's
  widening. Applying that widening to the *union* would have thrown away the
  status the database does have and kept every successful run for the failed-run
  retention, breaking `test_clean_with_keep_failed_flag`. Instead a directory
  with no row is given `RunAge { failed: true }` when `keep_failed > keep_days`,
  which is how "take the longer of the two cutoffs" is expressed through the
  shared `Retention`. Row-backed directories keep their real status. Pinned by
  `keep_failed_widens_the_cutoff_only_for_directories_with_no_row`.
- **`read_run_with_project` became `read_run`** (`state/manager.rs`). With the
  derivation gone, `delete_run` no longer needs the project's name or hash, and
  keeping the two-query helper would have meant two unused bindings.
- **Three existing tests were inverted by name rather than deleted**, because
  each pinned behaviour this phase changes on purpose:
  - `test_execute_with_database_keep_last` ->
    `keep_last_does_not_hold_back_rows_with_no_run_directory`. Four rows with no
    directory and `--keep-last 2` used to leave two; `--keep-last` now applies
    only to the directory union, so all four go by age.
  - `delete_run_derives_the_directory_for_a_pre_v5_row` ->
    `delete_run_does_not_guess_a_directory_for_a_pre_v5_row` (F8's ruling).
  - `tests/concurrent_cold_start_test.rs`'s v4 fixture row moved from a fixed
    2023 timestamp to an hour ago. A v4 row has no `run_dir`, so each racer's
    auto-prune now deletes it on age, and the test would have been measuring
    retention instead of whether a concurrent schema upgrade loses rows.

### Tradeoffs
- **The scan tests the lock and releases it, then re-takes it per deletion.**
  Holding one descriptor per scanned directory would want 7072 open files on the
  author's machine. The doc's "hold the lock through `remove_dir_all`" is
  honoured where it matters (both delete loops, on both paths); the scan's test
  is advisory, and the window between them is closed by the delete's own
  try-lock.
- **A directory a live run holds is reported and skipped, and that is not a
  failure.** It joins "a row another pruner deleted first" outside the exit code,
  for the same reason: `auto_prune` runs at the end of every task, beside other
  runs, and failing there also skips the cache prune and the interval marker. A
  lock that cannot be *tested* is a refusal and is fatal, which is the NFS case
  the doc names.
- **`otto Clean --keep-days 0` now prints one skip line per live run.**
  Quiet-gated, so `auto_prune` says nothing, but a foreground `Clean` beside
  another run mentions a directory it was never going to delete. Honest and cheap
  against inventing a second quiet level.
- **A lock survives its own drop for as long as a concurrent `fork` in the same
  process still holds a copy of the descriptor.** Found as a one-in-many flake in
  `a_directory_a_run_still_holds_is_not_selected_by_either_mode` under the full
  suite, root-caused rather than retried blindly: a forked child that has not yet
  `exec`ed keeps the open file description alive, measured directly
  (`STILL LOCKED by the forked child's inherited descriptor` / `ACQUIRED once the
  child exited`). It delays a reclaim by microseconds and can never lose a live
  run's directory, so it is documented in `runlock.rs` and absorbed by a bounded
  retry in that test.

### Open questions
- **The two deletion paths are fenced against two different roots.** The sweep
  fences against the home `Clean` was given; `StateManager::delete_run` fences
  against `resolve_otto_home()`, which it reads from the environment itself. In
  production they are the same directory. In a test that passes a home without
  also pinning `$OTTO_HOME`, the row pass refuses every directory delete, which
  is why `the_default_path_removes_both_a_row_backed_run_and_a_directory_only_run`
  pins both. Threading a root through `StateStore::delete_run` was out of scope
  for this phase; worth deciding before something else takes a home as an
  argument.
- **Nothing was run against the live `~/.otto`.** The 1993 orphans and 388
  NULL-`run_dir` rows are still there. Reclaiming them is an operator action the
  doc's addendum leaves to the author, and note that once a build carrying this
  phase is installed, `auto_prune` will do it on the next ordinary run.

## Phase 9: The run record says what was asked for

### Design decisions
- **`RunPlan` gained a `requested_tasks: Vec<String>` field** (`src/cli/parser.rs`),
  populated from the same local (`tasks_to_run`) that `parse()` already builds
  from `resolve_default_tasks()` or `extract_task_names_from_partitions()`,
  before `process_tasks_with_filter` expands it into the resolved DAG. That
  local is exactly "what was literally named, or the default list if nothing
  was": no flag values ever reach it, since `extract_task_names_from_partitions`
  takes only `p[0]` (the task name) from each command-line partition, and
  `resolve_default_tasks` reads `otto.tasks:` from the ottofile, not argv.
  `RunPlan::into_parts()` was left as a 6-tuple; the many call sites that
  destructure it (tests, mostly) did not need this field, so it stays a named
  field read directly off the struct.
- **Threaded to both construction sites via `RuntimeConfig`**, not `into_parts`:
  `RuntimeConfig` gained the same field, and `run()` / `execute_tasks()` /
  `execute_with_terminal_output()` / `execute_with_tui()` (`src/app.rs`) each
  gained a `requested_tasks: Vec<String>` parameter, threaded straight through
  to both `ExecutionContext::new()` call sites (`app.rs`, one per output mode).
  A builtin (`Clean`, `Stats`, ...) returns via `find_builtin`/`dispatch_builtin`
  before either site is reached, so `requested_tasks` is never read for a
  builtin invocation; the field still has to be threaded through the shared
  functions on the ordinary-task path.
- **`ExecutionContext::record_requested(&self, requested_tasks: &[String])`**
  (`src/executor/workspace.rs`) sets `self.args = [self.prog] + requested_tasks`.
  One function, called from both `execute_with_terminal_output` and
  `execute_with_tui` right after `execution_context.hash = hash;`, so the two
  output-mode paths cannot drift on what gets recorded. `self.prog` is already
  `"otto"` from `ExecutionContext::new()`, so this does not hardcode a second
  copy of it.
- **Both stores get the fix for free.** `save_execution_context` (`workspace.rs`)
  already serializes the whole `ExecutionContext` to `run.yaml` and passes it to
  `record_run_start_in_db`, which forwards `context.args` into `RunMetadata` and
  from there into the `runs.args` column (`state/manager.rs`). Setting
  `execution_context.args` before that one call, rather than after, was the
  entire persistence-side change; neither store's write path needed touching.
- **Documented in `docs/commands/history.md`**: the `args` field description and
  its JSON example now say `["otto", "lint"]` / `["otto", "ci"]`, not "the full
  argv", and name both exclusions (flag values, the resolved closure).

### Deviations
None. The doc's propagation-path bullet (`ExecutionContext::new()` built
identically at `app.rs:422` and `:578`, `RunPlan` carries only the resolved
closure, thread the requested root names from the parser to both construction
sites) is exactly the wiring implemented; no seam correction was needed.

### Tradeoffs
- **A named struct field plus threaded parameters, not a thread-local or a
  second field on `Task`.** Widens four function signatures in `app.rs`
  (`#[allow(clippy::too_many_arguments)]` already covered two of them and now
  covers the other two). The alternative, stashing the requested names on the
  scheduler or reading them back out of `ExecutionContext` after the fact,
  would hide the data flow instead of making the parser-to-persistence path
  explicit, and this repo already threads `hash`/`ottofile`/`jobs` the same way
  through the same functions.
- **The negative assertion runs the real parse-to-persistence path against a
  temp `OTTO_DB_PATH`/`OTTO_HOME`**, not a unit test on `record_requested`
  alone. `record_requested`'s own logic is one line and cannot leak a value it
  is never given; what needed proving is that nothing *upstream* of it (the
  resolved task's own `envs`, which does carry the param value for the task
  body to use) leaks into `requested_tasks`, `run.yaml`, or the `runs.args`
  column. `tests/execution_context_integration_test.rs::test_execution_context_never_records_a_param_value`
  asserts the fixture's `deploy` task actually received
  `SHOULD-NOT-APPEAR` in its own `envs` (proving the fixture is load-bearing),
  then asserts that string is absent from `requested_tasks`, from
  `execution_context.args`, from the written `run.yaml`, and from the `runs.args`
  column read back through `StateManager::get_runs_with_filters`.

### Open questions
None.

## Finalization: acceptance criteria verified, and three doc defects amended

### Design decisions
- **All nine criteria were executed against a binary built from the finished
  branch** (`target/release/otto`, `otto v2.3.0-9-g5287f6f`), not against the
  stale `v2.3.0` on PATH, and not inferred from the phase reports. Results:

  | AC | Observed | Verdict |
  |---|---|---|
  | AC1 | both `Clean --dry-run --keep-days 30` modes select 1452 directories, `diff` of the two sorted path sets is empty | PASS |
  | AC2 | `grep -rn 'Tasks this one runs' docs/commands/` returns 0 lines | PASS |
  | AC3 | `Usage: otto Clean [OPTIONS]` | PASS |
  | AC4 | `Stats --json \| jq -e 'has("tasks")'` prints `true`, exit 0 | PASS |
  | AC5 | `otto --format yaml` exits 1, and Phase 6's sentinel fixture proves no task ran | PASS |
  | AC6 | `hide_env_values` count 0, `GITHUB_TOKEN` count 1 | PASS |
  | AC7 | `overall_success_rate_ignores_running_runs` and `per_task_table_shares_the_overall_denominator` both pass | PASS |
  | AC8 | `both_callers_report_one_size_that_excludes_a_symlink_target` passes | PASS |
  | AC9 | `History -n 1 --json \| jq -r '.[0].args \| join(" ")'` returns `otto lint` | PASS |

- **AC1 was verified at repo level, not only by fixture.** The phase report proved
  it on a three-run fixture; this run proves it on the live `~/.otto`, dry-run
  only. DB mode reports `0 rows from the database and 1452 orphaned directories`,
  `--no-db` reports `1452 runs`, and the extracted path sets are identical. The
  summed sizes agree here (134.7 MB both ways) because every selected directory in
  this tree is orphaned, so no row-recorded size participates.

### Deviations
- Three amendments to the design doc, each a defect in the doc rather than a
  failing implementation, each recorded in place with its evidence:
  1. **`OverallStats` was anchored at `src/ports/db.rs:48`.** It is declared at
     `src/executor/state/manager.rs:84`. `db.rs:338` is a construction site, not
     the declaration. Found by Phase 5, confirmed here with
     `grep -rn 'struct OverallStats' src/`. The instruction the anchor served
     ("do not add a field to this type") was correct and unchanged.
  2. **AC8's fixture, as written, could not fail.** A symlink pointing at the 1 MB
     *file* never exercised the divergence, because `DirEntry::metadata()` does not
     traverse links; only the `is_dir()` branch followed. Phase 7 measured 102524
     vs 102552 for the file-link shape against 102524 vs 1151128 for the
     directory-link shape. The criterion now requires both links.
  3. **AC8's secondary grep goes 2 -> 0, not 2 -> 1**, because the survivor is
     named `directory_size`. AC8's own closing paragraph permits the rename, so
     this is the criterion's arithmetic being stale, not the code evading it.
- Phase 0's stale tree figure (5090 directories) corrected to the 7072 measured,
  and its conditional recorded as resolved rather than left open.
- All nine criteria checkboxes flipped to checked, and Status set to Implemented.

### Tradeoffs
- Amended the doc rather than the code in all three cases above. The test each
  amendment had to pass: can the criterion be shown wrong independent of the
  implementation? For the anchor and the grep arithmetic, trivially. For AC8's
  fixture, the evidence is the pair of measured byte counts showing the file-link
  shape produced no divergence to detect in either direction.

### Open questions
- **Installing this build arms the reclaim.** `auto_prune` will delete the live
  orphaned run directories on the next ordinary `otto` run in any project. Nothing
  in this branch has touched `~/.otto`. The doc's addendum leaves running the
  reclaim to the author, so this is his call at the install step, not a
  consequence to discover afterward.
- **The two deletion paths fence against different roots** (Phase 8): the sweep
  against the home `Clean` was given, `StateManager::delete_run` against
  `resolve_otto_home()` read from the environment. Identical in production,
  divergent in a test that passes a home without pinning `$OTTO_HOME`. Threading a
  root through `StateStore::delete_run` was out of scope for Phase 8; worth
  deciding before something else takes a home as an argument.

## Post-audit: four cheap-wins from the implementation audit

The implementation audit (Mode 2, Architect + Staff Engineer, round 1) returned
zero must-fix and four cheap-wins, all documentation or disclosure. This entry
supersedes nothing; it records what the audit could see that the phases could not.

### Design decisions
- **Regenerated three help blocks from observed output instead of patching the one
  wrong line.** `docs/commands/{clean,history,upgrade}.md` each showed an
  unprefixed `Usage:` line that Phase 3 fixed in the binary but not on the page,
  because Phase 3 carried no docs bullet. Hand-patching the `Usage:` line alone
  would have left the rest of each block frozen at v2.3.0 while claiming to be
  observed output. All three blocks are now a verbatim capture from the installed
  `otto v2.4.0`.

### Deviations
- **Phase 2 changed the entire `otto Upgrade --help` layout, and its notes say
  "Deviations: None".** That entry was wrong, and this entry is the correction.
  Removing the five-line note at `upgrade.rs:334-338` removed the only
  multi-paragraph doc comment in the struct, which left clap with no `long_help`
  anywhere in it, so the command fell back from long help (a paragraph per option)
  to short help (a line per option), and `-h` and `--help` became byte-identical
  where they had differed at v2.3.0. All eight options survive and the
  `hide_env_values(true)` guard is verifiably still in force: zero leaks with
  `GITHUB_TOKEN` set.

  Worth naming because of *how* it escaped. Phase 2's two success criteria are
  both greps (`grep -c hide_env_values` -> 0, `grep -c GITHUB_TOKEN` -> 1). Both
  pass, and both kept passing while the page rendering changed underneath them.
  Neither review seat could see it from source either; it appears only when you
  diff two built binaries' output, which is what the audit did. This is the same
  lesson AC3 and AC7 were written for, arriving at a phase whose criteria were
  written as greps anyway.

- **`docs/commands/clean.md:199` claimed the database mode is faster because it
  reads metadata "instead of measuring every directory".** After Phase 8 the
  database mode always scans and sizes what it selects, so the line stated the
  opposite of the code. The bullet directly below it had been updated for Phase 8
  and this one was read past. The advantage the mode actually has is that a run is
  named by its row rather than by its directory name, and the line now says that.

- **Phase 8's lock has an upgrade window, now stated in the phase.** Only a build
  carrying the lock writes `.lock`, so a run in flight when that build is installed
  is unprotected. Measured across all three combinations. Not a regression: the
  pre-lock build had the identical defect on every run, and it closes itself once
  every in-flight run postdates the upgrade.

- **Open Questions said "None" while these notes carried one.** The divergent-root
  question is now repeated in the design doc's Open Questions section so the two
  documents agree.

### Tradeoffs
- Left two things the audit surfaced that are real but out of scope here, rather
  than growing this pass: a task param value is written plaintext to
  `~/.otto/<project>/.cache/<hash>.sh` (pre-existing in both versions, outside
  Phase 9's stated scope, though the doc's Security section reads broader than what
  was proved), and AC8's `Workspace` leg still rests on the in-tree fixture because
  the audit could not drive that caller standalone from outside the process.

### Open questions
- None beyond the divergent-root question now recorded in the design doc.
