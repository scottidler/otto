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
        // The home this function was GIVEN, for the same reason the database
        // path below is anchored to it: cleanup now walks the filesystem on
        // both paths, and resolving `$OTTO_HOME` here would sweep run
        // directories out of a tree this call was never pointed at.
        otto_home: Some(otto_home.to_path_buf()),
    };

    // Prune the store belonging to the home this function was GIVEN.
    //
    // `cmd.execute()` resolves the store from `$OTTO_DB_PATH`/`$OTTO_HOME`,
    // ignoring `otto_home` entirely. In production the two agree, so this read
    // as correct - but it means a caller that passes a different home prunes
    // somebody else's database. That is how two unit tests in this very module,
    // which pass a `TempDir` for the marker, came to delete rows from the
    // developer's real `~/.otto/otto.db` on an ordinary `cargo test`; a review
    // of this audit measured one `runs` row disappearing. It is the same defect
    // Phase 4 fixed globally - `OTTO_HOME` moving the run directories but not
    // the database - surviving in one function that took the home as an
    // argument and then did not use it.
    //
    // `$OTTO_DB_PATH` still wins, matching `DatabaseManager::default_db_path`'s
    // documented precedence; only the fallback is anchored to `otto_home`.
    let db_path = std::env::var_os("OTTO_DB_PATH")
        .map(PathBuf::from)
        .unwrap_or_else(|| otto_home.join("otto.db"));
    // A store that will not open is reported and then treated like every other
    // failure below: carried past, with the marker still written on the way
    // out. Returning here skipped the marker, so an interval that had been
    // attempted looked like an interval that had not, and auto-prune re-fired
    // on every subsequent run. It also made the marker a dishonest observable:
    // `auto_prune_throttle_boundary_is_exact` infers "pruning happened" from
    // the marker's mtime, so a SQLite hiccup on a CI runner (measured once:
    // `Failed to read the journal mode after WAL was refused`) read as a wrong
    // throttle decision rather than as the environment failure it was.
    let store = match crate::executor::state::StateManager::with_db_path(db_path) {
        Ok(store) => Some(store),
        Err(e) => {
            report_prune_failure(&format!("could not open the store: {e}"));
            None
        }
    };

    // Warned about and carried past, not returned on. The run prune, the
    // orphan-cache prune and the interval marker are three independent pieces
    // of best-effort housekeeping; one refusing to delete a run directory is no
    // reason to leave orphaned cache entries behind, and leaving the marker
    // untouched made auto-prune re-fire on every subsequent run instead of once
    // per interval.
    if let Some(store) = store
        && let Err(e) = cmd.execute_with_store(Some(std::sync::Arc::new(store))).await
    {
        report_prune_failure(&format!("{e}"));
    }

    // Prune orphaned cache entries
    if let Err(e) = prune_orphaned_cache(otto_home) {
        report_prune_failure(&format!("cache prune failed: {e}"));
    }

    // Touch marker file
    if let Err(e) = fs::File::create(&marker) {
        report_prune_failure(&format!("failed to update the .last_prune marker: {e}"));
    }
}

/// Tell the user, not just the log, that housekeeping is failing.
///
/// Every failure in `auto_prune` itself used to be a `warn!`, and
/// `setup_logging` sends the logger to `otto.log`, so nothing about the prune
/// as a whole reached the terminal. What did reach it was one line per refused
/// run, printed by `Clean` itself (`  Error deleting run <ts>: Refusing to
/// delete run directory`) with no indication of who asked or what it means.
/// Measured: a sabotaged run directory under auto-prune printed exactly that
/// one line and nothing else. The user is left to work out that an unnamed
/// background prune is failing, and that old runs are therefore accumulating.
///
/// This adds the two things that line cannot say on its own: that the failure
/// belongs to auto-prune, and what it costs.
///
/// The interval throttle is what makes this affordable to print. `auto_prune`
/// returns early unless `.last_prune` is older than `prune_interval_hours`, so
/// this is one line per interval (24h by default), not one per run, and the
/// marker is written even when the prune fails precisely so a failure cannot
/// re-fire on every invocation.
///
/// Still a `warn!` as well, so the log keeps the whole history.
fn report_prune_failure(detail: &str) {
    warn!("Auto-prune failed: {detail}");
    eprintln!("otto: auto-prune failed: {detail}");
    eprintln!("otto: old runs under $OTTO_HOME are not being cleaned up; `otto Clean --dry-run` shows what is there");
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
