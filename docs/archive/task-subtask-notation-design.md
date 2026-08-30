# Design Document: task:subtask Notation and Improved Graph Display

**Author:** Claude
**Date:** 2026-01-25
**Status:** Approved
**Review Passes Completed:** 10/10 (5 initial + major revision + 4 final passes)

## Summary

Two related enhancements for otto-rs: (1) enable running specific subtasks via `otto install:td` without running all sibling subtasks, and (2) improve graph display to show `install:{td,ts,cs}` instead of `install:* [3 items]` for list-based foreach tasks.

### Quick Reference

| Feature | Current | Proposed | File |
|---------|---------|----------|------|
| `otto install:td` | Runs ALL subtasks | Runs only `install:td` | parser.rs |
| Graph (items) | `install:* [3 items]` | `install:{td,ts,cs}` | graph.rs |
| Graph (glob) | `examples:* [8 items]` | `examples:*.sh [8 items]` | graph.rs |
| Graph (range) | `batch:* [10 items]` | `batch:1..10` | graph.rs |

## Problem Statement

### Background

Otto's foreach feature generates subtasks from parent tasks. A task like `install` with `foreach: { items: [td, ts, cs] }` expands to `install:td`, `install:ts`, and `install:cs`. The CLI parser correctly recognizes these subtask names as valid task names (via `get_task_names()` at parser.rs:627-644).

### Problem

**Problem 1: Subtask targeting doesn't work correctly**

When a user runs `otto install:td`:
1. Parser correctly identifies `install:td` as a valid task name
2. `collect_transitive_deps("install:td", ...)` is called
3. Task is added to collected set
4. Lines 897-904 scan for ALL tasks starting with `install:` and add them ALL

The subtask collection logic doesn't distinguish between:
- User requesting parent `install` → should run all subtasks
- User requesting specific `install:td` → should run only that subtask

**Problem 2: Graph display is suboptimal for list-based foreach**

- For explicit lists like `items: [td, ts, cs]`, graph shows `install:* [3 items]`
- This is uninformative - the `*` doesn't indicate what the items are
- For small item lists, showing `install:{td,ts,cs}` would be clearer
- The `[N items]` suffix is redundant when using brace notation

### Goals

- Enable running specific subtasks: `otto install:td` runs only `install:td`
- Running parent still works: `otto install` runs all subtasks
- Allow subtasks in dependency lists: `before: ["install:td"]` or `after: ["install:td"]`
- Improve graph display for list-based foreach tasks
- Maintain backward compatibility

### Non-Goals

- Changing the foreach expansion logic
- Supporting nested foreach (foreach within subtasks)
- Changing how glob-based foreach displays (keep showing pattern)

## Proposed Solution

### Overview

**Feature 1:** Modify `collect_transitive_deps()` to only expand subtasks when the parent task (no colon in name) is explicitly requested.

**Feature 2:** Add a `parent` field to Task that points to the parent task name. Graph visualization navigates from subtask → parent → original `ForeachSpec` to get display metadata. No new types needed - we reuse the existing `ForeachSpec` that already contains items/glob/range info.

### Architecture

#### Feature 1: Subtask Targeting

The key insight is that a subtask name contains `:`, while a parent name does not. When collecting deps for a name WITH a colon, we should NOT expand to sibling subtasks.

```
User runs:         collect_transitive_deps receives:    Should expand subtasks?
-----------        --------------------------------     ----------------------
otto install       "install"                            YES (no colon)
otto install:td    "install:td"                         NO (has colon)
```

#### Feature 2: Graph Display

Navigate parent relationship to access existing metadata:

```
Task.parent → parser.config_spec.tasks[parent] → TaskSpec.foreach → ForeachSpec (has items/glob/range)
```

Key insight: The original `ForeachSpec` with all metadata already exists in `parser.config_spec.tasks`. We just need a way to navigate to it. Adding a `parent` field to subtasks enables this navigation without duplicating data.

### Module Structure

Understanding the existing module layout and data flow:

```
src/
├── cfg/
│   └── task.rs       # TaskSpec, ForeachSpec (YAML config representation)
├── cli/
│   └── parser.rs     # Parses CLI args, expands foreach, creates Tasks
│                     # parser.config_spec.tasks = original TaskSpecs WITH ForeachSpec
│                     # expanded_tasks = post-expansion (virtual parent has foreach: None)
└── executor/
    ├── task.rs       # Task (runtime representation, used by DAG/scheduler)
    └── graph.rs      # DAG visualization (creates its own Parser at line 121)
```

**Data flow:**
1. YAML → `TaskSpec` with `ForeachSpec` stored in `parser.config_spec.tasks`
2. `expand_foreach_tasks_with_serial()` creates subtask TaskSpecs + virtual parent (with `foreach: None`)
3. `TaskSpec` → `Task` for execution (virtual parents filtered out at line 686-688)
4. `Task`s form DAG for scheduling and graph display
5. **Original `ForeachSpec` preserved in `parser.config_spec.tasks`** - accessible if we know parent name

### Data Model

Add a simple `parent` field to `src/executor/task.rs`:

```rust
pub struct Task {
    pub name: String,
    pub parent: Option<String>,  // NEW: "install" for subtask "install:td", None for regular tasks
    pub task_deps: Vec<String>,
    pub file_deps: Vec<String>,
    pub output_deps: Vec<String>,
    pub envs: HashMap<String, String>,
    pub values: HashMap<String, Value>,
    pub action: String,
    pub hash: String,
}
```

This is similar to PyDoit's `subtask_of` field - a simple string reference to the parent task.

### Implementation Plan

#### Phase 1: Fix subtask targeting in collect_transitive_deps

**File:** `src/cli/parser.rs`

Change the subtask expansion logic (lines ~897-904) from:

```rust
// Collect subtasks for foreach parent tasks
let prefix = format!("{}:", task_name);
for subtask_name in task_specs.keys() {
    if subtask_name.starts_with(&prefix) {
        Self::collect_transitive_deps(subtask_name, ...)?;
    }
}
```

To:

```rust
// Only collect subtasks if this is a parent task (no colon in name)
// If user requests "install:td", don't also collect install:ts, install:cs
if !task_name.contains(':') {
    let prefix = format!("{}:", task_name);
    for subtask_name in task_specs.keys() {
        if subtask_name.starts_with(&prefix) {
            Self::collect_transitive_deps(subtask_name, ...)?;
        }
    }
}
```

#### Phase 2 & 3: Add and populate parent field

These phases are tightly coupled and should be implemented together.

**File:** `src/executor/task.rs`

1. Add `parent: Option<String>` field to `Task` struct
2. Update `Task::new()` to accept `parent` parameter
3. Update all ~40 call sites to pass `None` (mechanical change)
4. Update `from_task_with_cwd_and_global_envs()` to accept `parent` parameter

```rust
impl Task {
    pub fn new(
        name: String,
        parent: Option<String>,  // NEW parameter
        task_deps: Vec<String>,
        // ... rest unchanged
    ) -> Self {
        // ...
        Self {
            name,
            parent,
            task_deps,
            // ...
        }
    }
}
```

**File:** `src/cli/parser.rs` - At task creation time (~line 690), derive parent from task name:

```rust
for task_name in &tasks_needed {
    let task_spec = expanded_tasks.get(task_name)?;

    // Skip virtual parent tasks (empty action)
    if task_spec.action.is_empty() {
        continue;
    }

    // Derive parent for subtasks
    let parent = if task_name.contains(':') {
        Some(task_name.split(':').next().unwrap().to_string())
    } else {
        None
    };

    let mut task = Task::from_task_with_cwd_and_global_envs(
        task_spec, &self.cwd, &global_envs, parent
    );
    // ... rest of task creation
}
```

#### Phase 4: Update graph to navigate parent relationship

**File:** `src/executor/graph.rs`

The graph visualizer already creates its own Parser (line 121). Modify to keep access to original TaskSpecs:

```rust
pub async fn execute_command(task: &crate::cli::parser::Task) -> Result<()> {
    // ... existing format/options parsing ...

    let args: Vec<String> = env::args().collect();
    let mut parser = Parser::new(args)?;
    let (all_tasks, _, _) = parser.parse_all_tasks()?;

    // Pass original specs to visualizer for foreach metadata lookup
    // NOTE: config_spec is currently private - need to add pub or getter
    let original_specs = parser.original_task_specs(); // Add this getter to Parser

    let dag = Self::from_tasks(all_tasks)?;
    let visualizer = DagVisualizer::new(options);
    let result = visualizer.visualize(&dag, original_specs)?;

    // ...
}
```

Update `collapse_foreach_subtasks()` to use parent navigation:

```rust
fn collapse_foreach_subtasks(
    dag: &DAG<Task>,
    original_specs: &HashMap<String, TaskSpec>,  // NEW parameter
) -> HashMap<String, CollapsedTaskInfo> {
    // ... grouping logic unchanged ...

    // When formatting display for a foreach group:
    if let Some(parent_name) = subtasks[0].parent.as_ref() {
        if let Some(parent_spec) = original_specs.get(parent_name) {
            if let Some(foreach_spec) = parent_spec.foreach.as_ref() {
                let display_name = Self::format_foreach_display(parent_name, foreach_spec, subtasks.len());
                // ... use display_name
            }
        }
    }
    // Fallback to infer_subtask_pattern() if navigation fails
    // ...
}

fn format_foreach_display(parent: &str, foreach: &ForeachSpec, count: usize) -> String {
    if let Some(ref glob) = foreach.glob {
        // Glob notation: examples:*.sh [8 items]
        format!("{}:{} [{} items]", parent, glob, count)
    } else if !foreach.items.is_empty() {
        if foreach.items.len() <= 6 {
            // Brace notation for small lists: install:{td,ts,cs}
            format!("{}:{{{}}}", parent, foreach.items.join(","))
        } else {
            // Too many items: install:{...} [15 items]
            format!("{}:{{...}} [{} items]", parent, foreach.items.len())
        }
    } else if let Some(ref range) = foreach.range {
        // Range notation: batch:1..10
        format!("{}:{}", parent, range)
    } else {
        // Fallback
        format!("{}:* [{} items]", parent, count)
    }
}
```

#### Phase 5: Add dependency validation

**File:** `src/cli/parser.rs`

Add validation in `compute_task_deps_from_specs()` to warn/error on unknown dependencies:

```rust
// Validate all dependencies exist
for (task_name, deps) in &task_deps {
    for dep in deps {
        if !task_specs.contains_key(dep) {
            return Err(eyre!("Task '{}' has unknown dependency '{}'", task_name, dep));
        }
    }
}
```

## Alternatives Considered

### Alternative 1: Command-line flag for subtask-only mode

- **Description:** Add `--only` flag: `otto --only install:td`
- **Pros:** Explicit, no behavior change to existing commands
- **Cons:** Verbose, unintuitive, most users expect `task:subtask` to run just that subtask
- **Why not chosen:** The current behavior (running all siblings) is surprising and inconsistent with expectations

### Alternative 2: Infer items from subtask names in graph

- **Description:** Instead of storing metadata, extract items from actual subtask names
- **Pros:** No schema changes, works with existing data
- **Cons:** Can't distinguish glob from items, loses original information
- **Why not chosen:** Loses fidelity - can't show `*.sh` vs `{a.sh,b.sh}` distinction

### Alternative 3: Store ForeachSource enum on each subtask (Original Plan)

- **Description:** Create a new `ForeachSource` enum (Glob/Items/Range variants) and store it on each subtask Task
- **Pros:** Data travels with task through pipeline; graph access is simple
- **Cons:**
  - Requires new enum type
  - Every subtask duplicates the same data (N copies for N subtasks)
  - Must update ~40 Task::new() call sites with new parameter
  - Circular import concern between cfg and executor modules
- **Why not chosen:** Simpler solution found - navigate to existing ForeachSpec via parent pointer

### Alternative 4: Separate metadata map

- **Description:** Pass a separate `HashMap<String, ForeachSource>` to graph visualizer
- **Pros:** No changes to Task struct
- **Cons:** Parallel data structure can get out of sync; threading through call chain
- **Why not chosen:** More complex than parent navigation approach

### Alternative 5: Include virtual parents in DAG

- **Description:** Keep virtual parent tasks in DAG for graph purposes
- **Pros:** Single source of truth on parent
- **Cons:** Mixes executable and non-executable nodes; scheduler must filter
- **Why not chosen:** Changes DAG semantics; parent navigation is simpler

### Decision: Parent Navigation (Revised Approach)

After code review, we discovered a simpler solution:

1. **The original `ForeachSpec` already exists** in `parser.config_spec.tasks`
2. **Graph already creates a Parser** (line 121), so it has access to original specs
3. **Adding a simple `parent` field** enables navigation without data duplication

This approach:
- No new enum types needed
- Single source of truth (original ForeachSpec)
- Minimal Task struct change (one `Option<String>` field)
- Similar to PyDoit's proven `subtask_of` pattern

## Technical Considerations

### Dependencies

- No new external dependencies
- No new internal types - reuses existing `ForeachSpec`

### Performance

- `parent: Option<String>` adds ~24 bytes per subtask (Option discriminant + String pointer)
- No data duplication - ForeachSpec read from single source
- Graph lookup is O(1) HashMap access

### Security

No security implications - this is display/targeting logic only.

### Testing Strategy

**Unit tests (in parser.rs tests):**

```rust
#[test]
fn test_collect_transitive_deps_parent_expands_subtasks() {
    // Setup: task_specs contains "install", "install:td", "install:ts"
    let (task_deps, task_specs) = setup_foreach_task_specs();
    let mut collected = HashSet::new();

    Parser::collect_transitive_deps("install", &task_deps, &task_specs, &mut collected)
        .expect("should succeed");

    assert!(collected.contains("install"));
    assert!(collected.contains("install:td"));
    assert!(collected.contains("install:ts"));
}

#[test]
fn test_collect_transitive_deps_subtask_does_not_expand_siblings() {
    let (task_deps, task_specs) = setup_foreach_task_specs();
    let mut collected = HashSet::new();

    Parser::collect_transitive_deps("install:td", &task_deps, &task_specs, &mut collected)
        .expect("should succeed");

    assert!(collected.contains("install:td"));
    assert!(!collected.contains("install:ts"));  // Key assertion
    assert!(!collected.contains("install"));     // Parent not included either
}

#[test]
fn test_subtask_has_parent_field() {
    let subtask = Task::new(
        "install:td".to_string(),
        Some("install".to_string()),  // parent
        vec![], vec![], vec![],
        HashMap::new(), HashMap::new(),
        "echo test".to_string(),
    );
    assert_eq!(subtask.parent, Some("install".to_string()));

    let regular = Task::new(
        "build".to_string(),
        None,  // no parent
        vec![], vec![], vec![],
        HashMap::new(), HashMap::new(),
        "cargo build".to_string(),
    );
    assert_eq!(regular.parent, None);
}
```

**Integration tests (example ottofile):**

```yaml
# test-subtask-targeting.otto.yml
tasks:
  install:
    foreach:
      items: [td, ts, cs]
    bash: echo "Installing ${item}"

  deploy:
    before: ["install:td"]  # Only depends on one subtask
    bash: echo "Deploying"

  notify:
    before: ["install:td", "install:ts"]  # Depends on multiple specific subtasks
    bash: echo "Notifying about td and ts"
```

```bash
# Test 1: Run specific subtask only
$ otto install:td
# Expected output: "Installing td" (only)

# Test 2: Run parent task (all subtasks)
$ otto install
# Expected output: "Installing td", "Installing ts", "Installing cs"

# Test 3: Dependency on specific subtask
$ otto deploy
# Expected: runs install:td, then deploy (NOT install:ts, install:cs)

# Test 4: Dependency on multiple specific subtasks
$ otto notify
# Expected: runs install:td and install:ts (in parallel), then notify (NOT install:cs)
```

**How `before`/`after` with subtasks works:**

The key is that dependency resolution (`compute_task_deps_from_specs`) happens AFTER foreach expansion. At that point, `install:td` exists in `expanded_tasks`, so `before: ["install:td"]` resolves correctly. The Phase 1 fix ensures that when we collect transitive deps for `install:td`, we don't also pull in sibling subtasks.

**Graph display tests (in graph.rs tests):**

```rust
#[test]
fn test_graph_list_foreach_brace_notation() {
    let (dag, original_specs) = create_test_dag_with_foreach_items(&["td", "ts", "cs"]);
    let collapsed = DagVisualizer::collapse_foreach_subtasks(&dag, &original_specs);
    assert_eq!(collapsed["install"].display_name, "install:{td,ts,cs}");
}

#[test]
fn test_graph_glob_foreach_pattern_notation() {
    let (dag, original_specs) = create_test_dag_with_foreach_glob("*.sh", 8);
    let collapsed = DagVisualizer::collapse_foreach_subtasks(&dag, &original_specs);
    assert_eq!(collapsed["scripts"].display_name, "scripts:*.sh [8 items]");
}

#[test]
fn test_graph_range_foreach_notation() {
    let (dag, original_specs) = create_test_dag_with_foreach_range("1-10");
    let collapsed = DagVisualizer::collapse_foreach_subtasks(&dag, &original_specs);
    assert_eq!(collapsed["batch"].display_name, "batch:1-10");
}

#[test]
fn test_graph_fallback_without_original_specs() {
    // Legacy path - no original specs available
    let dag = create_test_dag_legacy();
    let collapsed = DagVisualizer::collapse_foreach_subtasks(&dag, &HashMap::new());
    // Falls back to inference
    assert!(collapsed["install"].display_name.contains("*"));
}
```

### Rollout Plan

| Phase | Description | Acceptance Criteria |
|-------|-------------|---------------------|
| 1 | Subtask targeting fix | `otto install:td` runs only that subtask; `otto install` runs all |
| 2-3 | Add and populate parent field | All existing tests pass; subtasks have parent set; regular tasks have None |
| 4 | Graph uses parent navigation | List shows `{items}`, glob shows pattern, range shows bounds; fallback works |
| 5 | Dependency validation | Unknown deps (including typos like `install:tx`) produce error with helpful message |

## Risks and Mitigations

| Risk | Likelihood | Impact | Mitigation |
|------|------------|--------|------------|
| Breaks existing workflows relying on current behavior | Low | Medium | Current behavior (running all siblings) is a bug, not a feature; add release note |
| Graph can't find original specs | Low | Low | Fallback to current inference behavior (`infer_subtask_pattern`) |
| Dependency validation too strict | Low | Medium | Make it a warning first, then error in future version |
| Brace notation unreadable for items with special chars | Low | Low | Escape or fall back to `{...} [N items]` for items containing `,` `{` `}` |

## Edge Cases

### Subtask of subtask (nested colons)

If a task name contains multiple colons (e.g., `group:subgroup:item`), the colon check still works correctly:
- `"group:subgroup:item".contains(':')` → `true` → don't expand
- This is correct behavior: we only expand from the top-level parent
- Parent extraction: `split(':').next()` → `"group"` (first segment only)

**Note:** Otto doesn't currently support nested foreach, so `group:subgroup:item` shouldn't occur in practice. If it does (e.g., user manually named a task this way), the parent lookup would find `"group"` which may or may not have a foreach spec. The fallback to `infer_subtask_pattern()` handles this gracefully.

### Regular tasks with colons in names

If a user defines a regular (non-foreach) task with a colon in the name:
```yaml
tasks:
  "api:v2":  # Not a foreach task, just has colon in name
    bash: echo "API v2"
```

This will NOT be expanded (correct) but also won't show up when running `otto api` (also correct - there's no parent `api` task). However, this could be confusing. Consider:
- Documenting that colons are reserved for foreach subtask notation
- Warning if a non-foreach task has a colon in its name

### Empty foreach

If foreach resolves to 0 items, there are no subtasks. The parent task name without colon will not exist in `task_specs`. Running `otto parent` will fail with "task not found" which is the correct behavior.

### Dependencies on non-existent subtasks (typos)

```yaml
tasks:
  install:
    foreach:
      items: [a, b, c]
    bash: echo ${item}

  deploy:
    before: ["install:x"]  # "x" doesn't exist in items! Typo for "a"?
    bash: echo deploy
```

**Current behavior:** Silently ignores the dependency. `deploy` runs immediately without waiting for anything. This is dangerous because the user thinks they have a dependency but they don't.

**Proposed (Phase 5):** Validation catches this and errors:
```
Error: Task 'deploy' has unknown dependency 'install:x'
```

This is a key reason why Phase 5 validation is important - it catches typos in subtask references.

### Subtask depending on sibling subtask

```yaml
tasks:
  install:
    foreach:
      items: [td, ts, cs]
    before: ["install:td"]  # Each subtask depends on install:td???
    bash: echo ${item}
```

This is already handled by existing foreach logic - dependencies are inherited. But this example creates a cycle (`install:td` depends on itself). The DAG builder should catch this. Worth adding a test to verify.

### Config changes between parse and graph

Graph creates its own Parser instance (line 121) which re-reads the ottofile. If the file changed on disk between task execution and graph visualization, the `original_specs` might not match the tasks in the DAG. This is an existing race condition in otto, not introduced by this change. The fallback to `infer_subtask_pattern()` handles mismatches gracefully.

### Graph with mixed foreach types

```yaml
tasks:
  by_list:
    foreach: { items: [a, b, c] }
    bash: echo ${item}

  by_glob:
    foreach: { glob: "*.sh" }
    bash: ./${item}

  by_range:
    foreach: { range: "1-5" }
    bash: echo ${item}
```

Each should display differently in graph. Verify the fallback (`infer_subtask_pattern`) still works for legacy Tasks without parent navigation.

## Open Questions

- [ ] Should the brace notation threshold (6 items) be configurable?
- [ ] Should we show both `{items}` AND count, e.g., `install:{td,ts,cs} [3]`?
- [ ] Should dependency validation be an error or warning?
- [ ] Should we warn/error on non-foreach tasks with colons in names?
- [ ] What if `items` contains special characters like `{`, `}`, or `,`? Need escaping in brace notation?

## Files to Modify

1. **src/cli/parser.rs**
   - `collect_transitive_deps()`: Add colon check before expanding subtasks (Phase 1)
   - Task creation (~line 690): Derive and pass `parent` for subtasks (Phase 3)
   - `compute_task_deps_from_specs()`: Add dependency validation (Phase 5)
   - Add `pub fn original_task_specs(&self) -> &HashMap<String, TaskSpec>` getter (Phase 4)

2. **src/executor/task.rs**
   - Add `parent: Option<String>` field to `Task` struct (Phase 2)
   - Update `Task::new()` signature and all constructors (Phase 2)
   - Update ~40 call sites to pass `None` for parent (Phase 2)

3. **src/executor/graph.rs**
   - `execute_command()`: Pass `parser.config_spec.tasks` to visualizer (Phase 4)
   - `collapse_foreach_subtasks()`: Accept original_specs parameter (Phase 4)
   - `format_foreach_display()`: Use ForeachSpec directly for display formatting (Phase 4)
   - Keep `infer_subtask_pattern()` as fallback for legacy paths (Phase 4)

## Changelog

| Pass | Changes |
|------|---------|
| 1 (Draft) | Initial structure from implementation plan |
| 2 (Correctness) | Fixed Task::new() call site count; added cfg→executor module flow |
| 3 (Clarity) | Added module structure diagram; expanded code examples with context |
| 4 (Edge Cases) | Added edge case analysis; new open questions; expanded risks |
| 5 (Excellence) | Added quick reference table; acceptance criteria for phases |
| 6 (Post-Review) | **Major revision**: Replaced ForeachSource enum approach with simpler parent navigation. Researched PyDoit's `subtask_of` pattern. Original ForeachSpec already exists in parser.config_spec.tasks - just navigate to it via parent pointer. |
| 7 (Correctness) | Fixed: config_spec is private - need getter; fixed `?` operator usage in non-Result context; corrected code examples |
| 8 (Clarity) | Combined Phase 2 & 3; removed Option A/B/C naming; fixed test code syntax |
| 9 (Edge Cases) | Added: nested colons handling note; config race condition edge case; updated rollout phases |
| 10 (Excellence) | Final polish; document ready for implementation |
| 11 (Deps Review) | Added: explicit `before`/`after` subtask examples; clarified dependency timing; made Phase 5 validation non-optional (catches typos) |

## References

- [foreach-subtasks.md](foreach-subtasks.md) - Original foreach feature design
- [foreach-builtin-flags-design.md](foreach-builtin-flags-design.md) - Related --Serial flag design
- PyDoit `subtask_of` pattern - `doit/task.py` lines 128-129, 174, 240-241
- `src/cli/parser.rs:627-644` - `get_task_names()` subtask name recognition
- `src/cli/parser.rs:871-907` - `collect_transitive_deps()` current implementation
- `src/cli/parser.rs:686-688` - Virtual parent filtering (empty action skip)
- `src/cli/parser.rs:690` - Task creation from TaskSpec
- `src/cli/parser.rs:817-865` - `expand_foreach_tasks_with_serial()` - creates virtual parent
- `src/cfg/task.rs:483-498` - `as_virtual_parent()` - sets `foreach: None`
- `src/executor/graph.rs:121` - Graph creates its own Parser
- `src/executor/graph.rs:368-431` - `collapse_foreach_subtasks()` graph collapsing
- `src/executor/graph.rs:437-467` - `infer_subtask_pattern()` pattern inference
