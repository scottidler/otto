# SQLite Integration Architecture

Otto uses a hybrid storage model: SQLite for metadata that `History`/`Stats`/`Clean` query, filesystem for the artifacts a run actually produces. Schema regenerated from `src/executor/state/schema.rs` (`SCHEMA_VERSION = 5`); every claim below that isn't a direct read of the schema was checked against the code that implements it.

## Design Principles

### What stays on the filesystem
- **Scripts** (`.cache/<hash>.sh`) - inspectable with `cat`
- **Logs** (`stdout.log`, `stderr.log`) - tailable with `tail -f`
- **Outputs** (`output.<task>.json`/`.env`) - parseable with standard tools
- **`run.yaml`** - the serialized `ExecutionContext` for a run

### What goes in the database
- Run and task metadata (timestamp, status, duration, user, hostname, exit code, sizes)
- Task skip provenance (`skip_reason`, `skip_kind`)
- Paths to the filesystem artifacts above (not their content)
- Project identity (hash, name, first/last seen, run count)

### Non-negotiables
- Scripts and logs remain plain files, not blobs in the database
- The database is optional: `StateManager::try_new()` returns `None` (logging a warning) rather than panicking when it can't open, and every caller (`History`, `Stats`, run recording) degrades gracefully rather than erroring
- `Clean` works without a database at all (`--no-db`, filesystem-scan mode)

## Database Schema

### `schema_version`

```sql
CREATE TABLE schema_version (
    version INTEGER PRIMARY KEY,
    applied_at INTEGER NOT NULL
)
```

### `projects`

```sql
CREATE TABLE projects (
    id INTEGER PRIMARY KEY,
    hash TEXT NOT NULL UNIQUE,
    name TEXT,
    ottofile_path TEXT,
    first_seen INTEGER NOT NULL,
    last_seen INTEGER NOT NULL,
    run_count INTEGER DEFAULT 0
)
```

Indexes: `hash` (unique), `name` (`idx_projects_name`).

### `runs`

```sql
CREATE TABLE runs (
    id INTEGER PRIMARY KEY,
    project_id INTEGER NOT NULL,
    timestamp INTEGER NOT NULL,
    status TEXT NOT NULL,              -- 'running' | 'success' | 'failed'
    duration_seconds REAL,
    size_bytes INTEGER,
    ottofile_path TEXT,
    cwd TEXT,
    user TEXT,
    hostname TEXT,
    args TEXT,                         -- JSON-serialized argv
    ended_at INTEGER,
    run_dir TEXT,                      -- the directory this run actually wrote into
    FOREIGN KEY (project_id) REFERENCES projects(id) ON DELETE CASCADE
)
```

Indexes: `timestamp`, `status`, `project_id`. **No unique constraint on `timestamp`** — it has one-second resolution, so two runs starting in the same second are ordinary, not a conflict (schema v5 dropped a `UNIQUE(timestamp)` that used to silently lose the second run's row). `run_dir` is `NULL` for rows written before v5; there is no back-fill, so `History`'s `run_dir` JSON field is `null` for those.

### `tasks`

```sql
CREATE TABLE tasks (
    id INTEGER PRIMARY KEY,
    run_id INTEGER NOT NULL,
    name TEXT NOT NULL,
    status TEXT NOT NULL,              -- 'pending' | 'running' | 'completed' | 'failed' | 'skipped'
    script_hash TEXT,
    exit_code INTEGER,
    started_at INTEGER,
    ended_at INTEGER,
    duration_seconds REAL,
    stdout_path TEXT,                  -- absolute
    stderr_path TEXT,                  -- absolute
    script_path TEXT,                  -- absolute
    skip_reason TEXT,
    skip_kind TEXT,                    -- 'up-to-date' | 'serial-predecessor' | 'unreachable'
    FOREIGN KEY (run_id) REFERENCES runs(id) ON DELETE CASCADE
)
```

Indexes: `run_id`, `name`, `status`, `(name, run_id)` composite. `stdout_path`/`stderr_path`/`script_path` are **absolute paths**, not relative to `$OTTO_HOME` — a run recorded from one `$OTTO_HOME` is still resolvable if read from a different one. `script_hash` hashes the fully-rendered script, including otto's own injected builtins prologue, so upgrading otto and touching that prologue changes every task's `script_hash` even when the ottofile is untouched. `skip_reason`/`skip_kind` are `NULL` on rows written before schema v3/v4 respectively.

## Migrations

`init_schema` creates the tables above fresh (schema v5, no migration needed). An existing database walks forward one version at a time via `migrate_v1_to_v2` (adds and backfills `projects.name`), `migrate_v2_to_v3` (adds `tasks.skip_reason`), `migrate_v3_to_v4` (adds `tasks.skip_kind`), and `migrate_v4_to_v5` (rebuilds `runs` to drop `UNIQUE(timestamp)` and add `run_dir`; SQLite can't drop a constraint in place, so this one rebuilds the table under a transaction). Every migration is idempotent — re-running one after a crash between its `ALTER`/rebuild and the version bump is a no-op, not an error.

There is no rollback: a migration only goes forward. There is no separate `migrations.rs`-level health check either — `DatabaseManager` is a single `Mutex<Connection>`, not a connection pool, and there is no periodic integrity check or automatic in-memory fallback on corruption. A corrupted database is a hard error from `StateManager::new()`, which `try_new()` turns into "no history/stats today", not into a substitute in-memory store.

## Component Architecture

### `StateManager` (`src/executor/state/manager.rs`)

Wraps a `DatabaseManager` and exposes the operations the CLI commands use:

**Recording**: `record_run_start`, `record_run_complete`, `record_task_start`, `record_task_complete`, `record_task_skipped`

**Querying**: `get_runs_with_filters` (status/project/limit — what `History` calls with no `[TASK]`), `get_task_history` (what `History <task>` calls), `get_overall_stats`, `get_task_stats`, `get_all_task_stats` (the Top-N table in `Stats`)

**Cleanup**: `find_old_runs`, `delete_run`

There is no `get_recent_runs` and no `get_run_tasks` — `get_recent_runs` was deleted once `get_runs_with_filters` (which also handles the unfiltered case) replaced it as `History`'s only caller.

### `DatabaseManager` (`src/executor/state/db.rs`)

```rust
pub struct DatabaseManager {
    conn: Mutex<Connection>,
}
```

One connection, behind a mutex — not a pool. On open it retries the `journal_mode=WAL` pragma (SQLite can refuse it transiently under concurrent openers), then sets `synchronous=NORMAL` and `foreign_keys=ON`. WAL plus `synchronous=NORMAL` means a crash can lose the last commit but never corrupts the file.

## Data Flow

### Run execution

```
otto <tasks>
  -> Workspace::init() creates <project>-<hash>/<timestamp>/, writes run.yaml
  -> StateManager::record_run_start() upserts the project row, inserts the run row (status: running)
  -> per task: record_task_start() / record_task_complete() / record_task_skipped()
  -> StateManager::record_run_complete() computes run_dir size, sets status: success/failed
```

### History query

```
otto History            -> StateManager::get_runs_with_filters(status, project, limit)
otto History <task>      -> StateManager::get_task_history(task, limit)
```

Both are plain indexed `SELECT`s against `runs`/`tasks`; there is no separate "recent runs" code path.

### Cleanup

```
otto Clean --keep-days D --keep-last N --keep-failed F
  -> StateManager::find_old_runs(D, N, F, project_filter)
  -> per run: StateManager::delete_run(id, delete_filesystem: true)
       - deletes the run row (cascades to its tasks via ON DELETE CASCADE)
       - removes run_dir from disk if it still exists
```

## Filesystem Layout

```
$OTTO_HOME/                       # default $HOME/.otto
├── otto.db                       # SQLite database
├── <project-name>-<hash>/        # per-project directory
│   ├── .cache/                   # rendered scripts, flat, named by content hash
│   ├── <timestamp>/              # one run
│   │   ├── run.yaml
│   │   └── tasks/<task-name>/{script.sh -> ../../../.cache/<hash>.sh, builtins.sh, stdout.log, stderr.log, output.<task>.json, output.<task>.env}
│   └── ...
```

There is no top-level `.cache/` under `$OTTO_HOME` — the cache is per-project. Full detail, including the `input.<dep>.{json,env}` files a task with dependencies gets, is in [`docs/directory-layout.md`](../directory-layout.md).

## Error Handling

- **Database unavailable**: `StateManager::try_new()` logs a warning and returns `None`. `History`/`Stats` print `No {history,statistics} database found. Run otto to create it.` and exit 0. `Clean` prints `Database not available, falling back to filesystem scan...` and switches to `--no-db` behavior.
- **`--no-db` / filesystem fallback**: `Clean` walks `$OTTO_HOME` directly. It cannot see run status (that only exists in the database), so `--keep-failed` is applied as the *longer* of the two cutoffs for every run in this mode, with a printed warning — it keeps more than asked rather than deleting something the flag meant to protect.
- **Database corruption**: a hard error surfaces from `StateManager::new()`; there is no automatic repair or in-memory substitute. Manual recovery is `rm $OTTO_HOME/otto.db` — the next run recreates an empty one.

## Related Documentation

- [History Command](../commands/history.md)
- [Stats Command](../commands/stats.md)
- [Clean Command](../commands/clean.md)
- [Directory Layout](../directory-layout.md)
