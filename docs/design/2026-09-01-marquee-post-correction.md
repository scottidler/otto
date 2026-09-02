# Marquee post correction: v2.1.0 teardown sentence

**Status:** Proposed, NOT published. Requires Scott's approval before edit — the post is outward-facing.

## Post

`https://marquee.internal.tatari.dev/p/~scott-idler/otto-v2-1-0-i-built-all-three-the-bone-is-clean/`

Section 5.

## Verification attempted, and failed

`marquee read` was run against the URL above to re-fetch the live text before drafting this
correction:

```
$ marquee read "https://marquee.internal.tatari.dev/p/~scott-idler/otto-v2-1-0-i-built-all-three-the-bone-is-clean/"
Error: authentication failed: Okta token is missing or expired and no controlling terminal is
available (non-interactive session). Force the device authorization grant to log in - it needs no
terminal (prints a code + URL, you approve on any device) - then retry.. Try `marquee login`.
```

No interactive terminal is available in this session to complete the Okta device-grant login, so
the live post was NOT re-fetched. The sentence quoted below is the one recorded, with citation,
in the design doc itself (`docs/design/2026-09-01-cancellation-reaping-and-foreach-concurrency.md`,
Problem Statement, finding 1): "It also falsifies a sentence in the v2.1.0 release post:
'They die from `kill_on_drop(true)` when `abandon_run` aborts them.'" That citation was measured
against the post before this doc was written, not guessed — but it was not re-verified as part of
this phase. Before publishing, re-run `marquee read` (after `marquee login`) and confirm this
sentence is still exactly what section 5 contains.

## Current sentence (as recorded in the design doc, section 1)

> They die from `kill_on_drop(true)` when `abandon_run` aborts them.

## Why it's wrong

`kill_on_drop(true)` sends SIGKILL to the **direct** task child only when its `Child` handle is
dropped. Every task child is made a process-group leader (`task_execution.rs:206-208`), but
nothing signalled the group: `grep -rn 'killpg|Pid::from_raw(-' src/` returned zero hits on the
pre-Phase-1 tree. A grandchild (e.g. `bash -> inner otto -> docker compose logs`) survived
Ctrl+C indefinitely. Phase 1 of this design fixed that by reaping the whole process group
(`killpg` SIGTERM, grace, `killpg` SIGKILL) from `abandon_run`, not by relying on `kill_on_drop`
alone.

## Proposed replacement sentence

> They die because `abandon_run` sends the process group a SIGTERM, waits a short grace period,
> then SIGKILLs the group — not because of `kill_on_drop(true)` alone, which only reaches the
> direct child.

This names the process group explicitly (the thing the original sentence omitted) and keeps
`kill_on_drop(true)` in the sentence as the backstop it actually is, rather than removing it and
implying it plays no role.

## Action required

Do NOT edit or republish the post as part of this phase. Scott approves the replacement wording
(or supplies his own), then publishes the correction himself via `marquee update` — this is
outward-facing content and per the phase brief requires separate approval.
