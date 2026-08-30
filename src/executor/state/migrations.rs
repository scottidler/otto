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
    log::debug!("migrate: current_version={current_version} target_version={SCHEMA_VERSION}");

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

#[path = "migrations_tests.rs"]
mod tests;
