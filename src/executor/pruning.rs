use crate::cfg::otto::RetentionSpec;
use crate::cli::CleanCommand;
use eyre::Result;
use log::warn;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use crate::executor::layout::parse_project_dir_name;

/// How recently a cached script may have been touched and still be considered in
/// use. A run that has written its cache entry but not yet its symlink looks
/// exactly like an orphan, and deleting it out from under a concurrent run is
/// how the cache prune broke builds.
const CACHE_PRUNE_GRACE: Duration = Duration::from_secs(15 * 60);

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
        // Only process project run roots, which are `<name>-<hash>`. This used
        // to test for an `otto-` prefix, which matches only projects that happen
        // to be named "otto".
        if parse_project_dir_name(&dir_name).is_none() {
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
                    && !written_recently(&cache_path)
                {
                    log::debug!("Removing orphaned cache entry: {}", cache_path.display());
                    let _ = fs::remove_file(&cache_path);
                }
            }
        }
    }

    Ok(())
}

/// Whether `path` was modified inside the grace period, i.e. whether a run in
/// flight may still be about to reference it. Unreadable or future-dated times
/// count as recent: the safe answer is "leave it alone".
fn written_recently(path: &Path) -> bool {
    let Ok(modified) = fs::metadata(path).and_then(|m| m.modified()) else {
        return true;
    };
    match SystemTime::now().duration_since(modified) {
        Ok(age) => age < CACHE_PRUNE_GRACE,
        Err(_) => true,
    }
}

#[path = "pruning_tests.rs"]
mod tests;
