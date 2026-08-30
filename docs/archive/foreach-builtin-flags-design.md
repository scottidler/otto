# Foreach Builtin Flags Design

## Problem Statement

Two usability issues with foreach tasks:

1. **Graph noise**: Running `otto Graph` in a project with foreach tasks shows every expanded subtask (e.g., `examples:01.rs`, `examples:02.rs`, ... `examples:08.rs`), making the graph hard to read. Foreach subtasks share the same dependency structure as their parent - they add noise without adding dependency information.

2. **Serial execution**: Users sometimes need to run foreach subtasks sequentially instead of in parallel. Currently this requires defining a separate task (`examples-serial`) with `parallel: false`, duplicating the foreach configuration.

## Decisions

### Decision 1: Collapsed Graph View

**Default behavior**: Show collapsed foreach tasks with glob pattern and count:
```
├─ examples:scripts/*.sh [8]
```

**Expanded behavior**: With `--Expand` flag, show all subtasks:
```
├─ examples:01-setup.sh
├─ examples:02-build.sh
├─ examples:03-test.sh
...
```

### Decision 2: Builtin `--Serial` Flag

Auto-inject `--Serial` flag on tasks with foreach configuration:

```
$ otto examples --help

Usage: otto examples [OPTIONS]

Run examples in parallel [8 items]

Options:
  --Serial     [builtin] Run subtasks sequentially
  --verbose    Show detailed output
  -h, --help   Print help
```

**Convention**: Capitalized flags indicate otto builtins (consistent with capitalized subcommands like `Graph`, `Clean`, etc.)

**Conflict handling**: Error at parse time if user defines a `--Serial` param in yaml.

## Technical Design

### Graph Changes

**File**: `src/executor/graph.rs`

#### GraphOptions Extension

```rust
#[derive(Debug, Clone)]
pub struct GraphOptions {
    pub show_details: bool,
    pub show_file_deps: bool,
    pub format: GraphFormat,
    pub style: NodeStyle,
    pub output_path: Option<std::path::PathBuf>,
    pub expand_foreach: bool,  // NEW - default: false
}

impl Default for GraphOptions {
    fn default() -> Self {
        Self {
            show_details: true,
            show_file_deps: true,
            format: GraphFormat::Svg,
            style: NodeStyle::Detailed,
            output_path: None,
            expand_foreach: false,  // Collapsed by default
        }
    }
}
```

#### execute_command Update

```rust
pub async fn execute_command(task: &crate::cli::parser::Task) -> Result<()> {
    let expand = task
        .values
        .get("Expand")  // Capitalized builtin flag
        .and_then(|v| match v {
            crate::cfg::config::Value::Item(s) => Some(s == "true"),
            _ => None,
        })
        .unwrap_or(false);

    let options = GraphOptions {
        expand_foreach: expand,
        // ... other options
    };
    // ...
}
```

#### Collapsed View Logic

When `expand_foreach` is false:
1. Identify foreach parent tasks (tasks where subtasks exist with `{parent}:{item}` naming)
2. Group subtasks by parent
3. Display single node with original glob pattern and count

```rust
fn collapse_foreach_tasks(dag: &DAG<Task>) -> CollapsedDAG {
    let mut parents: HashMap<String, ForeachInfo> = HashMap::new();

    for node in dag.raw_nodes() {
        let task = &node.weight;
        if let Some((parent, _item)) = task.name.split_once(':') {
            parents.entry(parent.to_string())
                .or_insert_with(|| ForeachInfo::new(parent))
                .add_subtask(&task.name);
        }
    }

    // Build collapsed DAG with parent nodes only
    // ...
}
```

### Parser Changes

**File**: `src/cli/parser.rs`

#### Reserved Builtin Params

```rust
/// Builtin params that are auto-injected (capitalized)
pub const BUILTIN_PARAMS: &[&str] = &["Serial", "Expand"];

/// Check if a param name is reserved for builtins
pub fn is_builtin_param(name: &str) -> bool {
    BUILTIN_PARAMS.contains(&name)
}
```

#### Param Validation

In `parse_task_params` or equivalent:

```rust
fn validate_user_params(&self, task_spec: &TaskSpec) -> Result<()> {
    for param_name in task_spec.params.keys() {
        if is_builtin_param(param_name) {
            return Err(eyre!(
                "Task '{}' defines reserved builtin param '--{}'. \
                 Capitalized params are reserved for otto builtins.",
                task_spec.name,
                param_name
            ));
        }
    }
    Ok(())
}
```

#### Auto-inject Serial Param

When building clap Command for a foreach task:

```rust
fn task_to_command(task_spec: &TaskSpec) -> Command {
    let mut cmd = Command::new(&task_spec.name);

    // Add user-defined params
    for (_, param) in &task_spec.params {
        cmd = cmd.arg(param_to_arg(param));
    }

    // Auto-inject --Serial for foreach tasks
    if task_spec.has_foreach() {
        cmd = cmd.arg(
            Arg::new("Serial")
                .long("Serial")
                .help("[builtin] Run subtasks sequentially")
                .action(ArgAction::SetTrue)
        );
    }

    cmd
}
```

### Execution Changes

**File**: `src/executor/scheduler.rs` (or equivalent)

When executing a foreach task, check for `--Serial` flag:

```rust
async fn execute_foreach_task(&self, task: &Task) -> Result<()> {
    let serial = task.values
        .get("Serial")
        .map(|v| matches!(v, Value::Item(s) if s == "true"))
        .unwrap_or(false);

    if serial {
        // Override parallel setting, run sequentially
        self.execute_subtasks_sequential(&task.subtasks).await
    } else {
        // Use yaml-defined parallelism
        self.execute_subtasks_parallel(&task.subtasks).await
    }
}
```

## Implementation Plan

### Phase 1: Builtin Params Infrastructure

1. Add `BUILTIN_PARAMS` constant to `src/cli/builtins.rs`
2. Add `is_builtin_param()` helper function
3. Add validation in parser to error on reserved param names
4. Add tests for reserved param detection

**Files**: `src/cli/builtins.rs`, `src/cli/parser.rs`

### Phase 2: Graph Collapse

1. Add `expand_foreach` to `GraphOptions`
2. Add `--Expand` flag parsing in `execute_command`
3. Implement `collapse_foreach_tasks()` logic
4. Update `generate_ascii()` to show collapsed view
5. Update `generate_dot()` to show collapsed view
6. Add tests for collapsed/expanded graph output

**Files**: `src/executor/graph.rs`

### Phase 3: Serial Flag

1. Auto-inject `--Serial` param on foreach tasks in `task_to_command()`
2. Parse `--Serial` flag value into task
3. Update execution to respect `--Serial` override
4. Add tests for serial execution

**Files**: `src/cli/parser.rs`, `src/executor/scheduler.rs`

### Phase 4: Documentation

1. Update graph command docs with `--Expand` flag
2. Document `--Serial` flag in foreach documentation
3. Add examples showing both features

**Files**: `docs/commands/graph.md`, `docs/foreach-subtasks.md`

## Help Output Examples

### Graph Command

```
$ otto Graph --help

Usage: otto Graph [OPTIONS]

[builtin] Visualize the task dependency graph

Options:
  -f, --format <FORMAT>  Output format: ascii, dot, svg, png, pdf [default: ascii]
      --output <FILE>    Output file path
      --Expand           [builtin] Show all foreach subtasks (default: collapsed)
  -h, --help             Print help
```

### Foreach Task

```
$ otto examples --help

Usage: otto examples [OPTIONS]

Run all example scripts [8 items]

Options:
      --Serial     [builtin] Run subtasks sequentially
      --verbose    Show verbose output
  -h, --help       Print help
```

## Edge Cases

### Nested Foreach

If a foreach subtask itself has foreach (not currently supported), the Serial flag applies only to the immediate subtasks.

### Empty Foreach

If foreach resolves to 0 items, `--Serial` has no effect (nothing to serialize).

### Graph with Mixed Tasks

Graph shows:
- Regular tasks: normal nodes
- Foreach tasks (collapsed): `taskname:pattern [N]`
- Foreach tasks (expanded): all subtask nodes

## Testing Strategy

### Unit Tests

```rust
#[test]
fn test_builtin_params_are_reserved() {
    assert!(is_builtin_param("Serial"));
    assert!(is_builtin_param("Expand"));
    assert!(!is_builtin_param("serial"));  // lowercase ok
    assert!(!is_builtin_param("verbose")); // user param ok
}

#[test]
fn test_error_on_reserved_param() {
    let yaml = r#"
tasks:
  test:
    params:
      --Serial:
        help: "My serial flag"
    bash: echo test
"#;
    let result = Parser::from_yaml(yaml);
    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("reserved builtin param"));
}
```

### Integration Tests

```rust
#[test]
fn test_graph_collapsed_by_default() {
    // Setup foreach task
    // Run `otto Graph`
    // Assert output contains `taskname:*.sh [N]` not individual subtasks
}

#[test]
fn test_graph_expanded_with_flag() {
    // Setup foreach task
    // Run `otto Graph --Expand`
    // Assert output contains individual subtasks
}

#[test]
fn test_serial_flag_on_foreach_task() {
    // Setup foreach task
    // Run `otto taskname --Serial`
    // Assert subtasks run sequentially
}
```

## Success Criteria

1. `otto Graph` shows collapsed foreach tasks by default
2. `otto Graph --Expand` shows all subtasks
3. Foreach tasks have `--Serial` flag in help
4. `--Serial` causes sequential execution
5. Defining `--Serial` in yaml errors at parse time
6. All tests pass
7. Documentation updated
