use chrono::{DateTime, Utc};
use eyre::Result;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::SystemTime;

use crate::executor::pruning::ensure_deletable_under_root;
use crate::executor::state::{RunMetadata, StateManager};
use crate::ports::StateStore;

/// Clean old otto run directories
#[derive(Debug, clap::Parser)]
#[command(name = "clean")]
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
            self.format_size(total_size)
        ));

        if self.dry_run {
            self.print("\nDry run - showing what would be deleted:\n");
            for run in &runs_to_delete {
                let date_time = self.format_timestamp(run.timestamp);
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
                    self.format_size(run.size_bytes.unwrap_or(0)),
                    run.status.as_str()
                ));
            }
            self.print("\nRun without --dry-run to actually delete these runs");
        } else {
            self.print("\nDeleting runs...\n");
            let mut deleted_size = 0u64;

            for run in &runs_to_delete {
                match store.delete_run(run.timestamp, true) {
                    Ok(Some(_)) => {
                        deleted_size += run.size_bytes.unwrap_or(0);
                        let date_time = self.format_timestamp(run.timestamp);
                        let ottofile_display = run
                            .ottofile_path
                            .as_ref()
                            .map(|p| p.display().to_string())
                            .unwrap_or_else(|| "<unknown>".to_string());
                        self.print(&format!(
                            "  Deleted {} - {} ({})",
                            date_time,
                            ottofile_display,
                            self.format_size(run.size_bytes.unwrap_or(0))
                        ));
                    }
                    Ok(None) => {
                        eprintln!("  Warning: Run {} not found in database", run.timestamp);
                    }
                    Err(e) => {
                        eprintln!("  Error deleting run {}: {}", run.timestamp, e);
                    }
                }
            }

            self.print(&format!("\nDeleted {} total", self.format_size(deleted_size)));
        }

        Ok(())
    }

    /// Execute cleanup using filesystem scanning (fallback mode)
    async fn execute_with_filesystem(&self, otto_home: &Path) -> Result<()> {
        self.print(&format!("Scanning {} for old runs...", otto_home.display()));

        let mut runs_to_delete = self.scan_for_old_runs(otto_home)?;

        if runs_to_delete.is_empty() {
            self.print(&format!("No runs older than {} days found", self.keep_days));
            return Ok(());
        }

        runs_to_delete.sort_by_key(|r| r.timestamp);

        // Apply --keep-last logic if specified
        if let Some(keep_last) = self.keep_last {
            // Keep the N most recent runs
            if runs_to_delete.len() > keep_last {
                runs_to_delete = runs_to_delete.split_off(runs_to_delete.len() - keep_last);
            } else {
                runs_to_delete.clear();
            }
        }

        if runs_to_delete.is_empty() {
            self.print("No runs to delete after applying retention policy");
            return Ok(());
        }

        let total_size = runs_to_delete.iter().map(|r| r.size_bytes).sum::<u64>();

        self.print(&format!(
            "\nFound {} runs older than {} days ({} total)",
            runs_to_delete.len(),
            self.keep_days,
            self.format_size(total_size)
        ));

        if self.dry_run {
            self.print("\nDry run - showing what would be deleted:\n");
            for run in &runs_to_delete {
                let date_time = self.format_timestamp(run.timestamp);
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
                    self.format_size(run.size_bytes)
                ));
            }
            self.print("\nRun without --dry-run to actually delete these runs");
        } else {
            self.print("\nDeleting runs...\n");
            let mut deleted_size = 0u64;

            for run in &runs_to_delete {
                // Never delete through a link and never delete outside the root
                // being cleaned. Checked again here, not only during the scan:
                // the scan and the delete are two separate walks of a tree the
                // user can change in between.
                if let Err(e) = ensure_deletable_under_root(&run.path, otto_home) {
                    eprintln!("  Refusing to delete {}: {}", run.path.display(), e);
                    continue;
                }
                match fs::remove_dir_all(&run.path) {
                    Ok(()) => {
                        deleted_size += run.size_bytes;
                        let date_time = self.format_timestamp(run.timestamp);
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
                            self.format_size(run.size_bytes)
                        ));
                    }
                    Err(e) => {
                        eprintln!("  Failed to delete {}: {}", run.path.display(), e);
                    }
                }
            }

            self.print(&format!("\nFreed {} of disk space", self.format_size(deleted_size)));
        }

        Ok(())
    }

    fn scan_for_old_runs(&self, otto_home: &Path) -> Result<Vec<RunInfo>> {
        let now = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH)?.as_secs();

        let mut runs_to_delete = Vec::new();

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

            // Skip otto.db and other non-project directories
            if dir_name == "otto.db" || !dir_name.starts_with("otto-") {
                continue;
            }

            // Apply project filter if specified
            if let Some(ref filter) = self.project_filter
                && !dir_name.contains(filter)
            {
                continue;
            }

            // Extract project hash from directory name (e.g., "otto-6b20a2e4" -> "6b20a2e4")
            let project_hash = dir_name.strip_prefix("otto-").unwrap_or("unknown").to_string();

            // Scan timestamp directories within this project
            runs_to_delete.extend(self.scan_project_runs(&path, &project_hash, now)?);
        }

        Ok(runs_to_delete)
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

            // Try to parse as timestamp
            if let Ok(timestamp) = dir_name.parse::<u64>() {
                let age_seconds = now.saturating_sub(timestamp);
                let age_days = age_seconds / 86400;

                if age_days > self.keep_days {
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
        }

        Ok(runs)
    }

    fn read_ottofile_path(&self, run_dir: &Path) -> Option<PathBuf> {
        let run_yaml_path = run_dir.join("run.yaml");
        if !run_yaml_path.exists() {
            return None;
        }

        let content = fs::read_to_string(&run_yaml_path).ok()?;
        let metadata: RunMetadata = serde_yaml::from_str(&content).ok()?;
        metadata.ottofile
    }

    fn format_timestamp(&self, timestamp: u64) -> String {
        let dt = DateTime::from_timestamp(timestamp as i64, 0).unwrap_or(DateTime::<Utc>::MIN_UTC);
        dt.format("%Y-%m-%d %H:%M:%S").to_string()
    }

    fn format_size(&self, bytes: u64) -> String {
        if bytes < 1024 {
            format!("{bytes} B")
        } else if bytes < 1024 * 1024 {
            format!("{:.1} KB", bytes as f64 / 1024.0)
        } else if bytes < 1024 * 1024 * 1024 {
            format!("{:.1} MB", bytes as f64 / (1024.0 * 1024.0))
        } else {
            format!("{:.1} GB", bytes as f64 / (1024.0 * 1024.0 * 1024.0))
        }
    }

    fn calculate_dir_size(path: &Path) -> Result<u64> {
        let mut total_size = 0u64;

        if path.is_dir() {
            for entry in fs::read_dir(path)? {
                let entry = entry?;
                let entry_path = entry.path();

                if entry_path.is_dir() {
                    total_size += Self::calculate_dir_size(&entry_path)?;
                } else {
                    total_size += entry.metadata()?.len();
                }
            }
        }

        Ok(total_size)
    }

    fn get_otto_home(&self) -> Result<PathBuf> {
        crate::executor::pruning::resolve_otto_home()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::executor::state::RunStatus;
    use crate::ports::MemoryStateStore;
    use std::fs;
    use tempfile::TempDir;

    fn create_test_run(base_dir: &Path, project: &str, timestamp: u64, size_kb: usize) -> Result<()> {
        let run_dir = base_dir
            .join(format!("otto-{}", project))
            .join(timestamp.to_string())
            .join("tasks");
        fs::create_dir_all(&run_dir)?;

        let file_path = run_dir.join("test.log");
        let content = vec![0u8; size_kb * 1024];
        fs::write(file_path, content)?;

        Ok(())
    }

    #[tokio::test]
    async fn test_scan_empty_directory() -> Result<()> {
        let temp_dir = TempDir::new()?;
        let cmd = CleanCommand {
            keep_days: 30,
            keep_last: None,
            keep_failed: None,
            dry_run: true,
            project_filter: None,
            no_db: true,
            quiet: false,
        };

        let runs = cmd.scan_for_old_runs(temp_dir.path())?;
        assert_eq!(runs.len(), 0);
        Ok(())
    }

    /// Reproduced under a scratch HOME: a symlinked project directory under
    /// `~/.otto` reported `Deleted [deadbeef] ...`, the real directory it pointed
    /// at was gone, and the symlink was still there. `is_dir()` follows links.
    #[tokio::test]
    async fn clean_never_deletes_through_a_symlinked_project_directory() -> Result<()> {
        let temp_dir = TempDir::new()?;
        let otto_home = temp_dir.path().join(".otto");
        fs::create_dir_all(&otto_home)?;

        let victim = temp_dir.path().join("otto-victim");
        fs::create_dir_all(victim.join("1000000000").join("tasks"))?;
        fs::write(victim.join("1000000000").join("tasks").join("data.log"), "precious")?;

        std::os::unix::fs::symlink(&victim, otto_home.join("otto-deadbeef"))?;

        let cmd = CleanCommand {
            keep_days: 1,
            keep_last: None,
            keep_failed: None,
            dry_run: false,
            project_filter: None,
            no_db: true,
            quiet: true,
        };

        cmd.execute_with_filesystem(&otto_home).await?;

        assert!(
            victim.join("1000000000").join("tasks").join("data.log").exists(),
            "the symlink target must survive"
        );
        assert!(
            otto_home.join("otto-deadbeef").exists(),
            "the link itself is left alone"
        );
        Ok(())
    }

    /// Same rule one level down: a symlinked *run* directory inside a real
    /// project directory is not a deletion candidate either.
    #[tokio::test]
    async fn clean_never_deletes_through_a_symlinked_run_directory() -> Result<()> {
        let temp_dir = TempDir::new()?;
        let otto_home = temp_dir.path().join(".otto");
        let project = otto_home.join("otto-abc123");
        fs::create_dir_all(&project)?;

        let victim = temp_dir.path().join("victim");
        fs::create_dir_all(&victim)?;
        fs::write(victim.join("precious.txt"), "keep me")?;

        std::os::unix::fs::symlink(&victim, project.join("1000000000"))?;

        let cmd = CleanCommand {
            keep_days: 1,
            keep_last: None,
            keep_failed: None,
            dry_run: false,
            project_filter: None,
            no_db: true,
            quiet: true,
        };

        cmd.execute_with_filesystem(&otto_home).await?;

        assert!(victim.join("precious.txt").exists(), "the symlink target must survive");
        Ok(())
    }

    #[tokio::test]
    async fn test_scan_with_old_runs() -> Result<()> {
        let temp_dir = TempDir::new()?;

        let now = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH)?.as_secs();
        let old_timestamp = now - (40 * 86400); // 40 days old
        let recent_timestamp = now - (10 * 86400); // 10 days old

        create_test_run(temp_dir.path(), "abc123", old_timestamp, 100)?;
        create_test_run(temp_dir.path(), "abc123", recent_timestamp, 50)?;

        let cmd = CleanCommand {
            keep_days: 30,
            keep_last: None,
            keep_failed: None,
            dry_run: true,
            project_filter: None,
            no_db: true,
            quiet: false,
        };
        let runs = cmd.scan_for_old_runs(temp_dir.path())?;

        // Should only find the 40-day-old run
        assert_eq!(runs.len(), 1);
        assert!(runs[0].age_days >= 39 && runs[0].age_days <= 41);
        Ok(())
    }

    #[tokio::test]
    async fn test_calculate_dir_size() -> Result<()> {
        let temp_dir = TempDir::new()?;
        create_test_run(temp_dir.path(), "test", 1234567890, 100)?;

        let run_dir = temp_dir.path().join("otto-test").join("1234567890");
        let size = CleanCommand::calculate_dir_size(&run_dir)?;

        // Should be approximately 100KB (may vary slightly due to filesystem overhead)
        assert!(size >= 100 * 1024);
        assert!(size < 110 * 1024);
        Ok(())
    }

    #[tokio::test]
    async fn test_project_filter() -> Result<()> {
        let temp_dir = TempDir::new()?;
        let now = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH)?.as_secs();
        let old_timestamp = now - (40 * 86400);

        create_test_run(temp_dir.path(), "abc123", old_timestamp, 100)?;
        create_test_run(temp_dir.path(), "def456", old_timestamp, 100)?;

        let cmd = CleanCommand {
            keep_days: 30,
            keep_last: None,
            keep_failed: None,
            dry_run: true,
            project_filter: Some("abc123".to_string()),
            no_db: true,
            quiet: false,
        };
        let runs = cmd.scan_for_old_runs(temp_dir.path())?;

        // Should only find runs from abc123 project
        assert_eq!(runs.len(), 1);
        assert!(runs[0].path.to_string_lossy().contains("abc123"));
        Ok(())
    }

    #[tokio::test]
    async fn test_read_ottofile_path_with_metadata() -> Result<()> {
        let temp_dir = TempDir::new()?;
        let run_dir = temp_dir.path().join("test_run");
        fs::create_dir_all(&run_dir)?;

        let metadata = RunMetadata::minimal(
            Some(PathBuf::from("/path/to/otto.yml")),
            "abc123".to_string(),
            1234567890,
        );
        let yaml_content = serde_yaml::to_string(&metadata)?;
        fs::write(run_dir.join("run.yaml"), yaml_content)?;

        let cmd = CleanCommand {
            keep_days: 30,
            keep_last: None,
            keep_failed: None,
            dry_run: true,
            project_filter: None,
            no_db: true,
            quiet: false,
        };
        let ottofile_path = cmd.read_ottofile_path(&run_dir);

        assert_eq!(ottofile_path, Some(PathBuf::from("/path/to/otto.yml")));
        Ok(())
    }

    #[tokio::test]
    async fn test_read_ottofile_path_missing_file() -> Result<()> {
        let temp_dir = TempDir::new()?;
        let run_dir = temp_dir.path().join("test_run");
        fs::create_dir_all(&run_dir)?;

        let cmd = CleanCommand {
            keep_days: 30,
            keep_last: None,
            keep_failed: None,
            dry_run: true,
            project_filter: None,
            no_db: true,
            quiet: false,
        };
        let ottofile_path = cmd.read_ottofile_path(&run_dir);

        assert_eq!(ottofile_path, None);
        Ok(())
    }

    #[tokio::test]
    async fn test_read_ottofile_path_no_ottofile_field() -> Result<()> {
        let temp_dir = TempDir::new()?;
        let run_dir = temp_dir.path().join("test_run");
        fs::create_dir_all(&run_dir)?;

        let metadata = RunMetadata::minimal(None, "abc123".to_string(), 1234567890);
        let yaml_content = serde_yaml::to_string(&metadata)?;
        fs::write(run_dir.join("run.yaml"), yaml_content)?;

        let cmd = CleanCommand {
            keep_days: 30,
            keep_last: None,
            keep_failed: None,
            dry_run: true,
            project_filter: None,
            no_db: true,
            quiet: false,
        };
        let ottofile_path = cmd.read_ottofile_path(&run_dir);

        assert_eq!(ottofile_path, None);
        Ok(())
    }

    #[tokio::test]
    async fn test_read_ottofile_path_malformed_yaml() -> Result<()> {
        let temp_dir = TempDir::new()?;
        let run_dir = temp_dir.path().join("test_run");
        fs::create_dir_all(&run_dir)?;

        fs::write(run_dir.join("run.yaml"), "invalid: yaml: content: {")?;

        let cmd = CleanCommand {
            keep_days: 30,
            keep_last: None,
            keep_failed: None,
            dry_run: true,
            project_filter: None,
            no_db: true,
            quiet: false,
        };
        let ottofile_path = cmd.read_ottofile_path(&run_dir);

        assert_eq!(ottofile_path, None);
        Ok(())
    }

    #[tokio::test]
    async fn test_format_timestamp() -> Result<()> {
        let cmd = CleanCommand {
            keep_days: 30,
            keep_last: None,
            keep_failed: None,
            dry_run: true,
            project_filter: None,
            no_db: true,
            quiet: false,
        };

        // Test a known timestamp
        let timestamp = 1609459200; // 2021-01-01 00:00:00 UTC
        let formatted = cmd.format_timestamp(timestamp);

        assert_eq!(formatted, "2021-01-01 00:00:00");
        Ok(())
    }

    #[tokio::test]
    async fn test_format_size_bytes() -> Result<()> {
        let cmd = CleanCommand {
            keep_days: 30,
            keep_last: None,
            keep_failed: None,
            dry_run: true,
            project_filter: None,
            no_db: true,
            quiet: false,
        };

        assert_eq!(cmd.format_size(0), "0 B");
        assert_eq!(cmd.format_size(512), "512 B");
        assert_eq!(cmd.format_size(1023), "1023 B");
        Ok(())
    }

    #[tokio::test]
    async fn test_format_size_kilobytes() -> Result<()> {
        let cmd = CleanCommand {
            keep_days: 30,
            keep_last: None,
            keep_failed: None,
            dry_run: true,
            project_filter: None,
            no_db: true,
            quiet: false,
        };

        assert_eq!(cmd.format_size(1024), "1.0 KB");
        assert_eq!(cmd.format_size(2048), "2.0 KB");
        assert_eq!(cmd.format_size(1536), "1.5 KB");
        assert_eq!(cmd.format_size(100 * 1024), "100.0 KB");
        Ok(())
    }

    #[tokio::test]
    async fn test_format_size_megabytes() -> Result<()> {
        let cmd = CleanCommand {
            keep_days: 30,
            keep_last: None,
            keep_failed: None,
            dry_run: true,
            project_filter: None,
            no_db: true,
            quiet: false,
        };

        assert_eq!(cmd.format_size(1024 * 1024), "1.0 MB");
        assert_eq!(cmd.format_size(5 * 1024 * 1024), "5.0 MB");
        assert_eq!(cmd.format_size(1536 * 1024), "1.5 MB");
        assert_eq!(cmd.format_size(100 * 1024 * 1024), "100.0 MB");
        Ok(())
    }

    #[tokio::test]
    async fn test_format_size_gigabytes() -> Result<()> {
        let cmd = CleanCommand {
            keep_days: 30,
            keep_last: None,
            keep_failed: None,
            dry_run: true,
            project_filter: None,
            no_db: true,
            quiet: false,
        };

        assert_eq!(cmd.format_size(1024 * 1024 * 1024), "1.0 GB");
        assert_eq!(cmd.format_size(5 * 1024 * 1024 * 1024), "5.0 GB");
        assert_eq!(cmd.format_size(1536 * 1024 * 1024), "1.5 GB");
        Ok(())
    }

    #[tokio::test]
    async fn test_runs_sorted_by_timestamp() -> Result<()> {
        let temp_dir = TempDir::new()?;
        let now = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH)?.as_secs();

        let timestamp1 = now - (60 * 86400); // 60 days old
        let timestamp2 = now - (45 * 86400); // 45 days old
        let timestamp3 = now - (50 * 86400); // 50 days old

        create_test_run(temp_dir.path(), "abc123", timestamp2, 100)?;
        create_test_run(temp_dir.path(), "abc123", timestamp1, 100)?;
        create_test_run(temp_dir.path(), "abc123", timestamp3, 100)?;

        let cmd = CleanCommand {
            keep_days: 30,
            keep_last: None,
            keep_failed: None,
            dry_run: true,
            project_filter: None,
            no_db: true,
            quiet: false,
        };
        let mut runs = cmd.scan_for_old_runs(temp_dir.path())?;

        // Sort by timestamp
        runs.sort_by_key(|r| r.timestamp);

        // Should be sorted oldest to newest
        assert_eq!(runs.len(), 3);
        assert_eq!(runs[0].timestamp, timestamp1);
        assert_eq!(runs[1].timestamp, timestamp3);
        assert_eq!(runs[2].timestamp, timestamp2);
        Ok(())
    }

    #[tokio::test]
    async fn test_scan_with_ottofile_metadata() -> Result<()> {
        let temp_dir = TempDir::new()?;
        let now = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH)?.as_secs();
        let old_timestamp = now - (40 * 86400);

        let project_dir = temp_dir.path().join("otto-abc123");
        let run_dir = project_dir.join(old_timestamp.to_string());
        let tasks_dir = run_dir.join("tasks");
        fs::create_dir_all(&tasks_dir)?;

        let file_path = tasks_dir.join("test.log");
        let content = vec![0u8; 100 * 1024];
        fs::write(file_path, content)?;

        let metadata = RunMetadata::minimal(
            Some(PathBuf::from("/test/project/otto.yml")),
            "abc123".to_string(),
            old_timestamp,
        );
        let yaml_content = serde_yaml::to_string(&metadata)?;
        fs::write(run_dir.join("run.yaml"), yaml_content)?;

        let cmd = CleanCommand {
            keep_days: 30,
            keep_last: None,
            keep_failed: None,
            dry_run: true,
            project_filter: None,
            no_db: true,
            quiet: false,
        };
        let runs = cmd.scan_for_old_runs(temp_dir.path())?;

        assert_eq!(runs.len(), 1);
        assert_eq!(runs[0].project_hash, "abc123");
        assert_eq!(runs[0].timestamp, old_timestamp);
        assert_eq!(runs[0].ottofile_path, Some(PathBuf::from("/test/project/otto.yml")));
        assert!(runs[0].size_bytes >= 100 * 1024);
        Ok(())
    }

    #[tokio::test]
    async fn test_scan_without_ottofile_metadata() -> Result<()> {
        let temp_dir = TempDir::new()?;
        let now = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH)?.as_secs();
        let old_timestamp = now - (40 * 86400);

        create_test_run(temp_dir.path(), "def456", old_timestamp, 100)?;

        let cmd = CleanCommand {
            keep_days: 30,
            keep_last: None,
            keep_failed: None,
            dry_run: true,
            project_filter: None,
            no_db: true,
            quiet: false,
        };
        let runs = cmd.scan_for_old_runs(temp_dir.path())?;

        assert_eq!(runs.len(), 1);
        assert_eq!(runs[0].project_hash, "def456");
        assert_eq!(runs[0].ottofile_path, None);
        Ok(())
    }

    // ========================================
    // Database-based cleanup tests using MemoryStateStore
    // ========================================

    fn now_timestamp() -> u64 {
        std::time::SystemTime::now()
            .duration_since(std::time::SystemTime::UNIX_EPOCH)
            .unwrap()
            .as_secs()
    }

    fn create_store_with_runs() -> Arc<MemoryStateStore> {
        let store = MemoryStateStore::new();
        let now = now_timestamp();

        // Create old successful run (40 days old)
        let old_meta = RunMetadata::minimal(
            Some(PathBuf::from("/project1/otto.yml")),
            "abc123".to_string(),
            now - (40 * 86400),
        );
        store.record_run_start(&old_meta).unwrap();
        store
            .record_run_complete(now - (40 * 86400), RunStatus::Success, Some(100_000))
            .unwrap();

        // Create old failed run (35 days old)
        let old_failed_meta = RunMetadata::minimal(
            Some(PathBuf::from("/project1/otto.yml")),
            "abc123".to_string(),
            now - (35 * 86400),
        );
        store.record_run_start(&old_failed_meta).unwrap();
        store
            .record_run_complete(now - (35 * 86400), RunStatus::Failed, Some(50_000))
            .unwrap();

        // Create recent run (10 days old)
        let recent_meta = RunMetadata::minimal(
            Some(PathBuf::from("/project1/otto.yml")),
            "abc123".to_string(),
            now - (10 * 86400),
        );
        store.record_run_start(&recent_meta).unwrap();
        store
            .record_run_complete(now - (10 * 86400), RunStatus::Success, Some(75_000))
            .unwrap();

        // Create run from different project (45 days old)
        let other_project_meta = RunMetadata::minimal(
            Some(PathBuf::from("/project2/otto.yml")),
            "def456".to_string(),
            now - (45 * 86400),
        );
        store.record_run_start(&other_project_meta).unwrap();
        store
            .record_run_complete(now - (45 * 86400), RunStatus::Success, Some(200_000))
            .unwrap();

        Arc::new(store)
    }

    #[tokio::test]
    async fn test_execute_with_database_empty_store() -> Result<()> {
        let store = Arc::new(MemoryStateStore::new());
        let cmd = CleanCommand {
            keep_days: 30,
            keep_last: None,
            keep_failed: None,
            dry_run: true,
            project_filter: None,
            no_db: false,
            quiet: false,
        };

        let result = cmd.execute_with_store(Some(store)).await;
        assert!(result.is_ok());
        Ok(())
    }

    #[tokio::test]
    async fn test_execute_with_database_dry_run() -> Result<()> {
        let store = create_store_with_runs();

        // Verify initial state
        let initial_runs = store.get_recent_runs(100, None)?;
        assert_eq!(initial_runs.len(), 4);

        let cmd = CleanCommand {
            keep_days: 30,
            keep_last: None,
            keep_failed: None,
            dry_run: true,
            project_filter: None,
            no_db: false,
            quiet: false,
        };

        let result = cmd.execute_with_store(Some(store.clone())).await;
        assert!(result.is_ok());

        // Dry run should not delete anything
        let final_runs = store.get_recent_runs(100, None)?;
        assert_eq!(final_runs.len(), 4);
        Ok(())
    }

    #[tokio::test]
    async fn test_execute_with_database_actual_delete() -> Result<()> {
        let store = create_store_with_runs();

        // Verify initial state
        let initial_runs = store.get_recent_runs(100, None)?;
        assert_eq!(initial_runs.len(), 4);

        let cmd = CleanCommand {
            keep_days: 30,
            keep_last: None,
            keep_failed: None,
            dry_run: false,
            project_filter: None,
            no_db: false,
            quiet: false,
        };

        let result = cmd.execute_with_store(Some(store.clone())).await;
        assert!(result.is_ok());

        // Should have deleted 3 old runs (40 day, 35 day, 45 day)
        let final_runs = store.get_recent_runs(100, None)?;
        assert_eq!(final_runs.len(), 1);
        Ok(())
    }

    #[tokio::test]
    async fn test_execute_with_database_project_filter() -> Result<()> {
        let store = create_store_with_runs();

        let cmd = CleanCommand {
            keep_days: 30,
            keep_last: None,
            keep_failed: None,
            dry_run: false,
            project_filter: Some("abc123".to_string()),
            no_db: false,
            quiet: false,
        };

        let result = cmd.execute_with_store(Some(store.clone())).await;
        assert!(result.is_ok());

        // Should have deleted 2 old runs from abc123, kept def456's old run
        let final_runs = store.get_recent_runs(100, None)?;
        // Remaining: recent abc123 + old def456
        assert_eq!(final_runs.len(), 2);
        Ok(())
    }

    #[tokio::test]
    async fn test_execute_with_database_keep_last() -> Result<()> {
        let store = create_store_with_runs();

        let cmd = CleanCommand {
            keep_days: 0, // Would delete everything
            keep_last: Some(2),
            keep_failed: None,
            dry_run: false,
            project_filter: None,
            no_db: false,
            quiet: false,
        };

        let result = cmd.execute_with_store(Some(store.clone())).await;
        assert!(result.is_ok());

        // Should keep the 2 most recent runs
        let final_runs = store.get_recent_runs(100, None)?;
        assert_eq!(final_runs.len(), 2);
        Ok(())
    }

    #[tokio::test]
    async fn test_execute_with_database_keep_failed_longer() -> Result<()> {
        let store = create_store_with_runs();

        let cmd = CleanCommand {
            keep_days: 30,
            keep_last: None,
            keep_failed: Some(60), // Keep failed runs for 60 days
            dry_run: false,
            project_filter: None,
            no_db: false,
            quiet: false,
        };

        let result = cmd.execute_with_store(Some(store.clone())).await;
        assert!(result.is_ok());

        // Should have deleted 2 old successful runs (40 day, 45 day)
        // but kept the 35-day failed run (within 60-day retention)
        let final_runs = store.get_recent_runs(100, None)?;
        assert_eq!(final_runs.len(), 2);
        Ok(())
    }

    #[tokio::test]
    async fn test_find_old_runs_basic() -> Result<()> {
        let store = create_store_with_runs();

        let old_runs = store.find_old_runs(30, None, None, None)?;

        // Should find 3 runs older than 30 days
        assert_eq!(old_runs.len(), 3);
        Ok(())
    }

    #[tokio::test]
    async fn test_find_old_runs_with_keep_last() -> Result<()> {
        let store = create_store_with_runs();

        let old_runs = store.find_old_runs(0, Some(2), None, None)?;

        // Should find 2 runs to delete (keeping 2 most recent)
        assert_eq!(old_runs.len(), 2);
        Ok(())
    }

    #[tokio::test]
    async fn test_find_old_runs_with_project_filter() -> Result<()> {
        let store = create_store_with_runs();

        let old_runs = store.find_old_runs(30, None, None, Some("abc123"))?;

        // Should find 2 runs older than 30 days from abc123 project
        assert_eq!(old_runs.len(), 2);
        Ok(())
    }

    #[tokio::test]
    async fn test_find_old_runs_with_keep_failed() -> Result<()> {
        let store = create_store_with_runs();

        let old_runs = store.find_old_runs(30, None, Some(60), None)?;

        // Should find 2 successful runs older than 30 days
        // Failed run (35 days) should not be included (within 60-day retention)
        assert_eq!(old_runs.len(), 2);
        Ok(())
    }

    #[tokio::test]
    async fn test_delete_run_from_store() -> Result<()> {
        let store = create_store_with_runs();
        let now = now_timestamp();
        let old_timestamp = now - (40 * 86400);

        // Verify run exists
        let initial_runs = store.get_recent_runs(100, None)?;
        assert!(initial_runs.iter().any(|r| r.timestamp == old_timestamp));

        // Delete it
        let deleted = store.delete_run(old_timestamp, false)?;
        assert!(deleted.is_some());

        // Verify it's gone
        let final_runs = store.get_recent_runs(100, None)?;
        assert!(!final_runs.iter().any(|r| r.timestamp == old_timestamp));
        Ok(())
    }

    #[tokio::test]
    async fn test_delete_nonexistent_run() -> Result<()> {
        let store = Arc::new(MemoryStateStore::new());

        let deleted = store.delete_run(9999999999, false)?;
        assert!(deleted.is_none());
        Ok(())
    }

    // ========================================
    // Regression: db-backed cleanup must not depend on a pre-existing
    // `~/.otto` directory (Phase 0, `2026-06-10-code-review-remediation.md`).
    // `execute_with_store` used to check `otto_home.exists()` before ever
    // looking at the injected store, so on a runner with no populated
    // `~/.otto` (unlike a developer's machine), every db-backed clean test
    // silently short-circuited to "No ~/.otto directory found" and passed
    // or failed on stale assumptions rather than on the store's state.
    // ========================================

    #[tokio::test]
    #[serial_test::serial]
    async fn test_execute_with_database_ignores_missing_otto_home() -> Result<()> {
        // Point OTTO_HOME at a directory that does not exist. If the old
        // `otto_home.exists()` early-return were still in place, this would
        // print "No ~/.otto directory found" and skip the store entirely,
        // leaving all 4 runs in place.
        let temp_dir = TempDir::new()?;
        let missing_otto_home = temp_dir.path().join("does-not-exist");
        unsafe {
            std::env::set_var("OTTO_HOME", &missing_otto_home);
        }

        let store = create_store_with_runs();
        let cmd = CleanCommand {
            keep_days: 30,
            keep_last: None,
            keep_failed: None,
            dry_run: false,
            project_filter: None,
            no_db: false,
            quiet: true,
        };

        let result = cmd.execute_with_store(Some(store.clone())).await;
        unsafe {
            std::env::remove_var("OTTO_HOME");
        }
        result?;

        // The 3 old runs (40, 35, 45 days) should have been deleted via the
        // injected store despite `OTTO_HOME` not existing on disk.
        let final_runs = store.get_recent_runs(100, None)?;
        assert_eq!(final_runs.len(), 1);
        assert!(
            !missing_otto_home.exists(),
            "clean must not create the otto home directory"
        );
        Ok(())
    }

    #[tokio::test]
    #[serial_test::serial]
    async fn test_get_otto_home_honors_otto_home_env() -> Result<()> {
        let temp_dir = TempDir::new()?;
        unsafe {
            std::env::set_var("OTTO_HOME", temp_dir.path());
        }

        let cmd = CleanCommand {
            keep_days: 30,
            keep_last: None,
            keep_failed: None,
            dry_run: true,
            project_filter: None,
            no_db: true,
            quiet: true,
        };
        let resolved = cmd.get_otto_home();
        unsafe {
            std::env::remove_var("OTTO_HOME");
        }

        assert_eq!(resolved?, temp_dir.path());
        Ok(())
    }
}
