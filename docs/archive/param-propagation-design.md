# Design Document: Param Propagation to Dependencies

**Author:** Scott Idler
**Date:** 2026-02-22
**Status:** Draft
**Review Passes Completed:** 5/5

## Summary

Otto has no mechanism to pass parameter values from a parent task down to its
`before` dependencies. This design adds compile-time param propagation using
implicit name-matching (Option A): when a parent task has a resolved param and
its dependency declares a param with the same name but wasn't given an explicit
CLI value, the dependency inherits the parent's value. The entire feature lives
in `parser.rs` with zero new YAML syntax.

## Problem Statement

### Background

Otto tasks can declare parameters (`params`) and depend on other tasks (`before`).
Values flow *upward* from deps to dependents via output/input files, but there
is no mechanism to flow values *downward* from a parent to its deps.

GNU Make solved this in the mid-90s with **target-specific variables**, which
propagate to all transitive prerequisites via dynamic scoping.

### Problem

Today, the only way to pass a param value to a dependency is to name it
explicitly on the CLI:

```
otto deploy --account work build --account work
```

This defeats the purpose of `before: [build]`. The dep should receive values
from its parent automatically.

### Current Workarounds

| Mechanism | Direction | Why it fails |
|---|---|---|
| **pargs** | CLI -> named task | Only covers tasks named on CLI; deps get defaults |
| **output/input** | dep -> dependent (upward) | Opposite direction — deps run first |
| **task-level envs** | scoped to one task | Don't propagate to deps |

The practical workaround is **inlining**: remove the dep from `before` and call
it directly in the task's script body, losing graph scheduling benefits.

### Goals

- Pass param values downward from parent tasks to `before` dependencies
- Support transitive propagation through chains (deploy -> middle -> build)
- Detect and reject conflicting values in diamond dependencies
- Require zero new YAML syntax

### Non-Goals

- Name-mapping between different param names (deploy.env -> build.target)
- Pass-through propagation when intermediate tasks don't declare the param
- Cloning graph nodes for diamond deps with conflicting values
- Runtime propagation — this is purely compile-time in the parser

## Terminology

In Otto's dependency graph, `before: [build]` on `deploy` means "build must
run before deploy." For propagation, we use these terms:

- **Dependent** (or **propagation source**): the task that has `before: [X]`.
  In `deploy.before = [build]`, `deploy` is the dependent.
- **Dependency** (or **propagation target**): the task listed in `before`.
  `build` is the dependency.

Values propagate from dependents down to their dependencies — the reverse of
execution order.

## Proposed Solution

### Overview

Implicit name-matching propagation: when the parser resolves params for a
dependency, if the dependent has a resolved param with the same name and the
dependency declares that param but wasn't given an explicit CLI value, the
dependency inherits the dependent's resolved value. The dependency's param
declaration acts as the contract — you can't accidentally inject values into a
task that doesn't expect them.

### Resolution Order

The key insight is that param resolution must happen in distinct phases to
avoid defaults blocking inheritance:

```
Phase 1: Apply CLI-provided values
Phase 2: Propagate along dependency edges (for params still not set)
Phase 3: Apply defaults for still-unset params, then validate choices/types
```

If defaults are applied eagerly (before propagation), a dep's param looks "set"
even though it's just a default, which blocks inheritance from the parent. The
internal distinction is:

| State | Meaning |
|---|---|
| **Provided** | Set from CLI or from propagation |
| **Not provided** | Eligible for inheritance |
| **Defaulted** | Applied only after propagation, if still not provided |

### Transitive Propagation

Propagation flows transitively through dependency chains as long as each
intermediate task declares the param:

```yaml
deploy:
  params: { --account: { default: home } }
  before: [middle]
  bash: cd ${account} && clasp push

middle:
  params: { --account: { default: home } }
  before: [build]
  bash: echo "middleware for ${account}"

build:
  params: { --account: { default: home } }
  bash: node scripts/build.mjs --account ${account}
```

Running `otto deploy --account work`:

1. Phase 1: `deploy.account = work` (CLI)
2. Phase 2: `middle.account = work` (propagated from deploy, middle declares it, no CLI override)
3. Phase 2: `build.account = work` (propagated from middle, build declares it, no CLI override)
4. Phase 3: No defaults needed — all `account` params already have values

If `middle` did not declare `--account`, the chain breaks at `middle`. `build`
would not receive `work` because `middle` has nothing to propagate. This is the
safety contract: you can't accidentally inject values into a task that doesn't
expect them, and you can't silently forward values through tasks that don't
participate.

### Algorithm

The propagation step runs inside `process_tasks_with_filter` in `parser.rs`.
The current code iterates `tasks_needed` and resolves each task independently.
The change introduces graph-aware ordering:

```
1. Compute task_deps (existing code, line 681)
2. Build reverse index: for each task, which tasks list it as a dep
3. Reverse-topological-sort tasks_needed
   (dependents before their deps: deploy, middle, build)
4. For each task in this order:
   a. Apply CLI-provided values (Phase 1)
   b. Track which params were CLI-provided in a HashSet<String>
   c. For each param NOT CLI-provided (Phase 2):
      - Look up parent tasks (dependents) via reverse index
      - Collect resolved values for this param name from all parents
      - If all parents agree on a value: inherit it
      - If parents disagree: error with clear message
   d. Apply defaults for any still-unset params (Phase 3)
   e. Validate choices/types
```

Note on ordering: `task_deps[deploy] = [build]` means deploy depends on build
(build runs first). But for *propagation*, deploy is the value source. So we
process deploy before build — the reverse of execution order. By the time we
process `build`, `middle` already has its propagated value, so the transitive
chain works naturally.

#### Pseudocode

```rust
// Step 1: Build reverse index (dep -> list of dependents that list it)
// task_deps: { deploy: [middle], middle: [build] }
// reverse:   { middle: [deploy], build: [middle] }
let mut dependents_of: HashMap<String, Vec<String>> = HashMap::new();
for (task_name, deps) in &task_deps {
    for dep in deps {
        dependents_of.entry(dep).or_default().push(task_name);
    }
}

// Step 2: Process tasks in propagation order (dependents before deps)
// For deploy -> middle -> build: process deploy, then middle, then build
let ordered = topo_sort_propagation_order(&task_deps, &tasks_needed);

// Step 3: Resolve each task
let mut resolved: HashMap<String, HashMap<String, String>> = HashMap::new();
let mut cli_provided: HashMap<String, HashSet<String>> = HashMap::new();

for task_name in &ordered {
    let spec = &expanded_tasks[task_name];

    // Phase 1: CLI values
    if let Some(args) = pargs_for(task_name) {
        for param in &spec.params {
            if cli_has_value(args, param) {
                resolved[task_name].insert(param.name, cli_value);
                cli_provided[task_name].insert(param.name);
            }
        }
    }

    // Phase 2: Propagation (only for params not CLI-provided)
    for param in &spec.params {
        if cli_provided[task_name].contains(&param.name) { continue; }

        let mut inherited_values = Vec::new();
        for dependent in dependents_of.get(task_name).unwrap_or(&vec![]) {
            if let Some(value) = resolved[dependent].get(&param.name) {
                inherited_values.push((dependent, value));
            }
        }

        match inherited_values.len() {
            0 => {} // no propagation, will fall through to defaults
            _ => {
                // Check all values agree
                let first = inherited_values[0].1;
                if inherited_values.iter().all(|(_, v)| v == first) {
                    resolved[task_name].insert(param.name, first);
                } else {
                    // Diamond conflict — error
                    return Err(conflict_error(task_name, param, &inherited_values));
                }
            }
        }
    }

    // Phase 3: Defaults for still-unset params
    for param in &spec.params {
        if !resolved[task_name].contains_key(&param.name) {
            if let Some(default) = &param.default {
                resolved[task_name].insert(param.name, default);
            }
        }
    }
}
```

### Diamond Problem

When two parents propagate conflicting values to the same dep. This only
manifests when both parents are in the same execution graph (e.g.,
`otto deploy-staging deploy-prod`):

```yaml
deploy-staging:
  params: { --account: { default: staging } }
  before: [build]

deploy-prod:
  params: { --account: { default: prod } }
  before: [build]
```

Otto deduplicates `build` to one graph node. Three options:

| Approach | How | Tradeoff |
|---|---|---|
| **Conflict = error** | Two parents disagree → fail loudly | Safe, simple, covers 90% of cases |
| **Clone** | Create `build@deploy-staging` and `build@deploy-prod` | Clean but deps run multiple times |
| **First parent wins** | Nondeterministic | Bad idea |

**Decision:** Conflict = error. If two parents propagate different values for
the same param to a shared dep, the parser fails with a clear error message
identifying the conflicting tasks and values. Clone behavior can be added later
as an opt-in.

Agreement is fine: if both parents propagate `account = work`, `build` gets
`work` with no error.

### Data Model Changes

No changes to `Task`, `TaskSpec`, or `ParamSpec` structs. The resolved values
are stored in `task.values` and `task.envs` exactly as they are today — the
only difference is *when* values are inserted (after propagation instead of
immediately from defaults).

Internal tracking of "provided vs not-provided" can use a temporary
`HashSet<String>` per task during resolution, discarded after parsing completes.

### Implementation Plan

All changes are in `parser.rs`, localized to the `process_tasks_with_filter`
method (lines 663-778):

**Phase 1: Restructure resolution order**
- Split the current param resolution loop into two passes: CLI values first,
  defaults after
- Track which params were CLI-provided per task using a HashMap<String, HashSet<String>>
- No behavior change yet — just restructuring

**Phase 2: Add propagation**
- After CLI values are applied, iterate tasks in topological order
- For each task, find parents (reverse lookup from task_deps)
- For each non-CLI-provided param, check if any parent has a resolved value
  with the same name
- Insert inherited values into `task.values` and `task.envs`
- Detect and error on diamond conflicts

**Phase 3: Tests**
- Unit test: single-level propagation (deploy -> build)
- Unit test: transitive propagation (deploy -> middle -> build)
- Unit test: chain breaks when intermediate doesn't declare param
- Unit test: CLI override on dep prevents inheritance
- Unit test: diamond conflict produces error
- Unit test: diamond agreement succeeds
- Integration test: end-to-end with `.otto.yml`

## Alternatives Considered

### Alternative B: Explicit `pass` Map on `before`

```yaml
deploy:
  params: { --account: { default: home } }
  before:
    - task: build
      pass: { account: ${account} }
```

- **Pros:** Explicit, visible in YAML. Supports name-mapping (e.g.,
  `pass: {target: ${account}}`). No magic.
- **Cons:** `before` becomes a mixed type (string or object). More verbose.
  Forces explicit threading through every intermediate in a chain.
- **Why not chosen:** Safety is already provided by the dep's param declaration.
  Adding explicit syntax doubles the ceremony without doubling the safety. Can
  be added later as an override mechanism for name-mapping cases.

### Alternative C: CLI-Style Args in `before`

```yaml
deploy:
  before: ["build --account ${account}"]
```

- **Pros:** Intuitive — looks like CLI invocation. Familiar to users.
- **Cons:** String parsing in YAML. Mixes task names with args. Harder to
  validate at parse time. Same threading problem as Option B.
- **Why not chosen:** Fragile string parsing; validation complexity.

### Alternative D: `propagate` Field (opt-in list)

```yaml
deploy:
  before: [build]
  propagate: [account]
```

- **Pros:** Explicit opt-in. Simple. `before` stays a clean list.
- **Cons:** Still name-matching under the hood. New top-level field. Doesn't
  add safety beyond what the dep's param declaration already provides.
- **Why not chosen:** The dep's declaration is already the opt-in. A second
  opt-in on the parent side is redundant.

## Technical Considerations

### Dependencies

No new crate dependencies. The topological sort can use the existing `daggy`
crate already in the dependency graph, or a simple Kahn's algorithm on the
`task_deps` HashMap.

### Performance

Negligible. Propagation adds one extra pass over the task set (typically <20
tasks). The parent-lookup is a reverse scan of `task_deps` which is O(n*m) but
n and m are tiny.

### Testing Strategy

- **Unit tests** in `parser.rs` covering all propagation scenarios (listed in
  Implementation Plan Phase 3)
- **Integration tests** using `.otto.yml` fixtures with `before` chains and
  params, verifying resolved `task.values` and generated script content
- **Error message tests** for diamond conflicts, ensuring the error identifies
  the conflicting tasks and param name

## Edge Cases

### CLI override prevents inheritance

`otto deploy --account work build --account staging` — `build` has an explicit
CLI value. Phase 2 skips it because `cli_provided[build]` contains `account`.
Build uses `staging`, not `work`. This is correct: explicit always wins.

### Choices validation on propagated values

If `deploy` propagates `account = work` to `build`, but `build` declares
`choices: [home, staging]`, the validation in Phase 3 catches the mismatch and
errors. The error should say the propagated value is not in the allowed choices,
naming both the source task and the target task.

### Dep invoked directly without dependent

`otto build` — `deploy` is not in the execution graph. No dependent exists for
`build`, so no propagation occurs. `build` gets its default. Correct behavior.

### `after` relationships

`build.after = [deploy]` is equivalent to `deploy.before = [build]`.
`compute_task_deps_from_specs` already normalizes `after` into `before`, so
the reverse index correctly identifies `deploy` as a dependent of `build`.
Propagation works identically.

### Foreach tasks

Foreach subtasks inherit params from their parent spec during expansion
(`expand_foreach_tasks_with_serial`). The virtual parent is skipped during
task creation. If a non-foreach task depends on a foreach parent, the
subtasks are the real deps in the graph. Propagation targets the subtasks,
not the virtual parent.

### Multiple params, partial propagation

Propagation is per-param, not per-task. `deploy` might propagate `account` to
`build` but not `region`, if `build` doesn't declare `region`. Each param is
resolved independently.

## Risks and Mitigations

| Risk | Likelihood | Impact | Mitigation |
|------|------------|--------|------------|
| Implicit propagation confuses users who don't expect it | Medium | Low | Propagation only occurs when dep declares the param — the declaration is the contract. Document the behavior. |
| Renaming a param silently breaks propagation | Medium | Medium | Names must match. Consider a lint/warning when a parent has a param that a dep doesn't declare (future enhancement). |
| Topological sort ordering affects which parent "wins" in non-conflict cases | Low | Low | Agreement check: all parents must agree. Disagreement is always an error, regardless of order. |
| Foreach subtasks need special handling | Medium | Medium | Foreach subtasks inherit the parent's params. Propagation should treat subtasks the same as regular tasks — they already have params from expansion. |

## Open Questions

- [x] Should `after` relationships also propagate params? Yes — `compute_task_deps_from_specs` normalizes `after` into `before`, so propagation works identically with no special handling. See Edge Cases.
- [ ] Should propagation work with `foreach` virtual parents? Virtual parents are skipped during task creation. Propagation should target the expanded subtasks directly. Needs verification during implementation.

## References

- [Param Propagation Syntax Options](param-propagation-syntax.md) — detailed
  syntax analysis of Options A-D
- GNU Make target-specific variables: https://www.gnu.org/software/make/manual/html_node/Target_002dspecific.html
- `src/cli/parser.rs` lines 663-778 — current param resolution code
