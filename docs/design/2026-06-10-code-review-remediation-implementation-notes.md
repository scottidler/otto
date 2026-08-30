# Implementation Notes: Code Review Remediation

Companion to `docs/design/2026-06-10-code-review-remediation.md`. Append-only,
one section per phase.

## Phase 0: Green gates and exposure removal

### Design decisions

- **`clean.rs`'s `execute_with_store` now checks `~/.otto` existence only on
  the filesystem-scan fallback path, not before the db-backed path** —
  `src/cli/commands/clean.rs:66-92`. The old code called `get_otto_home()` and
  early-returned "No ~/.otto directory found" before ever looking at an
  injected `StateStore`, so on a runner with no populated `~/.otto` (unlike a
  developer's machine) all 4 db-backed clean tests silently no-op'd instead of
  exercising the store. `StateManager::new` (`src/executor/state/db.rs:21-25`)
  already creates the directory itself via `create_dir_all` on first use, so
  the existence check was never load-bearing for the db path — it only makes
  sense for the plain filesystem scan, which really does need a directory to
  `read_dir`.
- **`clean.rs::get_otto_home()` now delegates to
  `crate::executor::pruning::resolve_otto_home()`** instead of
  reimplementing `$HOME/.otto` inline. That function already honors
  `OTTO_HOME` (the same override `workspace.rs`, `action.rs`, and
  `scheduler.rs` tests use for isolation, `#[serial]` + `std::env::set_var`),
  so `clean` picks up that convention for free and one less duplicate of the
  same 5 lines exists. Two new regression tests pin this:
  `test_execute_with_database_ignores_missing_otto_home` and
  `test_get_otto_home_honors_otto_home_env`
  (`src/cli/commands/clean.rs`).
- **`test_help_global_flags_no_drift`'s pinned string became a template.**
  `EXPECTED_GLOBAL_OPTIONS_HELP` was renamed
  `EXPECTED_GLOBAL_OPTIONS_HELP_TEMPLATE` with `{JOBS}` standing in for the
  `-j/--jobs` default (`src/cli/parser.rs:3986` area). A new
  `expected_global_options_help()` substitutes `DEFAULT_JOBS`
  (`num_cpus::get()`) at test time, so the assertion still catches real help
  drift everywhere except the one value that is legitimately
  machine-dependent. A second test,
  `test_expected_global_options_help_substitutes_actual_jobs_default`, pins
  the substitution itself so a future edit can't silently reintroduce a
  literal number.
- **CI now runs `otto quick` (compile + clippy + fmt-check + test) instead of
  three sequential `cargo` steps.** `.github/workflows/checks.yml` installs
  otto via `cargo install --path . --locked` and runs `otto quick`, which maps
  to `.otto.yml`'s `check: before: [compile, clippy, fmt-check]` plus `test`
  as independent siblings — verified locally that killing one sibling (e.g. a
  broken clippy lint) does not prevent `compile`, `test`, or `fmt-check` from
  running and reporting their own pass/fail, closing the "no `if: always()`"
  gap the doc named. This is also the literal "dogfood" the bullet asked for:
  the one clippy invocation (`--all-targets --all-features -- -D warnings`)
  now lives in exactly one place (`.otto.yml:22-25`) instead of drifting
  between `ci.yml` and `.otto.yml`.
- **`Release` gates on the shared `checks.yml` reusable workflow.**
  `.github/workflows/checks.yml` (`on: workflow_call`) is called as a
  `checks:` job from both `ci.yml` and `release-and-publish.yml`; `needs:
  checks` was added to `build-linux` and `build-macos` (not just
  `create-release`/`docker`, which already transitively depend on those two
  build jobs). `checks.yml` defines its own `RUST_VERSION` (caller `env:`
  does not propagate into a called reusable workflow) and its own
  `permissions: contents: read`; the caller job in `release-and-publish.yml`
  also sets `permissions: contents: read` explicitly rather than inheriting
  Release's `contents: write` / `packages: write`.
- **Tatari fixtures replaced with generically-named synthetic equivalents.**
  `makefiles/{auth-svc,devs,pre-commit-hooks,media-planning-service}/`
  (Makefile + otto.yml each) were deleted and replaced with
  `makefiles/{python-poetry-service,go-build-project,python-pre-commit,
  docker-compose-service}/`, each carrying the same converter-relevant shape
  (poetry/pytest/mypy, `$(shell find ...)` + `$(shell git describe ...)` Go
  build flags, `$(shell cat ~/.config/...)` nested shell substitution, Docker
  Compose + AWS S3 shell commands) but with every Tatari-specific name
  (service names, S3 bucket names, Python package names) replaced by generic
  `example-*` equivalents. `docs/list-of-all-makefiles` (6964 lines of
  absolute developer paths under `tatari-tv/`) was deleted outright with no
  replacement — nothing referenced it outside this design doc.
  `tests/makefile_converter_test.rs` and `tests/examples_integration_test.rs`
  were repointed and their test/assertion names updated to match.

### Deviations

- **CI runs `otto quick`, not `otto ci`, as the doc's prose literally says.**
  `otto ci` is `before: [lint, check, test]`, and `lint` shells out to
  `whitespace -r` — a personal CLI (`scottidler/whitespace`) with no
  published crate or GitHub release, so there is no portable way to install
  it on a GitHub-hosted runner. The pre-existing `ci.yml` never ran `lint`
  either (its three steps were `cargo test` / `cargo fmt --check` / `cargo
  clippy`), so running `otto quick` (`check` + `test`, no `lint`) preserves
  today's CI coverage exactly while fixing the two named bugs (drifted
  clippy invocation, non-independent steps) and satisfying the "dogfood
  `.otto.yml`" intent. `lint` stays a local/pre-commit-only check, unchanged
  from before this phase.
- **`ls makefiles/` success criterion verified more strongly than the bullet
  literally required.** The bullet's fix targets Makefile *content*
  (`cargo clippy` invocation, service Makefiles) but the doc's own success
  criteria line also requires "`ls makefiles/` contains no Tatari service
  name" — that requires renaming the *directories*, not just gutting their
  contents, since the directory names themselves (`auth-svc`,
  `media-planning-service`) are the real, disclosed service names. Both the
  Makefile and its companion `otto.yml` (which encoded Tatari-specific
  business logic: S3 bucket names, service names, poetry package names) were
  replaced, not just the Makefile.
- **Only 5 of the phase's `- [ ]` bullets needed marking; the first was
  already `[x] SHIPPED`.** No change to that bullet's text or status beyond
  re-verifying `cargo clippy --all-targets --all-features -- -D warnings`
  still finishes clean (confirmed via the `otto quick`/`otto ci` runs below).

### Tradeoffs

- **`checks.yml` installs otto via `cargo install --path . --locked` rather
  than reusing a prebuilt binary from the `build` job's matrix.** The `build`
  job's artifacts are release-profile binaries for the release matrix, not
  wired to pass artifacts to `checks`; `cargo install` is one extra compile
  but keeps `checks.yml` a fully self-contained reusable workflow with no
  cross-job artifact plumbing, which matters because it's called from two
  different workflows with different job graphs.
- **Synthetic fixture content is deliberately close to a straight rename of
  the original Makefiles**, not new content, so the converter continues to
  exercise the exact same feature surface (shell-var expansion, `$(shell
  ...)`, PHONY declarations, nested Docker Compose commands) it did before.
  Building genuinely new negative-case fixtures (missing `$(shell` handling,
  `$(VAR)` assertions) is explicitly Phase 7's job per the doc's own
  cross-reference ("coordinate with Phase 7's new negative fixtures") and is
  not attempted here.

### Open questions

- **The doc's success criterion "a tag push whose shared checks fail produces
  no uploaded artifacts, no GitHub release, and no GHCR image" was verified by
  inspection (the `needs: checks` dependency chain: `build-linux`/`build-macos`
  -> `create-release`/`docker`) and by local YAML validation (`yl`, PyYAML
  parse), not by an actual tag push — the doc explicitly forbids pushing a
  test tag to `otto-rs/otto`. Confirming this end-to-end requires either a
  scratch fork or `act`, per the doc's own "Testing this safely" note; that
  hasn't been done by this phase.
- **"The `CI` workflow concludes `success` on a runner" is unverified on an
  actual GitHub-hosted runner** — I don't have push access to trigger one from
  here. Locally, the equivalent commands (`otto ci`, `otto quick`,
  `cargo build --release`, `yl` on all three workflow files) all pass. The
  next push to `main` will be the first real confirmation; if it's red, the
  most likely culprit given what's testable from here is a runner-environment
  difference `otto quick`/`cargo install --path .` doesn't reproduce locally
  (e.g. `dtolnay/rust-toolchain`'s exact `cargo`/`~/.cargo/bin` PATH setup).

## Phase 0 follow-up: `OTTO_HOME` coupling (supersedes the Phase 0 "environment-coupled clean tests" entry above)

The Phase 0 fix was verified green and committed as `1f9a33f`, but it was not
green. The clean tests were still environment-coupled; only the variable
changed, from `HOME` to `OTTO_HOME`.

### Design decisions

- **Every spawned-binary test in `tests/cleanup_integration_test.rs` now pins
  `OTTO_HOME` explicitly** (9 call sites, each `cargo_bin_cmd!("otto")` chain
  gains `.env("OTTO_HOME", &otto_home)` alongside the existing
  `.env("HOME", home_dir)`). Each test already computed
  `otto_home = home_dir.join(".otto")`, which is exactly what `HOME=home_dir`
  was meant to resolve to, so this states the intended target directly rather
  than relying on a derivation the resolver can re-rank.

### Deviations

- None from the design doc. The doc's bullet sanctioned "env override or
  constructor param"; the env override stands, and this makes the tests
  declare their own value instead of inheriting the caller's.

### Tradeoffs

- **Pin `OTTO_HOME` per test vs. `.env_remove("OTTO_HOME")`.** Removing the
  var would also pass, by falling back through `HOME`. Pinning was chosen
  because it tests the resolution path the binary actually takes in production
  and cannot silently start depending on `HOME` again.
- **Fix the tests vs. re-rank `HOME` above `OTTO_HOME` in `resolve_otto_home`.**
  Re-ranking was rejected: `OTTO_HOME`-wins is the documented contract
  (`src/executor/pruning.rs:10-11`) and is what `workspace.rs`, `action.rs`,
  and `scheduler.rs` already assume. The defect was in the tests, not the
  resolver.

### Open questions

- None.

### Root cause, recorded because it is this document's own subject matter

`resolve_otto_home()` reads `OTTO_HOME` first and `HOME` second. The tests
spawn the real binary and set `HOME` on the child, but the child **inherits**
the parent process's `OTTO_HOME`, which then outranks the injected `HOME`, so
the temp directory was ignored. Reproduced against `1f9a33f`:
`env -u OTTO_HOME cargo test --test cleanup_integration_test` gave
`8 passed; 0 failed`, while `OTTO_HOME=/tmp/ottohome-probe` on the same commit
gave `4 passed; 4 failed`.

A GitHub-hosted runner has `OTTO_HOME` unset, so CI would have reported green
on a suite that is red for any developer who exports it. That is the same
green-on-the-runner / red-elsewhere split Phase 0 exists to close, inverted:
`test_help_global_flags_no_drift` was green locally and red on the runner;
this was green on the runner and red locally. Both were found only by running
the suite under more than one environment, which is now the standard for
calling a phase green in this plan.

**Verified after the fix**, both conditions, full pipeline, sandbox disabled so
`sccache` can run:

```
env -u OTTO_HOME otto ci            -> exit 0, [ci] ✅ All CI checks passed!
OTTO_HOME=/tmp/ottohome-final2 otto ci -> exit 0, [ci] ✅ All CI checks passed!
```

## Phase 1: Silent-success criticals in the parse/schedule core

### Design decisions

- **`Parser::parse` returns `ParseOutcome`, not a 6-tuple, and never ends the
  process** — `src/cli/parser.rs` (`ParseOutcome`, `RunPlan`), consumed by
  `RuntimeConfig::from_parser` (`src/app.rs`) which returns `Startup::Run` or
  `Startup::Exit(code)`, and by `main` (`src/main.rs`), which is now the only
  place in the binary that ends the process from a parse decision. All 12
  `std::process::exit` calls in `parser.rs` are gone;
  `git grep -c 'process::exit' src/cli/parser.rs` reports no matches. Paths
  that already print their own output (help, version, `--tasks`,
  `--list-subtasks`, the malformed-ottofile epilogue) keep printing it and
  return the code they used to exit with, so no stderr text moved.
- **Unknown args are detected before partitioning, not inside it** —
  `unconsumed_args()` + `unknown_task_error()` in `src/cli/parser.rs`, called
  from `parse()`. `partitions()` stays a pure split and its existing tests are
  untouched; the new function names the args `partitions()` would have dropped.
  Suggestions come from `nearest_task_name()`, which wires the previously
  unreferenced `levenshtein` dependency (edit distance <= 3 and strictly less
  than the candidate's length, ties broken alphabetically for a stable
  message). `otto buld` now says `unknown task 'buld'; did you mean 'build'?`.
- **`is_builtin()` replaces the stale lowercase `"graph"` filters** — three
  sites in `src/cli/parser.rs` (`parse_all_tasks` x2, `resolve_default_tasks`).
  `BUILTIN_COMMANDS` are capitalized, so the old filter never matched anything
  and `otto: tasks: ["*"]` expanded to include `Clean`, which is why a bare
  `otto` in a project defining only build/test printed
  `Querying database for old runs...` and exited 0.
- **The scheduler's completion channel carries `TaskReport { name, exit_code,
  error }`** — `src/executor/scheduler.rs`. Both bugs it closes are the same
  bug: the name used to be recovered with
  `error_str.split_whitespace().nth(1)` and the exit code by re-parsing the
  message and falling back to 1. A spawn failure ("No such file or directory
  (os error 2)") yielded the task name `such`, so the real task was never
  removed from the active set and the run hung; `exit 7` was recorded as 1.
- **Every task body has exactly one report site** —
  `TaskScheduler::execute_task`. The body became one fallible expression
  returning `Result<(), TaskFailure>`, so the semaphore acquire, the dependency
  double-check, `create_dir_all`, the symlinks and `ActionProcessor::new`/
  `process` all land at the same place instead of returning through a `?` that
  sent nothing. `TaskFailure` carries the observed process exit code so the
  database write is structural.
- **`ActiveTasks` owns a `JoinSet` and reaps it** —
  `src/executor/scheduler.rs`. `execute_all` selects between the report channel
  and `reap_unreported()`, which resolves only when a body ends *without*
  reporting; that case is logged and synthesized into a failure so a panicking
  task cannot hang the run. Remaining handles are joined at the end of
  `execute_all` rather than aborted by the `JoinSet` drop.
- **Cycle detection at scheduler init reuses the `otto Graph` construction** —
  `DagVisualizer::validate_acyclic` (`src/executor/graph.rs`), called from
  `TaskScheduler::new`. daggy decides *whether* there is a cycle;
  `find_cycle_path` supplies the readable path, because `WouldCycle` alone does
  not tell an operator what to edit. `a -> b -> a` now exits 1 with
  `dependency cycle detected: a -> b -> a`.
- **`execute_all` refuses to report success for a run in which nothing ran** —
  if `completed_set` and `failed_set` are both empty while `skipped_set` is
  not, it returns an error. Up-to-date skips are unaffected: those land in
  `completed_set`, so an entirely-cached run still exits 0.
- **Skip provenance reaches the run record** — `persist_skip_reasons()` at the
  end of `execute_all` gives `get_skip_reasons()` its first production caller
  and writes each skipped task through `record_task_skipped`, which gained a
  `skip_reason` parameter. Schema version 3 adds `tasks.skip_reason`.
- **`-j` is parsed as `u64` with `range(1..)`** — `global_args()` in
  `src/cli/parser.rs`, exactly as the doc specified. `-j 0` is now a clap usage
  error instead of a launch loop that spins at ~100% CPU forever.

### Deviations

- **The doc lists four dead `"graph"` filters (`:602, :631, :839, :870`); only
  three were filters.** `:870` was `task_names.push("graph")` in
  `get_task_names`, a partition-boundary entry, not a filter. It was equally
  dead (`otto graph` partitioned on a name no task has and then failed with
  `Task 'graph' not found`), so it was removed too; `otto graph` now produces
  `unknown task 'graph'; did you mean 'Graph'?`. Same intent, different
  mechanism than the bullet described.
- **`parse()`'s signature changed rather than gaining an out-parameter.** The
  doc says "propagate, exit only in main" without naming a shape. The 6-tuple
  became `RunPlan`; `RunPlan::into_parts()` exists so the ~25 existing call
  sites that want one or two fields did not all have to be rewritten. Same
  effect, correct seam.
- **The spawn error is wrapped with the task name.** Not in the bullet, but
  `Task pytask could not start python3: ...` is what makes the report readable
  once the name is no longer being scraped out of that same string.
- **`test_jobs_parameter_invalid` was inverted, not deleted.** It pinned the
  old silent fallback to `num_cpus::get()` for `-j invalid`. It is now
  `test_jobs_parameter_invalid_is_rejected` and asserts the rejection, with a
  comment saying why it flipped.
- **Two scheduler tests gained `setup_test_db`** —
  `test_file_dependencies_timestamp_precision` and
  `test_file_dependencies_empty_lists` built a `Workspace` without setting
  `OTTO_HOME`, so they inherited whatever the previously-run `#[serial]` test
  left it as. The MemFs workspace tests in `src/executor/workspace.rs` set it
  to `/otto-home` and never restore it, so under
  `OTTO_HOME=/tmp/otto-scratch-p1 otto ci` the first of them failed with
  `Failed to create directory /otto-home: Permission denied`. Pre-existing
  order-dependent coupling, surfaced by this phase's two new `#[serial]`
  scheduler tests changing the interleaving; fixed at the two coupled tests
  rather than by rewriting the MemFs suite.
- **This phase was implemented on `02522ea`, not the assigned base
  `6af8a51`.** HEAD had advanced by one docs-only commit
  (`docs: remove internal repo names from the public repo`) before work
  started.

### Tradeoffs

- **A v2-to-v3 migration inside Phase 1, vs. deferring skip persistence to
  Phase 4.** Phase 4 owns migration integrity and calls the current mechanism
  "non-idempotent with a crash window that permanently bricks the DB". Adding a
  step through it raises exposure, so the new step is written the way Phase 4
  will generalize: a `pragma_table_info` guard makes `migrate_v2_to_v3`
  idempotent, and the step plus its `set_version` run in one
  `unchecked_transaction`. Two tests pin it, including the
  crash-between-ALTER-and-bump case. The v1-to-v2 step is left exactly as it
  was; hardening it is Phase 4's bullet, not this one's.
- **Persisting skip reasons at the end of `execute_all` rather than inside
  `mark_skipped`.** Writing at skip time would order the row with the event but
  would leave `get_skip_reasons()` with no production caller, which is the
  literal defect the bullet names. The teardown write closes both.
- **`unconsumed_args` only inspects the args before the first task name.**
  Args after a task name belong to that task's clap parser, which now reports
  its own unknown flags (`try_get_matches_from` at the former
  `get_matches_from` site). Trying to second-guess them here would duplicate
  clap and get it wrong.
- **`-j 0`'s clap message reads `0 is not in 1..18446744073709551615`.** Ugly,
  but it is what `value_parser!(u64).range(1..)` renders, and the doc named
  that exact parser. Capping the upper bound at an invented number would be a
  new constraint nobody asked for.

### Open questions

- **`otto: tasks:` naming a built-in explicitly is still honored.** The
  wildcard `*` no longer expands to built-ins, but `otto: tasks: [Clean]`
  resolves, because the built-ins are injected into `tasks` before default
  resolution. That reads like an explicit operator choice rather than the
  accident the bullet described, so it was left alone. Confirm that is the
  wanted behavior.
- **`get_skip_reasons()` returns a `HashMap<String, String>` of free text.**
  Phase 2 introduces `SkipKind` as a typed provenance the scheduler can read.
  When it lands, the `skip_reason` column should probably carry the kind
  alongside the display text; this phase persists only the text.
