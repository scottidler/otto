# Data Passing Investigation - Complete Summary

**Date:** December 8, 2025
**Topic:** Inter-process data passing between Otto tasks

## What Was Requested

Investigate data passing mechanisms in Otto, particularly:
1. How data currently flows from one task to another
2. Current awkwardness of getting data in/out of bash/python scripts
3. Explore better methods (memory maps, IPC mechanisms, etc.)
4. Find alternatives to the "magic variable OTTO_SOMETHING" approach

## Key Discoveries

### 1. Current Mechanism is Sound ✅

Otto uses **files + JSON + symlinks**:
- Tasks write to `output.<task>.json` files
- Otto creates symlinks for dependencies: `input.<dep>.json` → `../dep/output.<dep>.json`
- **This is the RIGHT approach** for Otto's use case

**Why it's good:**
- ✅ Debuggable (`cat` the files)
- ✅ Persistent (history for replay)
- ✅ Language-agnostic (works with any language)
- ✅ Simple (no daemons, no external dependencies)
- ✅ Aligns with Otto's philosophy of transparency

### 2. Otto ALREADY Uses SQLite ✅

Discovered that Otto has **extensive SQLite integration** at `~/.otto/otto.db`:
- Tracks projects, runs, and tasks
- Stores metadata, durations, exit codes
- Stores **paths** to artifacts (not content)

**Deliberately keeps on filesystem:**
- Scripts (inspectable)
- Logs (tailable)
- **Task outputs (debuggable)**

This was an intentional design decision documented in `docs/architecture/sqlite-integration.md`.

### 3. The Real Problem: API Ergonomics ⚠️

The issue isn't the mechanism, it's the **user-facing API**:

**Current (awkward):**
```bash
otto_set_output "key" "value"           # Magic function
otto_deserialize_input "task_a"          # Manual step
value=$(otto_get_input "task_a.key")    # Verbose
```

**Users want:**
```bash
OUTPUT[key]="value"                      # Native syntax
value="${INPUT[task_a.key]}"            # Auto-loaded
```

## Documents Created

Created **9 comprehensive documents** in `docs/`:

### 1. `data-passing-executive-summary.md` ⭐ START HERE
- Quick TL;DR of findings
- Problem/solution pairs
- Clear recommendations
- What to do next
- **Read this first**

### 2. `ipc-methods-analysis.md` 📊 COMPREHENSIVE
- Detailed analysis of 9 IPC methods
- Performance comparison matrices
- Feature comparison tables
- Pros/cons for each method
- When to use each approach

### 3. `improved-data-passing-proposal.md` 🛠️ IMPLEMENTATION
- Concrete before/after code examples
- Implementation checklist
- Phase-by-phase plan
- Migration strategy
- Test requirements

### 4. `advanced-ipc-deep-dive.md` 🔬 TECHNICAL
- Deep technical implementations
- Full Rust/Bash/Python code examples
- Memory-mapped files details
- Unix domain sockets details
- Named pipes (FIFOs) details
- For future features (streaming, real-time)

### 5. `ipc-quick-reference.md` 📋 CHEAT SHEET
- One-page reference card
- Quick comparison table
- Decision matrix
- When to use what
- Code snippets

### 6. `data-passing-ergonomics-comparison.md` 🎯 OPTIONS
- All 5 options compared side-by-side
- Current state vs. proposals
- Implementation effort estimates
- Real code examples for each option
- Recommendation matrix

### 7. `relative-symlinks-plan.md` (Already existed)
- How Otto uses symlinks
- Relative vs. absolute paths
- Current implementation details

### 8. `data-passing-plan.md` (Already existed)
- Current action processing system
- Script generation details
- Known issues and fixes

### 9. `investigation-summary.md` (This document)
- Complete overview of investigation
- What was created
- Key recommendations
- Next steps

## Example Created

### `examples/ex14/` - Data Passing Demo ⭐

**Comprehensive example** showing `otto_set_output` and `otto_get_input` in both **Bash and Python**:

**Demonstrates:**
- ✅ Bash → Bash data passing
- ✅ Python → Python data passing
- ✅ Bash → Python cross-language flow
- ✅ Python → Bash cross-language flow
- ✅ Simple values (strings, numbers)
- ✅ Complex values (JSON objects, arrays)
- ✅ Multiple dependencies
- ✅ Real-world pipeline

**Files created:**
- `examples/ex14/otto.yml` - Complete working example
- `examples/ex14/README.md` - Detailed documentation
- `examples/ex14/test.sh` - Automated test script
- `examples/README.md` - Updated index with ex14

**To run:**
```bash
cd examples/ex14
otto final_report
./test.sh
```

## Alternative IPC Methods Explored

Thoroughly investigated **9 different approaches**:

1. **Files + JSON** (current) ✅ - Keep this
2. **Environment Variables** - Good for simple cases
3. **Named Pipes (FIFOs)** - Good for streaming
4. **Unix Domain Sockets** - Good for real-time
5. **Memory-Mapped Files** - Overkill, too complex
6. **Stdout/Stdin Pipes** - Limited to linear flows
7. **Stdout Parsing** - Clutters logs, fragile
8. **Redis** - External dependency, overkill
9. **SQLite** - Already there, but loses debuggability

### Comparison Summary

| Method | Speed | Simple | Debug | Persist | Best For |
|--------|-------|--------|-------|---------|----------|
| **Files + JSON** ✅ | ⭐⭐⭐ | ⭐⭐⭐⭐⭐ | ⭐⭐⭐⭐⭐ | ⭐⭐⭐⭐⭐ | Standard data passing |
| Env Vars | ⭐⭐⭐⭐⭐ | ⭐⭐⭐⭐⭐ | ⭐⭐⭐⭐ | ⭐⭐ | Simple strings |
| Named Pipes | ⭐⭐⭐⭐ | ⭐⭐⭐ | ⭐ | ⭐ | Streaming data |
| Unix Sockets | ⭐⭐⭐⭐ | ⭐⭐ | ⭐⭐ | ⭐ | Real-time coordination |
| mmap | ⭐⭐⭐⭐⭐ | ⭐ | ⭐ | ⭐⭐⭐ | Huge datasets |

## Recommendations

### Immediate: Improve the API (2-3 days) ⭐

**What:** Use native language constructs instead of magic functions

**Bash (4.0+):**
```bash
# Instead of:
otto_set_output "key" "value"

# Use:
OUTPUT[key]="value"
```

**Python:**
```python
# Instead of:
otto_set_output("key", "value")

# Use:
OUTPUT["key"] = "value"
```

**Why:**
- Feels natural (native syntax)
- Auto-loads dependencies (no manual steps)
- Still uses files (keeps debuggability)
- Minimal breaking changes

**Implementation:**
- Modify prologue generation in `src/executor/action.rs`
- Detect bash version (use associative arrays for 4.0+)
- Auto-load all dependencies in prologue
- Keep backward compatibility

**See:** `improved-data-passing-proposal.md` for details

### Quick Win: Environment Variables (1 day)

**What:** Auto-propagate `OTTO_*` exports

```bash
# Task A
export OTTO_VERSION="1.2.3"

# Task B (auto-propagated!)
echo "$OTTO_TASK_A_VERSION"
```

**Why:**
- Extremely simple
- Works for 80% of simple cases
- Zero learning curve

**Limitations:**
- Size limits (4KB per var)
- String-only
- Document when to use vs. files

### Skip: SQLite for Data Passing ❌

**Don't** store task outputs in the SQLite database because:
- ❌ Loses debuggability (can't `cat`)
- ❌ Requires tools (can't inspect easily)
- ❌ Goes against Otto's design principles
- ❌ Database is already used correctly (metadata only)

**Keep SQLite for:**
- ✅ Metadata (runs, tasks, durations)
- ✅ History queries
- ✅ Statistics

### Future: Streaming (Phase 3)

**What:** Add `streaming: true` flag with named pipes

```yaml
generate_logs:
  streaming: true
  bash: tail -f /var/log/app.log

parse_logs:
  before: [generate_logs]
  streaming: true
  bash: grep ERROR
```

**When:** When there's clear user demand for streaming workflows

## What Exists Today vs. Proposals

### ✅ Real (Exists Now)

```bash
# Bash functions in builtins.sh
otto_set_output "key" "value"
otto_get_input "task_a.key"

# Python functions in builtins.py
otto_set_output("key", "value")
otto_get_input("task_a.key")
```

These are **real functions** that work today. Example 14 demonstrates them.

### ❌ Proposed (Doesn't Exist Yet)

```bash
# Native syntax (proposed)
OUTPUT[key]="value"
INPUT[task_a.key]

# Environment variables (proposed)
export OTTO_VERSION="1.2.3"
echo "$OTTO_TASK_A_VERSION"

# SQLite helpers (hypothetical brainstorm)
otto data set key value
otto data get task_a key
```

These would need to be **built** if you decide to proceed.

## Key Insights

1. **File-based approach is correct** - Don't change the fundamental mechanism
2. **Otto's SQLite usage is already optimal** - Metadata in DB, artifacts on filesystem
3. **API ergonomics is the issue** - Make the interface feel natural
4. **Simplicity matters** - Complex IPC adds little value for typical use cases
5. **Debuggability is crucial** - Being able to `cat` output files is invaluable

## Architecture Principles Confirmed

From `docs/architecture/sqlite-integration.md`:

**Keep on Filesystem:**
- ✅ Scripts (inspectable)
- ✅ Logs (tailable)
- ✅ Outputs (debuggable)

**Keep in Database:**
- ✅ Metadata
- ✅ Relationships
- ✅ Metrics

This separation is **correct and intentional**.

## Next Steps

### 1. Review the Documents

**Start with:**
1. `data-passing-executive-summary.md` - Quick overview
2. `examples/ex14/README.md` - See current API in action
3. `improved-data-passing-proposal.md` - Concrete improvement plan

**Reference as needed:**
- `ipc-methods-analysis.md` - Detailed comparison
- `data-passing-ergonomics-comparison.md` - All options
- `ipc-quick-reference.md` - Quick lookup

### 2. Try Example 14

```bash
cd examples/ex14
otto final_report
./test.sh

# Inspect the outputs
cd ~/.otto/ex14-*/latest/tasks
ls -la */output.*.json
cat bash_producer/output.bash_producer.json
```

### 3. Decide on Implementation

**Option A: Improve API** (Recommended)
- Implement native constructs (2-3 days)
- See `improved-data-passing-proposal.md`
- Keeps all current benefits

**Option B: Add Environment Variables** (Quick win)
- Simple for basic cases (1 day)
- Good complement to file-based approach

**Option C: Keep Current** (No changes)
- If current API is acceptable
- Focus effort elsewhere

### 4. Optional: Implement

If you want to proceed with improvements, I can help:
1. Prototype the improved bash prologue
2. Add auto-dependency loading
3. Create migration guide
4. Update tests and examples

## Files Created Summary

### Documentation (9 files)
```
docs/
├── data-passing-executive-summary.md      (Quick overview)
├── ipc-methods-analysis.md                (Comprehensive survey)
├── improved-data-passing-proposal.md      (Implementation plan)
├── advanced-ipc-deep-dive.md              (Technical details)
├── ipc-quick-reference.md                 (Cheat sheet)
├── data-passing-ergonomics-comparison.md  (All options)
└── investigation-summary.md               (This file)
```

### Examples (4 files)
```
examples/
├── ex14/
│   ├── otto.yml          (Complete working example)
│   ├── README.md         (Detailed docs)
│   └── test.sh          (Automated tests)
└── README.md            (Updated index)
```

## Bottom Line

**The investigation confirms:**
1. ✅ Current architecture (files + JSON) is correct
2. ✅ SQLite usage is already optimal
3. ⚠️ API ergonomics could be improved
4. ❌ Advanced IPC methods add complexity without clear benefit

**Recommended action:**
Improve the user-facing API while keeping the proven file-based backend.

**Alternative:**
Keep current API if it's acceptable, document it better with example 14.

## Questions Answered

**Q: Should we use shared memory?**
A: No. Too complex, binary format not debuggable.

**Q: Should we use Unix domain sockets?**
A: Only for future real-time features (progress updates).

**Q: Should we use environment variables?**
A: As an option for simple cases, not as replacement.

**Q: Should we store task data in SQLite?**
A: No. Files are debuggable, database is not. Keep current separation.

**Q: What about memory maps?**
A: Overkill for typical use cases. Only if you need zero-copy of huge data.

**Q: What's the best improvement?**
A: Native language syntax (INPUT/OUTPUT arrays) + auto-loading dependencies.

---

**End of Investigation Summary**

For questions or to proceed with implementation, refer to the detailed documents or let me know which direction you'd like to take.
