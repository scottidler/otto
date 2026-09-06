# `otto History` - View Execution History

The `History` command displays a chronological record of Otto runs, or (with a task name) the execution history of one task across every run.

> **Note**: Built-in commands are capitalized (e.g., `History`, `Stats`, `Clean`) to avoid namespace conflicts with user-defined tasks.

Regenerated from `otto History --help` and a scratch run's `otto History --json`; every example below is observed output, not illustrative.

## Usage

```bash
otto History [OPTIONS] [TASK]
```

## Arguments

| Argument | Description |
|----------|-------------|
| `[TASK]` | Show history for this task only, across all runs. Positional, not a flag. |

## Options

```
$ otto History --help
Show execution history

Usage: otto History [OPTIONS] [TASK]

Arguments:
  [TASK]  Show history for a specific task

Options:
  -n, --limit <LIMIT>      Limit number of results [default: 20]
  -s, --status <STATUS>    Filter by status (success, failed, running) [possible values: success, failed, running]
  -p, --project <PROJECT>  Filter by project hash
      --json               Output as JSON
  -h, --help               Print help
```

## Examples

### Recent runs

```bash
otto History
otto History --limit 50
otto History -n 5
```

### Filter by status or project

```bash
otto History --status failed
otto History -s running
otto History --project 70af3bf4
```

### Task-specific history

```bash
otto History hello
otto History hello --limit 10
```

There is no dedicated task-selection flag: the task name is a bare positional argument.

### JSON output

```bash
otto History --json | jq '.[] | select(.status == "Failed")'
```

## Output Format

### Run history table

No `[TASK]` given:

```
Timestamp            Status  Duration     Size  User     Path
─────────────────────────────────────────────────────────────────────────────
2026-09-03 05:23:53    ✗          0ms  10.3 KB  saidler  ~/repos/otto-rs/otto
2026-09-03 05:23:53    ✓          0ms  10.4 KB  saidler  ~/repos/otto-rs/otto

Total runs: 2
```

**Columns:**
- **Timestamp**: When the run started, local time
- **Status**: `✓` (green) success, `✗` (red) failed, `⋯` (yellow) running
- **Duration**: `-` if the run has no recorded duration (still running)
- **Size**: On-disk size of the run directory
- **User**: Username that started the run
- **Path**: `cwd` at run start, `~`-abbreviated

### Task history table

`otto History <task>`:

```
History for task 'hello'

Timestamp            Status  Duration  Exit Code  Run ID
────────────────────────────────────────────────────────
2026-09-03 05:23:53    ✓          0ms      0           1

Total executions: 1
Success rate: 100.0%
```

Columns are **Timestamp, Status, Duration, Exit Code, Run ID** — there is no Path column here, and Status is the task's own status (`✓`/`✗`/`⋯`/`○` skipped/`·` pending), not the run's. "Success rate" is `successful / (successful + failed)`; skipped and pending executions are excluded from the denominator.

## JSON Output Schema

### Run list (no `[TASK]`)

```json
[
  {
    "id": 2,
    "project_id": 1,
    "timestamp": 1788438233,
    "status": "Failed",
    "duration_seconds": 0.0,
    "size_bytes": 10532,
    "ottofile_path": "/tmp/otto-doc-scratch/proj/.otto.yml",
    "cwd": "/home/saidler/repos/otto-rs/otto",
    "user": "saidler",
    "hostname": "desk",
    "args": ["otto", "ci"],
    "ended_at": 1788438233,
    "run_dir": "/tmp/otto-doc-scratch/home/proj-70af3bf4/1788438233-1"
  }
]
```

`status` is `"Success"`, `"Failed"`, or `"Running"` — the Rust enum's `Debug`-style spelling, not lowercase. Newest run first.

- **`hostname`**: real since it started being recorded (the hostname-collecting call existed with no caller before that, so every run written earlier has `hostname: null`). A run recorded before that point stays `null` forever — there is no back-fill, because guessing the host of a historical row would be inventing data the run never recorded. The table view above never prints hostname at all; this only matters to `--json` consumers.
- **`run_dir`**: the directory the run actually wrote into, recorded at run start. `null` for runs recorded before this column existed (schema v4 and earlier).
- **`args`**: `argv[0]` followed by the task and subtask names the run was asked for, e.g. `["otto", "lint"]`. Not the full command line: flag values (which can carry secrets, e.g. `--token`) are never recorded, and not the resolved dependency closure either, only what was literally named. A bare `otto` records the ottofile's `otto.tasks:` default list, e.g. `["otto", "ci"]` in this repo.

### Task history (`otto History <task> --json`)

```json
[
  {
    "id": 1,
    "run_id": 1,
    "name": "hello",
    "status": "Completed",
    "script_hash": "734d509e",
    "exit_code": 0,
    "started_at": 1788438233,
    "ended_at": 1788438233,
    "duration_seconds": 0.0,
    "stdout_path": "/home/user/.otto/proj-70af3bf4/1788438233/tasks/hello/stdout.log",
    "stderr_path": "/home/user/.otto/proj-70af3bf4/1788438233/tasks/hello/stderr.log",
    "script_path": "/home/user/.otto/proj-70af3bf4/1788438233/tasks/hello/script.sh",
    "skip_reason": null,
    "skip_kind": null
  }
]
```

`status` here is the task-status vocabulary: `Pending`, `Running`, `Completed`, `Failed`, `Skipped` — distinct from the run-status vocabulary (`Success`/`Failed`/`Running`) used by the run list. `stdout_path`/`stderr_path`/`script_path` are absolute, not relative to `$OTTO_HOME`. `skip_reason`/`skip_kind` are populated only for a `Skipped` task; `skip_kind` is one of `up-to-date`, `serial-predecessor`, `unreachable`.

## Notes

- **Database requirement**: `History` needs `$OTTO_HOME/otto.db` (or `$HOME/.otto/otto.db`). If it is missing, `History` prints `No history database found. Run otto to create it.` to stderr and exits 0 rather than erroring.
- **No back-fill**: fields added to the schema after a row was written (`hostname`, `run_dir`, `skip_kind`) are `null`/absent on that row forever.
- **`script_hash`** (visible via `otto Stats`/the database, not in `History`'s own output) hashes the fully-rendered script including otto's injected builtins prologue, so upgrading otto's own injected builtins changes it even when the ottofile did not change.

## Related Commands

- [`otto Stats`](stats.md) - Aggregate statistics
- [`otto Clean`](clean.md) - Clean up old runs

## See Also

- [Architecture: SQLite Integration](../architecture/sqlite-integration.md)
