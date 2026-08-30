# Design Document: Strict Ottofile Schema

**Author:** Scott A. Idler
**Date:** 2026-08-29
**Status:** Implemented
**Review Passes Completed:** 5/5 + panel rounds 1-3 (consensus reached)

## Summary

otto silently ignores unknown keys in an ottofile. A misplaced or misspelled key is accepted, discarded, and the run proceeds with the opposite of what the file asked for. This doc makes unknown keys a loud config error and makes the `otto.api` schema version load-bearing, in that order, because strict rejection without a version gate turns every future field addition into a break for anyone on an older otto. Both halves copy `borg` (second-brain), which already solved them. Measured blast radius: 7 of 24 in-repo examples, and 4 of the 159 ottofiles outside this repo under `~/repos`, every one of those a real bug that exists today.

## Problem Statement

### Background

- `otto v1.3.0` (`5640628`) made `parallel: false` the headline knob for serial `foreach` bring-up.
- Nothing in `src/` uses `deny_unknown_fields`. serde drops unrecognized keys.
- `otto.api` is parsed, defaulted to `"1"`, and round-tripped. It gates nothing. `api: 99` and `api: "2.5"` both run clean at HEAD.
- Found during the `2026-08-28-boundary-fixes-and-dynamic-foreach` implementation audit: the auditor wrote `parallel:` at the wrong level, otto ran the subtasks in parallel, and nothing said a word.
- This is not new scope invented here. `docs/design/2026-06-10-code-review-remediation.md:213` already specifies `deny_unknown_fields` on these structs. See Relationship to In-Flight Docs.

### Problem

**1. A correctly-spelled key at the wrong level is silently discarded.**

```yaml
tasks:
  up:
    parallel: false        # belongs under foreach:, silently dropped
    foreach: {items: [alpha, beta, gamma], as: svc}
    bash: |
      echo "start ${svc}"; sleep 0.3; echo "end ${svc}"
```

All three subtasks run concurrently. Exit 0, no warning. The user asked for serial and got parallel.

**2. A misspelled key is silently discarded.** `examples/dependency-ordering/otto.yml` ships `otto.task` (typo for `tasks`). Our own example carries the bug class.

**3. An invented key is silently discarded.** Top-level `envs:` is not a thing. Two live work repos declare it:

- `work repo A/.otto.yml` sets `GOMODCACHE`, `GOCACHE`, `VERSION`, `BUILD` at top level, under a `# Environment variables` comment. None have ever applied.
- `work repo B/.otto.yml` sets `PROJECT_NAME` the same way.

Neither is a misreading of the docs: **the README never mentions `envs` at all, and no ottofile key reference exists.** The key was invented because nothing documented the real one and nothing rejected the wrong one.

**4. `otto.api` cannot do its job.** When strict parsing lands, an old otto meeting a newer ottofile reports `unknown field 'tty'`. That message is true and useless. The operator needs "this file declares api: 2, I understand 1, upgrade otto". Without a live `api:`, strict rejection makes every future field addition a break for anyone behind.

### Goals

- Unknown keys are a loud, fail-closed config error naming the field and its path.
- `otto.api` gates the parse, checked BEFORE the strict parse, against a supported set.
- Migrate every in-repo ottofile and fixture so the repo ships clean.
- An ottofile key reference doc, because its absence is the documented root cause of problem 3.

### Non-Goals

- **Fixing the three affected work repos.** Different repos, different owners. Recorded as an operator step with the exact one-line fix each needs.
- **`divine()` key validation.** `divine()` parses the params-map *key string* (`-v|--verbose`); `deny_unknown_fields` governs the *value struct*. Orthogonal. `divine` silently accepting garbage keys (`-verbose` -> positional, two longs concatenate) stays remediation `:217` scope. Say so out loud so nobody assumes this doc covered it.
- **The "did you mean, wrong level" hint.** Parked with a revisit condition, see Addendum.
- **The rest of the remediation doc.** Only its Phase 6 bullet moves.

## Relationship to In-Flight Docs (ship order)

`docs/design/2026-06-10-code-review-remediation.md` (untracked working-tree draft, Status: In Review, nothing landed) already claims this work:

- `:213` (Phase 6) calls for `deny_unknown_fields` on `TaskSpecHelper`, `OttoSpec`, `RetentionSpec`, `ForeachSpec`, `ConfigSpec`.
- `:276` calls for negative tests.
- `:288` records the behavior-change caveat.

**Decision: this doc ships first and owns those three bullets.** Same argument the `2026-08-28` doc used: small, reproduced, driven by a live defect, while the remediation doc stays large and unbuilt after 2.5 months. Phase 0 marks all three superseded, exactly as `2026-08-28` Phase 0 did.

What stays in remediation, annotated as staying: the `divine()` garbage-key hazard (`:217`), the "strings where types belong" indictment (`:28`), and the unused-`levenshtein` note (`:153`), which this doc uses but does not resolve.

Cross-repo blast radius: **three work repos** (`work repo A`, `work repo B`, `work repo C`) will fail to load after this ships until each gets a one-line fix in its own repo. That is the ship order this doc forces: land here, then file the three fixes. No coordinated release; otto is the only thing that changes.

## Proposed Solution

### Overview

Six phases (1, 2, 3, 3b, 4, 5) plus a docs-only Phase 0 that carries no commit. Fixtures first, then in-repo ottofiles, then the api gate, then the help-fallback fix (added in panel round 1, hence 3b rather than a renumber), then the attributes, then docs. Deterministic and cheap first. The api gate lands BEFORE strict parsing so that the version error can win over the unknown-key error. Every phase one commit, `otto ci` green.

### Architecture

**Copy borg. Both halves are already solved there.**

| need | borg precedent | otto target |
|---|---|---|
| unknown-key rejection | `borg/src/config.rs:281-285` | six spec structs |
| version gate | `borg/src/harvest/contract.rs:47-54`, `:205-285` | `load_config_from_path` |

borg's `config.rs:281-285` is `#[serde(default, deny_unknown_fields, rename_all = "kebab-case")]` with the rationale in a doc comment: turn a stale key into a loud config-load error naming the field rather than a silently-ignored no-op. Its negative test asserts the message names the field: `test_distill_config_stale_article_transcript_key_fails_loudly`, `borg/src/config/tests.rs:831-848`. (Round 1 corrected this from `:809-820`, which is `test_distill_config_all_false_yaml_override`, a defaults test. One seat asserted the original cite was correct; the other read the lines and caught it.)

borg's `contract.rs` is the part that matters most here: `SUPPORTED_SCHEMA_VERSIONS: &[u32]`, documented as **"A SET, not a floor"**, with a tolerant `VersionHeader` struct parsed *before* the full parse, so a version mismatch surfaces as a clear "unsupported version" error instead of a confusing missing-field error deep in the payload. Then `bail!` naming both versions and telling the operator to upgrade.

That ordering is the whole design. Reversed, an old otto reading `api: 2` reports `unknown field 'whatever-is-new'` and the operator learns nothing.

Local precedent in otto: `src/cfg/edge.rs:76` already does `Error::unknown_field(other, &["task", "when"])` by hand. The house pattern exists; it was applied once and never generalized.

**Six structs take the attribute.** Five derive `Deserialize` directly (`ConfigSpec`, `RetentionSpec`, `OttoSpec`, `ForeachSpec`, `ParamSpec`; round 2 corrected this from four, which all three of us missed in round 1); `TaskSpec`'s hand-written impl (`task.rs:402-407`) does nothing but delegate to `TaskSpecHelper::deserialize`, and the helper (`task.rs:353`) is derived, so the attribute goes on the helper.

| struct | file:line |
|---|---|
| `ConfigSpec` | `src/cfg/config.rs:10` |
| `RetentionSpec` | `src/cfg/otto.rs:56` |
| `OttoSpec` | `src/cfg/otto.rs:107` |
| `ForeachSpec` | `src/cfg/task.rs:49` |
| `TaskSpecHelper` | `src/cfg/task.rs:353` |
| `ParamSpec` | `src/cfg/param.rs:43` |

No `#[serde(flatten)]` anywhere in `src/`, so nothing blocks the attribute.

**Free-form key sites, which strict parsing must NOT touch.** Exactly four (panel round 1 corrected this from three; the Risks table below rests on this inventory being complete):

- `ConfigSpec.tasks` keys -> task names (`deserialize_task_map`, `task.rs:685-711`)
- `envs` keys and values, both levels (`otto.rs:129`, `task.rs:371`)
- `TaskSpec.params` keys -> param titles, through `divine()` (`param.rs:343`, called at `:391`)
- `ParamSpec.constant` values -> an arbitrary YAML map, via a hand-written `visit_map` (`param.rs:249-258`) that accepts any key into a `HashMap<String, String>`. Verified loading clean under a patched strict build.

Everything else is fixed-field.

**Why `deny_unknown_fields` does not break those four sites, stated plainly because it is the first question an implementer will ask:** the attribute governs a struct's OWN field names. `tasks` is a *declared field* of `ConfigSpec`; the free-form-ness lives in its value type (a map). Same for `envs` on `OttoSpec`/`TaskSpec` and `params` on `TaskSpec`. `ParamSpec.constant` is the same shape one level down: `constant` is a declared field, and its arbitrary-map behavior lives in `Value`'s hand-written `visit_map` (`param.rs:249-258`), which the attribute never reaches. Strict parsing rejects an unknown key *beside* `tasks:`, never an unknown key *inside* it.

**serde_yaml already emits the error we want.** Probed through otto's real chain (derive -> custom map visitor -> hand-written `TaskSpec` -> derived helper):

```
tasks.up: unknown field `parallel`, expected one of `help`, `after`, `before`, `input`,
`output`, `envs`, `params`, `bash`, `python`, `action`, `foreach`, `on-failure`, `tty`
at line 3 column 5
```

Path, field, full expected set, line and column. No message layer needed to ship value.

### Data Model

- `OttoSpec.api` keeps its type. **No type change**: `serde_yaml` coerces `api: 1` into `String` fine, but switching to an int changes `otto convert`'s emitted form from `api: '1'` to `api: 1`, which is churn for no gain.
- New const `SUPPORTED_API_VERSIONS: &[&str]`, borg-shaped, documented as a SET not a floor.
- **Policy for growing the set, recorded because copying the mechanism without the decision record is how it rots** (borg documents its own at `contract.rs:40-47`): a new version is added when, and only when, otto makes a change that a prior otto would mis-execute rather than merely fail to understand. Adding an optional field does NOT bump it; strict parsing already rejects the unknown key with a truthful message. Renaming or re-typing an existing key, or changing what an existing key means, DOES. The old version stays in the set for as long as otto still executes it correctly, which is why it is a set and not a floor.
- **Deliberate deviation from borg:** borg's `VersionHeader.schema_version` is a required `u32`; `ApiHeader.api` is `Option<String>`. borg emits its own contracts and can require the field; otto's ottofiles are hand-written and `api:` is optional today (nine of the 190 files declare none). Requiring it would break them for no gain.
- New private `ApiHeader` struct for the tolerant pre-parse: `{ otto: { api } }`. It must NOT carry `deny_unknown_fields`, and every field is `Option` with a default: a file with no `otto:` block, or an `otto:` block with no `api:`, parses to `None` and is treated as the current version. The whole point is that this struct survives a document it does not understand.
- **Read the file once.** `load_config_from_path` reads to a `String`, runs the api check against that string, then typed-parses the same string. Do not read the file twice.
- `ParamSpec`'s five `#[serde(skip)]` fields become **rejected input** rather than ignored input. Verified: `name: foo` under a param errors after the change. Nothing in the repo or in the 159 external ottofiles writes them.

**Census predicate, stated so the numbers are reproducible:** `find ~/repos \( -name '.otto.yml' -o -name 'otto.yml' -o -name 'otto.yaml' \)` -> **190 files total, 159 outside `otto-rs/otto`**. Every count in this doc that says "external" means those 159. Of the 159, exactly 4 break under a patched strict build, and zero are already broken today.

### Edge cases, measured

- **The escape hatch is narrower than this doc first claimed, and its fallback lies about the cause.** Corrected after panel round 1; the original text asserted `--help` exits 0 and was wrong. Measured at `5640628` against a parse-failing `.otto.yml`: `otto --help` renders the global flag list but **exits 2**, the `Commands:` section is **absent**, the real serde error is **discarded**, and the output ends with `ERROR: No ottofile found in this directory or any parent directory!` -- otto tells the operator there is no ottofile when there is one and it is malformed. `otto <usertask> --help` exits 1 and prints no help, but it DOES print the correct parse error to stderr. `otto Clean --help` exits 0, but that proves nothing: `Clean` is a builtin and never needed the config. Source: `src/cli/parser.rs:417-421`, `Err(_) => { build_help_command_with_error(); exit(2) }` -- the error is bound to `_` and thrown away.
  **This doc owns the fix** (Phase 3b), because this doc is what routes more files into that branch. A message that misidentifies the cause is the exact failure class the api gate exists to prevent; shipping strict parsing on top of it would manufacture the disease it cures.
  It does **not** flip error-vs-warn: the global flag list still renders and `otto --tasks` still reports path, field, and line/column, so fail-closed stays affordable.
- **Enumeration surfaces fail loudly, as they should.** `otto --tasks` on the same broken file prints `tasks.up.before: invalid type: map, expected a sequence at line 7 column 13` and exits non-zero.
- **`otto Convert`'s own output stays loadable.** Checked, because a converter that emits a file its own parser rejects would be a self-inflicted break. `RetentionSpec` (`otto.rs:56`) declares `keep_days`, `keep_last`, `keep_failed`, `auto_prune`, `prune_interval_hours` as plain snake_case with no rename, and Convert emits exactly those. Safe. Phase 4 pins it anyway.
- **The schema mixes kebab and snake, and strict parsing sharpens that.** `on-failure` and `choices-command` are kebab; every `RetentionSpec` field is snake. Today writing `keep-days` is silently ignored and silently defaulted; afterwards it is a hard error. That is an improvement, but the inconsistency is now unforgiving and belongs in the Phase 5 reference doc.
- **Deliberate deviation from borg:** borg's line is `#[serde(default, deny_unknown_fields, rename_all = "kebab-case")]`. This doc copies `deny_unknown_fields` and **not** `rename_all`. Adding it would rename every snake_case key in the schema at once, which is a breaking change to every existing ottofile and is not what this doc is for. Unifying the convention is its own doc, with aliases and a deprecation window.
- **Recorded, out of scope:** `otto Convert` on a two-target Makefile emits `tasks: {}`, dropping both targets. Pre-existing, unrelated to this work, not fixed here. Noted because Phase 5 touches `converter.rs` and someone will see it.

### API Design

- No CLI surface changes. No new flags.
- `.otto.yml` gains no keys. It loses the ability to carry keys that never did anything.
- Two new error shapes on stderr, both exit 1:
  - `otto: unsupported api version '2' (this otto supports: 1). upgrade otto.`
  - `tasks.up: unknown field 'parallel', expected one of ... at line 3 column 5`

### Implementation Plan

**Phase 0 is not a spike.** The environmental assumption (can serde locate and name the offending key well enough to be useful) was measured during research, not assumed. Evidence is pasted in Architecture. Re-spiking it would be ritual.

#### Phase 0: De-duplicate the remediation doc
**Model:** sonnet
- Mark `docs/design/2026-06-10-code-review-remediation.md:213`, `:276`, `:288` as covered by this doc. Annotate `:217`, `:28`, `:153` as staying.
- Untracked working-tree draft; edits land in place, no commit of that file rides this plan.
- **Success criteria:** (a) `rg -c 'covered by docs/design/2026-08-29-strict-ottofile-schema' docs/design/2026-06-10-code-review-remediation.md` returns >= 3; (b) recorded manual check that the three not-moved items are annotated as staying.

#### Phase 1: Clean the inline test fixtures
**Model:** sonnet
- Delete the vestigial task-level `name:` line from the 15 inline fixtures in `src/cfg/param.rs`. It is not a `TaskSpec` field and never was.
- **Two literals, not one** (panel round 1; the original text named only the first, so anyone greping it would have fixed 8 and left 7): `grep -oP '^\s*name: \S+' src/cfg/param.rs | sort | uniq -c` returns **8 x `name: test_task`** and **7 x `name: switch`**. Both are task-level and both must go. The two unrelated hits, `name: String::new(),` and `name: "verbose".to_string(),`, are Rust struct literals: leave them.
- Lands green on current code, before any behavior change, so Phase 4's failures are only the ones Phase 4 causes.
- **Success criteria:** (a) under a **scratch build with the Phase 4 attributes applied**, `cargo test --lib cfg::param` reports zero failures (panel round 1: gating on unpatched code cannot bite, because deleting 0, 8, or 15 of these lines all pass identically until strict parsing exists; the scratch-build technique is the one Phase 2(a) already uses); (b) on unpatched `main`, the full suite still reports zero failures; (c) `git diff` touches nothing outside `#[cfg(test)]` blocks in `src/cfg/param.rs`.
  `Observed on main:` `cargo test --workspace --all-features --no-fail-fast` -> 730 passed, 0 failed. (Research reported 736; my count sums `test result:` lines across all binaries and gets 730. The discrepancy is unexplained and does not matter, which is exactly why the criterion asserts zero failures rather than a total.)

#### Phase 2: Migrate the in-repo ottofiles
**Model:** sonnet
- `examples/interactive-demo/otto.yml`: `interactive:` -> `tty: true` (7 occurrences; `tty:` superseded it in v1.3.0).
- `examples/basic-dependencies/otto.yaml`: drop `show`, `verbosity` under `tasks.example2`.
- `examples/dependency-ordering/otto.yml`: `otto.task` -> `otto.tasks`.
- `examples/complex-workflow/otto.yml`: root `name`/`description` -> `otto.name`/`otto.about`.
- `examples/old/ex1`, `ex2`, `ex3`: **delete.** All three lead with `defaults:`, a key that has never existed in any otto schema. Untouched since `8005fff`/`a2abe2d`, not test-covered, referenced only by `docs/flag-support.md` and the remediation doc. They document a schema otto never had. Update the `docs/flag-support.md` reference.
- **Success criteria:** (a) under a scratch build with the Phase 4 attributes applied, every remaining `examples/` ottofile both parses AND reports at least one task via `otto --tasks` (the task-count half is load-bearing: see the `ex2` evidence below); (b) `examples_integration_test` passes with zero failures; (c) `examples/_test_broken/otto.yml` still fails. **Not "for its original reason"** (panel round 1 falsified that): its message changes from `mapping values are not allowed in this context` to `unknown field 'invalid'` under strict parsing. The test survives because it only asserts that the file fails, and the criterion asserts the same.
  `Observed on main:` 24 ottofiles under `examples/`, **3 fail to parse today, before any change**: `examples/old/ex1/otto.yml` and `examples/old/ex3/otto.yaml` both die with `otto.tasks: invalid type: map, expected a sequence`, and `examples/_test_broken/otto.yml` fails by design (`mapping values are not allowed in this context`). `examples/old/ex2/.otto.yml` parses but `otto --tasks` returns `{}` -- **zero tasks, silently**. So two of the three `examples/old` files are already hard-broken and the third is silently inert. That is the deletion argument, measured.

#### Phase 3: Make `otto.api` load-bearing
**Model:** opus
- `SUPPORTED_API_VERSIONS` const, documented as a SET not a floor, per borg `contract.rs:47-54`.
- Tolerant `ApiHeader` pre-parse in `load_config_from_path` (`src/cli/parser.rs:2365`), **before** the typed parse. Ordering is load-bearing: reversed, the unknown-key error wins and the operator gets a useless message.
- Error names the declared version, the supported set, and the remedy, per borg `:205-285`.
- **Success criteria:** (a) `api: 2` errors naming both `2` and the supported set, exit non-zero; (b) `api: 1` and an absent `api:` are unchanged; (c) an ottofile with BOTH `api: 2` and an unknown key reports the version error, not the unknown-key error (this is the ordering assert, and it must bite).

#### Phase 3b: Truthful config-error help fallback
**Model:** opus
- Added in panel round 1. `src/cli/parser.rs:417-421` is `Err(_) => { build_help_command_with_error(); exit(2) }`. The config error is bound to `_` and discarded, and the fallback appends `ERROR: No ottofile found in this directory or any parent directory!` even when the ottofile exists and is merely malformed.
- **This doc owns it** because Phase 4 deliberately routes more files into that branch. Strict parsing on top of a fallback that misidentifies the cause would manufacture the exact defect the api gate exists to prevent.
- Bind the error, print the real one (serde already supplies field, path, and line/column), and stop claiming the file is missing when it is not. Distinguish the two states: no ottofile found vs. ottofile found and unparseable.
- Scope discipline: do NOT redesign the help fallback. `Commands:` staying absent is acceptable (the task list genuinely cannot be built from a config that did not load); exit 2 staying is acceptable. The lie is the defect.
- **Success criteria:** (a) with a parse-failing `.otto.yml` present, `otto --help` surfaces the real parse error and no longer claims `No ottofile found`. **Name the stream or the test is vacuous** (round 2): measured on `main`, global `--help` writes 928 bytes to **stdout** and 0 to stderr, and the false `No ottofile found` block is on **stdout**, emitted as a clap `after_help` epilogue (`src/cli/parser.rs:1763`). If the fix prints the serde error to stderr (the natural choice, matching `--tasks`), a test capturing only stdout passes while the user sees nothing new. Assert on both streams: the parse error present on whichever stream carries it, AND `No ottofile found` absent from stdout; (b) with NO ottofile anywhere up the tree, `otto --help` still says `No ottofile found`, unchanged; (c) **NON-REGRESSION, and labelled as such because it already passes on `main`:** `otto <usertask> --help` on a parse-failing file continues to print the parse error to stderr. Round 2 flagged the original wording ("rather than exiting 1 silently") as a criterion that cannot bite, since it holds before any work is done. **Why it is kept, which is more specific than "it happens to still pass"** (round 3): the global-`--help` path and the usertask path diverge at the `Err(_)` branch but SHARE `load_config_from_path` (`:2365`). The natural-but-wrong way to implement this phase is to change the error inside `load_config_from_path` -- enrich it, wrap it, re-type it -- rather than binding the discarded `Err(_)` at `:417-421`. That implementation moves the usertask path too, and this criterion is what catches it. Do NOT satisfy it by making task help render: that is the `Commands:` redesign this phase deliberately excludes.
  `Observed on main:` `otto --help` with a malformed ottofile exits 2, omits `Commands:`, discards the serde error, and prints `ERROR: No ottofile found in this directory or any parent directory!`. `otto up --help` exits 1 with **no help**, but stderr carries the correct diagnostic. Measured with streams separated (in `bash`; `zsh`'s MULTIOS corrupts this): stdout 0 bytes, stderr 76 bytes reading `tasks.up.before: invalid type: map, expected a sequence at line 7 column 13`. Round 2 caught this: my round-1 transcription turned "no help" into "no output", which is a different and false claim. `otto Clean --help` exits 0 (a builtin, which never needed the config, so it is not evidence of an escape hatch).

#### Phase 4: Strict parsing
**Model:** sonnet
- `#[serde(deny_unknown_fields)]` on the six structs in the table above.
- **One negative test per struct, all six** (panel round 1: the original list named five cases covering four structs, omitting `RetentionSpec` and `ParamSpec`; `ParamSpec` was the worst omission, since the Data Model makes its five `#[serde(skip)]` fields newly-rejected input, the most surprising behavior change in this doc):

  | struct | fixture | assert |
  |---|---|---|
  | `ConfigSpec` | root `envs:` | names `envs`, lists `otto`/`tasks` as expected |
  | `OttoSpec` | `otto.task` | names field AND path `otto` |
  | `RetentionSpec` | `otto.retention.keep-days` (kebab, where the schema is snake) | names field AND path `otto.retention` |
  | `ForeachSpec` | `tasks.up.foreach.itmes` | names field AND path `tasks.up.foreach` |
  | `TaskSpecHelper` | `tasks.up.parallel` | names field AND path `tasks.up` |
  | `ParamSpec` | `tasks.up.params.-s|--svc.name` | names field AND path, proving a `#[serde(skip)]` field is now rejected input |

- **The root-level case cannot assert a path**, and the criterion must not pretend otherwise: measured, the message is `unknown field 'envs', expected 'otto' or 'tasks' at line 8 column 1`, with no path prefix, because there is no parent. Assert field + expected-set + location for that one.
- **Amended at finalization, with evidence.** The `at line N column M` suffix above holds only when the unknown key is NOT the first key in the root mapping. When `envs:` is literally the first key, `serde_yaml`'s `deny_unknown_fields` error carries **no location at all**: the acceptance run against the doc's own envs-first repro produced exactly `unknown field `envs`, expected `otto` or `tasks``, exit 1, with no suffix. Confirmed against both the raw `serde_yaml::Error` and the eyre-wrapped one (identical, so not an eyre artifact). The shipped test fixture therefore orders `tasks:` before `envs:` so the location assertion has something to check; do not "simplify" that fixture or the location coverage is silently lost. The criterion itself is unaffected: it asserts field + expected-set + non-zero exit, all of which hold in both orderings.
- Doc comment on each attribute carrying the rationale, per borg `config.rs:281-285`.
- **Success criteria:** (a) full suite green and `tests/roundtrip.rs` unmodified; (b) the wrapped and unwrapped repro fixtures from Problem both exit non-zero with the field named; (c) with a load-failing ottofile, `otto --help` and `otto Clean --help` still render (the escape hatch), and `otto Convert -o .otto.yml` output still loads.

#### Phase 5: Documentation
**Model:** sonnet
- `docs/commands/ottofile-reference.md`: every key, its level, its type. Its absence is the root cause of problem 3, so this is a fix, not a chore.
- README gains an `envs:` example, since it currently never mentions the key.
- Migration note naming the three affected work repos and the exact one-line fix each needs.
- Update `src/makefile/converter.rs:33` if anything about `api` emission changed.
- **Success criteria** (panel round 1 rewrote all three; the originals were the weakest in the doc: `rg -c envs README.md >= 1` passes on a bare token mention, and gating on otto's own `.otto.yml` covers only 11 of ~39 schema fields, never touching `retention.*`, `foreach.*`, `tty`, `python`, `action`, `input`, `output`, `jobs`, `home`, `verbosity`, `choices-command`, `nargs`, `metavar`, `dest`, `constant`):
  (a) the README carries a working `envs:` block that a reader can copy, and that block parses under the strict build;
  (b) the reference doc names **every** field of the six structs in the Architecture table, enumerated from the struct definitions, plus all four free-form key sites, with a test that fails if a struct gains a field the reference does not mention;
  (c) every key appearing in any surviving `examples/` ottofile appears in the reference.

### Operator step, not a plan bullet

Three work repos need a one-line fix each, by a human, in their own repo, after this ships:

| repo | offending key | fix |
|---|---|---|
| `work repo A` | root `envs:` | move under `otto:` |
| `work repo B` | root `envs:` | move under `otto:` |
| `work repo C` | `tasks.dev.timeout` | delete (no such key) |

`scottidler/otto-old` is dead and needs nothing.

**The first two are not cosmetic.** Moving those keys under `otto:` makes them start working: `work repo A` would gain `GOMODCACHE`, `GOCACHE`, `VERSION`, and `BUILD` in every task for the first time. That is a behavior change in a work repo and needs testing there, by its owner. Deleting the block instead is the conservative option and preserves today's behavior exactly.

## Acceptance Criteria

Every criterion below was run against `main` at `5640628` (`v1.3.0`) and its output recorded.

- [ ] **Wrong-level key:** fixture with `tasks.up.parallel` alongside `foreach:`; `otto up` exits non-zero naming `parallel` and the path `tasks.up`.
  `Observed on main:` all three subtasks run concurrently (`start gamma`, `start beta`, `start alpha` before any `end`), exit 0, no diagnostic.
- [ ] **Invented root key:** fixture with root `envs:`; otto exits non-zero naming `envs` and listing `otto`/`tasks` as expected.
  `Observed on main:` `[show] PROJECT=[UNSET]`, exit 0. The key is accepted and discarded.
- [ ] **Version gate ordering:** fixture carrying BOTH `api: 2` and an unknown key; the error names the api version, NOT the unknown key.
  `Observed on main:` `[show] ran` / `[show] finished successfully`, exit 0. Neither is diagnosed.
- [ ] **Escape hatch tells the truth:** with an ottofile that fails to load, `otto --help` renders the global flag list AND reports the real parse error, and never prints `No ottofile found`. With no ottofile at all, it still prints `No ottofile found`.
  `Observed on main:` **fails today.** `otto --help` exits 2, omits `Commands:`, discards the serde error, and prints `ERROR: No ottofile found in this directory or any parent directory!` when the file exists and is malformed. `otto up --help` exits 1 with **no help**, but stderr carries the correct diagnostic. Measured with streams separated (in `bash`; `zsh`'s MULTIOS corrupts this): stdout 0 bytes, stderr 76 bytes reading `tasks.up.before: invalid type: map, expected a sequence at line 7 column 13`. Round 2 caught this: my round-1 transcription turned "no help" into "no output", which is a different and false claim. Panel round 1 falsified this criterion's original wording, which claimed exit 0 and treated `otto Clean --help` (a builtin) as evidence. Phase 3b fixes it.
- [ ] **Unchanged happy path:** otto's own `.otto.yml` runs `otto ci` green, and `tests/roundtrip.rs` passes unmodified.
  `Observed on main:` `otto ci` green; `tests/roundtrip.rs` 11/11.
- [ ] **Examples ship clean:** after Phase 2, every ottofile under `examples/` both parses and reports at least one task, with `examples/_test_broken` still failing (its message changes under strict parsing, which is fine: the test asserts failure, not a specific message).
  `Observed on main:` **21 parse, 3 fail** (`examples/old/ex1`, `examples/old/ex3`, `_test_broken`), and `examples/old/ex2` parses to `{}` -- zero tasks. The original wording of this criterion ("every ottofile in `examples/` parses") was FALSE on `main` before any change, and said nothing about the silently-inert case. Rewritten after being executed, which is the reason the gate exists.
- [ ] **Converter output stays loadable:** `otto Convert -o .otto.yml` on a Makefile produces a file otto itself loads without error.
  `Observed on main:` loads clean; emitted `retention:` keys are snake_case and match `RetentionSpec` exactly.

## Resolved Decisions

- 2026-08-29: **Error, not warn.** Measured: 4 of 159 external ottofiles under `~/repos` break, and every single one is a genuine bug, two of them silent misconfigurations live today. A warning that everyone learns to scroll past does not fix `work repo A`. House rule is fail loudly, fail closed; the measured hit rate says the cost is near zero.
- 2026-08-29: **The api gate ships in the same doc as strict parsing, and lands first.** They are one feature. Strict rejection without a version gate makes every future field addition a break for anyone on an older otto.
- 2026-08-29: **Copy borg** (`second-brain`) for both halves rather than inventing. Precedent found org-first, as the taste rules require.
- 2026-08-29: **`api` stays a `String`.** Int-typing works but changes `otto convert`'s emitted form for no benefit.
- 2026-08-29: **`examples/old/*` is deleted, not ported.** All three lead with `defaults:`, a key otto never had.
- 2026-08-29: **This doc supersedes remediation `:213`, `:276`, `:288`.** De-duplicated in Phase 0.
- 2026-08-29: **The "did you mean, wrong level" hint is parked**, not built. See Addendum for the revisit condition. Panel round 1 concurred and sharpened the reason: the strict message for the originating incident already reads `tasks.up: unknown field 'parallel', expected one of ... 'foreach', 'on-failure', 'tty' at line 3 column 5`, naming the path, the key, and `foreach` itself. The wrong-level case is the motivating incident but not the point; the point is that no wrong key is diagnosed at all.
- 2026-08-29 (panel round 2): **Phase 3b stays after Phase 3, not before.** The architect seat wanted it earlier, arguing a one-commit window where the new api-gate error is swallowed by the old lie on the `--help` path. The window is real and **empirically empty**: of the 190 ottofiles under `~/repos`, 181 declare `api:` and **every one declares `1`** (180 unquoted, 1 quoted: `examples/parallel-tasks/otto.yml`); nine declare none, so between Phase 3 and Phase 3b zero existing files can trigger the gate. Both placements ship safely; leaving it avoids renumbering a doc already two review rounds deep. Owner call, reasoning recorded rather than dropped.
- 2026-08-29 (panel round 2): **Phase 5(b)'s drift test is buildable and stays.** One seat called it "virtually unbuildable"; that was refuted with running code. Two zero-new-crate techniques, used together: exhaustive destructuring (`let ConfigSpec { otto: _, tasks: _ } = ...`) as the compile-time drift TRIGGER, which reaches private `TaskSpecHelper` from inside `src/cfg/task.rs`'s own `cfg(test)` module; and recovering the on-disk key list by feeding each struct a bogus key and reading it back out of serde's own `deny_unknown_fields` error. The probe was run: `ConfigSpec` 2, `OttoSpec` 9, `RetentionSpec` 5, `ForeachSpec` 7, `TaskSpecHelper` 13, `ParamSpec` 8 = **44 keys**, with renames already resolved (`as` not `var_name`, `on-failure` not `on_failure`) and `ParamSpec`'s five `#[serde(skip)]` fields correctly excluded, since those are not user-writable and have no place in a key reference.
- 2026-08-29 (panel round 1): **this doc owns the config-error help fallback fix** (Phase 3b), because it is the traffic this doc creates. Scope held to correcting the false cause, not redesigning the fallback.

## Alternatives Considered

### Warn instead of error
- **Description:** Emit a warning naming the key, keep loading.
- **Pros:** Cargo's model (`warning: unused manifest key`); no repo breaks on upgrade; forward-compatible by construction.
- **Cons:** `work repo A` has been silently broken long enough that nobody noticed; a warning is what nobody noticing looks like. Fails the house rule that unparseable input is a loud error, never a silent result.
- **Why not chosen:** The measured hit rate (4 files out of 159) makes fail-closed cheap, and the api gate solves the forward-compat objection that motivates warning in the first place.

### Strict parsing without the api gate
- **Description:** Ship `deny_unknown_fields` alone; leave `api:` decorative.
- **Pros:** Half the work; the remediation doc already scoped exactly this.
- **Cons:** Converts every future optional-field addition into a hard break for anyone on an older otto, with a message that misidentifies the cause.
- **Why not chosen:** It makes the next `tty:`-style feature a breaking change. The gate is what buys the right to be strict.

### Enrich serde's message at the call site
- **Description:** Catch the error at `serde_yaml::from_str` and rewrite it with a wrong-level hint.
- **Pros:** No second schema to maintain.
- **Cons:** Impossible cleanly. `serde::de::Error::unknown_field` formats the field and expected list into a string; `serde_yaml::Error` exposes only `location()`. Enriching means regex-scraping serde's own message, which is the "strings where types belong" pattern remediation `:28` already indicts.
- **Why not chosen:** The clean version is a pre-parse lint over `serde_yaml::Value`, which is the parked option in the Addendum, not this one.

### Type `api` as an integer
- **Description:** `api: u32` instead of `String`.
- **Pros:** Ordering comparisons come free; matches borg's `u32`.
- **Cons:** Changes `otto convert`'s emitted form from `api: '1'` to `api: 1`. borg's own const is documented as a SET, not a floor, so ordering buys nothing.
- **Why not chosen:** Churn without benefit.

## Technical Considerations

### Dependencies
Zero new crates. `levenshtein` is already a direct dependency and unused across the whole crate (remediation `:153`); it is what the parked hint layer would use.

### Performance
One additional YAML parse per invocation for the tolerant api header. Ottofiles are small and this happens once per run. `deny_unknown_fields` is compile-time codegen with no runtime cost.

### Security
Strictly narrowing: fewer inputs accepted, none newly executed. No new execution site, no secrets handling.

### Testing Strategy
- One negative test per struct, per borg `config/tests.rs:831-848`. Each asserts field AND path **except the root `ConfigSpec` case**, which has no parent and therefore no path: it asserts field + expected-set + location. See the Phase 4 table, which is authoritative. (Round 2 caught this section contradicting Phase 4; an implementer reading only here would have written an assert that cannot pass.)
- The Phase 3 ordering assert (both `api: 2` and an unknown key present) is the one most likely to rot; it must be demonstrated to bite.
- Every phase breaks its own code once to prove the new tests fail.
- `tests/roundtrip.rs` must stay unmodified and green throughout: measured clean under a patched tree (11/11), so any failure there means a real regression, not expected churn.

### Rollout Plan
Per-phase commits on `main`, `otto ci` green each. Minor bump after Phase 5: no new features, but existing ottofiles can stop loading, which is more than a patch. The three work-repo fixes are filed after the tag lands, not before.

## Risks and Mitigations

| Risk | Likelihood | Impact | Mitigation |
|------|------------|--------|------------|
| An ottofile outside `~/repos` breaks on upgrade | Med | Med | The api gate gives the next break a correct message; migration note names the key classes found in the wild |
| Phase 4 attribute breaks a free-form key site | Low | High | Four sites enumerated in Architecture (count corrected in panel round 1); Phase 4 negative tests cover task names, env keys, and param titles explicitly |
| `ParamSpec`'s `#[serde(skip)]` fields become rejected input and something writes them | Low | Med | Verified: nothing in the repo or in the 159 external ottofiles writes them |
| The version error and the unknown-key error race, and the wrong one wins | Med | High | Phase 3 lands before Phase 4, and criterion (c) asserts the ordering with both present |
| Strict parsing routes more files into a help fallback that misreports the cause | High | Med | Phase 3b lands before Phase 4 and fixes the fallback; criterion (a) asserts the real error appears and the false one does not |
| Two work repos gain env vars they never had | Med | Med | Called out explicitly in the operator table; deleting the dead block is the conservative option and is stated as such |

## Open Questions

(none)

## References

- `docs/design/2026-06-10-code-review-remediation.md` (In Review; `:213`, `:276`, `:288` superseded here, `:217`, `:28`, `:153` stay)
- `docs/design/2026-08-28-boundary-fixes-and-dynamic-foreach.md` (Implemented; its audit surfaced this defect)
- borg unknown-key precedent: `~/repos/scottidler/second-brain/main/borg/src/config.rs:281-285`, test at `borg/src/config/tests.rs:831-848`
- borg version-gate precedent: `~/repos/scottidler/second-brain/main/borg/src/harvest/contract.rs:47-54`, `:205-285`
- loopr strict-config precedent: `~/repos/scottidler/loopr/v5/crates/loopr/src/config.rs:31`, `:133`, `:202`, `:231`
- otto local precedent: `src/cfg/edge.rs:76`
- Reference SHA: `otto@5640628` (`v1.3.0`)

## Addendum: Parked Items

- **The "did you mean, wrong level" hint.** A pre-parse lint over `serde_yaml::Value`, driven by a static key -> legal-parent-paths table, running beside the existing `validate_*` calls, with `deny_unknown_fields` as the fail-closed backstop behind it. `levenshtein` (already a dep) covers near-misses. **Parked because** serde's own message already carries path, field, full expected set, and line/column, which is most of the value, and the table duplicates the schema and needs a per-struct drift test to stay honest. **Revisit condition:** a second wrong-level report from a real user after strict parsing ships.
- **`divine()` garbage-key validation.** `-verbose` becomes a positional; two longs concatenate. Remediation `:217` scope, untouched here, and named in Non-Goals so nobody assumes otherwise.
- **Typed error enrichment at the `serde_yaml` boundary.** Rejected outright, see Alternatives; recorded so it is not re-proposed.
