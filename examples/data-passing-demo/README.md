# Example 14: Data Passing Between Bash and Python

This example demonstrates Otto's data passing capabilities using `otto_set_output` and `otto_get_input` in both **Bash** and **Python** tasks.

## What This Demonstrates

1. ✅ **Bash → Bash** data passing
2. ✅ **Python → Python** data passing
3. ✅ **Bash → Python** cross-language data flow
4. ✅ **Python → Bash** cross-language data flow
5. ✅ Simple values (strings, numbers)
6. ✅ Complex values (JSON objects, arrays)
7. ✅ Multiple dependencies in a single task

## Task Flow

```
bash_producer (bash)
    ↓
bash_consumer (bash) ─────┐
    ↓                     ↓
bash_to_python (python)   |
    ↓                     |
                          |
python_producer (python)  |
    ↓                     |
python_consumer (python) ─┤
    ↓                     |
python_to_bash (bash) ────┘
    ↓
final_report (bash)
```

## Running the Example

### Run All Tasks
```bash
otto final_report
```

This will run all tasks in dependency order.

### Run Individual Tasks

```bash
# Bash producer and consumer
otto bash_producer
otto bash_consumer

# Python producer and consumer
otto python_producer
otto python_consumer

# Cross-language examples
otto bash_to_python
otto python_to_bash

# Final report
otto final_report
```

## How It Works

### Setting Output (Bash)

```bash
# In any bash task
otto_set_output "key" "value"
otto_set_output "count" "42"
otto_set_output "status" "success"
```

### Getting Input (Bash)

```bash
# In a dependent bash task
value=$(otto_get_input "task_name.key")
count=$(otto_get_input "task_name.count")
status=$(otto_get_input "task_name.status")
```

### Setting Output (Python)

```yaml
tasks:
  my_task:
    python: |
      import json

      # Simple values
      otto_set_output("key", "value")
      otto_set_output("count", "42")

      # Complex values - serialize to JSON
      data = {"nested": "object"}
      otto_set_output("data", json.dumps(data))

      tags = ["tag1", "tag2"]
      otto_set_output("tags", json.dumps(tags))
```

### Getting Input (Python)

```yaml
tasks:
  dependent_task:
    before: [my_task]
    python: |
      import json

      # Simple values
      value = otto_get_input("task_name.key")
      count = otto_get_input("task_name.count")

      # Complex values - deserialize from JSON
      data_json = otto_get_input("task_name.data")
      data = json.loads(data_json) if data_json else {}

      tags_json = otto_get_input("task_name.tags")
      tags = json.loads(tags_json) if tags_json else []
```

## Behind the Scenes

When you call `otto_set_output`, Otto:
1. Stores the key-value pair in an array/dict
2. At task completion, serializes to JSON file: `output.<task_name>.json`

When you call `otto_get_input`, Otto:
1. Reads from symlinked input file: `input.<dependency>.json`
2. Extracts the requested key
3. Returns the value

### File Locations

After running, you can inspect the data files:

```bash
# Find your run directory
cd ~/.otto/ex14-*/*/tasks/

# View a task's output
cat bash_producer/output.bash_producer.json

# View a task's input (symlink to dependency)
cat bash_consumer/input.bash_producer.json

# They point to the same data!
ls -la bash_consumer/input.bash_producer.json
```

Example output file:
```json
{
  "timestamp": "2025-12-08_14:30:45",
  "random_number": "742",
  "status": "success",
  "message": "Data generated from bash"
}
```

## Key Learnings

### 1. Manual Deserialization (Not Shown Here)

The tasks in this example don't manually call `otto_deserialize_input` because the builtins handle it. But you can call it explicitly if needed:

```bash
# Manually load a dependency
otto_deserialize_input "task_name"
value=$(otto_get_input "task_name.key")
```

### 2. Complex Data Types

For complex data (objects, arrays), always serialize to JSON:

```python
# ✅ Good
data = {"key": "value"}
otto_set_output("data", json.dumps(data))

# ❌ Bad - will stringify incorrectly
otto_set_output("data", str(data))  # Don't do this!
```

### 3. Cross-Language Compatibility

- Bash outputs are always strings
- Python can output any JSON-serializable type
- Always serialize complex types to JSON strings
- Use `jq` in bash to parse JSON

### 4. Naming Convention

By convention, use `<task_name>.<key>` when accessing data:

```bash
# Clear and explicit
value=$(otto_get_input "bash_producer.timestamp")

# Not recommended (won't work without task prefix)
# value=$(otto_get_input "timestamp")
```

## Debugging

### View Task Outputs

```bash
# Find the run directory
cd ~/.otto/ex14-*/latest/tasks

# View what each task produced
cat bash_producer/output.bash_producer.json
cat python_producer/output.python_producer.json

# Check task logs
cat bash_producer/stdout.log
cat python_producer/stdout.log
```

### Check Symlinks

```bash
# See how dependencies are linked
ls -la bash_consumer/input.*.json
ls -la final_report/input.*.json

# The symlinks show data flow!
```

## Common Issues

### Issue: `otto_get_input` reports `no input '<key>'`

Otto prints `otto: no input '<key>'; available: <the keys that are there>` and
returns non-zero. It used to return empty at exit 0, which said nothing; the
diagnostic replaced that, so read the `available:` list first - it usually names
the key you meant, with different spelling.

Note the non-zero status only fails the task in the form
`x=$(otto_get_input k)` under `set -e`. In `local x=$(...)` and
`echo "$(...)"`, bash discards it. The printed message is the reliable signal.
See `docs/commands/ottofile-reference.md` for the full table.

**Cause:** Task dependency not declared in `before:`

**Solution:**
```yaml
my_task:
  before: [dependency_task]  # ← Make sure this is set!
  bash: |
    value=$(otto_get_input "dependency_task.key")
```

### Issue: JSON parsing fails in bash

**Cause:** Complex data not properly serialized

**Solution:** Use `jq` to parse:
```bash
json_data=$(otto_get_input "python_task.data")
value=$(echo "$json_data" | jq -r '.key')
```

### Issue: Python can't deserialize data

**Cause:** Forgot to serialize when setting output

**Solution:**
```python
# When setting
import json
otto_set_output("data", json.dumps({"key": "value"}))

# When getting
data_json = otto_get_input("task.data")
data = json.loads(data_json) if data_json else {}
```

## Next Steps

- Try modifying tasks to pass different data types
- Add more tasks to the dependency chain
- Experiment with error handling (what if a key doesn't exist?)
- Look at `examples/ex11/` for a simpler bash-only example

## Related Examples

- **ex11** - Simpler bash-only data passing
- **ex10** - Environment variables (different approach)
- **ex8** - File dependencies (not task data)
