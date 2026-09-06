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

Usage: Clean [OPTIONS]

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

Found 15 runs to delete (342.5 MB total)

Dry run - showing what would be deleted:

  2025-10-15 08:23:10 - ~/repos/project1 (18 days old, 23.4 MB) [success]
  2025-10-14 14:52:33 - ~/repos/project2 (19 days old, 18.9 MB) [success]
  2025-10-13 11:05:47 - ~/repos/project1 (20 days old, 25.1 MB) [failed]
  ...

Run without --dry-run to actually delete these runs
```

### Filesystem Mode

```
Scanning /home/user/.otto for old runs...

Found 15 runs to delete by keeping everything for 30 days (342.5 MB total)

Dry run - showing what would be deleted:

  [abc123] 2025-10-15 08:23:10 - ~/repos/project1 (18 days old, 23.4 MB)
  [def456] 2025-10-14 14:52:33 - ~/repos/project2 (19 days old, 18.9 MB)
  ...

Run without --dry-run to actually delete these runs
```

### Actual Deletion

```
Querying database for old runs...

Found 15 runs to delete (342.5 MB total)

Deleting runs...

  Deleted 2025-10-15 08:23:10 - ~/repos/project1 (23.4 MB)
  Deleted 2025-10-14 14:52:33 - ~/repos/project2 (18.9 MB)
  ...

Deleted 342.5 MB total
```

## Retention Policy Logic

The clean command applies retention rules in this order:

1. **Keep Last N**: If `--keep-last N` is specified, the N most recent runs are always kept, regardless of age
2. **Age-Based**: Runs older than `--keep-days` are candidates for deletion
3. **Failed Run Exception**: If `--keep-failed` is specified, failed runs use that threshold instead

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
- ⚡ **100x faster** - Queries metadata instead of scanning filesystem
- 🎯 **Precise filtering** - By status, project, or retention policy
- 🔒 **Atomic operations** - Database and filesystem stay synchronized
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
- 🐌 **Slower** - Must scan entire directory tree
- 📉 **Limited filtering** - No status-based filtering
- ⚠️ **Manual sync** - Database not updated

**When to use:**
- Database is corrupted or unavailable
- Debugging or verification
- One-time cleanup before database migration

## Safety Features

1. **Dry Run Default**: Always preview before deleting
2. **Graceful Degradation**: Falls back to filesystem if database unavailable
3. **Metadata Preservation**: Database records deleted even if files already gone
4. **Atomic Deletion**: Both database and filesystem cleaned together

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
