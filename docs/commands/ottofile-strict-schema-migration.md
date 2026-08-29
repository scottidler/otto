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
| `tatari-tv/devs` | root `envs:` | Move the block under `otto:` (i.e. `otto.envs:`). |
| `tatari-tv/github-setup` | root `envs:` | Move the block under `otto:` (i.e. `otto.envs:`). |
| `tatari-tv/auth-svc` | `tasks.dev.timeout` | Delete the key. No such key exists on any task, under any name, in this or any prior schema. |

`scottidler/otto-old` is dead and needs nothing.

## Why the first two are not cosmetic

Root `envs:` has never been a real key (see
`docs/commands/ottofile-reference.md`). Both `tatari-tv/devs` and
`tatari-tv/github-setup` wrote environment variables there under a
`# Environment variables` comment, and neither block has ever applied:
before this change, the unknown root key was silently discarded and the
tasks ran with those variables `[UNSET]`.

Moving the block under `otto:` makes it a real, honored key for the first
time. Concretely, `tatari-tv/devs` would gain `GOMODCACHE`, `GOCACHE`,
`VERSION`, and `BUILD` in every task, values it has never actually had. **That
is a behavior change**, not a syntax fix, and needs testing in that repo, by
its owner, before merging.

The conservative alternative — delete the block instead of moving it —
preserves today's behavior exactly (those variables stay unset, same as
now) and requires no testing beyond confirming the ottofile loads. Either
fix satisfies strict parsing; only one of them changes what the tasks
actually do.

## `tatari-tv/auth-svc`

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
