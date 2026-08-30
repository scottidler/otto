# Improved Data Passing API - Implementation Proposal

## Overview

This document proposes concrete improvements to Otto's data passing API, focusing on better ergonomics while maintaining the proven file-based architecture.

## Current State Analysis

### Current User Experience Problems

**Problem 1: Magic Helper Functions**
```bash
# Current: Users must learn custom functions
otto_set_output "key" "value"
value=$(otto_get_input "task_a.key")
```

**Problem 2: Manual Dependency Loading**
```bash
# Current: Must explicitly deserialize each dependency
otto_deserialize_input "task_a"
otto_deserialize_input "task_b"
value=$(otto_get_input "task_a.key")
```

**Problem 3: Awkward Bash 3.2 Compatibility**
```bash
# Current implementation uses indexed arrays with string parsing
OTTO_OUTPUT+=("key=value")  # Not intuitive
```

**Problem 4: Hidden Magic**
```bash
# User doesn't see the prologue/epilogue
# Magic happens behind the scenes
# Hard to understand what's available
```

## Proposed Improvements

### Improvement 1: Native Data Structures

#### Bash (4.0+)

**Before:**
```bash
# Old way - awkward helper functions
otto_set_output "version" "1.2.3"
otto_set_output "status" "success"
otto_set_output "count" "42"

# Read from dependency
otto_deserialize_input "build"
version=$(otto_get_input "build.version")
```

**After:**
```bash
# New way - natural bash associative arrays
OUTPUT[version]="1.2.3"
OUTPUT[status]="success"
OUTPUT[count]=42

# Dependencies auto-loaded
echo "Build version: ${INPUT[build.version]}"
```

**Generated Prologue:**
```bash
#!/bin/bash
set -euo pipefail

# Otto-generated prologue
export OTTO_TASK_DIR="$(dirname "$0")"
export OTTO_TASK_NAME="deploy"

# Pre-load all dependencies into INPUT array
declare -A INPUT
# Auto-load from input.build.json
INPUT[build.version]="1.2.3"
INPUT[build.artifact]="/path/to/app"
INPUT[build.timestamp]="20250608"

# Create empty OUTPUT array for this task
declare -A OUTPUT

# ============ USER SCRIPT STARTS HERE ============
```

**Generated Epilogue:**
```bash
# ============ USER SCRIPT ENDS HERE ============

# Otto-generated epilogue: Serialize OUTPUT
{
    echo "{"
    first=true
    for key in "${!OUTPUT[@]}"; do
        if [ "$first" = true ]; then
            first=false
        else
            echo ","
        fi
        # Properly escape JSON
        printf '  "%s": "%s"' "$key" "$(echo "${OUTPUT[$key]}" | sed 's/"/\\"/g')"
    done
    echo ""
    echo "}"
} > "$OTTO_TASK_DIR/output.$OTTO_TASK_NAME.json"
```

#### Bash (3.2 - Legacy Compatibility)

For systems with old bash (macOS default), keep the current helper approach but improve it:

**After (Bash 3.2):**
```bash
# Fallback for Bash 3.2 - improved helper API
otto.set version "1.2.3"
otto.set status "success"
otto.set count 42

# Dependencies auto-loaded with improved getter
version=$(otto.get build.version)
artifact=$(otto.get build.artifact)
```

**Note:** Shorter names (`otto.set` vs `otto_set_output`), auto-loading still works.

#### Python

**Before:**
```python
# Old way - awkward __main__ access
import __main__

otto_deserialize_input("build")
build_version = __main__.OTTO_INPUT.get("build.version")

otto_set_output("version", "1.2.3")
# OR
__main__.OTTO_OUTPUT["version"] = "1.2.3"
```

**After:**
```python
# New way - clean module-level variables
# Dependencies auto-loaded
build_version = INPUT["build.version"]
artifact = INPUT["build.artifact"]

# Set outputs naturally
OUTPUT["version"] = "1.2.3"
OUTPUT["status"] = "success"
OUTPUT["count"] = 42
```

**Generated Prologue:**
```python
#!/usr/bin/env python3
"""
Otto-generated Python prologue
Task: deploy
"""
import json
import os
import sys

# Otto environment
os.environ["OTTO_TASK_DIR"] = os.path.dirname(os.path.abspath(__file__))
os.environ["OTTO_TASK_NAME"] = "deploy"

# Pre-load all dependencies
INPUT = {}

# Auto-load from input.build.json
try:
    with open(os.path.join(os.environ["OTTO_TASK_DIR"], "input.build.json")) as f:
        _build_data = json.load(f)
        for k, v in _build_data.items():
            INPUT[f"build.{k}"] = v
except (IOError, json.JSONDecodeError) as e:
    print(f"Warning: Could not load dependency 'build': {e}", file=sys.stderr)

# Create empty OUTPUT dict
OUTPUT = {}

# ============ USER SCRIPT STARTS HERE ============
```

**Generated Epilogue:**
```python
# ============ USER SCRIPT ENDS HERE ============

# Otto-generated epilogue: Serialize OUTPUT
try:
    _output_file = os.path.join(os.environ["OTTO_TASK_DIR"], f"output.{os.environ['OTTO_TASK_NAME']}.json")
    _temp_file = _output_file + ".tmp"

    with open(_temp_file, 'w') as f:
        json.dump(OUTPUT, f, indent=2)

    os.rename(_temp_file, _output_file)
except (IOError, OSError) as e:
    print(f"Error: Failed to serialize output: {e}", file=sys.stderr)
    sys.exit(1)
```

### Improvement 2: Auto-Loading Dependencies

**Current Problem:**
```bash
# User must remember to deserialize each dependency
otto_deserialize_input "task_a"
otto_deserialize_input "task_b"
```

**Solution:**
Otto's prologue generator knows all dependencies from the DAG. It should automatically load them all!

**Implementation in `src/executor/action.rs`:**

```rust
fn generate_bash_input_section(&self, dependencies: &[String]) -> String {
    if dependencies.is_empty() {
        return String::new();
    }

    let mut section = vec![
        "# Auto-load all task dependencies".to_string(),
    ];

    for dep in dependencies {
        section.push(format!(
            r#"# Load dependency: {}
if [ -f "$OTTO_TASK_DIR/input.{}.json" ]; then
    while IFS= read -r _key; do
        if [ "$_key" != "null" ] && [ "$_key" != "" ]; then
            _value=$(jq -r --arg k "$_key" '.[$k] // empty' "$OTTO_TASK_DIR/input.{}.json")
            if [ "$_value" != "" ] && [ "$_value" != "null" ]; then
                INPUT[{}.${_key}]="$_value"
            fi
        fi
    done < <(jq -r 'keys[]' "$OTTO_TASK_DIR/input.{}.json" 2>/dev/null)
fi"#,
            dep, dep, dep, dep, dep
        ));
    }

    section.push(String::new());
    section.join("\n")
}
```

### Improvement 3: Better Error Messages

**Current:** If OUTPUT isn't set, you get cryptic errors or empty JSON.

**Proposed:** Validate and provide helpful messages.

**Epilogue Enhancement (Bash):**
```bash
# Validate OUTPUT before serialization
if [ "${#OUTPUT[@]}" -eq 0 ]; then
    echo "Warning: Task '$OTTO_TASK_NAME' produced no output" >&2
    echo "Tip: Set OUTPUT[key]='value' to pass data to dependent tasks" >&2
    echo '{}' > "$OTTO_TASK_DIR/output.$OTTO_TASK_NAME.json"
    exit 0
fi

# Check for common mistakes
for key in "${!OUTPUT[@]}"; do
    if [[ "$key" == *" "* ]]; then
        echo "Error: OUTPUT key contains spaces: '$key'" >&2
        echo "Tip: Use underscores or camelCase instead" >&2
        exit 1
    fi
done
```

**Epilogue Enhancement (Python):**
```python
# Validate OUTPUT
if not OUTPUT:
    print(f"Warning: Task '{os.environ['OTTO_TASK_NAME']}' produced no output", file=sys.stderr)
    print("Tip: Set OUTPUT['key'] = 'value' to pass data to dependent tasks", file=sys.stderr)
    with open(_output_file, 'w') as f:
        f.write('{}')
    sys.exit(0)

# Check for non-JSON-serializable types
try:
    json.dumps(OUTPUT)
except TypeError as e:
    print(f"Error: OUTPUT contains non-JSON-serializable data: {e}", file=sys.stderr)
    print("Tip: Only use strings, numbers, booleans, lists, and dicts", file=sys.stderr)
    print(f"Problematic data: {OUTPUT}", file=sys.stderr)
    sys.exit(1)
```

### Improvement 4: Documentation in Generated Scripts

**Add Comments for Discoverability:**

```bash
#!/bin/bash
set -euo pipefail

# ============================================================
# OTTO TASK: deploy
# ============================================================
# This script is auto-generated by Otto. It includes:
#
# INPUT - Associative array with data from dependencies:
#   INPUT[task_name.key] - Access data from 'task_name'
#
# OUTPUT - Associative array for passing data forward:
#   OUTPUT[key]="value" - Set data for dependent tasks
#
# ENVIRONMENT:
#   OTTO_TASK_DIR  - This task's working directory
#   OTTO_TASK_NAME - This task's name ("deploy")
#
# To debug: cat "$OTTO_TASK_DIR/input.*.json"
# ============================================================

export OTTO_TASK_DIR="$(dirname "$0")"
export OTTO_TASK_NAME="deploy"

declare -A INPUT
declare -A OUTPUT

# Dependencies loaded:
#   - build (input.build.json)
#   - test (input.test.json)

# [auto-loading code...]

# ============================================================
# USER SCRIPT STARTS HERE
# ============================================================
```

### Improvement 5: Alternative Simple API (Environment Variables)

For simple use cases, provide an even simpler option:

```yaml
tasks:
  build:
    bash: |
      # Super simple: just export with OTTO_ prefix
      export OTTO_VERSION="1.2.3"
      export OTTO_ARTIFACT="/path/to/app"
      export OTTO_STATUS="success"

  deploy:
    before: [build]
    bash: |
      # Otto auto-propagates as OTTO_BUILD_*
      echo "Deploying version: $OTTO_BUILD_VERSION"
      echo "Artifact: $OTTO_BUILD_ARTIFACT"

      # Can still use OUTPUT for structured data
      OUTPUT[deployed_to]="production"
      OUTPUT[timestamp]="$(date -Iseconds)"
```

**Implementation:**
1. Otto scans for `OTTO_*` variables in task environment after completion
2. Writes to output.json automatically
3. Propagates to dependent tasks with `OTTO_TASKNAME_` prefix

**Limits to document:**
- Max 4KB per variable
- String-only (no arrays, objects)
- For complex data, use OUTPUT array/dict instead

## Migration Strategy

### Phase 1: Add New API (Backward Compatible)

1. **Detect Bash Version:**
```bash
if [ "${BASH_VERSINFO[0]}" -ge 4 ]; then
    # Use associative arrays (new API)
    declare -A INPUT
    declare -A OUTPUT
else
    # Use indexed arrays (old API)
    declare -a OTTO_INPUT
    declare -a OTTO_OUTPUT
    # Define helper functions
fi
```

2. **Support Both APIs:**
- Keep `otto_set_output` / `otto_get_input` working
- Add new `INPUT` / `OUTPUT` arrays
- Document both, recommend new API

3. **Update Examples:**
- Create ex14, ex15 showing new API
- Add migration guide
- Keep old examples working

### Phase 2: Improve Error Messages

1. Add validation in epilogues
2. Detect common mistakes
3. Provide helpful tips

### Phase 3: Auto-Loading

1. Implement automatic dependency loading
2. Deprecate manual `otto_deserialize_input` calls
3. Keep backward compatibility

### Phase 4: Environment Variable Option

1. Add OTTO_* variable scanning
2. Document usage and limitations
3. Add examples

## Implementation Checklist

### Code Changes

- [ ] `src/executor/action.rs`
  - [ ] Add bash version detection
  - [ ] Generate improved prologue with INPUT/OUTPUT
  - [ ] Auto-load dependencies
  - [ ] Add validation in epilogue
  - [ ] Add documentation comments

- [ ] `src/executor/action.rs` (Python)
  - [ ] Generate improved prologue with INPUT/OUTPUT
  - [ ] Auto-load dependencies
  - [ ] Add validation in epilogue
  - [ ] Add documentation comments

- [ ] `src/executor/scheduler.rs`
  - [ ] Pass dependency list to ActionProcessor
  - [ ] Ensure symlinks created before prologue runs

- [ ] Add OTTO_* environment variable scanning (optional)

### Documentation

- [ ] Update `docs/data-passing-plan.md`
- [ ] Create migration guide
- [ ] Update README examples
- [ ] Add troubleshooting guide

### Examples

- [ ] Create `examples/ex14/` - New API showcase
- [ ] Create `examples/ex15/` - Environment variable approach
- [ ] Update existing examples with comments

### Tests

- [ ] Test bash 3.2 compatibility (old API still works)
- [ ] Test bash 4+ with new API
- [ ] Test python new API
- [ ] Test auto-loading dependencies
- [ ] Test error messages and validation
- [ ] Test empty OUTPUT handling
- [ ] Test OTTO_* environment variables

## Example: Complete Before/After

### Before (Current - ex11)

```yaml
tasks:
  generate:
    bash: |
      timestamp=$(date +%Y%m%d%H%M%S)
      otto_set_output "timestamp" "$timestamp"
      otto_set_output "format" "digits"

  consume:
    before: ["generate"]
    bash: |
      otto_deserialize_input "generate"  # Manual!
      timestamp=$(otto_get_input "generate.timestamp")
      format=$(otto_get_input "generate.format")
      echo "Received: $timestamp ($format)"
```

### After (Proposed - ex14)

```yaml
tasks:
  generate:
    bash: |
      timestamp=$(date +%Y%m%d%H%M%S)

      # Natural bash syntax
      OUTPUT[timestamp]="$timestamp"
      OUTPUT[format]="digits"
      OUTPUT[app]="timestamp-demo"

  consume:
    before: ["generate"]
    bash: |
      # Dependencies auto-loaded!
      echo "App: ${INPUT[generate.app]}"
      echo "Timestamp: ${INPUT[generate.timestamp]}"
      echo "Format: ${INPUT[generate.format]}"

      # Parse and validate
      if [[ ${#INPUT[generate.timestamp]} -eq 14 ]]; then
        OUTPUT[validation]="PASSED"
        OUTPUT[parsed_date]="${INPUT[generate.timestamp]:0:8}"
      else
        OUTPUT[validation]="FAILED"
      fi
```

### After (Alternative - ex15 - Super Simple)

```yaml
tasks:
  generate:
    bash: |
      # Even simpler: just export!
      export OTTO_TIMESTAMP="$(date +%Y%m%d%H%M%S)"
      export OTTO_FORMAT="digits"
      export OTTO_APP="timestamp-demo"

  consume:
    before: ["generate"]
    bash: |
      # Auto-propagated!
      echo "App: $OTTO_GENERATE_APP"
      echo "Timestamp: $OTTO_GENERATE_TIMESTAMP"
      echo "Format: $OTTO_GENERATE_FORMAT"
```

## Conclusion

These improvements address all major pain points:

1. ✅ **No more magic functions** - Use native bash/python syntax
2. ✅ **Auto-loading** - Dependencies loaded automatically
3. ✅ **Better errors** - Helpful validation and messages
4. ✅ **Discoverable** - Comments explain what's available
5. ✅ **Backward compatible** - Old API still works
6. ✅ **Simple option** - Environment variables for trivial cases

The implementation complexity is low-to-medium, and the benefits are significant for user experience.
