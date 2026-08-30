# Migration: Strict Ottofile Schema

`docs/design/2026-08-29-strict-ottofile-schema.md` made an unrecognized
ottofile key a loud, fail-closed config-load error instead of a silent
no-op, and made `otto.api` load-bearing. After this ships, three work repos
fail to load until each gets a one-line fix, in its own repo, by its own
owner. `otto` itself does not change again for this; no coordinated release
across repos is required.

## The three affected repos

| repo | offending key | fix |
|---|---|---|
| `work repo A` | root `envs:` | Move the block under `otto:` (i.e. `otto.envs:`). |
| `work repo B` | root `envs:` | Move the block under `otto:` (i.e. `otto.envs:`). |
| `work repo C` | `tasks.dev.timeout` | Delete the key. No such key exists on any task, under any name, in this or any prior schema. |

`scottidler/otto-old` is dead and needs nothing.

The three repos are identified generically here because this repository is
public. Each owner knows which one is theirs from the offending key.

## Why the first two are not cosmetic

Root `envs:` has never been a real key (see
`docs/commands/ottofile-reference.md`). Both `work repo A` and
`work repo B` wrote environment variables there under a
`# Environment variables` comment, and neither block has ever applied:
before this change, the unknown root key was silently discarded and the
tasks ran with those variables `[UNSET]`.

Moving the block under `otto:` makes it a real, honored key for the first
time. Concretely, `work repo A` would gain `GOMODCACHE`, `GOCACHE`,
`VERSION`, and `BUILD` in every task, values it has never actually had. **That
is a behavior change**, not a syntax fix, and needs testing in that repo, by
its owner, before merging.

The conservative alternative — delete the block instead of moving it —
preserves today's behavior exactly (those variables stay unset, same as
now) and requires no testing beyond confirming the ottofile loads. Either
fix satisfies strict parsing; only one of them changes what the tasks
actually do.

## `work repo C`

`tasks.dev.timeout` has no meaning in any otto schema version and was always
discarded. Deleting it is the only fix; there is no key to move it to,
because a task-level execution timeout does not exist on the schema (see
`docs/commands/ottofile-reference.md`'s `tasks.<name>:` table). This one is
purely cosmetic: removing an already-inert key changes nothing about how the
task runs.

## After the fix

Confirm the ottofile still loads:

```bash
otto --tasks
```

A clean JSON task listing (rather than an `unknown field '...'` error) means
the fix landed correctly.


## Second wave: `otto.home` and `otto.verbosity` removed

The remediation plan's Phase 10 resolved the inert `otto:` keys, per its
mandate that each one be "either wired to real behavior or deleted from the
schema" — accepted-and-ignored-but-documented being the worst of the three
states. `jobs`, `name`, and `about` were wired. **`home` and `verbosity` were
deleted**, having zero readers, zero writers, and zero committed usage;
`home` also duplicated `$OTTO_HOME`, which is now the single knob for both the
run tree and the database.

**This is a breaking change for any ottofile that sets them**, because
`deny_unknown_fields` turns an unrecognized `otto:` key into a config-load
error rather than a silent no-op. Observed:

```
$ otto build          # ottofile with `otto: home: ~/.otto-custom`
otto: unknown field `home`, expected one of `name`, `about`, `api`, `jobs`,
`tasks`, `envs`, `retention` at line 3 column 3
exit 1
```

**The fix is deletion, and it costs nothing.** Neither key has ever done
anything: no code read either value in any released version, so removing the
line cannot change how any task runs. This is the same shape as the
`tasks.dev.timeout` case above — an already-inert key whose removal is purely
cosmetic — not the `envs:` case, where moving the block changes behavior.

If you were setting `home:` because you wanted otto's state somewhere else,
that is what `OTTO_HOME` is for, and as of the state-integrity work it moves
the run directories **and** the database together.
