# Data Passing Executive Summary

## TL;DR

**Current approach is correct.** Files + symlinks + JSON is the right architecture. The problem is the **API ergonomics**, not the underlying mechanism.

**Recommendation:** Improve the user-facing API while keeping the file-based backend.

## What I Investigated

1. ✅ Current state of Otto's data passing (files + symlinks + JSON)
2. ✅ 9 alternative IPC methods (mmap, sockets, pipes, env vars, etc.)
3. ✅ Performance characteristics of each method
4. ✅ Usability from bash/python inline scripts
5. ✅ Concrete proposals for improvement

## Current Problems (and Solutions)

### Problem 1: Magic Helper Functions
```bash
# Current (awkward)
otto_set_output "key" "value"
value=$(otto_get_input "task_a.key")
```

**Solution:** Use native language constructs
```bash
# Proposed (natural)
OUTPUT[key]="value"
value="${INPUT[task_a.key]}"
```

### Problem 2: Manual Dependency Loading
```bash
# Current (must remember each dependency)
otto_deserialize_input "task_a"
otto_deserialize_input "task_b"
```

**Solution:** Auto-load all dependencies in prologue
```bash
# Proposed (automatic)
# Dependencies just work - INPUT already populated
echo "${INPUT[task_a.key]}"
```

### Problem 3: Bash 3.2 Compatibility Complexity
```bash
# Current (indexed arrays with key=value strings)
OTTO_OUTPUT+=("key=value")  # Awkward!
```

**Solution:** Detect bash version and use best API
```bash
# Bash 4+: Use associative arrays
OUTPUT[key]="value"

# Bash 3.2: Keep helper functions but improve names
otto.set key value  # Shorter, cleaner
```

### Problem 4: Poor Discoverability
```bash
# Current: No comments, user doesn't know what's available
#!/bin/bash
# ... magic prologue ...
```

**Solution:** Add documentation in generated scripts
```bash
#!/bin/bash
# ============================================================
# INPUT[task_name.key]  - Access dependency data
# OUTPUT[key]="value"   - Set output for dependent tasks
# OTTO_TASK_DIR         - This task's directory
# ============================================================
```

## Alternative IPC Methods - Quick Reference

| Method | Best For | Why NOT Use for Otto |
|--------|----------|---------------------|
| **Files + JSON** (current) | ✅ Task data passing | **Use this!** |
| Environment Variables | Simple strings | Size limits, no structure |
| Named Pipes (FIFOs) | Streaming data | No history, ephemeral |
| Unix Domain Sockets | Real-time messaging | Requires daemon, complex |
| Memory-Mapped Files | Huge datasets | Binary format, complex |
| Stdout Parsing | Universal output | Clutters logs, fragile |
| Redis | Complex workflows | External dependency |
| SQLite | Structured queries | Verbose SQL syntax |
| Stdin/Stdout Pipes | Linear pipelines | No fan-out, no history |

## What to Keep from Current Design

1. ✅ **Files for persistence** - Debugging, history, replay
2. ✅ **JSON for structure** - Human-readable, language-agnostic
3. ✅ **Symlinks for data flow** - Visible dependencies
4. ✅ **Task-level isolation** - Each task has its own directory
5. ✅ **Prologue/epilogue pattern** - Automatic serialization

## What to Change

1. 🔄 **Use native language constructs** - Arrays in bash, dicts in python
2. 🔄 **Auto-load dependencies** - No manual deserialization
3. 🔄 **Better variable names** - INPUT/OUTPUT instead of OTTO_*
4. 🔄 **Add documentation comments** - Explain what's available
5. 🔄 **Validate and provide helpful errors** - Guide users

## Implementation Plan

### Phase 1: Improved API (2-3 days)
- Detect bash version in prologue
- Generate INPUT/OUTPUT arrays/dicts
- Auto-load all dependencies
- Add documentation comments
- Better error messages

**Files to modify:**
- `src/executor/action.rs` (prologue/epilogue generation)
- Examples: Create ex14, ex15

### Phase 2: Environment Variable Option (1 day)
- Scan for `OTTO_*` exports after task completion
- Auto-propagate to dependent tasks
- Document size limits and use cases

**Files to modify:**
- `src/executor/scheduler.rs` (capture env vars)
- `src/executor/action.rs` (inject into prologue)

### Phase 3: Streaming Support (Future)
- Add `streaming: true` flag
- Set up named pipes between tasks
- Document streaming patterns

**Files to modify:**
- `src/cfg/task.rs` (add streaming field)
- `src/executor/scheduler.rs` (create FIFOs)

## Key Insights from Research

1. **File-based is not slow** - For typical task data (< 1MB), JSON serialization is negligible
2. **Debuggability matters** - Being able to `cat` output files is invaluable
3. **Simplicity wins** - Complex IPC adds dependencies and failure modes
4. **History is a feature** - Persistent data enables replay and debugging
5. **Language-agnostic** - Files work for any language, not just bash/python

## Examples of Improved API

### Example 1: Simple Key-Value

**Before:**
```bash
otto_set_output "version" "1.2.3"
otto_set_output "status" "success"

otto_deserialize_input "build"
version=$(otto_get_input "build.version")
```

**After:**
```bash
OUTPUT[version]="1.2.3"
OUTPUT[status]="success"

# Dependencies auto-loaded
version="${INPUT[build.version]}"
```

### Example 2: Python Data Types

**Before:**
```python
import __main__
__main__.OTTO_OUTPUT["count"] = 42
otto_deserialize_input("data")
count = __main__.OTTO_INPUT.get("data.count", 0)
```

**After:**
```python
OUTPUT["count"] = 42
OUTPUT["items"] = ["a", "b", "c"]

# Dependencies auto-loaded
count = INPUT.get("data.count", 0)
```

### Example 3: Environment Variables (Simple Cases)

**New Option:**
```bash
# Task A - just export
export OTTO_VERSION="1.2.3"
export OTTO_STATUS="success"

# Task B - auto-propagated
echo "Version: $OTTO_BUILD_VERSION"
echo "Status: $OTTO_BUILD_STATUS"
```

## Documents Created

1. **ipc-methods-analysis.md** (comprehensive survey)
   - All 9 IPC methods analyzed
   - Performance comparison matrix
   - Feature comparison matrix
   - When to use each method

2. **improved-data-passing-proposal.md** (concrete implementation)
   - Before/after code examples
   - Implementation checklist
   - Migration strategy
   - Test plan

3. **advanced-ipc-deep-dive.md** (technical reference)
   - Detailed implementations (mmap, sockets, pipes)
   - Code examples in Rust, Bash, Python
   - When to use advanced IPC (streaming, real-time)

4. **data-passing-executive-summary.md** (this document)
   - Quick reference for decision makers
   - Clear recommendations
   - Next steps

## Recommendation

**Implement Phase 1 (Improved API) immediately.** This addresses 90% of user pain points with minimal risk and effort.

- Low implementation complexity (2-3 days)
- High user value (much better ergonomics)
- Zero breaking changes (backward compatible)
- Keeps all current benefits (debuggability, persistence, etc.)

**Skip** advanced IPC methods (mmap, sockets, Redis) - they add complexity without clear benefits for Otto's use case.

**Consider** Phase 2 (environment variables) as a quick win for simple cases - 1 day of work, covers 80% of simple data passing needs.

**Defer** Phase 3 (streaming) until there's clear user demand - it's a significant feature that needs careful design.

## Questions Answered

**Q: Should we use shared memory?**
A: No. Files are fast enough, and shared memory is complex and non-portable.

**Q: Should we use Unix domain sockets?**
A: Not for data passing. Maybe for future live progress updates.

**Q: Should we use environment variables?**
A: As an *option* yes, for simple cases. Not as a replacement for files.

**Q: Should we use named pipes?**
A: For future *streaming* features, yes. Not for standard data passing.

**Q: What's the best improvement we can make?**
A: Better API (native bash/python syntax) + auto-loading dependencies.

## Next Steps

1. Review the three detailed documents:
   - Start with `improved-data-passing-proposal.md` for concrete examples
   - Reference `ipc-methods-analysis.md` for comparison data
   - Use `advanced-ipc-deep-dive.md` for future features

2. Decide on implementation timeline:
   - Phase 1 (Improved API) - Recommend doing this
   - Phase 2 (Env vars) - Nice to have
   - Phase 3 (Streaming) - Future consideration

3. Create tickets/issues if you agree with the approach

4. I can help implement if you'd like to proceed
