# Unimplemented Global Flags

This document describes global CLI flags that were partially implemented (defined in the CLI parser) but never wired into the execution engine. They were removed to avoid user confusion but documented here for future implementation.

## Overview

These flags were defined in the Otto CLI but never extracted from the argument parser or passed to the task execution layer. They are preserved here as a specification for future implementation.

## Removed Flags

### `--verbose` / `-v`

**Purpose:** Enable verbose output during task execution

**Intended Behavior:**
- Show detailed information about task execution
- Display environment variable evaluation
- Show file dependency resolution
- Print command execution details
- Log scheduler decisions (which tasks run when, why tasks are skipped)

**Implementation Notes:**
- Should be extracted in `Parser::parse()` method
- Pass as parameter to `TaskScheduler::new()`
- Use to control logging levels throughout execution
- Consider interaction with `--quiet` (mutually exclusive?)

**Example Usage:**
```bash
otto --verbose build
otto -v test deploy
```

---

### `--quiet` / `-q`

**Purpose:** Suppress non-essential output

**Intended Behavior:**
- Only show errors and critical warnings
- Suppress task start/completion messages
- Hide stdout/stderr from successful tasks
- Still show output from failed tasks for debugging

**Implementation Notes:**
- Should be extracted in `Parser::parse()` method
- Pass as parameter to `TaskScheduler::new()`
- May want to suppress TUI mode when enabled
- Consider interaction with `--verbose` (mutually exclusive)

**Example Usage:**
```bash
otto --quiet build
otto -q test
```

---

### `--dry-run`

**Purpose:** Show what would be done without executing

**Intended Behavior:**
- Parse and validate the ottofile
- Build the task dependency graph
- Resolve which tasks would run
- Check file dependencies
- Print the execution plan
- **DO NOT** execute any task commands
- **DO NOT** create run directories or artifacts

**Implementation Notes:**
- Should be extracted in `Parser::parse()` method
- Pass as parameter to `TaskScheduler::new()`
- Modify task execution to skip actual command execution
- Still validate that commands/scripts are syntactically correct
- Show task order and parallelism plan
- Useful for debugging complex task graphs

**Example Usage:**
```bash
otto --dry-run build test deploy
otto --dry-run ci
```

**Expected Output Example:**
```
Dry run mode - no tasks will be executed

Tasks to execute (in order):
  1. [parallel] check, fmt-check
  2. test (depends on: check)
  3. clippy (depends on: fmt-check)
  4. build (depends on: test, clippy)

Would execute 4 tasks across 2 parallel stages
```

---

### `--force`

**Purpose:** Force execution even if up-to-date

**Intended Behavior:**
- Ignore file dependency checks
- Ignore output dependency checks
- Always execute tasks even if outputs are newer than inputs
- Useful for forcing rebuilds
- Useful when dependencies are external or not fully captured

**Implementation Notes:**
- Should be extracted in `Parser::parse()` method
- Pass as parameter to `TaskScheduler::new()`
- Modify up-to-date checking logic in task execution
- Consider caching mechanisms that would need to be bypassed
- May want to propagate to dependent tasks

**Example Usage:**
```bash
otto --force build
otto --force test  # Force rerun even if nothing changed
```

---

### `--no-deps`

**Purpose:** Don't run task dependencies

**Intended Behavior:**
- Execute only the specified task(s)
- Skip all dependencies (before/after/file/output)
- Useful for development/debugging
- Useful when you know dependencies are already satisfied
- Should validate that task exists but not validate dependencies

**Implementation Notes:**
- Should be extracted in `Parser::parse()` method
- Modify DAG construction to exclude dependencies
- Or pass to `TaskScheduler` to skip dependency execution
- Still need to handle task graph parsing for the specified tasks
- Warn user if skipping dependencies might cause issues?

**Example Usage:**
```bash
otto --no-deps test        # Run only test task, not check/fmt-check
otto --no-deps deploy      # Deploy without running build/test first
```

---

## Implementation Checklist

When implementing these flags:

- [ ] Extract flag values in `Parser::parse()` method (around line 320)
- [ ] Modify return signature: `Result<(Vec<Task>, String, Option<PathBuf>, usize, bool)>` → add flags
- [ ] Pass flags to `TaskScheduler::new()` in `main.rs` (lines 298, 380)
- [ ] Add fields to `TaskScheduler` struct
- [ ] Implement behavior in task execution logic
- [ ] Add tests for each flag
- [ ] Update `--help` documentation
- [ ] Consider flag interactions (verbose + quiet, dry-run + force, etc.)
- [ ] Update this document to link to implementation PR/commit

## Related Files

- `src/cli/parser.rs` - CLI argument parsing (where flags need to be extracted)
- `src/main.rs` - Main entry point (where flags need to be passed to scheduler)
- `src/executor/scheduler.rs` - Task execution (where flags need to be used)
- `src/executor/mod.rs` - Execution module

## References

Similar flags in other task runners:
- Make: `--dry-run` / `-n`, `--just-print`
- Cargo: `--verbose` / `-v`, `--quiet` / `-q`
- Just: `--dry-run`, `--verbose`
- Ninja: `--verbose`, `-n` (dry-run)

## Removal Information

**Removed:** 2025-11-06
**Reason:** Dead code - flags were defined but never wired to execution engine
**PR/Commit:** [Add reference when committed]
