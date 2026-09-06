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

Usage: otto Stats [OPTIONS] [TASK]

Arguments:
  [TASK]  Show stats for a specific task

Options:
  -n, --limit <LIMIT>  Limit number of tasks shown (when showing all tasks) [default: 10]
      --json           Output as JSON
  -h, --help           Print help
```

There is no dedicated task-selection flag: the task name is a bare positional argument. `-n/--limit` caps the per-task rows under overall stats, in both the "Top N Tasks" table and the `tasks` array of `--json`. It does not limit the run count `otto History` uses `-n` for.

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
│ Total Disk Usage     ┆   20.9 KB │
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

`Total Disk Usage` (`total_disk_usage` in JSON) sums the size each run recorded when it finished, and that size counts only what the run directory itself holds. Symlinks are neither followed nor counted, so a link into the project's shared `.cache/` charges the cached blob to nobody rather than to every run that referenced it. The number is therefore smaller than earlier versions of otto reported, where the recorded size followed symlinks and the same blob was counted once per referencing run. `otto Clean` sizes run directories through the same function, so the two numbers agree.

Every Success Rate on this page is the share of runs that reached a terminal state: `successful / (successful + failed)`. Runs still `Running` are counted in `Total Runs` and in the `Running` row, but never in the denominator, because they can never reach the numerator. When nothing has reached a terminal state the rate reads `n/a`, not `0.0%`.

There is no "Average Run Duration" row: "Total Execution Time" is the last row of the first table, and it is a sum, not a mean. The Top-N table is only printed when there is at least one task stats row; `-n`/`--limit` caps how many rows it shows (10 by default), heading text included ("Top 10 Tasks by Execution Count").

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
│ Last Executed    ┆ 2026-09-06 07:15:45 │
├╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌┼╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌┤
│ Last Status      ┆         ✓ Completed │
╰──────────────────┴─────────────────────╯
```

A task that has run under more than one project instead gets a per-project table: columns `Project, Total, Success, Failed, Success Rate, Avg Duration`, with `Success Rate` again over the executions that reached a terminal state. No `--limit` applies here.

## JSON Output Schema

### Overall stats (`otto Stats --json`)

```json
{
  "total_runs": 2,
  "successful_runs": 1,
  "failed_runs": 1,
  "running_runs": 0,
  "total_tasks": 2,
  "total_disk_usage": 21443,
  "total_duration_seconds": 0.0,
  "tasks": [
    {
      "project_id": 1,
      "project_hash": "a208048f",
      "project_name": "proj",
      "task_name": "fail",
      "total_executions": 1,
      "successful_executions": 0,
      "failed_executions": 1,
      "skipped_executions": 0,
      "avg_duration_seconds": 0.0,
      "min_duration_seconds": 0.0,
      "max_duration_seconds": 0.0,
      "last_executed": 1788704145,
      "last_status": "Failed"
    },
    {
      "project_id": 1,
      "project_hash": "a208048f",
      "project_name": "proj",
      "task_name": "hello",
      "total_executions": 1,
      "successful_executions": 1,
      "failed_executions": 0,
      "skipped_executions": 0,
      "avg_duration_seconds": 0.0,
      "min_duration_seconds": 0.0,
      "max_duration_seconds": 0.0,
      "last_executed": 1788704145,
      "last_status": "Completed"
    }
  ]
}
```

`tasks` holds the same rows the "Top N Tasks" table prints, so `-n`/`--limit` caps its length too. The seven aggregate keys stay at the top level, which is why `jq '.total_runs'` still resolves; the per-task `successful_executions`/`failed_executions`/`skipped_executions` keys appear only inside `tasks[]`, never beside the aggregates.

### Task stats (`otto Stats <task> --json`)

This is always a JSON **array**, one entry per project the task has run under, even when there is exactly one:

```json
[
  {
    "project_id": 1,
    "project_hash": "a208048f",
    "project_name": "proj",
    "task_name": "hello",
    "total_executions": 1,
    "successful_executions": 1,
    "failed_executions": 0,
    "skipped_executions": 0,
    "avg_duration_seconds": 0.0,
    "min_duration_seconds": 0.0,
    "max_duration_seconds": 0.0,
    "last_executed": 1788704145,
    "last_status": "Completed"
  }
]
```

`last_status` uses the task-status vocabulary (`Pending`/`Running`/`Completed`/`Failed`/`Skipped`), not the run-status one. Entries under the overall payload's `tasks` key have this same shape.

## Notes

- **Database requirement**: same as `History`, it needs `$OTTO_HOME/otto.db`. Missing database prints `No statistics database found. Run otto to create it.` to stderr and exits 0.
- **`-n`/`--limit` scope**: it caps the per-task rows under the overall-stats view, in the Top-N table and in `--json`'s `tasks` array; it has no effect on `otto Stats <task>`.

## Related Commands

- [`otto History`](history.md) - Detailed execution history
- [`otto Clean`](clean.md) - Manage disk usage

## See Also

- [Architecture: SQLite Integration](../architecture/sqlite-integration.md)
