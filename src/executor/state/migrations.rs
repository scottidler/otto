use eyre::{Context, Result};
use rusqlite::Connection;
use std::time::SystemTime;

use super::schema::{SCHEMA_VERSION, init_schema, migrate_v1_to_v2, migrate_v2_to_v3};

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
        // Run migrations in order
        if current_version < 2 {
            migrate_v1_to_v2(conn).context("Failed to migrate from v1 to v2")?;
            set_version(conn, 2)?;
        }
        if current_version < 3 {
            // Step and version bump in one transaction: a crash between them
            // leaves a database whose recorded version lies about its shape.
            let tx = conn.unchecked_transaction()?;
            migrate_v2_to_v3(conn).context("Failed to migrate from v2 to v3")?;
            set_version(conn, 3)?;
            tx.commit()?;
        }
        // Future migrations will go here (v3 to v4, etc.)
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

    /// A v2-shaped tasks table, i.e. the schema before `skip_reason` existed.
    fn init_v2_tasks_table(conn: &Connection) -> Result<()> {
        conn.execute(
            "CREATE TABLE schema_version (version INTEGER PRIMARY KEY, applied_at INTEGER NOT NULL)",
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
                script_path TEXT
            )",
            [],
        )?;
        set_version(conn, 2)?;
        Ok(())
    }

    fn has_skip_reason(conn: &Connection) -> Result<bool> {
        let count: i64 = conn.query_row(
            "SELECT COUNT(*) FROM pragma_table_info('tasks') WHERE name = 'skip_reason'",
            [],
            |row| row.get(0),
        )?;
        Ok(count > 0)
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
