# Project-Aware Statistics Design

> **Status note (2026-08-30):** not implemented. `grep -rn
> "get_task_stats_for_project|list_projects|list-projects" src/` returns
> zero hits, and `Stats`'s meta-task (`src/cli/parser/meta_tasks.rs`,
> `inject_stats_meta_task`) injects only `task`, `limit`, `json` - no
> `project`. The ✅ marks below on the Implementation Checklist's step
> headings and in Success Criteria are misleading: every individual checkbox
> item under those headings is unchecked (`- [ ]`), and no code implements
> any of them. `otto.jobs` and `otto.name`/`otto.about` (Phase 10 of
> `2026-06-10-code-review-remediation.md`) resolved similarly-inert `otto:`
> keys by wiring what a real usage pattern justified and deleting the rest;
> this doc's own decision, unchanged by that work, is still to implement
> `--project`/`--list-projects` rather than delete the ambition, since
> nothing here shipped even a stub to delete. The one claim below that is
> now true: `StateManager::try_new` (`executor/state/manager.rs`) does
> `log::warn!` on failure and returns `None` rather than panicking, so the
> "graceful degradation shows a warning" line under Migration Path is
> accurate as of Phase 4's state/DB rewrite, not aspirational.

## Problem Statement

The current `otto stats` command aggregates task statistics by task name only, without considering which project the task belongs to. This causes incorrect data aggregation when multiple projects use tasks with the same name (e.g., `test`, `build`, `fmt-check`).

### Example of the Problem

Given:
- **Project A** (otto): Has a `test` task that ran 100 times
- **Project B** (myapp): Has a `test` task that ran 45 times

**Current Behavior:**
```
Task Name: test
Total Executions: 145  ❌ WRONG - combines both projects
```

**Expected Behavior:**
```
Project      │ Task Name │ Total
─────────────┼───────────┼──────
otto         │ test      │ 100
myapp        │ test      │  45
```

## Root Cause Analysis

### Database Schema
The schema correctly models the relationship:
```
projects (id, hash, ottofile_path, ...)
    ↓ (1:N)
runs (id, project_id, timestamp, ...)
    ↓ (1:N)
tasks (id, run_id, name, status, ...)
```

### Current SQL Queries (Incorrect)
The stats queries in `src/executor/state/manager.rs` only query by task name:

```rust
// Line 481-485: get_all_task_stats()
"SELECT DISTINCT name FROM tasks
 ORDER BY (SELECT COUNT(*) FROM tasks t2 WHERE t2.name = tasks.name) DESC"

// Line 495: Aggregation by name only
"SELECT COUNT(*) FROM tasks WHERE name = ?1"
```

**Problem:** No JOIN to `runs` or `projects` tables, so `project_id` is never considered.

### Affected Code

1. **Data Structures** (`src/executor/state/manager.rs:62-74`)
   ```rust
   pub struct TaskStats {
       pub task_name: String,  // ❌ No project identifier
       pub total_executions: u64,
       // ... other fields
   }
   ```

2. **Query Methods** (`src/executor/state/manager.rs`)
   - `get_task_stats()` (line 393-473)
   - `get_all_task_stats()` (line 477-576)

3. **Display Code** (`src/cli/commands/stats.rs`)
   - `show_overall_stats()` (line 42-149)
   - `show_task_stats()` (line 152-202)

## Proposed Solution

### Phase 1: Database Schema Enhancement

**Goal:** Add a project name field to simplify display and queries.

#### 1.1 Schema Migration

Add `name` column to `projects` table:

```sql
ALTER TABLE projects ADD COLUMN name TEXT;
```

The name will be derived from the ottofile path (e.g., `/home/user/repos/otto` → `otto`).

**Migration Strategy:**
1. Add column with `ALTER TABLE` (SQLite supports this)
2. Populate existing projects: extract name from `ottofile_path` or use hash as fallback
3. Update `ensure_project()` to populate name for new projects
4. Create index: `CREATE INDEX idx_projects_name ON projects(name)`

#### 1.2 Schema Version Bump

Update `src/executor/state/schema.rs`:
- Bump `SCHEMA_VERSION` from `1` to `2`
- Add migration logic in `init_schema()` or new migration function

### Phase 2: Data Structure Changes

#### 2.1 Add Project Info to TaskStats

**File:** `src/executor/state/manager.rs`

```rust
// Current
pub struct TaskStats {
    pub task_name: String,
    // ... other fields
}

// New
pub struct TaskStats {
    pub project_id: i64,        // For filtering
    pub project_hash: String,   // 8-char identifier
    pub project_name: String,   // Display name (e.g., "otto")
    pub task_name: String,
    // ... other fields
}
```

#### 2.2 Add Project-Specific Stats Methods

```rust
impl StateManager {
    /// Get stats for a specific task within a specific project
    pub fn get_task_stats_for_project(
        &self,
        project_hash: &str,
        task_name: &str
    ) -> Result<Option<TaskStats>>;

    /// Get all task stats grouped by project
    pub fn get_all_task_stats_by_project(
        &self,
        limit: Option<usize>
    ) -> Result<Vec<TaskStats>>;

    /// Get all projects with summary info
    pub fn get_all_projects(&self) -> Result<Vec<ProjectSummary>>;
}

pub struct ProjectSummary {
    pub id: i64,
    pub hash: String,
    pub name: String,
    pub ottofile_path: Option<PathBuf>,
    pub run_count: u64,
    pub last_seen: u64,
}
```

### Phase 3: SQL Query Updates

#### 3.1 Update get_all_task_stats()

**Before:**
```sql
SELECT DISTINCT name FROM tasks
ORDER BY (SELECT COUNT(*) FROM tasks t2 WHERE t2.name = tasks.name) DESC
```

**After:**
```sql
SELECT DISTINCT
    t.name,
    p.id as project_id,
    p.hash as project_hash,
    p.name as project_name
FROM tasks t
JOIN runs r ON t.run_id = r.id
JOIN projects p ON r.project_id = p.id
ORDER BY (
    SELECT COUNT(*)
    FROM tasks t2
    JOIN runs r2 ON t2.run_id = r2.id
    WHERE t2.name = t.name AND r2.project_id = p.id
) DESC
LIMIT ?
```

#### 3.2 Update get_task_stats()

**Before:**
```sql
SELECT COUNT(*) FROM tasks WHERE name = ?1
```

**After:**
```sql
SELECT COUNT(*)
FROM tasks t
JOIN runs r ON t.run_id = r.id
WHERE t.name = ?1 AND r.project_id = ?2
```

All stat queries (count, avg, min, max) need similar updates.

#### 3.3 Add Index for Performance

```sql
CREATE INDEX idx_tasks_name_run ON tasks(name, run_id);
```

This composite index will speed up the JOINs.

### Phase 4: CLI Updates

#### 4.1 Add Project Column to Table Display

**File:** `src/cli/commands/stats.rs`

**Before:**
```
╭───────────────────┬───────┬─────────┬────────┬──────────────┬──────────────╮
│ Task              ┆ Total ┆ Success ┆ Failed ┆ Success Rate ┆ Avg Duration │
╞═══════════════════╪═══════╪═════════╪════════╪══════════════╪══════════════╡
│ test              ┆   145 ┆     117 ┆      8 ┆        93.6% ┆         3.8s │
```

**After:**
```
╭──────────┬───────────────────┬───────┬─────────┬────────┬──────────────┬──────────────╮
│ Project  ┆ Task              ┆ Total ┆ Success ┆ Failed ┆ Success Rate ┆ Avg Duration │
╞══════════╪═══════════════════╪═══════╪═════════╪════════╪══════════════╪══════════════╡
│ otto     ┆ test              ┆   100 ┆      95 ┆      5 ┆        95.0% ┆         2.1s │
│ myapp    ┆ test              ┆    45 ┆      22 ┆      3 ┆        88.0% ┆         6.3s │
```

Update `show_overall_stats()`:
```rust
task_table.set_header(vec![
    Cell::new("Project").set_alignment(CellAlignment::Left),   // NEW
    Cell::new("Task").set_alignment(CellAlignment::Left),
    Cell::new("Total").set_alignment(CellAlignment::Right),
    // ... rest of headers
]);

for task in &task_stats {
    task_table.add_row(vec![
        Cell::new(&task.project_name).set_alignment(CellAlignment::Left),  // NEW
        Cell::new(&task.task_name).set_alignment(CellAlignment::Left),
        // ... rest of cells
    ]);
}
```

#### 4.2 Add CLI Options for Filtering

**File:** `src/cli/commands/stats.rs`

```rust
#[derive(Debug, clap::Parser)]
#[command(name = "stats")]
pub struct StatsCommand {
    /// Show stats for a specific task
    #[arg(value_name = "TASK")]
    pub task_name: Option<String>,

    /// Filter by project (hash or name)
    #[arg(short = 'p', long, value_name = "PROJECT")]
    pub project: Option<String>,

    /// List all projects
    #[arg(long)]
    pub list_projects: bool,

    /// Limit number of tasks shown
    #[arg(short = 'n', long, default_value = "10")]
    pub limit: usize,

    /// Output as JSON
    #[arg(long)]
    pub json: bool,
}
```

**Usage Examples:**
```bash
# Show all tasks across all projects
otto stats

# Show only tasks from the 'otto' project
otto stats --project otto

# Show 'test' task stats from all projects
otto stats test

# Show 'test' task from specific project
otto stats test --project otto

# List all projects with summary
otto stats --list-projects
```

#### 4.3 Add Project Listing Display

```rust
fn show_projects(&self, manager: &StateManager) -> Result<()> {
    let projects = manager.get_all_projects()?;

    if self.json {
        println!("{}", serde_json::to_string_pretty(&projects)?);
        return Ok(());
    }

    println!("\n{}", "Projects".bold());

    let mut table = Table::new();
    table
        .load_preset(UTF8_FULL)
        .apply_modifier(UTF8_ROUND_CORNERS)
        .set_header(vec![
            Cell::new("Name").set_alignment(CellAlignment::Left),
            Cell::new("Hash").set_alignment(CellAlignment::Left),
            Cell::new("Runs").set_alignment(CellAlignment::Right),
            Cell::new("Last Seen").set_alignment(CellAlignment::Left),
        ]);

    for project in &projects {
        table.add_row(vec![
            Cell::new(&project.name),
            Cell::new(&project.hash),
            Cell::new(project.run_count.to_string()),
            Cell::new(format_timestamp(project.last_seen)),
        ]);
    }

    println!("{}", table);
    Ok(())
}
```

### Phase 5: JSON Output Updates

Update JSON output to include project information:

**Before:**
```json
{
  "task_name": "test",
  "total_executions": 145,
  "successful_executions": 117,
  "failed_executions": 8
}
```

**After:**
```json
{
  "project_id": 1,
  "project_hash": "6b20a2e4",
  "project_name": "otto",
  "task_name": "test",
  "total_executions": 100,
  "successful_executions": 95,
  "failed_executions": 5
}
```

### Phase 6: Backward Compatibility

#### 6.1 Overall Stats
- **Overall statistics** (total runs, total tasks, etc.) remain unchanged
- They correctly aggregate across all projects

#### 6.2 Migration Path
1. Existing databases will be automatically migrated on first run
2. If migration fails, stats gracefully degrade (show warning)
3. Old JSON output format is a subset of new format (additive change)

## Implementation Checklist

### Step 1: Schema Migration (not started)
- [ ] Add migration function to `src/executor/state/schema.rs`
- [ ] Implement `migrate_v1_to_v2()` function
- [ ] Add `name` column to projects table
- [ ] Populate names from existing `ottofile_path` data
- [ ] Create index: `idx_projects_name`
- [ ] Bump `SCHEMA_VERSION` to 2
- [ ] Add tests for migration

### Step 2: Data Structures (not started)
- [ ] Update `TaskStats` struct in `src/executor/state/manager.rs`
  - Add `project_id: i64`
  - Add `project_hash: String`
  - Add `project_name: String`
- [ ] Add `ProjectSummary` struct
- [ ] Update serialization derives

### Step 3: Query Methods (not started)
- [ ] Update `get_task_stats()` to join with projects
- [ ] Update `get_all_task_stats()` to join with projects
- [ ] Add `get_task_stats_for_project()`
- [ ] Add `get_all_projects()`
- [ ] Update all stat aggregation queries (count, avg, min, max)
- [ ] Add composite index: `idx_tasks_name_run`

### Step 4: CLI Display (not started)
- [ ] Update `StatsCommand` struct in `src/cli/commands/stats.rs`
  - Add `project: Option<String>`
  - Add `list_projects: bool`
- [ ] Update table headers to include "Project" column
- [ ] Update table row generation
- [ ] Add `show_projects()` method
- [ ] Update `execute()` to handle new flags

### Step 5: Filtering Logic (not started)
- [ ] Implement project filtering in query methods
- [ ] Support filtering by project name or hash
- [ ] Handle case where project doesn't exist

### Step 6: Testing (not started)
- [ ] Test migration with existing database
- [ ] Test stats with multiple projects
- [ ] Test filtering by project
- [ ] Test `--list-projects` flag
- [ ] Test JSON output format
- [ ] Test backward compatibility

### Step 7: Documentation (not started)
- [ ] Update `docs/commands/stats.md`
- [ ] Add examples with multiple projects
- [ ] Document new CLI flags
- [ ] Update JSON schema documentation

## Open Questions

1. **Project Name Derivation**
   - Extract from `ottofile_path`: Use parent directory name?
   - Example: `/home/user/repos/otto/otto.yml` → `otto`
   - Fallback: Use hash if path is unavailable?
   - **Decision:** Use directory name containing the ottofile, fall back to hash

2. **Name Uniqueness**
   - Project names are NOT unique (multiple projects can have same name)
   - Hash IS unique
   - Display name, filter by name or hash
   - **Decision:** Display name prominently, allow filtering by either

3. **Default Behavior**
   - Should `otto stats` show all projects or current project only?
   - **Recommendation:** Show all projects (current behavior is global)
   - Add `--current` flag to filter to current project

4. **Sorting**
   - Sort by total executions across all tasks in project? Or alphabetically?
   - **Recommendation:** Primary sort by execution count, secondary by project name

5. **UI Width Concerns**
   - Adding project column makes table wider
   - **Mitigation:** Truncate long project names, add `--verbose` for full names

## Alternative Approaches Considered

### Alternative 1: Keep Current Behavior, Add Warning
**Pros:** No changes needed
**Cons:** Wrong data, confusing to users
**Verdict:** ❌ Not acceptable

### Alternative 2: Separate Stats per Project
Show stats only for the current project based on current ottofile
**Pros:** Simpler queries, no UI width issues
**Cons:** Loses cross-project visibility
**Verdict:** ❌ Too limiting

### Alternative 3: Composite Key in Display
Show tasks as "project:task" (e.g., "otto:test")
**Pros:** Minimal code changes
**Cons:** Ugly display, harder to filter/sort
**Verdict:** ❌ Poor UX

### Alternative 4: Proposed Solution (Selected)
Add project column, update queries to join with projects
**Pros:** Clean display, accurate data, flexible filtering
**Cons:** More code changes, wider table
**Verdict:** ✅ Best balance

## Performance Considerations

### Query Performance
- JOINs add overhead, but tables are small (< 100k rows typical)
- Composite index `(name, run_id)` will help significantly
- Existing indexes on `run_id` and `project_id` already cover the joins

### Expected Impact
- Negligible for typical use (< 10k task executions)
- For large databases (> 100k tasks), queries may take a few hundred ms
- Can optimize with materialized views if needed in future

### Monitoring
- Add query timing logs in debug mode
- If performance degrades, consider:
  - Adding a `project_id` directly to tasks table (denormalized)
  - Creating a stats cache table that's updated on run completion

## Success Criteria

None of these are done; the ✅ marks were aspirational when the doc was
drafted and are corrected here (2026-08-30) rather than left standing as if
verified.

1. Task stats correctly segregated by project
2. No data loss during migration
3. Existing databases migrate automatically
4. UI clearly shows which project each task belongs to
5. JSON output includes project information
6. Performance remains acceptable (< 500ms for stats query)
7. Tests pass with multiple projects

## References

- Database schema: `docs/architecture/sqlite-integration.md`
- Current implementation: `src/executor/state/manager.rs`
- Stats command: `src/cli/commands/stats.rs`
- Project management: `src/executor/workspace.rs`
