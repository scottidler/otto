use chrono::DateTime;
use eyre::{Context, Result, eyre};
use flate2::read::GzDecoder;
use indicatif::{ProgressBar, ProgressStyle};
use log::debug;
use reqwest::Client;
use semver::Version;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::cmp::Ordering;
use std::env;
use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};
use tar::Archive;
use tempfile::TempDir;

/// GitHub API endpoint listing otto's releases.
const RELEASES_URL: &str = "https://api.github.com/repos/otto-rs/otto/releases";

/// User-Agent GitHub requires on API requests.
const USER_AGENT: &str = "otto-upgrade";

/// Time allowed to establish a connection to the release host.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(30);

/// Time allowed between response bytes. A connection that stops delivering data
/// fails here instead of hanging the upgrade forever.
const READ_TIMEOUT: Duration = Duration::from_secs(60);

/// Whole-request budget for the small metadata requests (release JSON, checksum
/// sibling). The archive download deliberately has no total cap, only the
/// between-bytes `READ_TIMEOUT`, so a slow link is not mistaken for a stall.
const METADATA_TIMEOUT: Duration = Duration::from_secs(60);

/// ETXTBSY: exec of a file that is still open for writing anywhere on the
/// system. It is transient by definition - a `fork` in another thread that
/// momentarily inherits the write descriptor is enough to produce it - so
/// verifying a freshly written binary retries rather than declaring it broken.
const ETXTBSY: i32 = 26;

/// Attempts and spacing for that retry.
const VERIFY_ATTEMPTS: u32 = 10;
const VERIFY_RETRY_DELAY: Duration = Duration::from_millis(50);

/// A downloaded release archive and the directory that owns it.
///
/// Holding the `TempDir` here is what keeps the archive on disk until the
/// caller is finished with it; returning a bare `PathBuf` deleted the archive at
/// the end of the download call.
#[derive(Debug)]
struct DownloadedArchive {
    path: PathBuf,
    _dir: TempDir,
}

impl DownloadedArchive {
    fn path(&self) -> &Path {
        &self.path
    }
}

/// Build the single HTTP client this command reuses for every request.
fn build_http_client(connect_timeout: Duration, read_timeout: Duration) -> Result<Client> {
    debug!(
        "build_http_client: connect_timeout={:?} read_timeout={:?}",
        connect_timeout, read_timeout
    );
    Client::builder()
        .connect_timeout(connect_timeout)
        .read_timeout(read_timeout)
        .user_agent(USER_AGENT)
        .build()
        .context("Failed to build HTTP client")
}

/// Read the sha256 digest out of a `.sha256` sibling file.
///
/// The published format is `<hex>  <filename>`, matching `sha256sum`; only the
/// digest is used, and anything that is not a 64-character hex string is an
/// error rather than a value that silently fails to match later.
fn parse_sha256_manifest(body: &str) -> Result<String> {
    debug!("parse_sha256_manifest: body_len={}", body.len());
    let digest = body
        .split_whitespace()
        .next()
        .ok_or_else(|| eyre!("Checksum file is empty"))?;

    if digest.len() != 64 || !digest.chars().all(|c| c.is_ascii_hexdigit()) {
        return Err(eyre!("Checksum file does not start with a sha256 digest: {:?}", digest));
    }

    Ok(digest.to_ascii_lowercase())
}

/// Compute the sha256 of a file as lowercase hex.
fn sha256_file(path: &Path) -> Result<String> {
    debug!("sha256_file: path={}", path.display());
    let mut file = File::open(path).with_context(|| format!("Failed to open {} for hashing", path.display()))?;
    let mut hasher = Sha256::new();
    let mut buf = [0u8; 64 * 1024];

    loop {
        let read = file
            .read(&mut buf)
            .with_context(|| format!("Failed to read {} while hashing", path.display()))?;
        if read == 0 {
            break;
        }
        hasher.update(&buf[..read]);
    }

    Ok(hex::encode(hasher.finalize()))
}

/// Copy `source` next to `target` under a temporary name and return that path.
///
/// Staging in the target's own directory is what makes the final rename atomic:
/// `rename` across filesystems fails, and the previous `fs::copy` over a running
/// executable is both non-atomic and ETXTBSY on Linux.
fn stage_beside(source: &Path, target: &Path) -> Result<PathBuf> {
    debug!("stage_beside: source={} target={}", source.display(), target.display());
    let dir = target
        .parent()
        .ok_or_else(|| eyre!("Install target has no parent directory: {}", target.display()))?;
    let name = target
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or_else(|| eyre!("Install target has no file name: {}", target.display()))?;

    let staged = dir.join(format!(".{}.upgrade-{}", name, std::process::id()));
    fs::copy(source, &staged)
        .with_context(|| format!("Failed to stage {} at {}", source.display(), staged.display()))?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = fs::metadata(&staged)?.permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&staged, perms)
            .with_context(|| format!("Failed to set permissions on {}", staged.display()))?;
    }

    Ok(staged)
}

/// Rename a staged binary over the target, removing the staged file if the
/// rename fails so a failed upgrade leaves no debris beside the binary.
fn commit_staged(staged: &Path, target: &Path) -> Result<()> {
    debug!("commit_staged: staged={} target={}", staged.display(), target.display());
    if let Err(err) = fs::rename(staged, target) {
        let _ = fs::remove_file(staged);
        return Err(err).with_context(|| format!("Failed to install {} over {}", staged.display(), target.display()));
    }
    Ok(())
}

/// Install `source` at `target` by staging beside it and renaming into place.
fn install_binary(source: &Path, target: &Path) -> Result<()> {
    debug!(
        "install_binary: source={} target={}",
        source.display(),
        target.display()
    );
    let staged = stage_beside(source, target)?;
    commit_staged(&staged, target)
}

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

        // These four strings are the release asset suffixes: they must match
        // install.sh's get_suffix() and the release workflow's matrix exactly,
        // or find_asset looks for a tarball that was never published.
        let platform_str = match (os, arch) {
            ("linux", "x86_64") => "linux-amd64",
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

        // 3. Fetch releases (one client, reused for metadata and download)
        let client = build_http_client(CONNECT_TIMEOUT, READ_TIMEOUT)?;
        let releases = self.fetch_releases(&client, RELEASES_URL).await?;

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
        let archive = self.download_and_verify(&client, &asset.browser_download_url).await?;

        println!("Download complete!");

        // 7. Create backup
        let current_exe = env::current_exe()?;
        if !self.no_backup {
            let backup_path = self.create_backup(&current_exe)?;
            println!("Backup created: {}", backup_path.display());
        }

        // 8. Install new version
        println!("Installing new version...");
        self.install_from_archive(archive.path(), &current_exe)?;

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

        // Verify the backup before it replaces anything
        self.verify_binary(&latest.path)?;

        // Restore from backup: stage beside the target, then rename
        install_binary(&latest.path, &current_exe).context("Failed to restore backup")?;

        println!("✓ Successfully rolled back to v{}!", latest.version);

        Ok(())
    }

    async fn execute_list_versions(&self) -> Result<()> {
        println!("Fetching available versions...");

        let client = build_http_client(CONNECT_TIMEOUT, READ_TIMEOUT)?;
        let releases = self.fetch_releases(&client, RELEASES_URL).await?;

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

    async fn fetch_releases(&self, client: &Client, url: &str) -> Result<Vec<GitHubRelease>> {
        debug!("fetch_releases: url={}", url);
        let mut request = client.get(url).timeout(METADATA_TIMEOUT);

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

    /// Download a release archive and verify it against its `.sha256` sibling.
    ///
    /// The returned `DownloadedArchive` owns the temporary directory, so the
    /// archive stays on disk for as long as the caller holds it.
    async fn download_and_verify(&self, client: &Client, url: &str) -> Result<DownloadedArchive> {
        debug!("download_and_verify: url={}", url);
        let archive = self.download_with_progress(client, url).await?;

        let checksum_url = format!("{}.sha256", url);
        println!("Verifying checksum...");
        let expected = self.fetch_expected_sha256(client, &checksum_url).await?;
        let actual = sha256_file(archive.path())?;

        if !actual.eq_ignore_ascii_case(&expected) {
            return Err(eyre!(
                "Checksum mismatch for {}: expected {}, got {}. Aborting without installing.",
                url,
                expected,
                actual
            ));
        }

        Ok(archive)
    }

    async fn fetch_expected_sha256(&self, client: &Client, url: &str) -> Result<String> {
        debug!("fetch_expected_sha256: url={}", url);
        let response = client
            .get(url)
            .timeout(METADATA_TIMEOUT)
            .send()
            .await
            .with_context(|| format!("Failed to fetch checksum from {}", url))?
            .error_for_status()
            .with_context(|| format!("Checksum file not available at {}", url))?;

        let body = response
            .text()
            .await
            .with_context(|| format!("Failed to read checksum from {}", url))?;

        parse_sha256_manifest(&body)
    }

    async fn download_with_progress(&self, client: &Client, url: &str) -> Result<DownloadedArchive> {
        debug!("download_with_progress: url={}", url);
        let response = client
            .get(url)
            .send()
            .await
            .with_context(|| format!("Failed to download {}", url))?
            .error_for_status()
            .with_context(|| format!("Download failed for {}", url))?;

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
            let chunk = chunk.with_context(|| format!("Download interrupted for {}", url))?;
            file.write_all(&chunk)?;
            downloaded += chunk.len() as u64;
            pb.set_position(downloaded);
        }

        file.flush()?;
        pb.finish_with_message("Download complete");

        Ok(DownloadedArchive {
            path: file_path,
            _dir: temp_dir,
        })
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

    /// Extract the archive, verify the binary inside it, and install it at
    /// `target`. Every failure here happens before `target` is touched.
    fn install_from_archive(&self, archive_path: &Path, target: &Path) -> Result<()> {
        debug!(
            "install_from_archive: archive={} target={}",
            archive_path.display(),
            target.display()
        );
        let file = File::open(archive_path)
            .with_context(|| format!("Failed to open release archive at {}", archive_path.display()))?;
        let gz = GzDecoder::new(file);
        let mut archive = Archive::new(gz);

        let temp_dir = tempfile::tempdir()?;
        archive
            .unpack(temp_dir.path())
            .with_context(|| format!("Failed to extract release archive at {}", archive_path.display()))?;

        // Find the otto binary in extracted files
        let extracted_binary = temp_dir.path().join("otto");

        if !extracted_binary.exists() {
            return Err(eyre!("Otto binary not found in archive"));
        }

        // Set executable permissions before verifying: the archive may not carry them
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;

            let mut perms = fs::metadata(&extracted_binary)?.permissions();
            perms.set_mode(0o755);
            fs::set_permissions(&extracted_binary, perms)?;
        }

        // Verify the new binary works
        self.verify_binary(&extracted_binary)?;

        // Stage beside the target and rename: the rename is the atomic step
        install_binary(&extracted_binary, target)
    }

    fn verify_binary(&self, binary_path: &Path) -> Result<()> {
        debug!("verify_binary: path={}", binary_path.display());
        use std::process::Command;

        let mut attempt = 0;

        loop {
            attempt += 1;

            match Command::new(binary_path).arg("--version").output() {
                Ok(output) => {
                    if !output.status.success() {
                        return Err(eyre!("New binary failed to run --version"));
                    }
                    return Ok(());
                }
                Err(err) => {
                    let busy = cfg!(unix) && err.raw_os_error() == Some(ETXTBSY);
                    if busy && attempt < VERIFY_ATTEMPTS {
                        debug!("verify_binary: ETXTBSY on attempt {}, retrying", attempt);
                        std::thread::sleep(VERIFY_RETRY_DELAY);
                        continue;
                    }
                    return Err(err).context("Failed to execute new binary");
                }
            }
        }
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
    use std::collections::HashMap;
    use std::io::{BufRead, BufReader};
    use std::net::TcpListener;
    use std::os::unix::fs::PermissionsExt;
    use std::process::Command;
    use std::sync::Arc;

    /// What the stub server does with a request for a given path.
    enum Route {
        Body(Vec<u8>),
        Stall,
    }

    /// Minimal HTTP/1.1 stub bound to an ephemeral localhost port. It lets the
    /// upgrade path be exercised through the real reqwest client without a
    /// network and without a fixture release on GitHub.
    fn spawn_stub(routes: HashMap<String, Route>) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind stub server");
        let base = format!("http://{}", listener.local_addr().expect("stub addr"));
        let routes = Arc::new(routes);

        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(stream) = stream else { continue };
                let routes = Arc::clone(&routes);
                std::thread::spawn(move || serve(stream, &routes));
            }
        });

        base
    }

    fn serve(mut stream: std::net::TcpStream, routes: &HashMap<String, Route>) {
        let Some(path) = read_request_path(&stream) else {
            return;
        };

        match routes.get(&path) {
            Some(Route::Body(body)) => {
                let _ = write!(
                    stream,
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                );
                let _ = stream.write_all(body);
            }
            Some(Route::Stall) => {
                // Headers promise a body that never arrives: the classic stall.
                let _ = write!(
                    stream,
                    "HTTP/1.1 200 OK\r\nContent-Length: 4096\r\nConnection: close\r\n\r\n"
                );
                let _ = stream.flush();
                std::thread::sleep(Duration::from_secs(10));
            }
            None => {
                let _ = write!(
                    stream,
                    "HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                );
            }
        }

        let _ = stream.flush();
    }

    fn read_request_path(stream: &std::net::TcpStream) -> Option<String> {
        let mut reader = BufReader::new(stream.try_clone().ok()?);
        let mut request_line = String::new();
        reader.read_line(&mut request_line).ok()?;

        loop {
            let mut header = String::new();
            if reader.read_line(&mut header).ok()? == 0 || header == "\r\n" || header == "\n" {
                break;
            }
        }

        request_line.split_whitespace().nth(1).map(|p| p.to_string())
    }

    /// Build a release tarball holding an executable `otto` that reports
    /// `version` when run with `--version`.
    fn fixture_tarball(scratch: &Path, version: &str) -> Vec<u8> {
        let staging = scratch.join(format!("fixture-{}", version));
        fs::create_dir_all(&staging).expect("fixture staging dir");

        let binary = staging.join("otto");
        fs::write(&binary, format!("#!/bin/sh\necho \"otto {}\"\n", version)).expect("write fixture binary");
        fs::set_permissions(&binary, fs::Permissions::from_mode(0o755)).expect("chmod fixture binary");

        let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        {
            let mut builder = tar::Builder::new(&mut encoder);
            builder.append_path_with_name(&binary, "otto").expect("append fixture");
            builder.finish().expect("finish tar");
        }

        encoder.finish().expect("finish gzip")
    }

    fn sha256_hex(bytes: &[u8]) -> String {
        hex::encode(Sha256::digest(bytes))
    }

    fn command() -> UpgradeCommand {
        UpgradeCommand {
            dry_run: false,
            version: None,
            list_versions: false,
            rollback: false,
            force: false,
            no_backup: false,
            backup_dir: None,
            github_token: None,
        }
    }

    fn run_version(path: &Path) -> String {
        let output = Command::new(path)
            .arg("--version")
            .output()
            .expect("run installed binary");
        assert!(output.status.success(), "installed binary failed --version");
        String::from_utf8_lossy(&output.stdout).trim().to_string()
    }

    #[tokio::test]
    async fn download_and_verify_keeps_the_archive_alive_after_the_call() {
        let scratch = tempfile::tempdir().unwrap();
        let tarball = fixture_tarball(scratch.path(), "9.9.9");

        let mut routes = HashMap::new();
        routes.insert("/otto.tar.gz".to_string(), Route::Body(tarball.clone()));
        routes.insert(
            "/otto.tar.gz.sha256".to_string(),
            Route::Body(format!("{}  otto.tar.gz\n", sha256_hex(&tarball)).into_bytes()),
        );
        let base = spawn_stub(routes);

        let client = build_http_client(CONNECT_TIMEOUT, READ_TIMEOUT).unwrap();
        let archive = command()
            .download_and_verify(&client, &format!("{}/otto.tar.gz", base))
            .await
            .expect("download and verify");

        // The bug this phase opens with: the archive used to be deleted the
        // moment the download call returned.
        assert!(archive.path().exists(), "archive must outlive the download call");
        assert_eq!(fs::read(archive.path()).unwrap(), tarball);

        let path = archive.path().to_path_buf();
        drop(archive);
        assert!(!path.exists(), "dropping the handle must clean the archive up");
    }

    #[tokio::test]
    async fn download_and_verify_rejects_a_checksum_mismatch() {
        let scratch = tempfile::tempdir().unwrap();
        let tarball = fixture_tarball(scratch.path(), "9.9.9");

        let mut routes = HashMap::new();
        routes.insert("/otto.tar.gz".to_string(), Route::Body(tarball));
        routes.insert(
            "/otto.tar.gz.sha256".to_string(),
            Route::Body(format!("{}  otto.tar.gz\n", "0".repeat(64)).into_bytes()),
        );
        let base = spawn_stub(routes);

        let client = build_http_client(CONNECT_TIMEOUT, READ_TIMEOUT).unwrap();
        let err = command()
            .download_and_verify(&client, &format!("{}/otto.tar.gz", base))
            .await
            .expect_err("a mismatched checksum must abort");

        assert!(
            err.to_string().contains("Checksum mismatch"),
            "unexpected error: {}",
            err
        );
    }

    #[tokio::test]
    async fn download_and_verify_rejects_a_missing_checksum_sibling() {
        let scratch = tempfile::tempdir().unwrap();
        let tarball = fixture_tarball(scratch.path(), "9.9.9");

        let mut routes = HashMap::new();
        routes.insert("/otto.tar.gz".to_string(), Route::Body(tarball));
        let base = spawn_stub(routes);

        let client = build_http_client(CONNECT_TIMEOUT, READ_TIMEOUT).unwrap();
        let err = command()
            .download_and_verify(&client, &format!("{}/otto.tar.gz", base))
            .await
            .expect_err("a missing checksum must abort");

        assert!(
            err.to_string().contains("Checksum file not available"),
            "unexpected error: {}",
            err
        );
    }

    #[tokio::test]
    async fn download_rejects_a_non_success_status() {
        let base = spawn_stub(HashMap::new());

        let client = build_http_client(CONNECT_TIMEOUT, READ_TIMEOUT).unwrap();
        let err = command()
            .download_with_progress(&client, &format!("{}/otto.tar.gz", base))
            .await
            .expect_err("a 404 must not be treated as an archive");

        assert!(err.to_string().contains("Download failed"), "unexpected error: {}", err);
    }

    #[tokio::test]
    async fn download_times_out_when_the_stream_stalls() {
        let mut routes = HashMap::new();
        routes.insert("/otto.tar.gz".to_string(), Route::Stall);
        let base = spawn_stub(routes);

        let client = build_http_client(CONNECT_TIMEOUT, Duration::from_millis(300)).unwrap();
        let started = SystemTime::now();
        let err = command()
            .download_with_progress(&client, &format!("{}/otto.tar.gz", base))
            .await
            .expect_err("a stalled stream must time out");

        let elapsed = started.elapsed().unwrap();
        assert!(elapsed < Duration::from_secs(5), "timed out too late: {:?}", elapsed);
        assert!(
            err.to_string().contains("Download interrupted"),
            "unexpected error: {}",
            err
        );
    }

    #[tokio::test]
    async fn upgrade_installs_a_fixture_release_end_to_end() {
        let scratch = tempfile::tempdir().unwrap();
        let tarball = fixture_tarball(scratch.path(), "9.9.9");

        let mut routes = HashMap::new();
        routes.insert("/otto.tar.gz".to_string(), Route::Body(tarball.clone()));
        routes.insert(
            "/otto.tar.gz.sha256".to_string(),
            Route::Body(format!("{}  otto.tar.gz\n", sha256_hex(&tarball)).into_bytes()),
        );
        let base = spawn_stub(routes);

        // The install target is a scratch path, never the running executable.
        let target = scratch.path().join("bin").join("otto");
        fs::create_dir_all(target.parent().unwrap()).unwrap();
        fs::write(&target, "#!/bin/sh\necho \"otto 0.0.1\"\n").unwrap();
        fs::set_permissions(&target, fs::Permissions::from_mode(0o755)).unwrap();
        assert_eq!(run_version(&target), "otto 0.0.1");

        let client = build_http_client(CONNECT_TIMEOUT, READ_TIMEOUT).unwrap();
        let cmd = command();
        let archive = cmd
            .download_and_verify(&client, &format!("{}/otto.tar.gz", base))
            .await
            .expect("download and verify");
        cmd.install_from_archive(archive.path(), &target).expect("install");

        assert_eq!(run_version(&target), "otto 9.9.9");
        assert_eq!(
            fs::metadata(&target).unwrap().permissions().mode() & 0o777,
            0o755,
            "installed binary must be executable"
        );
        let leftovers: Vec<_> = fs::read_dir(target.parent().unwrap())
            .unwrap()
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().to_string())
            .filter(|n| n != "otto")
            .collect();
        assert!(leftovers.is_empty(), "staging debris left behind: {:?}", leftovers);
    }

    #[test]
    fn install_from_archive_leaves_the_target_intact_when_the_binary_is_missing() {
        let scratch = tempfile::tempdir().unwrap();

        // An archive with no `otto` entry inside it.
        let stray = scratch.path().join("README");
        fs::write(&stray, "not a binary").unwrap();
        let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        {
            let mut builder = tar::Builder::new(&mut encoder);
            builder.append_path_with_name(&stray, "README").unwrap();
            builder.finish().unwrap();
        }
        let archive_path = scratch.path().join("otto.tar.gz");
        fs::write(&archive_path, encoder.finish().unwrap()).unwrap();

        let target = scratch.path().join("otto");
        fs::write(&target, "#!/bin/sh\necho \"otto 0.0.1\"\n").unwrap();
        fs::set_permissions(&target, fs::Permissions::from_mode(0o755)).unwrap();

        let err = command()
            .install_from_archive(&archive_path, &target)
            .expect_err("an archive without otto must not install");

        assert!(err.to_string().contains("not found in archive"), "unexpected: {}", err);
        assert_eq!(run_version(&target), "otto 0.0.1", "target must be untouched");
    }

    #[test]
    fn staging_leaves_the_target_untouched_until_the_rename() {
        let scratch = tempfile::tempdir().unwrap();

        let target = scratch.path().join("otto");
        fs::write(&target, "#!/bin/sh\necho \"otto 0.0.1\"\n").unwrap();
        fs::set_permissions(&target, fs::Permissions::from_mode(0o755)).unwrap();

        let replacement = scratch.path().join("new-otto");
        fs::write(&replacement, "#!/bin/sh\necho \"otto 9.9.9\"\n").unwrap();

        // Stage only: this is the window an interrupted upgrade dies in.
        let staged = stage_beside(&replacement, &target).expect("stage");
        assert_ne!(staged, target);
        assert_eq!(staged.parent(), target.parent(), "staging must share the target's dir");
        assert_eq!(
            run_version(&target),
            "otto 0.0.1",
            "an interrupted upgrade must leave the original intact and executable"
        );

        // Completing the rename is what makes the swap visible.
        commit_staged(&staged, &target).expect("commit");
        assert_eq!(run_version(&target), "otto 9.9.9");
        assert!(!staged.exists(), "staged file must be consumed by the rename");
    }

    #[test]
    fn rollback_restores_the_previous_binary() {
        let scratch = tempfile::tempdir().unwrap();
        let backup_dir = scratch.path().join("backups");
        fs::create_dir_all(&backup_dir).unwrap();

        for (version, timestamp, reported) in [("0.5.5", 100, "0.5.5"), ("0.5.6", 200, "0.5.6")] {
            let backup = backup_dir.join(format!("otto-{}-{}.backup", version, timestamp));
            fs::write(&backup, format!("#!/bin/sh\necho \"otto {}\"\n", reported)).unwrap();
            fs::set_permissions(&backup, fs::Permissions::from_mode(0o755)).unwrap();
        }

        let target = scratch.path().join("otto");
        fs::write(&target, "#!/bin/sh\necho \"otto 9.9.9\"\n").unwrap();
        fs::set_permissions(&target, fs::Permissions::from_mode(0o755)).unwrap();

        let mut cmd = command();
        cmd.backup_dir = Some(backup_dir);

        let backups = cmd.list_backups().expect("list backups");
        assert_eq!(backups[0].version, "0.5.6", "newest backup must come first");

        cmd.verify_binary(&backups[0].path).expect("verify backup");
        install_binary(&backups[0].path, &target).expect("restore");

        assert_eq!(run_version(&target), "otto 0.5.6");
    }

    #[test]
    fn parse_sha256_manifest_reads_the_sha256sum_format() {
        let digest = "a".repeat(64);
        let parsed = parse_sha256_manifest(&format!("{}  otto-v1.0.0-linux.tar.gz\n", digest.to_uppercase()));
        assert_eq!(parsed.unwrap(), digest);
    }

    #[test]
    fn parse_sha256_manifest_rejects_a_non_digest() {
        for body in ["", "not-a-digest  otto.tar.gz", &"a".repeat(63)] {
            assert!(
                parse_sha256_manifest(body).is_err(),
                "expected rejection for {:?}",
                body
            );
        }
    }

    #[test]
    fn sha256_file_matches_a_known_digest() {
        let scratch = tempfile::tempdir().unwrap();
        let path = scratch.path().join("payload");
        fs::write(&path, b"abc").unwrap();

        assert_eq!(
            sha256_file(&path).unwrap(),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[tokio::test]
    async fn fetch_releases_rejects_a_non_success_status() {
        let base = spawn_stub(HashMap::new());

        let client = build_http_client(CONNECT_TIMEOUT, READ_TIMEOUT).unwrap();
        let err = command()
            .fetch_releases(&client, &format!("{}/releases", base))
            .await
            .expect_err("a 404 release listing must be an error");

        assert!(err.to_string().contains("404"), "unexpected error: {}", err);
    }

    #[tokio::test]
    async fn fetch_releases_parses_a_release_listing() {
        let body = r#"[{"tag_name":"v9.9.9","name":"v9.9.9","published_at":"2026-01-02T03:04:05Z",
            "assets":[{"name":"otto-v9.9.9-linux-amd64.tar.gz","browser_download_url":"http://example.invalid/a.tar.gz"}]}]"#;

        let mut routes = HashMap::new();
        routes.insert("/releases".to_string(), Route::Body(body.as_bytes().to_vec()));
        let base = spawn_stub(routes);

        let client = build_http_client(CONNECT_TIMEOUT, READ_TIMEOUT).unwrap();
        let cmd = command();
        let releases = cmd
            .fetch_releases(&client, &format!("{}/releases", base))
            .await
            .expect("parse releases");

        assert_eq!(releases[0].tag_name, "v9.9.9");
        let asset = cmd.find_asset(&releases[0], "linux-amd64").expect("find asset");
        assert_eq!(asset.name, "otto-v9.9.9-linux-amd64.tar.gz");
    }

    #[test]
    fn platform_strings_match_the_published_asset_suffixes() {
        // install.sh get_suffix() and .github/workflows/release-and-publish.yml
        // are the source of truth; a drift here makes Upgrade look for a
        // tarball no release contains.
        let install_sh = fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/install.sh")).unwrap();

        for suffix in ["linux-amd64", "linux-arm64", "macos-x86_64", "macos-arm64"] {
            assert!(
                install_sh.contains(&format!("echo \"{}\"", suffix)),
                "install.sh no longer publishes {}",
                suffix
            );
        }

        let detected = PlatformInfo::detect().unwrap().platform_str;
        assert!(
            install_sh.contains(&format!("echo \"{}\"", detected)),
            "detected platform {:?} is not a published asset suffix",
            detected
        );
    }

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
