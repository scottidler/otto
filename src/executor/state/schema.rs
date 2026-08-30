use eyre::Result;
use rusqlite::Connection;

/// SQL schema for the otto database
pub const SCHEMA_VERSION: i64 = 5;

/// Status of a run
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub enum RunStatus {
    Running,
    Success,
    Failed,
}

impl RunStatus {
    pub fn as_str(&self) -> &str {
        match self {
            RunStatus::Running => "running",
            RunStatus::Success => "success",
            RunStatus::Failed => "failed",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "running" => Some(RunStatus::Running),
            "success" => Some(RunStatus::Success),
            "failed" => Some(RunStatus::Failed),
            _ => None,
        }
    }
}

/// Status of a task
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub enum TaskStatus {
    Pending,
    Running,
    Completed,
    Failed,
    Skipped,
}

impl TaskStatus {
    pub fn as_str(&self) -> &str {
        match self {
            TaskStatus::Pending => "pending",
            TaskStatus::Running => "running",
            TaskStatus::Completed => "completed",
            TaskStatus::Failed => "failed",
            TaskStatus::Skipped => "skipped",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "pending" => Some(TaskStatus::Pending),
            "running" => Some(TaskStatus::Running),
            "completed" => Some(TaskStatus::Completed),
            "failed" => Some(TaskStatus::Failed),
            "skipped" => Some(TaskStatus::Skipped),
            _ => None,
        }
    }
}

/// Why a task was skipped.
///
/// This is scheduling provenance, not decoration: `classify_edge` resolves a
/// skipped dependency against the edge's `when:` using the kind, so the three
/// variants are not interchangeable.
///
/// - `UpToDate` - the task's declared outputs were newer than its inputs, so it
///   did not need to run. Success-like: it satisfies `when: success`.
/// - `SerialPredecessor` - an earlier member of this task's serial foreach group
///   failed or was skipped, so ordering can never release this member.
/// - `Unreachable` - a dependency reached a terminal state that contradicts this
///   task's `when:` condition.
///
/// `when: failure` is satisfied by none of them: it means the source task ran and
/// its action exited non-zero.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize)]
pub enum SkipKind {
    UpToDate,
    SerialPredecessor,
    Unreachable,
}

impl SkipKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            SkipKind::UpToDate => "up-to-date",
            SkipKind::SerialPredecessor => "serial-predecessor",
            SkipKind::Unreachable => "unreachable",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "up-to-date" => Some(SkipKind::UpToDate),
            "serial-predecessor" => Some(SkipKind::SerialPredecessor),
            "unreachable" => Some(SkipKind::Unreachable),
            _ => None,
        }
    }

    /// Whether a source skipped for this reason counts as a success for a
    /// downstream `when: success` edge. Only an up-to-date skip does.
    pub fn is_success_like(&self) -> bool {
        matches!(self, SkipKind::UpToDate)
    }
}

/// The `runs` table as of schema v5.
///
/// Shared by `init_schema` and the v4-to-v5 rebuild so a freshly created
/// database and a migrated one cannot drift apart.
const RUNS_TABLE_DDL: &str = "CREATE TABLE IF NOT EXISTS runs (
    id INTEGER PRIMARY KEY,
    project_id INTEGER NOT NULL,
    timestamp INTEGER NOT NULL,
    status TEXT NOT NULL,
    duration_seconds REAL,
    size_bytes INTEGER,
    ottofile_path TEXT,
    cwd TEXT,
    user TEXT,
    hostname TEXT,
    args TEXT,
    ended_at INTEGER,
    run_dir TEXT,
    FOREIGN KEY (project_id) REFERENCES projects(id) ON DELETE CASCADE
)";

/// Initialize the database schema
pub fn init_schema(conn: &Connection) -> Result<()> {
    // Schema version table
    conn.execute(
        "CREATE TABLE IF NOT EXISTS schema_version (
            version INTEGER PRIMARY KEY,
            applied_at INTEGER NOT NULL
        )",
        [],
    )?;

    // Projects table
    conn.execute(
        "CREATE TABLE IF NOT EXISTS projects (
            id INTEGER PRIMARY KEY,
            hash TEXT NOT NULL UNIQUE,
            name TEXT,
            ottofile_path TEXT,
            first_seen INTEGER NOT NULL,
            last_seen INTEGER NOT NULL,
            run_count INTEGER DEFAULT 0
        )",
        [],
    )?;

    // Runs table.
    //
    // `timestamp` is deliberately not UNIQUE: it has one-second resolution, so
    // two runs started in the same second are ordinary, not a conflict. Runs are
    // identified by `id`.
    conn.execute(RUNS_TABLE_DDL, [])?;

    // Runs indexes
    conn.execute("CREATE INDEX IF NOT EXISTS idx_runs_timestamp ON runs(timestamp)", [])?;
    conn.execute("CREATE INDEX IF NOT EXISTS idx_runs_status ON runs(status)", [])?;
    conn.execute("CREATE INDEX IF NOT EXISTS idx_runs_project ON runs(project_id)", [])?;

    // Tasks table
    conn.execute(
        "CREATE TABLE IF NOT EXISTS tasks (
            id INTEGER PRIMARY KEY,
            run_id INTEGER NOT NULL,
            name TEXT NOT NULL,
            status TEXT NOT NULL,
            script_hash TEXT,
            exit_code INTEGER,
            started_at INTEGER,
            ended_at INTEGER,
            duration_seconds REAL,
            stdout_path TEXT,
            stderr_path TEXT,
            script_path TEXT,
            skip_reason TEXT,
            skip_kind TEXT,
            FOREIGN KEY (run_id) REFERENCES runs(id) ON DELETE CASCADE
        )",
        [],
    )?;

    // Tasks indexes
    conn.execute("CREATE INDEX IF NOT EXISTS idx_tasks_run ON tasks(run_id)", [])?;
    conn.execute("CREATE INDEX IF NOT EXISTS idx_tasks_name ON tasks(name)", [])?;
    conn.execute("CREATE INDEX IF NOT EXISTS idx_tasks_status ON tasks(status)", [])?;
    conn.execute(
        "CREATE INDEX IF NOT EXISTS idx_tasks_name_run ON tasks(name, run_id)",
        [],
    )?;

    // Projects indexes
    conn.execute("CREATE INDEX IF NOT EXISTS idx_projects_name ON projects(name)", [])?;

    Ok(())
}

/// Migrate from schema version 2 to 3
///
/// Adds `skip_reason` to the tasks table so a skipped task carries *why* it was
/// skipped, not just that it was. Idempotent: re-running after a crash between
/// the ALTER and the version bump is a no-op rather than a hard error.
pub fn migrate_v2_to_v3(conn: &Connection) -> Result<()> {
    if column_exists(conn, "tasks", "skip_reason")? {
        return Ok(());
    }
    conn.execute("ALTER TABLE tasks ADD COLUMN skip_reason TEXT", [])?;
    Ok(())
}

/// Migrate from schema version 3 to 4
///
/// Adds `skip_kind` to the tasks table. v3's `skip_reason` is free text built for
/// display ("dep a failed; this task required when: success"); `skip_kind` is the
/// typed provenance the scheduler actually branches on, so history can be queried
/// by reason class instead of by substring. Same idempotency guard as v2-to-v3.
pub fn migrate_v3_to_v4(conn: &Connection) -> Result<()> {
    if column_exists(conn, "tasks", "skip_kind")? {
        return Ok(());
    }
    conn.execute("ALTER TABLE tasks ADD COLUMN skip_kind TEXT", [])?;
    Ok(())
}

/// Whether `table` already has `column`.
fn column_exists(conn: &Connection, table: &str, column: &str) -> Result<bool> {
    let mut stmt = conn.prepare("SELECT COUNT(*) FROM pragma_table_info(?1) WHERE name = ?2")?;
    let count: i64 = stmt.query_row(rusqlite::params![table, column], |row| row.get(0))?;
    Ok(count > 0)
}

/// Migrate from schema version 1 to 2
///
/// Adds `name` to the projects table and backfills it from each project's
/// ottofile path. Idempotent, like every other step: a crash between the ALTER
/// and the version bump used to leave a database that could never be opened
/// again, because the retry died on a duplicate column.
pub fn migrate_v1_to_v2(conn: &Connection) -> Result<()> {
    if !column_exists(conn, "projects", "name")? {
        conn.execute("ALTER TABLE projects ADD COLUMN name TEXT", [])?;
    }

    // Populate names from existing ottofile_path data
    // Extract directory name from path, or use hash as fallback
    let mut stmt = conn.prepare("SELECT id, hash, ottofile_path FROM projects WHERE name IS NULL")?;
    let projects: Vec<(i64, String, Option<String>)> = stmt
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))?
        .collect::<Result<Vec<_>, _>>()?;

    for (id, hash, ottofile_path) in projects {
        let name = if let Some(path) = ottofile_path {
            // Extract parent directory name from ottofile path
            // e.g., "/home/user/repos/otto/otto.yml" -> "otto"
            std::path::Path::new(&path)
                .parent()
                .and_then(|p| p.file_name())
                .and_then(|n| n.to_str())
                .unwrap_or(&hash)
                .to_string()
        } else {
            hash.clone()
        };

        conn.execute("UPDATE projects SET name = ?1 WHERE id = ?2", [&name, &id.to_string()])?;
    }

    // Create indexes
    conn.execute("CREATE INDEX IF NOT EXISTS idx_projects_name ON projects(name)", [])?;
    conn.execute(
        "CREATE INDEX IF NOT EXISTS idx_tasks_name_run ON tasks(name, run_id)",
        [],
    )?;

    Ok(())
}

/// Migrate from schema version 4 to 5
///
/// Two changes to `runs`, both about identity:
///
/// - Drops `UNIQUE(timestamp)`. Timestamps have one-second resolution, so two
///   runs started in the same second collided; the loser's run row and every
///   task row that would have hung off it were simply never written.
/// - Adds `run_dir`, the directory the run actually wrote into, so deleting a
///   run no longer rebuilds a path from a naming convention it guessed wrong.
///
/// SQLite cannot drop a constraint in place, so the table is rebuilt. The
/// caller must have foreign keys disabled and a transaction open. Idempotent:
/// `run_dir` existing means a previous attempt already finished the rebuild.
pub fn migrate_v4_to_v5(conn: &Connection) -> Result<()> {
    if column_exists(conn, "runs", "run_dir")? {
        return Ok(());
    }
    conn.execute_batch(
        "CREATE TABLE runs_v5 (
            id INTEGER PRIMARY KEY,
            project_id INTEGER NOT NULL,
            timestamp INTEGER NOT NULL,
            status TEXT NOT NULL,
            duration_seconds REAL,
            size_bytes INTEGER,
            ottofile_path TEXT,
            cwd TEXT,
            user TEXT,
            hostname TEXT,
            args TEXT,
            ended_at INTEGER,
            run_dir TEXT,
            FOREIGN KEY (project_id) REFERENCES projects(id) ON DELETE CASCADE
         );
         INSERT INTO runs_v5 (id, project_id, timestamp, status, duration_seconds, size_bytes,
                              ottofile_path, cwd, user, hostname, args, ended_at)
              SELECT id, project_id, timestamp, status, duration_seconds, size_bytes,
                     ottofile_path, cwd, user, hostname, args, ended_at
                FROM runs;
         DROP TABLE runs;
         ALTER TABLE runs_v5 RENAME TO runs;
         CREATE INDEX IF NOT EXISTS idx_runs_timestamp ON runs(timestamp);
         CREATE INDEX IF NOT EXISTS idx_runs_status ON runs(status);
         CREATE INDEX IF NOT EXISTS idx_runs_project ON runs(project_id);",
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::Connection;

    #[test]
    fn test_init_schema() -> Result<()> {
        let conn = Connection::open_in_memory()?;
        init_schema(&conn)?;

        let mut stmt = conn.prepare("SELECT name FROM sqlite_master WHERE type='table' AND name='schema_version'")?;
        let exists = stmt.exists([])?;
        assert!(exists);

        let mut stmt = conn.prepare("SELECT name FROM sqlite_master WHERE type='table' AND name='projects'")?;
        let exists = stmt.exists([])?;
        assert!(exists);

        let mut stmt = conn.prepare("SELECT name FROM sqlite_master WHERE type='table' AND name='runs'")?;
        let exists = stmt.exists([])?;
        assert!(exists);

        let mut stmt = conn.prepare("SELECT name FROM sqlite_master WHERE type='table' AND name='tasks'")?;
        let exists = stmt.exists([])?;
        assert!(exists);

        Ok(())
    }

    #[test]
    fn test_run_status_conversions() {
        assert_eq!(RunStatus::Running.as_str(), "running");
        assert_eq!(RunStatus::Success.as_str(), "success");
        assert_eq!(RunStatus::Failed.as_str(), "failed");

        assert_eq!(RunStatus::parse("running"), Some(RunStatus::Running));
        assert_eq!(RunStatus::parse("success"), Some(RunStatus::Success));
        assert_eq!(RunStatus::parse("failed"), Some(RunStatus::Failed));
        assert_eq!(RunStatus::parse("invalid"), None);
    }

    #[test]
    fn test_task_status_conversions() {
        assert_eq!(TaskStatus::Pending.as_str(), "pending");
        assert_eq!(TaskStatus::Running.as_str(), "running");
        assert_eq!(TaskStatus::Completed.as_str(), "completed");
        assert_eq!(TaskStatus::Failed.as_str(), "failed");
        assert_eq!(TaskStatus::Skipped.as_str(), "skipped");

        assert_eq!(TaskStatus::parse("pending"), Some(TaskStatus::Pending));
        assert_eq!(TaskStatus::parse("running"), Some(TaskStatus::Running));
        assert_eq!(TaskStatus::parse("completed"), Some(TaskStatus::Completed));
        assert_eq!(TaskStatus::parse("failed"), Some(TaskStatus::Failed));
        assert_eq!(TaskStatus::parse("skipped"), Some(TaskStatus::Skipped));
        assert_eq!(TaskStatus::parse("invalid"), None);
    }
}
