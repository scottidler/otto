# Implementation Notes: Boundary Fixes and Dynamic Foreach

Companion to `docs/design/2026-08-28-boundary-fixes-and-dynamic-foreach.md`.
Append-only. One section per phase, four buckets each.

## Pre-phase: ready-to-build gate (2026-08-29)

### Design decisions
- None.

### Deviations
- None.

### Tradeoffs
- None.

### Open questions
- None.

### Acceptance-criteria amendments (doc defects found by the gate)
- **Phase 8 criterion (b)** — amended `rg -l 'Positional parameters' docs/` to
  `rg -l 'Positional parameters' docs/commands/ README.md`. Evidence: on `main`
  the original command exits 0 with a single hit,
  `docs/design/2026-08-28-boundary-fixes-and-dynamic-foreach.md:253`, which is
  the criterion line itself. The criterion was self-satisfying and could never
  bite. The Phase 8 body already names `docs/commands/` and the README as where
  the documentation lands, so the amendment narrows the criterion to match the
  work the phase actually specifies. Amended baseline on `main`: no hits,
  exit 1.

### Missing observed-output lines recorded (no criterion changed)
- **Phase 2 (c)** circular env: fixture `A: '${B}'` / `B: '${A}'` warns
  `Failed to resolve environment variable 'A': Environment variable 'B' not found`,
  task fails on the unbound var, exit 1.
- **Phase 3 (b)** `PARENQ: '$(echo ")")'` warns
  `Command 'echo "' failed with exit code 2: sh: 1: Syntax error: Unterminated quoted string`,
  `PARENQ` unbound, exit 1.
- **Phase 3 (c)** `cargo test env` on `main`: 13 tests, 0 failed.

### Environment note
- `target/` symlinks to `/media/saidler/intel-480gb-ssd/cargo-target/otto-rs/otto/target`,
  which did not exist (drive contents predate 2026-07-25). Created it; cargo
  then built clean. Not a code issue.

## Phase 0: De-duplicate the in-flight docs

### Design decisions
- Used the literal marker string `covered by docs/design/2026-08-28-boundary-fixes-and-dynamic-foreach`
  (lowercase "covered by", matching the success criterion's `rg` pattern exactly)
  on each superseded bullet in `docs/design/2026-06-10-code-review-remediation.md`,
  paired with a strikethrough of the stale fix description so the bullet still
  shows what it used to claim without asserting it as remaining remediation work.
- Annotated all three explicitly-NOT-moved remediation bullets (`-j 0`,
  help-path `-o`/`$OTTOFILE`, token-boundary-collision lookahead) inline with
  "**Stays remediation scope**" plus the specific reason the 08-28 doc does not
  cover them, so a reader hitting any of the six touched bullets in the
  remediation doc gets the disposition without cross-referencing this doc.
- Added the Phase 2 (serial-foreach mid-chain-failure) note as a new paragraph
  under the existing bullet rather than editing the bullet's own prose in
  place, since the existing analysis (chain-edge assumption) is itself stale
  once the 08-28 doc's Phase 4 lands, and the note explains why without
  deleting the historical bug description.
- Placed the architecture-doc note directly under its header block (next to
  "Sequenced after") rather than inline in Problem/Goals, since it is scoped
  to the whole doc's relationship to `--tasks`/`--plan`, not one bullet.

### Deviations
- None. The task's four numbered instructions map directly onto the design
  doc's own "Relationship to In-Flight Docs" bullet list; no seam ambiguity
  requiring a different anchor point than specified.

### Tradeoffs
- Strikethrough (`~~text~~`) vs. deleting the superseded bullet text outright:
  kept the original wording struck through rather than removed, so the
  remediation doc's history of "what it used to claim" stays visible in a
  diff/blame sense even though it's an uncommitted working-tree file today.
  Matches the instruction to "annotate" rather than rewrite.

### Open questions
- None.

### Verification
- `rg -c 'covered by docs/design/2026-08-28-boundary-fixes-and-dynamic-foreach' docs/design/2026-06-10-code-review-remediation.md` → `3`
- `rg -c 'covered by docs/design/2026-08-28-boundary-fixes-and-dynamic-foreach' docs/design/2026-06-10-architecture-product-completion.md` → `1`
- Manual check: the remediation doc's help-drift, `$()` nesting, and `-C=DIR`
  bullets are struck through and marked covered by the 08-28 doc, so it no
  longer claims those three as its own open work. The architecture doc's note
  states `--tasks`/`--plan`/foreach-command disposition without claiming
  either surface as its own near-term deliverable. The three explicitly-NOT-moved
  items (`-j 0`, help-path `-o`/`$OTTOFILE`, token-boundary-collision lookahead)
  are each annotated "Stays remediation scope" with the specific reason, so
  they remain visible as live remediation-doc work.
- `otto ci`: green (full run, all unit/integration/doc tests + clippy + fmt
  passed on this commit's tree, no source files touched by this phase).

## Phase 1: Help drift

### Design decisions
- `global_args() -> Vec<Arg>` (`src/cli/parser.rs`, new fn just above
  `otto_command()`) is the single declaration of cwd/ottofile/list-subtasks/
  jobs/tui, in that order per the Architecture bullet. `otto_command()`,
  `build_help_command()`, and `build_help_command_with_error()` each loop
  `for arg in Self::global_args() { cmd = cmd.arg(arg); }` instead of
  hand-declaring a subset — the loop-and-consume shape (not, say, a builder
  method that takes `Command` and returns it) was chosen so each call site
  reads identically and a diff review can see all three consume the same
  function by name.
- `apply_cwd_flag` (`src/main.rs:74-81`) gained a `strip_prefix("-C=")` arm
  alongside the existing `"-C"`/`"--cwd"`/`"--cwd="` arms, so every `-C`
  variant is stripped before clap ever sees it. The parser-side `cwd` Arg in
  `global_args()` stays registered per the doc's explicit reconciliation: it
  is what renders in `--help`, and single-sourcing only works if the Arg
  exists in the one function all three builders share.
- Help snapshot test (`src/cli/parser.rs`, `test_help_global_flags_no_drift`
  in the `tests` mod): builds all three commands, extracts just the
  `Options:` block (`options_section()` helper, anchored from `"Options:"`
  through the auto-appended `-V, --version` entry so builder-specific
  `after_help` text doesn't pollute the comparison), and asserts each against
  one pinned `EXPECTED_GLOBAL_OPTIONS_HELP` constant. Demonstrated to bite:
  temporarily made `build_help_command_with_error()` skip the `cwd` arg,
  reran the test (failed, diff showed the missing `-C, --cwd` block), then
  reverted and reran (passed) — see Verification below for both outputs.

### Deviations
- None. Implemented at the seam the doc names (`parser.rs:486-532` /
  `:1388-1441` / `:1443-1465`); no signature or seam correction was needed.

### Tradeoffs
- Pinned string constant vs. a lighter "contains" assertion per flag: chose
  the exact pinned block (byte-for-byte, including clap's wrapped-value blank
  lines) over checking that each flag substring is merely present, because a
  substring check wouldn't catch a builder that renders the right flags in a
  different order or drops formatting — the failure mode this test exists to
  catch is specifically "a builder's Options: block silently diverges from
  the other two," which only an exact-match snapshot pins.
- `apply_cwd_flag` recognizes `-C=DIR` (per the design doc and the remediation
  doc's exact bullet wording) but not the bare attached form `-CDIR` (no
  `=`). Not implemented: out of the phase's stated scope (the doc's
  Architecture bullet and remediation cross-reference both name `-C=DIR`
  specifically), and `-CDIR` isn't a form clap itself would otherwise accept
  for this Arg either, so there's no silent-swallow hazard to close there.

### Open questions
- None.

### Verification
- `otto --help` (built from this working tree, `target/debug/otto`) Options
  block:
  ```
  Options:
    -C, --cwd <DIR>        Change to DIR before doing anything
    -o, --ottofile <PATH>  path to the ottofile [env: OTTOFILE=] [default: .]
        --list-subtasks    List all foreach subtasks and exit
    -j, --jobs <N>         Number of parallel jobs [default: 32]
    -t, --tui              Enable interactive TUI dashboard for task monitoring
    -h, --help             Print help
    -V, --version          Print version
  ```
  All three of `-C/--cwd`, `-o/--ottofile`, `--list-subtasks` present, criterion (a) met.
- `rg -c 'global_args' src/cli/parser.rs` → `6` (>= 4: one `fn global_args()`
  definition, three call sites, plus two doc-comment mentions), criterion (b)
  met.
- Snapshot test bite demonstration, criterion (c):
  - Broken (temporarily skipped `cwd` in `build_help_command_with_error()`'s
    loop): `cargo test --lib test_help_global_flags_no_drift` →
    `FAILED`, `assertion `left == right` failed: build_help_command_with_error()
    global flags drifted from the pinned snapshot` (left output missing the
    `-C, --cwd <DIR>` line entirely).
  - Restored: `cargo test --lib test_help_global_flags_no_drift` → `test
    result: ok. 1 passed; 0 failed`.
- `-C=DIR` end-to-end (not just the unit test): from `/tmp/otto-cwd-test`
  running a fixture `otto.yml` with a `hi: bash: pwd` task,
  `otto -C=/tmp/otto-cwd-test hi` (run from a different cwd) → `[hi]
  /tmp/otto-cwd-test`, `finished successfully`, exit 0.
- `otto ci`: green (full run: lint, clippy, compile, fmt-check, test — all
  unit + integration + doctest suites passed) on this phase's commit
  `bb0f8dc`. (Two `sccache: Operation not permitted` failures were transient
  Bash-sandbox interference with the sccache daemon socket, not code
  failures — resolved by rerunning outside the sandbox; unrelated to this
  phase's changes.)

## Phase 2: Env self-reference

### Design decisions
- Per-expression context built by a new helper rather than one mutated map —
  `src/cfg/env.rs:evaluation_context` — the loop previously carried a single
  `current_env` that was both the context and the accumulator; separating them
  (`base_env` = inherited minus all declared keys, `evaluated` = resolved
  declared values, `inherited` = the untouched invocation environment) is what
  makes "seed the inherited value for exactly one key" expressible.
- Precedence inside the context is base < inherited-seed < resolved —
  `src/cfg/env.rs:evaluation_context` — a declared key that has already
  resolved must win over its own inherited value, so the seed can never shadow
  a resolved value if a key is ever re-evaluated.
- The seed is keyed on the variable under evaluation only —
  `src/cfg/env.rs:evaluate_envs` — this is the hazard the design names: seeding
  all declared keys would let a cross-reference resolve to an inherited value,
  nondeterministically by HashMap iteration order. Demonstrated: with that
  break in place the property test reports e.g. `OTTO_TEST_ORDER_C =
  "INHERITED_B-c", want "inherited-a-x-b-c"`.
- Property test drives all 4! insertion orders x 8 repeats —
  `src/cfg/env.rs:tests::test_evaluate_envs_cross_references_never_read_inherited_values`
  — insertion order alone does not fix HashMap iteration order, so repeats
  sample fresh hashers for the same order; 192 runs make the seed-all break
  bite deterministically in practice.
- Env-mutating unit tests in `src/cfg/env.rs` and the new one in
  `src/executor/task.rs` are `#[serial]` — these tests set process-wide env
  vars that `evaluate_envs` reads via `env::vars()`; the repo already uses
  `serial_test` for env-mutating unit tests (`src/main.rs`,
  `src/executor/action.rs`). The pre-existing `test_evaluate_envs_simple` was
  annotated too, since it mutates env in the same binary.

### Deviations
- The circular case errors through the no-progress fallback branch
  (`src/cfg/env.rs:50-65`), not through `MAX_ITERATIONS`. That is unchanged
  from main and matches the doc's recorded `Observed on main` output;
  `MAX_ITERATIONS` remains the backstop for a shape that keeps making partial
  progress. Criterion (c) is met on behavior (loud error, exit 1), with the
  mechanism named accurately here.
- Also added `test_evaluate_envs_circular_reference_errors_even_when_inherited`
  beyond the stated criteria: a circular pair whose keys DO exist in the
  inherited environment must still error. This is the direct regression test
  for the seed-all hazard on the circular path (with the break, it resolves to
  inherited values and passes silently).
- Scope note, not a deviation: the merged scope (`src/executor/task.rs:156`)
  has no production caller — `Task::from_task*` in `src/executor/` is only
  reached from unit and integration tests; the live CLI path builds
  `cli::parser::Task` (`src/cli/parser.rs:755`). It is still covered by this
  fix because it calls the same `evaluate_envs`, and a unit test now pins it.

### Tradeoffs
- Cloning the base context per expression vs. threading a mutable map with
  save/restore around each evaluation — chose the clone: environments are tens
  of entries and declared env maps are small, and the clone makes the
  "which keys are visible to this one expression" question answerable by
  reading one function.
- Property test over real insertion orders vs. adding a test-only seam to
  inject a deterministic evaluation order — chose the orders: a seam would test
  the seam, and 192 randomized runs already bite reliably (demonstrated).

### Open questions
- None.

### Verification (verbatim)
- Criterion (a), fixture `otto: envs: {MYVAR: '$(echo "${MYVAR:-fallback}")'}`
  with `show: bash: echo "MYVAR=[${MYVAR}]"`:
  - Before (at `bb0f8dc`): `MYVAR=from-shell otto show` → `[show]
    MYVAR=[fallback]`, exit 0.
  - After: `MYVAR=from-shell otto show` → `[show] MYVAR=[from-shell]`, `[show]
    finished successfully`, exit 0.
  - After, unset case: `env -u MYVAR otto show` → `[show] MYVAR=[fallback]`,
    exit 0.
- Criterion (b): `cargo test --lib cfg::env` → `test result: ok. 11 passed; 0
  failed` including
  `test_evaluate_envs_cross_references_never_read_inherited_values`.
- Criterion (c), fixture `A: '${B}'` / `B: '${A}'`, after the change:
  `Warning: Failed to evaluate global environment variables: Failed to resolve
  environment variable 'A': Environment variable 'B' not found`, then `[show]
  script.sh: line 17: A: unbound variable`, `[show] failed`, `Task show failed
  with exit code Some(1)`, exit 1. (Which of A/B is named varies with HashMap
  order; main printed `'B' ... 'A' not found` on the same fixture.)
- Bite demonstration, break 1 (seed inherited for ALL declared keys):
  `cargo test --lib cfg::env` → `test result: FAILED. 9 passed; 2 failed`
  (`test_evaluate_envs_cross_references_never_read_inherited_values`,
  `test_evaluate_envs_circular_reference_errors_even_when_inherited`);
  `cargo test --test env_self_reference_test` → `FAILED. 4 passed; 1 failed`,
  `test_cross_reference_prefers_declared_value_over_inherited` reporting
  `[show] DERIVED=[inherited-base/child]`.
- Bite demonstration, break 2 (drop the self-seed, i.e. main's behavior):
  `cargo test --lib cfg::env` → `FAILED. 8 passed; 3 failed`
  (both self-reference tests plus the property test);
  `cargo test --test env_self_reference_test` → `FAILED. 3 passed; 2 failed`
  (`[show] MYVAR=[fallback]` where `from-shell` was expected);
  `cargo test --lib evaluate_merged_envs_self` → `FAILED. 0 passed; 1 failed`.
- Restored: `cargo test --lib cfg::env` → `ok. 11 passed`;
  `cargo test --test env_self_reference_test` → `ok. 5 passed`;
  `cargo test --lib evaluate_merged_envs` → `ok. 4 passed`.
- `otto ci`: green — `[ci] ✅ All CI checks passed!` (log confirms the new
  binary ran: `Running tests/env_self_reference_test.rs`).

## Phase 3: `$()` depth scan

### Design decisions
- Scanner carries a quote state PER NESTING LEVEL, not one state overall —
  `src/cfg/env.rs:find_command_substitution` — sh treats a nested `$(` inside
  double quotes as a fresh quoting context, so `$(echo "$(echo ")")")` closes
  where sh says it closes. A single `Quote` variable would end the outer
  substitution at the inner closing quote's paren.
- Any unquoted `(` opens a level, not just `$(` — same function — a bare
  subshell (`$( (echo hi) )`) balances the same way sh balances it; counting
  only `$(` would truncate at the subshell's `)`.
- Backslash escapes are honored outside quotes and inside double quotes, but
  NOT inside single quotes — same function — that is sh's rule (`'a\'` is
  `a\`), and getting it wrong flips where a single-quoted region ends.
- The search for the OPENING `$(` is not quote-aware — `find_substitution_start`
  — the value is a YAML scalar, not shell input, so there is no outer quoting
  context to honor; this preserves the previous behavior exactly and keeps the
  change confined to boundary finding.
- Unmatched `$(` is checked in a pre-pass over every raw value at the top of
  `evaluate_envs`, before the resolution loop — `src/cfg/env.rs:evaluate_envs` —
  the loop swallows per-key errors as "might depend on an unresolved variable",
  so a structural error routed through it would be retried and then reported
  with the generic deferral message. The pre-pass is what lets the error name
  both the key and the value, per the design doc.
- `validate_command_substitutions` walks every substitution in a value, not just
  the first, so `ok $(echo done) then $(echo oops` is caught.

### Deviations
- The design doc says "Replace the regex find ... with a scanner that finds
  `$(` and walks to the matching `)` counting depth". Implemented at that seam,
  with one addition the doc does not name: the loud unmatched-`$(` error is
  raised from a pre-pass in `evaluate_envs` rather than only from
  `resolve_shell_commands_with_env`. Same effect, correct seam: the doc requires
  the error to name the key, and only `evaluate_envs` knows the key. The scanner
  still returns the error too, so a future caller cannot bypass it.
- Incidental behavior refinement, not requested: the old code did
  `result.replace(full_match, &output)`, which pasted one execution's output
  over every identical occurrence in the value. The scanner substitutes each
  occurrence in place with its own execution's output. Same result for pure
  commands; correct for impure ones.

### Tradeoffs
- Paren/quote scanning vs a real sh tokenizer — chose scanning. The scanner's
  only job is finding the boundary; sh still parses and executes the content, so
  a case the scanner reads differently than sh degrades to sh's own error rather
  than to silent misbehavior. The adversarial table plus a differential check
  against real `sh` is the guard the design doc's risk row asked for.
- Pre-pass validation costs one extra scan of every declared value per
  `evaluate_envs` call — chose it over threading a distinguishable error kind
  through the retry loop. Cheaper to read, and it fails before any subprocess
  runs.

### Open questions
- The loud unmatched-`$(` error is demoted to a warning by both call sites:
  global envs (`parser.rs:712`, `unwrap_or_else` -> empty map) and task envs
  (the task-scope caller), so a fixture with `${BROKEN:-UNSET}` defaults still
  exits 0. This is defect 2's recorded side effect ("a failed substitution
  abandons the entire global env map"), explicitly out of scope for this phase.
  Worth confirming whether a config-shape error like this should be fatal at
  load rather than a warning — that decision belongs with whoever fixes the
  abandonment behavior.

### Verification (verbatim)
- Criterion (a), fixture `NESTED: '$(echo "$(basename /a/b)")'`:
  - Before (at `c026a7d`): `Warning: Failed to evaluate global environment
    variables: Failed to resolve environment variable 'NESTED': Command 'echo
    "$(basename /a/b' failed with exit code 2: sh: 1: Syntax error: end of file
    unexpected (expecting ")")`, then `[show] NESTED=[UNSET]`.
  - After: `[show] NESTED=[b]`.
- Criterion (b), fixture `PARENQ: '$(echo ")")'`, same run:
  - Before: unset (the whole map was abandoned by NESTED's failure):
    `[show] PARENQ=[UNSET]`.
  - After: `[show] PARENQ=[)]`.
  - Sibling key in the same block, `SIBLING: 'plain-value'`: before
    `[show] SIBLING=[UNSET]`, after `[show] SIBLING=[plain-value]`.
- Criterion (c): `cargo test env` -> `ok. 27 passed` + `ok. 1 passed` +
  `ok. 4 passed` + `ok. 1 passed`, 0 failed (baseline at `c026a7d` was 19 + 4 +
  1 = 24). Zero existing tests modified: the entire removed-line set of the diff
  is the six regex lines inside `resolve_shell_commands_with_env`.
- Unmatched `$(` end to end, fixture `BROKEN: '$(echo hello'`:
  - Before (at `c026a7d`): passed through literally into the task script —
    `[show] .../script.sh: line 26: unexpected EOF while looking for matching
    `"'`, `Task show failed with exit code Some(2)`, exit 1. No mention of the
    env at all.
  - After: `Warning: Failed to evaluate global environment variables:
    Environment variable 'BROKEN': unmatched '$(' (no closing ')') in value
    '$(echo hello'`.
- Whole-map abandonment on a genuine command failure is UNCHANGED, fixture
  `BAD: '$(exit 3)'` + `GOOD: 'still-here'`: `Warning: Failed to evaluate global
  environment variables: Failed to resolve environment variable 'BAD': Command
  'exit 3' failed with exit code 3:` then `[show] GOOD=[UNSET]`.
- Adversarial table (14 cases, `test_command_substitution_adversarial_table`),
  each asserting the carved command text AND the resolved value, with
  `test_command_substitution_agrees_with_sh` checking every expectation against
  real `sh`: plain; nested in double quotes; nested unquoted; `)` in double
  quotes; `)` in single quotes; `(` in single quotes; backslash-escaped `)`;
  backslash-escaped `"` inside double quotes; backslash literal inside single
  quotes; `'` inside double quotes; `"` inside single quotes; subshell group;
  adjacent to literal text; empty `$()`. Plus separate tests for multiple
  substitutions in one value, a repeated substitution, and five unmatched-input
  shapes. All green.
- Bite demonstration, break 1 (quote-blind: force the level's quote state to
  `Quote::None`): `cargo test --lib cfg::env` -> `FAILED. 15 passed; 4 failed`
  (`test_command_substitution_adversarial_table`,
  `test_evaluate_envs_nested_and_quoted_substitutions`,
  `test_multiple_substitutions_in_one_value`,
  `test_unmatched_substitution_is_an_error`);
  `cargo test --test env_command_substitution_test` -> `FAILED. 2 passed; 2
  failed`. Table detail: `close paren in double quotes: boundary carved "echo
  \"", want "echo \")\""`.
- Bite demonstration, break 2 (nesting-blind: return at the first unquoted `)`
  regardless of depth): `cargo test --lib cfg::env` -> `FAILED. 16 passed; 3
  failed`; `cargo test --test env_command_substitution_test` -> `FAILED. 1
  passed; 3 failed`. Table detail: `nested substitution: boundary carved "echo
  \"$(basename /a/b", want "echo \"$(basename /a/b)\""`.
- Restored: `cargo test --lib cfg::env` -> `ok. 19 passed`;
  `cargo test --test env_command_substitution_test` -> `ok. 4 passed`.
- `otto ci`: green — `[ci] ✅ All CI checks passed!` (one `fmt-check` failure on
  the first run, auto-fixed by the `on-failure: fmt-fix` sugar, green on rerun).

## Phase 4: Serial foreach as a scheduler property

### Design decisions
- Serial ordering became a typed pair on both `Task` structs —
  `serial_group: Option<String>` + `serial_index: usize` on
  `src/cli/parser.rs::Task` and `src/executor/task.rs::Task` — the Data Model
  bullet calls for typed fields, not env-string parsing; `OTTO_FOREACH_INDEX`
  stays untouched as the env-facing value.
- `expand_foreach_tasks_with_serial` returns `(TaskSpecs, SerialMembership)`
  instead of writing the group onto `TaskSpec` —
  `src/cli/parser.rs:expand_foreach_tasks_with_serial` — group membership is a
  scheduling fact about a run, not config a user wrote; putting it on `TaskSpec`
  would have leaked it into the round-trip serializer and forced two new fields
  into ~20 test literals for a value the config never carries.
- The gate lives in one type, `SerialGroups` —
  `src/executor/scheduler.rs:SerialGroups` — built from the run set only, so
  `predecessor()` returns the nearest preceding member THAT IS IN THE RUN SET by
  construction rather than by a filter the caller must remember to apply.
- `SerialGroups::classify` returns the existing `EdgeState`, and `classify_gates`
  appends it to the dependency-edge states — `src/executor/scheduler.rs` — the
  gate composes with dependency readiness at every one of the four readiness
  sites instead of replacing it, which is what makes the mixed-edges case
  (Risks row 1) fall out for free.
- A member with a predecessor starts in `blocked_tasks`, never in `ready_queue`
  — `src/executor/scheduler.rs:execute_all` init loop — the ready loop's
  not-yet-satisfied branch re-queues and rotates, and with three or more pending
  members in the queue and nothing active that rotation is an infinite spin.
  The chain edges used to keep members out of the ready queue; the init
  condition now does it explicitly. The same reasoning added the gate check to
  the up-to-date-skip sweep inside `try_start_ready_task`, which pushes blocked
  tasks straight to the ready queue on `task_deps` alone.
- Five duplicated skip blocks collapsed into `TaskScheduler::mark_skipped`, which
  also PRINTS the reason — `src/executor/scheduler.rs:mark_skipped` — the phase
  criterion demands visible skip reasons, and the cascade rides the same code
  path as an unreachable-edge skip. Two output policies for identical skips would
  be incoherent: in the `before: [dep]` fixture the first member is skipped by
  its own edge and the rest by the cascade, so a cascade-only print would show
  beta and gamma and silently drop alpha.
- `skip_reasons` moved from a local map (previously `drop`ped at the end of
  `execute_all`) to `TaskScheduler.skip_reasons` with a `get_skip_reasons()`
  getter — the criterion "report skipped with a reason" is otherwise unassertable
  from a test, and the design doc's own note said this map should be surfaced.
- Parser -> executor task conversion single-sourced as
  `impl From<cli::parser::Task> for executor::Task` —
  `src/executor/task.rs` — it was copy-pasted at three sites (`app.rs` plain
  path, `app.rs` TUI path, `graph.rs`), and serial ordering has to survive all
  three including `--tui`.
- DOT ordering edges are derived from the group index at render time —
  `src/executor/graph.rs:serial_order_pairs` — only members present in the graph
  are chained, so a targeted single-member run renders no ordering edge.

### Deviations
- **User-visible output change, disclosed:** every non-up-to-date skip now
  prints `[task] skipped (<reason>)` to the terminal. Before this phase those
  skips were silent (`info!` only, and the reasons map was dropped). The design
  doc only demanded visibility for the group members the serial gate skips; the
  print is at the shared site for the coherence reason above, so ordinary
  unreachable-edge skips became visible too. Visible in otto's own CI:
  `[fmt-fix] skipped (dep fmt-check succeeded; this task required when: failure)`.
- **User-visible output change, disclosed and designed:** DOT-family output
  (`dot`, `svg`, `png`, `pdf`, `Auto`) renders serial chains as
  `[label="order", color="gray40", style="dashed"]` where it rendered
  `[label="depends", color="black", style="solid"]`. Measured diff against
  `10eb334` on the serial fixture is exactly those two lines. ASCII output is
  byte-identical (verified by diff, not assumed).
- **The design doc's Phase 4 criterion (c) claim "parent aggregates Failed" is
  wrong, and is not implemented.** Measured at `10eb334` and again after this
  phase: with alpha failing, the virtual parent is `Skipped`. The parent's
  `when: always` edges point at subtasks that are `Skipped`, and `classify_edge`
  makes a Skipped source Unreachable for every `when:` variant, so the parent
  never executes and the failed-first aggregation at the virtual-parent success
  arm never runs. The same doc bullet states twice that aggregation does not
  change ("The virtual parent aggregates as today", "with no aggregation
  change"), so this phase preserved the behavior and pinned `Skipped` in
  `tests/serial_foreach_test.rs`. Exit code is unaffected: the run still exits
  non-zero from alpha's failure. Making the parent Failed here would be a new
  aggregation behavior nobody scoped.
- `skip_reasons` and the parser->executor `From` impl are seams the design doc
  does not name. Same effect, correct seam: the phase's central claim ("skipped
  with a visible reason") needs an observable, and the ordering fields need to
  reach the scheduler on all three conversion paths.
- Rider, recorded not chased: the TUI conversion path in `app.rs` was silently
  dropping `is_virtual_parent`. Routing it through the shared `From` impl fixes
  that. It is not this phase's subject and has no test here.

### Tradeoffs
- Group property vs ordering-only edge type — chose the property, per the
  design doc's rejected alternative. Confirmed empirically why: with the
  property, `otto up:gamma` produces a run set of exactly `["up:gamma"]` and
  the executor's per-task dependency check (`scheduler.rs`, "Dependency {} not
  satisfied") never sees a reference to a task that was not scheduled. An
  ordering edge would have survived the unfiltered `task_deps` copy in
  `process_tasks_with_filter` and hit that hard error.
- Blocking members at init vs letting the ready loop rotate them — chose
  blocking. Rotation is where the pre-existing re-queue loop turns into a spin,
  and the fix keeps the gate defensive at the ready loop anyway (it is checked
  in `classify_gates` there too, so a member that somehow reaches the queue with
  an unreachable predecessor is skipped rather than spun on).
- Printing every skip vs only cascade skips — chose every skip, accepting a
  wider output change than the doc scoped, because the alternative prints a
  reason for the second and third member of a group and stays silent for the
  first.

### Open questions
- The parent-aggregates-Failed contradiction above is a design-doc defect, not a
  code defect. Worth a decision: should a virtual parent whose subtasks are
  Failed + Skipped report Failed rather than Skipped? That is an aggregation
  change (probably belonging with the remediation doc's all-up-to-date-Skipped
  work), not Phase 4 scope.
- The visible-skip output change touches every otto run that skips a task, not
  just serial foreach. Flagging it in case the wider blast radius should be
  narrowed before release.

### Verification (verbatim)
- Criterion (a), serial fixture (`items: [alpha, beta, gamma]`,
  `parallel: false`), `otto up:gamma`:
  `[up:gamma] running gamma` / `[up:gamma] finished successfully`, exit 0.
  Exactly gamma. On main this run printed `[up:alpha]`, `[up:beta]`,
  `[up:gamma]`. Control under `parallel: true`: identical two lines, exit 0.
- Criterion (b), `otto up` on the serial fixture: `[up:alpha] running alpha` /
  `[up:alpha] finished successfully` / `[up:beta] ...` / `[up:gamma] ...` /
  `[up] finished successfully`, exit 0. Interleave test
  (`test_serial_group_never_interleaves`, 8 jobs, each member sleeps 0.2s
  between START and END markers appended to a shared file) asserts the exact
  sequence `start alpha, end alpha, start beta, end beta, start gamma, end
  gamma`. Passes.
- Criterion (c), alpha fails, `otto up`:
  `[up:alpha] failed` /
  `[up:beta] skipped (serial predecessor up:alpha failed)` /
  `[up:gamma] skipped (serial predecessor up:beta skipped; cascade)` /
  `[up] skipped (dep up:beta skipped; cascade)` /
  `Task up:alpha failed with exit code Some(1)`, exit 1. Parent status
  `Skipped` (see Deviations).
- Criterion (c), up-to-date predecessor
  (`test_up_to_date_skipped_predecessor_does_not_block_successor`): all three
  members reach `TaskStatus::Skipped` via the up-to-date path and NONE of them
  appears in `get_skip_reasons()`, proving beta and gamma were reached rather
  than gated out. Passes.
- Criterion (c), panel fixture 1 (`before: [dep]`, dep fails), `otto up`:
  `[dep] failed` /
  `[up:alpha] skipped (dep dep failed; this task required when: success)` /
  same for `up:beta`, `up:gamma` /
  `[up] skipped (dep up:alpha skipped; cascade)` /
  `Task dep failed with exit code Some(1)`, exit 1.
- Criterion (c), panel fixture 2 (failing `boom` with `after: [up:alpha]`),
  `otto up`: `[boom] failed` /
  `[up:alpha] skipped (dep boom failed; this task required when: success)` /
  `[up:beta] skipped (serial predecessor up:alpha skipped; cascade)` /
  `[up:gamma] skipped (serial predecessor up:beta skipped; cascade)` /
  `[up] skipped (dep up:alpha skipped; cascade)`, exit 1. On `10eb334` the same
  run printed only the `[boom] failed` lines and nothing about the group.
- Risks row 1 (mixed edges: group members also carry `before: [dep]`, dep
  succeeds), `otto up`: `[dep] finished successfully` then alpha, beta, gamma in
  order, `[up] finished successfully`, exit 0. No deadlock.
- ASCII graph, `Graph --format ascii` on the serial fixture, diffed against a
  `10eb334` worktree build: `IDENTICAL` (verified, not assumed).
- DOT graph, `Graph --format dot`, same comparison: two lines changed,
  `task_1 -> task_2` and `task_2 -> task_3`, `label="depends", color="black",
  style="solid"` -> `label="order", color="gray40", style="dashed"`.
- Rewritten chain-edge tests (disclosed):
  `src/cli/parser.rs::test_foreach_subtasks_chained_when_parallel_false` ->
  `test_foreach_subtasks_grouped_when_parallel_false` (asserts group + index +
  no sibling edge); `tests/flag_integration_test.rs::
  test_serial_flag_chains_foreach_subtasks` serial half rewritten to assert the
  ordered group. Both still bite (see below).
- Bite demonstration, break 1 (gate treats `skipped_set` as eligible:
  `SerialGroups::classify` returns `Satisfied` for a skipped predecessor):
  `cargo test --test serial_foreach_test` -> `FAILED. 8 passed; 2 failed` —
  `test_failed_predecessor_skips_remaining_group_members` (`left: Completed`,
  beta ran after alpha failed) and
  `test_predecessor_skipped_by_conditional_edge_cascades_visibly`
  (`left: None`, "a predecessor skipped by an ordinary when: edge must cascade,
  not hang").
- Bite demonstration, break 2 (parser pushes the chain `before:` edges again,
  the defect itself): `cargo test --test serial_foreach_test` ->
  `FAILED. 6 passed; 4 failed` — including
  `test_targeting_serial_subtask_schedules_only_that_subtask`
  (`left: ["up:alpha", "up:gamma", "up:beta"]`, "serial targeting must not
  expand the run set") and `test_serial_members_carry_group_not_edges`;
  `cargo test --test flag_integration_test` -> `FAILED. 9 passed; 1 failed`
  (`test_serial_flag_chains_foreach_subtasks`).
- Restored: `cargo test --test serial_foreach_test --test flag_integration_test`
  -> `ok. 10 passed` and `ok. 10 passed`.
- `otto ci`: green — `[ci] ✅ All CI checks passed!` (468 lib tests plus the full
  integration suite). One transient `sccache: error: Operation not permitted`
  from Bash-sandbox interference, cleared by `sccache --stop-server` and rerun;
  not a code failure.

### Addendum (post-report, evidence requested by the orchestrator)

- **Parent aggregation, pre-state evidence.** Fixture
  `items: [alpha, beta, gamma]`, `parallel: false`, alpha exits 1. At `10eb334`
  (pre-Phase-4) a status dump gives `up = Skipped`, `up:alpha = Failed(..)`,
  `up:beta = Skipped`, `up:gamma = Skipped`, run result is an error. At
  `121f384` the same dump gives byte-identical statuses. The doc's "verified"
  failed-first-aggregation claim was already false before this phase; the phase
  did not cause it. Mechanism: `src/executor/scheduler.rs:135` (same line at
  both SHAs), the `if skipped.contains(&edge.task) { return EdgeState::
  Unreachable; }` short-circuit at the top of `classify_edge`, which fires
  before the `when:` match, so the parent's `When::Always` edge to a Skipped
  subtask is unreachable and the parent never executes. The failed-first
  aggregation it would have run is at the virtual-parent arm of `execute_all`.
- **Aggregation IS failed-first when the parent runs.** Measured accidentally
  and then deliberately: a conversion that drops `serial_group` leaves the group
  unordered, beta and gamma complete, alpha fails, no subtask is Skipped, the
  parent executes and aggregates to `Failed`. So the doc's mechanism claim is
  right and its scenario claim is wrong: failed-first only decides anything in
  runs where no subtask is Skipped, which is never the case once a predecessor
  failure cascades.
- **Pre-existing silent-success hole, not introduced here, not fixed here.**
  Fixture `exit0hole`: task `probe` (succeeds) with `on-failure: ["up:alpha"]`,
  plus the serial group. `probe` succeeding makes up:alpha's `when: failure`
  edge unreachable, so the whole group is skipped with nothing failed. At
  `10eb334`, `otto up` prints only `[probe] probe ok` / `[probe] finished
  successfully`, exit 0 — the entire group vanishes silently. At `121f384` the
  exit code is unchanged (0) but the skips are now visible:
  `[up:alpha] skipped (dep probe succeeded; this task required when: failure)`,
  `[up:beta] skipped (serial predecessor up:alpha skipped; cascade)`,
  `[up] skipped (dep up:alpha skipped; cascade)`,
  `[up:gamma] skipped (serial predecessor up:beta skipped; cascade)`. Every
  branch of Phase 4 criterion (c) exits non-zero; this fixture is outside
  criterion (c) and is the only shape found where a group is skipped at exit 0.
  Whether a run that skips everything it was asked to do should exit 0 is a
  run-outcome policy question, not a serial-foreach question.

## Orchestrator note after Phase 4 (2026-08-29)

### Design decisions
- None (no code written at this level).

### Deviations
- None.

### Tradeoffs
- None.

### Open questions
- **Pre-existing silent-success hole, recorded not fixed, remediation scope.** A
  requested task can run nothing and still exit 0 when an edge goes Unreachable
  without anything failing, because `final_error` is never set. Reproduced at
  BOTH `10eb334` (pre-Phase-4) and `121f384` (post), so Phase 4 did not
  introduce it. Fixture: a `probe` task that succeeds, with
  `on-failure: ["up:alpha"]` pointing at the first member of a serial foreach
  group. `otto up` at `121f384`:

  ```
  [probe] probe ok
  [probe] finished successfully
  [up:alpha] skipped (dep probe succeeded; this task required when: failure)
  [up:beta] skipped (serial predecessor up:alpha skipped; cascade)
  [up] skipped (dep up:alpha skipped; cascade)
  [up:gamma] skipped (serial predecessor up:beta skipped; cascade)
  EXIT=0
  ```

  The same run at `10eb334` printed only the two `probe` lines and exited 0: the
  entire group vanished with zero output. Phase 4's visible-skip change did not
  create the hole, it made it legible. Whether "requested task ran nothing"
  should be non-zero is a whole-run exit-code policy question and belongs with
  the remediation doc's other exit-code work, not here.

### Acceptance-criteria amendments (doc defects found during Phases 3-4)
- **Phase 3 spec body, "unmatched `$(` is a loud config error"** — amended in
  place in the design doc. The doc demanded a fatality while the same doc
  assigns the fatal-vs-warn call-site policy (defect 2's whole-map abandonment)
  to the remediation doc; the bullet contradicted its own scoping. The error is
  raised and names the key and value; it is demoted to a warning at all three
  call sites (`src/cli/parser.rs:712`, `src/cli/parser.rs:159`,
  `src/executor/task.rs:122`). Evidence recorded in the doc.
- **Phase 4 Architecture aggregation bullet and Phase 4 criterion (c),
  "parent aggregates Failed"** — amended in place. The bullet was marked
  "verified" and was not. The parent aggregates to `Skipped`, identically at
  `10eb334` and `121f384`: `classify_edge` short-circuits a Skipped source to
  `EdgeState::Unreachable` at `src/executor/scheduler.rs:135`, above the `when:`
  match, so the failed-first branch at `:545-549` is unreachable and the virtual
  parent never executes. Exit code is non-zero either way, sourced from the
  failed subtask. Phase 4 preserved behavior per the same bullet's other
  instruction and pinned `Skipped` in its test.

## Phase 5: `--tasks`

### Design decisions
- Added `--tasks` and `--format {yaml,json}` to `global_args()` (single
  source of truth from Phase 1), handled in `Parser::parse()` right after the
  `--list-subtasks` block — `src/cli/parser.rs`. Both flags render in the
  Phase 1 help snapshot; `EXPECTED_GLOBAL_OPTIONS_HELP` was updated to include
  them (disclosed churn, the snapshot test's whole point).
- New module `src/cli/commands/tasks.rs` holds the dedicated serde view
  (`TaskView`/`ParamView`/`EdgesView`/`TasksView = BTreeMap<String, TaskView>`),
  `build_tasks_view()`, `choose_format()` (the tty-detect seam), and
  `render_tasks_view()`. Kept out of `parser.rs` to match the one-file-per-
  builtin convention already used for Clean/History/Stats/Upgrade under
  `src/cli/commands/`, even though `--tasks` is a global flag rather than a
  dispatched subcommand.
- `BTreeMap` (not `HashMap`) for the view so JSON and YAML key order is
  identical and deterministic — load-bearing for the "one logical shape, two
  encodings" contract and for tests that diff key sets between formats.
- `choose_format(explicit: Option<&str>, stdout_is_tty: bool) -> TasksFormat`
  is a pure function — `src/cli/commands/tasks.rs:135` — so the tty branch is
  unit-testable without a real terminal. The call site supplies
  `atty::is(atty::Stream::Stdout)` (same crate/call `src/app.rs:236` already
  uses for `--tui`'s tty gate).
- Builtins excluded by filtering `is_builtin(name)` on the already-injected
  `self.config_spec.tasks` map, rather than snapshotting task names before
  `inject_builtin_commands()` runs. Simpler, and doesn't require reordering
  the existing `parse()` flow.
- Foreach subtask ids resolved the same way `print_subtasks()` already does:
  `foreach.resolve_items(self.base_dir())` per parent task, formatted as
  `"{name}:{identifier}"`. No separate top-level entries — subtasks live only
  in the parent's `subtasks` array, satisfying the frozen contract.
- Zero-item foreach treated as a legitimate empty scope, not an error: prints
  `Notice: task '<name>' foreach matched 0 items` to stderr and contributes an
  empty `subtasks` array. This generalizes the Phase 6 `foreach: command:`
  posture ("empty output is a notice, not an error") to glob/items/range for
  Phase 5, and gave a real, controllable notice source to prove criterion (c)
  ("stdout parseable even when a notice fires") against actual `eprintln!`
  output rather than the log-file-only `log::warn!` that `resolve_items`
  already emits internally (verified: `env_logger` in `src/main.rs` pipes to
  a log *file*, not stderr, so that existing warning could not have served as
  the stderr-notice proof).
- Any foreach resolution *error* (not empty-match) aborts the whole `--tasks`
  view: nothing is printed to stdout, the error goes to stderr, exit 1. This
  extends the frozen contract's "a command-source resolution failure exits
  non-zero with nothing on stdout" line — written with Phase 6 in mind, before
  `foreach: command:` exists — to glob/items/range failures too, so `--tasks`
  has one failure policy instead of two (see Deviations).

### Deviations
- The frozen contract paragraph names "a command-source resolution failure"
  specifically (Phase 6 doesn't exist yet). Phase 5 applies the same
  fail-closed, no-partial-stdout rule to *any* foreach resolution error
  (glob/items/range), not only future command sources. Same effect, broader
  seam: one resolution-failure policy for the whole function instead of
  reintroducing print_subtasks's per-task "warn and continue" behavior for
  `--tasks` specifically. When Phase 6 lands, command-source failures fall
  through this same path with no further change needed.
- `--format` was added to `global_args()` as a second new flag beyond the
  `--tasks` the task description named explicitly. The design doc's API
  Design bullet and Resolved Decisions both require `--format {yaml,json}` as
  part of `--tasks`'s contract, and Phase 1's "single source of truth" intent
  applies equally to any new global flag, so it was added there rather than
  declared ad hoc at the `--tasks` handling site. No id collision with a
  task's own `--format` param: clap's external-subcommand split means a
  top-level `--format` is only consumed when it appears *before* the task
  name, i.e. only in `otto --tasks --format yaml`-shaped invocations where no
  task is ever named.

### Tradeoffs
- Dedicated view module (`src/cli/commands/tasks.rs`) vs. inlining the
  structs/functions in `parser.rs` next to the flag handling (which is where
  `--list-subtasks`'s `print_subtasks()` lives) — chose the module to keep
  `parser.rs` from growing further and to give the view types/tests a home
  that mirrors the other builtin-command modules; the tradeoff is the
  handling in `parser.rs` now calls out to `crate::cli::commands::tasks::*`
  by full path rather than a local private fn.
- Sorting `params` by name inside `build_tasks_view()` (params are stored in
  a `HashMap` on `TaskSpec`, so iteration order is otherwise nondeterministic)
  — small, cheap, and removes a source of flaky test/output diffs; the design
  doc's JSON example doesn't specify param order, so this doesn't contradict
  anything.
- Used `assert_cmd`'s `cargo_bin_cmd!` macro (spawns the real binary) for all
  of `tests/tasks_flag_test.rs` rather than calling `Parser::parse()`
  in-process (the pattern most existing integration tests use) — required,
  because `--tasks` calls `std::process::exit()` on both success and failure
  paths, which would kill the test process if invoked in-process.

### Open questions
- None.

### Verification

`otto --tasks` (piped, default JSON) against a fixture with a plain task
(`down`) and a serial-foreach task (`up` over `[alpha, beta]`):
```json
{
  "down": { "help": "Stop each service", "params": [], "edges": {"before": [], "after": ["up"]}, "subtasks": [] },
  "up": {
    "help": "Build + start each service in scope",
    "params": [{"name": "svc", "flags": ["-s", "--svc"], "choices": [], "default": null, "positional": false}],
    "edges": {"before": [], "after": []},
    "subtasks": ["up:alpha", "up:beta"]
  }
}
```
`jq -e 'type == "object" and (keys | length > 0)'` on that output: `true`,
exit 0.

`otto --tasks --format yaml` on the same fixture: top-level keys `down`, `up`
— identical set to the JSON default, confirmed by both a unit test
(`render_tasks_view_json_and_yaml_share_key_set`) and an integration test
(`tasks_yaml_format_has_same_keys_as_json_default`) that diff the sorted key
lists from two real process invocations.

TTY branch (real pty via `script -qec`, test
`tasks_defaults_to_yaml_on_a_real_tty`): with no `--format` given and stdout
attached to a pty, output does not start with `{` and parses as YAML with
keys `down`, `up` — proves the tty-default branch fires, not just the piped
one.

No-builtin-leak / subtask-nesting (test
`tasks_reports_subtask_ids_and_excludes_builtins`): none of `Clean`,
`Convert`, `Graph`, `History`, `Stats`, `Upgrade` appear as keys; `up:alpha`
and `up:beta` are absent as top-level keys and present in `up.subtasks`.

Sentinel test (`tasks_executes_no_task_body`): fixture task's `bash:` touches
a sentinel file; after `otto --tasks`, the sentinel does not exist.

Notice-on-stderr-only (`tasks_notice_goes_to_stderr_stdout_stays_pure_data`):
fixture with a glob foreach matching nothing; stderr contains `Notice:`,
stdout still parses as JSON with `orphan.subtasks == []`, exit 0.

Bite demonstration (house rule): broke `choose_format`'s tty branch to always
return `Json` — `choose_format_defaults_to_tty_detection` (unit) and
`tasks_defaults_to_yaml_on_a_real_tty` (integration, real pty) both failed
with the expected `Json` vs `Yaml` mismatch; reverted, both green again.
Separately, commented out the `is_builtin` filter in `build_tasks_view` —
`build_tasks_view_excludes_builtins` (unit) and
`tasks_reports_subtask_ids_and_excludes_builtins` (integration) both failed,
reporting `Clean`/`History`/etc. leaking into the key set; reverted, both
green again.

`otto ci`: green (`cargo check`, `clippy -D warnings`, `cargo fmt --check`,
full test suite). One unrelated pre-existing flaky test,
`executor::scheduler::tests::test_file_dependencies_timestamp_precision`
(hits `Permission denied` writing to a hard-coded `/otto-home` path when run
under `cargo test`'s default parallelism, alongside another test that
mutates `OTTO_HOME`/env state), failed once and passed both in isolation and
on two subsequent full-suite reruns; not touched by this phase and not
related to `--tasks`.

Commit: `e647d48`.

---

## Phase 6: `foreach: command:`

### Design decisions

- **One resolver object, not a bare cache** — `src/cfg/resolver.rs`
  (`DynamicResolver`, `run_lines_command`) — the doc asked for "an
  interior-mutable side cache or resolver object" on `Parser`. It is a new
  module holding (a) the memoized resolved global `envs:`, (b) the per-task
  foreach item cache, and (c) the one execution contract every dynamic source
  shares (`sh -c`, ottofile-dir cwd, inherited env + globals, recursion-guard
  env var). Phase 6b adds a `task:param` map beside the foreach map and reuses
  `run_lines_command` with `OTTO_CHOICES_COMMAND` as the guard var; no shape
  change needed there.
- **Global env evaluation is memoized, not physically moved** —
  `Parser::global_envs` (`src/cli/parser.rs`) — the doc's ordering constraint
  was "move global env evaluation ahead of partitioning" so a partition-time
  resolution sees resolved globals. Implemented as a `OnceCell` accessor that
  both the partition path and `process_tasks_with_filter` read: whichever site
  needs a command source first forces evaluation before the command runs, and
  globals do not depend on task args so the values are identical either way.
  Same effect, correct seam, and it also means `--tasks`/`--list-subtasks` on
  a static-only ottofile still evaluate no global envs at all.
- **The lazy trigger is two predicates, not one** — `Parser::args_mention_task`
  and `Parser::reachable_task_names` — partition time asks "do the raw args
  name this task or a `task:` token"; expansion time asks "can the run set
  reach this task", walking `before:`, `after:`, and the inverted `after:`
  relation over the *unexpanded* specs (the same three relations
  `collect_transitive_deps` walks, so it is a superset of the run set).
- **A deferred command source is proven unreachable, not assumed** —
  `process_tasks_with_filter` — tasks left unresolved are collected in
  `deferred_foreach`, and if any of them (or a subtask of one) turns up in
  `tasks_needed`, otto errors instead of silently scheduling an empty virtual
  parent. Fail-loud backstop for the reachability analysis.
- **`resolve_items` no longer answers for a command source** —
  `ForeachSpec::resolve_items` returns an internal error naming the resolver
  if reached with `command:` set. That makes "every production call site was
  converted" a mechanically enforced claim rather than a review claim; the
  static paths are byte-identical to before.
- **Mutual exclusion validates at load, not only at resolve** —
  `Parser::validate_foreach_sources` beside `validate_no_builtin_params` — a
  shape check executes nothing, so the config error is loud on every surface
  including `--help`, where a resolve-time-only check would stay invisible.
  `ForeachSpec::resolve_command_items` re-checks, so the error also fires for
  any caller that skips load validation.
- **Command stderr is forwarded on success, folded into the error on failure**
  — `run_lines_command` — stdout stays pure data either way.

### Deviations

- **`--tasks`/`--list-subtasks` error printing changed from `{e}` to `{e:#}`**
  (`src/cli/parser.rs`). Phase 5 wraps a foreach failure with
  `task 'up': failed to resolve foreach items`; printing only the outermost
  message dropped the actual cause, so `--tasks` on a failing command source
  said nothing about the exit code. `{e:#}` prints the eyre cause chain. This
  is a Phase 5 surface touched by Phase 6, disclosed as churn; it strictly adds
  information and no test pinned the old truncation.
- **`build_tasks_view`'s signature changed** from `(tasks, base_dir)` to
  `(tasks, resolve)` (`src/cli/commands/tasks.rs`), taking the parser's
  resolver instead of re-resolving from a path. Its five unit tests pass a
  `static_resolver` helper; behavior for static sources is unchanged.
- **The doc's `parser.rs:1303` help call site is replaced only for command
  sources.** Static foreach help still renders `[N items]` (pinned by a new
  test); only a command source renders `[dynamic]`. The doc's wording
  ("replacing the `parser.rs:1303` call") reads as unconditional; removing the
  static count would be a gratuitous help regression.
- **No Phase 1 help-snapshot churn after all.** The phase brief expected the
  `[dynamic]` rendering to churn the Phase 1 snapshot test; it did not, because
  that test pins the global-flags block against an empty config_spec and never
  renders a task's `about` line. Snapshot untouched, still green.

### Tradeoffs

- **Reachability over "expand only the run set"** — computing the run set
  requires the expanded specs, which is the chicken-and-egg this phase had to
  break. A superset walk over unexpanded specs plus the loud post-check is
  strictly safer than pruning after the fact, at the cost of resolving a
  command source for a task that is reachable but ends up skipped by a
  conditional edge.
- **Cache only command sources** — glob/items/range still resolve at each call
  site exactly as before. Caching them would be free performance but would make
  "glob/items/range behavior is untouched" a weaker claim.
- **Recursion guard keyed by task name, chained with commas** — an inner otto
  resolving a *different* task's command source is allowed (and its own name is
  appended), so nesting is only blocked at a real cycle. A same-named task in
  an unrelated inner ottofile is a false positive; naming the cycle in the
  error makes that diagnosable, and fail-closed is the right default.
- **The empty-scope notice comes from the resolver, not the surface** — so
  `--tasks` prints one notice for a command source instead of its own
  `matched 0 items` line; the two would otherwise double up on stderr.

### Open questions

- **`otto --help` does not honor `-o/--ottofile`** (pre-existing: the help path
  re-divines from `.` at `src/cli/parser.rs:307-309`, assigned to the
  remediation doc by this doc's Phase 0 list). Consequence for Phase 6: the
  `--help` counter test has to run from the fixture directory, and a user
  pointing `-o` at another repo's ottofile sees this repo's help. Not fixed
  here; flagged because the `[dynamic]` guarantee is verified through that
  workaround.
- **`otto --tasks` on a command-sourced foreach executes user code.** That is
  the designed contract (enumeration surfaces resolve), and it is documented in
  `docs/commands/tasks.md`. Worth a second look when the architecture doc's
  staged pipeline lands, since `--plan` will face the same call.

### Verification

Criteria, run against a fresh `target/debug/otto` from this tree:

- (a) doc fixture `foreach: {command: "printf 'alpha\nbeta\n'", as: svc,
  parallel: false}`: `otto up` -> `[up:alpha] up alpha`, `[up:beta] up beta`,
  exit 0. `otto up:beta` runs beta only (Phase 4 serial-group ordering holds
  for a command source), and the full run keeps command line order.
- (b) counter fixture (command appends a line per execution):
  `otto up` -> 1, `otto up:beta` -> 1, `otto --tasks` -> 1,
  `otto --list-subtasks` -> 1, `otto build` -> 0, `otto --help` -> 0 with
  `up  Bring up each service [dynamic]`, `otto help up` -> 0,
  `otto up --help` -> 0.
- (c) `foreach: {command: "echo boom >&2; exit 7"}`: exit 1, stdout empty,
  stderr `Task 'up' foreach: command 'echo boom >&2; exit 7' failed with exit
  code 7: boom`. Same loudness on `--tasks`/`--list-subtasks` (exit non-zero,
  empty stdout, `exit code 3` in the message).

Also verified: `--tasks` reports `"subtasks": ["up:alpha","up:beta"]` for a
command source; recursion guard (`OTTO_FOREACH_COMMAND=up otto up` -> exit 1,
`cycle: up -> up`, while `... otto build` succeeds); mutual exclusion; duplicate
lines -> `duplicate subtask name 'up:a'`; zero lines -> exit 0 plus one stderr
notice; cwd = ottofile dir and inherited env + resolved global `envs:` reach the
command while task params do not.

Bite demonstration (house rule): (1) forced expansion to ignore the reachability
gate — `targeting_an_unrelated_task_never_executes_the_command` failed
(`left: 1, right: 0`) and `recursion_guard_...` failed too (the unrelated inner
run now resolved); reverted, green. (2) forced help to resolve instead of
rendering `[dynamic]` — `help_never_executes_the_command_and_renders_dynamic`
(integration) and `test_help_renders_dynamic_for_a_command_sourced_foreach`
(unit) both failed; reverted, both green.

`otto ci`: green (`[ci] ✅ All CI checks passed!`) — compile, clippy
`-D warnings`, `cargo fmt --check`, lint, full test suite. `cargo fmt` was
applied once by the pipeline's own `fmt-fix` task. Two `sccache: Operation not
permitted` aborts along the way were sandbox interference, cleared by
`sccache --stop-server`; not code failures.

Commit: `fb864a8`.

## Phase 6b: Dynamic param `choices`

### Design decisions
- `BuildMode { Help, Bind }` replaces the `cwd: Option<&Path>` discriminator — `src/cli/parser.rs::BuildMode` / `Parser::task_to_command` — the doc asked for an explicit mode and this is what makes the seam honest: `Help` is infallible and executes nothing, `Bind` returns `Result` because it may run a command. Both are now methods on `Parser`, so both modes read `self.base_dir()` and `self.resolver` instead of one of them being handed a path and the other nothing.
- `Parser::task_to_command_for_help` kept as an infallible wrapper — `src/cli/parser.rs` — so every help call site visibly cannot fail; the `expect` inside it documents the invariant rather than hiding a fallible path behind `unwrap_or_default`.
- One funnel, `Parser::param_choices(task, param, mode)`, serves both bind triggers (`task_to_command`'s arg construction and `propagate_params`' validation) — `src/cli/parser.rs` — which is *why* the two triggers cost one execution rather than two; the cache is only reachable through it.
- Resolution lives on `ParamSpec::resolve_choices_command` — `src/cfg/param.rs` — mirroring `ForeachSpec::resolve_command_items`, so both dynamic sources share `resolver::run_lines_command` and therefore share one execution contract (`sh -c`, ottofile dir, inherited env + resolved global `envs:`, recursion guard).
- Cache key is `task:param`, which is also the `OTTO_CHOICES_COMMAND` guard key — `src/cfg/resolver.rs::DynamicResolver::choices` — one string means one concept: "this param's value set, this invocation."
- `--tasks` emits exactly one of `choices` / `choices-command` per param — `src/cli/commands/tasks.rs::param_view` — modelled as two `Option` fields with `skip_serializing_if`, so the JSON/YAML mirrors the config-level mutual exclusion instead of showing an empty `choices: []` beside a command.

### Deviations
- The doc says Phase 6's `[dynamic]` change "already removes help's only use of `cwd` here". It did not: `task_to_command_for_help` still resolved *static* foreach sources against `cwd` for the `[N items]` count. Help mode therefore still needs the ottofile directory, which it now takes from `self.base_dir()` rather than a parameter. Same effect the doc wanted (no `Option`-as-mode-flag), correct seam.
- Bind mode no longer computes the foreach item count at all (it renders `[foreach]`). Binding never displays that string, and resolving it would walk the filesystem for nothing. Previously bind passed `cwd: None` and got `[foreach]` by accident; this makes the same outcome deliberate.
- Docs beyond the phase's letter: `docs/commands/tasks.md` (Phase 5 froze that contract and this phase changes its param shape, so leaving it would make the frozen contract wrong) and a `Dynamic Choices from a Command` section in `docs/flag-support.md`, next to the static `choices` section it belongs beside.
- `param_to_arg` and `task_to_command` became `&self` methods, so the five `Parser::param_to_arg` / three `Parser::task_to_command` unit-test call sites now go through a `test_parser()` helper. Mechanical; no assertion changed.

### Tradeoffs
- **Resolve when a task gets any CLI args** vs. **resolve only when the specific param is typed**: chose the former. Bind builds the whole clap `Command` up front, and clap owns validation; splitting the arg construction to defer one param's `value_parser` would mean re-implementing which arg the user supplied before clap has parsed. Cost is one extra execution in the narrow case "task named with other flags but not this one" — still exactly once, still only for a task actually being run.
- **`ParamView.choices: Option<Vec<String>>`** vs. **`#[serde(flatten)]` over an untagged enum**: chose the `Option` pair. Two fields with `skip_serializing_if` produce the identical wire shape with no flatten/untagged interaction to reason about, and the type still says "exactly one of these."
- **`choices_command: Option<String>` serialized unconditionally** (emitting `choices-command: null` for ordinary params) vs. **`skip_serializing_if`**: chose unconditional, matching every existing optional `ParamSpec` field (`dest`, `metavar`, `default`, `help`), which already emit `null`. Consistency inside the struct beat tidier output; the round-trip is unaffected either way.
- Zero-lines-is-an-error is asymmetric with `foreach: command:`, which treats an empty result as a legitimate empty scope. Deliberate and per the doc: an empty scope runs nothing, an empty value set makes a param unsatisfiable.

### Open questions
- Neither `otto --help` nor `otto <task> --help` honours `-o/--ottofile`: the help path re-divines the ottofile from `.` (`parser.rs` ottofile divining, remediation-doc scope, explicitly not moved into this doc). Phase 6 already worked around it with `current_dir`, and this phase's help counter test does the same. Nothing here depends on fixing it, but the workaround is now in two test files.
- The mutual-exclusion error surfaces on any invocation that loads config, but *not* on `otto --help`: the clap `DisplayHelp` branch swallows the config error and prints help with exit 2 (pre-existing behavior for every config error, not specific to this field). Criterion (c) is verified on a run invocation and on `--tasks`; whether `--help` should report config errors is a separate call.

### Verification
Success criteria, run against a fresh `target/debug/otto` from this tree:

- (a) `otto switch --svc beta` -> `[switch] switched to beta`, exit 0. `otto switch --svc nosuch` -> `error: invalid value 'nosuch' for '--svc <svc>'` / `[possible values: alpha, beta]`, exit 2. Propagated: `otto deploy --svc nosuch` (dependency `prep` owns the dynamic set) -> `Propagated value 'nosuch' for param 'svc' on task 'prep' (from task 'deploy') is not in allowed choices: [alpha, beta]`, exit 1; `--svc beta` runs `prep` then `deploy`, exit 0. PASS.
- (b) Counter fixture: `otto --help` 0, `otto help switch` 0, `otto switch --help` 0, `otto --tasks` 0, `otto switch --svc beta` exactly 1. `otto help switch` renders `-s, --svc <svc>  Service to switch to [dynamic choices: echo ran >> ...; echo alpha; echo beta]`. `otto --tasks` emits `"choices-command": "echo ran >> ...; echo alpha; echo beta"` with no `choices` key. Exactly-once also covered for propagation alone and for direct-binding-plus-propagation in one invocation (`deploy --svc beta prep --tag t1` -> 1). PASS.
- (c) Non-zero exit: `Task 'switch' param 'svc' choices-command: command 'echo boom >&2; exit 3' failed with exit code 3: boom`, exit 1. Zero lines: `Task 'switch' param 'svc' choices-command: command 'true' produced no values; a param whose valid set is empty can never be given a value`, exit 1. Both set: `tasks.switch.params: param 'svc': choices-command 'printf alpha' cannot be combined with choices [alpha, beta]; a param takes exactly one source of valid values`, exit 1. Round-trip: `config_with_choices_command_roundtrips` + `choices_command_emits_its_kebab_case_key_verbatim` green. PASS.

Tests added: `tests/dynamic_choices_test.rs` (14 integration tests), 8 unit tests in `src/cfg/param.rs`, 3 in `src/cfg/resolver.rs`, 2 in `tests/roundtrip.rs`.

Bite demonstration (house rule): (1) removed the `BuildMode::Help` early return from `param_choices` so help resolved — `help_surfaces_never_execute_the_command_and_render_the_marker` failed with `otto --help executed the choices command / left: 1, right: 0`; reverted, green. (2) changed `#[serde(rename = "choices-command")]` to `alias` (the deserialize-only trap the doc named) — `choices_command_emits_its_kebab_case_key_verbatim` and `choices_command_serializes_back_to_the_kebab_case_key` both failed (`emitted yaml lost the kebab-case key`); reverted, both green. Worth recording: the *structural* round-trip test (`config_with_choices_command_roundtrips`) stayed green under `alias`, because `alias` still accepts the snake-case form serde emits. The structural round-trip alone would NOT have caught this; the verbatim-emission assertions are what bite.

`otto ci`: green (`[ci] ✅ All CI checks passed!`) — compile, clippy `-D warnings`, `cargo fmt --check`, lint, full test suite. `cargo fmt` applied once after the first run flagged `fmt-check`.

Commit: `d021c41`.

## Phase 7: `tty: true`

### Design decisions
- One acquisition call site, not two — `src/executor/scheduler.rs::execute_task` — `semaphore.acquire_many(permits)` serves both cases, with `permits_for(tty, max_parallel)` deciding the count. `acquire_many(1)` is `acquire()`, so there is no tty-only branch to drift.
- `permits_for` is a free function returning `Result<u32>` — `src/executor/scheduler.rs` — so the precondition the doc asked for ("`acquire_many` requests exactly the initial permit count") is unit-testable rather than only observable through a race. It carries both debug assertions and converts `usize -> u32` loudly instead of clamping (a clamped request would wait forever and look like a hang).
- `max_concurrent` in the launch loop now reads `self.max_parallel` instead of `self.semaphore.available_permits()` — `src/executor/scheduler.rs::execute_all` — same value at that point today, but it removes the second place that treats live availability as the configured limit.
- `TaskSpec::tty` serializes only when `Some` — `src/cfg/task.rs::Serialize for TaskSpec` — an absent `tty:` round-trips as absent, so re-emitting an ottofile does not decorate every task with `tty: false` or turn "unset" into "explicitly off".
- `as_virtual_parent` drops `tty` — `src/cfg/task.rs` — the foreach parent runs no script; letting it inherit `tty: true` would make it take the exclusive permit to do nothing.
- The `--tui` conflict is checked before the existing "not a TTY, falling back" branch — `src/app.rs::execute_tasks` — otherwise the conflict would be silently resolved by the fallback whenever stdout is piped, which is exactly where tests and CI live.
- A failing tty task's error keeps the `Task <name> failed with exit code ...` shape the scheduler parses, and points at the log paths, but carries no stderr preview: nothing was captured, and echoing the marker line back as "error output" would be a lie.

### Deviations
- The doc's phrasing "skip `TaskStreams` entirely" left the log files unaccounted for; `TaskStreams::new` is what creates them. Added `write_tty_log_markers` so the paths history recorded at task start exist and contain the marker. Same effect the doc wanted (marker-line logs), the seam is a small writer rather than a TaskStreams variant.
- otto's own per-task status lines (`[login] finished successfully`, `[login] failed`) stay prefixed for tty tasks. The doc's "don't prefix" is about the task's output; those lines are otto speaking, and dropping them would remove the only signal that an uncaptured task finished. The prefix test asserts on task-written lines specifically.
- The `-j 0` precondition is asserted with `debug_assert!(max_parallel >= 1)` but is unreachable today, exactly as the doc predicts: re-verified on this build, `timeout 5 otto -j 0 login` exits 124 with zero output, hanging in the launch loop before any task is spawned.
- No user-facing docs added. There is no task-key reference doc in `docs/` to add `tty:` to (`on-failure:` from an earlier phase is likewise documented only in design docs), and the phase spec named none.

### Tradeoffs
- **Whole-semaphore `acquire_many`** vs. **a dedicated exclusivity mutex**: took the doc's `acquire_many`. It reuses the FIFO fairness already relied on (re-verified: a waiting `acquire_many` blocks later single-permit acquires, which the exclusivity and foreach tests both demonstrate with real timing), and adds no second synchronization primitive whose interaction with the semaphore would need reasoning about.
- **`tty: Option<bool>`** vs. **`tty: bool` with `#[serde(default)]`**: took the option, for the round-trip reason above. Cost is `unwrap_or(false)` at the two conversion sites; every consumer downstream sees a plain `bool`.
- **Test timestamps from bash's `EPOCHREALTIME`** vs. **`date +%s%3N`**: took `EPOCHREALTIME`. Root cause, found when the exclusivity test failed under `otto ci`: this box ships uutils coreutils 0.8.0, whose `date` ignores the `%3N` width modifier and emits variable-length nanoseconds, so stamps were not comparable as integers and the interval test was silently meaningless. `EPOCHREALTIME` is fixed-width microseconds with no external process.
- The exclusivity test carries a **control run** of the identical fixture without `tty:`, asserting the four tasks *do* overlap. It doubles the test's runtime (~1.5s total) and is what makes the main assertion mean something: it failed loudly on the bad-timestamp version, which is how the `date` bug was caught rather than shipped as a permanently-green test.

### Open questions
- `--tasks` does not report `tty`. Phase 5 froze that contract and Phase 6b amended it for `choices-command`; whether a machine consumer needs to know a task is interactive is a call for the `--tasks` surface, not this phase.
- The doc's Risks row 5 (a tty task wedging a run) is unmitigated by design: there is still no per-task timeout. Every fixture here is self-terminating, but nothing in otto stops a user's tty task from blocking a CI run forever.
- `--tui` plus a tty task now fails the whole run, including its non-tty tasks. The alternative (run the non-tty tasks under the TUI and refuse only the tty ones) is a partial-run semantic otto has nowhere else; flagging it as the deliberate trade the doc called it.

### Verification
Success criteria, run against a fresh `target/debug/otto` from this tree:

- (a) PASS. Under `script -qec`, the tty task prints `STDIN: tty` / `STDOUT: tty` / `hello from tty` unprefixed, while the sibling `build` task prints `[build] hello from build`; `tasks/login/stdout.log` and `tasks/login/stderr.log` both contain exactly `otto: tty task, output not captured`, and `tasks/build/stdout.log` contains `hello from build`. Pre-state on main was `STDIN: tty` / `STDOUT: NOT a tty`.
- (b) PASS. `a_tty_task_runs_exclusively_while_the_same_tasks_otherwise_overlap`: four tasks, `-j 4`, each stamping start/end into a shared timeline; no other task's interval overlaps the tty task's. Control run of the same fixture without `tty:` overlaps, proving the assertion can fail. `foreach_subtasks_inherit_tty_and_are_serialized_by_exclusivity` makes the same interval assertion across three `parallel: true` subtasks.
- (c) PASS. `otto --tui login` prints `--tui cannot run alongside a tty task; the TUI owns the terminal. Tasks declaring tty: true in this run: login. Drop --tui, or drop tty: true from the task.` and exits 1, with no `$OTTO_HOME` directory created at all. The integration test asserts the same via sentinel files that neither the tty task nor its non-tty sibling created.

Tests added: `tests/tty_task_test.rs` (6 integration tests), 2 round-trip tests in `tests/roundtrip.rs`, 5 unit tests in `src/cfg/task.rs`, 4 in `src/executor/scheduler.rs`, 2 in `src/executor/task.rs`.

Bite demonstration (house rule): (1) `permits_for` forced to return 1 for tty tasks — the precondition assertion fired first (`src/executor/scheduler.rs:56`), failing both `permits_for` unit tests and all 6 integration tests; forcing the *call site* to `permits_for(false, ...)` instead (which the assertion cannot see) failed the timing tests on their real payload: `gamma overlapped the tty task: tty=("interactive", 1787994783098336, 1787994783702180) other=("gamma", 1787994783096186, 1787994783500189)` and `tty subtasks overlapped: ("gamma", ...) and ("alpha", ...)`; reverted, green. (2) tty tasks routed back through the captured/prefixed path — the pty test failed with the exact pre-state the design doc recorded on main, `[interactive] STDIN: tty` / `[interactive] STDOUT: NOT a tty`, along with the marker, prefix, and failing-task tests (4 of 6 integration tests, plus the scheduler-level marker test); reverted, green.

`otto ci`: green (`[ci] ✅ All CI checks passed!`) — compile, clippy `-D warnings`, `cargo fmt --check`, lint, full test suite. One intermediate red was real and fixed: the exclusivity test's `date +%s%3N` timestamps (see Tradeoffs).

Commit: `10f00e6`.

## Phase 8: Small items

### Design decisions
- `--no-prefix` added via `global_args()` — `src/cli/parser.rs::global_args` — a plain `SetTrue` flag, no new builder-specific declaration, matching the "single source of truth" rule Phase 1 established; the pinned help-snapshot test (`test_help_global_flags_no_drift`) churned as designed and was updated.
- Prefix suppression lives entirely in the output layer, not the scheduler's decision logic — `src/executor/output.rs::TeeWriter::write` (via the new pure `format_terminal_output` helper) and `TaskStreams::process_output` gain a `no_prefix: bool` parameter alongside the existing `suppress_terminal: bool`. File logs were already prefix-free (colors and prefixes are terminal-only), so no file-writing code needed to change; only what reaches the terminal changes.
- `TaskScheduler` carries `no_prefix: bool` as a field defaulted to `false` in `new()`, toggled after construction via `set_no_prefix()` — `src/executor/scheduler.rs` — mirroring the existing `set_message_channel`/`set_task_streams` setter pattern instead of adding a 6th positional bool to `TaskScheduler::new()`. `new()` already has ~25 call sites (24 of them tests); a setter means zero of them needed touching, versus every one taking a new argument for a flag they don't exercise.
- `execute_tasks`/`execute_with_terminal_output` in `src/app.rs` gained a `no_prefix: bool` parameter and thread it to `scheduler.set_no_prefix(...)` before `execute_all()`. `execute_with_tui` was deliberately left untouched: TUI mode already sets `suppress_terminal = true` unconditionally in the scheduler, so `--no-prefix` has nothing to act on there (no terminal prefix is ever printed while the TUI owns the screen) — documented at the call site rather than silently ignored.
- Positional-parameter documentation lands as a new page, `docs/commands/positional-parameters.md`, matching the house shape of `docs/commands/{tasks,clean,history,stats}.md` (Usage / declaration example / the one sharp edge / See also), plus a short "Task Parameters" section in `README.md` (README had no prior task-declaration examples of any kind, so this is new content, not an edit to an existing section) and a cross-link from `docs/flag-support.md`'s existing `ParamType::POS` paragraph, which already described the divining rule but not the collision edge.

### Deviations
- None from the doc's Phase 8 body. `--no-prefix` is opt-in exactly as specified (no `OTTO_TASK` auto-detection); positional args are documented, not modified.
- Constructor-vs-setter for `TaskScheduler.no_prefix`: the doc doesn't specify a signature here (it only names `src/executor/output.rs:98-106` as the target). Setter is the correct seam given the existing `tui_mode`-vs-`set_message_channel` precedent already established in the same struct; recorded as a deviation from "just add a parameter" only in the sense that no signature was actually specified to deviate from.

### Tradeoffs
- Extracted `format_terminal_output` as a free, pure function rather than testing `TeeWriter::write` end-to-end through real `print!`/`eprint!`. `write()` prints to the real process stdout/stderr, which isn't capturable from a unit test without a global mutex or a fake-stdout crate; the pure function isolates exactly the prefix-selection logic Phase 8 is about, at the cost of not exercising the surrounding I/O in the same test (that path is already covered by `test_output_processing`/`test_multiple_streams`, unaffected by this phase).
- `docs/commands/positional-parameters.md` as a new page vs. folding the material into the existing `docs/flag-support.md`: took the new page because the amended success criterion explicitly scopes the grep to `docs/commands/ README.md` (not `docs/flag-support.md`), and because the house `docs/commands/` pattern is one page per user-facing surface; `flag-support.md` gets a cross-link instead of the primary content.

### Open questions
- None.

### Verification
Success criteria, run against a fresh `target/debug/otto` from this tree (`otto v1.2.6-9-g10f00e6`, no version bump yet):

- (a) PASS. `otto --no-prefix hello` (fixture task `echo "hello from task"`) prints `hello from task` with no `[hello]` prefix; the same fixture without the flag prints `[hello] hello from task`. otto's own per-task status line (`[hello] finished successfully`) stays prefixed either way — that line is otto speaking about the task (scheduler.rs:742, `colorize_task_prefix` called directly, not through `TeeWriter`), not task output, and the doc's target was `src/executor/output.rs:98-106` specifically. Noted here rather than silently expanding scope.
- (b) PASS. `rg -l 'Positional parameters' docs/commands/ README.md` returns `README.md` and `docs/commands/positional-parameters.md`, exit 0 (pre-state on main: no hits, exit 1, per the doc's amendment).
- tty interaction (doc-requested check, not a numbered criterion): built a fixture with a plain task and a `tty: true` task, ran `otto --no-prefix -j 1 plain login` under `script -qec`. The plain task's output was unprefixed (the flag's effect); the tty task's output was unprefixed as it always is (tty tasks bypass `TaskStreams`/`TeeWriter` entirely via `Stdio::inherit()`, so `no_prefix` never reaches that code path). Coherent, not double-handled: the two mechanisms don't overlap because tty output never passes through the prefixing code at all.
- Positional-args sharp edge, verified on a real fixture (not asserted from the design doc's prose): ottofile with `sw` (positional param `svc`) and `philo` (unrelated task) in the same file. `otto sw philo` mis-partitions: `sw` runs with `svc` unbound and fails (`script.sh: line 17: svc: unbound variable`), and `philo` runs as its own separate task — reproduced exactly as the doc predicted. `otto sw someservice` (no collision) correctly binds `svc=someservice`. Both transcripts are in the new doc page verbatim.

Tests added: `src/executor/output.rs` — `test_no_prefix_omits_task_prefix`, `test_prefix_present_by_default` (both against the new `format_terminal_output` helper); existing `test_output_processing`/`test_multiple_streams` updated for the new parameter. `src/cli/parser.rs` — `EXPECTED_GLOBAL_OPTIONS_HELP` snapshot updated to include the `--no-prefix` entry (`test_help_global_flags_no_drift`). `src/app.rs` — `test_runtime_config_fields` updated for the new `no_prefix` field. 13 pre-existing test-only tuple destructures of `parser.parse()`'s return value (across `src/cli/parser.rs` and 6 files under `tests/`) grew a trailing `_` for the new 6th tuple element; no assertions changed.

Bite demonstration (house rule): forced `format_terminal_output` to always render the prefix (ignoring `no_prefix`) — `test_no_prefix_omits_task_prefix` failed with `left: "[loud-task] hello\n"` vs `right: "hello\n"`; reverted, green. Separately, before updating the pinned constant, running `test_help_global_flags_no_drift` against the new `--no-prefix` arg (added to `global_args()` but with the old snapshot still in place) failed all three builder assertions with the drifted `Options:` block shown in the diff; updated the constant, green.

`otto ci`: green (`[ci] ✅ All CI checks passed!`) — compile, clippy `-D warnings`, `cargo fmt --check`, lint, full test suite (one `cargo fmt` pass was needed after the initial edits; re-ran clean).

This is the final phase; its commit also flips this design doc's `Status:` to `Implemented` (see the doc itself for the line).
