# Design Document: Foreach Dynamic Subtask Generation

**Author:** Claude (with Scott)
**Date:** 2026-01-10
**Status:** Draft
**Review Passes:** 5/5

## Summary

Add a `foreach` directive to otto that dynamically expands a single task definition into multiple parallel subtasks at config load time. This enables patterns like running all `examples/*.sh` files in parallel instead of serially, reducing execution time from ~2 minutes to seconds.

## Problem Statement

### Background

Otto is a YAML-based task runner that supports parallel task execution via a Tokio-based scheduler with semaphore-controlled concurrency. Currently, when users need to run the same action across multiple files or items (like running all example scripts), they must either:

1. Write a bash loop that runs items serially (current engram approach - ~2 minutes)
2. Manually define N separate tasks in YAML (tedious, not maintainable)

PyDoit solves this elegantly with Python generators - a `task_*` function can `yield` multiple task dictionaries, creating subtasks like `taskname:subtaskname` that are scheduled as first-class citizens. However, otto is YAML-based and cannot execute code during task loading.

### Problem

Users cannot leverage otto's parallel execution infrastructure for dynamic workloads where the number of items is determined at runtime (e.g., files matching a glob pattern). This forces serial execution of what could be embarrassingly parallel work.

**Current engram examples task** (`~/repos/neuraphage/engram/.otto.yml`):
```yaml
examples:
  help: "Run all CLI usage examples"
  bash: |
    for example in examples/*.sh; do
      name=$(basename "$example")
      echo "--- Running $name ---"
      if bash "$example"; then
        echo "passed"
      else
        echo "failed"
        failed=$((failed + 1))
      fi
    done
```

This runs ~10 examples serially, taking ~2 minutes total when they could run in parallel.

### Goals

- Enable dynamic subtask generation from glob patterns or explicit lists
- Subtasks are first-class scheduled entities (parallel execution, proper dependency handling)
- Subtask naming follows `parent:item` convention (like pydoit)
- Minimal YAML syntax that feels natural alongside existing otto patterns
- Preserve all existing task features (params, envs, before/after, input/output)

### Non-Goals

- Runtime code execution (we stay within YAML's declarative model)
  - **Amended 2026-08-29** (`docs/design/2026-08-28-boundary-fixes-and-dynamic-foreach.md`, Phase 6): `foreach: command:` adds one execution site, deliberately and with the tension weighed. otto already shells out at load time for every `envs:` `$()`, unconditionally, on every invocation including `--help`. A command source is *lazier* than that existing posture, not looser: it fires only when the invocation actually needs the expansion, at most once, and never for `--help`. The declarative model still holds for the three static sources, which execute nothing.
- Complex iteration logic (nested loops, conditionals during expansion)
- Support for infinite/streaming iteration sources
- Breaking changes to existing task syntax

## Proposed Solution

### Overview

Introduce a `foreach` field in task definitions that triggers subtask expansion during YAML parsing. When present, the single task definition becomes a template that generates N subtasks, one per item in the foreach source.

**Expansion timing:** Subtasks are generated at config load time (when otto parses the YAML), not at task execution time. This means:
- Glob patterns are resolved once when otto starts
- If files are added/removed after otto starts, the subtask list doesn't change
- This is consistent with how otto's `input:` file dependencies work

### YAML Syntax

```yaml
tasks:
  examples:
    help: "Run all CLI usage examples"
    foreach:
      glob: "examples/*.sh"        # Source: glob pattern
      # OR
      items: [dev, staging, prod]  # Source: explicit list
      # OR
      range: "1-10"                # Source: numeric range (inclusive)
      # OR
      command: "scripts/list.sh"   # Source: the command's stdout lines

      as: example                  # Variable name (default: "item")
      parallel: true               # Run subtasks in parallel (default: true)
    bash: |
      echo "Running ${example}"
      bash "${example}"
```

**Generated subtasks:**
- `examples:01-basic.sh`
- `examples:02-search.sh`
- `examples:03-context.sh`
- ... (one per matched file)

### Subtask Naming

Subtask names follow the pattern `{parent}:{item_identifier}`:

| Source Type | Item Identifier | Ordering |
|-------------|-----------------|----------|
| `glob` | Filename without directory (e.g., `01-basic.sh`) | Alphabetically sorted |
| `items` | The item value itself (e.g., `dev`) | Preserved from YAML |
| `range` | Zero-padded number (e.g., `01`, `02`) | Numeric order |
| `command` | The output line, spaces replaced with `_` (e.g., `alpha`) | The command's line order |

**Ordering guarantee:** Glob results are always sorted alphabetically for deterministic, reproducible builds. This ensures `examples:01-basic.sh` always comes before `examples:02-search.sh`.

### Command Source

`command:` draws the item list from a command's stdout, so a task can expand over something computed per invocation (a service registry, a manifest, a `git` query) instead of something written into the ottofile:

```yaml
tasks:
  up:
    foreach:
      command: "scripts/list-services.sh"
      as: svc
      parallel: false
    bash: |
      scripts/svc.sh run "${svc}" up
```

**Items:** the non-empty lines of stdout, each whitespace-trimmed. The identifier is sanitized exactly as a glob identifier is (internal spaces become `_`); the value keeps its original spacing. `max_items` and the duplicate-subtask-name check apply unchanged.

**Execution contract:**

- The command runs via `sh -c`, with cwd = the ottofile's directory.
- Its environment is otto's inherited environment plus the resolved global `envs:`.
- **Task params are NOT available.** Params resolve *after* foreach expansion, so `${svc}` (or any other param) is unset while the command runs. Dynamic input comes from the shell environment or from a file the command reads.
- `command:` is mutually exclusive with `glob:`, `items:`, and `range:`. Combining them is a config error naming the task.

**When it runs (lazy, cached):** unlike the static sources, a command source executes only when the invocation actually needs the expansion, and at most once per invocation:

| Invocation | Runs the command? |
|------------|-------------------|
| `otto up` / `otto up:alpha` (the task or one of its subtasks is targeted) | yes, once |
| `otto <default tasks>` where the task is a default | yes, once |
| `otto --tasks`, `otto --list-subtasks` (enumeration surfaces) | yes, once |
| `otto --help`, `otto help up`, `otto up --help` | **never** - help renders `[dynamic]` in place of the item count |
| `otto build` (an unrelated task) | **never** |

**Failure is loud:**

- Non-zero exit: a config error naming the task, the command, and the exit code; nothing reaches stdout, and otto exits non-zero.
- Zero lines with exit 0: a legitimate empty scope. otto prints one notice on stderr and the task expands to zero subtasks.
- Recursion: the command runs with `OTTO_FOREACH_COMMAND=<task>` in its environment. If a nested otto would resolve a command source for a task already named there, it errors with the cycle named instead of recursing. A nested otto that targets ordinary tasks is unaffected.

### Variable Injection

The `as` field defines the variable name injected into the task's environment:

```yaml
foreach:
  glob: "tests/*.rs"
  as: test_file
bash: |
  cargo test --test "${test_file}"
```

The variable is injected as an environment variable, which the shell automatically expands. No string substitution is performed on the script—standard shell variable expansion handles it.

**Available variables:**
- `${test_file}` or `$test_file` - the full path to the matched file
- `${OTTO_FOREACH_ITEM}` - same value (standard name for scripting)
- `${OTTO_FOREACH_INDEX}` - zero-based index of this item in the expansion

### Architecture

**Processing Pipeline:**

```
┌─────────────────┐     ┌──────────────────┐     ┌──────────────────┐     ┌─────────────────┐
│   otto.yml      │────▶│   YAML Parser    │────▶│   Foreach        │────▶│   Scheduler     │
│                 │     │   (serde)        │     │   Expander       │     │                 │
│ foreach:        │     │                  │     │   (cli/parser)   │     │ Task execution  │
│   glob: "*.sh"  │     │ TaskSpec with    │     │                  │     │ with semaphore  │
│                 │     │ foreach field    │     │ Resolves globs,  │     │                 │
└─────────────────┘     │ preserved        │     │ creates subtasks │     └─────────────────┘
                        └──────────────────┘     └──────────────────┘
```

**Key insight:** Expansion happens in `cli/parser.rs` during `process_tasks_with_filter()`, NOT during YAML deserialization. This is because:
1. Glob resolution requires `cwd` (the working directory)
2. The Parser already transforms `TaskSpec` → `Task`
3. Dependency resolution happens here anyway

**Modified processing flow:**

```rust
// In cli/parser.rs, inside process_tasks_with_filter()

// Step 1: Expand all foreach tasks FIRST
let mut expanded_task_specs: HashMap<String, TaskSpec> = HashMap::new();
for (name, spec) in &self.config_spec.tasks {
    if spec.foreach.is_some() {
        // Expand to N subtasks
        let subtasks = spec.expand_foreach(&self.cwd)?;
        for subtask in subtasks {
            expanded_task_specs.insert(subtask.name.clone(), subtask);
        }
        // Also keep a virtual parent task for dependency resolution
        let parent = spec.as_virtual_parent();
        expanded_task_specs.insert(name.clone(), parent);
    } else {
        expanded_task_specs.insert(name.clone(), spec.clone());
    }
}

// Step 2: Compute dependencies using expanded task set
// Step 3: Convert to Task objects as before
```

**Parent-child tracking:**

Add a `parent` field to `Task`:

```rust
pub struct Task {
    pub name: String,
    pub parent: Option<String>,  // NEW: e.g., Some("examples") for "examples:01-basic.sh"
    // ... existing fields
}
```

This enables:
- TUI grouping: Tasks with same parent are displayed together
- History queries: `SELECT * FROM tasks WHERE parent_task = 'examples'`
- Aggregate status: Parent is "completed" when all children complete

**Database schema migration:**

```sql
-- Add parent_task column to task_runs table
ALTER TABLE task_runs ADD COLUMN parent_task TEXT;
CREATE INDEX idx_task_runs_parent ON task_runs(parent_task);
```

**Scheduler changes:**

The scheduler itself needs minimal changes—it already handles arbitrary task graphs. The key additions:

1. **Virtual parent completion:** When all subtasks of a foreach complete, mark the virtual parent as complete
2. **TUI aggregation:** Send `TaskMessage::GroupProgress` messages for foreach parents
3. **Fail tracking:** Track which subtasks failed for summary reporting

### Data Model

**New struct in `cfg/task.rs`:**

```rust
#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize, Serialize)]
pub struct ForeachSpec {
    /// Glob pattern to match files
    #[serde(default)]
    pub glob: Option<String>,

    /// Explicit list of items
    #[serde(default)]
    pub items: Vec<String>,

    /// Numeric range (e.g., "1-10" for 1 through 10 inclusive)
    #[serde(default)]
    pub range: Option<String>,

    /// Variable name for the current item (default: "item")
    #[serde(default = "default_as")]
    pub r#as: String,

    /// Whether subtasks run in parallel (default: true)
    #[serde(default = "default_parallel")]
    pub parallel: bool,

    /// Maximum number of items before erroring (default: 1000)
    #[serde(default = "default_max_items")]
    pub max_items: usize,
}

fn default_as() -> String { "item".to_string() }
fn default_parallel() -> bool { true }
fn default_max_items() -> usize { 1000 }

/// Represents a single item from foreach expansion
#[derive(Clone, Debug)]
pub struct ForeachItem {
    /// The identifier used in subtask naming (e.g., "01-basic.sh")
    pub identifier: String,
    /// The full value passed to the script (e.g., "examples/01-basic.sh")
    pub value: String,
}
```

**Modified `TaskSpec` (cfg/task.rs):**

```rust
pub struct TaskSpec {
    pub name: String,
    pub help: Option<String>,
    pub after: Vec<String>,
    pub before: Vec<String>,
    pub input: Vec<String>,
    pub output: Vec<String>,
    pub envs: HashMap<String, String>,
    pub params: ParamSpecs,
    pub action: String,
    pub foreach: Option<ForeachSpec>,  // NEW
}

impl TaskSpec {
    /// Create a virtual parent task (no action, just for dependency tracking)
    pub fn as_virtual_parent(&self) -> TaskSpec {
        TaskSpec {
            name: self.name.clone(),
            help: self.help.clone(),
            after: self.after.clone(),
            before: self.before.clone(),
            input: vec![],
            output: vec![],
            envs: HashMap::new(),
            params: ParamSpecs::new(),
            action: String::new(),  // No action - virtual task
            foreach: None,
        }
    }
}
```

**Modified `Task` (cli/parser.rs):**

```rust
pub struct Task {
    pub name: String,
    pub parent: Option<String>,  // NEW: foreach parent task name
    pub task_deps: Vec<String>,
    pub file_deps: Vec<String>,
    pub output_deps: Vec<String>,
    pub envs: HashMap<String, String>,
    pub values: HashMap<String, Value>,
    pub action: String,
    pub hash: String,
}
```

### API Design

**Expansion function in `cfg/task.rs`:**

```rust
impl TaskSpec {
    /// Expand a foreach task into multiple concrete subtasks
    pub fn expand_foreach(&self, cwd: &Path) -> Result<Vec<TaskSpec>> {
        let foreach = match &self.foreach {
            Some(f) => f,
            None => return Ok(vec![self.clone()]),
        };

        let items = foreach.resolve_items(cwd)?;

        items.iter().enumerate().map(|(index, item)| {
            let mut subtask = self.clone();
            subtask.name = format!("{}:{}", self.name, item.identifier);
            subtask.foreach = None;  // Prevent recursive expansion

            // Inject foreach variables into environment
            subtask.envs.insert(foreach.r#as.clone(), item.value.clone());
            subtask.envs.insert("OTTO_FOREACH_ITEM".to_string(), item.value.clone());
            subtask.envs.insert("OTTO_FOREACH_INDEX".to_string(), index.to_string());

            Ok(subtask)
        }).collect()
    }
}

impl ForeachSpec {
    pub fn resolve_items(&self, cwd: &Path) -> Result<Vec<ForeachItem>> {
        if let Some(glob_pattern) = &self.glob {
            self.resolve_glob(glob_pattern, cwd)
        } else if !self.items.is_empty() {
            self.resolve_list()
        } else if let Some(range) = &self.range {
            self.resolve_range(range)
        } else {
            Err(eyre!("foreach requires glob, items, or range"))
        }
    }
}
```

### Edge Case Behaviors

**Zero matches:**
When a glob pattern matches no files, the task expands to zero subtasks. This is treated as a successful no-op (not an error). A warning is logged: `"foreach glob 'examples/*.sh' matched 0 files"`.

**Serial execution (`parallel: false`):**
When `parallel: false`, subtasks are chained with implicit `before` dependencies:
- `examples:01-basic.sh` has no dependencies
- `examples:02-search.sh` depends on `examples:01-basic.sh`
- `examples:03-context.sh` depends on `examples:02-search.sh`
- etc.

This preserves execution order while still creating individual subtasks with separate logs/status.

**Duplicate item identifiers:**
If two glob matches produce the same identifier (e.g., `a/test.sh` and `b/test.sh` both become `test.sh`), an error is raised: `"foreach produced duplicate subtask name 'examples:test.sh'"`. Users must use more specific globs or rename files.

**Failure behavior:**
When a subtask fails:
- Other running subtasks continue to completion (fail-fast is not the default)
- Pending subtasks are still started (unless `--fail-fast` flag is added later)
- The parent task is marked "failed" once all subtasks complete, with summary: `"3/10 subtasks failed"`
- Exit code is 1 if any subtask failed

**Resource limits:**
A configurable `max_items` field (default: 1000) prevents runaway expansion:
```yaml
foreach:
  glob: "**/*.txt"
  max_items: 100  # Error if glob matches > 100 files
```
If exceeded: `"foreach glob matched 5432 files, exceeding max_items (1000)"`

**Params inheritance:**
Task `params:` are inherited by all subtasks. Users can override params for specific subtasks:
```bash
otto examples --verbose                    # All subtasks get --verbose
otto examples:01-basic.sh --verbose        # Only this subtask gets --verbose
```

**Input/output per-item:**
The `input:` and `output:` fields can use the foreach variable:
```yaml
tasks:
  compile:
    foreach:
      glob: "src/*.c"
      as: source
    input: ["${source}"]
    output: ["build/${source%.c}.o"]
    bash: gcc -c "${source}" -o "build/${source%.c}.o"
```
Note: Variable expansion in `input:`/`output:` happens during subtask creation, not shell execution.

**Naming edge cases:**
- Colons in filenames: Escaped as `\:` in subtask name (`file:name.sh` → `examples:file\:name.sh`)
- Empty items: Skipped with warning `"foreach skipped empty item at index 2"`
- Whitespace: Preserved in value, replaced with `_` in identifier (`"my file.sh"` → `examples:my_file.sh`)
- Leading/trailing whitespace: Trimmed from identifier

**Direct subtask dependencies:**
Users can depend on specific subtasks:
```yaml
tasks:
  deploy:
    before: [examples:01-basic.sh]  # Only waits for this specific subtask
```
Or use wildcard (future consideration):
```yaml
    before: [examples:*]  # Same as [examples] - waits for all subtasks
```

### TUI Integration

Subtasks appear in the TUI as indented children of the parent task group:

```
examples                      [3/10 running]
  examples:01-basic.sh        completed  0.8s
  examples:02-search.sh       running    1.2s
  examples:03-context.sh      running    0.9s
  examples:04-filter.sh       pending
  ...
```

The parent row shows aggregate progress. Clicking/selecting the parent expands/collapses the subtask list.

### Graph Visualization

In `otto Graph` output, foreach tasks display as a cluster:

```
┌─────────────────────────────┐
│ examples (foreach)          │
│ ┌─────────┐ ┌─────────┐     │
│ │ :01-... │ │ :02-... │ ... │
│ └─────────┘ └─────────┘     │
└─────────────────────────────┘
         │
         ▼
    [downstream tasks]
```

The `--format=dot` output uses subgraph clusters for foreach groups.

### History and Database

Each subtask is recorded as a separate task execution in the database:
- `task_name`: `examples:01-basic.sh` (full subtask name)
- `parent_task`: `examples` (new column for grouping)
- Standard columns: `status`, `duration_ms`, `stdout_path`, `stderr_path`

The `otto History` command shows subtasks grouped under their parent:
```
Run #42 (2026-01-10 14:30:00)
  examples                    [10/10 completed]  12.3s total
    examples:01-basic.sh      completed          0.8s
    examples:02-search.sh     completed          1.2s
    ...
```

### Implementation Plan

**Phase 1: Core foreach expansion**
1. Add `ForeachSpec` struct to `cfg/task.rs`
2. Add `foreach` field to `TaskSpec` and `TaskSpecHelper`
3. Implement `ForeachSpec::resolve_items()` for glob patterns
4. Implement `TaskSpec::expand_foreach()`
5. Integrate expansion into `deserialize_task_map()`

**Phase 2: Variable injection and edge cases**
1. Inject foreach variables into task environment (`as`, `OTTO_FOREACH_ITEM`, `OTTO_FOREACH_INDEX`)
2. Handle special characters in filenames (spaces, quotes) - proper shell escaping in envs
3. Validate variable names don't conflict with existing task params

**Phase 3: Dependency handling**
1. Subtasks inherit parent's `before` dependencies (each subtask waits for parent's deps)
2. Parent task becomes a virtual "group" task with no action of its own
3. Running `otto examples` runs all subtasks; the parent completes when all subtasks complete
4. If another task has `before: [examples]`, it waits for ALL `examples:*` subtasks
5. The parent's `after` field still works: those tasks auto-run after all subtasks finish

**Phase 4: CLI integration**
1. Update `--help` to show foreach tasks with `[N items]` indicator
2. Allow running specific subtasks: `otto examples:01-basic.sh`
3. Add `--list-subtasks` flag to show expanded tasks

**Phase 5: Additional iteration sources**
1. Implement `items:` explicit list support
2. Implement `range:` numeric range support
3. Add validation and error messages

### Worked Example: Engram Examples Task

**Before (serial, ~2 minutes):**

```yaml
# ~/repos/neuraphage/engram/.otto.yml
tasks:
  examples:
    help: "Run all CLI usage examples"
    bash: |
      for example in examples/*.sh; do
        name=$(basename "$example")
        echo "--- Running $name ---"
        if bash "$example"; then
          echo "passed"
        else
          failed=$((failed + 1))
        fi
      done
```

**After (parallel, ~12 seconds):**

```yaml
tasks:
  examples:
    help: "Run all CLI usage examples"
    foreach:
      glob: "examples/*.sh"
      as: example
    bash: |
      echo "--- Running ${example} ---"
      bash "${example}"
```

**What otto does internally:**

Given files: `examples/01-basic.sh`, `examples/02-search.sh`, `examples/03-context.sh`

1. Parse YAML, preserve `foreach` field in TaskSpec
2. During `process_tasks_with_filter()`, call `examples.expand_foreach(cwd)`
3. Generate three TaskSpecs:
   ```
   examples:01-basic.sh  { envs: {example: "examples/01-basic.sh", OTTO_FOREACH_INDEX: "0"} }
   examples:02-search.sh { envs: {example: "examples/02-search.sh", OTTO_FOREACH_INDEX: "1"} }
   examples:03-context.sh { envs: {example: "examples/03-context.sh", OTTO_FOREACH_INDEX: "2"} }
   ```
4. Create virtual parent `examples` with no action
5. All subtasks have `parent: Some("examples")`
6. Scheduler runs subtasks in parallel (up to `-j` limit)
7. When all subtasks complete, virtual parent marked complete

**CLI behavior:**

```bash
$ otto examples              # Runs all 3 subtasks in parallel
$ otto examples:01-basic.sh  # Runs only this specific subtask
$ otto --list-subtasks       # Shows expanded task list
```

### Files to Modify

| File | Changes |
|------|---------|
| `src/cfg/task.rs` | Add `ForeachSpec`, `ForeachItem` structs; add `foreach` field to `TaskSpec`; implement `expand_foreach()` |
| `src/cli/parser.rs` | Add `parent` field to `Task`; integrate expansion into `process_tasks_with_filter()` |
| `src/executor/scheduler.rs` | Track virtual parents; aggregate subtask status; send group progress messages |
| `src/executor/state/schema.rs` | Add `parent_task` column to task_runs table |
| `src/cli/builtins/graph.rs` | Render foreach tasks as clusters |
| TUI components | Add collapsible subtask grouping |

## Alternatives Considered

### Alternative 1: Runtime expansion via shell

**Description:** Keep everything in bash, use `xargs -P` or GNU parallel for parallelism.

```yaml
examples:
  bash: |
    find examples -name "*.sh" | xargs -P 8 -I {} bash {}
```

**Pros:**
- No otto changes required
- Familiar to shell users

**Cons:**
- Subtasks aren't first-class (no individual status, logs, or retry)
- Can't leverage otto's TUI, history, or dependency system
- Error handling is manual and error-prone

**Why not chosen:** Defeats the purpose of using otto for task orchestration.

### Alternative 2: External task generator

**Description:** Use a preprocessor to generate expanded YAML before otto runs.

```bash
./generate-otto.py > .otto.generated.yml
otto -o .otto.generated.yml examples
```

**Pros:**
- Full Python/script power for generation
- No otto changes required

**Cons:**
- Extra build step users must remember
- Generated file can get out of sync
- Breaks otto's single-file simplicity

**Why not chosen:** Adds tooling complexity and breaks the declarative model.

### Alternative 3: Matrix syntax (GitHub Actions style)

**Description:** Use a matrix to define combinations of values.

```yaml
examples:
  matrix:
    example: [01-basic.sh, 02-search.sh, 03-context.sh]
  bash: |
    bash examples/${{ matrix.example }}
```

**Pros:**
- Familiar to GitHub Actions users
- Supports multi-dimensional matrices

**Cons:**
- Requires explicit enumeration (no glob support)
- More verbose for simple cases
- Different variable syntax from otto conventions

**Why not chosen:** Glob support is essential; matrix is overkill for single-dimension iteration.

## Technical Considerations

### Dependencies

- `glob` crate: Already used in otto for file pattern matching
- No new external dependencies required

### Performance

- Expansion happens once at config load time (not per-execution)
- Glob resolution is synchronous but fast for typical directory sizes
- Subtask count is bounded by filesystem (practical limit ~1000s)
- Scheduler semaphore prevents resource exhaustion

### Security

- Glob patterns are resolved relative to cwd (no path traversal)
- Variable substitution uses simple string replacement (no shell expansion)
- Filenames with special characters are properly escaped in envs

### Testing Strategy

1. **Unit tests** for `ForeachSpec::resolve_items()`
   - Glob patterns with various wildcards
   - Empty matches (should produce zero subtasks)
   - Invalid patterns (should error gracefully)

2. **Unit tests** for `TaskSpec::expand_foreach()`
   - Subtask naming conventions
   - Variable injection
   - Dependency inheritance

3. **Integration tests**
   - End-to-end foreach expansion and execution
   - Parallel execution verification
   - Error propagation from failed subtasks

4. **Manual testing**
   - Apply to engram examples task
   - Verify ~10x speedup (2min -> ~12sec with 10 parallel)

### Rollout Plan

1. Implement behind feature flag (`--enable-foreach` or env var)
2. Document syntax and migration guide
3. Test with engram examples as first real-world case
4. Remove feature flag, make generally available
5. Update otto help and README

## Risks and Mitigations

| Risk | Likelihood | Impact | Mitigation |
|------|------------|--------|------------|
| Glob matches thousands of files | Low | High | Configurable `max_items` limit (default 1000) |
| Variable name conflicts with params | Medium | Medium | Validate at expansion time; error on collision |
| Subtask naming collision | Low | Low | Sanitize identifiers; error on duplicate names |
| Performance regression in parser | Low | Low | Single-pass expansion; alphabetical sort is O(n log n) |
| User confusion about parent vs subtask | Medium | Medium | Clear help text; `--list-subtasks` command |
| Cascading failures overwhelm logs | Medium | Medium | Aggregate failure summary; TUI collapse by default |
| Special characters in filenames | Low | Medium | Escape colons; replace whitespace in identifiers |
| Env var injection attacks | Low | High | Values passed through env vars, not shell interpolation |

## Open Questions

- [x] Should parent task (`examples`) be runnable, or only subtasks (`examples:*`)?
  - **Decision:** Running `otto examples` runs all subtasks. The parent is a virtual task.
- [x] How to handle `before: [examples]` - wait for all subtasks or just the parent?
  - **Decision:** Waits for ALL subtasks to complete (the virtual parent gates downstream).
- [ ] Should foreach support filtering (e.g., `exclude: ["*.bak"]`)?
  - **Deferred:** Start without filtering; users can use more specific glob patterns.
- [x] What's the maximum sensible subtask count before warning?
  - **Decision:** Default `max_items: 1000`; configurable per-task.

## References

- [PyDoit Task Creation](https://pydoit.org/task-creation.html) - Generator-based dynamic task creation
- [Engram .otto.yml](~/repos/neuraphage/engram/.otto.yml) - Current serial examples task
- [Otto scheduler.rs](src/executor/scheduler.rs) - Existing parallel execution infrastructure
- [GitHub Actions Matrix](https://docs.github.com/en/actions/using-jobs/using-a-matrix-for-your-jobs) - Similar feature in CI/CD
