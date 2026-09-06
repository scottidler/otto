# `otto Clean` - Manage Run Artifacts

The `Clean` command helps manage disk space by removing old Otto run artifacts based on configurable retention policies.

## Usage

```bash
otto Clean [OPTIONS]
```

## Options

```
$ otto Clean --help
Clean old otto run directories

Usage: otto Clean [OPTIONS]

Options:
      --keep-days <KEEP_DAYS>
          Keep runs newer than this many days [default: 30]
      --keep-last <KEEP_LAST>
          Keep at least this many most recent runs (regardless of age)
      --keep-failed <KEEP_FAILED>
          Keep failed runs for this many days (overrides --keep-days for failed runs)
      --dry-run
          Dry run - show what would be deleted without deleting
      --project-filter <PROJECT_FILTER>
          Filter by project hash
      --no-db
          Use filesystem scan instead of database (fallback mode)
  -h, --help
          Print help
```

## Examples

### Basic Cleanup

```bash
# Delete runs older than 30 days (default)
otto Clean

# Delete runs older than 7 days
otto Clean --keep-days 7

# Delete runs older than 90 days
otto Clean --keep-days 90
```

### Preview Before Deleting

```bash
# Dry run - see what would be deleted
otto Clean --dry-run

# Preview 7-day cleanup
otto Clean --keep-days 7 --dry-run
```

### Smart Retention Policies

```bash
# Keep last 10 runs regardless of age, delete older runs beyond 30 days
otto Clean --keep-days 30 --keep-last 10

# Keep failed runs for 60 days, successful runs for 30 days
otto Clean --keep-days 30 --keep-failed 60

# Keep last 5 runs, successful runs for 14 days, failed runs for 30 days
otto Clean --keep-days 14 --keep-failed 30 --keep-last 5
```

### Project-Specific Cleanup

```bash
# Clean specific project only
otto Clean --project-filter abc123

# Dry run for specific project
otto Clean --project-filter abc123 --dry-run
```

### Filesystem Fallback Mode

```bash
# Force filesystem scan (no database)
otto Clean --no-db

# Useful if database is unavailable or for debugging
otto Clean --no-db --dry-run
```

## Output

### Database Mode (Default)

```
Querying database for old runs...
Scanning /home/user/.otto for run directories...

Found 15 rows from the database and 3 orphaned directories to delete by keeping everything for 30 days (342.5 MB total)

Dry run - showing what would be deleted:

  2025-10-15 08:23:10 - ~/repos/project1 (18 days old, 23.4 MB) [success] /home/user/.otto/project1-abc12345/1760516590
  2025-10-14 14:52:33 - ~/repos/project2 (19 days old, 18.9 MB) [success] /home/user/.otto/project2-def45678/1760453553
  [abc12345] 2025-10-13 11:05:47 - <unknown> (20 days old, 25.1 MB) /home/user/.otto/project1-abc12345/1760353547
  ...

Run without --dry-run to actually delete these runs
```

Two populations, counted separately. The first number counts **rows**: runs the
database knows about, whose directory is removed with them when there is one.
The second counts **directories** that no row names, removed by path. Lines in
the second group are prefixed with the project hash, the way filesystem mode
prints them, and every line ends with the run directory.

### Filesystem Mode

```
Scanning /home/user/.otto for old runs...

Found 15 runs to delete by keeping everything for 30 days (342.5 MB total)

Dry run - showing what would be deleted:

  [abc12345] 2025-10-15 08:23:10 - ~/repos/project1 (18 days old, 23.4 MB) /home/user/.otto/project1-abc12345/1760516590
  [def45678] 2025-10-14 14:52:33 - ~/repos/project2 (19 days old, 18.9 MB) /home/user/.otto/project2-def45678/1760453553
  ...

Run without --dry-run to actually delete these runs
```

### Actual Deletion

```
Querying database for old runs...
Scanning /home/user/.otto for run directories...

Found 15 rows from the database and 3 orphaned directories to delete by keeping everything for 30 days (342.5 MB total)

Deleting runs...

  Deleted 2025-10-15 08:23:10 - ~/repos/project1 (23.4 MB)
  Deleted 2025-10-14 14:52:33 - ~/repos/project2 (18.9 MB)
  Deleted orphaned directory [abc12345] 2025-10-13 11:05:47 - <unknown> (25.1 MB)
  ...

Freed 342.5 MB of disk space
```

## Retention Policy Logic

The clean command applies retention rules in this order:

1. **Keep Last N**: If `--keep-last N` is specified, the N most recent **run directories** are always kept, regardless of age. It counts directories, once, across both populations above: N in total, not N rows plus N orphans. A database row whose directory is gone, or that never recorded one, has no directory to keep and is removed on age alone (`--keep-days`, or `--keep-failed` for a failed run)
2. **Age-Based**: Runs older than `--keep-days` are candidates for deletion
3. **Failed Run Exception**: If `--keep-failed` is specified, failed runs use that threshold instead. Run status only exists in the database, so a run directory that no row names takes the longer of the two thresholds rather than being deleted by a flag meant to protect it. `--no-db` has no statuses at all and applies that same widening to everything it scans

### Example Policy Flow

```bash
otto Clean --keep-days 30 --keep-failed 60 --keep-last 5
```

For each run:
1. Is it in the 5 most recent? → **KEEP** (regardless of age or status)
2. Is it a failed run? → Delete if older than 60 days
3. Is it a successful run? → Delete if older than 30 days

## Storage Location

Otto stores run artifacts in:
```
$OTTO_HOME/ (default ~/.otto/)
├── <project-name>-<hash>/      # Per-project directory, e.g. proj-70af3bf4/
│   ├── .cache/                 # Rendered scripts, flat, named by content hash
│   ├── <timestamp>/            # Individual run directories
│   │   ├── .lock              # Held by the run while it is going; cleanup skips it
│   │   ├── run.yaml           # Serialized ExecutionContext for this run
│   │   ├── tasks/             # Task execution data
│   │   │   ├── <task-name>/
│   │   │   │   ├── script.sh -> ../../../.cache/<hash>.sh
│   │   │   │   ├── stdout.log
│   │   │   │   ├── stderr.log
│   │   │   │   └── output.<task-name>.json
│   │   └── ...
└── otto.db                     # SQLite database (metadata only)
```

Full detail, including the input/output files a task with dependencies gets, is in [`docs/directory-layout.md`](../directory-layout.md).

## Database vs Filesystem Mode

### Database Mode (Default, Recommended)

**Advantages:**
- 📇 **Named** - Reads run metadata from the database, so a run is identified by its row rather than by its directory name. Both modes walk the filesystem and size what they select: since Phase 8 the database mode scans too, because that is the only way to see run directories no row names
- 🎯 **Precise filtering** - By status, project, or retention policy
- 🔒 **Reconciled** - Both stores are considered: rows the database holds, and run directories on disk that no row names. The two modes select the same set of run directories
- 📊 **Rich queries** - Complex retention policies possible

**Requirements:**
- SQLite database available (`~/.otto/otto.db`)
- Database initialized (automatic on first run with database support)

### Filesystem Mode (`--no-db`)

**Advantages:**
- 🔧 **Always works** - No database dependency
- 🔍 **Simple** - Direct filesystem inspection
- 🛡️ **Fallback** - Automatic when database unavailable

**Limitations:**
- 📉 **Limited filtering** - No status-based filtering, so `--keep-failed` becomes the longer cutoff for every run
- ⚠️ **Manual sync** - Rows are left behind for the directories it deletes

**When to use:**
- Database is corrupted or unavailable
- Debugging or verification
- One-time cleanup before database migration

## Safety Features

1. **Dry Run Default**: Always preview before deleting
2. **Graceful Degradation**: Falls back to filesystem if database unavailable
3. **Metadata Preservation**: Database records deleted even if files already gone
4. **Ordered Deletion, Not Atomic**: The rows and the directory are two stores and there is no transaction across them. The path is resolved and checked first, the rows are committed next, and the directory is removed last, so a failure at any point errs toward an orphaned directory rather than a deleted directory whose history is still claimed. An orphan left that way is reclaimed by path on the next clean
5. **Never Through a Link, Never Outside the Root**: A run directory replaced by a symlink is refused rather than followed, and a path that resolves outside the tree being cleaned is refused
6. **Live Runs Are Skipped**: A run holds an advisory lock on its own directory for as long as it is running, and cleanup skips any directory whose lock is held. It applies to both modes, including `--dry-run`, which takes the same lock so that what it reports is what a real invocation would delete. A directory whose lock cannot be tested at all is skipped and reported rather than deleted

## Common Use Cases

### Regular Maintenance

```bash
# Weekly cleanup script
otto Clean --keep-days 30 --keep-last 10
```

### Aggressive Space Recovery

```bash
# Free up space aggressively
otto Clean --keep-days 7 --keep-last 3 --keep-failed 14
```

### Long-Term Archival

```bash
# Keep recent history
otto Clean --keep-days 90 --keep-last 20
```

### Per-Project Cleanup

```bash
# Clean old project only
otto Clean --project-filter old_proj --keep-days 7
```

### Audit Before Delete

```bash
# See what will be deleted
otto Clean --dry-run

# Review and confirm
otto Clean
```

## Performance

### Database Mode
- **Query time**: <10ms for 1,000 runs
- **Query time**: <100ms for 10,000 runs
- **Deletion time**: ~100ms per run (filesystem bound)

### Filesystem Mode
- **Scan time**: ~1s per 1,000 run directories
- **Memory usage**: Minimal (streaming scan)

## Troubleshooting

### Database Unavailable

If database is missing or corrupted:
```bash
# Use filesystem fallback
otto Clean --no-db

# Database will be automatically recreated on next run
```

### Inconsistent State

If database and filesystem are out of sync:
```bash
# Verify with dry run
otto Clean --dry-run

# Database will self-heal on next run
```

### Disk Space Not Freed

Check actual file deletion:
```bash
# Verify cleanup happened
ls -lh ~/.otto/*-*/

# Check disk usage
du -sh ~/.otto
```

## Related Commands

- [`otto History`](history.md) - View runs before cleaning
- [`otto Stats`](stats.md) - Understand disk usage patterns

## See Also

- [Architecture: SQLite Integration](../architecture/sqlite-integration.md)
