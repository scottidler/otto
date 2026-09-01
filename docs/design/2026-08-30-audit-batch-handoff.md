# Handoff: batched implementation audit, batches 3-14

**Author:** Scott A. Idler
**Date:** 2026-08-30
**Status:** Complete — all 14 batches done; batches 8-14 ran 2026-09-01 at `33b3731` (post-feature-work tree, satisfying the re-run obligation from `2026-08-31-buffered-foreach-computed-envs-required-params.md`); every must-fix committed, finalization checkpoint with the user
**Subject doc:** `docs/design/2026-06-10-code-review-remediation.md` (`Status: Implemented`)
**HEAD at handoff:** `a567af9` (batches 3-4 audited at `6b28b52`, fixed through `16c2975`)

## What this is

The remediation doc's twelve phases are implemented and committed. This
document hands off the *verification* of that work: a per-item audit of every
claim the doc makes, run in batches so it cannot be sampled.

**Read this whole file before spawning anything.** The traps below are not
hypothetical; every one of them has already cost this effort real time.

## Why batches exist

A single whole-doc audit was run first and **failed by sampling**. Two
reviewer seats returned twelve findings each against ~108 claims, with no
per-item verdict. It missed four real defects, including one neither seat
found: a universally-worded success criterion ("every fixture converts to
exactly its `expected.yml`") satisfied by a `continue` statement, where five
of seven fixtures had no `expected.yml` at all.

Batching makes sampling structurally impossible: the scope is small enough
that "one verdict per item" is enforceable, and a batch returning fewer
verdicts than items is rejected and re-run.

## Batch plan: 14 batches, ~108 items

| Batch | Scope | Doc lines | Items | State |
|---|---|---|---|---|
| 1 | Phase 0 — Green gates and exposure removal | 296-319 | 7 | **DONE** — 1 must-fix, fixed at `a5cd889` |
| 2 | Phase 1 — Silent-success criticals | 320-335 | 10 | **DONE** — 1 must-fix, fixed at `a567af9` |
| 3 | Phase 2 — Conditional-deps and foreach semantics | 336-360 | 8 | **DONE** — 1 must-fix, fixed at `5558ae9` |
| 4 | Phase 3 — Containment (injection, deletion, output) | 361-379 | 9 | **DONE** — 2 must-fix, fixed at `42704a9`, `16c2975` |
| 5 | Phase 4 — State and DB integrity | 380-403 | 10 | **DONE** — 2 must-fix, fixed at `a10f911`, `0254baa` |
| 6 | Phase 5 — Upgrade and HTTP safety | 404-413 | 4 | **DONE** — 1 must-fix, fixed at `e71638e` |
| 7 | Phase 6 — cfg correctness | 414-448 | 10 | **DONE** — 1 must-fix, fixed at `7f7ffd5` |
| 8 | Phase 7 — Makefile converter truth | 449-464 | 7 | **DONE** (2026-09-01, at `33b3731`) — 1 must-fix, fixed at `d8d9cfc` |
| 9 | Phase 8 — TUI and CLI surface | 465-485 | 10 | **DONE** — 2 must-fix (doc annotations), fixed at `021fd06`; no regression from `2a396e1..9f3c5d1` |
| 10 | Phase 9 — Repo and dependency hygiene | 486-502 | 7 | **DONE** — 2 must-fix: `graph.rs` unwrap fixed at `8c402a0`, doc absolute at `021fd06` |
| 11 | Phase 10 — Doc truth | 503-515 | 8 | **DONE** — 2 must-fix (bullet 3 + its own audit header), fixed at `021fd06`; 45-key inventory verified bijective |
| 12 | Phase 11 — Test-gap closure | 516-589 | 10 | **DONE** — 2 must-fix (coverage denominator, unpinned cargo-llvm-cov), fixed at `0ca381d`, `ba71ad4`; 16 mutation proofs |
| 13 | Acceptance Criteria | 590-638 | 8 | **DONE** — 2 must-fix: release-gate hole fixed at `574c2a4`, name-leak scrub at `b77d016`; its one DEFER (sweep against the private repo list) closed 2026-09-01, see criterion 8's dated correction |
| 14 | Resolved Decisions that assert something about code | 639-662 | 21 | **DONE** — 3 must-fix (entries 12, 20, and the closing summary), fixed at `b77d016`, `574c2a4`, `021fd06` |

Line numbers drift as fixes land. Re-derive with:
`grep -n '^#### Phase \|^## Acceptance Criteria\|^## Resolved Decisions' docs/design/2026-06-10-code-review-remediation.md`

Batches were run two at a time in parallel. That worked; more may saturate.

## The batch prompt

Use the `review-panel` agent. The prompt below is the one that produced both
useful batches; adapt the phase-specific paragraph and keep everything else.

```
**Implementation Audit, BATCH N of 14 — Phase P ONLY.**

Repo <this repo's absolute path>, branch main, HEAD <sha>.
Doc: docs/design/2026-06-10-code-review-remediation.md, Status: Implemented.

Scope: ONLY the bullets under `#### Phase P: <title>` (starts ~line L), plus
that phase's `**Success criteria:**` line. There are K bullets. Ignore every
other phase entirely.

This is a per-item audit, not a sample. A whole-doc round failed precisely
because both seats sampled: 12 findings each against 108 claims, missing four
real defects, one of which was a universally-worded criterion satisfied by a
`continue` statement that neither seat found.

Required output shape. One verdict per bullet, in doc order, numbered. For
EACH bullet: the bullet's opening words so I can match it; a verdict of
CONFIRMED / DIVERGENT / UNIMPLEMENTED / UNVERIFIABLE; the exact command(s)
run and their real output; and for DIVERGENT, doc claim versus code reality.
A batch returning fewer verdicts than bullets is rejected and re-run. A
CONFIRMED verdict costs the same evidence as a defect.

Execution environment. Use $TMPDIR or /tmp/review-panel/ for scratch. Every
seat must state up front whether it can execute commands; if it cannot run
cargo/otto, its first line says so and its verdicts are marked STATIC-ONLY.
Use target/debug/otto, never ~/.cargo/bin/otto (deliberately stale v1.4.0).
Any probe running the binary MUST set OTTO_HOME=$TMPDIR/<scratch>. `otto ci`
needs the Bash sandbox disabled or sccache fails with "Operation not
permitted", which reads as a false code failure.

<phase-specific paragraph: what the bullets claim, and what to be
 particularly skeptical of>

<disclosed-known-issues paragraph, so they are not reported as discoveries —
 but ask whether they are worse than stated>

Do not modify code or the doc. Read-only. Report the synthesis file path.
```

## Seat reliability — the single most important thing here

**Neither reviewer seat can be trusted to have executed anything.** Measured
across three rounds:

- **Architect (Gemini):** declared STATIC-ONLY in batches 1 and 2 — it could
  execute nothing and returned verdicts built entirely from source reading. In
  the whole-doc round it did *not* declare that, and closed with "no
  undisclosed deviations, no skipped requirements, no fabricated assertions"
  while four real defects sat in scope. Treat its unexecuted CONFIRMEDs as
  hypotheses. In batch 1 its bullet-5 reasoning ("not in `.gitignore`, meaning
  it is tracked") was an invalid inference that happened to reach the right
  answer.
- **Staff Engineer (Codex):** sandbox-limited and honest about it. In the
  whole-doc round it could not reach `/tmp`, `cargo test`, or even
  `otto --version`. In batch 1 it had `git`/`rg` but not `cargo`, not the
  binary, and `gh` was proxy-blocked; it correctly returned UNVERIFIABLE
  rather than guessing. In batch 2 it executed everything with real output.

**Batches 3 and 4 made this worse, not better.** In batch 3 the architect
declared on line 1 that it could not execute and still returned 10/10
CONFIRMED, one of them provably false and another the exact inverse of the
measured result; the staff engineer had a read-only filesystem (`cargo test`
died on `.cargo-build-lock`, `mktemp -d` gave "Read-only file system (os error
30)") and honestly marked 5 of 10 UNVERIFIABLE. In batch 4 the architect was in
plan mode and the staff engineer hit the same read-only sandbox. **Across both
batches, neither seat ran a single binary probe.** Every behavioral result in
both synthesis files came from the panel lead.

**Consequence:** the panel lead must re-run every behavioral claim itself.
All four useful batches worked because the lead did exactly that. Budget for
it. If cross-model runtime independence actually matters for a later batch, the
seats need a writable scratch directory and a non-plan mode first - otherwise
the panel is a second pair of eyes on source text, which is worth something but
is not verification.

A concrete illustration of why: in batch 1 the architect asserted the
`needs:` graph "logically prevents artifact and GHCR uploads" having executed
nothing. Had `otto quick` returned 0 on failure, that assertion would have
been confidently wrong. The lead tested the exit code; it was 1.

## Findings so far, and their fixes

### Batch 1 (Phase 0) — 1 must-fix, 3 cheap-win

**Must-fix, DIVERGENT.** Phase 0 deleted four internal service Makefile
fixture directories, and then its own implementation-notes file republished
all four names verbatim, as did the design doc's bullet and its `Observed`
line — on a repo `gh repo view` confirms is `"visibility":"PUBLIC"`. A prior
dedicated scrub commit touched five other docs, never opened either file, and
its commit message asserted zero matches. That claim was false. Fixed at
`a5cd889`; the only surviving tree-wide match is the legitimate upstream
`pre-commit/pre-commit-hooks`.

**Why both seats missed it:** both stopped at the literal `ls makefiles/`
criterion and called the bullet CONFIRMED. The defect sat in the gap between
the criterion and the bullet's stated purpose. **Generalise this: when a
bullet's criterion is narrower than its intent, audit the intent.**

Cheap-wins fixed in the same commit: `.pre-commit-config.yaml` clippy now
carries `--workspace` (the third of three invocations the bullet named to
align); and bullet 1's prose was corrected — it cited nine `file:line` sites
for an eight-site fix, every line number stale, and claimed "All sites now
`sort_by_key`" when five `.sort_by(` calls survive. The clippy-clean outcome
was real; the description of it was not.

### Batch 2 (Phase 1) — 1 must-fix, 1 cheap-win

**Must-fix, a live hang.** `otto.jobs: 0` in an ottofile hung the process:
`timeout 12s otto build` → exit 124, zero output, 100% CPU. The doc had
**deliberately DROPPED** this validation, recording that "`otto.jobs` has zero
consumers, so `jobs: 0` in an ottofile runs fine and validating it would guard
nothing." Phase 10 later wired `otto.jobs` to real behavior (consumed at
`cli/parser.rs:777-779` whenever `-j` is absent), which made both halves of
that rationale false and reopened the exact hot spin the `-j 0` fix had
closed. Fixed at `a567af9` with a validating deserializer that fails closed
even when `-j` would override, plus three tests.

**The generalisable lesson, now recorded in the doc:** *a DROPPED item's
rationale is a claim about the code, and it expires when a later phase changes
that code.* Phase 1 dropped it; Phase 10 invalidated the reason; nothing
re-checked it. **Batches 3-14 must treat every DROPPED, MOVED OUT, SUPERSEDED
and "already moot" annotation as a claim to re-verify, not as settled.** One
seat explicitly skipped this sub-bullet *because* it said DROPPED.

**Cheap-win:** Phase 1's success criterion `git grep -c 'process::exit'
src/cli/parser.rs` **returns 0** could never pass — `git grep -c` prints
nothing and exits 1 on no matches — and it scoped only the parent file, which
Phase 9's `include!` split would have let a migrated call site evade. Amended
to cover parent and all seven fragments; verified zero matches.

### Batch 3 (Phase 2) — 1 must-fix, 4 cheap-win

**Must-fix, DIVERGENT.** The success criterion claims the nine `SkipKind` x
`when` cells are "asserted against the worker's dependency double-check, not
only `classify_edge`, so the two gates cannot drift", and the doc uses that
guarantee as the argument for the whole five-site design. It was not
delivered. The test's Gate 2 block was a hand-transcribed copy of the
production match arms, and the production gate was an inline `match` inside a
`tokio::spawn` closure inside `execute_task`, so no symbol existed to call.
Proven by mutation, twice: flipping the `when: success` skip arm to `=> true`
(the exact pre-Phase-2 bug) and the `when: always` skip arm to `=> false` (the
spawn-time-rejection mode the doc names verbatim) both left the matrix test
green. Fixed at `5558ae9` by extracting `edge_satisfied_by_status`, which both
the worker and the test now call; the same two mutations are red against it.

**Generalise this: a test that re-transcribes the thing it claims to pin
asserts its own copy.** The tell is a criterion naming a code site the test
never references. It is the same shape as batch 1's — criterion narrower than
intent — but the gap is between the assertion and its subject, not between the
criterion and the purpose.

**This is also known-issue #3 being worse than stated.** "Revert the fix,
confirm the test goes red" was spot-checked, and the one criterion whose whole
value is a red-on-revert guarantee is precisely the one that stayed green.

Cheap-wins, all doc-side, fixed in the doc commit: the "Explicitly unchanged"
line was false in letter (`c72f911` edited `foreach_aggregation_test.rs` and
two other test files; the `:204`/`:260` anchors moved to `:206`/`:262`), though
its semantic claim — exactly one deliberate inversion — is true; the doc names
a fix site `skip_reason_for` that ships as `skip_record_for`; and
`process_group(0)` carries an undisclosed `if !tty` carve-out while the bullet
promises it unconditionally.

**Cross-batch catch.** Phase 1's still-open sub-bullet said `get_skip_reasons()`
has zero callers so skip provenance is "built and dropped rather than
persisted". Phase 2 renamed it and wired `persist_skip_records()`. Verified
against the database, not the call sites: a failed `a` with dependent `b` gives
`b|skipped|unreachable|dep a failed; this task required when: success`. That
annotation had expired exactly as `otto.jobs: 0`'s did.

### Batch 4 (Phase 3) — 2 must-fix, 3 cheap-win

**Must-fix 1, task envs failed open.** Task-level `envs:` warned and
substituted an empty map, so one bad key took every *other* key with it and the
task ran anyway at exit 0. With three keys where only `A` and `B` cycle:
`[t] GOOD=[MISSING] A=[MISSING] B=[MISSING]` / `finished successfully` /
`EXIT=0`. The healthy `GOOD: iamfine` is gone too. Phase 1 had already made the
*global* path fail closed, and `cfg/resolver.rs`'s own doc comment names the
defect left standing next door. Fixed at `42704a9`, both constructor copies,
two regression tests, red on revert.

**Generalise this: when a phase fixes a failure mode on one path, ask which
sibling paths have the same mode.** Fail-open was fixed on globals and left on
tasks, in the same release, by the same author.

**Must-fix 2, fail-closed but nondeterministic.** Phase 3's deep-chain fix
moved the failure from "silently drops your globals" to "refuses to run" —
right direction, still a coin flip. Twenty runs of one unchanged 200-deep
ottofile: `1 0 0 0 0 1 0 1 0 1 1 1 1 0 1 1 1 1 0 0`, nine resolving correctly
and eleven exiting 1. `pending` was seeded off `envs.iter()` and the budget was
a flat 100 outer passes. Fixed at `16c2975`: sorted seed, budget `envs.len() +
1`, which no resolvable map can exceed. Depths 105/200/250/400 now pass 10/10.

**Generalise this: "it fails closed now" is not the end of the question. Ask
whether it fails closed *deterministically*.** A reviewer measuring once would
have called this CONFIRMED; five reps caught it, twenty reps sized it.

Cheap-wins outstanding, not yet fixed: the shipped injection test table pins
only `"` and `'` payloads while `;`, backtick, newline and `${IFS}` pass but
are unpinned; `Clean` `continue`s past a refusal so it can report one on stderr
and still exit 0 (reachable only by a TOCTOU race, since both scans already
skip symlinks).

**Defect found outside the doc's claims, NOT fixed, needs a decision.**
`otto_get_input` silently returns empty for any output key containing
uppercase. `otto_set_output "MIXED_Case"` preserves case;
`otto_deserialize_input` lowercases when populating `OTTO_INPUT`
(`executor/action.rs:459`), because the round-trip goes through an uppercased
`OTTO_INPUT_<TASK>_<KEY>` shell variable and the original case is not
recoverable from it. So `otto_get_input producer.MIXED_Case` returns `[]` at
exit 0 while `producer.mixed_case` returns the value. It is the silent-success
class this whole doc exists to kill, but the fix picks a source of truth for
key names (the JSON, or a parallel key list) and that is a design decision, not
an audit finding to apply unilaterally. Left for the user.

### Batch 5 (Phase 4) — 2 must-fix, 4 cheap-win

All ten bullets landed as specified. Two live defects sat in the gaps between
them, both found by running concurrent processes rather than by reading.

**Must-fix 1, concurrent cold start lost runs silently.** Five processes racing a
database that does not exist yet persisted as few as 1 of 5, every one exiting 0
with empty stderr. Two defects stacked:

1. `migrations.rs`'s `current_version == 0` branch was the one branch with no
   transaction (Phase 4 wrapped the four upgrade branches and left initialize
   bare), and `set_version` was a bare `INSERT` into a PRIMARY KEY column, so
   losers died on `UNIQUE constraint failed: schema_version.version`.
2. Underneath it, `db.rs` set `busy_timeout` **after** the `journal_mode=WAL`
   pragma, so the one statement needing a brief exclusive lock ran with a zero
   timeout: `Failed to enable WAL mode: database is locked`.

**A trap worth recording for whoever fixes something like this.** The first fix
attempt used `conn.unchecked_transaction()` and made it *worse*, 1-2 of 5,
because that is a DEFERRED transaction: it starts read-only, must upgrade to a
write lock on first write, and SQLite returns SQLITE_BUSY immediately for that
upgrade rather than deadlock, so `busy_timeout` cannot cover it. `BEGIN
IMMEDIATE` takes the lock up front, which is the case `busy_timeout` does cover.
Only re-measuring caught this; the fix looked obviously correct. Fixed at
`0254baa`. After: 12 trials of 5 racers and 8 of 10 racers, all persist.

**Must-fix 2, same-second runs shared one directory.** Bullet 2 dropped
`UNIQUE(runs.timestamp)` so the rows stopped colliding; the directory still
collided, because `layout.rs` named it after the timestamp alone. N same-second
runs became N rows pointing at ONE directory: they raced creating it (`File
exists (os error 17)`), overwrote each other's task logs, and cleaning any one of
them left the others with a dangling `run_dir`. The criterion "two runs started
in the same second both persist" was true of the rows and false of their
artifacts. Fixed at `a10f911` by reserving the directory with an exclusive
create, `<timestamp>` then `<timestamp>-<seq>`, plus the matching parser so
fs-mode clean does not walk past the suffixed ones and leak them.

**These two interlock, which is worth knowing before splitting work like this.**
Must-fix 1's regression test spawns 8 racers, which all start in the same second,
so it could not pass until must-fix 2 was fixed. The `File exists` error looked
like a third defect and was not.

Cheap-wins: `manager.rs:646,804` still used `and_then(TaskStatus::parse)`,
nulling an unknown status while bullet 9 closed that exact conflation at `:445`
(fixed, `173407f`); `Clean` reported bytes it never freed for runs whose
directory was already gone, because `delete_run` returns `Ok(Some(..))` either
way (fixed, `42dce98`); the `status='completed'` criterion can never be true, the
runs table stores `success` (doc); the preamble says nine bullets and there are
ten (doc).

**Also chased and closed: an apparent data loss that was not one.** The batch
flagged 352 runs / 446 tasks / 136 projects missing from `~/.otto/otto.db`
against the `pre-phase11-backup`. Of the 352, 15 belong to one unrelated local project, older than
the 30-day retention cutoff (normal), and every one of the other 232 in-window
rows belongs to a `.tmp*` project: integration-test runs that leaked into the
real database before the `OTTO_HOME` isolation fix. Filtering for missing rows
whose project is not `.tmp%` returns nothing. Nothing real was lost.

### Batch 6 (Phase 5) — 1 must-fix, 2 cheap-win

The upgrade path genuinely works end to end and rolls back correctly, proven by
driving the real CLI, including a self-replacing install (the ETXTBSY case
`fs::copy` would have failed) and 10 SIGKILL trials landing inside the copy
window with the original intact 9 of 9. The defect is that the phase's own
criterion for it was never tested.

**Must-fix, a criterion nothing could satisfy.** "`otto Upgrade` completes an
install end-to-end against a fixture release" had no test: `RELEASES_URL` was a
const with no injection point and the install target was `env::current_exe()`, so
`execute_upgrade`/`execute_rollback` structurally could not be aimed at a
fixture, and both had zero test call sites. The test that stood in for it
hand-composed `download_and_verify` + `install_from_archive`, skipping
`current_version`, the already-on-target and downgrade short-circuits,
`find_asset` and `create_backup`. Same for rollback. Fixed at `e71638e`.

**Proven by mutation, which is the standard this audit should hold to.** Breaking
`find_asset` leaves the OLD end-to-end test green and fails both new ones;
making rollback restore onto itself leaves the OLD rollback test green and fails
the new one.

**Security note for anyone touching this again.** The injection point is two
`#[arg(skip)]` fields written only by a `#[cfg(test)]` builder. It must never
become a flag or an `env =`: a settable releases URL turns self-upgrade into
"download and execute a binary from wherever this string points". Verified
closed: `--releases-url` gives `error: unexpected argument`.

Cheap-wins outstanding: a signal-killed upgrade orphans `.otto.upgrade-<pid>`
beside the binary and nothing reaps it, while `commit_staged`'s comment claims a
failed upgrade "leaves no debris" (true for a returned Err, false for SIGKILL);
the `git grep -c ReleaseFetcher src/` criterion prints nothing and exits 1 rather
than printing 0 (doc, same defect batch 2 found in Phase 1).

One correction to the batch's report: it listed `clean.rs:270` as printing a
refusal without its cause. It does print the cause, and `manager.rs:1016`
propagates with `?`. The real residue is narrower: `clean.rs` `continue`s past a
refusal so Clean can exit 0 having refused, and that branch is reachable only by
a TOCTOU race since both scans already skip symlinks.

### Batch 7 (Phase 6) — 1 must-fix, 4 cheap-win

All six phase success criteria hold on a real binary, including the per-item
foreach cache. The defects were in bullet 10.

**Must-fix, and the batch reported it backwards.** Batch 7 filed it as "a
legitimate `otto.envs` variable in a foreach path hard-fails, while the identical
construct in a non-foreach task runs clean". The non-foreach task running clean
was itself the bug. Measured: `input: ["${SRCDIR}/a.txt"]` expanded nothing, so
the glob matched no file, the task tracked no inputs, and it re-ran on EVERY
invocation while reporting success; the same task with a literal `src/a.txt`
skipped as up to date. Meanwhile `examples/environment-variables/otto.yml` ships
`output: ["${BUILD_DIR}/${PROJECT_NAME}"]`, so variables in paths are a
documented feature that silently did nothing.

So the fix is not "stop the foreach task erroring", it is "make both work". Fixed
at `7f7ffd5`: foreach resolves its loop variable and preserves the rest,
`expand_env_in_paths` resolves those against the task's evaluated environment
before globbing, undefined is an error naming task/field/path. One shipped test
deliberately inverted, disclosed in the commit.

**Generalise this, and it is the fourth lesson for the prompt: a control that
"runs clean" is not automatically a passing control.** When two paths disagree,
establish which one is CORRECT before assuming the failing one is wrong. Both a
reviewer seat and the batch lead read "runs clean" as "works".

**Placement decision, which bullet 10 told itself to record and never did.** The
check runs at task construction, not config load, because that is the first point
a task's own `envs:` merge with the globals; validating earlier would guess at
task-level envs and reject valid ottofiles. Cost: `--help` and `--list-subtasks`
do not build tasks and so do not report a bad path variable. Batch 7 filed the
run-time-vs-load-time gap as a second must-fix; it is really this decision, now
made and written down, plus a comment that was wrong and is now accurate.

Cheap-wins: dedent coverage widened past U+2002 to five multibyte widths plus a
mixed indent, and a non-bash shebang round-trip added (`4bd6b25`) - each shipped
test pinned exactly the one input that produced its original panic, which is
narrower than the defect. Bullet 9's `envs.get` evidence is stale: it returns
`env.rs:505`, a CHECKED lookup added by `16c2975`, a later commit in this same
audit. Fourth instance of a criterion true only as of the commit that measured
it.

Deliberately not changed: the multi-action-source error names the sources but not
the task, because the `TaskSpec` deserializer cannot see the map key and serde's
`at line 5 column 3` already locates it.

## Standing rules for whoever continues

### Verification standard
- **A control that "runs clean" is not automatically a passing control.** When
  two code paths disagree, establish which one is correct before assuming the
  failing one is wrong. Batch 7 filed a must-fix backwards on exactly this: the
  "working" path was silently doing nothing.
- `otto ci` with the Bash sandbox **disabled**, or `sccache` fails with
  `Operation not permitted (os error 1)` and reads as a false code failure.
- Run under two environments: `env -u OTTO_HOME -u OTTO_DB_PATH otto ci` and
  `OTTO_HOME=/tmp/<scratch> otto ci`. Capture each exit code by assigning
  `$?` **immediately** — `PIPESTATUS` after an intervening command returns
  empty and will fool you.
- Record `md5sum ~/.otto/otto.db` before and after any full `cargo test`; it
  must be byte-identical. This property was hard-won and is easy to regress.
- Use `target/debug/otto`. **Never** `~/.cargo/bin/otto` — it is a
  deliberately stale pre-session v1.4.0 and reading it produces false
  negatives. One phase already lost a probe to this.

### Do not
- **Do not bump, push, tag, or `cargo install`.** All 32 commits are local and
  the finalization checkpoint is still open, awaiting the user.
- **Do not have two agents editing the checkout at once.** This already
  happened once and cost real time untangling. If you find uncommitted changes
  you did not make, stop and ask.
- Do not treat an idle notification as a completion signal. Several arrived as
  stale restatements of earlier status, and acting on one caused the
  two-writer collision. Check `git log` and the working tree instead.

### New gotchas, learned in batches 5 and 6
- **`conn.unchecked_transaction()` is DEFERRED.** It starts read-only and must
  upgrade to a write lock on first write, and SQLite returns SQLITE_BUSY for
  that upgrade immediately rather than deadlock, so `busy_timeout` does not
  cover it. Use `BEGIN IMMEDIATE` for anything that will write. A fix using the
  deferred form measured *worse* than the bug it replaced.
- **`/tmp` is a 16 GB tmpfs and the panels fill it.** A full `cargo test` fails
  with two log-rotation tests writing 11 MB each, which reads as a code
  regression and is not. Check `df -h /tmp` before believing a test failure in
  `test_rotate_log_*`. Stale panel scratch (`*-target` dirs, upgrade fixtures)
  is the usual culprit; ~11 GB was reclaimed during batch 5-6 cleanup.
- **Probe litter is the panel lead's to clean.** Cold-start race trials create a
  run directory per racer per trial.

### Gotchas
- Ottofile syntax: `after:`/`before:` take sequences (`after: [build]`);
  `foreach:` is a struct (`foreach: {items: [a,b], as: pkg}`).
- `OTTO_HOME` alone is now sufficient DB isolation (this was not true before
  Phase 4 — `default_db_path()` used to read only `OTTO_DB_PATH`).
- rust-analyzer diagnostics lag badly on this tree and routinely show errors
  for a state that compiles cleanly. Trust `cargo check`, not the IDE stream.

## Open items not owned by this audit

- **The `include!` decomposition stays.** The user ruled keep-for-now. It
  satisfies the 1500-line cap by textual splice: `parser.rs` is 1044 lines
  plus 1938 in fragments (2982 real), `scheduler.rs` 1188 + 724 (1912). The
  user has since amended his personal Rust rules to forbid `include!` for this
  purpose in future work. Batches may note it; do not restructure it.
- **Three bullets are correctly unchecked** and should stay that way unless
  the underlying fact changes: the kebab-case key flip (moved to the
  Addendum), the criterion naming a GitHub Actions `conclusion` (nothing local
  can produce one), and the "None." line under Open Questions.
- **Release-gate history.** Batch 1 reached GitHub and measured six
  consecutive SHAs where Release concluded `success` while CI concluded
  `failure` on the same commit. That is the mechanism Phase 0's gate exists to
  close, and it has never been exercised because nothing has been pushed.

## Known-unverified, disclose in every batch prompt

State these up front so seats do not report them as discoveries — but ask
whether they are **worse** than stated, which is how the coverage gap below
was found:

1. **CI has never run on a real runner.** No push access from this
   environment. The 85% coverage floor was measured locally at 90.4%.
2. **A panic inside the TUI is not exercised end to end.** The mechanism is
   present and correct (`install_panic_hook()` at `src/tui/mod.rs:137`, called
   from `init_terminal()`, plus `Drop for TerminalGuard`), just untested. A
   panic-injection hook in the terminal-takeover path was deliberately
   declined.
3. **"Revert the fix, confirm the test goes red" was spot-checked**, not
   performed across all assertions.

Item 1 was found to be *worse* than originally stated during the whole-doc
round: the coverage floor was not merely unmeasured on a runner, it did not
**run** there at all, because the reusable workflow invoked `otto quick`
(which has no `cov`) rather than `otto ci`. Fixed by adding a dedicated
`coverage` job. That is the payoff for asking "is it worse", so keep asking.

## When batches 3-14 are done

1. Fix every must-fix, each with its own commit and `otto ci` green.
2. Re-run the acceptance-criteria walk end to end.
3. Return to the user with the finalization checkpoint: version bump, push,
   tag, install — **all four still require explicit approval and none has been
   given.**
