use crate::cfg::otto::RetentionSpec;
use crate::cli::CleanCommand;
use eyre::Result;
use log::warn;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// Resolve the otto home directory.
///
/// Uses `$OTTO_HOME` if set, otherwise `$HOME/.otto`.
pub fn resolve_otto_home() -> Result<PathBuf> {
    if let Ok(otto_home) = std::env::var("OTTO_HOME") {
        Ok(PathBuf::from(otto_home))
    } else {
        let home = std::env::var("HOME").map_err(|e| eyre::eyre!("Failed to get HOME: {}", e))?;
        Ok(PathBuf::from(home).join(".otto"))
    }
}

/// Decide whether `path` may be recursively deleted as part of cleaning `root`,
/// returning the canonical path when it may.
///
/// Two rules, both of which `remove_dir_all` needs and neither of which
/// `Path::is_dir()` supplies:
///
/// - **Never follow a link.** `is_dir()` follows symlinks, so a symlinked run
///   directory under `~/.otto` made `remove_dir_all` delete the real directory
///   it pointed at - anywhere on the disk - and leave the link behind.
/// - **Never leave the root.** After resolving links, the target must still sit
///   under the tree being cleaned, so a `..` component or a relocated mount
///   cannot walk out of it.
pub fn ensure_deletable_under_root(path: &Path, root: &Path) -> Result<PathBuf> {
    let meta = fs::symlink_metadata(path).map_err(|e| eyre::eyre!("cannot stat {}: {}", path.display(), e))?;
    if meta.file_type().is_symlink() {
        return Err(eyre::eyre!(
            "{} is a symlink; refusing to delete through it",
            path.display()
        ));
    }

    let root_canonical = fs::canonicalize(root).map_err(|e| eyre::eyre!("cannot resolve {}: {}", root.display(), e))?;
    let path_canonical = fs::canonicalize(path).map_err(|e| eyre::eyre!("cannot resolve {}: {}", path.display(), e))?;

    if !path_canonical.starts_with(&root_canonical) {
        return Err(eyre::eyre!(
            "{} resolves to {}, which is outside {}",
            path.display(),
            path_canonical.display(),
            root_canonical.display()
        ));
    }
    if path_canonical == root_canonical {
        return Err(eyre::eyre!(
            "refusing to delete the otto root {}",
            root_canonical.display()
        ));
    }

    Ok(path_canonical)
}

/// Run automatic pruning if enough time has elapsed since the last prune.
///
/// This is best-effort: errors are logged but not propagated.
/// Called after task execution completes (even on failure).
pub async fn auto_prune(otto_home: &Path, retention: &RetentionSpec) {
    if !retention.auto_prune {
        return;
    }

    let marker = otto_home.join(".last_prune");
    if let Ok(meta) = fs::metadata(&marker)
        && let Ok(modified) = meta.modified()
        && let Ok(age) = modified.elapsed()
        && age < Duration::from_secs(retention.prune_interval_hours * 3600)
    {
        return; // Fast path: too soon
    }
    // .last_prune missing or stale → prune now

    log::info!("Auto-pruning old runs (interval: {}h)", retention.prune_interval_hours);

    let cmd = CleanCommand {
        keep_days: retention.keep_days,
        keep_last: Some(retention.keep_last),
        keep_failed: Some(retention.keep_failed),
        dry_run: false,
        project_filter: None,
        no_db: false,
        quiet: true,
    };

    if let Err(e) = cmd.execute().await {
        warn!("Auto-prune failed: {}", e);
        return;
    }

    // Prune orphaned cache entries
    if let Err(e) = prune_orphaned_cache(otto_home) {
        warn!("Cache prune failed: {}", e);
    }

    // Touch marker file
    if let Err(e) = fs::File::create(&marker) {
        warn!("Failed to update .last_prune marker: {}", e);
    }
}

/// Remove orphaned cache entries that are no longer referenced by any run.
///
/// For each project dir under otto_home, scans the `.cache/` directory
/// and checks if any remaining run's symlinks reference each cached script.
/// Unreferenced cache files are deleted.
fn prune_orphaned_cache(otto_home: &Path) -> Result<()> {
    let entries = match fs::read_dir(otto_home) {
        Ok(e) => e,
        Err(_) => return Ok(()), // otto_home doesn't exist or unreadable
    };

    for entry in entries {
        let entry = entry?;
        let project_dir = entry.path();
        if !project_dir.is_dir() {
            continue;
        }
        let dir_name = match project_dir.file_name().and_then(|n| n.to_str()) {
            Some(name) => name.to_string(),
            None => continue,
        };
        // Only process otto project directories (otto-<hash>)
        if !dir_name.starts_with("otto-") {
            continue;
        }

        let cache_dir = project_dir.join(".cache");
        if !cache_dir.is_dir() {
            continue;
        }

        // Collect all symlink targets referenced by remaining runs
        let mut referenced: std::collections::HashSet<String> = std::collections::HashSet::new();
        if let Ok(run_entries) = fs::read_dir(&project_dir) {
            for run_entry in run_entries.flatten() {
                let run_path = run_entry.path();
                if !run_path.is_dir() {
                    continue;
                }
                let run_name = run_path.file_name().and_then(|n| n.to_str()).unwrap_or("");
                // Skip .cache directory itself
                if run_name == ".cache" {
                    continue;
                }
                // Walk tasks dir for symlinks
                let tasks_dir = run_path.join("tasks");
                if let Ok(task_entries) = fs::read_dir(&tasks_dir) {
                    for task_entry in task_entries.flatten() {
                        let task_dir = task_entry.path();
                        if !task_dir.is_dir() {
                            continue;
                        }
                        // Check for script.sh or script.py symlinks
                        for script_name in &["script.sh", "script.py"] {
                            let script_path = task_dir.join(script_name);
                            if let Ok(target) = fs::read_link(&script_path) {
                                // Target is like ../../../.cache/<hash>.sh
                                if let Some(filename) = target.file_name().and_then(|n| n.to_str()) {
                                    referenced.insert(filename.to_string());
                                }
                            }
                        }
                    }
                }
            }
        }

        // Delete unreferenced cache files
        if let Ok(cache_entries) = fs::read_dir(&cache_dir) {
            for cache_entry in cache_entries.flatten() {
                let cache_path = cache_entry.path();
                if !cache_path.is_file() {
                    continue;
                }
                if let Some(filename) = cache_path.file_name().and_then(|n| n.to_str())
                    && !referenced.contains(filename)
                {
                    log::debug!("Removing orphaned cache entry: {}", cache_path.display());
                    let _ = fs::remove_file(&cache_path);
                }
            }
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_resolve_otto_home_default() {
        // When OTTO_HOME is not set, should use $HOME/.otto
        // This test just verifies it doesn't panic
        let home = resolve_otto_home();
        assert!(home.is_ok());
    }

    #[test]
    fn ensure_deletable_under_root_refuses_a_symlink() {
        let temp_dir = TempDir::new().unwrap();
        let root = temp_dir.path().join(".otto");
        let victim = temp_dir.path().join("victim");
        fs::create_dir_all(&root).unwrap();
        fs::create_dir_all(&victim).unwrap();
        fs::write(victim.join("precious.txt"), "keep me").unwrap();

        let link = root.join("otto-deadbeef");
        std::os::unix::fs::symlink(&victim, &link).unwrap();

        let err = ensure_deletable_under_root(&link, &root).unwrap_err().to_string();
        assert!(err.contains("symlink"), "{err}");
        assert!(victim.join("precious.txt").exists(), "the target must be untouched");
    }

    #[test]
    fn ensure_deletable_under_root_refuses_a_path_outside_the_root() {
        let temp_dir = TempDir::new().unwrap();
        let root = temp_dir.path().join(".otto");
        let outside = temp_dir.path().join("elsewhere");
        fs::create_dir_all(&root).unwrap();
        fs::create_dir_all(&outside).unwrap();

        let err = ensure_deletable_under_root(&outside, &root).unwrap_err().to_string();
        assert!(err.contains("outside"), "{err}");
    }

    #[test]
    fn ensure_deletable_under_root_refuses_the_root_itself() {
        let temp_dir = TempDir::new().unwrap();
        let root = temp_dir.path().join(".otto");
        fs::create_dir_all(&root).unwrap();

        let err = ensure_deletable_under_root(&root, &root).unwrap_err().to_string();
        assert!(err.contains("refusing to delete the otto root"), "{err}");
    }

    #[test]
    fn ensure_deletable_under_root_accepts_a_real_run_directory() {
        let temp_dir = TempDir::new().unwrap();
        let root = temp_dir.path().join(".otto");
        let run = root.join("otto-abc123").join("1700000000");
        fs::create_dir_all(&run).unwrap();

        let canonical = ensure_deletable_under_root(&run, &root).unwrap();
        assert!(canonical.starts_with(fs::canonicalize(&root).unwrap()));
    }

    #[test]
    fn test_resolve_otto_home_with_env() {
        let temp_dir = TempDir::new().unwrap();
        unsafe {
            std::env::set_var("OTTO_HOME", temp_dir.path());
        }
        let home = resolve_otto_home().unwrap();
        assert_eq!(home, temp_dir.path());
        unsafe {
            std::env::remove_var("OTTO_HOME");
        }
    }

    #[tokio::test]
    async fn test_auto_prune_disabled() {
        let temp_dir = TempDir::new().unwrap();
        let retention = RetentionSpec {
            auto_prune: false,
            ..Default::default()
        };
        // Should return immediately without error
        auto_prune(temp_dir.path(), &retention).await;
        // .last_prune should not be created
        assert!(!temp_dir.path().join(".last_prune").exists());
    }

    #[tokio::test]
    async fn test_auto_prune_throttle_skip() {
        let temp_dir = TempDir::new().unwrap();
        let marker = temp_dir.path().join(".last_prune");
        // Create a fresh marker (just now)
        fs::File::create(&marker).unwrap();

        let retention = RetentionSpec {
            auto_prune: true,
            prune_interval_hours: 24,
            ..Default::default()
        };

        // Should skip because marker is fresh
        auto_prune(temp_dir.path(), &retention).await;
        // Marker should still exist but not be re-touched significantly
    }

    #[tokio::test]
    async fn test_auto_prune_runs_when_stale() {
        let temp_dir = TempDir::new().unwrap();
        let marker = temp_dir.path().join(".last_prune");

        // Create a marker with old mtime
        fs::File::create(&marker).unwrap();
        let old_time = std::time::SystemTime::now() - Duration::from_secs(25 * 3600);
        filetime::set_file_mtime(&marker, filetime::FileTime::from_system_time(old_time)).unwrap();

        let retention = RetentionSpec {
            auto_prune: true,
            prune_interval_hours: 24,
            ..Default::default()
        };

        // Should prune and update marker
        auto_prune(temp_dir.path(), &retention).await;
        // Marker should be updated to recent time
        let meta = fs::metadata(&marker).unwrap();
        let age = meta.modified().unwrap().elapsed().unwrap();
        assert!(age < Duration::from_secs(5));
    }

    #[tokio::test]
    async fn test_auto_prune_creates_marker_when_missing() {
        let temp_dir = TempDir::new().unwrap();
        assert!(!temp_dir.path().join(".last_prune").exists());

        let retention = RetentionSpec {
            auto_prune: true,
            prune_interval_hours: 24,
            ..Default::default()
        };

        auto_prune(temp_dir.path(), &retention).await;
        assert!(temp_dir.path().join(".last_prune").exists());
    }

    // =========================================================================
    // Cache pruning tests
    // =========================================================================

    fn setup_cache_test(temp_dir: &TempDir) -> (PathBuf, PathBuf, PathBuf) {
        let project_dir = temp_dir.path().join("otto-abc123");
        let cache_dir = project_dir.join(".cache");
        let run_dir = project_dir.join("1234567890");
        let tasks_dir = run_dir.join("tasks").join("build");

        fs::create_dir_all(&cache_dir).unwrap();
        fs::create_dir_all(&tasks_dir).unwrap();

        // Create cached scripts
        fs::write(cache_dir.join("aabb1122.sh"), "#!/bin/bash\necho hi").unwrap();
        fs::write(cache_dir.join("ccdd3344.sh"), "#!/bin/bash\necho orphan").unwrap();

        // Create symlink from run to one cache entry
        #[cfg(unix)]
        std::os::unix::fs::symlink("../../../.cache/aabb1122.sh", tasks_dir.join("script.sh")).unwrap();

        (project_dir, cache_dir, run_dir)
    }

    #[test]
    fn test_prune_orphaned_cache_removes_unreferenced() {
        let temp_dir = TempDir::new().unwrap();
        let (_project_dir, cache_dir, _run_dir) = setup_cache_test(&temp_dir);

        prune_orphaned_cache(temp_dir.path()).unwrap();

        // Referenced cache entry should remain
        assert!(cache_dir.join("aabb1122.sh").exists());
        // Orphaned cache entry should be removed
        assert!(!cache_dir.join("ccdd3344.sh").exists());
    }

    #[test]
    fn test_prune_orphaned_cache_empty_otto_home() {
        let temp_dir = TempDir::new().unwrap();
        // Should not error on empty directory
        prune_orphaned_cache(temp_dir.path()).unwrap();
    }

    #[test]
    fn test_prune_orphaned_cache_no_cache_dir() {
        let temp_dir = TempDir::new().unwrap();
        let project_dir = temp_dir.path().join("otto-abc123");
        fs::create_dir_all(&project_dir).unwrap();
        // No .cache dir — should be fine
        prune_orphaned_cache(temp_dir.path()).unwrap();
    }

    #[test]
    fn test_prune_orphaned_cache_no_runs_deletes_all() {
        let temp_dir = TempDir::new().unwrap();
        let project_dir = temp_dir.path().join("otto-abc123");
        let cache_dir = project_dir.join(".cache");
        fs::create_dir_all(&cache_dir).unwrap();

        // Cache entries with no runs to reference them
        fs::write(cache_dir.join("aabb1122.sh"), "orphan1").unwrap();
        fs::write(cache_dir.join("ccdd3344.sh"), "orphan2").unwrap();

        prune_orphaned_cache(temp_dir.path()).unwrap();

        // All cache entries should be removed
        assert!(!cache_dir.join("aabb1122.sh").exists());
        assert!(!cache_dir.join("ccdd3344.sh").exists());
    }
}
