# Migration Guide: SQLite Integration

This guide helps you upgrade to Otto with SQLite integration and understand the changes.

## Overview

Otto now uses a hybrid storage model:
- **SQLite database**: Fast queries for metadata (history, stats, cleanup)
- **Filesystem**: Scripts, logs, and outputs remain inspectable

## What's New

### New Commands

- **`otto history`**: View execution history
- **`otto stats`**: View aggregate statistics

### Enhanced Commands

- **`otto clean`**: Now has advanced retention policies
  - `--keep-last N`: Keep most recent N runs
  - `--keep-failed N`: Different retention for failed runs
  - `--no-db`: Filesystem fallback mode

## Upgrade Process

### Automatic Upgrade (Recommended)

The database is automatically created on first use:

```bash
# Just run Otto normally
otto build

# Database created at ~/.otto/otto.db
# All new runs automatically recorded
```

**That's it!** No manual migration needed.

### What Happens

1. **First run with database support:**
   - Otto creates `~/.otto/otto.db`
   - Initializes schema
   - Records current run

2. **Subsequent runs:**
   - Automatically record metadata
   - Filesystem artifacts work as before
   - No user action required

### Existing Runs

**Historical runs** (before database) are NOT automatically imported. You have two options:

#### Option 1: Start Fresh (Recommended)

Simply start using the database going forward:

```bash
# New runs are tracked automatically
otto build
otto history  # Shows runs from now on
```

**Pros:**
- Zero effort
- Clean start
- No import issues

**Cons:**
- Historical data not in database
- Can still access via filesystem

#### Option 2: Import Historical Runs (Advanced)

Import existing runs into database:

```bash
# Future feature (not yet implemented)
otto import --scan ~/.otto
```

**This will be available in a future release.**

## Compatibility

### Backward Compatibility ✅

- **Existing commands**: Work exactly as before
- **Existing scripts**: No changes needed
- **Ottofiles**: No modifications required
- **Filesystem layout**: Unchanged
- **`run.yaml` files**: Still created for compatibility

### Forward Compatibility ✅

- **Without database**: Otto still works (degrades gracefully)
- **Database optional**: Not required for basic operation
- **Fallback mode**: `--no-db` flag available

## Verification

### Check Database Creation

```bash
# Verify database exists
ls -lh ~/.otto/otto.db

# Should show database file
```

### Test New Commands

```bash
# Run a task
otto build

# Check history
otto history

# Check stats
otto stats

# Test cleanup
otto clean --dry-run
```

## Troubleshooting

### Database Not Created

**Symptom:** No `~/.otto/otto.db` file after running Otto

**Possible Causes:**
1. Using older version without database support
2. Permissions issue
3. Disk space issue

**Solution:**
```bash
# Check Otto version
otto --version

# Verify ~/.otto directory writable
touch ~/.otto/test && rm ~/.otto/test

# Check disk space
df -h ~/.otto
```

### Commands Show Empty History

**Symptom:** `otto history` shows no results

**Cause:** Only runs AFTER database creation are tracked

**Solution:**
```bash
# Run a task to create first database entry
otto build

# Now history will show it
otto history
```

### Database Corruption

**Symptom:** Database errors in output

**Solution:**
```bash
# Backup database (if needed)
cp ~/.otto/otto.db ~/.otto/otto.db.bak

# Delete corrupted database
rm ~/.otto/otto.db

# Otto will recreate on next run
otto build

# Verify
otto history
```

### "Database Not Available" Warnings

**Symptom:** Warnings about database unavailable

**Cause:** SQLite dependency not installed or database locked

**Solution:**

For locked database:
```bash
# Check for stale lock
fuser ~/.otto/otto.db

# Wait for other Otto process to finish
# Or use filesystem fallback
otto clean --no-db
```

For missing SQLite:
```bash
# On Ubuntu/Debian
sudo apt-get install libsqlite3-dev

# On macOS
brew install sqlite3

# Rebuild Otto
cargo build --release
```

## Feature Comparison

| Feature | Without Database | With Database |
|---------|------------------|---------------|
| Basic execution | ✅ Yes | ✅ Yes |
| Task dependencies | ✅ Yes | ✅ Yes |
| Script caching | ✅ Yes | ✅ Yes |
| View history | ❌ Manual | ✅ `otto history` |
| Statistics | ❌ Manual | ✅ `otto stats` |
| Smart cleanup | ⚠️ Age only | ✅ Advanced policies |
| Filter by status | ❌ No | ✅ Yes |
| JSON export | ❌ No | ✅ Yes |
| Cleanup speed | 🐌 Slow scan | ⚡ Fast query |

## Performance Impact

### Run Execution

**Impact:** Negligible (<10ms overhead per run)

```
Without database:
- Create run directory
- Write run.yaml
- Execute tasks
Total: 10.5s

With database:
- Create run directory
- Write run.yaml
- Record in database (+5ms)
- Execute tasks
- Update database (+5ms)
Total: 10.5s (imperceptible difference)
```

### Cleanup Operations

**Impact:** Significant improvement

```
Without database (filesystem scan):
- 1000 runs: ~2-3 seconds
- 10000 runs: ~20-30 seconds

With database (query):
- 1000 runs: ~10ms
- 10000 runs: ~50ms

Speedup: 100-600x faster
```

### Disk Usage

**Impact:** Minimal

```
Database overhead:
- ~2-5 KB per run
- 1000 runs: ~2-5 MB
- 10000 runs: ~20-50 MB

Compared to run artifacts: Negligible
(runs typically 10-100+ MB each)
```

## Rollback (If Needed)

### Remove Database

If you want to revert to file-only mode:

```bash
# Stop using database
rm ~/.otto/otto.db

# Otto will continue without database
# (history/stats won't work, but basic execution will)
```

### Preserve Historical Data

```bash
# Backup database before removing
cp ~/.otto/otto.db ~/otto-db-backup.db

# Can restore later if needed
cp ~/otto-db-backup.db ~/.otto/otto.db
```

## Best Practices

### Backup Strategy

```bash
# Include database in backups
tar -czf otto-backup.tar.gz ~/.otto/otto.db ~/.otto/otto-*/

# Exclude old runs to save space
tar --exclude='~/.otto/otto-*/*/tasks/*' \
    -czf otto-metadata-backup.tar.gz ~/.otto/
```

### Maintenance Schedule

```bash
# Weekly cleanup (via cron)
0 0 * * 0 otto clean --keep-days 30 --keep-last 10

# Monthly deep clean
0 0 1 * * otto clean --keep-days 7 --keep-last 5
```

### Monitoring

```bash
# Check disk usage
du -sh ~/.otto

# Check database size
ls -lh ~/.otto/otto.db

# Review statistics
otto stats
```

## FAQ

### Q: Will Otto still work without the database?

**A:** Yes! Otto gracefully degrades. The database is for convenience features (history, stats, fast cleanup). Core execution works fine without it.

### Q: Do I need to import old runs?

**A:** No. Old runs remain accessible on the filesystem. The database is for future runs. You can manually inspect old runs in `~/.otto/otto-*/`.

### Q: Can I use Otto on multiple machines?

**A:** Yes. Each machine has its own `~/.otto/otto.db`. Projects are identified by hash, so you can run the same project on different machines without conflict.

### Q: How do I share history across team?

**A:** Currently, each user has their own database. Team-wide history (PostgreSQL backend) is planned for a future release.

### Q: What if the database gets too big?

**A:** Use `otto clean` regularly. The database is typically <1% of total Otto storage. Most space is in run artifacts (logs, outputs), not metadata.

### Q: Can I query the database directly?

**A:** Yes! It's standard SQLite:

```bash
sqlite3 ~/.otto/otto.db "SELECT * FROM runs ORDER BY timestamp DESC LIMIT 10"
```

See [Architecture: SQLite Integration](architecture/sqlite-integration.md) for schema details.

### Q: Is this a breaking change?

**A:** No. All existing workflows, scripts, and ottofiles work unchanged. The database is additive functionality.

## Getting Help

### Check Logs

```bash
# Run with verbose output
otto -v build

# Check for database-related messages
```

### Verify Setup

```bash
# Test database commands
otto history
otto stats

# Test cleanup
otto clean --dry-run
```

### Report Issues

If you encounter problems:

1. Check this migration guide
2. Review [Architecture Documentation](architecture/sqlite-integration.md)
3. Run with `-v` flag for verbose output
4. Report issues with:
   - Otto version (`otto --version`)
   - OS and version
   - Error messages
   - Database file size and permissions

## Related Documentation

- [History Command](commands/history.md)
- [Stats Command](commands/stats.md)
- [Clean Command](commands/clean.md)
- [Architecture: SQLite Integration](architecture/sqlite-integration.md)
- [Implementation Plan](sqlite-implementation-plan.md)
