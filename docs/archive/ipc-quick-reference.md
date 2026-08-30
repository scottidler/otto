# IPC Methods Quick Reference Card

## One-Page Cheat Sheet for Inter-Process Communication

### 1. Files + Symlinks (Current - ✅ RECOMMENDED)

**What:** JSON files in task directories, symlinked between tasks

**Usage:**
```bash
# Bash
OUTPUT[key]="value"
echo "${INPUT[task_a.key]}"

# Python
OUTPUT["key"] = "value"
print(INPUT["task_a.key"])
```

**Metrics:**
- Speed: ⭐⭐⭐ (Medium)
- Simple: ⭐⭐⭐⭐⭐ (Very Easy)
- Debug: ⭐⭐⭐⭐⭐ (Cat files!)
- Persist: ⭐⭐⭐⭐⭐ (History)

**Use When:** Standard task data passing (default choice)

---

### 2. Environment Variables

**What:** OS environment variables (OTTO_* prefix)

**Usage:**
```bash
# Export from task A
export OTTO_VERSION="1.2.3"
export OTTO_STATUS="success"

# Access in task B (auto-propagated)
echo "$OTTO_TASK_A_VERSION"
```

**Metrics:**
- Speed: ⭐⭐⭐⭐⭐ (Very Fast)
- Simple: ⭐⭐⭐⭐⭐ (Extremely Easy)
- Debug: ⭐⭐⭐⭐ (env command)
- Persist: ⭐⭐ (Otto must capture)

**Limits:** 4KB per var, strings only, ~128KB total
**Use When:** Simple string values, config flags

---

### 3. Named Pipes (FIFOs)

**What:** Filesystem pipes for streaming

**Usage:**
```bash
# Producer
echo '{"id":1}' > "$OTTO_OUTPUT_PIPE"
echo '{"id":2}' > "$OTTO_OUTPUT_PIPE"

# Consumer
while read -r line; do
  echo "Got: $line"
done < "$OTTO_INPUT_PIPE"
```

**Metrics:**
- Speed: ⭐⭐⭐⭐ (Fast)
- Simple: ⭐⭐⭐ (Blocking issues)
- Debug: ⭐ (Ephemeral)
- Persist: ⭐ (Gone after read)

**Use When:** Streaming data, real-time processing

---

### 4. Unix Domain Sockets

**What:** Local socket connections

**Usage:**
```bash
# Requires Otto server + client
echo "SET key value" | nc -U $OTTO_SOCKET
value=$(echo "GET key" | nc -U $OTTO_SOCKET)
```

**Metrics:**
- Speed: ⭐⭐⭐⭐ (Fast)
- Simple: ⭐⭐ (Protocol needed)
- Debug: ⭐⭐ (Network tools)
- Persist: ⭐ (In-memory)

**Use When:** Real-time coordination, pub/sub

---

### 5. Memory-Mapped Files

**What:** Shared memory regions

**Usage:**
```bash
# Requires C/Rust helper tools
otto-mmap-set task_a key value
value=$(otto-mmap-get task_a key)
```

**Metrics:**
- Speed: ⭐⭐⭐⭐⭐ (Extremely Fast)
- Simple: ⭐ (Very Complex)
- Debug: ⭐ (Binary format)
- Persist: ⭐⭐⭐ (File-backed)

**Use When:** Huge datasets (>100MB), zero-copy needs

---

### 6. Stdout/Stdin Pipes

**What:** Direct process pipelines

**Usage:**
```bash
# Task A stdout → Task B stdin
# In otto.yml (hypothetical):
tasks:
  producer:
    bash: echo '{"key":"value"}'
  consumer:
    stdin_from: producer
    bash: read -r data && echo "$data"
```

**Metrics:**
- Speed: ⭐⭐⭐⭐⭐ (Zero-copy)
- Simple: ⭐⭐⭐⭐⭐ (Shell pipes!)
- Debug: ⭐⭐ (Can't inspect)
- Persist: ⭐ (Ephemeral)

**Limits:** Linear dependencies only (A→B, not A→B+C)
**Use When:** Sequential processing pipelines

---

### 7. Stdout Parsing with Markers

**What:** Special tags in output

**Usage:**
```bash
echo "Regular output"
echo "<OTTO:OUT:key>value</OTTO:OUT:key>"
echo "More output"
```

**Metrics:**
- Speed: ⭐⭐⭐ (Parsing overhead)
- Simple: ⭐⭐⭐ (Remember format)
- Debug: ⭐⭐⭐⭐ (In logs)
- Persist: ⭐⭐⭐⭐ (In logs)

**Issues:** Clutters output, fragile parsing
**Use When:** Language-agnostic output capture

---

### 8. Redis

**What:** In-memory database

**Usage:**
```bash
redis-cli -s $OTTO_REDIS SET task:key value
value=$(redis-cli -s $OTTO_REDIS GET task:key)
```

**Metrics:**
- Speed: ⭐⭐⭐⭐ (Fast)
- Simple: ⭐⭐⭐ (Learn Redis)
- Debug: ⭐⭐⭐⭐ (Redis CLI)
- Persist: ⭐⭐⭐⭐ (Snapshots)

**Requires:** External Redis installation
**Use When:** Complex data structures, pub/sub

---

### 9. SQLite

**What:** Embedded SQL database

**Usage:**
```bash
sqlite3 $OTTO_DB "INSERT INTO kv VALUES ('k','v')"
value=$(sqlite3 $OTTO_DB "SELECT v FROM kv WHERE k='k'")
```

**Metrics:**
- Speed: ⭐⭐⭐ (DB overhead)
- Simple: ⭐⭐ (SQL syntax)
- Debug: ⭐⭐⭐⭐⭐ (sqlite3 CLI)
- Persist: ⭐⭐⭐⭐⭐ (DB file)

**Note:** Otto already uses SQLite for history!
**Use When:** Complex queries, transactions

---

## Decision Matrix

### Choose Files + JSON If:
- ✅ Standard data passing between tasks
- ✅ Need debugging/inspection
- ✅ Want persistent history
- ✅ Data size < 10MB
- ✅ Not time-critical

### Choose Environment Variables If:
- ✅ Simple strings only
- ✅ Very small data (< 4KB)
- ✅ Config flags or parameters
- ✅ Want dead-simple API

### Choose Named Pipes If:
- ✅ Streaming data
- ✅ Real-time processing
- ✅ Large data that doesn't fit in memory
- ✅ Producer/consumer pattern

### Choose Unix Sockets If:
- ✅ Need bidirectional communication
- ✅ Real-time coordination
- ✅ Pub/sub pattern
- ✅ Multiple consumers

### Choose Memory-Mapped Files If:
- ✅ Huge datasets (> 100MB)
- ✅ Performance critical
- ✅ Need zero-copy
- ❌ (Rarely needed for Otto)

---

## Comparison Table

| Method | Speed | Simple | Debug | Persist | Size Limit | Bash/Py Friendly |
|--------|-------|--------|-------|---------|------------|------------------|
| **Files + JSON** | ⭐⭐⭐ | ⭐⭐⭐⭐⭐ | ⭐⭐⭐⭐⭐ | ⭐⭐⭐⭐⭐ | None | ✅✅ |
| Env Vars | ⭐⭐⭐⭐⭐ | ⭐⭐⭐⭐⭐ | ⭐⭐⭐⭐ | ⭐⭐ | 4KB | ✅✅ |
| Named Pipes | ⭐⭐⭐⭐ | ⭐⭐⭐ | ⭐ | ⭐ | None | ✅✅ |
| Unix Sockets | ⭐⭐⭐⭐ | ⭐⭐ | ⭐⭐ | ⭐ | None | ⭐⭐ |
| mmap | ⭐⭐⭐⭐⭐ | ⭐ | ⭐ | ⭐⭐⭐ | Pre-alloc | ❌ |
| Stdout Pipes | ⭐⭐⭐⭐⭐ | ⭐⭐⭐⭐⭐ | ⭐⭐ | ⭐ | None | ✅✅ |
| Stdout Parse | ⭐⭐⭐ | ⭐⭐⭐ | ⭐⭐⭐⭐ | ⭐⭐⭐⭐ | None | ✅✅ |
| Redis | ⭐⭐⭐⭐ | ⭐⭐⭐ | ⭐⭐⭐⭐ | ⭐⭐⭐⭐ | None | ⭐⭐ |
| SQLite | ⭐⭐⭐ | ⭐⭐ | ⭐⭐⭐⭐⭐ | ⭐⭐⭐⭐⭐ | None | ⭐⭐ |

---

## Otto's Current Choice: Files + JSON ✅

**Why it's correct:**
1. Debuggable - `cat ~/.otto/.../tasks/task_a/output.task_a.json`
2. Persistent - Full history for replay
3. Simple - No daemons, no dependencies
4. Language-agnostic - Works with any language
5. Visible - Symlinks show data flow

**What to improve:**
1. API ergonomics (use native bash/python syntax)
2. Auto-load dependencies (no manual calls)
3. Better error messages
4. Documentation in generated scripts

**See:** `docs/improved-data-passing-proposal.md` for implementation

---

## When to Use Advanced IPC

### Streaming (Named Pipes)
```yaml
# Log processing pipeline
tasks:
  tail_logs:
    streaming: true
    bash: tail -f /var/log/app.log

  parse_errors:
    before: [tail_logs]
    streaming: true
    bash: grep ERROR
```

### Real-time (Unix Sockets)
```yaml
# Live progress updates
tasks:
  long_task:
    bash: |
      for i in {1..100}; do
        otto-progress $i 100 "Processing..."
        # Otto shows in TUI
      done
```

### Huge Data (mmap)
```yaml
# 1GB dataset, zero-copy
tasks:
  process_large_file:
    bash: |
      # Otto provides mmap region
      otto-mmap-process $INPUT_MMAP $OUTPUT_MMAP
```

---

## Bottom Line

- **95% of cases:** Use files + JSON (current approach)
- **Simple strings:** Consider environment variables
- **Streaming:** Use named pipes (future feature)
- **Real-time:** Use Unix sockets (future feature)
- **Everything else:** Probably don't need it

**Focus on improving the API, not the mechanism.**
