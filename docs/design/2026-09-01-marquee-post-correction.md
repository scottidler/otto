# Marquee post correction: v2.1.0 teardown sentence

**Status:** PUBLISHED 2026-09-02. Approved by Scott, applied, and verified against the live post.

## Post

`https://marquee.internal.tatari.dev/p/~scott-idler/otto-v2-1-0-i-built-all-three-the-bone-is-clean/`

Section 5, "Ctrl+C on a plain run", first bullet.

## The sentence this file originally named did not exist

This file was staged during Phase 5 while `marquee read` was failing on Okta auth in a
non-interactive session. With no way to re-fetch the post, it quoted the sentence recorded in the
design doc's own Problem Statement finding 1:

> They die from `kill_on_drop(true)` when `abandon_run` aborts them.

**That string is not in the post, and never was.** The re-fetch after `marquee login` is what
found it. The post's actual bullet read:

> - **otto does not forward the signal to your task children.** A terminal Ctrl+C already goes to
>   the whole foreground process group, and otto puts every task child in its own group, so the
>   children do not get it and cannot race otto's teardown. **They die with the parent.** A
>   `tty: true` task is the exception, since it deliberately stays in otto's group to own the
>   terminal.

So the false claim was **"They die with the parent."**

Two consequences, both recorded in the design doc and the implementation notes:

1. The design doc's finding 1 carried a misquote of a post it cited as measured evidence.
2. Phase 5's success criterion (c) was `marquee read <url> | grep -c 'They die from'` returning 0.
   It returned 0 *before any edit*, so it could not fail. A criterion that names a string the
   target never contained verifies nothing. Amended to grep the sentence the post actually
   carried.

## Why the claim was wrong

`kill_on_drop(true)` (`src/executor/task_execution.rs`) sends SIGKILL to the **direct** task child
only, when its `Child` handle is dropped. Every task child was already made a process-group leader,
but nothing signalled the group: `grep -rn 'killpg\|Pid::from_raw(-' src/` returned zero hits on the
pre-Phase-1 tree. A grandchild (`bash -> inner otto -> docker compose logs`) survived Ctrl+C
indefinitely, which for `otto logs` meant a leaked inner otto and compose tail per service, on every
interrupt.

Phase 1 fixed it by reaping the whole process group from `abandon_run`: SIGTERM to `-pgid`, a grace
period, then SIGKILL over the same snapshot.

## What was published

The bullet's third sentence was replaced with a dated, visible correction rather than a silent
rewrite, because the original text is what told readers descendants die and some of them run
`otto logs`:

> **Correction, 2026-09-02:** "They die with the parent" was only ever true of the direct child.
> `kill_on_drop(true)` reaches the process otto spawned and nothing below it, so a task body's own
> grandchildren survived the interrupt: for `otto logs`, an inner otto and a compose tail per
> service, every time. [v2.2.0](https://github.com/otto-rs/otto/releases/tag/v2.2.0) reaps the whole
> group instead: SIGTERM to the process group, a grace period, then SIGKILL. A `tty: true` task is
> still the exception, since it deliberately stays in otto's group to own the terminal, and for that
> reason its descendants are not group-reaped.

The `tty: true` carve-out is stated because it is real and permanent: that task deliberately stays
in otto's own process group, so otto cannot group-signal it without signalling itself. Acceptance
Criterion 1 was amended for the same reason.

## Verification, on the live post

Published with `marquee replace <url> <dir>` against a directory whose entry document was the
post's own `marquee read` output with exactly one line changed (diff was 4 lines; the replacement
anchor was asserted to match exactly once before the edit).

```console
$ marquee read <url> | grep -c 'They die with the parent\.'
0
$ marquee read <url> | grep -c 'reaps the whole group instead: SIGTERM to the process group'
1
```

Phase 5 criterion (c) as amended: **PASS**.

Note on the grep: the trailing period is load-bearing. The published correction quotes the old
wording inside itself, so an unanchored search for the bare phrase still matches by design.
