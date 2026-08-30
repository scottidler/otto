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
        // Ensure parent directory exists
        if let Some(parent) = db_path.parent() {
            std::fs::create_dir_all(parent).context("Failed to create database directory")?;
        }

        let conn = Connection::open(&db_path).context(format!("Failed to open database at {}", db_path.display()))?;

        // Enable WAL mode for better concurrency
        conn.pragma_update(None, "journal_mode", "WAL")
            .context("Failed to enable WAL mode")?;

        // Wait for a writer instead of failing instantly. Two otto runs in the
        // same repo write to one database; without this, the second one gets
        // SQLITE_BUSY and silently loses its history.
        conn.busy_timeout(BUSY_TIMEOUT)
            .context("Failed to set the database busy timeout")?;

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
