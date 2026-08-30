use eyre::{Context, Result};
use rusqlite::Connection;
use std::time::SystemTime;

use super::schema::{
    SCHEMA_VERSION, init_schema, migrate_v1_to_v2, migrate_v2_to_v3, migrate_v3_to_v4, migrate_v4_to_v5,
};

pub fn get_current_version(conn: &Connection) -> Result<i64> {
    let table_exists: bool = conn
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='schema_version'",
            [],
            |row| {
                let count: i64 = row.get(0)?;
                Ok(count > 0)
            },
        )
        .context("Failed to check if schema_version table exists")?;

    if !table_exists {
        return Ok(0);
    }

    // Now query the version, handling NULL from MAX()
    let version: Result<Option<i64>, rusqlite::Error> =
        conn.query_row("SELECT MAX(version) FROM schema_version", [], |row| row.get(0));

    match version {
        Ok(Some(v)) => Ok(v),
        Ok(None) => Ok(0), // Table exists but is empty
        Err(rusqlite::Error::QueryReturnedNoRows) => Ok(0),
        Err(e) => Err(e).context("Failed to get schema version"),
    }
}

fn set_version(conn: &Connection, version: i64) -> Result<()> {
    let timestamp = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .context("Failed to get current time")?
        .as_secs() as i64;

    conn.execute(
        "INSERT INTO schema_version (version, applied_at) VALUES (?1, ?2)",
        [version, timestamp],
    )
    .context("Failed to set schema version")?;

    Ok(())
}

/// Run all pending migrations
pub fn migrate(conn: &Connection) -> Result<()> {
    let current_version = get_current_version(conn)?;

    if current_version == 0 {
        init_schema(conn).context("Failed to initialize schema")?;
        set_version(conn, SCHEMA_VERSION)?;
    } else if current_version < SCHEMA_VERSION {
        // Run migrations in order. Step and version bump go in one transaction:
        // a crash between them leaves a database whose recorded version lies
        // about its shape, and the retry then dies on a duplicate column.
        if current_version < 2 {
            let tx = conn.unchecked_transaction()?;
            migrate_v1_to_v2(conn).context("Failed to migrate from v1 to v2")?;
            set_version(conn, 2)?;
            tx.commit()?;
        }
        if current_version < 3 {
            let tx = conn.unchecked_transaction()?;
            migrate_v2_to_v3(conn).context("Failed to migrate from v2 to v3")?;
            set_version(conn, 3)?;
            tx.commit()?;
        }
        if current_version < 4 {
            let tx = conn.unchecked_transaction()?;
            migrate_v3_to_v4(conn).context("Failed to migrate from v3 to v4")?;
            set_version(conn, 4)?;
            tx.commit()?;
        }
        if current_version < 5 {
            // The runs rebuild drops and recreates a table that `tasks`
            // references, which needs foreign keys off; `PRAGMA foreign_keys` is
            // a no-op inside a transaction, so it is toggled around one, and
            // restored whether the step succeeds or fails.
            conn.pragma_update(None, "foreign_keys", "OFF")
                .context("Failed to disable foreign keys for the v4 to v5 rebuild")?;
            let rebuilt = (|| -> Result<()> {
                let tx = conn.unchecked_transaction()?;
                migrate_v4_to_v5(conn).context("Failed to migrate from v4 to v5")?;
                set_version(conn, 5)?;
                tx.commit()?;
                Ok(())
            })();
            conn.pragma_update(None, "foreign_keys", "ON")
                .context("Failed to re-enable foreign keys after the v4 to v5 rebuild")?;
            rebuilt?;
        }
        // Future migrations will go here (v5 to v6, etc.)
    } else if current_version > SCHEMA_VERSION {
        return Err(eyre::eyre!(
            "Database schema version {} is newer than supported version {}. Please upgrade otto.",
            current_version,
            SCHEMA_VERSION
        ));
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::Connection;

    #[test]
    fn test_get_current_version_empty_db() -> Result<()> {
        let conn = Connection::open_in_memory()?;

        // Before any tables exist, getting version should return 0
        let version = get_current_version(&conn)?;
        assert_eq!(version, 0);
        Ok(())
    }

    #[test]
    fn test_migrate_fresh_db() -> Result<()> {
        let conn = Connection::open_in_memory()?;

        // Migrate should initialize schema and set version
        migrate(&conn)?;

        let version = get_current_version(&conn)?;
        assert_eq!(version, SCHEMA_VERSION);

        let mut stmt = conn
            .prepare("SELECT name FROM sqlite_master WHERE type='table' AND name IN ('projects', 'runs', 'tasks')")?;
        let count = stmt
            .query_map([], |row| row.get::<_, String>(0))?
            .collect::<Result<Vec<_>, _>>()?
            .len();
        assert_eq!(count, 3);

        Ok(())
    }

    #[test]
    fn test_migrate_idempotent() -> Result<()> {
        let conn = Connection::open_in_memory()?;

        migrate(&conn)?;
        let version1 = get_current_version(&conn)?;

        // Second migration should be no-op
        migrate(&conn)?;
        let version2 = get_current_version(&conn)?;

        assert_eq!(version1, version2);

        Ok(())
    }

    /// A v1-shaped database: `projects` has no `name`, `runs` still carries the
    /// global `UNIQUE(timestamp)` and no `run_dir`, `tasks` has neither skip
    /// column.
    fn init_v1_schema(conn: &Connection) -> Result<()> {
        conn.execute(
            "CREATE TABLE schema_version (version INTEGER PRIMARY KEY, applied_at INTEGER NOT NULL)",
            [],
        )?;
        conn.execute(
            "CREATE TABLE projects (
                id INTEGER PRIMARY KEY,
                hash TEXT NOT NULL UNIQUE,
                ottofile_path TEXT,
                first_seen INTEGER NOT NULL,
                last_seen INTEGER NOT NULL,
                run_count INTEGER DEFAULT 0
            )",
            [],
        )?;
        conn.execute(
            "CREATE TABLE runs (
                id INTEGER PRIMARY KEY,
                project_id INTEGER NOT NULL,
                timestamp INTEGER NOT NULL UNIQUE,
                status TEXT NOT NULL,
                duration_seconds REAL,
                size_bytes INTEGER,
                ottofile_path TEXT,
                cwd TEXT,
                user TEXT,
                hostname TEXT,
                args TEXT,
                ended_at INTEGER,
                FOREIGN KEY (project_id) REFERENCES projects(id) ON DELETE CASCADE
            )",
            [],
        )?;
        conn.execute(
            "CREATE TABLE tasks (
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
                FOREIGN KEY (run_id) REFERENCES runs(id) ON DELETE CASCADE
            )",
            [],
        )?;
        set_version(conn, 1)?;
        Ok(())
    }

    /// A v2-shaped database, i.e. the schema before `skip_reason` existed.
    fn init_v2_tasks_table(conn: &Connection) -> Result<()> {
        init_v1_schema(conn)?;
        conn.execute("ALTER TABLE projects ADD COLUMN name TEXT", [])?;
        set_version(conn, 2)?;
        Ok(())
    }

    fn has_column(conn: &Connection, table: &str, column: &str) -> Result<bool> {
        let count: i64 = conn.query_row(
            "SELECT COUNT(*) FROM pragma_table_info(?1) WHERE name = ?2",
            [table, column],
            |row| row.get(0),
        )?;
        Ok(count > 0)
    }

    fn has_task_column(conn: &Connection, column: &str) -> Result<bool> {
        let count: i64 = conn.query_row(
            "SELECT COUNT(*) FROM pragma_table_info('tasks') WHERE name = ?1",
            [column],
            |row| row.get(0),
        )?;
        Ok(count > 0)
    }

    fn has_skip_reason(conn: &Connection) -> Result<bool> {
        has_task_column(conn, "skip_reason")
    }

    fn has_skip_kind(conn: &Connection) -> Result<bool> {
        has_task_column(conn, "skip_kind")
    }

    /// A v3-shaped tasks table: `skip_reason` exists, `skip_kind` does not.
    fn init_v3_tasks_table(conn: &Connection) -> Result<()> {
        init_v2_tasks_table(conn)?;
        conn.execute("ALTER TABLE tasks ADD COLUMN skip_reason TEXT", [])?;
        set_version(conn, 3)?;
        Ok(())
    }

    #[test]
    fn test_migrate_v2_to_v3_adds_skip_reason() -> Result<()> {
        let conn = Connection::open_in_memory()?;
        init_v2_tasks_table(&conn)?;
        assert!(!has_skip_reason(&conn)?, "the v2 table must not have the column yet");

        migrate(&conn)?;

        assert_eq!(get_current_version(&conn)?, SCHEMA_VERSION);
        assert!(has_skip_reason(&conn)?, "v3 adds skip_reason to tasks");
        Ok(())
    }

    #[test]
    fn test_migrate_v2_to_v3_survives_an_interrupted_run() -> Result<()> {
        // The column landed but the version bump did not: re-running must fix
        // the version rather than failing on a duplicate column.
        let conn = Connection::open_in_memory()?;
        init_v2_tasks_table(&conn)?;
        conn.execute("ALTER TABLE tasks ADD COLUMN skip_reason TEXT", [])?;

        migrate(&conn)?;

        assert_eq!(get_current_version(&conn)?, SCHEMA_VERSION);
        assert!(has_skip_reason(&conn)?);
        Ok(())
    }

    #[test]
    fn test_migrate_v3_to_v4_adds_skip_kind() -> Result<()> {
        let conn = Connection::open_in_memory()?;
        init_v3_tasks_table(&conn)?;
        assert!(has_skip_reason(&conn)?, "the v3 table already has the free-text column");
        assert!(!has_skip_kind(&conn)?, "the v3 table must not have skip_kind yet");

        migrate(&conn)?;

        assert_eq!(get_current_version(&conn)?, SCHEMA_VERSION);
        assert!(has_skip_kind(&conn)?, "v4 adds skip_kind to tasks");
        Ok(())
    }

    #[test]
    fn test_migrate_v3_to_v4_survives_an_interrupted_run() -> Result<()> {
        // The crash window: the ALTER committed but the version bump did not.
        // Re-running must repair the version instead of dying on a duplicate column.
        let conn = Connection::open_in_memory()?;
        init_v3_tasks_table(&conn)?;
        conn.execute("ALTER TABLE tasks ADD COLUMN skip_kind TEXT", [])?;

        migrate(&conn)?;

        assert_eq!(get_current_version(&conn)?, SCHEMA_VERSION);
        assert!(has_skip_kind(&conn)?);
        Ok(())
    }

    #[test]
    fn test_migrate_v2_reaches_v4_in_one_pass() -> Result<()> {
        // A database two versions behind must land on both columns, not stop at v3.
        let conn = Connection::open_in_memory()?;
        init_v2_tasks_table(&conn)?;

        migrate(&conn)?;

        assert_eq!(get_current_version(&conn)?, SCHEMA_VERSION);
        assert!(has_skip_reason(&conn)?);
        assert!(has_skip_kind(&conn)?);
        Ok(())
    }

    /// A v4-shaped database: both skip columns present, `runs` still v1-shaped.
    fn init_v4_schema(conn: &Connection) -> Result<()> {
        init_v3_tasks_table(conn)?;
        conn.execute("ALTER TABLE tasks ADD COLUMN skip_kind TEXT", [])?;
        set_version(conn, 4)?;
        Ok(())
    }

    #[test]
    fn test_migrate_v1_to_v2_adds_and_backfills_name() -> Result<()> {
        let conn = Connection::open_in_memory()?;
        init_v1_schema(&conn)?;
        assert!(!has_column(&conn, "projects", "name")?, "v1 has no name column");
        conn.execute(
            "INSERT INTO projects (hash, ottofile_path, first_seen, last_seen)
             VALUES ('abc12345', '/home/u/repos/widget/otto.yml', 1, 1)",
            [],
        )?;

        migrate(&conn)?;

        assert_eq!(get_current_version(&conn)?, SCHEMA_VERSION);
        let name: String = conn.query_row("SELECT name FROM projects WHERE hash = 'abc12345'", [], |r| r.get(0))?;
        assert_eq!(name, "widget", "the name is backfilled from the ottofile's directory");
        Ok(())
    }

    #[test]
    fn test_migrate_v1_to_v2_survives_an_interrupted_run() -> Result<()> {
        // The crash window that used to brick the database permanently: the
        // ALTER committed, the version bump did not, and every later open died
        // on `duplicate column name: name`.
        let conn = Connection::open_in_memory()?;
        init_v1_schema(&conn)?;
        conn.execute("ALTER TABLE projects ADD COLUMN name TEXT", [])?;
        conn.execute(
            "INSERT INTO projects (hash, ottofile_path, first_seen, last_seen)
             VALUES ('abc12345', '/home/u/repos/widget/otto.yml', 1, 1)",
            [],
        )?;

        migrate(&conn)?;

        assert_eq!(get_current_version(&conn)?, SCHEMA_VERSION);
        let name: String = conn.query_row("SELECT name FROM projects WHERE hash = 'abc12345'", [], |r| r.get(0))?;
        assert_eq!(name, "widget");
        Ok(())
    }

    #[test]
    fn test_migrate_v1_to_v2_does_not_clobber_a_backfilled_name() -> Result<()> {
        // A resumed migration must leave names an earlier pass already wrote.
        let conn = Connection::open_in_memory()?;
        init_v1_schema(&conn)?;
        conn.execute("ALTER TABLE projects ADD COLUMN name TEXT", [])?;
        conn.execute(
            "INSERT INTO projects (hash, name, first_seen, last_seen) VALUES ('abc12345', 'kept', 1, 1)",
            [],
        )?;

        migrate(&conn)?;

        let name: String = conn.query_row("SELECT name FROM projects WHERE hash = 'abc12345'", [], |r| r.get(0))?;
        assert_eq!(name, "kept");
        Ok(())
    }

    #[test]
    fn test_migrate_v4_to_v5_drops_the_timestamp_uniqueness() -> Result<()> {
        let conn = Connection::open_in_memory()?;
        init_v4_schema(&conn)?;
        conn.execute(
            "INSERT INTO projects (hash, name, first_seen, last_seen) VALUES ('abc12345', 'widget', 1, 1)",
            [],
        )?;
        conn.execute(
            "INSERT INTO runs (id, project_id, timestamp, status) VALUES (1, 1, 1700000000, 'success')",
            [],
        )?;
        // Before the migration, a second run in the same second is rejected.
        assert!(
            conn.execute(
                "INSERT INTO runs (project_id, timestamp, status) VALUES (1, 1700000000, 'success')",
                [],
            )
            .is_err(),
            "v4 still enforces UNIQUE(timestamp)"
        );

        migrate(&conn)?;

        assert_eq!(get_current_version(&conn)?, SCHEMA_VERSION);
        assert!(has_column(&conn, "runs", "run_dir")?, "v5 adds run_dir");
        conn.execute(
            "INSERT INTO runs (project_id, timestamp, status) VALUES (1, 1700000000, 'failed')",
            [],
        )?;
        let same_second: i64 = conn.query_row("SELECT COUNT(*) FROM runs WHERE timestamp = 1700000000", [], |r| {
            r.get(0)
        })?;
        assert_eq!(same_second, 2, "two runs may share a second after v5");
        Ok(())
    }

    #[test]
    fn test_migrate_v4_to_v5_preserves_rows_and_task_links() -> Result<()> {
        let conn = Connection::open_in_memory()?;
        init_v4_schema(&conn)?;
        conn.execute(
            "INSERT INTO projects (hash, name, first_seen, last_seen) VALUES ('abc12345', 'widget', 1, 1)",
            [],
        )?;
        conn.execute(
            "INSERT INTO runs (id, project_id, timestamp, status, size_bytes) VALUES (7, 1, 1700000000, 'success', 42)",
            [],
        )?;
        conn.execute(
            "INSERT INTO tasks (run_id, name, status, exit_code) VALUES (7, 'build', 'completed', 0)",
            [],
        )?;

        migrate(&conn)?;

        let (id, size): (i64, i64) =
            conn.query_row("SELECT id, size_bytes FROM runs", [], |r| Ok((r.get(0)?, r.get(1)?)))?;
        assert_eq!((id, size), (7, 42), "run rows survive the rebuild with their ids");
        let joined: i64 = conn.query_row(
            "SELECT COUNT(*) FROM tasks t JOIN runs r ON t.run_id = r.id WHERE t.name = 'build'",
            [],
            |r| r.get(0),
        )?;
        assert_eq!(joined, 1, "task rows still join to their run");
        let fk_ok: i64 = conn.query_row("SELECT COUNT(*) FROM pragma_foreign_key_check", [], |r| r.get(0))?;
        assert_eq!(fk_ok, 0, "the rebuild leaves no dangling foreign keys");
        Ok(())
    }

    #[test]
    fn test_migrate_v4_to_v5_survives_an_interrupted_run() -> Result<()> {
        // The rebuild landed but the version bump did not: re-running must not
        // rebuild a second time or fail.
        let conn = Connection::open_in_memory()?;
        init_v4_schema(&conn)?;
        conn.pragma_update(None, "foreign_keys", "OFF")?;
        migrate_v4_to_v5(&conn)?;

        migrate(&conn)?;

        assert_eq!(get_current_version(&conn)?, SCHEMA_VERSION);
        assert!(has_column(&conn, "runs", "run_dir")?);
        Ok(())
    }

    #[test]
    fn test_migrate_v1_reaches_v5_in_one_pass() -> Result<()> {
        let conn = Connection::open_in_memory()?;
        init_v1_schema(&conn)?;

        migrate(&conn)?;

        assert_eq!(get_current_version(&conn)?, 5);
        assert!(has_column(&conn, "projects", "name")?);
        assert!(has_column(&conn, "tasks", "skip_reason")?);
        assert!(has_column(&conn, "tasks", "skip_kind")?);
        assert!(has_column(&conn, "runs", "run_dir")?);
        Ok(())
    }

    #[test]
    fn test_set_version() -> Result<()> {
        let conn = Connection::open_in_memory()?;

        init_schema(&conn)?;

        set_version(&conn, 1)?;

        let version = get_current_version(&conn)?;
        assert_eq!(version, 1);

        let applied_at: i64 = conn.query_row("SELECT applied_at FROM schema_version WHERE version = 1", [], |row| {
            row.get(0)
        })?;
        assert!(applied_at > 0);

        Ok(())
    }
}
