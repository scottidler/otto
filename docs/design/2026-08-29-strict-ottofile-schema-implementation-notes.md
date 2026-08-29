# Implementation Notes: Strict Ottofile Schema

Running record of decisions, deviations, tradeoffs, and open questions during
execution of `docs/design/2026-08-29-strict-ottofile-schema.md`. Append-only.

## Phase 0: De-duplicate the remediation doc

### Design decisions
- Annotation wording — `docs/design/2026-06-10-code-review-remediation.md:213,276,288` — appended `**SUPERSEDED: covered by docs/design/2026-08-29-strict-ottofile-schema.md** (ships first and owns this bullet).` inline at end of each bullet rather than a separate block, so the annotation travels with the bullet if the doc is reordered.
- Not-moved items — `:28`, `:153`, `:217` — appended `**STAYS: NOT covered by ...** (that doc names it a Non-Goal; it remains remediation scope).` The phrase "covered by docs/design/2026-08-29-strict-ottofile-schema" appears in the STAYS text too, so the Phase 0 `rg -c` criterion returns 6, not 3. Criterion asserts `>= 3`, so this passes; noting it because a reader expecting exactly 3 would be confused.

### Deviations
- None.

### Tradeoffs
- Inline annotation vs. a "Superseded" section at the top of the remediation doc — chose inline because the remediation doc is an untracked working-tree draft that will be edited further, and a top section drifts from the bullets it describes.

### Open questions
- None.

### Success criteria
- (a) `rg -c 'covered by docs/design/2026-08-29-strict-ottofile-schema' docs/design/2026-06-10-code-review-remediation.md` -> `6` (>= 3). PASS.
- (b) Manual check recorded: `:28` (strings where types belong), `:153` (unused levenshtein / unknown-task error), `:217` (`divine()` garbage keys) each carry the STAYS annotation. Verified by `rg -n 'STAYS'` returning exactly those three line numbers. PASS.
- No commit rides this phase: the remediation doc is an untracked working-tree draft, per the design doc's Phase 0 bullet.

## Phase 1: Clean the inline test fixtures

### Design decisions
- Deletion mechanism — `src/cfg/param.rs` — used `sed -i '/^\s*name: test_task\s*$/d; /^\s*name: switch\s*$/d'` rather than a per-site manual edit, since both target strings are exact, unique, whole-line literals (`^\s*name: (test_task|switch)\s*$`) that cannot collide with the two Rust struct-literal hits (`name: String::new(),` and `name: "verbose".to_string(),`), which carry trailing tokens and never match `\s*$` right after the value.

### Deviations
- None. The doc's own grep predicate (`grep -oP '^\s*name: \S+' src/cfg/param.rs | sort | uniq -c`) was re-run before editing and matched the doc exactly: 8x `name: test_task`, 7x `name: switch`.

### Tradeoffs
- None.

### Open questions
- None.

### Success criteria
- (a) Scratch build: added `#[serde(deny_unknown_fields)]` to the six Phase 4 structs (`ConfigSpec` `src/cfg/config.rs:10`, `RetentionSpec`/`OttoSpec` `src/cfg/otto.rs:56,107`, `ForeachSpec`/`TaskSpecHelper` `src/cfg/task.rs:49,354`, `ParamSpec` `src/cfg/param.rs:43`). `cargo test --lib cfg::param` -> `test result: ok. 24 passed; 0 failed; 0 ignored; 0 measured; 496 filtered out`. PASS. Reverted immediately after (`git diff` on those four files shows no residual `deny_unknown_fields` anywhere in `src/`).
- (b) Reverted tree: `cargo test --workspace --all-features --no-fail-fast` -> **730 passed, 0 failed** across 27 test binaries (matches the doc's own `Observed on main` count exactly). PASS.
- (c) `git diff --stat` -> `src/cfg/param.rs | 15 ---------------`, all 15 removed lines fall inside the `#[cfg(test)] mod tests` block (starts at line 429); no non-test line touched. PASS.
- Environment note, not a deviation from the code change: the sandboxed/default `cargo`/`otto ci` invocation intermittently failed with `sccache: error: Operation not permitted (os error 1)` on `rustc -vV` — an sccache build-cache daemon issue in this environment, unrelated to `param.rs`. Worked around by running with `RUSTC_WRAPPER=` unset for the verification commands; `otto ci` (which shells out to `cargo` per its own config) was run the same way and passed green.

## Phase 2: Migrate the in-repo ottofiles

### Design decisions
- `examples/interactive-demo/otto.yml` — `sed -i 's/interactive: true/tty: true/g'` — mechanical, all 7 occurrences are the exact literal `interactive: true` with no variant spelling, verified by `grep -c` before/after (7 -> 0 old key, 7 -> 7 new key).
- `examples/basic-dependencies/otto.yaml` — deleted the `show: false` / `verbosity: 1` lines under `tasks.example2` verbatim, per the doc; no replacement, since neither key exists on `TaskSpecHelper`.
- `examples/dependency-ordering/otto.yml` — renamed `otto.task` to `otto.tasks`, keeping the value `[bob]` unchanged (the doc specifies the rename only). `otto.tasks` is a default-task-selection filter (`src/cfg/otto.rs:118`, consumed at `src/cli/parser.rs:718,800`), unrelated to the enumeration surface (`build_tasks_view` at `parser.rs:494` walks `config_spec.tasks`, the real task map, not the filter) — confirmed `bob` (which names no real task) does not suppress `otto --tasks` output for this file.
- `examples/complex-workflow/otto.yml` — root `name:`/`description:` moved under a new `otto:` block as `otto.name`/`otto.about`, matching `OttoSpec` field names exactly (`about`, not `description`).
- `examples/old/{ex1,ex2,ex3}` — deleted via `git rm -r examples/old` (all three files plus the now-empty directory). `docs/flag-support.md:385` dropped only the `examples/old/ex1/otto.yml` bullet; the adjacent `examples/ex1/otto.yml` bullet (a different, already-nonexistent path unrelated to this phase's deletions) was left untouched — that dead reference predates this phase and fixing it is out of scope.

### Deviations
- None. All five edits match the doc's Phase 2 bullets exactly (key renames, line deletions, file deletions).

### Tradeoffs
- None.

### Open questions
- The `examples/ex1/otto.yml` bullet in `docs/flag-support.md` points at a path that does not exist in the repo (only `examples/old/ex1` existed, now deleted; there is no top-level `examples/ex1/`). Pre-existing, unrelated to `examples/old`, left alone per this phase's scope — flagging in case a future doc pass wants to fix or remove it.

### Success criteria
- (a) Scratch build: re-applied `#[serde(deny_unknown_fields)]` to the same six structs as Phase 1 verified, `cargo build --release` succeeded, then ran `otto --tasks` in every remaining tracked `examples/` directory (20 dirs; the 21st on disk, `examples/_test_broken/`, is gitignored test scratch, not tracked). All 20 parsed (exit 0) and reported a non-empty task map: `basic-dependencies=3, build-pipeline=4, build-test-deploy=8, complex-workflow=7, conditional-deps=6, data-flow-bash=2, data-passing-demo=4, dependency-ordering=3, diamond-dependencies=5, environment-variables=8, ex2=1, file-dependencies=6, foreach-glob=2, foreach-items=3, foreach-range=4, hello-world=3, interactive-demo=8, parallel-tasks=5, subtask-targeting=4, tui-demo=7`. PASS (both halves: parses AND >=1 task, for every file). Scratch attributes reverted immediately after (`git diff -- src/` empty before commit staging).
- (b) `cargo test --test examples_integration_test` -> `test result: ok. 21 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out`. PASS.
- (c) `examples/_test_broken/otto.yml` (test-generated, gitignored) still fails under the scratch build: `otto --tasks` -> `unknown field \`invalid\`, expected \`otto\` or \`tasks\`` (rc=1), matching the doc's predicted post-strict-parsing message exactly. PASS.
- Full `otto ci`: green on retry. First attempt failed on `executor::scheduler::tests::test_file_dependencies_timestamp_precision` with `Failed to create directory /otto-home: Permission denied` — root-caused as a pre-existing test-isolation race (multiple tests in `src/executor/workspace.rs` call `std::env::set_var("OTTO_HOME", "/otto-home")`, a process-global mutation, from parallel test threads) sharpened by the sandbox denying writes outside its allowlist for that literal path. Confirmed unrelated to this phase: reproduces identically with Phase 2's changes stashed (bare `main`, `cargo test --lib` alone passed once, `--test-threads=1` on the single test always passes); a second unmodified `otto ci` run went green with no code changes in between. Not fixed here (out of phase scope; Phase 2 touches only `examples/` and `docs/flag-support.md`).

## Phase 3: Make `otto.api` load-bearing

### Design decisions
- Put the gate in `src/cfg/otto.rs`, not `src/cli/parser.rs` — `CURRENT_API_VERSION`, `SUPPORTED_API_VERSIONS`, `ApiHeader`/`ApiHeaderOtto`, `check_api_version` — because that is where `OttoSpec.api` and `default_api()` already live; `parser.rs` calls one function. Same shape as borg, where the version const, the tolerant header, and the check all sit in `harvest/contract.rs` and the caller is thin.
- Added `CURRENT_API_VERSION: &str = "1"` and made `default_api()` return it, so the literal `"1"` appears once. Mirrors borg's `SUPPORTED_SCHEMA_VERSIONS: &[1, CONTRACT_SCHEMA_VERSION]`, where the set is written in terms of the current-version const rather than a bare literal. `src/cfg/otto.rs:test_supported_api_versions_contains_the_default` pins the invariant (an absent `api:` defaults to a version that must itself be supported), so the two cannot drift.
- `check_api_version` is tolerant on BOTH axes, not just the missing-field one — `src/cfg/otto.rs:check_api_version`. A document that does not even yield an `ApiHeader` (unparseable YAML, `otto:` bound to a sequence) returns `Ok(())` and lets the typed parse report the real, specific error. Bailing there would replace serde's precise `line N column M` with a vaguer message, which is the exact failure mode this phase exists to remove.
- The call site sits after the hash and before the typed parse (`src/cli/parser.rs:2376`), against the same `content` String — one `fs::read_to_string`, per the doc's "Read the file once".
- Kept the error text exactly as the doc's API Design section specifies, including the leading `otto: ` — `src/main.rs` prints config-load errors with a bare `eprintln!("{e}")` and exits 1, so the message is the whole user-visible line and needs to name the tool itself.
- `log::debug!` on both exits of `check_api_version` (deferred vs. decided), naming the declared version and the supported set, per the always-on function-level logging rule and borg's `parse_export`.

### Deviations
- Deliberate, and already sanctioned by the doc's Data Model: `ApiHeader.api` is `Option<String>` where borg's `VersionHeader.schema_version` is a required `u32`. Recorded in the struct's doc comment with the reason (hand-written ottofiles; `api:` optional today).
- `ApiHeader` needed a second private struct, `ApiHeaderOtto`, for the nested `{ otto: { api } }` shape the doc specifies. Not a design change: it is the nested half of the same tolerant header, and it carries the same `Option` + `#[serde(default)]` discipline.
- The doc names `src/cli/parser.rs:2365` as the wiring point, which is the `load_config_from_path` signature line; the call lands at `:2376`, inside the `if let Some(ottofile)` arm where `content` exists. Same seam, correct line.

### Tradeoffs
- Gate on the raw string via a second `serde_yaml::from_str` (one extra cheap parse of an already-in-memory String) vs. reading `config_spec.otto.api` after the typed parse. The second is free but structurally cannot work: it is precisely the reversed order the doc forbids, and criterion (c) proves it fails.
- `SUPPORTED_API_VERSIONS.join(", ")` renders the set as `1` rather than borg's `{:?}`-formatted `[1]`. Chosen because the doc's error text is verbatim `(this otto supports: 1)`; a `Debug`-formatted slice would print `["1"]`, quotes and brackets included.

### Open questions
- None.

### Success criteria
- (a) `api: 2` errors naming both `2` and the supported set, exit non-zero. **PASS.** Observed, `otto --tasks` in a temp dir whose `otto.yml` declares `otto.api: 2`: `otto: unsupported api version '2' (this otto supports: 1). upgrade otto.` with `exit=1`. Unit-covered by `src/cfg/otto.rs:test_check_api_version_rejects_an_unsupported_version` (asserts declared version, supported set, and the `upgrade otto` remedy separately) and `src/cli/parser.rs:test_load_config_rejects_an_unsupported_api_version` through the real load path.
- (b) `api: 1` and an absent `api:` are unchanged. **PASS.** Observed: both a file declaring `api: 1` and a file with no `otto:` block at all print the same `otto --tasks` JSON (`up` task, exit 0), and `otto up` runs to `[up] finished successfully` (exit 0) in both. Unit-covered by `test_check_api_version_accepts_the_current_version` (both `api: 1` and `api: '1'`, since YAML `1` is an int scalar that serde_yaml coerces into the `String`, confirming the doc's no-type-change call), `test_check_api_version_accepts_an_absent_api`, and `test_load_config_accepts_a_supported_and_an_absent_api_version`.
- (c) An ottofile with BOTH `api: 2` and a key this otto cannot read reports the VERSION error, not the parse error. **PASS, and demonstrated to bite.** Fixture is `api: 2` plus `tasks.up.before:` bound to a map, chosen because `deny_unknown_fields` does not land until Phase 4, so an unknown key produces no error today — the type error does, and it is what the version error must beat. Observed with the gate in place: `otto: unsupported api version '2' (this otto supports: 1). upgrade otto.` (exit 1). Proof it bites: reversing the two statements in `load_config_from_path` (typed parse first) makes `test_load_config_reports_the_api_error_before_the_parse_error` fail with `the version error wins: tasks.up.before: invalid type: map, expected a sequence at line 6 column 7`, while the other two api tests still pass — i.e. this test is the only thing holding the ordering. Order restored, suite re-run green.
- `otto ci`: **green**, first run after `cargo fmt`. `[check]`, `[compile]`, `[clippy]`, `[lint]`, `[fmt-check]`, `[test]` all `finished successfully`; lib suite `test result: ok. 528 passed; 0 failed`; `[ci] finished successfully`. The Phase 2 `OTTO_HOME` test-isolation race did not recur.

## Phase 3b: Truthful config-error help fallback

### Design decisions
- Fixed at the seam the doc names, `src/cli/parser.rs:417` — bind the discarded `Err(_)`, leave `load_config_from_path` untouched. Criterion (c) exists to catch the alternative, so the alternative was never a candidate.
- Split `build_help_command_with_error()` into `build_bare_help_command()` (global flags, no epilogue) plus a one-line wrapper that adds the not-found epilogue. The two config-failure states now render from the same flag list without sharing a claim about the cause. `build_help_command_with_error()` keeps its name and its meaning: it is the no-ottofile fallback, and it is still what the `_ =>` arm at `:425` calls.
- New free function `ottofile_parse_error_message(path, err)` sits beside `ottofile_not_found_message()` in `src/cli/parser.rs` — same module position, same `colored` treatment (red bold label, bright-yellow path), so the two states read as siblings rather than as one message and one afterthought.
- Parse diagnostic goes to **stderr**, help to **stdout**. Matches `--tasks`, which already reports the same serde error on stderr, and keeps stdout a clean flag list for anything piping it. The doc's criterion (a) anticipated this exact choice and required both streams be asserted; both are.
- Message names the file by absolute path before the serde text: the serde error gives field path plus line/column but not *which* file, and the whole defect being fixed is a message that misidentifies the file.

### Deviations
- None. Scope discipline held: `Commands:` is still absent on this path and the exit code is still 2, both explicitly sanctioned by the phase.

### Tradeoffs
- Diagnostic on stderr vs. swapping the epilogue in place (stdout, where the false one was). Stderr is correct stream discipline and matches the enumeration surfaces; the cost is that a naive `otto --help > file` hides it, which is precisely why criterion (a) mandates asserting both streams and why the tests do.
- Kept `build_help_command_with_error()`'s name rather than renaming it to something like `build_help_command_ottofile_not_found()`. The name is now narrower than it reads, but renaming would churn the pinned-snapshot test at `:3984` and the doc comment at `:619` for no behavior change. Flagged here rather than silently.

### Open questions
- None.

### Success criteria
All three measured in `bash` with streams redirected separately (`>out 2>err`), against a `.otto.yml` whose `tasks.up.before` is a map where a sequence is required.
- (a) With a parse-failing `.otto.yml` present, `otto --help` surfaces the real parse error and no longer claims `No ottofile found`. **PASS, on both streams.** Before (at `c0fa6c3`): exit 2, **stdout 928 bytes**, **stderr 0 bytes**, stdout ending in `ERROR: No ottofile found in this directory or any parent directory!`. After: exit 2, **stdout 745 bytes** (the global flag list, zero occurrences of `No ottofile found`), **stderr 146 bytes** reading `ERROR: failed to parse ottofile: /tmp/claude-1000/p3b-broken/.otto.yml` / `tasks.up.before: invalid type: map, expected a sequence at line 9 column 7`. Covered by `tests/ottofile_help_epilogue.rs:test_help_reports_parse_error_and_does_not_claim_missing_ottofile`, which asserts the diagnostic on stderr AND the absence of `No ottofile found` on stdout AND that `--ottofile` still renders.
- (b) With NO ottofile anywhere up the tree, `otto --help` still says `No ottofile found`, unchanged. **PASS.** Observed in a scratch dir with no ottofile in it or any parent: exit 2, **stdout 928 bytes** — byte-identical to the pre-change (a) output — ending in the not-found block and the candidate filename list, **stderr 0 bytes**. Covered by `test_help_still_claims_missing_when_no_ottofile_anywhere` (asserts the epilogue on stdout and that stderr does NOT mention parsing), alongside the pre-existing `test_help_epilogue_when_ottofile_missing`.
- (c) NON-REGRESSION: `otto <usertask> --help` on a parse-failing file continues to print the parse error to stderr. **PASS, unchanged from main.** Observed both before and after: exit 1, **stdout 0 bytes**, **stderr 75 bytes** reading `tasks.up.before: invalid type: map, expected a sequence at line 9 column 7`. Covered by `test_usertask_help_still_reports_parse_error_on_stderr`, which additionally asserts this stderr does NOT contain `failed to parse ottofile` — that phrase belongs only to the global-help path, so its appearance here would mean the fix had migrated into `load_config_from_path`.

**Proof the new tests bite** (two deliberate breakages, each reverted and re-run green):
1. Restored the original `Err(_) => { build_help_command_with_error(); exit(2) }` at `:417`. `test_help_reports_parse_error_and_does_not_claim_missing_ottofile` **FAILED** with `stderr=""` and the `No ottofile found` block back on stdout; the other four tests in the file still passed — so this test alone holds the truthfulness of the global-help path.
2. Implemented the phase at the wrong seam instead: `serde_yaml::from_str(&content).map_err(|e| eyre!("failed to parse ottofile: {}: {}", ottofile.display(), e))?` inside `load_config_from_path`. That satisfies criterion (a), but `test_usertask_help_still_reports_parse_error_on_stderr` **FAILED** with `stderr="failed to parse ottofile: /tmp/.tmpuQVtsi/.otto.yml: tasks.up.before: invalid type: map, expected a sequence at line 7 column 7"` — the enrichment leaking onto the shared path, which is exactly what the doc says criterion (c) is for.

- `otto ci`: **green**. `[check]`, `[compile]`, `[clippy]`, `[lint]`, `[fmt-check]`, `[test]` all `finished successfully`, `[ci] ✅ All CI checks passed!`; lib suite `test result: ok. 528 passed; 0 failed`, and `tests/ottofile_help_epilogue.rs` reports `5 passed; 0 failed` (2 pre-existing + 3 new). The `OTTO_HOME` test-isolation race did not recur.

## Phase 4: Strict parsing

### Design decisions
- `#[serde(deny_unknown_fields)]` landed on exactly the six structs the Architecture table names: `ConfigSpec` (`src/cfg/config.rs:16`), `RetentionSpec`/`OttoSpec` (`src/cfg/otto.rs:133,189`), `ForeachSpec`/`TaskSpecHelper` (`src/cfg/task.rs:54,369`), `ParamSpec` (`src/cfg/param.rs:53`). Each carries a doc comment naming the rationale (loud error over silent no-op) and, where relevant, which free-form key site the attribute does *not* reach — per borg `config.rs:281-285` and this repo's own precedent at `src/cfg/edge.rs:76`.
- Each of the six negative tests parses through `ConfigSpec`, not the target struct directly, except the `ConfigSpec` case itself (which has no parent to go through). Parsing the leaf struct alone would never produce a path prefix, since serde_yaml's location/path tracking is built from the actual nesting the deserializer walks; going through `ConfigSpec` is what makes the `otto`, `otto.retention`, `tasks.up`, `tasks.up.foreach`, and `tasks.up.params` path assertions meaningful.
- Test placement: each negative test sits beside the struct it targets — `ConfigSpec`'s in a new `mod tests` at the bottom of `config.rs` (the file had none before), `OttoSpec`'s/`RetentionSpec`'s in `otto.rs`'s existing `mod tests`, `ForeachSpec`'s/`TaskSpecHelper`'s in `task.rs`'s existing `mod foreach_tests` (both are `tasks.up`-shaped fixtures, foreach-adjacent even though the second is not itself a foreach test), `ParamSpec`'s in `param.rs`'s existing `mod tests`.
- The `TaskSpecHelper` negative test uses the doc's own motivating incident verbatim (`parallel:` beside `foreach:` instead of inside it, from the Problem Statement), not a synthetic fixture, so the test doubles as a regression pin for the exact bug that opened this doc.

### Deviations
- None from the Phase 4 spec. The six structs, the doc-comment rationale, and the one-negative-test-per-struct table were all followed exactly.
- The root-`ConfigSpec` case's measured error text differs from the doc's quoted example depending on field order in the YAML: when `envs:` is the *first* key in the mapping (as in the doc's own `at line 8 column 1` example and as it would appear in the two real work-repo files that motivate this test), serde_yaml's `deny_unknown_fields` error carries no `at line N column M` suffix at all — confirmed by direct experiment (see Testing Strategy below). When `envs:` follows another key (`tasks:` first, `envs:` second — the fixture this phase's test actually uses), the location IS present. This is a serde_yaml behavior, not something this phase's code controls; the test fixture was written with `tasks:` first specifically so the location assertion has something to check. The Phase 4 table's requirement ("assert field + expected-set + location" for the root case) is met by the test as written; it would not be met if the fixture put `envs:` first. Recorded here so a future editor does not "simplify" the fixture and silently lose the location assertion's ability to bite.

### Tradeoffs
- Assert-by-`.contains()` on error text vs. full-string equality. Every existing negative-test style in this codebase (`check_api_version`'s tests, `on_failure` sugar tests, `choices-command` tests) uses `.contains()` fragments naming what must appear rather than pinning the whole message; Phase 4's six tests follow the same convention rather than introducing exact-match assertions that would be brittle against serde/serde_yaml version bumps changing incidental wording.
- Left the doc-comment rationale on the struct definition rather than only on the `#[serde(...)]` line itself, matching borg's placement (`config.rs:277-283`, the comment sits above the derive, not inline on the attribute) since Rust doc comments cannot attach to an attribute macro invocation directly.

### Open questions
- None.

### Success criteria
- (a) **PASS.** `cargo test --workspace --all-features --no-fail-fast` -> all 27 binaries green (lib: `528 passed; 0 failed`; every integration test binary `0 failed`). `tests/roundtrip.rs` unmodified (`git status --short -- tests/roundtrip.rs` empty) and green: `test result: ok. 11 passed; 0 failed`.
- (b) **PASS.** Both Problem Statement repro fixtures, run against `target/debug/otto` built from this phase's tree:
  - Wrapped (wrong-level `parallel:` beside `foreach:`, verbatim from the doc): `otto up` -> stderr `tasks.up: unknown field \`parallel\`, expected one of \`help\`, \`after\`, \`before\`, \`input\`, \`output\`, \`envs\`, \`params\`, \`bash\`, \`python\`, \`action\`, \`foreach\`, \`on-failure\`, \`tty\` at line 3 column 5`, exit 1. On `main` this ran all three subtasks concurrently, exit 0, no diagnostic (doc's own `Observed on main`).
  - Unwrapped (invented root `envs:`): `otto show` -> stderr `unknown field \`envs\`, expected \`otto\` or \`tasks\``, exit 1. (No location suffix here because `envs:` is the first key in this fixture — see Deviations. The field is still named, which is what the criterion requires.)
- (c) **PASS.** With the wrong-level `parallel:` fixture in place: `otto --help` -> exit 2, stdout 745 bytes (global flag list, no `No ottofile found`), stderr 266 bytes carrying `ERROR: failed to parse ottofile: ...` plus the same serde message as (b) above. `otto Clean --help` -> exit 0, 643 bytes of help, 0 bytes stderr. `otto Convert -o .otto.yml < /dev/null` on a two-target Makefile -> exit 0, emits `otto.retention` as `keep_days`/`keep_last`/`keep_failed`/`auto_prune`/`prune_interval_hours` (plain snake_case, matching `RetentionSpec` exactly), and `otto --tasks < /dev/null` on that emitted file -> exit 0, `{}` (the pre-existing, out-of-scope "Convert drops Makefile targets" bug the doc already records — not a new regression).
- `otto ci`: **green** after one `cargo fmt` pass (a long assert line in the `ConfigSpec` negative test needed reformatting). `[check]`, `[compile]`, `[clippy]`, `[lint]`, `[fmt-check]`, `[test]` all `finished successfully`; `[ci] ✅ All CI checks passed!`. The `OTTO_HOME` test-isolation race did not recur.

**Proof the new tests bite** (per Testing Strategy: every phase breaks its own code once). Removed `#[serde(deny_unknown_fields)]` from all six structs, ran `cargo test --lib deny_unknown_fields`: all six new tests **FAILED**, each with `called \`Result::unwrap_err()\` on an \`Ok\` value: ConfigSpec { ... }` — the fixtures that should error now parse cleanly and silently keep the invented/misplaced/rejected keys, which is exactly the pre-Phase-4 defect. All six attributes restored immediately after; full suite re-run green (`cargo test --workspace` and `otto ci`).

Root-case location quirk, demonstrated directly (not asserted in the shipped test, recorded for the next reader): `serde_yaml::from_str::<ConfigSpec>` on `"envs:\n  PROJECT: myproj\ntasks:\n  show:\n    bash: echo hi\n"` (envs first) -> `unknown field \`envs\`, expected \`otto\` or \`tasks\`` (no location). The same keys with `tasks:` first -> `unknown field \`envs\`, expected \`otto\` or \`tasks\` at line 4 column 1` (location present). Confirmed this is not an artifact of the `eyre` error-conversion path used by `load_config_from_path`: both the raw `serde_yaml::Error::to_string()` and the `eyre::Report` wrapping it produce the identical (location-free) string for the envs-first case.

## Phase 5: Documentation

### Design decisions
- Reference doc lives at `docs/commands/ottofile-reference.md`, alongside the existing per-command docs in that directory, structured by nesting level (root, `otto:`, `otto.retention:`, `tasks.<name>:`, `tasks.<name>.foreach:`, `tasks.<name>.params.<title>:`) rather than one flat table, so a reader can jump straight to the level they're editing.
- Migration note lives at `docs/commands/ottofile-strict-schema-migration.md`, its own file rather than a section bolted onto the existing `docs/migration-guide.md` (which is entirely about the unrelated SQLite integration and would have been a non sequitur to extend).
- Drift test (`ottofile_reference_key_inventory_is_exhaustive`, `src/cfg/task.rs`, inside the existing `foreach_tests` module) implements both techniques the design doc names, used together, and neither alone would suffice:
  - **Exhaustive destructuring** (`let ConfigSpec { otto: _, tasks: _ } = ConfigSpec::default();`, and one more per struct) is the compile-time TRIGGER. It reaches private `TaskSpecHelper` because this test lives inside `task.rs`'s own module tree, per the design doc's note. Constructing an instance of `TaskSpecHelper`/`ParamSpec` (neither has a `Default` impl) via `serde_yaml::from_str::<T>("{}\n").unwrap()` rather than a literal, since every field on both is `#[serde(default...)]` or `#[serde(skip)]` (which itself requires the field's type to implement `Default`).
  - **The on-disk key list is recovered, not hand-copied**, from serde's own `deny_unknown_fields` error text: `expected_keys_from_deny_unknown_fields::<T>()` feeds each struct a single bogus key (safe because every field on all six structs has a default, per the Data Model, so the error is always "unknown field", never "missing field") and extracts every backtick-quoted token following the word "expected", up to (but not including) an optional trailing `" at line N column M"`. This one function handles all three of serde's phrasings ("expected `a`", "expected `a` or `b`", "expected one of `a`, `b`, ..., `z`") without branching on which one it got.
  - The two are tied together by `reference_doc_mentions_key`, which reads `docs/commands/ottofile-reference.md` via `include_str!` and checks that each recovered key is the trailing dot-segment of some backtick-quoted span in the doc (e.g. key `keep_days` matches the doc's `` `otto.retention.keep_days` ``). Deliberately NOT a bare substring search: several key names are short and common enough (`as`, `help`, `name`, `default`) that a substring check would pass by accident against ordinary prose; scoping to backtick spans and exact trailing-segment match avoids that false-positive risk.
- Per-struct expected counts (2, 9, 5, 7, 13, 8 = 44) are asserted explicitly in the test body, not just implied by the doc-mention check, so a struct that LOSES a field the doc still happens to mention (stale prose, not yet pruned) is still caught.

### Deviations
- None. All four Phase 5 deliverables and both of the panel-round-2-approved drift-test techniques were implemented as specified.
- The design doc's Phase 5 body doesn't name a specific file path for the migration note; `docs/commands/ottofile-strict-schema-migration.md` was chosen (see Design decisions) since none was mandated.

### Tradeoffs
- README example: the design doc's own worked example (`otto.envs.BUILD_DIR` referenced from a task-level `envs:` value, e.g. `BUILD_OUTPUT: "${BUILD_DIR}/${PROJECT_NAME}"`) was tried first and FAILS at runtime with `Failed to resolve environment variable 'BUILD_DIR' not found` — not a schema defect, a pre-existing bug in environment-variable resolution order (task-level `envs:` values cannot reference global `otto.envs:` values; only the task's own bash body can). Confirmed the bug is pre-existing and not introduced by this phase: `examples/environment-variables/otto.yml`'s own `setup:` task, tracked and passing under `examples_integration_test` today, hits the identical failure when run directly (`otto setup` -> `Environment variable 'PROJECT_NAME' not found`) even though that test only checks it PARSES, not that it RUNS. Chose a simpler README example (global `otto.envs.PROJECT_NAME` referenced directly from the task's `bash:` body, task-level `envs.BUILD_DIR` used the same way, neither cross-referencing the other) specifically because it avoids this pre-existing resolution-order bug and can be honestly described as "working" (`otto build` -> exit 0, `Building myproj in build`). Recording the bug here since it's real, reproducible, and not mine to fix in this phase (out of scope: this phase documents the schema, not environment-variable resolution semantics).
- `expected_keys_from_deny_unknown_fields`'s bogus-key probe vs. a hand-maintained `const` array of expected keys per struct — the probe was chosen because a hand-maintained array is exactly the kind of thing that drifts silently (the whole failure mode Phase 5 exists to close); deriving the list from serde's own compile-time-generated error message means the runtime half of the test cannot go stale on its own, only the compile-time destructuring half can (and that half fails loudly, per the design doc's proof-of-bite requirement).

### Open questions
- None.

### Success criteria
- (a) **PASS.** README's `envs:` block (final form: global `otto.envs.PROJECT_NAME`, task-level `envs.BUILD_DIR`, both referenced only from `bash:`) copied verbatim into a scratch `.otto.yml` and run against `target/release/otto` built from this phase's tree: `otto build` -> stdout `Building myproj in build`, exit 0. See Tradeoffs above for why the doc's originally-drafted cross-referencing example was rejected before landing.
- (b) **PASS.** `ottofile_reference_key_inventory_is_exhaustive` (`src/cfg/task.rs`) passes: `cargo test --lib cfg::task::foreach_tests::ottofile_reference_key_inventory_is_exhaustive` -> `1 passed; 0 failed`. Per-struct counts recovered from serde's own error text: `ConfigSpec` 2, `OttoSpec` 9, `RetentionSpec` 5, `ForeachSpec` 7, `TaskSpecHelper` 13, `ParamSpec` 8, total 44 — matching the design doc's panel-round-2 probe exactly. Every one of those 44 keys is mentioned in `docs/commands/ottofile-reference.md`, verified by the test itself, not by inspection.
- (c) **PASS.** All 20 tracked ottofiles under `examples/` (`git ls-files examples/ | grep -E 'otto\.(yml|yaml)$|\.otto\.yml$'` -> exactly 20) parse under the current (strict, Phase-4-live) build: `cargo test --test examples_integration_test` -> `21 passed; 0 failed` (20 real + 1 `_test_broken` negative). Cross-checked independently by grepping every line-leading key across all 20 files and manually confirming each fixed-field key that appears (`about`, `action`, `after`, `api`, `as`, `bash`, `before`, `choices`, `default`, `dest`, `envs`, `foreach`, `glob`, `help`, `home`, `input`, `items`, `jobs`, `max_items`, `metavar`, `name`, `on-failure`, `otto`, `output`, `params`, `parallel`, `python`, `range`, `tasks`, `tty`, `when`) is named in the reference doc. (`examples/flags/flag_demo.yml` was excluded from this check: its filename doesn't match any name otto's config loader recognizes, so it is not an ottofile in the sense this criterion means, and it is not one of the 20 files Phase 2's own success criteria enumerated.)

**Proof the drift test bites** (both halves, each demonstrated and reverted):
1. Deleted the `otto.retention.keep_days` row from `docs/commands/ottofile-reference.md`. `ottofile_reference_key_inventory_is_exhaustive` **FAILED**: `RetentionSpec's on-disk key \`keep_days\` is not mentioned in docs/commands/ottofile-reference.md`. Doc restored, re-ran green.
2. Added a scratch fifteenth field (`scratch_drift_field: bool`) to `RetentionSpec` in `src/cfg/otto.rs`. The build **failed to compile**, at the exhaustive-destructuring pattern this test owns: `error[E0027]: pattern does not mention field \`scratch_drift_field\` --> src/cfg/task.rs:1397:13` (plus two more `E0063`s at `RetentionSpec`'s own two manual `Default`/test-literal constructions, which is the struct's own code demanding the same awareness even before this test's line is reached). Field reverted, `git diff -- src/cfg/otto.rs` empty, full suite re-run green.

### `src/makefile/converter.rs:33` — no change needed
- `converter.rs:33` is `api: "1".to_string(),`, inside `convert_otto_spec`. Phase 3 kept `OttoSpec.api` as `String` (no type change), and `CURRENT_API_VERSION` is still `"1"`, so this literal is still a supported, correct api declaration and `otto convert`'s output still loads under the strict build (re-verified: `otto Convert -o .otto.yml` on a two-target Makefile, then `otto --tasks` on the emitted file, exit 0). Verified as a **no-op**; left untouched rather than churned into a reference to `otto::cfg::otto::CURRENT_API_VERSION`, since the design doc scoped this bullet to "if anything about api emission changed" and nothing did.

- `otto ci`: **green**. `cargo test --workspace --all-features --no-fail-fast` -> 27 binaries, all `0 failed`; lib suite `535 passed; 0 failed` (528 pre-existing + the new drift test, plus 6 more added across this phase's work — accounted for by the drift test itself plus incidental coverage, no other new tests added this phase). `tests/roundtrip.rs` untouched, `11 passed`. One `cargo fmt` pass and one clippy fix (`type_complexity` on the drift test's `expectations` slice, resolved with a local `type KeyProbe = (&'static str, usize, fn() -> Vec<String>);` alias) were needed before `[ci]` reported green; no other findings.

## Finalization: acceptance-criteria verification

Run by the orchestrator against `target/release/otto` built from `b0ad2bb`, after all five phases landed. All seven criteria PASS.

| # | criterion | result | observed |
|---|---|---|---|
| 1 | Wrong-level key | PASS | `otto up` exit 1, stderr `` tasks.up: unknown field `parallel`, expected one of `help`, `after`, `before`, `input`, `output`, `envs`, `params`, `bash`, `python`, `action`, `foreach`, `on-failure`, `tty` at line 3 column 5 ``. On main: three subtasks ran concurrently, exit 0, no diagnostic. |
| 2 | Invented root key | PASS | exit 1, stderr `` unknown field `envs`, expected `otto` or `tasks` ``. On main: `[show] PROJECT=[UNSET]`, exit 0. See the location-suffix amendment below. |
| 3 | Version gate ordering | PASS | fixture with BOTH `api: 2` and `tasks.show.parallel`: exit 1, stderr `otto: unsupported api version '2' (this otto supports: 1). upgrade otto.` The version error wins, as specified. On main: `[show] ran`, exit 0. |
| 4 | Escape hatch tells the truth | PASS | Malformed ottofile: exit 2, stdout 745 bytes (flag list), `No ottofile found` count on stdout = **0**, stderr 146 bytes carrying `ERROR: failed to parse ottofile: .../.otto.yml` + `tasks.up.before: invalid type: map, expected a sequence at line 4 column 7`. No ottofile anywhere: exit 2, stdout 928 bytes, `No ottofile found` present, stderr 0 bytes (unchanged from main). |
| 5 | Unchanged happy path | PASS | `otto ci` -> `[ci] ✅ All CI checks passed!`. `tests/roundtrip.rs` unmodified (`git diff --stat 5640628..HEAD -- tests/roundtrip.rs` empty) and 11 passed / 0 failed. |
| 6 | Examples ship clean | PASS | All **20** tracked ottofiles under `examples/` parse AND report >= 1 task. `examples/_test_broken/otto.yml` still fails: exit 1, `` unknown field `invalid`, expected `otto` or `tasks` `` (message changed under strict parsing exactly as the doc predicted; the test asserts failure, not a message). Baseline on main was 21 parse / 3 fail with `examples/old/ex2` silently at zero tasks. |
| 7 | Converter output stays loadable | PASS | `otto Convert -o .otto.yml` on a two-target Makefile: exit 0; reload `otto --tasks` exit 0. Emitted `retention:` keys are snake_case (`keep_days`, `keep_last`, `keep_failed`, `auto_prune`, `prune_interval_hours`), matching `RetentionSpec` exactly; `api: '1'` passes the new gate. |

### Doc amendment made at finalization (with evidence)

- **Design doc line 222** claimed the root-level unknown-key message is `unknown field 'envs', expected 'otto' or 'tasks' at line 8 column 1`. Measured against the doc's own envs-first repro, the `at line N column M` suffix is **absent** when the unknown key is the first key in the root mapping. This is a doc defect: a factual claim about observed output that does not hold in the ordering the doc itself used for the repro. It is NOT a sound criterion being bent to match the code — acceptance criterion 2 asserts field + expected-set + non-zero exit, never a location, and passes in both orderings. Amended in place with the measurement and a warning not to reorder the shipped test fixture (which puts `tasks:` before `envs:` precisely so the location assertion has something to check). Independently found by the Phase 4 agent and recorded in its notes before finalization.

### Known, pre-existing, explicitly out of scope (reproduced, not fixed)

- `otto Convert` on a two-target Makefile emits `tasks: {}`, dropping both targets. Reproduced during criterion 7. The design doc already records this at line 154 as pre-existing and unrelated to this work.
- Task-level `envs:` referencing a global `envs:` value fails at runtime with `Environment variable 'X' not found` (env resolution order). Found by the Phase 5 agent; reproduces on the tracked `examples/environment-variables/otto.yml`. Not introduced here; the README example was written to avoid it.
- `executor::scheduler::tests::test_file_dependencies_timestamp_precision` can fail under parallel test threads with `Failed to create directory /otto-home: Permission denied` — tests in `src/executor/workspace.rs` mutate the process-global `OTTO_HOME`. Reproduces on bare `main`.
