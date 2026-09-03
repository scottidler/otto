# `otto Stats` - Execution Statistics

The `Stats` command reports aggregate metrics across all runs, or (with a task name) per-task metrics.

> **Note**: Built-in commands are capitalized (e.g., `Stats`, `Clean`) to avoid namespace conflicts with user-defined tasks.

Regenerated from `otto Stats --help` and a scratch run's `otto Stats --json`; every example below is observed output.

## Usage

```bash
otto Stats [OPTIONS] [TASK]
```

## Options

```
$ otto Stats --help
Show execution statistics

Usage: Stats [OPTIONS] [TASK]

Arguments:
  [TASK]  Show stats for a specific task

Options:
  -n, --limit <LIMIT>  Limit number of tasks shown (when showing all tasks) [default: 10]
      --json           Output as JSON
  -h, --help           Print help
```

There is no dedicated task-selection flag: the task name is a bare positional argument. `-n/--limit` only affects the "Top N Tasks" table below overall stats — it does not limit the run count `otto History` uses `-n` for.

## Examples

```bash
otto Stats
otto Stats --json
otto Stats hello
otto Stats hello --json
otto Stats -n 20
```

## Output Format

### Overall statistics (no `[TASK]`)

```
Overall Statistics
╭──────────────────────┬───────────╮
│ Metric               ┆     Value │
╞══════════════════════╪═══════════╡
│ Total Runs           ┆         2 │
├╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌┼╌╌╌╌╌╌╌╌╌╌╌┤
│ Successful           ┆ 1 (50.0%) │
├╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌┼╌╌╌╌╌╌╌╌╌╌╌┤
│ Failed               ┆         1 │
├╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌┼╌╌╌╌╌╌╌╌╌╌╌┤
│ Running              ┆         0 │
├╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌┼╌╌╌╌╌╌╌╌╌╌╌┤
│ Total Tasks Executed ┆         2 │
├╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌┼╌╌╌╌╌╌╌╌╌╌╌┤
│ Total Disk Usage     ┆   20.7 KB │
├╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌┼╌╌╌╌╌╌╌╌╌╌╌┤
│ Total Execution Time ┆       0ms │
╰──────────────────────┴───────────╯

Top 10 Tasks by Execution Count
╭─────────┬───────┬───────┬─────────┬────────┬──────────────┬──────────────╮
│ Project ┆ Task  ┆ Total ┆ Success ┆ Failed ┆ Success Rate ┆ Avg Duration │
╞═════════╪═══════╪═══════╪═════════╪════════╪══════════════╪══════════════╡
│ proj    ┆ fail  ┆     1 ┆       0 ┆      1 ┆         0.0% ┆          0ms │
├╌╌╌╌╌╌╌╌╌┼╌╌╌╌╌╌╌┼╌╌╌╌╌╌╌┼╌╌╌╌╌╌╌╌╌┼╌╌╌╌╌╌╌╌┼╌╌╌╌╌╌╌╌╌╌╌╌╌╌┼╌╌╌╌╌╌╌╌╌╌╌╌╌╌┤
│ proj    ┆ hello ┆     1 ┆       1 ┆      0 ┆       100.0% ┆          0ms │
╰─────────┴───────┴───────┴─────────┴────────┴──────────────┴──────────────╯
```

There is no "Average Run Duration" row — "Total Execution Time" is the last row of the first table, and it is a sum, not a mean. The Top-N table is only printed when there is at least one task stats row; `-n`/`--limit` caps how many rows it shows (10 by default), heading text included ("Top 10 Tasks by Execution Count").

### Task-specific statistics

`otto Stats hello`, single project:

```
Statistics for task 'hello'
╭──────────────────┬─────────────────────╮
│ Metric           ┆               Value │
╞══════════════════╪═════════════════════╡
│ Project          ┆                proj │
├╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌┼╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌┤
│ Total Executions ┆                   1 │
├╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌┼╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌┤
│ Successful       ┆          1 (100.0%) │
├╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌┼╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌┤
│ Failed           ┆                   0 │
├╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌┼╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌┤
│ Skipped          ┆                   0 │
├╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌┼╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌┤
│ Average Duration ┆                 0ms │
├╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌┼╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌┤
│ Min Duration     ┆                 0ms │
├╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌┼╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌┤
│ Max Duration     ┆                 0ms │
├╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌┼╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌┤
│ Last Executed    ┆ 2026-09-03 05:23:53 │
├╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌┼╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌┤
│ Last Status      ┆         ✓ Completed │
╰──────────────────┴─────────────────────╯
```

A task that has run under more than one project instead gets a per-project table: columns `Project, Total, Success, Failed, Success Rate, Avg Duration` — no `--limit` applies here.

## JSON Output Schema

### Overall stats (`otto Stats --json`)

```json
{
  "total_runs": 2,
  "successful_runs": 1,
  "failed_runs": 1,
  "running_runs": 0,
  "total_tasks": 2,
  "total_disk_usage": 21162,
  "total_duration_seconds": 0.0
}
```

There are no `successful_executions`/`failed_executions`/`skipped_executions` keys here — those only exist in the per-task array below.

### Task stats (`otto Stats <task> --json`)

This is always a JSON **array**, one entry per project the task has run under, even when there is exactly one:

```json
[
  {
    "project_id": 1,
    "project_hash": "94a760b8",
    "project_name": "proj",
    "task_name": "hello",
    "total_executions": 1,
    "successful_executions": 1,
    "failed_executions": 0,
    "skipped_executions": 0,
    "avg_duration_seconds": 0.0,
    "min_duration_seconds": 0.0,
    "max_duration_seconds": 0.0,
    "last_executed": 1788438233,
    "last_status": "Completed"
  }
]
```

`last_status` uses the task-status vocabulary (`Pending`/`Running`/`Completed`/`Failed`/`Skipped`), not the run-status one.

## Notes

- **Database requirement**: same as `History` — needs `$OTTO_HOME/otto.db`. Missing database prints `No statistics database found. Run otto to create it.` to stderr and exits 0.
- **`-n`/`--limit` scope**: it caps the Top-N table under the overall-stats view only; it has no effect on `otto Stats <task>`.

## Related Commands

- [`otto History`](history.md) - Detailed execution history
- [`otto Clean`](clean.md) - Manage disk usage

## See Also

- [Architecture: SQLite Integration](../architecture/sqlite-integration.md)
