# Param Propagation: Syntax Options

## Physical Mechanism

Param propagation is **compile-time**. It happens in `parser.rs` when building
the `Task` structs, before any process spawns. The propagated value gets
inserted into `task.values` and `task.envs` exactly like a CLI-provided value
or a default. The generated script prologue includes it as a variable
assignment. Nothing changes in the runtime output/input/symlink machinery —
no new files or symlinks are needed.

## Current State (the problem)

Today the only way to get a value into a dependency is to name it explicitly
on the CLI:

```
otto deploy --account work build --account work
```

This defeats the purpose of `before: [build]`. The dep should receive values
from its parent automatically.

## YAML Syntax Options

### Option A: Implicit Name-Matching (no new syntax)

```yaml
deploy:
  params:
    --account:
      default: home
      choices: [home, work]
  before: [build]
  bash: |
    cd ${account} && clasp push

build:
  params:
    --account:
      default: home
      choices: [home, work]
  bash: |
    node scripts/build.mjs --account ${account}
```

`otto deploy --account work` — parser resolves `deploy.account = work`, sees
`build` also declares `account` with no CLI override, inherits `work`.

**Pros:** Zero new syntax. Convention-based. Matches Make's model.

**Cons:** Implicit — renaming a param silently breaks propagation. Can't map
different names (`deploy.env` -> `build.target`).

### Option B: Explicit `pass` Map on `before`

```yaml
deploy:
  params:
    --account:
      default: home
  before:
    - task: build
      pass:
        account: ${account}
  bash: |
    cd ${account} && clasp push

build:
  params:
    --account:
      default: home
  bash: |
    node scripts/build.mjs --account ${account}
```

**Pros:** Explicit, visible in YAML. Can map different names
(`pass: {target: ${account}}`). No magic.

**Cons:** `before` becomes a mixed type (string or object). More verbose.

### Option C: CLI-Style Args in `before`

```yaml
deploy:
  params:
    --account:
      default: home
  before:
    - "build --account ${account}"
  bash: |
    cd ${account} && clasp push
```

**Pros:** Intuitive — looks like what you'd type on the CLI. Familiar.

**Cons:** String parsing. Mixes task names with args. Harder to validate at
parse time.

### Option D: `propagate` Field (opt-in list)

```yaml
deploy:
  params:
    --account:
      default: home
  before: [build]
  propagate: [account]
  bash: |
    cd ${account} && clasp push
```

**Pros:** Explicit opt-in. Simple. `before` stays a clean list.

**Cons:** Still name-matching under the hood. New top-level field.

## Recommendation: Option A

The dep must declare the param. Propagation only happens when:

1. Parent has a resolved param `X`
2. Dep declares a param named `X`
3. Dep wasn't given an explicit value on the CLI

This keeps it safe — you can't accidentally inject values into a task that
doesn't expect them. The param declaration on `build` acts as the contract.
And it requires zero new YAML syntax — the entire feature lives in `parser.rs`
resolution logic.

The implicit model also sidesteps a subtle problem with Options B/C: if you
have a chain `deploy -> build -> lint` and `lint` also needs `account`,
explicit syntax forces you to thread `pass` through every intermediate task.
Implicit propagation flows transitively through the whole dep chain for free,
just like Make's target-specific variables.

Option B is a solid alternative if explicitness is preferred over convenience.
The two are not mutually exclusive — start with A and add B later as an
override mechanism for name-mapping cases.

## Diamond Problem (from design doc)

If two parents propagate conflicting values to the same dep, fail with an
error. See `param-propagation-design.md` for full analysis.
