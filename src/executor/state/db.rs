use eyre::{Context, Result};
use rusqlite::Connection;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use super::migrations::migrate;
use crate::executor::layout::resolve_otto_home;

/// How long a writer waits for another otto process to release the database
/// before giving up.
const BUSY_TIMEOUT: Duration = Duration::from_secs(5);

/// How many times to attempt the `journal_mode=WAL` switch before giving up, and
/// the base backoff between attempts (it scales linearly with the attempt index).
///
/// A journal-mode switch needs a brief exclusive lock that `busy_timeout` does not
/// cover, so concurrent cold starts collide here. The window is short; the retry
/// only has to outlive another opener's switch.
const WAL_PRAGMA_ATTEMPTS: u32 = 10;
const WAL_PRAGMA_BACKOFF: Duration = Duration::from_millis(20);

/// The database file name inside the otto home.
const DB_FILE_NAME: &str = "otto.db";

/// Database manager for Otto's SQLite database
pub struct DatabaseManager {
    conn: Arc<Mutex<Connection>>,
    db_path: PathBuf,
}

impl DatabaseManager {
    /// Create a new DatabaseManager
    ///
    /// This will:
    /// 1. Create the database file at the specified path
    /// 2. Enable WAL mode for better concurrency
    /// 3. Run schema migrations
    pub fn new(db_path: PathBuf) -> Result<Self> {
        #[cfg(test)]
        Self::refuse_the_developers_real_database(&db_path);

        // Ensure parent directory exists
        if let Some(parent) = db_path.parent() {
            std::fs::create_dir_all(parent).context("Failed to create database directory")?;
        }

        let conn = Connection::open(&db_path).context(format!("Failed to open database at {}", db_path.display()))?;

        // Wait for a writer instead of failing instantly. Two otto runs in the
        // same repo write to one database; without this, the second one gets
        // SQLITE_BUSY and silently loses its history.
        //
        // This goes FIRST. It used to sit after the journal_mode pragma below,
        // which meant that pragma - the one statement that needs a brief
        // exclusive lock - ran with the default zero timeout. Concurrent cold
        // starts lost runs to "Failed to enable WAL mode: database is locked".
        conn.busy_timeout(BUSY_TIMEOUT)
            .context("Failed to set the database busy timeout")?;

        // Enable WAL mode for better concurrency.
        //
        // Tolerated, not required: switching the journal mode wants an exclusive
        // lock, and a concurrent opener can hold it. If another process already
        // put the database in WAL, this one inherits it and there is nothing to
        // do; failing the whole connection there would throw away the run history
        // over a mode that is already correct.
        // Retried, because the exclusive lock a mode switch needs is held only for
        // the switch itself: a concurrent opener doing the same thing releases it
        // almost immediately, and this loop outlives that window.
        let mut last_err = None;
        for attempt in 0..WAL_PRAGMA_ATTEMPTS {
            match conn.pragma_update(None, "journal_mode", "WAL") {
                Ok(()) => {
                    last_err = None;
                    break;
                }
                Err(e) => {
                    let mode: String = conn
                        .query_row("PRAGMA journal_mode", [], |row| row.get(0))
                        .context("Failed to read the journal mode after WAL was refused")?;
                    if mode.eq_ignore_ascii_case("wal") {
                        log::debug!("journal_mode already WAL, set by a concurrent opener");
                        last_err = None;
                        break;
                    }
                    last_err = Some((e, mode));
                    std::thread::sleep(WAL_PRAGMA_BACKOFF * (attempt + 1));
                }
            }
        }
        if let Some((e, mode)) = last_err {
            return Err(e).context(format!(
                "Failed to enable WAL mode after {WAL_PRAGMA_ATTEMPTS} attempts (journal_mode is {mode})"
            ));
        }

        // WAL plus NORMAL is the durability the run history needs: a crash can
        // lose the last commit, but the file is never corrupted, and every write
        // stops waiting on an fsync.
        conn.pragma_update(None, "synchronous", "NORMAL")
            .context("Failed to set synchronous mode")?;

        // Enable foreign keys
        conn.pragma_update(None, "foreign_keys", "ON")
            .context("Failed to enable foreign keys")?;

        // Run migrations
        migrate(&conn).context("Failed to run database migrations")?;

        Ok(Self {
            conn: Arc::new(Mutex::new(conn)),
            db_path,
        })
    }

    /// Open the database at the default Otto location.
    pub fn open_default() -> Result<Self> {
        let db_path = Self::default_db_path()?;
        Self::new(db_path)
    }

    /// Where the database lives, in precedence order:
    ///
    /// 1. `$OTTO_DB_PATH`, an escape hatch for pointing several projects at one
    ///    store or putting the store on a tmpfs.
    /// 2. `<otto home>/otto.db`, where the otto home is `$OTTO_HOME` or
    ///    `$HOME/.otto`.
    ///
    /// The database is derived from the otto home rather than hardcoded to
    /// `$HOME/.otto`, so `OTTO_HOME` is the single knob that moves otto's state.
    /// It used to move only the run directories: a run under a scratch
    /// `OTTO_HOME` still wrote its rows into the developer's real database.
    /// Under `cfg(test)` only: abort rather than open `$HOME/.otto/otto.db`.
    ///
    /// Several `--lib` tests mutate `OTTO_HOME` process-globally and a few
    /// `remove_var` it, which leaves a window where `default_db_path()`
    /// resolves to the developer's real database. Rust runs the tests in one
    /// process across parallel threads, so a concurrent test that builds a
    /// default-path store during that window writes - or deletes - real rows.
    /// A review of this audit measured exactly that: a full `cargo test` that
    /// moved `~/.otto/otto.db` and dropped one `runs` row, not reproducible on
    /// a later run because it is a race.
    ///
    /// The window is not closed by fixing the call sites one at a time; the
    /// next test to add a `remove_var` reopens it. This makes the failure
    /// impossible to have silently: a test that would touch the real store
    /// panics naming itself instead. Production is unaffected - the whole
    /// function compiles out.
    ///
    /// Path resolution itself is untouched, so the tests that assert the
    /// `$HOME/.otto` fallback still pass: they compute the path, they do not
    /// open it.
    #[cfg(test)]
    fn refuse_the_developers_real_database(db_path: &std::path::Path) {
        let Some(home) = std::env::var_os("HOME") else {
            return;
        };
        let real = PathBuf::from(home).join(".otto").join(DB_FILE_NAME);
        assert!(
            db_path != real,
            "a test tried to open the developer's real database at {}. \
             Something left OTTO_HOME unset, or built a store with no explicit path. \
             Use an isolated home (tests/common::isolated_state_manager, or \
             StateManager::with_db_path into a TempDir).",
            real.display()
        );
    }

    pub fn default_db_path() -> Result<PathBuf> {
        if let Ok(db_path) = std::env::var("OTTO_DB_PATH") {
            return Ok(PathBuf::from(db_path));
        }

        Ok(resolve_otto_home()?.join(DB_FILE_NAME))
    }

    /// Get the database path
    pub fn path(&self) -> &Path {
        &self.db_path
    }

    /// Execute a closure with access to the database connection
    ///
    /// This is the primary way to interact with the database.
    /// The connection is locked for the duration of the closure.
    pub fn with_connection<F, T>(&self, f: F) -> Result<T>
    where
        F: FnOnce(&Connection) -> Result<T>,
    {
        let conn = self
            .conn
            .lock()
            .map_err(|e| eyre::eyre!("Failed to lock database connection: {}", e))?;
        f(&conn)
    }

    pub fn health_check(&self) -> Result<()> {
        self.with_connection(|conn| {
            conn.query_row("SELECT 1", [], |_| Ok(()))?;
            Ok(())
        })
    }

    /// Get database statistics
    pub fn stats(&self) -> Result<DatabaseStats> {
        self.with_connection(|conn| {
            let project_count: i64 = conn.query_row("SELECT COUNT(*) FROM projects", [], |row| row.get(0))?;

            let run_count: i64 = conn.query_row("SELECT COUNT(*) FROM runs", [], |row| row.get(0))?;

            let task_count: i64 = conn.query_row("SELECT COUNT(*) FROM tasks", [], |row| row.get(0))?;

            Ok(DatabaseStats {
                project_count,
                run_count,
                task_count,
            })
        })
    }
}

/// Database statistics
#[derive(Debug, Clone)]
pub struct DatabaseStats {
    pub project_count: i64,
    pub run_count: i64,
    pub task_count: i64,
}

#[path = "db_tests.rs"]
mod tests;
