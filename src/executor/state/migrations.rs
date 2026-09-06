use eyre::{Context, Result};
use rusqlite::Connection;
use std::time::SystemTime;

use super::db::TransactionGuard;
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

    // OR IGNORE, because `version` is the PRIMARY KEY and recording a version
    // that is already recorded is a no-op, not an error. A bare INSERT here made
    // two processes racing the same migration step a hard failure for the loser:
    // `migrate()` returned Err, `try_new()` returned None, and the run executed
    // with no history at exit 0.
    conn.execute(
        "INSERT OR IGNORE INTO schema_version (version, applied_at) VALUES (?1, ?2)",
        [version, timestamp],
    )
    .context("Failed to set schema version")?;

    Ok(())
}

/// Apply one upgrade step and its version bump as a single immediate
/// transaction.
///
/// Every step reads before it writes: `column_exists` decides whether the ALTER
/// is needed, and the version is re-read here to decide whether the step is
/// needed at all. A deferred transaction cannot serve that shape - see
/// [`TransactionGuard`] - so the write lock is taken up front.
///
/// The version is re-read *inside* the transaction because the read that
/// scheduled this step happened outside it: a concurrent process may have
/// applied the whole step in between, and then this one has nothing to do. That
/// also makes an interrupted upgrade safe to resume: the step's own
/// `column_exists` guard turns the replay into a no-op and only the version bump
/// lands.
fn migrate_step(conn: &Connection, target: i64, step: impl FnOnce(&Connection) -> Result<()>) -> Result<()> {
    let tx = TransactionGuard::immediate(conn)?;
    if get_current_version(conn)? < target {
        step(conn)?;
        set_version(conn, target)?;
    }
    tx.commit()
}

/// Run all pending migrations
pub fn migrate(conn: &Connection) -> Result<()> {
    let current_version = get_current_version(conn)?;
    log::debug!("migrate: current_version={current_version} target_version={SCHEMA_VERSION}");

    if current_version == 0 {
        // Same transaction discipline as the upgrade branches below, and for a
        // sharper reason: on a cold database every concurrent process reads
        // version 0, every one of them runs `init_schema` (all CREATE TABLE IF
        // NOT EXISTS, so all succeed), and every one of them then recorded the
        // version. Measured before this fix, five concurrent runs against a cold
        // OTTO_HOME persisted as few as 1 of 5, silently, at exit 0.
        //
        // The version is re-read inside the transaction because the first read
        // happened outside it: another process may have completed the whole
        // initialization in between, and then this one has nothing to do.
        //
        // BEGIN IMMEDIATE, not the default deferred transaction. A deferred
        // transaction starts read-only and has to upgrade to a write lock on its
        // first write, and SQLite refuses to make that upgrade wait - it returns
        // SQLITE_BUSY at once rather than deadlock, so `busy_timeout` cannot help
        // there. Taking the write lock up front is the case `busy_timeout` does
        // cover. Measured: deferred gave "database is locked: Error code 5" and
        // persisted 1-2 of 5 concurrent cold starts, worse than the bug being
        // fixed; immediate persists 5 of 5.
        let tx = TransactionGuard::immediate(conn)?;
        if get_current_version(conn)? == 0 {
            init_schema(conn).context("Failed to initialize schema")?;
            set_version(conn, SCHEMA_VERSION)?;
        }
        tx.commit()?;
    } else if current_version < SCHEMA_VERSION {
        // Run migrations in order. Step and version bump go in one immediate
        // transaction: a crash between them leaves a database whose recorded
        // version lies about its shape, and a deferred transaction around a
        // read-then-write step hands a concurrent upgrader SQLITE_BUSY with no
        // wait. Both were live defects; `migrate_step` documents the mechanics.
        if current_version < 2 {
            migrate_step(conn, 2, |conn| {
                migrate_v1_to_v2(conn).context("Failed to migrate from v1 to v2")
            })?;
        }
        if current_version < 3 {
            migrate_step(conn, 3, |conn| {
                migrate_v2_to_v3(conn).context("Failed to migrate from v2 to v3")
            })?;
        }
        if current_version < 4 {
            migrate_step(conn, 4, |conn| {
                migrate_v3_to_v4(conn).context("Failed to migrate from v3 to v4")
            })?;
        }
        if current_version < 5 {
            // The runs rebuild drops and recreates a table that `tasks`
            // references, which needs foreign keys off; `PRAGMA foreign_keys` is
            // a no-op inside a transaction, so it is toggled around one, and
            // restored whether the step succeeds or fails.
            conn.pragma_update(None, "foreign_keys", "OFF")
                .context("Failed to disable foreign keys for the v4 to v5 rebuild")?;
            let rebuilt = migrate_step(conn, 5, |conn| {
                migrate_v4_to_v5(conn).context("Failed to migrate from v4 to v5")
            });
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
