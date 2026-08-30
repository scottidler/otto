# Design Document: Graph After-Relationship Visualization

**Author:** Scott Idler
**Date:** 2026-02-10
**Status:** Draft
**Review Passes Completed:** 5/5

## Summary

Otto's `Graph` command currently displays `after` relationships identically to `before` dependencies, which is visually misleading. The `after` relationship is semantically bidirectional — running the source task triggers the after-task, and running the after-task independently pulls in the source as a prerequisite. This design introduces Unicode arrow annotations (double-stroke Set: `⇐` / `⇔`) into the Graph tree output to distinguish before-dependencies from after-relationships, and reverses the display order so after-tasks appear nested under their source task.

## Problem Statement

### Background

Otto tasks support two relationship types:
- **`before`**: Traditional dependencies. `ci` with `before: [lint, check, test]` means lint, check, and test must run before ci.
- **`after`**: Side-effect triggers. `cov` with `after: [cov-report]` means running `cov` will automatically trigger `cov-report` afterward. However, running `cov-report` directly will also pull in `cov` as an implicit prerequisite.

In `compute_task_deps_from_specs` (`cli/parser.rs:784-818`), both relationship types are flattened into a single `task_deps: Vec<String>` on the executor `Task` struct. This is correct for execution — the executor only needs to know ordering, not provenance. But the Graph visualization then renders all edges identically.

### Problem

The current Graph output for the `aka` project:

```
├─ cov-report
│  └─ cov
├─ ci
│  ├─ lint
│  ├─ check
│  └─ test
```

Both `cov → cov-report` (after) and `ci → lint` (before) appear identical. A user cannot tell:
1. That running `otto cov` will automatically trigger `cov-report`
2. That the `cov`/`cov-report` relationship is bidirectional (unlike `lint` which is a one-way dep of `ci`)
3. Which relationships were declared as `before` vs `after` in the ottofile
4. That `cov-report` is shown as the parent when `cov` is actually the task that owns the relationship

### Goals

- Visually distinguish `before` dependencies from `after` relationships in Graph output
- Show the bidirectional nature of `after` relationships
- Show `after` tasks nested under their source task (not the reverse)
- Apply the same distinction to DOT/Graphviz output formats
- Preserve all existing Graph functionality (foreach collapsing, file deps, etc.)
- Keep all changes contained within the graph visualization layer — do not modify the execution pipeline

### Non-Goals

- Changing execution semantics of `before`/`after`
- Adding new relationship types
- Modifying `compute_task_deps_from_specs` or the executor `Task` struct
- Changing the CLI interface for the Graph command

## Proposed Solution

### Overview

Add Unicode arrow annotations between tree connectors and task names to indicate relationship type. Use double-stroke arrows for visual consistency:
- `⇐` (U+21D0) for `before` dependencies (one-way: dep flows into parent)
- `⇔` (U+21D4) for `after` relationships (bidirectional: trigger + implicit dep)

Reverse the display order for `after` relationships so the source task appears as the parent and after-tasks appear nested beneath it.

All changes are contained within the graph visualization layer (`executor/graph.rs`). The graph renderer already has access to `original_specs` (the raw `TaskSpec` map with separate `before`/`after` fields) via `parser.original_task_specs()`. This provides all the information needed to determine edge types without modifying the execution pipeline.

### Target Output

```
┌─────────────────────────────────────┐
│           Otto Task DAG             │
└─────────────────────────────────────┘

├─ build
├─ ci
│  ├─ ⇐ lint
│  ├─ ⇐ check
│  └─ ⇐ test
├─ clean
├─ cov
│  └─ ⇔ cov-report
└─ install

┌─────────────────────────────────────┐
│ Legend:                             │
│ ⇐ before (dependency)              │
│ ⇔ after (bidirectional)            │
└─────────────────────────────────────┘
```

### Architecture

The change is contained entirely within the graph visualization layer:

1. **`CollapsedTaskInfo`** — Add a `DepKind` enum and carry it alongside each dep name
2. **`collapse_foreach_subtasks`** — Determine dep kinds by cross-referencing `original_specs`
3. **`generate_ascii`** — Fix leaf detection and add after-tasks as children of their source
4. **`render_collapsed_ascii_subtree`** — Select arrow glyph based on `DepKind`
5. **`generate_dot`** — Use edge styles based on `DepKind`

### Data Model

#### New enum: `DepKind` (in `executor/graph.rs`)

```rust
/// The kind of dependency relationship between two tasks (for graph display only)
#[derive(Clone, Debug, PartialEq, Eq)]
enum DepKind {
    /// Traditional dependency declared via `before:` — the dep must run before the parent
    Before,
    /// Bidirectional relationship declared via `after:` — parent triggers this task,
    /// and this task depends on the parent when run independently
    After,
}
```

This enum lives only in `graph.rs` — the execution pipeline is untouched.

#### Modified: `CollapsedTaskInfo`

```rust
struct CollapsedTaskInfo {
    display_name: String,
    deps: Vec<(String, DepKind)>,    // was Vec<String>
    file_deps_count: usize,
    output_deps_count: usize,
}
```

### Determining DepKind from original_specs

The graph renderer already receives `original_specs: &TaskSpecs` which contains the raw `before` and `after` fields from the ottofile. For each dep in the flattened `task_deps`, we determine the kind by cross-referencing:

```rust
fn classify_dep(
    task_name: &str,
    dep_name: &str,
    original_specs: &TaskSpecs,
) -> DepKind {
    // Check if dep_name appears in task_name's before list
    if let Some(spec) = original_specs.get(task_name) {
        if spec.before.contains(&dep_name.to_string()) {
            return DepKind::Before;
        }
    }
    // Check if task_name appears in dep_name's after list
    // (meaning dep_name declared "after: [task_name]", which was inverted)
    if let Some(dep_spec) = original_specs.get(dep_name) {
        if dep_spec.after.contains(&task_name.to_string()) {
            return DepKind::After;
        }
    }
    // Default to Before for safety (e.g., foreach-generated deps)
    DepKind::Before
}
```

### Rendering Changes

#### ASCII: Leaf detection and after-task display (`generate_ascii`)

The current leaf detection finds tasks that no other task depends on. This needs two changes:

1. **After-tasks should not be standalone leaves** — if `cov-report` only appears in the graph because `cov` has `after: [cov-report]`, then `cov-report` should be nested under `cov`, not shown as a top-level leaf.

2. **Source tasks with after-relationships should show their after-tasks as children** — `cov` should render `cov-report` beneath it with the `⇔` arrow.

```rust
// Build a set of tasks that are someone else's after-task
let after_tasks: HashSet<String> = original_specs
    .iter()
    .flat_map(|(_, spec)| spec.after.iter().cloned())
    .collect();

// A task is a leaf if:
// 1. No other task has a Before dep pointing to it, AND
// 2. It is not an after-task (those are shown under their source)
let mut leaf_tasks: Vec<_> = collapsed_tasks
    .iter()
    .filter(|(name, _)| {
        // Exclude after-tasks from being leaves
        if after_tasks.contains(*name) {
            return false;
        }
        // Exclude tasks that are before-deps of other tasks
        !collapsed_tasks.values().any(|info| {
            info.deps.iter().any(|(dep_name, kind)| {
                dep_name == *name && *kind == DepKind::Before
            })
        })
    })
    .collect();
```

When rendering a task's children in `render_collapsed_ascii_subtree`, render both:
- Before-deps (from `info.deps` where kind is `Before`) — with `⇐`
- After-tasks (looked up from `original_specs`) — with `⇔`

This requires passing `original_specs` into the render function so it can look up after-tasks for each node.

#### ASCII: Arrow rendering (`render_collapsed_ascii_subtree`)

```rust
let arrow = match dep_kind {
    DepKind::Before => "⇐",
    DepKind::After  => "⇔",
};
output.push_str(&format!("{}{} {} {}", indent, connector, arrow, info.display_name));
```

#### ASCII: Updated legend

```rust
output.push_str("\n┌─────────────────────────────────────┐\n");
output.push_str("│ Legend:                             │\n");
output.push_str("│ ├─ Task name [inputs:N] [outputs:M] │\n");
output.push_str("│ ⇐ before (dependency)              │\n");
output.push_str("│ ⇔ after (bidirectional)            │\n");
output.push_str("└─────────────────────────────────────┘\n");
```

#### DOT/Graphviz (`generate_dot`)

Use edge styles to distinguish relationship types:

```rust
match dep_kind {
    DepKind::Before => {
        dot.push_str(&format!(
            "  {source} -> {target} [label=\"before\", color=\"black\"];\n"
        ));
    }
    DepKind::After => {
        dot.push_str(&format!(
            "  {source} -> {target} [label=\"after\", color=\"blue\", \
             style=\"dashed\", dir=\"both\"];\n"
        ));
    }
}
```

The `dir="both"` attribute renders a bidirectional arrow in Graphviz.

### Edge Case: After-task with its own before-deps

If `cov-report` had both an after-relationship with `cov` AND its own `before` deps:

```yaml
cov:
  after: [cov-report]
  bash: cargo llvm-cov ...

cov-report:
  before: [some-formatter]
  bash: display report ...
```

The graph should show:

```
├─ cov
│  └─ ⇔ cov-report
│     └─ ⇐ some-formatter
```

`cov-report` appears under `cov` (after relationship), and `some-formatter` appears under `cov-report` (before dependency). This works naturally with the proposed approach since `cov-report` still has its own `CollapsedTaskInfo` with before-deps.

### Edge Case: Task with both before and after

```yaml
deploy:
  before: [build, test]
  after: [notify, cleanup]
  bash: deploy.sh
```

```
├─ deploy
│  ├─ ⇐ build
│  ├─ ⇐ test
│  ├─ ⇔ notify
│  └─ ⇔ cleanup
```

Before-deps and after-tasks are both shown as children, distinguished by their arrow glyph. Before-deps are rendered first, then after-tasks.

### Implementation Plan

#### Phase 1: Data model changes in graph.rs

1. Add `DepKind` enum (before `CollapsedTaskInfo`, ~line 61)
2. Add `classify_dep(task_name, dep_name, original_specs) -> DepKind` as a `DagVisualizer` method
3. Change `CollapsedTaskInfo.deps` from `Vec<String>` to `Vec<(String, DepKind)>` (line 66)
4. Update `collapse_foreach_subtasks` (line 386): in both the foreach branch (line 425-433) and regular branch (line 452), call `classify_dep` for each dep when building the `deps` vec. Pass `original_specs` which is already available as a parameter

#### Phase 2: ASCII rendering

1. Update `generate_ascii` leaf detection (line 342-348):
   - Build `after_tasks: HashSet<String>` from `original_specs.iter().flat_map(|(_, spec)| spec.after.iter())`
   - Exclude after-tasks from leaf set (they'll render under their source)
   - Change `info.deps.contains(*name)` to check only `DepKind::Before` deps
2. After computing `leaf_tasks`, inject after-tasks as children: for each leaf task, look up its `after` field in `original_specs` and append `(after_name, DepKind::After)` entries to its `CollapsedTaskInfo.deps`. Before-deps render first, then after-deps
3. Update `render_collapsed_ascii_subtree` signature (line 531) to also accept `original_specs: &TaskSpecs` — thread it through from `generate_ascii` and through recursive calls (line 580)
4. In `render_collapsed_ascii_subtree`, change the dep iteration (line 564-576) to destructure `(dep_name, dep_kind)` and format with arrow glyph:
   - `DepKind::Before` → `"⇐ "` prefix
   - `DepKind::After` → `"⇔ "` prefix
5. Update the legend (line 371-375) to show `⇐ before (dependency)` and `⇔ after (bidirectional)` lines

#### Phase 3: DOT/Graphviz rendering

1. Change `generate_dot` signature (line 255) from `(&self, dag: &DAG<Task>)` to `(&self, dag: &DAG<Task>, original_specs: &TaskSpecs)` — update the call site in `visualize` accordingly
2. In the edge loop (lines 309-320), replace the flat `task.task_deps` iteration with classified deps: for each dep, call `classify_dep` and select edge attributes:
   - `DepKind::Before` → `[label="before", color="black"]`
   - `DepKind::After` → `[label="after", color="blue", style="dashed", dir="both"]`
3. Add a legend subgraph after the edge block with sample before/after edges

#### Phase 4: Testing

1. Unit tests for `classify_dep`:
   - Dep in task's `before` list → `Before`
   - Dep in dep's `after` list → `After`
   - Neither (foreach-generated) → `Before` (default)
2. Integration tests for Graph ASCII output — build `TaskSpecs` + `DAG<Task>` fixtures and assert exact string output:
   - Before-only ottofile (all `⇐`)
   - After-only relationships (all `⇔`)
   - Mixed before and after on same task
   - After-task with its own before-deps (nested: `cov → ⇔ cov-report → ⇐ formatter`)
   - Multiple after-tasks on a single source
3. Verify `cargo test` passes with no changes to execution tests (executor untouched)

## Alternatives Considered

### Alternative 1: Modify compute_task_deps_from_specs to carry DepKind

- **Description:** Thread `DepKind` through the entire pipeline from `compute_task_deps_from_specs` → `Task` struct → graph renderer
- **Pros:** Single source of truth for dep kinds
- **Cons:** Touches the execution pipeline (`cli/parser.rs`, `executor/task.rs`), requires modifying `collect_transitive_deps`, `task.task_deps` type, and line 772 where deps are assigned. High risk of breaking execution for a purely visual change
- **Why not chosen:** The graph renderer already has `original_specs` — we can derive dep kinds at render time without touching execution code

### Alternative 2: Annotation-only (no display order change)

- **Description:** Keep `cov-report` as the parent with `cov` nested under it, but annotate the edge with `(after)`
- **Pros:** Minimal structural change
- **Cons:** The display direction is misleading — it shows `cov` as a dep of `cov-report` when the actual authoring intent is `cov` triggering `cov-report`
- **Why not chosen:** Doesn't fix the directional confusion

### Alternative 3: Same-line bidirectional arrows

- **Description:** Show after-linked tasks on the same tree line: `├─ cov ⟷ cov-report`
- **Pros:** Compact, clearly shows bidirectionality
- **Cons:** Breaks down with multiple after-tasks; can't represent one-to-many without implying chaining
- **Why not chosen:** Doesn't scale to one-to-many after relationships

### Alternative 4: Separate sections (deps: / after:)

- **Description:** Group relationships by type under each task with labeled sections
- **Pros:** Maximally explicit
- **Cons:** Verbose, breaks the clean tree aesthetic
- **Why not chosen:** Too noisy for typical use

## Technical Considerations

### Dependencies

- No new crate dependencies required
- Unicode support: `⇐` (U+21D0) and `⇔` (U+21D4) are well-supported in modern terminal emulators

### Performance

- No performance impact — the change only affects graph construction and rendering
- The `classify_dep` cross-reference is O(1) per dep (HashMap lookups + Vec contains on small lists)

### Testing Strategy

- Unit tests for `classify_dep` with various relationship configurations
- Snapshot tests for ASCII graph output comparing before/after rendering
- Integration test: ottofile with mixed before/after relationships, verify Graph output
- Manual verification in terminal for Unicode glyph rendering
- Verify execution is completely unchanged by running existing test suite

## Risks and Mitigations

| Risk | Likelihood | Impact | Mitigation |
|------|------------|--------|------------|
| Unicode arrows render poorly in some terminals | Low | Low | Characters are widely supported; could add ASCII fallback later if needed |
| After-task exclusion from leaves hides tasks from graph | Low | Medium | After-tasks are still shown — just nested under their source instead of as leaves |
| Foreach subtasks with after relationships | Low | Medium | Test with foreach + after combinations; `classify_dep` handles subtask names via parent lookup |

## Acceptance Criteria

1. `otto graph` on an ottofile with `after` relationships shows after-tasks nested under their source with `⇔` prefix
2. `otto graph` on an ottofile with `before` relationships shows before-deps with `⇐` prefix
3. `cov-report` (an after-task of `cov`) no longer appears as a top-level leaf — it appears nested under `cov`
4. `otto graph --dot` produces bidirectional dashed blue edges for after-relationships
5. Legend includes both `⇐` and `⇔` line items
6. All existing tests pass unchanged (executor pipeline untouched)
7. New unit tests cover `classify_dep` and new ASCII output for all edge cases listed in Phase 4

## Resolved Questions

- **Should `⇐` be shown for all before-deps, or only in mixed ottofiles?** → Always show `⇐`. Consistency wins over compactness — users learn one visual language. A before-only ottofile just shows all `⇐`, which is still informative ("these are dependencies"). Conditional rendering adds complexity for marginal noise reduction.

## References

- Existing graph visualization doc: `docs/graph-visualization-implementation.md`
- Key source files:
  - `src/cfg/task.rs:217-230` — `TaskSpec` struct (`before`/`after` fields)
  - `src/cli/parser.rs:784-818` — `compute_task_deps_from_specs` (flattening — NOT modified)
  - `src/executor/task.rs:14-26` — executor `Task` struct (NOT modified)
  - `src/executor/graph.rs:62-71` — `CollapsedTaskInfo` (modified: deps carry `DepKind`)
  - `src/executor/graph.rs:126` — `original_task_specs()` access point
  - `src/executor/graph.rs:331-378` — `generate_ascii` (modified: leaf detection, after-task rendering)
  - `src/executor/graph.rs:531-579` — `render_collapsed_ascii_subtree` (modified: arrow glyphs)
  - `src/executor/graph.rs:255-329` — `generate_dot` (modified: edge styles)
  - `src/executor/graph.rs:386-461` — `collapse_foreach_subtasks` (modified: classify deps)
