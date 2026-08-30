# File-Based Run Management - Implementation Plan

**Context**: Current Otto implementation stores all runs as directories under `~/.otto/`

**Problem**: Files accumulate over time, no way to list/query/cleanup old runs

**Solution**: Add management commands for current file-based system

---

## Phase 1: Basic Cleanup Command

### Goal
Add `otto clean` command to remove old runs from filesystem

### Deliverables
- [ ] `otto clean --keep <days>` command
- [ ] Scan `~/.otto/otto-*/` directories by timestamp
- [ ] Delete run directories older than threshold
- [ ] `--dry-run` mode to preview deletions
- [ ] Report disk space freed

### Implementation Details

**Scan Strategy**:
```
~/.otto/
  otto-<hash>/
    <timestamp>/    # Parse timestamp, calculate age
      tasks/
```

**Delete Logic**:
- Parse directory name as Unix timestamp
- Calculate age in days: `(now - timestamp) / 86400`
- Delete if age > keep_days
- Calculate and report size freed

**Safety**:
- Skip `.cache` directories (Phase 3)
- Skip `otto.db` if exists
- Dry-run by default or require explicit confirmation

### Usage Examples
```bash
# Preview what would be deleted
otto clean --keep 30 --dry-run

# Actually delete runs older than 30 days
otto clean --keep 30

# More aggressive cleanup
otto clean --keep 7

# Per-project cleanup
otto clean --project abc123 --keep 30
```

### Success Criteria
- [ ] Old runs deleted from filesystem
- [ ] Dry-run shows accurate preview
- [ ] Disk space actually freed
- [ ] No data loss for recent runs
- [ ] Safe error handling (e.g., permission denied)

---

## Phase 2: List Runs Command

### Goal
Add `otto list` command to show run history from filesystem

### Deliverables
- [ ] `otto list` command
- [ ] Scan and parse run directories
- [ ] Display runs in table format
- [ ] Show: project, timestamp, task count, status, size
- [ ] `--limit` flag for recent N runs
- [ ] `--project` filter for specific project

### Implementation Details

**Scan Strategy**:
```rust
for project_dir in ~/.otto/otto-*/ {
    for timestamp_dir in project_dir/*/ {
        // Parse timestamp
        // Count tasks in tasks/ directory
        // Determine success/failure (check stderr.log sizes or exit codes)
        // Calculate directory size
    }
}
```

**Status Detection**:
- Check if all `tasks/*/stderr.log` are empty (success)
- Or check for presence of error indicators
- Or scan logs for exit codes (if available)

**Output Format**:
```
PROJECT      TIMESTAMP            TASKS    STATUS     SIZE
────────────────────────────────────────────────────────────
abc123       2025-11-01 14:30:15  12       success    45 MB
abc123       2025-11-01 10:15:32  12       failed     42 MB
def456       2025-10-31 16:22:08  8        success    23 MB
```

### Usage Examples
```bash
# List all runs (newest first)
otto list

# List last 10 runs only
otto list --limit 10

# List runs for specific project
otto list --project abc123

# Combined filters
otto list --project abc123 --limit 5

# JSON output for scripting
otto list --json
```

### Success Criteria
- [ ] Lists all runs correctly
- [ ] Sorted by timestamp (newest first)
- [ ] Status detection accurate
- [ ] Size calculations correct
- [ ] Performance acceptable (<1s for 1000 runs)

---

## Phase 3: Cache Cleanup

### Goal
Clean orphaned cache entries that no recent runs reference

### Deliverables
- [ ] Scan `.cache/` directories
- [ ] Find script hashes not used by recent runs
- [ ] Add `--cache` flag to `otto clean`
- [ ] Report orphaned cache size
- [ ] Safe deletion (keep if referenced by any run in keep period)

### Implementation Details

**Cache Structure**:
```
~/.otto/
  otto-<hash>/
    .cache/
      <task>/
        <script-hash>    # Actual script content
    <timestamp>/
      tasks/
        <task>/
          script.sh -> ../../../.cache/<task>/<script-hash>
```

**Orphan Detection**:
1. Scan all runs within keep period
2. Collect all script hashes referenced by symlinks
3. Scan `.cache/` directories
4. Find hashes not in referenced set
5. Delete orphaned entries

**Safety**:
- Only clean if run is outside keep period
- Double-check symlink doesn't exist
- Warn if symlink is broken

### Usage Examples
```bash
# Clean runs and orphaned cache
otto clean --keep 30 --cache

# Only clean cache (keep all runs)
otto clean --cache-only --keep 30

# Show what would be deleted
otto clean --keep 30 --cache --dry-run
```

### Success Criteria
- [ ] Orphaned cache entries detected correctly
- [ ] No deletion of referenced cache entries
- [ ] Cache size reported accurately
- [ ] Safe with concurrent runs

---

## Phase 4: Run Details Command

### Goal
Show detailed information about a specific run

### Deliverables
- [ ] `otto info <timestamp>` command
- [ ] Display run metadata
- [ ] List all tasks with status
- [ ] Show task durations if available
- [ ] Link to log files for inspection

### Implementation Details

**Information to Display**:
- Run timestamp (human-readable)
- Project hash and path
- Number of tasks
- Overall status (success/failed)
- Task list with individual status
- Paths to logs and scripts

**Task Information**:
- Parse `tasks/` directory
- For each task: name, status, duration
- Link to stdout.log, stderr.log, script.sh

### Usage Examples
```bash
# Show details about specific run
otto info 1699876543

# Output:
# Run: 1699876543 (2024-11-13 14:30:15)
# Project: otto-abc123 (/home/user/project)
# Status: failed
# Duration: 8.5s
#
# Tasks:
#   build - success (2.3s)
#     stdout: ~/.otto/otto-abc123/1699876543/tasks/build/stdout.log
#     script: ~/.otto/otto-abc123/1699876543/tasks/build/script.sh
#   test - success (5.1s)
#   deploy - failed (0.5s)
#     stderr: ~/.otto/otto-abc123/1699876543/tasks/deploy/stderr.log
```

### Success Criteria
- [ ] Finds run by timestamp
- [ ] Displays complete information
- [ ] Paths are correct and accessible
- [ ] Human-readable formatting

---

## Phase 5: Smart Cleanup Policies

### Goal
Intelligent cleanup with retention policies

### Deliverables
- [ ] Keep last N runs regardless of age
- [ ] Keep all successful runs for X days, failed for Y days
- [ ] Per-project retention policies
- [ ] Config file for default policies
- [ ] Summary statistics before cleanup

### Implementation Details

**Retention Policy Options**:
```yaml
# ~/.otto/config.yml
retention:
  default:
    keep_days: 30
    keep_last_n: 10
    keep_failed_days: 7

  per_project:
    otto-abc123:
      keep_days: 90
      keep_last_n: 50
```

**Cleanup Logic**:
1. Load retention policy
2. Scan all runs
3. Apply policy rules:
   - Always keep last N runs
   - Keep successful within keep_days
   - Keep failed within keep_failed_days
4. Delete remaining runs
5. Clean orphaned cache

### Usage Examples
```bash
# Use default policy
otto clean

# Override policy
otto clean --keep 30 --keep-last 20

# Keep failed runs for less time
otto clean --keep 30 --keep-failed 7

# Per-project policy
otto clean --project abc123 --keep 90
```

### Success Criteria
- [ ] Policy rules applied correctly
- [ ] Important runs preserved
- [ ] Config file loaded and respected
- [ ] Clear summary of what was kept/deleted

---

## Phase 6: Disk Usage Analysis

### Goal
Understand disk usage breakdown

### Deliverables
- [ ] `otto usage` command
- [ ] Show total disk usage
- [ ] Break down by project
- [ ] Show cache size vs runs size
- [ ] Identify largest runs
- [ ] Recommendations for cleanup

### Implementation Details

**Statistics to Show**:
- Total `~/.otto/` size
- Per-project breakdown
- Cache size per project
- Oldest run per project
- Largest runs (top 10)

**Output Format**:
```
Otto Disk Usage: 2.3 GB

By Project:
  otto-abc123  1.8 GB  (250 runs, oldest: 90 days)
  otto-def456  500 MB  (45 runs, oldest: 30 days)

Cache Usage:
  otto-abc123  200 MB  (cached scripts)
  otto-def456  50 MB

Largest Runs:
  1. 1699876543 - 150 MB
  2. 1699876321 - 120 MB
  ...

Recommendations:
  • Clean runs older than 30 days: would free 800 MB
  • Clean orphaned cache: would free 50 MB
```

### Usage Examples
```bash
# Show overall usage
otto usage

# Show for specific project
otto usage --project abc123

# JSON output
otto usage --json
```

### Success Criteria
- [ ] Accurate size calculations
- [ ] Fast execution (<5s for large directories)
- [ ] Helpful recommendations
- [ ] Clear breakdown

---

## Phase Dependencies

| Phase | Focus | Depends On |
|-------|-------|------------|
| 1 | Basic cleanup | None |
| 2 | List runs | None |
| 3 | Cache cleanup | Phase 1 |
| 4 | Run details | Phase 2 |
| 5 | Smart policies | Phase 1, 2 |
| 6 | Usage analysis | Phase 2 |

---

## Key Implementation Decisions

### File Scanning Strategy
- **Walk filesystem** for each query (no persistent index)
- **Cache results** for duration of single command
- **Parallel scanning** for better performance
- **Abort on filesystem errors** (don't corrupt partial data)

### Status Detection
- **Primary**: Check stderr.log size (empty = success)
- **Fallback**: Parse logs for exit codes
- **Heuristic**: All tasks with empty stderr = success

### Safety Measures
- **Always default to dry-run** or require confirmation
- **Verify deletions** with fs::remove_dir_all error handling
- **Atomic operations** where possible
- **Clear warnings** for irreversible operations

### Performance Targets
- `otto list`: <1s for 1000 runs
- `otto clean --dry-run`: <2s for 1000 runs
- `otto usage`: <5s for 1000 runs
- `otto info`: <100ms

---

## Migration to SQLite

These commands are designed for the **current file-based system**.

When SQLite is implemented (see `sqlite-implementation-plan.md`):

### Phase 1-2 Obsoleted
- `otto list` queries database instead of scanning
- `otto clean` queries database for old runs
- 100x faster, more powerful queries

### Phase 3 Enhanced
- Cache cleanup uses database references
- Guaranteed correctness (no missed references)

### Phase 4 Enhanced
- `otto info` reads from database
- Can show historical trends
- Cross-run comparisons

### Phase 5 Enhanced
- Policies stored in database
- Automatic application
- Audit trail of cleanups

### Phase 6 Enhanced
- Real-time metrics
- Historical trends
- Predictive analysis

**Strategy**: Implement minimal versions now, enhance with SQLite later

---

## Command Summary

After all phases:

```bash
# Cleanup
otto clean --keep 30                  # Delete old runs
otto clean --keep 30 --cache          # Also clean cache
otto clean --keep 30 --dry-run        # Preview only

# List/Query
otto list                             # Show all runs
otto list --limit 10                  # Recent 10 runs
otto list --project abc123            # Filter by project
otto info 1699876543                  # Details about run

# Analysis
otto usage                            # Disk usage breakdown
otto usage --project abc123           # Per-project usage

# Config
~/.otto/config.yml                    # Retention policies
```

---

## Testing Strategy

### Unit Tests
- Directory scanning logic
- Timestamp parsing
- Size calculation
- Status detection

### Integration Tests
- Create test runs
- Run cleanup commands
- Verify deletions
- Check preserved runs

### Performance Tests
- 1000+ runs
- Large directories (>1GB)
- Concurrent access
- Edge cases (symlinks, permissions)

---

## Documentation Needs

### User Documentation
- Command reference (`otto clean --help`)
- Retention policy configuration
- Best practices for cleanup
- Troubleshooting guide

### Internal Documentation
- Filesystem layout assumptions
- Scanning algorithm
- Status detection heuristics
- Performance optimization notes

---

## Risk Mitigation

### Accidental Deletion
- **Risk**: User deletes important runs
- **Mitigation**: Dry-run by default, clear warnings, confirmation prompts

### Performance Degradation
- **Risk**: Scanning 10,000+ runs is slow
- **Mitigation**: Parallel scanning, early termination, limits

### Concurrent Access
- **Risk**: Cleanup while Otto is running
- **Mitigation**: Skip directories with active runs (recent timestamp)

### Broken Symlinks
- **Risk**: Cache cleanup breaks running tasks
- **Mitigation**: Never delete cache entries for recent runs

---

## Success Criteria

### Phase 1
- [ ] Can delete old runs safely
- [ ] Dry-run shows accurate preview
- [ ] Disk space freed as reported

### Phase 2
- [ ] Can list all runs in reasonable time
- [ ] Status detection is accurate
- [ ] Useful filtering options

### Phase 3
- [ ] Cache cleanup is safe
- [ ] No false positives (deleting referenced cache)
- [ ] Measurable disk savings

### Phase 4
- [ ] Run details show complete information
- [ ] Paths are correct and accessible

### Phase 5
- [ ] Policies applied correctly
- [ ] Config file works as expected

### Phase 6
- [ ] Usage analysis is accurate
- [ ] Recommendations are helpful

### Overall
- [ ] Users can manage disk usage effectively
- [ ] No data loss
- [ ] Reasonable performance
- [ ] Clear error messages
- [ ] Good user experience

---

## Future Enhancements (Post-SQLite)

### Advanced Queries
```bash
otto history build --failed          # All failed builds
otto stats build                     # Task statistics
otto compare 1699876543 1699876321   # Compare runs
```

### Automated Cleanup
```bash
otto daemon --auto-clean             # Background cleanup
```

### Export/Archive
```bash
otto export --run 1699876543 --output archive.tar.gz
otto import --from archive.tar.gz
```

### Retention Policies
```bash
otto retention --keep-last 50        # Set global policy
otto retention --project abc123 --keep 90  # Per-project
```

---

## Open Questions

1. **Default retention period**: 30 days? 90 days? Unlimited?
2. **Dry-run default**: Should cleanup require explicit `--force`?
3. **Status detection**: Is stderr.log size sufficient? Parse exit codes?
4. **Cache cleanup safety**: What's safe threshold (7 days? 30 days?)
5. **Performance target**: What's acceptable scan time for 10,000 runs?

---

## Recommendations

### Immediate Priority
1. **Phase 1** (Basic cleanup) - Solves immediate disk usage problem
2. **Phase 2** (List runs) - Provides visibility into what exists

### Nice to Have
3. **Phase 3** (Cache cleanup) - Additional disk savings
4. **Phase 4** (Run details) - Better debugging

### Lower Priority
5. **Phase 5** (Smart policies) - Can be manual for now
6. **Phase 6** (Usage analysis) - Nice but not critical

### Long-term
- Implement SQLite for much better performance
- All these features become faster and more powerful
- Don't over-engineer the file-based version

---

## Next Steps

1. **Review this plan** - Confirm scope and priorities
2. **Implement Phase 1** - Basic cleanup (immediate value)
3. **Implement Phase 2** - List runs (high value)
4. **Gather feedback** - Real-world usage patterns
5. **Iterate** - Adjust based on user needs
