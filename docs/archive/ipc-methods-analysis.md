# IPC Methods Analysis for Otto Data Passing

## Executive Summary

This document provides a thorough survey of inter-process communication (IPC) methods suitable for passing data between tasks in Otto. Currently, Otto uses JSON files written to `.otto/` with symlinks for data passing. This analysis explores alternatives that could improve the user experience for bash/python script authors.

## Current State: File-Based with JSON + Symlinks

### How It Works Now

**Data Flow:**
```
Task A (bash/python)
    ↓
  otto_set_output "key" "value"  # Writes to array/dict
    ↓
  Epilogue: otto_serialize_output "task_a"
    ↓
  ~/.otto/project-hash/timestamp/tasks/task_a/output.task_a.json
    ↓ (symlink created by scheduler)
  ~/.otto/project-hash/timestamp/tasks/task_b/input.task_a.json
    ↓
  Prologue: otto_deserialize_input "task_a"
    ↓
  otto_get_input "task_a.key"  # Reads from array/dict
    ↓
Task B (bash/python)
```

### Current API

**Bash:**
```bash
# Writing data
otto_set_output "key" "value"
otto_set_output "count" "42"

# Reading data (from dependency "task_a")
value=$(otto_get_input "task_a.key")
count=$(otto_get_input "task_a.count")
```

**Python:**
```python
# Writing data
OTTO_OUTPUT["key"] = "value"
OTTO_OUTPUT["count"] = 42

# Reading data (from dependency "task_a")
value = OTTO_INPUT.get("task_a.key")
count = OTTO_INPUT.get("task_a.count")
```

### Current Problems

1. **Awkward API**: Helper functions (`otto_set_output`, `otto_get_input`) feel like magic
2. **Implementation Complexity**: Bash 3.2 compatibility requires indexed arrays with `key=value` format
3. **Serialization Overhead**: Converting to/from JSON with jq (bash) or json module (python)
4. **Limited Data Types**: Everything becomes strings in bash; python preserves types but has constraints
5. **No Streaming**: Must wait for entire task completion before data is available
6. **Hidden Magic**: User doesn't see the prologue/epilogue that handles serialization

### Current Benefits

1. **Debuggable**: JSON files are human-readable and inspectable
2. **Persistent**: Data survives in history for debugging
3. **Language Agnostic**: Works with any language (bash, python, future: ruby, etc.)
4. **Simple Dependencies**: Symlinks make data flow visible in filesystem
5. **Cross-Machine**: Could work in distributed scenarios (with shared filesystem)

## Alternative IPC Methods Survey

### 1. Memory-Mapped Files (mmap)

**Description**: Use shared memory regions backed by files in `/dev/shm` or similar tmpfs.

**Architecture:**
```
Otto creates: /dev/shm/otto-<run-id>-<task>.data
Task A writes: Binary or text data directly to mmap region
Otto creates: Another mmap for Task B pointing to same physical memory
Task B reads: Direct memory access (no copy)
```

**Pros:**
- ✅ Extremely fast (memory-speed access)
- ✅ Zero-copy between tasks
- ✅ Survives process death (until unmapped)
- ✅ Can use filesystem permissions for security

**Cons:**
- ❌ Requires size pre-allocation (or complex resizing)
- ❌ Binary format complicates bash access (would need helper tools)
- ❌ Platform-specific (different on Linux vs macOS vs Windows)
- ❌ Cleanup on crash is tricky
- ❌ Not intuitive for script authors

**Implementation Complexity:** HIGH

**User API Complexity:** HIGH (would need wrappers)

**Example Usage:**
```bash
# Would require C/Rust helper tools
otto-mmap-set task_a key value
value=$(otto-mmap-get task_a key)
```

### 2. Unix Domain Sockets

**Description**: Use local sockets for bidirectional communication between Otto coordinator and tasks.

**Architecture:**
```
Otto creates socket: /tmp/otto-<run-id>.sock
Otto runs server: Listens for task connections
Task A connects: Sends key-value pairs
Otto stores: In memory or local storage
Task B connects: Requests keys from Task A's data
```

**Pros:**
- ✅ Bidirectional communication
- ✅ Can stream data (partial results available immediately)
- ✅ Standard Unix primitive (widely supported)
- ✅ Automatic cleanup on process death
- ✅ Can implement pub/sub patterns

**Cons:**
- ❌ Requires Otto to run a daemon/server during execution
- ❌ More complex error handling (connection failures, timeouts)
- ❌ Protocol design needed (how to encode requests/responses?)
- ❌ Bash/Python would need socket client code (not trivial)
- ❌ Doesn't persist after run completes (breaks history/debugging)

**Implementation Complexity:** VERY HIGH

**User API Complexity:** MEDIUM (could hide behind helpers)

**Example Usage:**
```bash
# Would need protocol and client tools
otto-send key value
value=$(otto-recv task_a key)
```

### 3. Named Pipes (FIFOs)

**Description**: Use filesystem-based pipes for one-way or bidirectional streaming.

**Architecture:**
```
Otto creates: /tmp/otto-<run-id>-task_a.pipe
Task A writes: echo '{"key":"value"}' > $OTTO_OUTPUT_PIPE
Otto reads: Blocking read on pipe, stores data
Task B reads: Data provided via similar pipe or env var
```

**Pros:**
- ✅ Simple Unix primitive
- ✅ Natural for streaming data
- ✅ Automatic blocking/synchronization
- ✅ No size limits (unlike env vars)

**Cons:**
- ❌ Unidirectional (would need pairs for bidirectional)
- ❌ Blocking behavior can cause deadlocks if misused
- ❌ Data is ephemeral (gone after read)
- ❌ Difficult to debug (can't inspect pipe contents)
- ❌ Doesn't persist for history

**Implementation Complexity:** MEDIUM

**User API Complexity:** LOW (just redirect I/O)

**Example Usage:**
```bash
# Could be as simple as
echo '{"key":"value"}' > $OTTO_OUTPUT_PIPE

# Or read
read -r data < $OTTO_INPUT_PIPE
```

### 4. Enhanced Environment Variables

**Description**: Use environment variables more intelligently, possibly with namespacing and encoding.

**Architecture:**
```
Task A exports: OTTO_OUT_TASK_A_KEY="value"
Otto captures: Parse all OTTO_OUT_* variables after task completes
Otto propagates: Export as OTTO_IN_TASK_A_KEY="value" for Task B
```

**Pros:**
- ✅ Native to all shells and languages
- ✅ Zero additional code needed in user scripts
- ✅ Extremely simple mental model
- ✅ Fast (no I/O)
- ✅ Works across exec calls

**Cons:**
- ❌ Size limits (typically 128KB total environment, 4KB per var on Linux)
- ❌ Only strings (no structured data without encoding)
- ❌ No nested structures without serialization
- ❌ Environment pollution (many variables)
- ❌ Security concerns (env vars visible in process listings)
- ❌ Not persistent (gone after task completes unless Otto saves them)

**Implementation Complexity:** LOW

**User API Complexity:** VERY LOW

**Example Usage:**
```bash
# Task A
export OTTO_OUT_KEY="value"
export OTTO_OUT_COUNT="42"

# Task B (Otto automatically propagates)
echo "Received: $OTTO_IN_TASK_A_KEY"
echo "Count: $OTTO_IN_TASK_A_COUNT"
```

### 5. Stdout/Stderr Parsing with Markers

**Description**: Parse specially-formatted output from stdout/stderr to extract data.

**Architecture:**
```
Task A prints: <OTTO:OUTPUT:KEY>value</OTTO:OUTPUT:KEY>
Otto parses: Extracts key-value pairs from stdout
Otto provides: Via env vars, files, or stdin to Task B
```

**Pros:**
- ✅ No special API needed
- ✅ Works with any language
- ✅ Output naturally logged and preserved
- ✅ Human-readable in logs
- ✅ Simple to understand

**Cons:**
- ❌ Ugly output in logs (cluttered with markers)
- ❌ Fragile (what if user output contains markers?)
- ❌ Parsing complexity
- ❌ Binary data requires encoding (base64)
- ❌ Not discoverable (how does user know format?)

**Implementation Complexity:** MEDIUM

**User API Complexity:** MEDIUM (need to remember format)

**Example Usage:**
```bash
echo "Regular output"
echo "<OTTO:OUTPUT:KEY>value</OTTO:OUTPUT:KEY>"
echo "<OTTO:OUTPUT:COUNT>42</OTTO:OUTPUT:COUNT>"
echo "More regular output"
```

### 6. Redis/In-Memory Database

**Description**: Use a lightweight in-memory database for structured data storage.

**Architecture:**
```
Otto starts: Redis server on local socket
Task A writes: redis-cli -s /tmp/otto.sock SET task_a:key value
Task B reads: redis-cli -s /tmp/otto.sock GET task_a:key
Otto stops: Saves snapshot to disk, stops Redis
```

**Pros:**
- ✅ Rich data types (strings, lists, sets, hashes, JSON)
- ✅ Atomic operations
- ✅ Pub/sub for reactive tasks
- ✅ TTLs for automatic cleanup
- ✅ Mature, battle-tested
- ✅ Can persist to disk

**Cons:**
- ❌ External dependency (must install Redis)
- ❌ Overhead of running separate process
- ❌ Overkill for simple use cases
- ❌ Complexity for users (learn Redis commands)
- ❌ Cross-platform considerations

**Implementation Complexity:** HIGH

**User API Complexity:** MEDIUM (Redis CLI is simple but extra tool)

**Example Usage:**
```bash
# Task A
redis-cli -s $OTTO_REDIS_SOCK SET key value

# Task B
value=$(redis-cli -s $OTTO_REDIS_SOCK GET key)
```

### 7. SQLite Database

**Description**: Use a local SQLite database for structured data storage.

**Architecture:**
```
Otto creates: ~/.otto/project/run.db
Task A writes: sqlite3 $OTTO_DB "INSERT INTO kv VALUES ('key', 'value')"
Task B reads: sqlite3 $OTTO_DB "SELECT value FROM kv WHERE key='key'"
Otto keeps: Database persists in history
```

**Pros:**
- ✅ SQL is widely known
- ✅ Structured data with schemas
- ✅ Transactions for atomicity
- ✅ Persistent (built-in history)
- ✅ No external daemon needed
- ✅ Cross-platform
- ✅ Otto already uses SQLite for history!

**Cons:**
- ❌ Requires sqlite3 CLI tool
- ❌ Verbose syntax for simple operations
- ❌ File locking can cause contention
- ❌ Overkill for simple key-value pairs

**Implementation Complexity:** MEDIUM (Otto already has SQLite)

**User API Complexity:** HIGH (SQL syntax)

**Example Usage:**
```bash
# Task A
sqlite3 $OTTO_DB "INSERT INTO task_data (task, key, value) VALUES ('task_a', 'key', 'value')"

# Task B
value=$(sqlite3 $OTTO_DB "SELECT value FROM task_data WHERE task='task_a' AND key='key'")
```

### 8. Hybrid: File-Based with Improved API

**Description**: Keep file-based approach but improve the user-facing API.

**Architecture:**
```
Task A: Native bash arrays or python dicts
Otto automatically: Serializes on task completion
Task B: Native bash arrays or python dicts (auto-loaded)
```

**Improvements over current:**
1. **Bash**: Use declare -A (associative arrays) instead of indexed arrays
2. **Python**: Use more intuitive __main__.OUTPUT instead of OTTO_OUTPUT
3. **Auto-loading**: Automatically load all dependencies without explicit calls
4. **Better naming**: Use natural variable names

**Pros:**
- ✅ Keeps all current benefits (debuggable, persistent, etc.)
- ✅ Minimal implementation changes
- ✅ Backward compatible (could support both APIs)
- ✅ Language-native feel

**Cons:**
- ❌ Still has serialization overhead
- ❌ Still limited to JSON-serializable types
- ❌ Bash associative arrays not available in Bash 3.2 (macOS default)

**Implementation Complexity:** LOW to MEDIUM

**User API Complexity:** LOW

**Example Usage (Improved Bash):**
```bash
# Task A - just use a regular associative array
declare -A OUTPUT
OUTPUT[key]="value"
OUTPUT[count]=42
# Otto automatically serializes on exit

# Task B - dependencies auto-loaded into INPUT
echo "Key from task_a: ${INPUT[task_a.key]}"
echo "Count: ${INPUT[task_a.count]}"
```

**Example Usage (Improved Python):**
```python
# Task A - just set variables
OUTPUT = {
    "key": "value",
    "count": 42
}
# Otto automatically serializes on exit

# Task B - dependencies auto-loaded
print(f"Key from task_a: {INPUT['task_a.key']}")
print(f"Count: {INPUT['task_a.count']}")
```

### 9. Stdin/Stdout Piping (Direct Connection)

**Description**: Directly connect task outputs to inputs using pipes, like Unix pipelines.

**Architecture:**
```
Otto spawns: Task A with stdout captured
Otto spawns: Task B with stdin connected to Task A's stdout
Data flows: Directly from A to B without intermediate storage
```

**Pros:**
- ✅ True streaming (data flows immediately)
- ✅ Zero intermediate storage
- ✅ Unix philosophy (do one thing well)
- ✅ Natural for many tasks

**Cons:**
- ❌ Only works for linear dependencies (A→B, not A→B and A→C)
- ❌ Can't inspect intermediate data
- ❌ No history/replay
- ❌ Tasks must complete in order (no parallelization)
- ❌ Binary protocols needed for structured data

**Implementation Complexity:** MEDIUM

**User API Complexity:** VERY LOW (standard I/O)

**Example Usage:**
```bash
# Task A - just print to stdout
echo '{"key": "value", "count": 42}'

# Task B - read from stdin
read -r json_data
# parse json_data...
```

## Comparative Analysis

### Performance Comparison

| Method | Latency | Throughput | Memory Usage | Disk I/O |
|--------|---------|------------|--------------|----------|
| Current (Files+Symlinks) | High (disk I/O) | Medium | Low | High |
| Memory-Mapped Files | Very Low | Very High | Medium | Low |
| Unix Domain Sockets | Low | High | Low | None |
| Named Pipes | Low | High | Low | None |
| Environment Variables | Very Low | Low | Very Low | None |
| Stdout Parsing | Low | Medium | Low | Low |
| Redis | Low | High | Medium | Low |
| SQLite | Medium | Medium | Low | Medium |
| Stdin/Stdout Pipes | Very Low | Very High | Low | None |

### Complexity Comparison

| Method | Implementation | User API | Debugging | Cross-Platform |
|--------|----------------|----------|-----------|----------------|
| Current (Files+Symlinks) | Medium | Medium | Easy | Excellent |
| Memory-Mapped Files | High | High | Hard | Fair |
| Unix Domain Sockets | Very High | Medium | Hard | Good |
| Named Pipes | Medium | Low | Hard | Good |
| Environment Variables | Low | Very Low | Easy | Excellent |
| Stdout Parsing | Medium | Medium | Easy | Excellent |
| Redis | High | Medium | Medium | Fair |
| SQLite | Medium | High | Easy | Excellent |
| Stdin/Stdout Pipes | Medium | Very Low | Medium | Excellent |

### Feature Comparison

| Method | Structured Data | Streaming | Persistent | Multi-Reader | Type Safety |
|--------|----------------|-----------|------------|--------------|-------------|
| Current (Files+Symlinks) | ✅ | ❌ | ✅ | ✅ | Partial |
| Memory-Mapped Files | ⚠️ | ⚠️ | ⚠️ | ✅ | ❌ |
| Unix Domain Sockets | ✅ | ✅ | ❌ | ✅ | ❌ |
| Named Pipes | ⚠️ | ✅ | ❌ | ❌ | ❌ |
| Environment Variables | ❌ | ❌ | ⚠️ | ✅ | ❌ |
| Stdout Parsing | ⚠️ | ✅ | ✅ | ✅ | ❌ |
| Redis | ✅ | ✅ | ✅ | ✅ | Partial |
| SQLite | ✅ | ❌ | ✅ | ✅ | ✅ |
| Stdin/Stdout Pipes | ⚠️ | ✅ | ⚠️ | ❌ | ❌ |

## Recommendations

### Short Term: Enhanced File-Based (Hybrid Approach #8)

**Rationale:**
- Preserves all current benefits (debuggability, persistence, history)
- Minimal implementation risk
- Significantly improves user experience
- Backward compatible

**Concrete Improvements:**

1. **Better Bash API** (when Bash 4+ available):
```bash
# Prologue auto-creates:
declare -A INPUT   # All dependencies pre-loaded
declare -A OUTPUT  # Empty, ready to use

# User writes naturally:
OUTPUT[result]="success"
OUTPUT[count]=42

# No more otto_set_output calls needed!
```

2. **Better Python API**:
```python
# Prologue auto-creates:
INPUT = {}   # All dependencies pre-loaded (dict)
OUTPUT = {}  # Empty dict

# User writes naturally:
OUTPUT["result"] = "success"
OUTPUT["count"] = 42

# No more __main__.OTTO_OUTPUT
```

3. **Auto-loading dependencies**:
- Current: Must call `otto_deserialize_input "task_a"` manually
- Improved: Otto prologue automatically loads ALL dependencies
- User just accesses `INPUT[task_a.key]` directly

4. **Better error messages**:
- Detect when user forgets to set OUTPUT
- Warn if OUTPUT contains non-JSON-serializable data
- Show friendly errors for missing dependencies

### Medium Term: Add Environment Variable Option

**Rationale:**
- Extremely simple for users
- Works for 80% of use cases (small data, simple strings)
- Can coexist with file-based approach
- Zero learning curve

**Implementation:**
```yaml
tasks:
  task_a:
    bash: |
      # Simple option: just export with OTTO_OUT_ prefix
      export OTTO_OUT_VERSION="1.2.3"
      export OTTO_OUT_STATUS="success"

  task_b:
    before: [task_a]
    bash: |
      # Otto automatically propagates as OTTO_IN_TASK_A_*
      echo "Version: $OTTO_IN_TASK_A_VERSION"
      echo "Status: $OTTO_IN_TASK_A_STATUS"
```

**Limitations to document:**
- Size limits (keep under 4KB per variable)
- String-only (use files for structured data)
- Not persistent (only available during run)

### Long Term: Consider Streaming with Named Pipes

**Rationale:**
- Enables true streaming workflows
- Natural for log processing, data transformation
- Can coexist with file-based for structured data

**Use Case Example:**
```yaml
tasks:
  generate_data:
    streaming: true  # New flag
    bash: |
      # Generates continuous stream of data
      for i in {1..1000}; do
        echo "{\"id\": $i, \"data\": \"...\"}"
      done
      # Writes to $OTTO_OUTPUT_STREAM (FIFO)

  process_data:
    before: [generate_data]
    streaming: true
    bash: |
      # Reads from $OTTO_INPUT_STREAM (FIFO connected to generate_data)
      while read -r line; do
        # Process each line as it arrives
        echo "Processing: $line"
      done
```

## Conclusion

The current file-based approach is fundamentally sound and should be retained. The main issues are:

1. **API ergonomics** - helper functions feel magic
2. **Bash 3.2 compatibility** - forces awkward indexed arrays
3. **Manual dependency loading** - requires explicit deserialization calls

These can all be addressed with **Hybrid Approach #8** (Enhanced File-Based) without abandoning the proven benefits of the current architecture.

More exotic IPC methods (mmap, sockets, Redis) add complexity without clear benefits for Otto's use case. The file-based approach aligns perfectly with Otto's philosophy:

- **Transparency**: Users can inspect `.otto/` directory
- **Debuggability**: JSON files are human-readable
- **History**: Data persists for replay and analysis
- **Simplicity**: No daemons, no dependencies, just files

## Next Steps

1. **Prototype Enhanced API** (Hybrid #8)
   - Implement improved bash prologue/epilogue
   - Add auto-loading of dependencies
   - Test with bash 3.2, 4.x, 5.x

2. **User Testing**
   - Get feedback on new API
   - Document limitations and gotchas
   - Create migration guide

3. **Environment Variable Option** (Quick Win)
   - Implement OTTO_OUT_*/OTTO_IN_* pattern
   - Document when to use files vs env vars
   - Add to examples

4. **Explore Streaming** (Future)
   - Research named pipe integration
   - Identify streaming use cases
   - Design API for streaming tasks
