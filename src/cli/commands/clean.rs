use crate::cli::commands::format::{format_size, format_timestamp};
use eyre::Result;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::SystemTime;

use crate::executor::layout::parse_project_dir_name;
use crate::executor::pruning::ensure_deletable_under_root;
use crate::executor::state::{Retention, RunAge, RunMetadata, StateManager};
use crate::ports::StateStore;

/// Clean old otto run directories
#[derive(Debug, clap::Parser)]
#[command(name = "Clean")]
pub struct CleanCommand {
    /// Keep runs newer than this many days
    #[arg(long, default_value = "30")]
    pub keep_days: u64,

    /// Keep at least this many most recent runs (regardless of age)
    #[arg(long)]
    pub keep_last: Option<usize>,

    /// Keep failed runs for this many days (overrides --keep-days for failed runs)
    #[arg(long)]
    pub keep_failed: Option<u64>,

    /// Dry run - show what would be deleted without deleting
    #[arg(long)]
    pub dry_run: bool,

    /// Filter by project hash
    #[arg(long)]
    pub project_filter: Option<String>,

    /// Use filesystem scan instead of database (fallback mode)
    #[arg(long)]
    pub no_db: bool,

    /// Suppress output (used by auto-prune)
    #[arg(skip)]
    pub quiet: bool,
}

struct RunInfo {
    path: PathBuf,
    project_hash: String,
    timestamp: u64,
    age_days: u64,
    size_bytes: u64,
    ottofile_path: Option<PathBuf>,
}

impl CleanCommand {
    /// Which retention rule actually decided the selection, in words.
    ///
    /// The messages used to say "older than N days" unconditionally, including
    /// when `--keep-last` or `--keep-failed` was the rule that ran, and the
    /// empty-result message said "no runs older than N days" when the scan had
    /// returned every run regardless of age.
    fn retention_description(&self) -> String {
        let mut rules = Vec::new();
        if let Some(keep_last) = self.keep_last {
            rules.push(format!("keeping the {keep_last} newest"));
        }
        if let Some(keep_failed) = self.keep_failed {
            rules.push(format!("keeping failed runs for {keep_failed} days"));
        }
        rules.push(format!("keeping everything for {} days", self.keep_days));
        rules.join(", ")
    }

    /// Print a message unless quiet mode is enabled.
    fn print(&self, msg: &str) {
        if !self.quiet {
            println!("{msg}");
        }
    }

    pub async fn execute(&self) -> Result<()> {
        self.execute_with_store(None).await
    }

    /// Execute cleanup with an optional injected StateStore (for testing)
    pub async fn execute_with_store(&self, store: Option<Arc<dyn StateStore>>) -> Result<()> {
        if !self.no_db {
            // Use injected store or create default StateManager
            let store: Option<Arc<dyn StateStore>> =
                store.or_else(|| StateManager::try_new().map(|m| Arc::new(m) as Arc<dyn StateStore>));

            if let Some(store) = store {
                // Database-backed cleanup does not require `~/.otto` to
                // pre-exist: an injected store may be entirely in-memory
                // (tests), and `StateManager::new` creates the directory
                // itself on first use. The existence check only makes
                // sense for the filesystem-scan fallback below.
                return self.execute_with_database(store.as_ref()).await;
            }
            self.print("Database not available, falling back to filesystem scan...");
        }

        // Fallback to filesystem-based cleanup, which does require a real
        // `~/.otto` directory to scan.
        let otto_home = self.get_otto_home()?;
        if !otto_home.exists() {
            self.print("No ~/.otto directory found");
            return Ok(());
        }

        self.execute_with_filesystem(&otto_home).await
    }

    /// Execute cleanup using database queries
    async fn execute_with_database(&self, store: &dyn StateStore) -> Result<()> {
        self.print("Querying database for old runs...");

        let runs_to_delete = store.find_old_runs(
            self.keep_days,
            self.keep_last,
            self.keep_failed,
            self.project_filter.as_deref(),
        )?;

        if runs_to_delete.is_empty() {
            self.print("No runs matching deletion criteria found");
            return Ok(());
        }

        let total_size = runs_to_delete.iter().filter_map(|r| r.size_bytes).sum::<u64>();

        self.print(&format!(
            "\nFound {} runs to delete ({} total)",
            runs_to_delete.len(),
            format_size(total_size)
        ));

        if self.dry_run {
            self.print("\nDry run - showing what would be deleted:\n");
            for run in &runs_to_delete {
                let date_time = format_timestamp(run.timestamp);
                let ottofile_display = run
                    .ottofile_path
                    .as_ref()
                    .map(|p| p.display().to_string())
                    .unwrap_or_else(|| "<unknown>".to_string());
                let age_days = (SystemTime::now()
                    .duration_since(SystemTime::UNIX_EPOCH)?
                    .as_secs()
                    .saturating_sub(run.timestamp))
                    / (24 * 60 * 60);
                self.print(&format!(
                    "  {} - {} ({} days old, {}) [{}]",
                    date_time,
                    ottofile_display,
                    age_days,
                    format_size(run.size_bytes.unwrap_or(0)),
                    run.status.as_str()
                ));
            }
            self.print("\nRun without --dry-run to actually delete these runs");
        } else {
            self.print("\nDeleting runs...\n");
            let mut deleted_size = 0u64;
            let mut missing = 0usize;
            let mut failed = 0usize;

            for run in &runs_to_delete {
                // Whether there is actually a directory to reclaim, checked before
                // the delete. `delete_run` returns Ok(Some(..)) either way - it
                // logs a warning and removes the rows when the directory is
                // already gone - so reporting off its return value alone told the
                // user bytes had been freed that never existed. Rows whose
                // directory had been removed behind the database's back were
                // printed as `Deleted ... (4.9 KB)` and counted into the total.
                let reclaimable = run
                    .run_dir
                    .as_ref()
                    .is_none_or(|dir| std::fs::symlink_metadata(dir).is_ok());

                match store.delete_run(run.id, true) {
                    Ok(Some(_)) => {
                        let date_time = format_timestamp(run.timestamp);
                        let ottofile_display = run
                            .ottofile_path
                            .as_ref()
                            .map(|p| p.display().to_string())
                            .unwrap_or_else(|| "<unknown>".to_string());
                        if reclaimable {
                            deleted_size += run.size_bytes.unwrap_or(0);
                            self.print(&format!(
                                "  Deleted {} - {} ({})",
                                date_time,
                                ottofile_display,
                                format_size(run.size_bytes.unwrap_or(0))
                            ));
                        } else {
                            self.print(&format!(
                                "  Removed database rows for {} - {} (directory was already gone)",
                                date_time, ottofile_display
                            ));
                        }
                    }
                    Ok(None) => {
                        eprintln!("  Warning: Run {} not found in database", run.timestamp);
                        missing += 1;
                    }
                    Err(e) => {
                        eprintln!("  Error deleting run {}: {}", run.timestamp, e);
                        failed += 1;
                    }
                }
            }

            self.print(&format!("\nDeleted {} total", format_size(deleted_size)));

            // Counted and surfaced in the exit code, matching the filesystem
            // path. This path used to print each failure and still return
            // `Ok(())`, so a script driving `Clean` could not tell a clean
            // sweep from one that left behind a run it was told to remove.
            if missing > 0 || failed > 0 {
                return Err(eyre::eyre!(
                    "clean did not remove every run it selected: {missing} not found in the database, {failed} failed"
                ));
            }
        }

        Ok(())
    }

    /// Execute cleanup using filesystem scanning (fallback mode)
    async fn execute_with_filesystem(&self, otto_home: &Path) -> Result<()> {
        self.print(&format!("Scanning {} for old runs...", otto_home.display()));

        let now = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH)?.as_secs();
        let all_runs = self.scan_runs(otto_home, now)?;

        if all_runs.is_empty() {
            self.print(&format!("No runs found under {}", otto_home.display()));
            return Ok(());
        }

        // A directory scan cannot tell a failed run from a successful one - the
        // status only exists in the database - so `--keep-failed` is applied as
        // the longer of the two cutoffs for every run. That keeps more than
        // asked rather than deleting something the flag meant to protect, and it
        // says so instead of ignoring the flag, which is what it used to do.
        let keep_days = match self.keep_failed {
            Some(keep_failed) if keep_failed > self.keep_days => {
                eprintln!(
                    "  --keep-failed needs run status, which only the database has; \
                     keeping every run for {keep_failed} days in filesystem mode"
                );
                keep_failed
            }
            _ => self.keep_days,
        };

        // The same pure retention the database path uses. This was open-coded
        // here as an ascending sort plus `split_off`, which returned the tail:
        // `--keep-last 2` deleted the two newest runs and kept every older one.
        let policy = Retention {
            keep_days,
            keep_last: self.keep_last,
            keep_failed_days: None,
        };
        let ages: Vec<RunAge> = all_runs
            .iter()
            .map(|r| RunAge {
                timestamp: r.timestamp,
                failed: false,
            })
            .collect();

        let mut runs_to_delete: Vec<&RunInfo> = policy.expired(&ages, now).into_iter().map(|i| &all_runs[i]).collect();
        runs_to_delete.sort_by_key(|r| r.timestamp);

        if runs_to_delete.is_empty() {
            self.print("No runs to delete after applying retention policy");
            return Ok(());
        }

        let total_size = runs_to_delete.iter().map(|r| r.size_bytes).sum::<u64>();

        self.print(&format!(
            "\nFound {} runs to delete by {} ({} total)",
            runs_to_delete.len(),
            self.retention_description(),
            format_size(total_size)
        ));

        if self.dry_run {
            self.print("\nDry run - showing what would be deleted:\n");
            for run in &runs_to_delete {
                let date_time = format_timestamp(run.timestamp);
                let ottofile_display = run
                    .ottofile_path
                    .as_ref()
                    .map(|p| p.display().to_string())
                    .unwrap_or_else(|| "<unknown>".to_string());
                self.print(&format!(
                    "  [{}] {} - {} ({} days old, {})",
                    run.project_hash,
                    date_time,
                    ottofile_display,
                    run.age_days,
                    format_size(run.size_bytes)
                ));
            }
            self.print("\nRun without --dry-run to actually delete these runs");
        } else {
            self.print("\nDeleting runs...\n");
            let mut deleted_size = 0u64;
            // Counted, not just printed. `continue` alone let Clean refuse a
            // deletion, say so on stderr, and still exit 0 - a script driving it
            // could not tell a clean sweep from one that skipped a run it was
            // told to remove.
            let mut refused = 0usize;
            let mut failed = 0usize;

            for run in &runs_to_delete {
                // Never delete through a link and never delete outside the root
                // being cleaned. Checked again here, not only during the scan:
                // the scan and the delete are two separate walks of a tree the
                // user can change in between.
                if let Err(e) = ensure_deletable_under_root(&run.path, otto_home) {
                    eprintln!("  Refusing to delete {}: {}", run.path.display(), e);
                    refused += 1;
                    continue;
                }
                match fs::remove_dir_all(&run.path) {
                    Ok(()) => {
                        deleted_size += run.size_bytes;
                        let date_time = format_timestamp(run.timestamp);
                        let ottofile_display = run
                            .ottofile_path
                            .as_ref()
                            .map(|p| p.display().to_string())
                            .unwrap_or_else(|| "<unknown>".to_string());
                        self.print(&format!(
                            "  Deleted [{}] {} - {} ({})",
                            run.project_hash,
                            date_time,
                            ottofile_display,
                            format_size(run.size_bytes)
                        ));
                    }
                    Err(e) => {
                        eprintln!("  Failed to delete {}: {}", run.path.display(), e);
                        failed += 1;
                    }
                }
            }

            self.print(&format!("\nFreed {} of disk space", format_size(deleted_size)));

            if refused > 0 || failed > 0 {
                return Err(eyre::eyre!(
                    "clean did not remove every run it selected: {refused} refused, {failed} failed"
                ));
            }
        }

        Ok(())
    }

    /// Every run under `otto_home`, regardless of age.
    ///
    /// Retention is decided afterwards, over the whole list: `--keep-last`
    /// means "keep the N newest runs", and a scan that had already dropped the
    /// recent ones could not honour that.
    fn scan_runs(&self, otto_home: &Path, now: u64) -> Result<Vec<RunInfo>> {
        let mut runs = Vec::new();

        for entry in fs::read_dir(otto_home)? {
            let entry = entry?;
            let path = entry.path();

            // `is_dir()` follows symlinks; a symlinked project directory is a
            // directory somewhere else, and cleaning is not allowed to reach it.
            if entry.file_type()?.is_symlink() {
                eprintln!("  Skipping symlink {}", path.display());
                continue;
            }

            if !path.is_dir() {
                continue;
            }

            let dir_name = match path.file_name().and_then(|n| n.to_str()) {
                Some(name) => name,
                None => continue,
            };

            // A run root is `<name>-<hash>`, which is what `Workspace` creates.
            // This used to test for an `otto-` prefix and so skipped every
            // project not named "otto" - 220 of 222 directories in a real
            // `~/.otto`.
            let Some((_, project_hash)) = parse_project_dir_name(dir_name) else {
                continue;
            };

            // Match the hash exactly, as the database path does. A substring
            // match meant `--project-filter abc` also swept up `fabc1234`.
            if let Some(ref filter) = self.project_filter
                && project_hash != filter
            {
                continue;
            }

            // Scan timestamp directories within this project
            runs.extend(self.scan_project_runs(&path, project_hash, now)?);
        }

        Ok(runs)
    }

    fn scan_project_runs(&self, project_dir: &Path, project_hash: &str, now: u64) -> Result<Vec<RunInfo>> {
        let mut runs = Vec::new();

        for entry in fs::read_dir(project_dir)? {
            let entry = entry?;
            let path = entry.path();

            // Same rule as the project scan: a symlinked run directory is never
            // a deletion candidate, because deleting it deletes its target.
            if entry.file_type()?.is_symlink() {
                eprintln!("  Skipping symlink {}", path.display());
                continue;
            }

            if !path.is_dir() {
                continue;
            }

            let dir_name = match path.file_name().and_then(|n| n.to_str()) {
                Some(name) => name,
                None => continue,
            };

            // Skip .cache directory
            if dir_name == ".cache" {
                continue;
            }

            // Try to parse as a run directory name. Not a bare `parse::<u64>()`:
            // a same-second run is named `<timestamp>-<seq>` and would otherwise be
            // invisible here and leak forever.
            if let Some(timestamp) = crate::executor::layout::parse_run_dir_name(dir_name) {
                let age_days = now.saturating_sub(timestamp) / 86400;
                let size_bytes = Self::calculate_dir_size(&path)?;

                // Try to read ottofile path from run.yaml
                let ottofile_path = self.read_ottofile_path(&path);

                runs.push(RunInfo {
                    path,
                    project_hash: project_hash.to_string(),
                    timestamp,
                    age_days,
                    size_bytes,
                    ottofile_path,
                });
            }
        }

        Ok(runs)
    }

    fn read_ottofile_path(&self, run_dir: &Path) -> Option<PathBuf> {
        let run_yaml_path = run_dir.join("run.yaml");
        if !run_yaml_path.exists() {
            return None;
        }

        let content = fs::read_to_string(&run_yaml_path).ok()?;
        let metadata: RunMetadata = yaml_serde::from_str(&content).ok()?;
        metadata.ottofile
    }

    fn calculate_dir_size(path: &Path) -> Result<u64> {
        let mut total_size = 0u64;

        if path.is_dir() {
            for entry in fs::read_dir(path)? {
                let entry = entry?;

                // `is_dir()` and `metadata()` both follow symlinks. A link
                // inside a run directory pointing at a large or unreadable tree
                // would make this scan slow, or abort `Clean` through the `?`.
                // Sizing a run means sizing what the run owns, so links are
                // skipped - the same rule `scan_runs` applies one level up.
                let file_type = entry.file_type()?;
                if file_type.is_symlink() {
                    continue;
                }

                if file_type.is_dir() {
                    total_size += Self::calculate_dir_size(&entry.path())?;
                } else {
                    total_size += entry.metadata()?.len();
                }
            }
        }

        Ok(total_size)
    }

    fn get_otto_home(&self) -> Result<PathBuf> {
        crate::executor::layout::resolve_otto_home()
    }
}

#[path = "clean_tests.rs"]
mod tests;
