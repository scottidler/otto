# Data Passing Between Bash and Python

This example demonstrates otto's data passing capabilities using
`otto_set_output`, `otto_deserialize_input`, and `otto_get_input`, in both
**Bash** and **Python** tasks, chained across a dependency graph.

## What This Demonstrates

1. Bash → Bash data passing (`task_a` → `task_b`)
2. Bash → Python cross-language data flow (`task_b` → `task_c`)
3. A consumer three hops downstream that only declares its direct
   dependency, and therefore can only read that dependency's outputs
   (`report`, which declares `before: [task_c]` and cannot see `task_a`
   or `task_b` directly)
4. Simple string values, and doing arithmetic on a value read back as a
   string

## Task Flow

```
task_a (bash, producer)
    |
    v
task_b (bash, consumer of task_a, producer for task_c)
    |
    v
task_c (python, consumer of task_b, producer for report)
    |
    v
report (bash, consumer of task_c; validates the whole pipeline)
```

## Running the Example

```bash
otto -o examples/data-passing-demo report   # runs the whole chain
otto -o examples/data-passing-demo task_a   # or run any task by name
```

## How It Works

### Setting output (Bash)

```bash
otto_set_output "message" "hello"
otto_set_output "number" "42"
```

### Reading input (Bash)

A dependency's output has to be deserialized before `otto_get_input` can see
it - the generated prologue does this automatically for every task listed in
`before:`, so a task body reading its own declared dependencies (the common
case) never calls `otto_deserialize_input` itself:

```bash
received_message=$(otto_get_input "task_a.message")
received_number=$(otto_get_input "task_a.number")
```

This example's `task_b` and `task_c` call `otto_deserialize_input` explicitly
anyway, to show the mechanism the generated prologue runs on your behalf.

### Setting output (Python)

```python
otto_set_output("tripled", str(tripled))
```

### Reading input (Python)

```python
otto_deserialize_input("task_b")
doubled = otto_get_input("task_b.doubled")
```

## Key Learnings

### 1. A dependency is only visible to a task that declares it

`report` declares `before: [task_c]` only, so it can read `task_c.tripled`
but has no way to read `task_a.number` or `task_b.doubled` directly - it
only knows about them because `task_c` already read and re-derived them.
There is no ambient sharing across the whole run.

### 2. Complex data types

For anything beyond a plain string, serialize to JSON on the way out and
parse it on the way in:

```python
import json
otto_set_output("data", json.dumps({"key": "value"}))
```

```bash
json_data=$(otto_get_input "producer.data")
value=$(echo "$json_data" | jq -r '.key')
```

### 3. Naming convention

Values are always addressed as `<task_name>.<key>`:

```bash
value=$(otto_get_input "task_a.message")
```

## Common Issues

### `otto_get_input` reports `no input '<key>'`

otto prints `otto: no input '<key>'; available: <the keys that are there>`
and returns non-zero. Read the `available:` list first - it usually names
the key you meant, with different spelling. See
[`docs/commands/ottofile-reference.md`](../../docs/commands/ottofile-reference.md#passing-data-between-tasks-otto_set_output-and-otto_get_input)
for the full behavior, including which shell shapes actually see the
non-zero exit status.

**Cause:** the producing task isn't in this task's `before:` list.

**Solution:**
```yaml
my_task:
  before: [dependency_task]
  bash: |
    value=$(otto_get_input "dependency_task.key")
```

## Inspecting the run

```bash
# Find the run directory (see docs/directory-layout.md)
ls ~/.otto/data-passing-demo-*/

# View what a task produced
cat ~/.otto/data-passing-demo-*/<run-timestamp>/tasks/task_a/output.task_a.json

# View a task's copy of its dependency's output
cat ~/.otto/data-passing-demo-*/<run-timestamp>/tasks/task_b/input.task_a.json
```

## Related Examples

- [`examples/file-dependencies`](../file-dependencies) - file-based dependencies, not task-to-task data
- [`examples/environment-variables`](../environment-variables) - the `envs:` approach instead of `otto_set_output`/`otto_get_input`
