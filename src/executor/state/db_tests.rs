#![cfg(test)]

use super::*;
use serial_test::serial;
use tempfile::TempDir;

/// Restore the two knobs to "unset" so one test cannot leak into the next.
fn clear_path_env() {
    // SAFETY: every test that touches these is `#[serial]`.
    unsafe {
        std::env::remove_var("OTTO_DB_PATH");
        std::env::remove_var("OTTO_HOME");
    }
}

#[test]
#[serial]
fn test_db_path_follows_otto_home_alone() -> Result<()> {
    // The defect this pins: `OTTO_HOME` moved the run directories but not
    // the database, so a run under a scratch home wrote its rows into the
    // developer's real `~/.otto/otto.db`.
    clear_path_env();
    // SAFETY: serialized.
    unsafe {
        std::env::set_var("OTTO_HOME", "/tmp/otto-scratch-home");
    }
    let path = DatabaseManager::default_db_path();
    clear_path_env();

    assert_eq!(path?, PathBuf::from("/tmp/otto-scratch-home/otto.db"));
    Ok(())
}

#[test]
#[serial]
fn test_db_path_prefers_an_explicit_override() -> Result<()> {
    // `OTTO_DB_PATH` is kept as an override of the derived default, not
    // deleted: pointing several projects at one store is legitimate.
    clear_path_env();
    // SAFETY: serialized.
    unsafe {
        std::env::set_var("OTTO_HOME", "/tmp/otto-scratch-home");
        std::env::set_var("OTTO_DB_PATH", "/tmp/elsewhere/shared.db");
    }
    let path = DatabaseManager::default_db_path();
    clear_path_env();

    assert_eq!(path?, PathBuf::from("/tmp/elsewhere/shared.db"));
    Ok(())
}

#[test]
#[serial]
fn test_db_path_falls_back_to_home_dot_otto() -> Result<()> {
    clear_path_env();
    let path = DatabaseManager::default_db_path()?;

    let expected = PathBuf::from(std::env::var("HOME")?).join(".otto").join("otto.db");
    assert_eq!(path, expected);
    Ok(())
}

#[test]
fn test_busy_timeout_is_set() -> Result<()> {
    let temp_dir = TempDir::new()?;
    let db = DatabaseManager::new(temp_dir.path().join("test.db"))?;

    db.with_connection(|conn| {
        let timeout: i64 = conn.pragma_query_value(None, "busy_timeout", |row| row.get(0))?;
        assert_eq!(timeout, BUSY_TIMEOUT.as_millis() as i64);
        Ok(())
    })
}

#[test]
fn test_synchronous_is_normal() -> Result<()> {
    let temp_dir = TempDir::new()?;
    let db = DatabaseManager::new(temp_dir.path().join("test.db"))?;

    db.with_connection(|conn| {
        // 1 is NORMAL in SQLite's synchronous enum.
        let synchronous: i64 = conn.pragma_query_value(None, "synchronous", |row| row.get(0))?;
        assert_eq!(synchronous, 1);
        Ok(())
    })
}

#[test]
fn test_new_database() -> Result<()> {
    let temp_dir = TempDir::new()?;
    let db_path = temp_dir.path().join("test.db");

    let _db = DatabaseManager::new(db_path.clone())?;
    assert!(db_path.exists());

    Ok(())
}

#[test]
fn test_wal_mode_enabled() -> Result<()> {
    let temp_dir = TempDir::new()?;
    let db_path = temp_dir.path().join("test.db");

    let db = DatabaseManager::new(db_path)?;

    db.with_connection(|conn| {
        let journal_mode: String = conn.pragma_query_value(None, "journal_mode", |row| row.get(0))?;
        assert_eq!(journal_mode.to_lowercase(), "wal");
        Ok(())
    })?;

    Ok(())
}

#[test]
fn test_foreign_keys_enabled() -> Result<()> {
    let temp_dir = TempDir::new()?;
    let db_path = temp_dir.path().join("test.db");

    let db = DatabaseManager::new(db_path)?;

    db.with_connection(|conn| {
        let foreign_keys: i64 = conn.pragma_query_value(None, "foreign_keys", |row| row.get(0))?;
        assert_eq!(foreign_keys, 1);
        Ok(())
    })?;

    Ok(())
}

#[test]
fn test_health_check() -> Result<()> {
    let temp_dir = TempDir::new()?;
    let db_path = temp_dir.path().join("test.db");

    let db = DatabaseManager::new(db_path)?;
    db.health_check()?;

    Ok(())
}

#[test]
fn test_stats() -> Result<()> {
    let temp_dir = TempDir::new()?;
    let db_path = temp_dir.path().join("test.db");

    let db = DatabaseManager::new(db_path)?;
    let stats = db.stats()?;

    // New database should have zero records
    assert_eq!(stats.project_count, 0);
    assert_eq!(stats.run_count, 0);
    assert_eq!(stats.task_count, 0);

    Ok(())
}

#[test]
fn test_with_connection() -> Result<()> {
    let temp_dir = TempDir::new()?;
    let db_path = temp_dir.path().join("test.db");

    let db = DatabaseManager::new(db_path)?;

    // Test that we can execute queries through with_connection
    db.with_connection(|conn| {
        let count: i64 = conn.query_row("SELECT COUNT(*) FROM projects", [], |row| row.get(0))?;
        assert_eq!(count, 0);
        Ok(())
    })?;

    Ok(())
}

#[test]
fn test_schema_initialized() -> Result<()> {
    let temp_dir = TempDir::new()?;
    let db_path = temp_dir.path().join("test.db");

    let db = DatabaseManager::new(db_path)?;

    db.with_connection(|conn| {
        let tables: Vec<String> = conn
            .prepare("SELECT name FROM sqlite_master WHERE type='table' ORDER BY name")?
            .query_map([], |row| row.get(0))?
            .collect::<Result<Vec<_>, _>>()?;

        assert!(tables.contains(&"projects".to_string()));
        assert!(tables.contains(&"runs".to_string()));
        assert!(tables.contains(&"tasks".to_string()));
        assert!(tables.contains(&"schema_version".to_string()));

        Ok(())
    })?;

    Ok(())
}
