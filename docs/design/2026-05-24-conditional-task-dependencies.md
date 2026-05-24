# Design Document: Conditional Task Dependencies

**Author:** Scott Idler
**Date:** 2026-05-24
**Status:** Implemented
**Review Passes Completed:** 5/5 + 4 Architect rounds

## Summary

Generalize otto's task-to-task dependency edges (`before:`, `after:`) from `Vec<String>` to `Vec<EdgeSpec>`, where each edge carries an optional `when:` condition (`success`, `failure`, or `always`). The default `when:` is `success`, preserving every existing ottofile's behavior verbatim. New conditions enable:

- **`when: failure`** - fixer/fallback tasks (cargo fmt auto-apply, ruff `--fix`, prettier `--write`).
- **`when: always`** - cleanup tasks that must run regardless of upstream outcome (tmpdir teardown, log shipping, status reporting).

The user-facing `on-failure:` field from v2 is kept as **parse-time sugar**: `on-failure: [Y]` on host X is rewritten to `{task: X, when: failure}` on Y's `after:` list before the rest of the pipeline sees it. This preserves the host-named ergonomic surface (X declares what fires when it fails, locally) while the mechanism underneath is the general conditional-edge model. Scheduler gains an edge-condition check at dependency satisfaction time, an unreachability sweep that marks tasks as `Skipped` when their `when:` predicate can never be true, and a drain-on-failure loop that lets in-flight tasks and conditional follow-ups complete before the run returns its error.

## Problem Statement

### Background

Otto's runtime pipeline is two-stage. Parsing (`src/cli/parser.rs`) walks the requested tasks plus their `before:`/`after:` transitive closure, builds a flat `task_deps` map, resolves params/envs/foreach, and produces a `Vec<executor::Task>`. Scheduling (`src/executor/scheduler.rs`) takes that vec, builds `in_degree`/`ready_queue`/`blocked_tasks` machinery, and runs the DAG using `task.task_deps` as the only source of edge information. On task failure (`scheduler.rs:415-446`), the scheduler logs, broadcasts to the TUI, and returns `Err(e)` - halting the entire run, not just the failed branch.

Every edge in this pipeline is implicitly `when: success`. If a dep fails, dependents stay blocked forever and the scheduler exits. There is no way to express "run only if X failed" or "run regardless of X's outcome." Users who want either pattern today end up with one of three workarounds, all of which compromise the task model:

- **Fixer logic conflated with check logic.** `cargo fmt --check` and its fixer `cargo fmt` live inside the same task body - the fixer can't be invoked on its own, doesn't show up in `otto Graph`, doesn't appear in run history.
- **Cleanup via bash `trap`.** Every project reimplements `mktemp -d` + `trap rm -rf $TMP EXIT`. Cleanup never appears as its own task.
- **CI vs. local branching via env-var sniffing.** Whether a fixer should run is decided by ad-hoc `[ -z "$CI" ]` checks scattered through task bodies, rather than by the structure of the graph.

### Problem

There is no first-class way to express "run task Y conditional on the outcome of task X." The two specific shapes that matter today:

1. **Run Y only if X failed** - the cargo-fmt auto-fix case.
2. **Run Y regardless of X's outcome** - the cleanup case.

A previous design ([v2 of this doc](#alternative-1-on-failure-only-the-narrow-version-of-this-design)) added an `on-failure:` field that solved (1) but not (2), required scheduler changes that overlap heavily with what (2) would need, and introduced a separate "hook" task category alongside normal tasks. The general mechanism (conditional edges) handles both shapes with one schema change and one scheduler change, and `on-failure:` survives as parse-time sugar so the ergonomic surface from v2 is preserved.

### Goals

- Promote dependency edges from `Vec<String>` to `Vec<EdgeSpec>` where `EdgeSpec = { task: String, when: When }` and `When ∈ {Success, Failure, Always}`.
- Default `when: Success` so every existing ottofile parses, behaves, and round-trips identically.
- Accept polymorphic YAML: bare strings (sugar for `when: Success`) and full `{task, when}` objects, in the same list.
- Round-trip preservation: a `TaskSpec` deserialized from bare strings serializes back as bare strings. A `TaskSpec` deserialized from `{task, when: failure}` serializes back in the long form. Mixed lists preserve per-element style.
- Scheduler gains: edge-condition check at dep-satisfaction time, unreachability marking, drain-on-failure.
- Keep `on-failure:` as a parse-time sugar surface that desugars to `after: [{task: host, when: failure}]` on the named target task.
- `otto Graph` renders edge conditions distinctly (line style or color).

### Non-Goals

- Conditions on `input:` / `output:` (file globs, not task references - different concept).
- Conditions on `foreach.items` (data, not task edges).
- Retry semantics. `when: failure` does not re-run the failed task; the original task stays failed.
- Per-edge `when:` clauses that depend on multiple upstreams (`when: any-failure-of-X-or-Y`). One source per edge.
- Custom `When` variants beyond `Success | Failure | Always`. No `when: timeout`, no `when: cancelled`, etc. The exit-status model is binary.
- Catching otto-internal failures (config parse error, missing binary, scheduler crash). `when: failure` fires only on a non-zero exit from the task's action script.

## Prerequisites

### Pre-existing virtual-parent deadlock (must fix before v3 ships)

Architect review round 2 identified a latent bug in the existing parser that v3 will exacerbate. Verified against the code:

- `Parser::process_tasks_with_filter` skips virtual-parent tasks from `task_entries` (`parser.rs:733-735`: `if task_spec.virtual_parent { continue; }`).
- `task.task_deps` is assigned from the raw `task_deps` map (`parser.rs:784`: `task.task_deps = task_deps.get(task_name).map(|deps| deps.to_vec()).unwrap_or_default()`).
- That raw map (from `compute_task_deps_from_specs`, `parser.rs:1053-1087`) contains virtual-parent task names verbatim - there is no flattening pass between `compute_task_deps_from_specs` and the executor.
- `build_filtered_deps` (`parser.rs:956-982`) DOES flatten virtual parents into their subtasks, but it is only used inside `propagate_params` for param propagation - its output never reaches the executor's `task_deps`.

Concrete failure: write `ci: { before: [install] }` where `install` is a foreach task. The scheduler builds `in_degree[ci] = 1`, ci goes to `blocked_tasks` waiting on `install`, but `install` was filtered out of the task vec. The loop's `completed_tasks < total_tasks` condition never advances. Deadlock.

This bug is dormant today because no committed ottofile happens to depend on a foreach task by parent name. v3 makes it newly reachable: `on-failure: [<foreach-task>]` and `after: [{task: <foreach-task>, when: failure}]` both produce dep entries that reference the parent name, hitting the same deadlock the moment anyone tries to wire a `when: failure` dependent onto a foreach task. Same problem for `when: always` cleanup attached to a foreach test target.

**Resolution (Architect-consensus, round 3):** make virtual parents real aggregating tasks, with the gating implemented through `When::Always` edges (not a special scheduler branch). The transformation runs in the parser during foreach expansion:

- Subtasks **inherit** the parent's `before:` edges (so each subtask waits for the parent's original prerequisites before starting).
- Subtasks **do NOT** inherit the parent's `after:` edges (only the parent triggers downstreams; subtasks don't double-fire).
- The parent's `before:` is **replaced** with `[{task: subtask_1, when: always}, ..., {task: subtask_N, when: always}]`. Because `When::Always` is satisfied by Completed | Failed | Skipped, the scheduler's existing dep-gating queues the parent once all subtasks reach a terminal state - no new branch needed in `blocked_tasks` logic.
- When the parent executes (empty action), the scheduler's task-completion handler evaluates the subtask statuses and overrides the parent's nominal Completed status with the aggregation rule:
  - ANY subtask `Failed` → parent `Failed`.
  - Else ANY subtask `Skipped`, none `Failed` → parent `Skipped`.
  - Else (all `Completed`) → parent `Completed`.

This is the only new scheduler logic - a post-action evaluation at virtual-parent completion. Everything else flows through the conditional-edge machinery v3 introduces anyway.

**Why this resolves the foreach-with-failure-conditional question cleanly:** a downstream `{task: <foreach-parent>, when: failure}` now reads the parent's aggregated status. The dependent fires when at least one subtask failed, which is exactly the user's natural expectation ("if any foreach subtask broke, run the fixer"). No grouped-ANY logic in the scheduler.

**Why skipped doesn't fire failure-conditional dependents:** if subtasks were Skipped because a prerequisite failed (subtasks never started), the parent aggregates as Skipped, not Failed. A `when: failure` edge to the parent is then unreachable (Skipped is neither Completed nor Failed), so the dependent doesn't fire. This matches user expectation: don't run the fixer if the foreach group never actually started.

**Counter-option considered (and rejected):** flatten `task.task_deps` to all subtasks at parse time and teach the scheduler that a foreach group satisfies when ANY of its members satisfies the edge's `when:` predicate. Rejected because:
- More invasive in the scheduler (rewriting the `.all()` predicate into per-group OR-fold logic).
- Less informative semantics - "aggregated status of the parent" is one concept; "OR-fold of N subtask outcomes evaluated against an edge's `when:`" is N+1 concepts.
- Loses the parent task as a first-class observable (run history, `otto Graph`, `--help`).

**Phase ordering:** the virtual-parent fix is structurally a prerequisite for the conditional-edge feature work being *useful* on foreach tasks, without it, anyone wiring a `when: failure` dependent onto a foreach task hits the deadlock. But its implementation depends on:

- Phase 1 (`EdgeSpec` + `When::Always` types must exist).
- Phase 2 (`TaskSpec.before` must already be `Vec<EdgeSpec>` so the parser can replace it with `When::Always` edges).
- Phase 3 (`executor::Task.task_deps` must be `Vec<TaskEdge>` so the `When::Always` edges propagate from parser to scheduler instead of being downcast to strings).
- Phase 4 (the scheduler must honor `When::Always` *and* use drain-on-failure; without drain, a subtask failure aborts the whole run before the parent task ever gets to execute and aggregate).

So the chronological order is: **Phase 1 → Phase 2 → Phase 3 → Phase 4 → Phase 0 → Phases 5-7**. The name "Phase 0" is preserved to mark its role as a prerequisite for the *conditional-edge feature being correct on foreach tasks*, not because it lands first. The earlier doc claim that Phase 0 is "independently shippable" is withdrawn - Phase 0 sits at the *end* of the mechanism stack, not the beginning. It's the cap, not the foundation. (Architect review round 4 caught an earlier ordering claim that placed Phase 0 between Phase 2 and Phase 3; that was wrong for both type-flow and scheduler-behavior reasons.)

## Proposed Solution

### Overview - worked examples

**1. Fixer (the cargo-fmt motivator), via `on-failure:` sugar:**

```yaml
tasks:
  fmt-check:
    bash: cargo fmt --all --check
    on-failure: [fmt-fix]

  fmt-fix:
    help: "Apply cargo fmt to fix format drift"
    bash: cargo fmt --all
```

After parse-time desugaring, `fmt-fix` is internally treated as:

```yaml
fmt-fix:
  after:
    - task: fmt-check
      when: failure
```

**2. Same fixer, written in the long form directly:**

```yaml
tasks:
  fmt-check:
    bash: cargo fmt --all --check

  fmt-fix:
    bash: cargo fmt --all
    after:
      - task: fmt-check
        when: failure
```

Equivalent. Choose whichever locality reads better for the project.

**3. Cleanup (the `when: always` case):**

```yaml
tasks:
  test:
    envs:
      TEST_TMPDIR: "$(mktemp -d)"
    bash: cargo test --all-features

  cleanup-tmpdir:
    bash: rm -rf "$TEST_TMPDIR"
    after:
      - task: test
        when: always
```

`cleanup-tmpdir` runs after `test` whether `test` passed or failed. If `test` is not selected for the run, `cleanup-tmpdir` is not selected either (standard closure rules).

**4. Existing ottofiles, untouched:**

```yaml
ci:
  before: [lint, check, test]   # bare strings, sugar for when: success
```

Bare strings continue to mean "depend on success." Every existing ottofile parses, runs, and serializes identically - there is no migration step.

### Data Model

#### `EdgeSpec` and `When`

New types in `src/cfg/edge.rs`:

```rust
#[derive(Clone, Debug, PartialEq, Eq, Default, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum When {
    #[default]
    Success,
    Failure,
    Always,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EdgeSpec {
    pub task: String,
    pub when: When,
    /// Tracks whether this edge was authored as a bare string (sugar form).
    /// Used by Serialize to preserve round-trip style.
    pub from_sugar: bool,
    /// Set to true when this edge was injected into `after:` by the on-failure: desugar pass.
    /// Used by `TaskSpec::serialize` to filter out injected edges so they don't appear as
    /// duplicates alongside the host's `on-failure:` field.
    pub is_injected_sugar: bool,
}

impl EdgeSpec {
    /// Construct a sugared bare-string edge with default `when: Success`.
    /// Test fixtures and scaffold generation should use this helper to ensure
    /// round-trip serialization emits the bare-string form.
    pub fn sugar(task: impl Into<String>) -> Self {
        Self {
            task: task.into(),
            when: When::Success,
            from_sugar: true,
            is_injected_sugar: false,
        }
    }
}
```

`from_sugar` is set to `true` by the deserializer when the YAML element was a bare string; it is set to `false` when the element was an object. Serializer reads it to choose the output form.

`is_injected_sugar` is set to `true` by `apply_on_failure_sugar` (Phase 5) when this edge was synthesized from a host's `on-failure:` field. `TaskSpec::serialize` filters out injected edges before emitting `after:`, so they don't appear in serialized output alongside the host's `on-failure:` (which is emitted from the host's own field).

The `EdgeSpec::sugar()` constructor exists specifically so programmatic construction (tests, scaffold generation) defaults to the sugar-shaped form. Without it, `EdgeSpec { task, when: Success, from_sugar: false, is_injected_sugar: false }` would serialize as the long object form, breaking byte-equivalence with hand-authored ottofiles.

#### `TaskSpec` (`src/cfg/task.rs:217-232`)

Two field type changes plus one new transient field:

```rust
pub struct TaskSpec {
    pub name: String,
    pub help: Option<String>,
    pub after: Vec<EdgeSpec>,           // was Vec<String>
    pub before: Vec<EdgeSpec>,          // was Vec<String>
    pub input: Vec<String>,             // unchanged (file globs)
    pub output: Vec<String>,            // unchanged (file globs)
    pub envs: HashMap<String, String>,
    pub params: ParamSpecs,
    pub action: String,
    pub foreach: Option<ForeachSpec>,
    pub virtual_parent: bool,
    pub on_failure: Vec<String>,        // <-- new: hosts what fires on this task's failure
}
```

`on_failure` is the author's-intent surface: when host task X has `on-failure: [Y]`, X's `TaskSpec.on_failure` contains `["Y"]`. This is what the deserializer reads from YAML and what the serializer emits.

`apply_on_failure_sugar` (the parse-time desugar pass in Phase 5) reads each `TaskSpec.on_failure` and pushes synthetic `EdgeSpec { task: <host>, when: Failure, from_sugar: false, is_injected_sugar: true }` entries onto the named target tasks' `after:` lists. The host's `on_failure` field is *not* cleared after desugaring, it's preserved verbatim so `TaskSpec::serialize` can re-emit it.

`TaskSpec::serialize` emits `on-failure:` from the host's own `on_failure` field, and when emitting `after:` for any task, filters out any edge where `is_injected_sugar == true`. Both sides of the round-trip read from local fields only - no cross-spec lookups required. This closes the structural flaw identified in Architect review v2.

#### `executor::Task` (`src/executor/task.rs:1-50`)

One field type change plus one new flag:

```rust
pub struct Task {
    pub name: String,
    pub parent: Option<String>,
    pub task_deps: Vec<TaskEdge>,       // was Vec<String>
    pub file_deps: Vec<String>,
    pub output_deps: Vec<String>,
    pub envs: HashMap<String, String>,
    pub values: HashMap<String, Value>,
    pub action: String,
    pub hash: String,
    pub is_virtual_parent: bool,        // <-- new: mirrors TaskSpec.virtual_parent
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TaskEdge {
    pub task: String,
    pub when: When,
}
```

`TaskEdge` is the runtime equivalent of `EdgeSpec` minus the `from_sugar`/`is_injected_sugar` cosmetic fields. The Parser strips both when building `executor::Task`.

`is_virtual_parent` is added in Phase 0 (when virtual parents become executable) so the scheduler can recognize them at runtime without consulting the parser's `TaskSpec` map. Today, the flag lives only on `TaskSpec`; once Phase 0 lands, `Task::from_task_with_cwd_and_global_envs` (`executor/task.rs:71`) copies it across. This flag drives the aggregation-override branch in the scheduler's success-path arm - see `Scheduler changes → Aggregation override` below.

#### Polymorphic YAML deserialization

The pattern is already used in otto at `src/cfg/param.rs:88-117` (`deserialize_value`): a `Visitor` with both `visit_str` and `visit_seq`/`visit_map` implementations, dispatched via `deserialize_any`. Apply the same shape to `EdgeSpec`.

Single-edge deserializer (handles one element of `before:`/`after:`):

```rust
impl<'de> Deserialize<'de> for EdgeSpec {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where D: Deserializer<'de>
    {
        struct EdgeVisitor;
        impl<'de> Visitor<'de> for EdgeVisitor {
            type Value = EdgeSpec;
            fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
                f.write_str("a task name string or a {task, when} object")
            }
            fn visit_str<E: Error>(self, value: &str) -> Result<EdgeSpec, E> {
                Ok(EdgeSpec {
                    task: value.to_string(),
                    when: When::default(),
                    from_sugar: true,
                    is_injected_sugar: false,
                })
            }
            fn visit_map<M: MapAccess<'de>>(self, mut map: M) -> Result<EdgeSpec, M::Error> {
                let mut task: Option<String> = None;
                let mut when: When = When::default();
                while let Some(k) = map.next_key::<String>()? {
                    match k.as_str() {
                        "task" => task = Some(map.next_value()?),
                        "when" => when = map.next_value()?,
                        other => return Err(Error::unknown_field(other, &["task", "when"])),
                    }
                }
                let task = task.ok_or_else(|| Error::missing_field("task"))?;
                Ok(EdgeSpec { task, when, from_sugar: false, is_injected_sugar: false })
            }
        }
        deserializer.deserialize_any(EdgeVisitor)
    }
}

impl Serialize for EdgeSpec {
    fn serialize<S: Serializer>(&self, ser: S) -> Result<S::Ok, S::Error> {
        // Note: filtering of is_injected_sugar happens at TaskSpec::serialize
        // (around the seq element walk), not here - EdgeSpec serializes itself
        // unconditionally if asked.
        if self.from_sugar && self.when == When::Success {
            ser.serialize_str(&self.task)
        } else {
            let mut map = ser.serialize_map(Some(2))?;
            map.serialize_entry("task", &self.task)?;
            map.serialize_entry("when", &self.when)?;
            map.end()
        }
    }
}
```

The filter for `is_injected_sugar` lives in `TaskSpec::serialize` (Phase 5), not in `EdgeSpec::serialize`. The serializer code is approximately:

```rust
// Inside TaskSpec::serialize, when emitting the `after:` field:
let visible_after: Vec<&EdgeSpec> = self.after.iter()
    .filter(|e| !e.is_injected_sugar)
    .collect();
if !visible_after.is_empty() {
    map.serialize_entry("after", &visible_after)?;
}

// And on:
if !self.on_failure.is_empty() {
    map.serialize_entry("on-failure", &self.on_failure)?;
}
```

`Vec<EdgeSpec>` (after filtering) derives its de/serialize from the element impls, so mixed lists like `[a, {task: b, when: failure}, c]` round-trip per-element.

**Round-trip guarantee:**
- Bare-string element with default `when: success` → serialized as bare string.
- Object element OR any element with `when != success` → serialized as object.
- An ottofile that uses only bare strings (every existing ottofile today) serializes back byte-equivalent (modulo serde's normal whitespace).

### Parser changes

#### Sugar desugaring (`on-failure:` → `after:`)

In `Parser::process_tasks_with_filter` (`parser.rs:691`), after the raw `ConfigSpec` is loaded but before `compute_task_deps_from_specs` is called, run a desugar pass that reads each host's `on_failure` field and injects synthetic `after:` edges on the named targets:

```rust
fn apply_on_failure_sugar(specs: &mut HashMap<String, TaskSpec>) -> Result<()> {
    // Walk every host's on_failure list and collect (host, target) pairs.
    // We collect first, then mutate, to avoid simultaneous &mut on the map.
    let mut pairs: Vec<(String, String)> = specs.iter()
        .flat_map(|(host_name, spec)|
            spec.on_failure.iter().map(move |target| (host_name.clone(), target.clone())))
        .collect();

    // HashMap iteration order is non-deterministic; sort so the order edges get
    // appended to target.after is stable across runs. Without this, byte-equivalent
    // round-trip on ottofiles with multiple on-failure: relationships would
    // sporadically reorder the synthetic edges between reads.
    pairs.sort();

    for (host, target) in pairs {
        // Self-loops are invalid: a task can't fire on its own failure.
        if host == target {
            return Err(eyre!(
                "on-failure on task '{}' references itself; a task cannot depend on its own failure",
                host));
        }
        let target_spec = specs.get_mut(&target)
            .ok_or_else(|| eyre!(
                "on-failure on task '{}' references unknown task '{}'", host, target))?;
        target_spec.after.push(EdgeSpec {
            task: host.clone(),
            when: When::Failure,
            from_sugar: false,           // synthetic edge, not user-authored sugar
            is_injected_sugar: true,     // marks edge for filtered output on serialize
        });
    }
    Ok(())
}
```

After this pass, the dependency-computation and scheduling layers operate on the unified `before:`/`after:` model - they read `task_spec.after` and don't care whether an edge came from user-written YAML or from sugar injection. The host's `on_failure` field is preserved verbatim (we did not clear it); `TaskSpec::serialize` reads from it directly when emitting `on-failure:`.

Serialization round-trip:
- Host's `on-failure:` field emits from `TaskSpec.on_failure` (the verbatim author field).
- Target's `after:` field iterates `TaskSpec.after` but filters out any edge where `is_injected_sugar == true`, those are duplicates of what the host's `on-failure:` already conveys.
- The result: an ottofile authored with `on-failure: [Y]` on X serializes back with `on-failure: [Y]` on X and no synthetic `after:` entry on Y. An ottofile authored with explicit `after: [{task: X, when: failure}]` on Y has `is_injected_sugar = false`, so it serializes back in its explicit form.

#### Closure traversal - unchanged topology, condition propagation

`collect_transitive_deps` (`parser.rs:1166`) is unchanged in *topology* - it still walks `before:` and `after:` by task name, recursively pulling referenced tasks into `tasks_needed`. Edge conditions don't affect what's reachable, only when it runs.

`compute_task_deps_from_specs` (`parser.rs:1053`) gains condition propagation:

```rust
fn compute_task_deps_from_specs(
    task_specs: &HashMap<String, TaskSpec>
) -> Result<HashMap<String, Vec<TaskEdge>>> {     // was Vec<String>
    let mut task_deps: HashMap<String, Vec<TaskEdge>> = HashMap::new();

    for (task_name, task_spec) in task_specs {
        let edges: Vec<TaskEdge> = task_spec.before.iter()
            .map(|e| TaskEdge { task: e.task.clone(), when: e.when.clone() })
            .collect();
        task_deps.insert(task_name.clone(), edges);
    }

    // `after` on task X means: every task in X's `after` list depends on X.
    // The condition lives on the inverted edge: X.after = [{task: Y, when: W}]
    // means Y now has a dependency {task: X, when: W} added to Y's task_deps.
    //
    // Dedup is by (task, when) tuple, NOT by task alone. Two edges from the same
    // source with different `when:` conditions are legal and semantically distinct
    // (e.g., Y might want to react to X on both success AND failure with separate
    // bookkeeping). Identical (task, when) pairs collapse to one edge.
    for (task_name, task_spec) in task_specs {
        for after_edge in &task_spec.after {
            let deps = task_deps.entry(after_edge.task.clone()).or_default();
            let new_edge = TaskEdge {
                task: task_name.clone(),
                when: after_edge.when.clone(),
            };
            if !deps.iter().any(|d| d.task == new_edge.task && d.when == new_edge.when) {
                deps.push(new_edge);
            }
        }
    }

    Ok(task_deps)
}
```

**Same-source-multiple-conditions:** if a user writes both `{task: X, when: success}` and `{task: X, when: failure}` on the same dependent, both edges survive dedup. The scheduler's `classify_edge` evaluates each independently - and because exactly one of (success, failure) will be true at runtime, exactly one of the two edges will end up `Satisfied` while the other is `Unreachable`. That makes the dependent perpetually `Skipped`. **The correct way to express "depend on X completing either way" is `when: always`, not paired success+failure edges.** A config-load validation pass (Phase 4) detects the pair and rejects it with a hint to use `when: always`.

Validation step (also in this function or immediately after) checks every edge's `task` field resolves to a real task, AND that no `(task, *)` pair has both `when: success` and `when: failure` on the same dependent. Unchanged from today in spirit, just walks `Vec<EdgeSpec>` instead of `Vec<String>` and adds the new pair check.

#### `executor::Task` construction

`Task::from_task_with_cwd_and_global_envs` (`executor/task.rs:71`) copies `task_deps` from the parser's computed map directly. The `task_spec.before.clone()` line at `executor/task.rs:77` becomes:

```rust
let task_deps: Vec<TaskEdge> = task_spec.before.iter()
    .map(|e| TaskEdge { task: e.task.clone(), when: e.when.clone() })
    .collect();
```

(The Parser then overrides `task.task_deps` with the fully-computed map from `compute_task_deps_from_specs`, same as today at `parser.rs:784`.)

### Scheduler changes (`src/executor/scheduler.rs`)

#### Edge-condition check at dep-satisfaction time

The current check at `scheduler.rs:329` and `scheduler.rs:396-397` is:

```rust
let deps_completed = task.task_deps.iter().all(|dep| completed_set.contains(dep));
```

Replace with a per-edge predicate:

```rust
let deps_satisfied = task.task_deps.iter().all(|edge| match edge.when {
    When::Success => completed_set.contains(&edge.task),
    When::Failure => failed_set.contains(&edge.task),
    When::Always  => completed_set.contains(&edge.task) || failed_set.contains(&edge.task),
});
```

`failed_set: HashSet<String>` is a new sibling to the existing `completed_set`, populated on the failure path.

#### Unreachability marking

A task becomes unreachable when any of its edges can never satisfy:

- `when: Success` edge whose source is in `failed_set` → unreachable (source can never succeed; it already failed).
- `when: Failure` edge whose source is in `completed_set` → unreachable (source can never fail; it already succeeded).
- `when: Always` edges never make a dependent unreachable.

After each task completion or failure, sweep `blocked_tasks` for newly-unreachable tasks and mark them `Skipped`:

```rust
fn classify_edge(edge: &TaskEdge, completed: &HashSet<String>, failed: &HashSet<String>)
    -> EdgeState
{
    match edge.when {
        When::Success => {
            if completed.contains(&edge.task) { EdgeState::Satisfied }
            else if failed.contains(&edge.task) { EdgeState::Unreachable }
            else { EdgeState::Pending }
        }
        When::Failure => {
            if failed.contains(&edge.task) { EdgeState::Satisfied }
            else if completed.contains(&edge.task) { EdgeState::Unreachable }
            else { EdgeState::Pending }
        }
        When::Always => {
            if completed.contains(&edge.task) || failed.contains(&edge.task) { EdgeState::Satisfied }
            else { EdgeState::Pending }
        }
    }
}
```

A task is `Skipped` when *any* of its edges is `Unreachable`. A task is `Ready` when *all* of its edges are `Satisfied`. Otherwise it remains `Blocked`.

This is a real new behavior: today, a failed dep leaves dependents in mysteriously-blocked limbo and the run exits because the scheduler returns early. With unreachability marking, the dependent is explicitly marked `Skipped`, broadcast to the TUI, and recorded in run history - the user sees *why* it didn't run.

**Skip-reason wiring.** The skip reason ("`dep X failed; this task required when: success`" or `"dep X succeeded; this task required when: failure`") is carried via a sidecar map on the scheduler: `skip_reasons: HashMap<String, String>`. Populated when a task is marked Skipped; consumed by the TUI broadcast and the run-history record. The `TaskStatus::Skipped` enum variant stays simple (no struct variant) - keeps the existing enum's pattern-matching call sites unchanged. Tasks that complete normally have no entry in the sidecar.

#### Drain-on-failure

Replace the immediate `return Err(e)` at `scheduler.rs:446` with a record-and-continue pattern:

```rust
Some(Err(e)) => {
    let task_name = /* extract from error, existing logic at scheduler.rs:418-444 */;

    // Record failure
    failed_set.insert(task_name.to_string());
    {
        let mut statuses = self.task_statuses.lock().await;
        statuses.insert(task_name.to_string(), TaskStatus::Failed);
    }
    active_tasks.remove(task_name);
    completed_tasks += 1;  // slot is consumed; do NOT add to completed_set

    // Sweep blocked_tasks: any task whose edges are now Satisfied gets queued;
    // any task whose edges are now Unreachable gets Skipped.
    let mut newly_ready = Vec::new();
    let mut newly_skipped = Vec::new();
    blocked_tasks.retain(|task| {
        let states: Vec<EdgeState> = task.task_deps.iter()
            .map(|e| classify_edge(e, &completed_set, &failed_set))
            .collect();
        if states.iter().any(|s| matches!(s, EdgeState::Unreachable)) {
            newly_skipped.push(task.clone());
            false  // remove from blocked
        } else if states.iter().all(|s| matches!(s, EdgeState::Satisfied)) {
            newly_ready.push(task.clone());
            false  // remove from blocked
        } else {
            true   // still blocked
        }
    });
    for t in newly_ready { ready_queue.push_back(t); }
    for t in newly_skipped {
        let mut statuses = self.task_statuses.lock().await;
        statuses.insert(t.name.clone(), TaskStatus::Skipped);
        // broadcast skip with reason "dep <X> failed; this task required when: success"
        completed_tasks += 1;
    }

    // Save the error; do not return yet
    if final_error.is_none() {
        final_error = Some(e);
    }
    // fall through to next loop iteration
}
```

The same sweep also needs to run on the success path (the `Some(Ok(completed_task))` arm) to catch `when: failure` edges whose source just *succeeded* and which are therefore unreachable. Today's success path already walks `blocked_tasks` to find newly-ready items; extend that walk to also detect newly-unreachable ones.

Termination: when `completed_tasks >= total_tasks` (every task is in `completed_set`, `failed_set`, or `Skipped`), the loop exits. After the loop:

```rust
if let Some(err) = final_error {
    return Err(err);
}
Ok(())
```

#### `total_tasks` accounting

`total_tasks` is `self.tasks.len()` (unchanged from today). Failed tasks count toward `completed_tasks`; skipped tasks count toward `completed_tasks`. Every task ends in exactly one terminal state: `Completed`, `Failed`, or `Skipped`. Loop terminates deterministically.

#### Aggregation override for virtual parents (Phase 0 - added to Phase 4's success-path arm)

When Phase 0 lands, virtual parents become real tasks that the scheduler executes. Their `task_deps` is `[{task: subtask_i, when: Always} for i in 1..N]`, so the scheduler queues them only after every subtask reaches a terminal state. The parent's empty action succeeds nominally, and the scheduler's `Some(Ok(completed_task))` arm receives it like any other Ok completion.

The aggregation override happens *inside* that arm, **before** the task is inserted into `completed_set` and **before** `blocked_tasks` is swept for newly-ready/newly-unreachable items. Order is critical: if the parent enters `completed_set` first, the sweep will see its successful entry and queue downstream `when: success` dependents - bypassing the failure aggregation.

```rust
// Inside Some(Ok(completed_task)) arm, before completed_set.insert(...).
let mut final_status = TaskStatus::Completed;

if completed_task.is_virtual_parent {
    // Find this parent's subtasks via the existing `parent: Option<String>` field
    // on executor::Task. No new index structure needed; lookup is O(N) over the
    // task vec, which is small and runs once per parent completion.
    let subtask_statuses: Vec<&TaskStatus> = self.tasks.iter()
        .filter(|t| t.parent.as_deref() == Some(&completed_task.name))
        .filter_map(|t| /* read t.name's status from task_statuses */)
        .collect();

    final_status = if subtask_statuses.iter().any(|s| matches!(s, TaskStatus::Failed)) {
        TaskStatus::Failed
    } else if subtask_statuses.iter().any(|s| matches!(s, TaskStatus::Skipped)) {
        TaskStatus::Skipped
    } else {
        TaskStatus::Completed
    };
}

// Now route the (possibly overridden) status into the right set.
match final_status {
    TaskStatus::Completed => { completed_set.insert(completed_task.name.clone()); }
    TaskStatus::Failed    => { failed_set.insert(completed_task.name.clone()); }
    TaskStatus::Skipped   => { /* update task_statuses; do not enter either set */ }
    _ => unreachable!(),
}
self.task_statuses.lock().await.insert(completed_task.name.clone(), final_status);
// THEN sweep blocked_tasks for newly-ready / newly-unreachable.
```

**`final_error` preservation:** when a virtual parent aggregates to `Failed`, it MUST NOT touch `final_error`. The subtask whose actual failure caused the aggregation already set `final_error` to its own error. The parent's failure is a *derived* status - it has no original error to report, and overwriting `final_error` would replace the root-cause subtask error with a generic "virtual parent aggregated to failed" message, masking the diagnostic. The `if final_error.is_none()` guard in the existing `Some(Err(e))` arm handles this naturally because the parent never enters that arm - it enters the Ok arm and gets its status overridden.

This is the only new scheduler logic Phase 0 introduces; everything else flows through the conditional-edge machinery Phases 1-4 built.

### Visualizer (`src/executor/graph.rs`)

`graph.rs` is not on the scheduling path; this change is cosmetic.

When building the visual DAG, walk each task's `before:`/`after:` edges and use `edge.when` to pick the line style:

- `When::Success` → solid black (or current default).
- `When::Failure` → dashed red.
- `When::Always` → solid green.

Plus an edge label showing the condition for non-default edges. No other behavior change.

### `on-failure:` sugar (recap)

The YAML field `on-failure: [Y]` on host X is stored on `X.on_failure: Vec<String>`. During `Parser::process_tasks_with_filter`, the desugar pass `apply_on_failure_sugar` (Phase 5) walks every spec's `on_failure` list and pushes a synthetic `EdgeSpec { task: X, when: Failure, from_sugar: false, is_injected_sugar: true }` onto each named target's `after:`. The host's `on_failure` field is preserved verbatim.

From the dependency-computation and scheduler perspective, failure-conditional tasks are reached through `after:` edges like any other dependency, they don't know `on-failure:` exists.

For serialization, `TaskSpec::serialize` reads from local fields only - no cross-spec lookups, no parent `ConfigSpec` access:
- `on-failure:` emits from `self.on_failure`.
- `after:` iterates `self.after` and filters out elements where `is_injected_sugar == true`.

This means:
- Hosts can name their fixers locally (`on-failure: [fmt-fix]` on `fmt-check`) - the ergonomic surface from v2 is preserved.
- Or users can write `after: [{task: fmt-check, when: failure}]` on `fmt-fix` directly if they prefer the inverted locality.
- Both styles round-trip in their original form: the first because `is_injected_sugar` keeps the synthetic edge out of serialized output; the second because the user-written edge has `is_injected_sugar = false` and serializes through.

### Implementation Plan

#### Phase 0: Make virtual parents executable aggregator tasks (prerequisite for foreach, runs after Phase 4)
**Model:** opus

This phase depends on Phases 1, 2, 3, and 4. The chronological order is: Phase 1 → Phase 2 → Phase 3 → Phase 4 → **Phase 0** → Phases 5-7. The name "Phase 0" is preserved to mark its role as a prerequisite for the conditional-edge feature being *correct* on foreach tasks, not because it lands first. It sits at the end of the mechanism stack - by the time it runs, `When::Always` edges propagate end-to-end and the scheduler honors them with drain-on-failure semantics.

- Remove the `if task_spec.virtual_parent { continue; }` early-skip at `parser.rs:733-735`. Virtual parents now become real `executor::Task` entries with an empty action.
- Add `is_virtual_parent: bool` field to `executor::Task`. Populate it in `Task::from_task_with_cwd_and_global_envs` (`executor/task.rs:71`) from `task_spec.virtual_parent`. The scheduler reads this at task-completion time to know whether to run the aggregation override.
- `task::Task::from_task_with_cwd_and_global_envs` handles the empty-action case: the task immediately succeeds with no subprocess if it has no `bash:`/`python:`/`action:`. Existing virtual parents already have `action = String::new()` (see `cfg/task.rs:489-503` `as_virtual_parent`). Add a sanity test verifying empty-action execution succeeds.
- DAG rewrite in `expand_foreach_tasks_with_serial` (`parser.rs:1113-1160`):
  - Subtasks inherit parent's `before:` edges (existing behavior at `parser.rs:1141` - unchanged).
  - Subtasks do NOT inherit parent's `after:` edges (existing behavior - unchanged).
  - **New:** the parent's `before:` is replaced with `[{task: subtask_i, when: Always} for each subtask_i]`. The parent now depends on every subtask via `when: Always` edges, so the scheduler's existing dep-gating queues the parent once all subtasks reach a terminal state.
- Add the aggregation-override block in the scheduler's `Some(Ok(completed_task))` arm. Implementation per the "Aggregation override for virtual parents" section above. Subtask discovery uses the existing `parent: Option<String>` field on `executor::Task` - no new index structure required. The override must run BEFORE `completed_set.insert(...)` and BEFORE the `blocked_tasks` sweep. When the override resolves to `Failed`, it must NOT modify `final_error` (the originally-failed subtask already set it; preserving that gives users the root-cause error rather than a derived "parent aggregated to failed" message).
- Integration test: `ci: { before: [install] }` where `install` is foreach with subtasks `install:td`, `install:ts`. Verify ci waits for all subtasks AND the parent (which gates on the subtasks via `when: Always`), then runs.
- Integration test: one subtask fails → parent aggregates as Failed → ci's `before: [install]` (which is `when: success` by default) is unreachable → ci is Skipped.
- Integration test: serial foreach (parallel: false), S1 fails → S2/S3 marked Skipped → parent aggregates as Failed → downstream `when: failure` dependent fires.
- Integration test: prerequisite of foreach group fails → all subtasks marked Skipped → parent aggregates as Skipped → downstream `when: failure` dependent does NOT fire (Skipped is not Failure).
- Integration test: `final_error` preservation - subtask exits with a specific error message; parent aggregates as Failed; run exits non-zero; the surfaced error is the subtask's original message, not a generic parent-aggregation message.
- **Deliverable:** the latent deadlock at `parser.rs:784` is closed. Foreach-by-parent-name dependencies work. The aggregation semantic is implemented via the same `When::Always` mechanism used elsewhere in v3.

#### Phase 1: `EdgeSpec` + `When` + polymorphic de/serialize
**Model:** opus

- Add `src/cfg/edge.rs` with `EdgeSpec`, `When`, `Deserialize`, `Serialize` impls following the pattern at `src/cfg/param.rs:88-117`.
- Add `TaskEdge` (runtime equivalent) in `src/executor/task.rs`.
- Unit tests in `src/cfg/edge/tests.rs`: bare string parses to `when: Success` with `from_sugar = true`; object form parses to specified `when` with `from_sugar = false`; serializer emits bare string for sugared + success, object form otherwise; round-trip on mixed lists.
- **Deliverable:** types exist, de/serialize correctly, no callers updated yet.

#### Phase 2: Migrate `TaskSpec.before`/`after` from `Vec<String>` to `Vec<EdgeSpec>`
**Model:** opus

- Change field types in `src/cfg/task.rs:217-232`.
- Update `TaskSpecHelper` and `Deserialize`/`Serialize` impls.
- Compile fix every consumer of `task_spec.before` / `task_spec.after` - this is mechanical and will touch parser, executor, graph, and tests. Update each call site to read `.task` from the edge.
- Update existing tests that construct `TaskSpec` programmatically: every `before: vec!["foo".to_string()]` becomes `before: vec![EdgeSpec::sugar("foo")]` (add a helper constructor for ergonomics).
- **Deliverable:** entire codebase compiles; every existing ottofile parses, serializes, and runs identically. No new behavior yet.

#### Phase 3: Migrate `executor::Task.task_deps` from `Vec<String>` to `Vec<TaskEdge>`
**Model:** opus

- Change field type in `src/executor/task.rs`.
- Update `compute_task_deps_from_specs` (`parser.rs:1053`) to compute `Vec<TaskEdge>` and propagate edge conditions.
- Update `executor::Task::from_task_with_cwd_and_global_envs` (`executor/task.rs:71`) to build `Vec<TaskEdge>`.
- Compile fix every consumer of `task.task_deps`.
- **Deliverable:** runtime type carries edge conditions end-to-end; scheduler still ignores them (treats all as `When::Success`); behavior unchanged.

#### Phase 4: Scheduler edge-condition check + unreachability + drain
**Model:** opus

- Add `failed_set: HashSet<String>` and `final_error: Option<Report>` to the scheduler's `execute_all` state.
- Replace `deps_completed` checks at `scheduler.rs:329` and `scheduler.rs:396-405` with `classify_edge`-based logic.
- Rewrite the `Some(Err(e))` arm at `scheduler.rs:415-446` per the drain pattern.
- Add post-loop reconciliation: any task still in `blocked_tasks` after main loop exits is marked `Skipped`.
- Extend success-path sweep to detect newly-unreachable tasks (e.g., `when: failure` deps whose source just succeeded).
- Integration tests in `tests/`:
  - `when: success` failure: dep fails → dependent marked `Skipped`, run exits non-zero.
  - `when: failure` success: dep succeeds → dependent marked `Skipped`, run exits zero.
  - `when: failure` failure: dep fails → dependent runs, run exits non-zero.
  - `when: always`: runs regardless of dep outcome.
  - Two parallel branches, one fails (releases its `when: failure` dependent), other completes; verify other branch's downstream completes and the failure-dependent runs.
- **Deliverable:** conditional edges actually work; current ottofiles (all `when: success` implicit) behave identically.

#### Phase 5: `on-failure:` sugar layer
**Model:** sonnet

- Add `on_failure: Vec<String>` field to `TaskSpec` (`src/cfg/task.rs`). Deserialize from YAML `on-failure:` (kebab-case rename). Empty by default. Stays on the spec - not cleared after desugaring.
- Add `is_injected_sugar: bool` field to `EdgeSpec` (default `false`).
- Add `apply_on_failure_sugar` desugar pass in `Parser::process_tasks_with_filter` after config load, before `compute_task_deps_from_specs`. Walks every spec's `on_failure`, pushes synthetic `EdgeSpec { task: host, when: Failure, from_sugar: false, is_injected_sugar: true }` onto the named target's `after:`.
- Sugar serialization: `TaskSpec::serialize` emits `on-failure:` from `self.on_failure` when non-empty. When emitting `self.after`, filter out elements where `is_injected_sugar == true`.
- Tests: ottofile written with `on-failure:` desugars correctly; round-trip preserves the `on-failure:` form on the host AND does not emit a duplicate `after:` entry on the target; ottofile written with explicit `after: [{task: X, when: failure}]` is *not* converted into `on-failure:` (only sugar-tagged edges with `is_injected_sugar = true` are filtered).
- **Deliverable:** v2 ergonomic surface preserved on top of v3 machinery, with serializer using local-field-only access (no parent ConfigSpec lookups, which is structurally impossible in serde).

#### Phase 6: Visualizer + TUI
**Model:** sonnet

- Update `src/executor/graph.rs` rendering to pick line styles by `edge.when` and label non-default edges.
- TUI: add a `TaskStatus::Skipped` rendering distinct from `Completed`/`Failed`, with the skip reason if available.
- **Deliverable:** `otto Graph` shows conditional edges visually; TUI distinguishes skipped tasks.

#### Phase 7: Documentation, templates, and internal tools
**Model:** sonnet

- Update `README.md` task model section: `before:`/`after:` polymorphic forms, the `When` values, and the `on-failure:` sugar.
- Update otto skill (`~/.claude/skills/otto/SKILL.md`) with examples for all three `When` values and the sugar form.
- Update rust scaffold template (`~/.claude/skills/ottofile/references/rust.md`) to emit the `fmt-check` + `fmt-fix` + `on-failure:` pattern by default.
- Update `otto Convert` (Makefile → ottofile) to emit bare-string edges via `EdgeSpec::sugar()`.
- Update `otto Graph` visualizer to render virtual parents as distinct-looking aggregator nodes (now that they're real tasks after Phase 0).
- Add `examples/conditional-deps/` with worked examples for `when: failure` (fixer), `when: always` (cleanup), and the sugar form.
- CHANGELOG entry calling out: (a) new schema; (b) `on-failure:` sugar; (c) scheduler drain-on-failure behavior change; (d) `--skip foo` now cascades Skipped through dependents regardless of edge condition.

## Alternatives Considered

### Alternative 1: `on-failure:` only (the narrow version of this design)

- **Description:** v2 of this doc. Add `on-failure: Vec<String>` to `TaskSpec`. Park hook tasks at scheduler init. Release on host failure. No general `when:` mechanism.
- **Pros:** Smaller schema change (additive field, no polymorphic types). Less test fixture churn.
- **Cons:** Solves only the `when: failure` case, not `when: always`. Requires introducing a parallel "hook task" category (`is_failure_hook` flag) and parked-hook scheduler state that does not generalize. Future `when: always` support would require a second, overlapping change to the same scheduler code paths.
- **Why not chosen:** The required scheduler change (drain-on-failure, unreachability marking) is the same in both designs. The schema change for conditional edges is mechanical (the polymorphic-string-or-object pattern is already used at `src/cfg/param.rs:88-117`). The marginal complexity to ship the general version is small, and it absorbs both the fixer pattern and the cleanup pattern in one shot.

### Alternative 2: Pure bash trap inside the host task

- **Description:** Document the `trap` pattern (gated on `[ -z "$CI" ]`) and don't touch otto.
- **Pros:** Zero code change. Works today.
- **Cons:** Fixup and cleanup logic stay buried inside task bodies; not invocable independently; invisible to `otto Graph` and history; every project re-implements the same pattern.
- **Why not chosen:** Doesn't solve the problem; just papers over it.

### Alternative 3: A `--fix` flag mode on tasks

- **Description:** Add `fix:` as a sibling of `bash:` on a task; `otto check --fix` runs the fixer on failure.
- **Pros:** Mirrors clippy/ruff/eslint conventions.
- **Cons:** Couples the fixer to the host task body; can't be invoked independently; doesn't generalize to cleanup or other conditional patterns.
- **Why not chosen:** Explicitly removed from consideration by the author.

### Alternative 4: Dynamic task injection at scheduler runtime

- **Description:** Look up hooks/cleanup tasks at failure time, instantiate fresh `executor::Task`s, inject into `ready_queue`.
- **Pros:** Hooks not in the static graph wouldn't waste resolution.
- **Cons:** Scheduler has no access to `TaskSpecs`, params evaluation, env evaluation, foreach expansion. Major architectural change. Hidden dependencies make `otto Graph` and `--help` unreliable.
- **Why not chosen:** Static parse-time resolution is simpler and uses the existing pipeline. Resolved hooks that don't run cost nothing.

## Technical Considerations

### Dependencies

No new crates.

### Performance

- Parse time: no new graph traversals. `compute_task_deps_from_specs` does the same number of edge walks as today, just carrying `When` alongside `task: String`.
- Scheduler init: unchanged.
- Per-completion sweep: `O(B)` where B is the size of `blocked_tasks`. Same as today's sweep, with one extra `match` per edge.
- Memory: each `TaskEdge` is `String + enum`, vs. today's `String`. One-byte overhead per edge (the `When` discriminant). Negligible.

### Behavior change: scheduler no longer fails fast

This is a real behavior change: today, the first failure causes `return Err(e)` and the run ends abruptly. With this design, the first failure records into `failed_set`, sweeps `blocked_tasks` for newly-ready/newly-unreachable tasks, and the loop continues until all in-flight tasks plus any conditional follow-ups drain.

**Why this is acceptable:**
- The user-visible exit code is unchanged - the run still exits non-zero.
- Today, in-flight `tokio::spawn`'d tasks continue running after the scheduler returns `Err(e)` (tokio detaches dropped `JoinHandle`s); their work happens (files written, processes spawned), but their stdout/stderr aren't captured and their exit codes aren't recorded. The new behavior captures and records what they do. Strictly better observability for the same wall-clock cost.
- The drain is bounded: no new `when: success` work starts after a failure (those tasks are marked unreachable). Only already-in-flight tasks plus newly-satisfied `when: failure`/`when: always` tasks run.
- This applies whether or not the ottofile uses any conditional edges. Even all-`when: success` ottofiles get the "let in-flight finish before returning" benefit.

**Mitigation if unwanted:** Gate behind a config flag (`otto.fail-fast: true`) or CLI flag (`--fail-fast`). Default drain. Easy back-out.

### Round-trip stability

A primary risk for any schema-polymorphic change is that ottofiles change shape on every read/write cycle. Mitigations:

- `from_sugar` flag on `EdgeSpec` records the author's choice for bare-string vs object form.
- `When` defaults to `Success`; combined with `from_sugar = true`, the Serialize impl emits the bare string form.
- `is_injected_sugar` flag on `EdgeSpec` marks edges synthesized by `apply_on_failure_sugar`; the serializer filters these out of `after:` so they don't appear as duplicates alongside the host's `on-failure:`.
- `on_failure: Vec<String>` field on `TaskSpec` preserves the host-named surface verbatim across round-trips - the serializer reads from this field directly, no parent-ConfigSpec lookups required.
- `EdgeSpec::sugar(name)` constructor for programmatic construction (tests, scaffold generation) defaults to bare-string-shape semantics, so test fixtures don't accidentally serialize to long-form objects.
- Tests in Phase 1 explicitly verify byte-for-byte round-trip on every existing ottofile in the repo (`examples/*/.otto.yml`, the repo's own `.otto.yml`, scaffolded templates).

### `when: always` footgun

`when: always` cleanup tasks can themselves fail. If a cleanup task calls into the same infrastructure that just failed (e.g., a network-dependent log-shipper after a network-failure test), the cleanup task will fail too. This is identical to the problem with bash `trap` today (a `trap`ed handler that calls a broken function), so it's not a new failure mode - but it's newly visible because the cleanup is now a first-class task with its own row in history.

Documented in CHANGELOG and the canonical `examples/conditional-deps/` example: prefer `when: always` cleanup tasks that depend on no external state. If cleanup needs external resources, gate the cleanup body with `set +e` and a final `exit 0` so cleanup failure doesn't pollute the run's exit code beyond the original failure.

### Security

No new attack surface. Conditional edges don't grant tasks any new privileges; they only control when tasks run.

### Testing Strategy

**Unit tests (`src/cfg/edge/tests.rs`):**
- Polymorphic deserialize: bare string → `from_sugar=true, when=Success`; object → `from_sugar=false`.
- Polymorphic serialize: sugared+success → bare string; otherwise → object.
- Round-trip on mixed lists.

**Unit tests (`src/cfg/task/tests.rs`):**
- `before:`/`after:` accept mixed list (some bare strings, some objects).
- Round-trip preserves per-element form.
- `on-failure:` field deserializes onto `TaskSpec.on_failure` verbatim.
- After `apply_on_failure_sugar`, host's `on_failure` is preserved; target's `after` has the injected edge with `is_injected_sugar = true`.
- Serializer emits `on-failure:` from host's field and filters injected edges from target's `after:`.

**Unit tests (`src/cli/parser/tests.rs`):**
- `apply_on_failure_sugar` translates correctly.
- `apply_on_failure_sugar` rejects self-loops (`on-failure: [<self>]`).
- `apply_on_failure_sugar` produces deterministic edge order across runs (sort verified).
- `compute_task_deps_from_specs` propagates `When` through `before:`/`after:` inversion.
- `compute_task_deps_from_specs` dedup respects (task, when) tuples, not task alone.
- Validation: unknown task in any edge fails; cycle detection still works; same-source paired `when: success`/`when: failure` edges on a dependent are rejected with a hint to use `when: always`.

**Integration tests (`tests/`):**
- The four scheduler scenarios listed in Phase 4.
- `on-failure:` sugar end-to-end (using the cargo-fmt motivating case).
- `when: always` cleanup runs whether host succeeds or fails.
- Mixed scenarios: branch A succeeds, branch B fails, B's `when: failure` dependent runs, A's downstream completes, run exits non-zero.

**Round-trip suite:**
- Every ottofile in `examples/*/.otto.yml` and the repo's own `.otto.yml` round-trips byte-equivalently (modulo serde's normal whitespace handling).

### Rollout Plan

Single release. Schema is forward-compatible: every existing ottofile parses, serializes, and behaves identically. Behavior change (drain-on-failure) is documented in CHANGELOG. No migration tooling needed.

Two tools internal to otto need targeted updates as part of Phase 7:

- **`otto Convert`** (Makefile → ottofile generator). Must emit `before:`/`after:` as bare strings (sugar form), not as `{task, when: success}` objects, so generated ottofiles read naturally. Use `EdgeSpec::sugar()` when constructing edges.
- **`otto Graph`** rendering. Virtual parents previously didn't appear as standalone nodes (they were dependency-only constructs). After Phase 0 they're real tasks with edges to their subtasks - the rendered graph gains explicit parent nodes for every foreach task. Visualizer should mark them distinctly (e.g., a different border style) so users see they're aggregators rather than ordinary tasks.

Run-history storage on disk does not change schema: `task_deps` was already serialized as task-name strings on the database side (via the existing executor → store path); the runtime `Vec<TaskEdge>` change is in-memory only. Existing run history loads unchanged.

## Risks and Mitigations

| Risk | Likelihood | Impact | Mitigation |
|------|------------|--------|------------|
| `Vec<EdgeSpec>` migration ripples through more code than expected | Medium | Medium | Phases 2 and 3 are explicitly mechanical migration phases. Compile errors at every old call site will surface them; the build won't pass until all are converted. |
| Round-trip changes shape of existing ottofiles | Low | Medium | `from_sugar` flag + explicit byte-equivalence test in Phase 1 against every committed ottofile. |
| `when: failure` dependents hide host failures from CI | Medium | High | Host task failure is always recorded in `failed_set` and propagated to exit code via `final_error`. Conditional dependents running successfully does *not* mask host failure. Documented prominently. |
| `when: always` cleanup tasks themselves fail and cascade | Medium | Low | Identical to today's bash `trap` problem. Documented; canonical example shows `set +e` + `exit 0` pattern in cleanup bodies. |
| Drain-on-failure surprises users expecting fail-fast | Low | Medium | Documented in CHANGELOG. Add `otto.fail-fast: true` config field as a back-out if anyone hits this. Default drain produces more useful CI output. |
| Sugar emission for `on-failure:` round-trip is fiddly | Low | Low | `is_injected_sugar` flag on `EdgeSpec` lets the serializer filter injected edges using local-field-only access. No cross-spec lookups needed. Tests verify byte-equivalence on hand-authored sugar and explicit forms. |
| Conditional edge interpretation diverges between parser closure and scheduler runtime | Low | High | Single source of truth: `compute_task_deps_from_specs` carries `When` through; scheduler's `classify_edge` is the only place conditions are interpreted at runtime. Cross-checked in integration tests. |
| Virtual-parent → subtask aggregation has surprising failure semantics | Medium | Medium | Phase 0 explicitly defines the aggregation rule (any failed → parent failed; any skipped, none failed → parent skipped; all succeeded → parent succeeded) and tests it. The rule is intuitive enough that documentation in the canonical example should be sufficient. |

## Open Questions

- [x] **Resolved.** `--skip foo` marks `foo` as `Skipped`, not `Completed`. Skipped is its own terminal state - neither success nor failure. Dependents of `foo` via `when: success` become `Unreachable` (foo never succeeded) → Skipped themselves. Dependents via `when: failure` also become `Unreachable` (foo never failed) → Skipped. Dependents via `when: always` are pending: they evaluate as `Unreachable` because `classify_edge` checks membership in `completed_set ∪ failed_set`, and Skipped is in neither. **This means `--skip foo` cascades skips through the dependency chain regardless of edge condition.** Rationale: `--skip` is the user explicitly saying "this task did not run at this time"; carrying that through the graph is more honest than pretending it succeeded. Document prominently - this differs from how some task runners treat `--skip` (e.g., GNU Make's `-W` treats a skipped target as "made"). Phase 4 implements this; Phase 7 docs call it out.
- [ ] Foreach interaction: resolved by the Prerequisites section. Virtual parents become real aggregator tasks; their status is the aggregation of their subtasks (any failed → parent failed). A downstream `{task: host, when: failure}` therefore fires when ANY subtask of `host` failed. Pending Architect consensus in round 3 - this is the position v3 takes, not the only option.
- [ ] Should `When` get more variants later (e.g., `when: timeout`, `when: signal`)? The enum is `#[derive(Deserialize)]` with `rename_all = "kebab-case"` - adding variants is forward-compatible at the type level but breaks ottofiles that pin against the exhaustive set in tests. Leave at `Success | Failure | Always` for now.
- [ ] Should there be an `otto.fail-fast` config flag in the first cut, or wait for someone to ask? Lean wait. Less code, fewer knobs.

## References

### Design history
- v1: simple `on-failure:` design; Architect review found it wired through the wrong module (`graph.rs` instead of the scheduler).
- v2: `on-failure:` design re-anchored on Parser + scheduler; superseded by this generalization to conditional edges.
- Architect review round 1: surfaced the `graph.rs`/scheduler disconnect, whole-run-halt reality at `scheduler.rs:446`, and `executor::Task` schema gap.
- Architect review round 2: identified the structurally-impossible round-trip serialization in v3 (Serialize has no access to parent ConfigSpec), and the pre-existing virtual-parent deadlock.
- Architect review round 3: consensus on virtual-parent aggregator using `When::Always` edges and the corrected round-trip story.
- Driving conversation: cargo-fmt failure auto-fix pattern (2026-05-23 / 2026-05-24).

### Code locations
- Polymorphic string-or-object deserialize pattern (template for `EdgeSpec`): `src/cfg/param.rs:88-117` (`deserialize_value`).
- Parser entry point: `src/cli/parser.rs:691` (`process_tasks_with_filter`).
- Closure expansion: `src/cli/parser.rs:1166` (`collect_transitive_deps`).
- Dependency computation: `src/cli/parser.rs:1053` (`compute_task_deps_from_specs`).
- Virtual-parent skip site (Phase 0 removes this): `src/cli/parser.rs:733-735`.
- Foreach expansion (Phase 0 modifies): `src/cli/parser.rs:1113-1160` (`expand_foreach_tasks_with_serial`).
- Build-filtered-deps (only used by propagate_params, NOT scheduler): `src/cli/parser.rs:956-982`.
- Scheduler entry point: `src/executor/scheduler.rs:300+` (`execute_all`).
- Scheduler failure path (Phase 4 rewrites): `src/executor/scheduler.rs:415-446`.
- Scheduler success-path sweep (Phase 4 extends): `src/executor/scheduler.rs:395-405`.
- Existing task model: `src/cfg/task.rs:217-232`.
- Runtime task envelope: `src/executor/task.rs:1-50`.
- Bash prologue (for `when: always` footgun context): `src/executor/action.rs:269`.
- Visualizer (not on scheduling path; cosmetic in Phase 6): `src/executor/graph.rs`.

### External docs
- Rust scaffold template (updated in Phase 7): `~/.claude/skills/ottofile/references/rust.md`.
- Otto skill (updated in Phase 7): `~/.claude/skills/otto/SKILL.md`.
