use crate::cli::commands::format::{format_size, format_timestamp};
use eyre::Result;
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::SystemTime;

use crate::executor::layout::{directory_size, parse_project_dir_name};
use crate::executor::pruning::ensure_deletable_under_root;
use crate::executor::runlock;
use crate::executor::state::{Retention, RunAge, RunMetadata, RunRecord, RunStatus, StateManager};
use crate::ports::StateStore;

/// The row cap `get_runs_with_filters` takes, when what is wanted is every row.
///
/// `i64::MAX` rather than `usize::MAX`: the SQLite store passes the cap through
/// `limit as i64`, where `usize::MAX` wraps to `-1`. That happens to mean "no
/// limit" to SQLite, which is the right answer for the wrong reason.
const ALL_ROWS: usize = i64::MAX as usize;

/// How many per-run delete errors are printed before they are summarised.
///
/// The errors are one per run that could not be deleted, and a prune that keeps
/// failing is a prune whose backlog grows every interval, so the unbounded form
/// got longer every time it ran.
const MAX_DELETE_ERRORS_SHOWN: usize = 3;

/// The stderr line for a run that could not be deleted: the error itself for
/// the first few, one summary line at the cap, then nothing.
///
/// Capped rather than gated on `quiet`, because a refused delete is worth
/// interrupting for. But there is one per run that could not be removed, and a
/// prune that keeps failing is a prune whose backlog grows every interval, so
/// the uncapped form got one line longer every time it ran. The total is in the
/// failure message the command returns.
fn delete_error_notice(shown: usize, timestamp: u64, error: &str) -> Option<String> {
    match shown.cmp(&MAX_DELETE_ERRORS_SHOWN) {
        std::cmp::Ordering::Less => Some(format!("  Error deleting run {timestamp}: {error}")),
        std::cmp::Ordering::Equal => {
            Some("  ... further delete errors not shown; the count is in the failure below".to_string())
        }
        std::cmp::Ordering::Greater => None,
    }
}

/// The stderr line for a run another pruner deleted first, or `None` under
/// `quiet`.
///
/// A row someone else deleted first is the expected outcome of a race, and
/// `auto_prune` runs with `quiet: true` on the way out of an ordinary `otto
/// <task>`. Ungated, a racing prune wrote one of these per selected run into
/// the middle of the user's build output, about something that went right.
///
/// Pure, and returned rather than printed, so the gate itself is testable:
/// stderr cannot be captured from a unit test, and the race that produces this
/// arm cannot be staged deterministically through the binary.
fn already_gone_notice(quiet: bool, timestamp: u64) -> Option<String> {
    if quiet {
        return None;
    }
    Some(format!("  Warning: Run {timestamp} not found in database"))
}

/// Clean old otto run directories
#[derive(Debug, clap::Parser)]
#[command(name = "Clean", bin_name = "otto Clean")]
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

    /// The tree to clean, when the caller has one in hand.
    ///
    /// Not a flag: `$OTTO_HOME` is the knob users have. `auto_prune` sets it,
    /// for the same reason it anchors the database path to the home it was
    /// given rather than to the environment. Both cleanup paths now walk the
    /// filesystem, so a caller that passes one home and gets another swept is
    /// how a unit test with a `TempDir` reaches the developer's real `~/.otto`.
    #[arg(skip)]
    pub otto_home: Option<PathBuf>,
}

struct RunInfo {
    path: PathBuf,
    project_hash: String,
    timestamp: u64,
    age_days: u64,
    size_bytes: u64,
    ottofile_path: Option<PathBuf>,
}

/// What one database-backed `Clean` deletes.
///
/// Two populations, counted apart, because they are not the same thing. The
/// first counts **rows**: a row the database can identify, whose directory goes
/// with it when there is one. The second counts **directories** reclaimed by
/// path. A run whose row records no directory is counted once in each, which is
/// correct rather than a double delete, and is why the report says `rows`.
#[derive(Default)]
struct Selection {
    /// Indices into the rows read from the store.
    rows: Vec<usize>,
    /// Indices into the scanned run directories.
    orphans: Vec<usize>,
    /// How many of the selected rows recorded no run directory at all. Reported
    /// rather than logged: the row is all that can be deleted for those, and
    /// until the sweep existed the directory they left was unreachable.
    nameless_rows: usize,
}

/// The ottofile a run came from, or a placeholder. Optional in the row and in
/// the run's own `run.yaml`.
fn display_ottofile(path: Option<&PathBuf>) -> String {
    path.map(|p| p.display().to_string())
        .unwrap_or_else(|| "<unknown>".to_string())
}

/// The directory a row names, or a placeholder saying it names none.
fn display_run_dir(dir: Option<&PathBuf>) -> String {
    dir.map(|p| p.display().to_string())
        .unwrap_or_else(|| "<no run directory recorded>".to_string())
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

    /// Execute cleanup using database queries, plus a sweep for the run
    /// directories no row names.
    ///
    /// The database knows about the runs it recorded. The disk knows about
    /// every run directory that exists, which is not the same set: a run whose
    /// store would not open is never recorded at all, a row deleted with its
    /// directory left behind takes the only pointer to it, and rows written
    /// before the directory was a column cannot say where their run wrote. From
    /// the default path those directories were unreachable forever - 1993 of
    /// them, 173 MB, on the author's machine - while `--no-db` could see every
    /// one. This selects over both and deletes both.
    async fn execute_with_database(&self, store: &dyn StateStore) -> Result<()> {
        self.print("Querying database for old runs...");

        let now = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH)?.as_secs();
        let rows = store.get_runs_with_filters(None, self.project_filter.as_deref(), ALL_ROWS)?;

        // Every present run directory, unfiltered, through the same scan
        // `--no-db` uses so the two modes cannot drift again on which directory
        // names count as a run. Never sourced from `find_old_runs`, not even
        // with `keep_last: None`: that applies retention itself, so the union
        // would be pre-filtered to the already-expired and `--keep-last` would
        // then keep the newest of those instead of the newest overall.
        let otto_home = self.get_otto_home()?;
        let present = if otto_home.exists() {
            self.print(&format!("Scanning {} for run directories...", otto_home.display()));
            self.scan_runs(&otto_home, now)?
        } else {
            Vec::new()
        };

        let selection = self.select(&rows, &present, now);

        // Only once both passes have found nothing. This return used to fire on
        // an empty row selection alone, which is precisely the case the sweep
        // exists for: a home full of directories no row named reported "nothing
        // to do" and left every one of them where it was.
        if selection.rows.is_empty() && selection.orphans.is_empty() {
            self.print("No runs matching deletion criteria found");
            return Ok(());
        }

        let rows_size: u64 = selection.rows.iter().filter_map(|&index| rows[index].size_bytes).sum();
        let orphans_size: u64 = selection.orphans.iter().map(|&index| present[index].size_bytes).sum();

        // Rows and directories, said separately, because they count different
        // things: one row can stand for a directory this pass will not touch,
        // and one directory can have no row at all.
        self.print(&format!(
            "\nFound {} rows from the database and {} orphaned directories to delete by {} ({} total)",
            selection.rows.len(),
            selection.orphans.len(),
            self.retention_description(),
            format_size(rows_size + orphans_size)
        ));
        if selection.nameless_rows > 0 {
            self.print(&format!(
                "  {} of those rows recorded no run directory; the rows go, and any directory they left is reclaimed by path",
                selection.nameless_rows
            ));
        }

        if self.dry_run {
            self.report_dry_run(&selection, &rows, &present, now);
            return Ok(());
        }

        self.delete_selection(store, &selection, &rows, &present, &otto_home)
    }

    /// Which rows and which directories this invocation deletes.
    ///
    /// A run reaches this in one of three shapes: a row whose directory is
    /// present, a row with no directory to reclaim (gone, or never recorded),
    /// or a directory no row names. Retention is applied **once** over the
    /// directories, so `--keep-last N` keeps N runs, not N rows plus N
    /// directories - which is what applying it to each population separately
    /// would do, and is twice what `--no-db` keeps.
    ///
    /// A row whose directory is present but held by a live run is in neither
    /// population: the scan skipped the directory, and deleting the row on its
    /// own would orphan a directory still being written to.
    fn select(&self, rows: &[RunRecord], present: &[RunInfo], now: u64) -> Selection {
        // Which scanned directory each row names, if any. Resolved rather than
        // compared as text: `$OTTO_HOME` and the path recorded at run start can
        // differ by a symlinked parent and still name one directory.
        let mut scanned_by_path: HashMap<PathBuf, usize> = HashMap::new();
        for (index, info) in present.iter().enumerate() {
            if let Ok(path) = fs::canonicalize(&info.path) {
                scanned_by_path.insert(path, index);
            }
        }

        let mut owners: Vec<Option<usize>> = vec![None; present.len()];
        // Rows the scan did not account for: no path recorded, a path that is
        // gone, or one the scan refused to treat as a run directory. A missing
        // path is not reconstructed, on purpose - the guess that used to be made
        // could not match a `<timestamp>-<seq>` directory or survive a moved
        // ottofile, and the sweep reclaims by path what the guess was for.
        let mut unmatched: Vec<usize> = Vec::new();
        for (index, row) in rows.iter().enumerate() {
            match row
                .run_dir
                .as_ref()
                .and_then(|dir| fs::canonicalize(dir).ok())
                .and_then(|dir| scanned_by_path.get(&dir).copied())
            {
                Some(directory) if owners[directory].is_none() => owners[directory] = Some(index),
                _ => unmatched.push(index),
            }
        }

        // `--keep-failed` needs a run's status, and a directory with no row has
        // none. The filesystem path answers that by widening `keep_days` to the
        // larger of the two cutoffs for everything it scans; here only the
        // status-unknown directories need it, because the rest have a row that
        // says. Marking those "failed" is how the widening is expressed through
        // the shared policy: it is the longer cutoff, so an unknown run is kept
        // rather than deleted by the flag that meant to protect it.
        let unknown_status_keeps_longer = self.keep_failed.is_some_and(|days| days > self.keep_days);

        let ages: Vec<RunAge> = present
            .iter()
            .zip(&owners)
            .map(|(info, owner)| RunAge {
                timestamp: info.timestamp,
                failed: match owner {
                    Some(index) => matches!(rows[*index].status, RunStatus::Failed),
                    None => unknown_status_keeps_longer,
                },
            })
            .collect();

        let over_directories = Retention {
            keep_days: self.keep_days,
            keep_last: self.keep_last,
            keep_failed_days: self.keep_failed,
        };

        let mut selection = Selection::default();
        for index in over_directories.expired(&ages, now) {
            match owners[index] {
                Some(row) => selection.rows.push(row),
                None => selection.orphans.push(index),
            }
        }

        // The unmatched rows are not in that union: there is no directory for
        // `--keep-last` to keep, and letting one consume a slot would make this
        // mode delete a directory `--no-db` keeps. They are still deleted, by
        // age alone, and by `--keep-failed` too since a row knows its own
        // status. Without that they accumulate without bound - 388 of them on
        // the author's machine.
        //
        // One exception, and it is why this is a lock test and not a `.exists()`
        // test: a directory a run is still using. Its row is the only thing that
        // points at it, so deleting the row alone would orphan a directory that
        // is still being written to.
        let mut deletable: Vec<usize> = Vec::new();
        for &index in &unmatched {
            if let Some(dir) = rows[index].run_dir.as_ref() {
                match runlock::try_take(dir) {
                    Ok(Some(_)) => {}
                    Ok(None) => continue,
                    Err(e) => {
                        eprintln!("  Skipping run {}: {}", rows[index].timestamp, e);
                        continue;
                    }
                }
            }
            deletable.push(index);
        }

        let by_age = Retention {
            keep_days: self.keep_days,
            keep_last: None,
            keep_failed_days: self.keep_failed,
        };
        let deletable_ages: Vec<RunAge> = deletable.iter().map(|&index| RunAge::from(&rows[index])).collect();
        for index in by_age.expired(&deletable_ages, now) {
            let row = deletable[index];
            if rows[row].run_dir.is_none() {
                selection.nameless_rows += 1;
            }
            selection.rows.push(row);
        }

        selection.rows.sort_by_key(|&index| rows[index].timestamp);
        selection.orphans.sort_by_key(|&index| present[index].timestamp);
        selection
    }

    /// Print what a real run would delete, run directory included.
    ///
    /// The path is printed for every line in both modes because that is the
    /// thing the two modes have to agree about: the same set of run
    /// directories, which a reader can only check if the output names them.
    fn report_dry_run(&self, selection: &Selection, rows: &[RunRecord], present: &[RunInfo], now: u64) {
        self.print("\nDry run - showing what would be deleted:\n");
        for &index in &selection.rows {
            let run = &rows[index];
            self.print(&format!(
                "  {} - {} ({} days old, {}) [{}] {}",
                format_timestamp(run.timestamp),
                display_ottofile(run.ottofile_path.as_ref()),
                now.saturating_sub(run.timestamp) / 86400,
                format_size(run.size_bytes.unwrap_or(0)),
                run.status.as_str(),
                display_run_dir(run.run_dir.as_ref())
            ));
        }
        for &index in &selection.orphans {
            let info = &present[index];
            self.print(&format!(
                "  [{}] {} - {} ({} days old, {}) {}",
                info.project_hash,
                format_timestamp(info.timestamp),
                display_ottofile(info.ottofile_path.as_ref()),
                info.age_days,
                format_size(info.size_bytes),
                info.path.display()
            ));
        }
        self.print("\nRun without --dry-run to actually delete these runs");
    }

    /// Delete what `select` chose: the rows first, then the directories no row
    /// names.
    ///
    /// Every directory is deleted while this process holds its run lock, not
    /// merely after testing it: two concurrent cleanups that each released
    /// after the test would each go on to delete.
    fn delete_selection(
        &self,
        store: &dyn StateStore,
        selection: &Selection,
        rows: &[RunRecord],
        present: &[RunInfo],
        otto_home: &Path,
    ) -> Result<()> {
        self.print("\nDeleting runs...\n");
        let mut deleted_size = 0u64;
        // Counted apart from `failed`, and deliberately not fatal: see the
        // exit-code comment below.
        let mut already_gone = 0usize;
        let mut in_use = 0usize;
        let mut refused = 0usize;
        let mut failed = 0usize;

        for &index in &selection.rows {
            let run = &rows[index];

            // Whether there is actually a directory to reclaim, checked before
            // the delete. `delete_run` returns Ok(Some(..)) either way - it logs
            // a warning and removes the rows when the directory is already gone
            // - so reporting off its return value alone told the user bytes had
            // been freed that never existed. Rows whose directory had been
            // removed behind the database's back were printed as `Deleted ...
            // (4.9 KB)` and counted into the total.
            let reclaimable = run
                .run_dir
                .as_ref()
                .is_some_and(|dir| std::fs::symlink_metadata(dir).is_ok());

            // Bound, not dropped: the lock has to outlive `delete_run`, which
            // unlinks the directory after committing the rows.
            let _run_lock = match run.run_dir.as_ref() {
                Some(dir) => match runlock::try_take(dir) {
                    Ok(Some(lock)) => Some(lock),
                    Ok(None) => {
                        self.print(&format!(
                            "  Skipping run {}: a run is still using its directory",
                            run.timestamp
                        ));
                        in_use += 1;
                        continue;
                    }
                    Err(e) => {
                        eprintln!("  Refusing to delete {}: {}", dir.display(), e);
                        refused += 1;
                        continue;
                    }
                },
                // No directory recorded, so nothing to lock and nothing to
                // unlink: the row goes on its own.
                None => None,
            };

            match store.delete_run(run.id, true, otto_home) {
                Ok(Some(_)) => {
                    let date_time = format_timestamp(run.timestamp);
                    let ottofile_display = display_ottofile(run.ottofile_path.as_ref());
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
                            "  Removed database rows for {} - {} (no directory of its own to reclaim)",
                            date_time, ottofile_display
                        ));
                    }
                }
                Ok(None) => {
                    if let Some(line) = already_gone_notice(self.quiet, run.timestamp) {
                        eprintln!("{line}");
                    }
                    already_gone += 1;
                }
                Err(e) => {
                    if let Some(line) = delete_error_notice(failed, run.timestamp, &e.to_string()) {
                        eprintln!("{line}");
                    }
                    failed += 1;
                }
            }
        }

        for &index in &selection.orphans {
            let info = &present[index];

            // Never delete through a link and never outside the tree being
            // cleaned. Checked here as well as during the scan: the scan and the
            // delete are two separate walks of a tree that can change between
            // them.
            if let Err(e) = ensure_deletable_under_root(&info.path, otto_home) {
                eprintln!("  Refusing to delete {}: {}", info.path.display(), e);
                refused += 1;
                continue;
            }

            // Same rule as above: held through `remove_dir_all`, not released
            // after the test.
            let _run_lock = match runlock::try_take(&info.path) {
                Ok(Some(lock)) => lock,
                Ok(None) => {
                    self.print(&format!("  Skipping {}: a run is still using it", info.path.display()));
                    in_use += 1;
                    continue;
                }
                Err(e) => {
                    eprintln!("  Refusing to delete {}: {}", info.path.display(), e);
                    refused += 1;
                    continue;
                }
            };

            match fs::remove_dir_all(&info.path) {
                Ok(()) => {
                    deleted_size += info.size_bytes;
                    self.print(&format!(
                        "  Deleted orphaned directory [{}] {} - {} ({})",
                        info.project_hash,
                        format_timestamp(info.timestamp),
                        display_ottofile(info.ottofile_path.as_ref()),
                        format_size(info.size_bytes)
                    ));
                }
                Err(e) => {
                    eprintln!("  Failed to delete {}: {}", info.path.display(), e);
                    failed += 1;
                }
            }
        }

        self.print(&format!("\nFreed {} of disk space", format_size(deleted_size)));

        // Counted and surfaced in the exit code, matching the filesystem path.
        // This path used to print each failure and still return `Ok(())`, so a
        // script driving `Clean` could not tell a clean sweep from one that left
        // behind a run it was told to remove.
        //
        // Two outcomes are deliberately outside that. A row that is already gone
        // (`delete_run` -> `Ok(None)`) is the normal result of two pruners
        // racing over one store, which `auto_prune` makes routine: the row is
        // gone, which is what was asked for. Folding it in failed the losing
        // racer and, through `auto_prune`'s `return` on that error, also skipped
        // the orphan-cache prune and the `.last_prune` touch, so auto-prune
        // re-fired on every subsequent run. A directory a run is still using is
        // the lock doing its job.
        if refused > 0 || failed > 0 {
            return Err(eyre::eyre!(
                "clean did not remove every run it selected: {refused} refused, {failed} failed \
                 ({already_gone} rows were already gone, {in_use} still in use)"
            ));
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

        let mut runs_to_delete: Vec<&RunInfo> = self
            .select_filesystem(&all_runs, now)
            .into_iter()
            .map(|i| &all_runs[i])
            .collect();
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
                // The run directory is named here and in the database path's
                // listing, because the set of directories is the thing the two
                // modes have to agree about and a reader can only check that if
                // the output says which ones.
                self.print(&format!(
                    "  [{}] {} - {} ({} days old, {}) {}",
                    run.project_hash,
                    format_timestamp(run.timestamp),
                    display_ottofile(run.ottofile_path.as_ref()),
                    run.age_days,
                    format_size(run.size_bytes),
                    run.path.display()
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
                // Held through the delete, not released after the test: two
                // concurrent cleanups that each let go would each go on to
                // delete.
                let _run_lock = match runlock::try_take(&run.path) {
                    Ok(Some(lock)) => lock,
                    Ok(None) => {
                        self.print(&format!("  Skipping {}: a run is still using it", run.path.display()));
                        continue;
                    }
                    Err(e) => {
                        eprintln!("  Refusing to delete {}: {}", run.path.display(), e);
                        refused += 1;
                        continue;
                    }
                };
                match fs::remove_dir_all(&run.path) {
                    Ok(()) => {
                        deleted_size += run.size_bytes;
                        self.print(&format!(
                            "  Deleted [{}] {} - {} ({})",
                            run.project_hash,
                            format_timestamp(run.timestamp),
                            display_ottofile(run.ottofile_path.as_ref()),
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

    /// Which scanned runs the filesystem path deletes, as indices into
    /// `all_runs`.
    ///
    /// Its own method so the two modes' selections can be compared directly, and
    /// so the comparison is against what the command actually does rather than
    /// against a copy of it.
    fn select_filesystem(&self, all_runs: &[RunInfo], now: u64) -> Vec<usize> {
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

        policy.expired(&ages, now)
    }

    /// The run directories this invocation would delete, sorted, whichever mode
    /// it is in.
    ///
    /// Both arms run the command's own selection, so what this returns is what
    /// the command does, not a restatement of it. Only directories that are
    /// actually there are listed: a row whose directory is already gone is still
    /// deleted, by the row pass, but it has no directory for the filesystem mode
    /// to have an opinion about.
    #[cfg(test)]
    fn select_for_test(&self, store: Option<Arc<dyn StateStore>>, now: u64) -> Result<Vec<String>> {
        let otto_home = self.get_otto_home()?;
        let present = if otto_home.exists() { self.scan_runs(&otto_home, now)? } else { Vec::new() };

        let mut directories: Vec<String> = if self.no_db {
            self.select_filesystem(&present, now)
                .into_iter()
                .map(|index| present[index].path.display().to_string())
                .collect()
        } else {
            let store = store.expect("database mode needs a store");
            let rows = store.get_runs_with_filters(None, self.project_filter.as_deref(), ALL_ROWS)?;
            let selection = self.select(&rows, &present, now);
            selection
                .rows
                .iter()
                .filter_map(|&index| rows[index].run_dir.clone())
                .filter(|dir| dir.exists())
                .chain(selection.orphans.iter().map(|&index| present[index].path.clone()))
                .map(|dir| dir.display().to_string())
                .collect()
        };

        directories.sort();
        Ok(directories)
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
                // A run that is still going holds the lock on its own directory.
                // Tested before the size walk, which is the expensive part, and
                // before the directory can reach any selection - including a
                // `--dry-run` selection, which is compared against a real one.
                //
                // The lock is released again here: holding one per scanned
                // directory would want thousands of open descriptors. It is
                // taken again, and held, around each delete.
                match runlock::try_take(&path) {
                    Ok(Some(_)) => {}
                    Ok(None) => {
                        self.print(&format!("  Skipping {}: a run is still using it", path.display()));
                        continue;
                    }
                    Err(e) => {
                        // Fail closed and say so: a lock that cannot be tested
                        // is not a lock that is free.
                        eprintln!("  Skipping {}: {}", path.display(), e);
                        continue;
                    }
                }

                let age_days = now.saturating_sub(timestamp) / 86400;
                let size_bytes = directory_size(&path)?;

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

    /// The tree this invocation cleans.
    ///
    /// `$OTTO_HOME` unless the caller handed one over. `auto_prune` hands one
    /// over, for the same reason it anchors the database path to the home it was
    /// given: it is called with a home, and resolving a different one from the
    /// environment means pruning somebody else's tree.
    fn get_otto_home(&self) -> Result<PathBuf> {
        match &self.otto_home {
            Some(home) => Ok(home.clone()),
            None => crate::executor::layout::resolve_otto_home(),
        }
    }
}

#[path = "clean_tests.rs"]
mod tests;
