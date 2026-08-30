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
