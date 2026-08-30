# Handoff: batched implementation audit, batches 3-14

**Author:** Scott A. Idler
**Date:** 2026-08-30
**Status:** In Progress — batches 1-2 complete, 3-14 outstanding
**Subject doc:** `docs/design/2026-06-10-code-review-remediation.md` (`Status: Implemented`)
**HEAD at handoff:** `a567af9`

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
| 3 | Phase 2 — Conditional-deps and foreach semantics | 336-360 | 8 | TODO |
| 4 | Phase 3 — Containment (injection, deletion, output) | 361-379 | 9 | TODO |
| 5 | Phase 4 — State and DB integrity | 380-403 | 10 | TODO |
| 6 | Phase 5 — Upgrade and HTTP safety | 404-413 | 4 | TODO |
| 7 | Phase 6 — cfg correctness | 414-448 | 10 | TODO |
| 8 | Phase 7 — Makefile converter truth | 449-464 | 7 | TODO |
| 9 | Phase 8 — TUI and CLI surface | 465-485 | 10 | TODO |
| 10 | Phase 9 — Repo and dependency hygiene | 486-502 | 7 | TODO |
| 11 | Phase 10 — Doc truth | 503-515 | 8 | TODO |
| 12 | Phase 11 — Test-gap closure | 516-589 | 10 | TODO |
| 13 | Acceptance Criteria | 590-638 | 8 | TODO |
| 14 | Resolved Decisions that assert something about code | 639-662 | 21 | TODO |

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

**Consequence:** the panel lead must re-run every behavioral claim itself.
Both useful batches worked because the lead did exactly that. Budget for it.

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

## Standing rules for whoever continues

### Verification standard
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
