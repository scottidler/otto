# Data Passing Ergonomics - Complete Comparison

## The Problem

**Current approach feels awkward:**
```bash
otto_set_output "key" "value"           # Not intuitive
value=$(otto_get_input "task_a.key")    # Must remember function names
otto_deserialize_input "task_a"         # Manual loading step
```

This document explores ALL possible solutions for better ergonomics.

---

## Option 1: Current State (What Exists Today)

### Implementation
- ✅ **Exists today** in `src/executor/action.rs`
- Bash functions in `builtins.sh`
- Python functions in `builtins.py`
- Writes to JSON files: `output.<task>.json`
- Reads from symlinked files: `input.<dep>.json`

### Bash Example
```bash
tasks:
  task_a:
    bash: |
      # Set outputs
      otto_set_output "version" "1.2.3"
      otto_set_output "status" "success"

  task_b:
    before: [task_a]
    bash: |
      # Must manually load dependency
      otto_deserialize_input "task_a"

      # Get inputs
      version=$(otto_get_input "task_a.version")
      status=$(otto_get_input "task_a.status")

      echo "Version: $version"
```

### Python Example
```python
tasks:
  task_a:
    python3: |
      # Set outputs
      otto_set_output("version", "1.2.3")
      otto_set_output("status", "success")

  task_b:
    before: [task_a]
    python3: |
      # Must manually load dependency
      otto_deserialize_input("task_a")

      # Get inputs
      version = otto_get_input("task_a.version")
      status = otto_get_input("task_a.status")

      print(f"Version: {version}")
```

### Pros
- ✅ Works today
- ✅ JSON files are debuggable
- ✅ Language-agnostic
- ✅ No dependencies

### Cons
- ❌ Function names feel "magic"
- ❌ Must manually deserialize dependencies
- ❌ Not using native language constructs
- ❌ Verbose

### Backend
```
task_a runs
  ↓
otto_set_output "version" "1.2.3"
  ↓
OTTO_OUTPUT array: ["version=1.2.3", "status=success"]
  ↓
otto_serialize_output (epilogue)
  ↓
Write to: ~/.otto/.../tasks/task_a/output.task_a.json
  ↓
Otto creates symlink: tasks/task_b/input.task_a.json → ../task_a/output.task_a.json
  ↓
task_b runs
  ↓
otto_deserialize_input "task_a"
  ↓
Read from: input.task_a.json
  ↓
OTTO_INPUT array: ["task_a.version=1.2.3", "task_a.status=success"]
  ↓
otto_get_input "task_a.version" returns "1.2.3"
```

---

## Option 2: Native Language Constructs (Proposed in My Docs)

### Implementation
- ❌ **Doesn't exist yet** - needs to be built
- Modify prologue generation in `src/executor/action.rs`
- Use native bash associative arrays (Bash 4+)
- Use native python dicts
- Still writes to JSON files (keeps debuggability)
- Auto-load all dependencies in prologue

### Bash Example
```bash
tasks:
  task_a:
    bash: |
      # Use native bash associative array
      OUTPUT[version]="1.2.3"
      OUTPUT[status]="success"

  task_b:
    before: [task_a]
    bash: |
      # Dependencies auto-loaded! No manual step!
      # INPUT array already populated by prologue

      echo "Version: ${INPUT[task_a.version]}"
      echo "Status: ${INPUT[task_a.status]}"
```

### Python Example
```python
tasks:
  task_a:
    python3: |
      # Use native python dict
      OUTPUT["version"] = "1.2.3"
      OUTPUT["status"] = "success"

  task_b:
    before: [task_a]
    python3: |
      # Dependencies auto-loaded!
      # INPUT dict already populated by prologue

      print(f"Version: {INPUT['task_a.version']}")
      print(f"Status: {INPUT['task_a.status']}")
```

### Pros
- ✅ Native language syntax (feels natural)
- ✅ Auto-loading (no manual deserialize)
- ✅ Still uses JSON files (debuggable)
- ✅ Shorter, cleaner code
- ✅ Discoverable (looks like normal bash/python)

### Cons
- ❌ Bash 4+ only for associative arrays
- ❌ Need fallback for Bash 3.2 (macOS)
- ❌ Breaking change (old code won't work)
- ⚠️  Could support both old and new API

### Backend
```
Same as Option 1, but:
  - Prologue auto-loads ALL dependencies
  - Uses declare -A INPUT/OUTPUT in bash
  - Uses native dicts in python
  - Still writes to JSON files
```

### Implementation Effort
- **2-3 days** to implement
- Modify bash prologue generator
- Modify python prologue generator
- Add auto-loading logic
- Keep backward compatibility

---

## Option 3: Environment Variables (Simple Cases)

### Implementation
- ❌ **Doesn't exist yet** - needs to be built
- Scan for `OTTO_*` exports after task completion
- Auto-propagate to dependent tasks
- Best for simple string values

### Example
```bash
tasks:
  task_a:
    bash: |
      # Super simple - just export!
      export OTTO_VERSION="1.2.3"
      export OTTO_STATUS="success"
      export OTTO_BUILD_DIR="/tmp/build"

  task_b:
    before: [task_a]
    bash: |
      # Auto-propagated by Otto!
      echo "Version: $OTTO_TASK_A_VERSION"
      echo "Status: $OTTO_TASK_A_STATUS"
      echo "Build dir: $OTTO_TASK_A_BUILD_DIR"
```

### Pros
- ✅ Extremely simple (zero learning curve)
- ✅ Native to all shells
- ✅ Fast (no I/O)
- ✅ Works across exec calls

### Cons
- ❌ Size limits (4KB per var, 128KB total on Linux)
- ❌ String-only (no objects/arrays)
- ❌ Environment pollution
- ❌ Security concerns (visible in `ps`)

### Backend
```
task_a runs
  ↓
export OTTO_VERSION="1.2.3"
  ↓
Otto epilogue captures all OTTO_* variables
  ↓
Writes to output.task_a.json (for history/debugging)
  ↓
task_b prologue injects as OTTO_TASK_A_* variables
  ↓
User accesses $OTTO_TASK_A_VERSION
```

### Implementation Effort
- **1 day** to implement
- Scan env vars in epilogue
- Inject in prologue of dependent tasks
- Add to docs with size limits warning

---

## Option 4: SQLite Database (New Idea!)

### Implementation
- ❌ **Doesn't exist** - would need to be built
- Otto ALREADY has SQLite at `~/.otto/otto.db`
- Could add a `task_data` table
- Build helper CLI tools: `otto-data-set`, `otto-data-get`
- Or add subcommands: `otto data set`, `otto data get`

### Database Schema (New Table)
```sql
CREATE TABLE task_data (
    id INTEGER PRIMARY KEY,
    task_id INTEGER NOT NULL,
    key TEXT NOT NULL,
    value TEXT NOT NULL,  -- JSON-encoded
    FOREIGN KEY (task_id) REFERENCES tasks(id) ON DELETE CASCADE
);

CREATE INDEX idx_task_data_lookup ON task_data(task_id, key);
```

### Example with CLI Tools
```bash
tasks:
  task_a:
    bash: |
      # Write to database
      otto data set version "1.2.3"
      otto data set status "success"
      otto data set config '{"port": 8080, "host": "localhost"}'

  task_b:
    before: [task_a]
    bash: |
      # Read from database
      version=$(otto data get task_a version)
      status=$(otto data get task_a status)
      config=$(otto data get task_a config)

      echo "Version: $version"
      echo "Config: $config" | jq .port
```

### Example with Library Import
```python
tasks:
  task_a:
    python3: |
      from otto.data import set_output, get_input

      # Write to database
      set_output("version", "1.2.3")
      set_output("status", "success")
      set_output("config", {"port": 8080, "host": "localhost"})

  task_b:
    before: [task_a]
    python3: |
      from otto.data import get_input

      # Read from database
      version = get_input("task_a", "version")
      status = get_input("task_a", "status")
      config = get_input("task_a", "config")  # Returns dict

      print(f"Version: {version}")
      print(f"Port: {config['port']}")
```

### Pros
- ✅ Database already exists (no new dependency)
- ✅ Queryable (can do complex queries)
- ✅ ACID transactions
- ✅ Can store structured data natively
- ✅ Automatically in history
- ✅ Can query across runs

### Cons
- ❌ **NOT debuggable** (can't `cat` database)
- ❌ **Requires tools** (sqlite3 or custom CLI)
- ❌ **Breaks graceful degradation** (DB required)
- ❌ More complex than files
- ❌ Goes against Otto's design principles

### Backend
```
task_a runs
  ↓
otto data set version "1.2.3"
  ↓
Writes to SQLite:
  INSERT INTO task_data (task_id, key, value)
  VALUES (42, 'version', '"1.2.3"')
  ↓
task_b runs
  ↓
otto data get task_a version
  ↓
Reads from SQLite:
  SELECT value FROM task_data
  WHERE task_id = (SELECT id FROM tasks WHERE name='task_a' AND run_id=...)
    AND key = 'version'
  ↓
Returns: "1.2.3"
```

### Implementation Effort
- **3-5 days** to implement
- Add `task_data` table schema
- Build `otto data set/get` subcommands
- Or build standalone tools
- Add Python library
- Migrate existing file-based API

---

## Option 5: Hybrid (Files + Database)

### Implementation
- ❌ **Doesn't exist** - most complex option
- Write to BOTH JSON files AND SQLite
- Keep debuggability of files
- Add queryability of database
- Best of both worlds, but most complexity

### Example
```bash
tasks:
  task_a:
    bash: |
      # Single API writes to both
      OUTPUT[version]="1.2.3"
      OUTPUT[status]="success"

      # Behind the scenes:
      # 1. Writes to output.task_a.json
      # 2. Also inserts into task_data table
```

### Pros
- ✅ Debuggable (files still exist)
- ✅ Queryable (database has it too)
- ✅ Backward compatible
- ✅ Graceful degradation possible

### Cons
- ❌ Highest complexity
- ❌ Duplicate storage
- ❌ Sync issues (what if DB write fails?)
- ❌ Performance overhead (double writes)

### Implementation Effort
- **5-7 days**
- Most complex option
- Need to handle sync failures

---

## Recommendation Matrix

### For Simple Use Cases (80% of users)

```yaml
# Current (verbose)
otto_set_output "version" "1.2.3"
value=$(otto_get_input "task_a.version")

# Proposed: Environment Variables ⭐ BEST
export OTTO_VERSION="1.2.3"
echo "$OTTO_TASK_A_VERSION"
```

**Winner: Option 3 (Environment Variables)**
- Simplest possible
- Zero learning curve
- 1 day to implement

### For Structured Data (20% of users)

```yaml
# Current (awkward)
otto_set_output "config" '{"port":8080}'
config=$(otto_get_input "task_a.config")

# Proposed: Native Constructs ⭐ BEST
OUTPUT[config]='{"port":8080}'
config="${INPUT[task_a.config]}"
```

**Winner: Option 2 (Native Constructs)**
- Still uses files (debuggable)
- Natural syntax
- 2-3 days to implement

### For Power Users (5% of users)

```bash
# Need complex queries across runs?
# Then SQLite makes sense:
otto data query "SELECT key, value FROM task_data
  WHERE task_id IN (SELECT id FROM tasks WHERE name='build')
  ORDER BY id DESC LIMIT 10"
```

**Winner: Option 4 (SQLite) - But only if you need queries**

---

## My Recommendation

### Phase 1: Environment Variables (1 day)
```bash
export OTTO_VERSION="1.2.3"  # Simple cases
```

### Phase 2: Native Constructs (2-3 days)
```bash
OUTPUT[key]="value"           # Complex cases
```

### Phase 3: Keep Files Forever
- Don't switch to SQLite for data passing
- Files are debuggable, SQLite is not
- Goes against Otto's design principles

### Optional: Add SQLite Queries (later)
```bash
# Only for power users who need it
otto data query "SELECT ..."  # Query historical data
```

---

## Complete Code Comparison

### Scenario: Pass build version and config to deploy task

**Option 1: Current (Exists Today)**
```yaml
build:
  bash: |
    version="1.2.3"
    config='{"port": 8080, "ssl": true}'

    otto_set_output "version" "$version"
    otto_set_output "config" "$config"

deploy:
  before: [build]
  bash: |
    otto_deserialize_input "build"

    version=$(otto_get_input "build.version")
    config=$(otto_get_input "build.config")
    port=$(echo "$config" | jq -r .port)

    echo "Deploying version $version on port $port"
```

**Option 2: Native Constructs (Proposed)**
```yaml
build:
  bash: |
    OUTPUT[version]="1.2.3"
    OUTPUT[config]='{"port": 8080, "ssl": true}'

deploy:
  before: [build]
  bash: |
    # Auto-loaded!
    version="${INPUT[build.version]}"
    config="${INPUT[build.config]}"
    port=$(echo "$config" | jq -r .port)

    echo "Deploying version $version on port $port"
```

**Option 3: Environment Variables (Simple)**
```yaml
build:
  bash: |
    export OTTO_VERSION="1.2.3"
    export OTTO_PORT="8080"
    export OTTO_SSL="true"

deploy:
  before: [build]
  bash: |
    # Auto-propagated!
    echo "Deploying version $OTTO_BUILD_VERSION on port $OTTO_BUILD_PORT"
```

**Option 4: SQLite Database (New)**
```yaml
build:
  bash: |
    otto data set version "1.2.3"
    otto data set config '{"port": 8080, "ssl": true}'

deploy:
  before: [build]
  bash: |
    version=$(otto data get build version)
    port=$(otto data get build config | jq -r .port)

    echo "Deploying version $version on port $port"
```

---

## Bottom Line

**You have OPTIONS:**

1. **Keep current** - Works, but verbose
2. **Add env vars** - Quick win for simple cases (1 day)
3. **Native syntax** - Better for complex data (2-3 days)
4. **SQLite** - Only if you need queries (3-5 days, loses debuggability)

**I recommend: Option 3 (env vars) + Option 2 (native) together**
- Use env vars for simple strings
- Use native constructs for structured data
- Keep files for debugging
- Skip SQLite for data passing (use it for queries only)

This gives you the best ergonomics while maintaining all the benefits of the current file-based approach.
