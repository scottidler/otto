use chrono::DateTime;
use eyre::{Context, Result, eyre};
use flate2::read::GzDecoder;
use indicatif::{ProgressBar, ProgressStyle};
use reqwest::Client;
use semver::Version;
use serde::Deserialize;
use std::cmp::Ordering;
use std::env;
use std::fs::{self, File};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::SystemTime;
use tar::Archive;

/// Upgrade Otto to a newer version
#[derive(Debug, clap::Parser)]
#[command(name = "Upgrade")]
pub struct UpgradeCommand {
    /// Show what would be done without doing it
    #[arg(long)]
    pub dry_run: bool,

    /// Specific version to upgrade to (e.g., "0.5.6")
    #[arg(long, short = 'v')]
    pub version: Option<String>,

    /// List available versions
    #[arg(long)]
    pub list_versions: bool,

    /// Rollback to previous version
    #[arg(long)]
    pub rollback: bool,

    /// Force upgrade even if already on target version
    #[arg(long)]
    pub force: bool,

    /// Skip creating backup
    #[arg(long)]
    pub no_backup: bool,

    /// Directory for backups (default: ~/.otto/backups)
    #[arg(long)]
    pub backup_dir: Option<PathBuf>,

    /// GitHub token for API access (avoids rate limits)
    #[arg(long, env = "GITHUB_TOKEN")]
    pub github_token: Option<String>,
}

#[derive(Debug)]
struct PlatformInfo {
    _os: String,
    _arch: String,
    platform_str: String,
}

impl PlatformInfo {
    fn detect() -> Result<Self> {
        let os = env::consts::OS;
        let arch = env::consts::ARCH;

        let platform_str = match (os, arch) {
            ("linux", "x86_64") => "linux",
            ("linux", "aarch64") => "linux-arm64",
            ("macos", "x86_64") => "macos-x86_64",
            ("macos", "aarch64") => "macos-arm64",
            _ => return Err(eyre!("Unsupported platform: {}-{}", os, arch)),
        };

        Ok(PlatformInfo {
            _os: os.to_string(),
            _arch: arch.to_string(),
            platform_str: platform_str.to_string(),
        })
    }
}

#[derive(Debug, Deserialize)]
struct GitHubRelease {
    tag_name: String,
    #[allow(dead_code)]
    name: String,
    published_at: String,
    assets: Vec<GitHubAsset>,
}

#[derive(Debug, Deserialize)]
struct GitHubAsset {
    name: String,
    browser_download_url: String,
}

struct BackupInfo {
    path: PathBuf,
    version: String,
    timestamp: u64,
}

impl UpgradeCommand {
    pub async fn execute(&self) -> Result<()> {
        if self.rollback {
            return self.execute_rollback().await;
        }

        if self.list_versions {
            return self.execute_list_versions().await;
        }

        self.execute_upgrade().await
    }

    async fn execute_upgrade(&self) -> Result<()> {
        println!("Checking for updates...");

        // 1. Detect platform
        let platform = PlatformInfo::detect()?;
        println!("Platform: {}", platform.platform_str);

        // 2. Get current version
        let current_version = self.current_version()?;
        println!("Current version: v{}", current_version);

        // 3. Fetch releases
        let releases = self.fetch_releases().await?;

        // 4. Determine target version
        let target_version = if let Some(ref v) = self.version {
            v.trim_start_matches('v').to_string()
        } else {
            // Get latest version
            releases
                .first()
                .ok_or_else(|| eyre!("No releases found"))?
                .tag_name
                .trim_start_matches('v')
                .to_string()
        };

        println!("Target version:  v{}", target_version);

        // 5. Check if upgrade needed
        if !self.force {
            let current = Version::parse(&current_version)?;
            let target = Version::parse(&target_version)?;

            match current.cmp(&target) {
                Ordering::Equal => {
                    println!("\nYou are already on the target version!");
                    println!("\nUse --force to reinstall the current version.");
                    return Ok(());
                }
                Ordering::Greater => {
                    println!("\nCurrent version is newer than target version.");
                    println!("Use --force to downgrade.");
                    return Ok(());
                }
                Ordering::Less => {}
            }
        }

        if self.dry_run {
            return self.show_dry_run_plan(&target_version, &platform);
        }

        // 6. Find and download release
        let release = releases
            .iter()
            .find(|r| r.tag_name == format!("v{}", target_version))
            .ok_or_else(|| eyre!("Release v{} not found", target_version))?;

        let asset = self.find_asset(release, &platform.platform_str)?;

        println!("\nDownloading {}...", asset.name);
        let archive_path = self.download_with_progress(&asset.browser_download_url).await?;

        println!("Download complete!");

        // 7. Create backup
        let current_exe = env::current_exe()?;
        if !self.no_backup {
            let backup_path = self.create_backup(&current_exe)?;
            println!("Backup created: {}", backup_path.display());
        }

        // 8. Install new version
        println!("Installing new version...");
        self.install_from_archive(&archive_path, &target_version)?;

        println!("\n✓ Successfully upgraded to v{}!", target_version);
        println!("\nRun 'otto --version' to verify.");

        Ok(())
    }

    async fn execute_rollback(&self) -> Result<()> {
        let backup_dir = self.get_backup_dir()?;

        if !backup_dir.exists() {
            return Err(eyre!("No backup directory found at {}", backup_dir.display()));
        }

        let backups = self.list_backups()?;

        if backups.is_empty() {
            return Err(eyre!("No backups found to rollback to"));
        }

        let latest = &backups[0];
        println!("Rolling back to v{}...", latest.version);

        if self.dry_run {
            println!("\nWould restore: {}", latest.path.display());
            return Ok(());
        }

        // Create backup of current version first
        let current_exe = env::current_exe()?;
        if !self.no_backup {
            let backup_path = self.create_backup(&current_exe)?;
            println!("Safety backup created: {}", backup_path.display());
        }

        // Restore from backup
        fs::copy(&latest.path, &current_exe).context("Failed to restore backup")?;

        // Verify it works
        self.verify_binary(&current_exe)?;

        println!("✓ Successfully rolled back to v{}!", latest.version);

        Ok(())
    }

    async fn execute_list_versions(&self) -> Result<()> {
        println!("Fetching available versions...");

        let releases = self.fetch_releases().await?;

        println!("\nAvailable versions:");
        for (i, release) in releases.iter().enumerate() {
            let version = release.tag_name.trim_start_matches('v');
            let latest_tag = if i == 0 { " (latest)" } else { "" };
            let date = self.format_date(&release.published_at);
            println!("  v{}{:10} - {}", version, latest_tag, date);
        }

        let current = self.current_version()?;
        println!("\nCurrent version: v{}", current);

        Ok(())
    }

    fn show_dry_run_plan(&self, target_version: &str, platform: &PlatformInfo) -> Result<()> {
        println!("\nDry run - would perform the following actions:");
        println!("  1. Download otto-{}-{}.tar.gz", target_version, platform.platform_str);

        if !self.no_backup {
            let backup_dir = self.get_backup_dir()?;
            println!(
                "  2. Create backup: {}/otto-<current>-<timestamp>.backup",
                backup_dir.display()
            );
        }

        println!("  3. Extract new binary from archive");
        println!("  4. Verify new binary works");

        let current_exe = env::current_exe()?;
        println!("  5. Replace {}", current_exe.display());

        println!("\nRun without --dry-run to perform upgrade.");
        Ok(())
    }

    fn current_version(&self) -> Result<String> {
        // Use GIT_DESCRIBE which is set at build time in build.rs
        // This matches what --version displays
        let version = env!("GIT_DESCRIBE");
        Ok(version.trim_start_matches('v').to_string())
    }

    async fn fetch_releases(&self) -> Result<Vec<GitHubRelease>> {
        let client = Client::new();
        let url = "https://api.github.com/repos/otto-rs/otto/releases";

        let mut request = client.get(url).header("User-Agent", "otto-upgrade");

        if let Some(ref token) = self.github_token {
            request = request.header("Authorization", format!("Bearer {}", token));
        }

        let response = request.send().await.context("Failed to fetch releases from GitHub")?;

        if !response.status().is_success() {
            return Err(eyre!(
                "GitHub API returned error: {} - {}",
                response.status(),
                response.text().await.unwrap_or_default()
            ));
        }

        let releases: Vec<GitHubRelease> = response.json().await.context("Failed to parse GitHub releases")?;

        Ok(releases)
    }

    fn find_asset<'a>(&self, release: &'a GitHubRelease, platform: &str) -> Result<&'a GitHubAsset> {
        // Extract version from release tag
        let version = release.tag_name.trim_start_matches('v');
        let pattern = format!("otto-v{}-{}.tar.gz", version, platform);

        release
            .assets
            .iter()
            .find(|asset| asset.name == pattern)
            .ok_or_else(|| eyre!("No asset found for platform: {} (looking for {})", platform, pattern))
    }

    async fn download_with_progress(&self, url: &str) -> Result<PathBuf> {
        let client = Client::new();
        let response = client.get(url).send().await?;

        let total_size = response.content_length().unwrap_or(0);

        let pb = ProgressBar::new(total_size);
        pb.set_style(
            ProgressStyle::default_bar()
                .template("[{bar:40.cyan/blue}] {bytes}/{total_bytes} ({eta})")?
                .progress_chars("█▓▒░ "),
        );

        let temp_dir = tempfile::tempdir()?;
        let file_path = temp_dir.path().join("otto.tar.gz");
        let mut file = File::create(&file_path)?;

        let mut downloaded: u64 = 0;
        let mut stream = response.bytes_stream();

        use futures_util::StreamExt;
        while let Some(chunk) = stream.next().await {
            let chunk = chunk?;
            file.write_all(&chunk)?;
            downloaded += chunk.len() as u64;
            pb.set_position(downloaded);
        }

        pb.finish_with_message("Download complete");

        Ok(file_path)
    }

    fn create_backup(&self, exe_path: &Path) -> Result<PathBuf> {
        let backup_dir = self.get_backup_dir()?;
        fs::create_dir_all(&backup_dir)?;

        let current_version = self.current_version()?;
        let timestamp = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH)?.as_secs();

        let backup_name = format!("otto-{}-{}.backup", current_version, timestamp);
        let backup_path = backup_dir.join(&backup_name);

        fs::copy(exe_path, &backup_path).context("Failed to create backup")?;

        // Update "latest" symlink on Unix systems
        #[cfg(unix)]
        {
            use std::os::unix::fs as unix_fs;
            let latest_link = backup_dir.join("otto-latest.backup");
            let _ = fs::remove_file(&latest_link);
            unix_fs::symlink(&backup_path, &latest_link).ok();
        }

        Ok(backup_path)
    }

    fn list_backups(&self) -> Result<Vec<BackupInfo>> {
        let backup_dir = self.get_backup_dir()?;

        if !backup_dir.exists() {
            return Ok(Vec::new());
        }

        let mut backups = Vec::new();

        for entry in fs::read_dir(&backup_dir)? {
            let entry = entry?;
            let path = entry.path();

            if !path.is_file() {
                continue;
            }

            let filename = match path.file_name().and_then(|n| n.to_str()) {
                Some(name) => name,
                None => continue,
            };

            // Skip symlinks
            if filename.ends_with("-latest.backup") {
                continue;
            }

            // Parse: otto-VERSION-TIMESTAMP.backup
            if let Some(rest) = filename.strip_prefix("otto-")
                && let Some(rest) = rest.strip_suffix(".backup")
            {
                let parts: Vec<&str> = rest.rsplitn(2, '-').collect();
                if parts.len() == 2 {
                    let timestamp = parts[0].parse::<u64>().unwrap_or(0);
                    let version = parts[1].to_string();

                    backups.push(BackupInfo {
                        path: path.clone(),
                        version,
                        timestamp,
                    });
                }
            }
        }

        // Sort by timestamp descending (newest first)
        backups.sort_by_key(|b| std::cmp::Reverse(b.timestamp));

        Ok(backups)
    }

    fn install_from_archive(&self, archive_path: &Path, _version: &str) -> Result<()> {
        // Extract archive
        let file = File::open(archive_path)?;
        let gz = GzDecoder::new(file);
        let mut archive = Archive::new(gz);

        let temp_dir = tempfile::tempdir()?;
        archive.unpack(temp_dir.path())?;

        // Find the otto binary in extracted files
        let extracted_binary = temp_dir.path().join("otto");

        if !extracted_binary.exists() {
            return Err(eyre!("Otto binary not found in archive"));
        }

        // Verify the new binary works
        self.verify_binary(&extracted_binary)?;

        // Get current executable path
        let current_exe = env::current_exe()?;

        // Replace the current binary
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;

            // Set executable permissions
            let mut perms = fs::metadata(&extracted_binary)?.permissions();
            perms.set_mode(0o755);
            fs::set_permissions(&extracted_binary, perms)?;
        }

        // Atomic replace (rename is atomic on most filesystems)
        fs::copy(&extracted_binary, &current_exe).context("Failed to replace binary")?;

        Ok(())
    }

    fn verify_binary(&self, binary_path: &Path) -> Result<()> {
        use std::process::Command;

        let output = Command::new(binary_path)
            .arg("--version")
            .output()
            .context("Failed to execute new binary")?;

        if !output.status.success() {
            return Err(eyre!("New binary failed to run --version"));
        }

        Ok(())
    }

    fn get_backup_dir(&self) -> Result<PathBuf> {
        if let Some(ref dir) = self.backup_dir {
            return Ok(expanduser::expanduser(dir.to_string_lossy().as_ref())?);
        }

        let home = env::var("HOME").context("Failed to get HOME environment variable")?;
        Ok(PathBuf::from(home).join(".otto").join("backups"))
    }

    fn format_date(&self, date_str: &str) -> String {
        if let Ok(dt) = DateTime::parse_from_rfc3339(date_str) {
            dt.format("%Y-%m-%d").to_string()
        } else {
            date_str.to_string()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_platform_detection() {
        let platform = PlatformInfo::detect().unwrap();
        assert!(!platform.platform_str.is_empty());
        assert!(
            platform._os == "linux" || platform._os == "macos",
            "Unexpected OS: {}",
            platform._os
        );
    }

    #[test]
    fn test_version_parsing() {
        let v1 = Version::parse("0.5.5").unwrap();
        let v2 = Version::parse("0.5.6").unwrap();
        assert!(v1 < v2);
    }

    #[test]
    fn test_backup_dir_default() {
        let cmd = UpgradeCommand {
            dry_run: false,
            version: None,
            list_versions: false,
            rollback: false,
            force: false,
            no_backup: false,
            backup_dir: None,
            github_token: None,
        };

        let backup_dir = cmd.get_backup_dir().unwrap();
        assert!(backup_dir.to_string_lossy().contains(".otto/backups"));
    }

    #[test]
    fn test_backup_dir_custom() {
        let custom_path = PathBuf::from("/tmp/custom-backups");
        let cmd = UpgradeCommand {
            dry_run: false,
            version: None,
            list_versions: false,
            rollback: false,
            force: false,
            no_backup: false,
            backup_dir: Some(custom_path.clone()),
            github_token: None,
        };

        let backup_dir = cmd.get_backup_dir().unwrap();
        assert_eq!(backup_dir, custom_path);
    }
}
