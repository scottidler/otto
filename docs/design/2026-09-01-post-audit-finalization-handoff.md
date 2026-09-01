# Handoff: post-audit follow-ups and finalization

**Author:** Scott A. Idler
**Date:** 2026-09-01
**Status:** Open — three approved tasks below, plus the finalization checkpoint
**Predecessors:** `2026-08-30-audit-batch-handoff.md` (Complete),
`2026-08-31-buffered-foreach-computed-envs-required-params.md` (Implemented)
**HEAD at handoff:** `68017b8` (local main, **15 commits ahead of `origin/main` at `4d9ca4e`, unpushed**)

## What this is

The 2026-06-10 remediation audit is complete (all 14 batches) and the
buffered-foreach/envs-command/required-params feature work is implemented and
audited. This document hands off the three follow-ups the owner approved on
2026-09-01, records what this session completed so nothing is redone, and
carries the still-open finalization checkpoint.

## What was completed this session (do NOT redo)

- **Feature implementation verified.** All 5 phases of the 2026-08-31 design
  doc are in (`2a396e1..9f3c5d1`, doc marked Implemented at `33b3731`). All
  three features re-verified behaviorally against the built binary: subdirectory
  `envs:` `$(...)` resolution, bare-invocation `required` preflight (usage error
  before any subprocess), buffered foreach (contiguous blocks in item order).
- **Audit batches 8-14 run at `33b3731`**, which also discharged the
  re-run-after-implementation obligation on batches 9, 11, 14. Per-batch
  results and fix commits are in the batch table of `2026-08-30-audit-batch-handoff.md`.
- **Every must-fix committed**, `otto ci` green over the final tree
  (91.9% local coverage, exit 0):
  - `b77d016` internal-name scrub (slugs renamed `c82ec2a`-style, the internal
    task-dump JSON deleted). Word-boundary sweep of the tree is clean.
  - `574c2a4` `bash32` moved into `checks.yml` so Release gates on it
    (v2.0.5 published from a red-CI SHA; the job was invisible to the gate).
  - `ba71ad4` `cargo-llvm-cov` pinned at 0.9.0 in the coverage job.
  - `8c402a0` last production `.unwrap()` (`src/executor/graph.rs` foreach
    collapse) replaced with visible degradation.
  - `0ca381d` coverage floor re-sighted: runner truth **87.4%**
    (12517/14317, run 33433873821); local numbers are inflated ~4.5 points by
    test code in the 0.6.x denominator.
  - `d8d9cfc` converter load-path test docstring stops claiming graph
    resolution it never performs.
  - `021fd06` six dated corrections in the remediation doc (false STAYS
    annotations, four-not-two attributes, shipped-but-claimed-unshipped
    paired-edge/cycle-detection, two overclaiming audit summaries).
  - `68017b8` audit handoff closed.
- **Already fixed, verified, do not re-open:** the `otto_get_input`
  uppercase-key silent-empty defect shipped fixed in v2.0.5 (`5f1392c` keeps
  producer case, `7acb653` makes a miss loud: `no input '...'; available: ...`,
  exit 1). Proven behaviorally 2026-09-01.
- **`review-panel` agent definition fixed** (claude repo): seats must never be
  detached — launches AND `wait` in one foreground Bash call. Detached and
  `run_in_background` dispatch is reaped silently by the sandbox PID namespace
  (three confirmed kills, ~100-byte banner files).

## Approved task 1: ratchet the coverage floor to 87

Owner approved 87 on 2026-09-01.

- Edit `.otto.yml` `cov-report` `--fail-under` `default: "85"` -> `"87"`, and
  update the comment block above it (it states the 85 rationale and the 2.4-point
  margin; at 87 the margin over the runner's 87.4% is **0.4 points**, which is
  the point — the floor is the alarm).
- Judge only against runner numbers: local `cargo-llvm-cov` 0.6.x reads ~4.5
  points high. Local `otto cov` will still pass trivially (91.9% local).
- Proof: next CI run on a runner is green with the 87 floor; a local
  `otto ci` green before commit.

## Approved task 2: wire SIGINT for non-TUI runs

Owner approved wiring on 2026-09-01. Today only the TUI path handles Ctrl+C
(`src/app.rs:576-588`: `tokio::signal::ctrl_c()` -> shutdown flag ->
`scheduler.cancel_signal()`). A non-TUI run has no handler, so terminal Ctrl+C
default-kills the process group and the buffered-foreach cancellation flush
(`abandon_run`, `src/executor/scheduler/support.rs`) never runs.

- Install a `ctrl_c` handler on the non-TUI execute path feeding the same
  `cancel_signal()`, so an interrupted run flushes completed buffered blocks
  and prints log paths for killed ones, per the cancellation table in
  `2026-08-31-buffered-foreach-computed-envs-required-params.md` (Phase 4).
- **This changes teardown for every run** — that is why it was parked. Mind:
  - task children run in their own process groups
    (`src/executor/scheduler/task_execution.rs:200`) so a signal aimed at otto
    does not race them, but a TERMINAL Ctrl+C signals the whole foreground
    group including children; decide and document whether otto forwards,
    escalates on second Ctrl+C, or lets the group delivery stand.
  - exit code on interrupt must stay non-zero and the run row must persist as
    failed/cancelled, not vanish.
  - `tests/tui_panic_test.rs` shows the pty-driven test pattern; batch 9's
    probe (Ctrl+C under a pty, 5/5 deterministic) is the evidence standard.
- Proof: a pty test that interrupts a buffered parallel group mid-run and
  asserts flushed blocks + did-not-start lines per the six-state table, plus
  `otto ci` green.

## Approved task 3: private-org name sweep with the work persona

Owner approved 2026-09-01. Batch 13 could not reproduce the remediation doc's
"350 names swept, 6 hits, all demonstrably benign" claim: this repo maps to the
home persona, whose token sees only ~30 public org repos. The 6-benign
conclusion is unverified against the private list, and the tree has since
gained the (now scrubbed) post-handoff text.

- From a session with the WORK persona token (`~/repos/.claude/refs/personas.md`
  has the setup), enumerate the full org repo list, then sweep this repo's
  tracked tree (`git ls-files | xargs rg -iw --` per name, word-boundary —
  substring sweeps false-positive on ordinary words; "philosophy" already
  burned one audit claim).
- Expected: the 2 hits batch 13 found on the public subset (an upstream
  pre-commit project, an English word), nothing else. Any new hit is a
  must-fix before push.
- Record the result as a dated correction on the remediation doc's acceptance
  criterion 8 and close batch 13's single DEFER.

## Finalization checkpoint (still open, owner approval required for each)

Version bump (minor — the feature doc's rollout plan), push, tag, install.
**None has been given.** Sequencing that matters:

- Task 3 (sweep) and task 1 (floor) should land before the push.
- The first tag push after `574c2a4` is the release-gate fix's first
  production proof: Release must now wait on `bash32`.

## Deferred, NOT approved (list only, decide before acting)

- Recipe-internal converter diagnostics cite the rule's line (off by recipe
  length); `otto Convert` output is not byte-reproducible (`otto.envs` HashMap
  ordering, proven order-independent, diff churn only).
- `a_non_utf8_byte_does_not_truncate_the_log` never asserts `contents[1]`, so
  silent blanking passes; the git-worktree root test can't distinguish the two
  anchorings.
- Anchor drift: `src/cfg/otto.rs:184-185` comment cites `cli/parser.rs:777-779`,
  real site `:918`; remediation-doc entry 2 `:319`->`:369`, entry 4 `:324`->`:332`.
- `ottofile_reference_key_inventory_is_exhaustive` is one-directional (schema->doc);
  one table-row-count assertion closes it.
- README Usage spells only short forms; the `--tui` grep alternand can't fail.

## Gotchas for whoever continues (hard-won, all hit this session)

- `RUSTC_WRAPPER=""` to dodge the sccache sandbox EPERM; do not disable the
  sandbox for it.
- `target/debug/otto` for probes, never `~/.cargo/bin/otto` (stale until the
  finalization install). `OTTO_HOME=$TMPDIR/<scratch>` on every probe.
- `before: [x]` on a task means x runs before it (x is the dependency);
  `after: [x]` schedules x after. Getting this backwards inverts a fixture.
- Never detach review-panel seats; never treat an idle notification as new
  information — check `git log` and the tree.
- Name sweeps are word-boundary (`rg -iw`) or they lie in both directions.
