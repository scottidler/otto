use chrono::DateTime;
use eyre::{Context, Result, eyre};
use flate2::read::GzDecoder;
use indicatif::{ProgressBar, ProgressStyle};
use log::debug;
use reqwest::Client;
use semver::Version;
use serde::{Deserialize, de::DeserializeOwned};
use sha2::{Digest, Sha256};
use std::cmp::Ordering;
use std::env;
use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};
use tar::Archive;
use tempfile::TempDir;

/// GitHub API root for otto's own repository.
///
/// Every endpoint this command reads derives from this one string; the three
/// that do are `releases_url`, `latest_release_url` and `tagged_release_url`.
const REPO_API_URL: &str = "https://api.github.com/repos/otto-rs/otto";

/// User-Agent GitHub requires on API requests.
const USER_AGENT: &str = "otto-upgrade";

/// Time allowed to establish a connection to the release host.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(30);

/// Time allowed between response bytes. A connection that stops delivering data
/// fails here instead of hanging the upgrade forever.
const READ_TIMEOUT: Duration = Duration::from_secs(60);

/// Whole-request budget for the small metadata requests (release JSON, checksum
/// sibling).
const METADATA_TIMEOUT: Duration = Duration::from_secs(60);

/// Whole-transfer budget for the release archive.
///
/// The between-bytes `READ_TIMEOUT` alone does not bound anything: a server
/// dribbling one byte every 59 seconds resets it forever, and an `otto Upgrade`
/// against one runs until the user kills it. Measured at over 150 seconds
/// against a 1-byte-per-20s stream before this existed. The cap is deliberately
/// generous - a release archive is a few megabytes, so ten minutes is far
/// outside any honest slow link, and the point is to terminate rather than to
/// be tight.
const DOWNLOAD_BUDGET: Duration = Duration::from_secs(600);

/// ETXTBSY: exec of a file that is still open for writing anywhere on the
/// system. It is transient by definition - a `fork` in another thread that
/// momentarily inherits the write descriptor is enough to produce it - so
/// verifying a freshly written binary retries rather than declaring it broken.
const ETXTBSY: i32 = 26;

/// Spacing and total budget for that retry.
///
/// A wall-clock budget rather than an attempt count: what matters is how long a
/// user waits before otto gives up, and an attempt count says nothing about that
/// (10 attempts at 50ms was a 500ms ceiling nobody had measured against the
/// thing it is racing). The holder is another process momentarily inheriting a
/// write descriptor, so the wait is bounded by that process's scheduling, not by
/// a number of tries.
const VERIFY_RETRY_DELAY: Duration = Duration::from_millis(50);
const VERIFY_BUSY_BUDGET: Duration = Duration::from_secs(2);

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

/// The published name of a release asset.
///
/// One function for both readers. `find_asset` looks for `otto-v{version}-...`,
/// which is what the release workflow publishes, while the `--dry-run` plan
/// printed `otto-{version}-...`: the plan named a file no release contains.
fn asset_name(version: &str, platform: &str) -> String {
    format!("otto-v{}-{}.tar.gz", version.trim_start_matches('v'), platform)
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
    stage_beside_with(source, target, make_executable)
}

/// `stage_beside` with its "make the copy executable" step injected, so a test
/// can prove the staged file is removed when that step fails.
///
/// Production has one caller and it always passes [`make_executable`]. A chmod
/// that fails on a file this process has just created cannot be arranged from a
/// test any other way, and the failures it stands for are real: a full or
/// read-only filesystem, an immutable attribute, a mandatory access control
/// policy.
fn stage_beside_with(
    source: &Path,
    target: &Path,
    set_executable: impl FnOnce(&Path) -> Result<()>,
) -> Result<PathBuf> {
    debug!("stage_beside: source={} target={}", source.display(), target.display());
    let dir = target
        .parent()
        .ok_or_else(|| eyre!("Install target has no parent directory: {}", target.display()))?;
    let name = target
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or_else(|| eyre!("Install target has no file name: {}", target.display()))?;

    reap_stale_staging(dir, name);

    let staged = dir.join(format!(".{}.upgrade-{}", name, std::process::id()));
    fs::copy(source, &staged)
        .with_context(|| format!("Failed to stage {} at {}", source.display(), staged.display()))?;

    // The copy has already succeeded here, so a bare `?` on the step below
    // stranded `.<name>.upgrade-<pid>` beside the binary until some later
    // upgrade's reaper found it. This staging attempt is over either way, so it
    // takes its own file with it.
    if let Err(err) = set_executable(&staged) {
        let _ = fs::remove_file(&staged);
        return Err(err);
    }

    Ok(staged)
}

/// Give a staged copy the executable mode a released binary needs.
#[cfg(unix)]
fn make_executable(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let mut perms = fs::metadata(path)
        .with_context(|| format!("Failed to read permissions of {}", path.display()))?
        .permissions();
    perms.set_mode(0o755);
    fs::set_permissions(path, perms).with_context(|| format!("Failed to set permissions on {}", path.display()))
}

#[cfg(not(unix))]
fn make_executable(_path: &Path) -> Result<()> {
    Ok(())
}

/// Remove staging files left behind by an upgrade that died between the copy and
/// the rename.
///
/// `commit_staged` cleans up when it returns an error, which covers every failure
/// the process survives. It cannot cover a signal: SIGKILL between the two leaves
/// `.<name>.upgrade-<pid>` beside the binary forever, and nothing ever reaped it.
/// Measured over 10 SIGKILL trials landing inside the copy window: the original
/// binary was intact 9 of 9 times, with a staging file stranded each time.
///
/// Only files whose pid is no longer running are removed, so a concurrent upgrade
/// cannot delete the file the other process is still writing. A failure to reap
/// is not a failure to upgrade, so it warns and continues.
fn reap_stale_staging(dir: &Path, name: &str) {
    let prefix = format!(".{name}.upgrade-");
    let Ok(entries) = fs::read_dir(dir) else { return };

    for entry in entries.flatten() {
        let file_name = entry.file_name();
        let Some(file_name) = file_name.to_str() else { continue };
        let Some(pid) = file_name.strip_prefix(&prefix) else {
            continue;
        };
        let Ok(pid) = pid.parse::<u32>() else { continue };
        if pid == std::process::id() || pid_is_running(pid) {
            continue;
        }
        match fs::remove_file(entry.path()) {
            Ok(()) => debug!("reap_stale_staging: removed {}", entry.path().display()),
            Err(e) => log::warn!("Could not remove stale staging file {}: {e}", entry.path().display()),
        }
    }
}

/// Whether a pid is still live, used only to decide if a staging file is
/// abandoned.
///
/// `kill(pid, 0)` sends no signal and only asks the kernel whether the pid
/// exists: `EPERM` means it exists and belongs to somebody else, so only
/// `ESRCH` means gone. This used to read `/proc/<pid>` on Linux and return
/// `true` unconditionally everywhere else, which meant macOS never reaped
/// anything: `otto Upgrade` accumulated a `.otto.upgrade-<pid>` file next to
/// the binary for every upgrade a signal ever interrupted, forever.
fn pid_is_running(pid: u32) -> bool {
    // Only a strictly positive pid names a process. `kill` reads 0 as "my own
    // process group" and a negative number as "that process group", so a
    // filename carrying a pid that does not fit a positive `pid_t` must never
    // reach the syscall: casting 4294967290 lands on -6 and asks about process
    // group 6, which is a different question with a different answer.
    let Ok(pid) = libc::pid_t::try_from(pid) else {
        return false;
    };
    if pid <= 0 {
        return false;
    }
    // SAFETY: `kill` with signal 0 performs the existence and permission check
    // and delivers nothing, so there is no process state to corrupt.
    if unsafe { libc::kill(pid, 0) } == 0 {
        return true;
    }
    // Anything other than "no such process" leaves the file alone: a live pid
    // owned by another user reports EPERM, and guessing wrong here deletes a
    // file an upgrade is using.
    std::io::Error::last_os_error().raw_os_error() != Some(libc::ESRCH)
}

/// Rename a staged binary over the target, removing the staged file if the
/// rename fails.
///
/// This covers every failure the process survives. A signal between the copy and
/// the rename is covered by `reap_stale_staging` on the next upgrade instead.
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
#[derive(Debug, Default, clap::Parser)]
#[command(name = "Upgrade", bin_name = "otto Upgrade")]
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
    //
    // `hide_env_values(true)` is not optional here. clap renders an env-backed
    // arg as `[env: VAR=<value>]` in `--help`, so without it `otto Upgrade
    // --help` printed the user's live token to stdout - into terminal
    // scrollback, CI logs, and any transcript of a help invocation.
    #[arg(long, env = "GITHUB_TOKEN", hide_env_values = true)]
    pub github_token: Option<String>,

    /// Which API to ask about releases. `None` means [`REPO_API_URL`].
    ///
    /// `#[arg(skip)]` and no `env =` on purpose, and it must stay that way: a
    /// user-settable release API turns `otto Upgrade` into "download and
    /// execute a binary from wherever this string points". The only writer is
    /// [`UpgradeCommand::with_fixture`], which is `#[cfg(test)]`.
    #[arg(skip)]
    api_base: Option<String>,

    /// Which file to replace. `None` means `env::current_exe()`.
    ///
    /// Same rule as above, plus a practical one: a test that drove the real
    /// `execute_upgrade` without this would overwrite the test binary that is
    /// running it.
    #[arg(skip)]
    install_target: Option<PathBuf>,

    /// What version this invocation is running. `None` means `GIT_DESCRIBE`.
    ///
    /// Same `#[arg(skip)]` rule as above. Rollback reads this to refuse a
    /// backup that is not older than the version running now, and a process's
    /// own version is fixed at build time, so a test that needs two
    /// invocations at two versions - roll back twice - has no other way to say
    /// so. The only writer is [`UpgradeCommand::with_current_version`], which
    /// is `#[cfg(test)]`.
    #[arg(skip)]
    current_version: Option<String>,
}

#[derive(Debug)]
struct PlatformInfo {
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
            platform_str: platform_str.to_string(),
        })
    }
}

#[derive(Debug, Deserialize)]
struct GitHubRelease {
    tag_name: String,
    published_at: String,
    assets: Vec<GitHubAsset>,
}

#[derive(Debug, Deserialize)]
struct GitHubAsset {
    name: String,
    browser_download_url: String,
}

#[derive(Debug)]
struct BackupInfo {
    path: PathBuf,
    version: String,
    timestamp: u64,
}

/// A version that can be ordered even when it came from `git describe`.
///
/// `git describe --tags --always` (what `build.rs` records as `GIT_DESCRIBE`)
/// prints `v2.2.1` on a tag and `v2.2.1-27-g85bb8fb` twenty-seven commits past
/// it. Semver reads that suffix as a PRERELEASE of 2.2.1, which sorts BELOW the
/// release, so every dev build compared as older than the tag it descends from:
/// `otto Upgrade` on one planned a downgrade to that release and called it an
/// upgrade, and `--rollback` treated a newer dev build as a rollback candidate.
/// A build past a tag is newer than the tag, so the commit count breaks the tie
/// upward instead. A genuine prerelease tag (`v2.3.0-rc.1`) has no `-<n>-g<sha>`
/// suffix and keeps semver's own ordering.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct BuildVersion {
    release: Version,
    commits: u64,
}

/// `2.2.1-27-g85bb8fb` -> `("2.2.1", 27)`. `None` for anything that is not a
/// `git describe` suffix, including a real prerelease.
fn split_describe(text: &str) -> Option<(&str, u64)> {
    let (rest, sha) = text.rsplit_once('-')?;
    let hex = sha.strip_prefix('g')?;
    if hex.is_empty() || !hex.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    let (core, count) = rest.rsplit_once('-')?;
    Some((core, count.parse::<u64>().ok()?))
}

/// Every shape `git describe --tags --always [--dirty]` can print, and nothing
/// else:
///
/// | printed                     | ordered as              |
/// |-----------------------------|-------------------------|
/// | `v2.2.1`                    | 2.2.1, 0 commits        |
/// | `v2.2.1-dirty`              | 2.2.1, 0 commits        |
/// | `v2.2.1-27-g85bb8fb`        | 2.2.1, 27 commits       |
/// | `v2.2.1-27-g85bb8fb-dirty`  | 2.2.1, 27 commits       |
/// | `85bb8fb` / `85bb8fb-dirty` | refused, with a reason  |
///
/// `-dirty` is stripped before anything else and then ignored. It says the
/// working tree had uncommitted changes at build time, which is not a position
/// in the version order: the build still descends from exactly the commits its
/// `-<n>-g<sha>` names.
///
/// `build.rs` does not pass `--dirty` today. Handled anyway because the failure
/// mode if someone adds it is silent: the suffix match would fail, semver would
/// read the whole string as a PRERELEASE of the tag, and a dev build would sort
/// BELOW the release again, which is the exact inversion this function exists
/// to prevent. A comment in `build.rs` is a request; this is a guarantee.
fn parse_build_version(version: &str) -> Result<BuildVersion> {
    let text = version.trim_start_matches('v');
    let text = text.strip_suffix("-dirty").unwrap_or(text);
    let (core, commits) = split_describe(text).unwrap_or((text, 0));
    let release = Version::parse(core).map_err(|err| {
        eyre!(
            "Cannot order the version 'v{version}': {err}. `git describe --tags --always` prints a bare commit sha when no tag is reachable, and a sha cannot be compared with a release."
        )
    })?;
    Ok(BuildVersion { release, commits })
}

/// The backup a rollback should restore: the most recent one whose version is
/// older than the version running now.
///
/// "The most recent backup" alone was wrong. A rollback writes a safety backup
/// of the binary it replaces, so that file is the newest entry in the directory
/// when the next rollback runs, and the next rollback restored it: the two
/// commands undid each other and no run of rollbacks ever reached the version
/// before last. Rolling back is a move backwards, so only an older version is a
/// candidate, and repeating it walks the versions down.
///
/// A backup whose version is not a semver is skipped rather than guessed at:
/// the name comes from a directory anything can write into, and a version that
/// cannot be ordered cannot be shown to be older than this one.
fn select_rollback_target<'a>(backups: &'a [BackupInfo], current_version: &str) -> Result<&'a BackupInfo> {
    let current = parse_build_version(current_version)
        .with_context(|| format!("Cannot order backups against the current version v{current_version}"))?;

    // `list_backups` returns newest first, so the first match is the newest.
    backups
        .iter()
        .find(|backup| match parse_build_version(&backup.version) {
            Ok(version) => version < current,
            Err(err) => {
                debug!(
                    "select_rollback_target: skipping {}, version {:?} is not a semver: {err}",
                    backup.path.display(),
                    backup.version
                );
                false
            }
        })
        .ok_or_else(|| {
            let present: Vec<String> = backups.iter().map(|b| format!("v{}", b.version)).collect();
            eyre!(
                "No backup older than the current v{} to roll back to (present: {})",
                current_version,
                present.join(", ")
            )
        })
}

impl UpgradeCommand {
    /// The release API this invocation should ask.
    fn api_base(&self) -> &str {
        self.api_base.as_deref().unwrap_or(REPO_API_URL)
    }

    /// Every release, newest-created first, including prereleases. Only
    /// `--list-versions` wants this.
    fn releases_url(&self) -> String {
        format!("{}/releases", self.api_base())
    }

    /// The one release GitHub itself marks latest, which is what `install.sh`
    /// reads. Page 1 of the listing is creation order and includes prereleases,
    /// so its first entry answers a different question.
    fn latest_release_url(&self) -> String {
        format!("{}/releases/latest", self.api_base())
    }

    /// One release by tag. Reaches a release that is too old to be on page 1 of
    /// the listing, which `--version` used to report as "not found".
    fn tagged_release_url(&self, version: &str) -> String {
        format!("{}/releases/tags/v{}", self.api_base(), version.trim_start_matches('v'))
    }

    /// The binary this invocation should replace.
    fn install_target(&self) -> Result<PathBuf> {
        match &self.install_target {
            Some(path) => Ok(path.clone()),
            None => Ok(env::current_exe()?),
        }
    }

    /// Skip the backup step, so a fixture test asserts on the install alone.
    #[cfg(test)]
    fn tap_no_backup(mut self) -> Self {
        self.no_backup = true;
        self
    }

    /// Point this command at a fixture release API and a scratch binary.
    ///
    /// Test-only, so that `execute()` itself can be driven end to end. Without
    /// it the only reachable path was to hand-compose
    /// `download_and_check_checksum` plus `install_from_archive` in the test,
    /// which skipped `current_version`, the already-on-target and downgrade
    /// short-circuits, `find_asset` and `create_backup` - the test asserted its
    /// own transcription of the command body rather than the command.
    #[cfg(test)]
    fn with_fixture(mut self, api_base: impl Into<String>, install_target: impl Into<PathBuf>) -> Self {
        self.api_base = Some(api_base.into());
        self.install_target = Some(install_target.into());
        self
    }

    /// Pretend this invocation is running `version`.
    ///
    /// Test-only. See the field's doc: rolling back twice is two processes at
    /// two versions, and `GIT_DESCRIBE` is one value for the life of a process.
    #[cfg(test)]
    fn with_current_version(mut self, version: impl Into<String>) -> Self {
        self.current_version = Some(version.into());
        self
    }

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

        // 3. Resolve the one release this run is about (one client, reused for
        //    metadata and the download). `--version X` asks for that tag by
        //    name, which reaches a release too old to be on page 1 of the
        //    listing; the default asks GitHub which release is latest rather
        //    than reading the newest-created entry off that page, whose answer
        //    includes prereleases.
        let client = build_http_client(CONNECT_TIMEOUT, READ_TIMEOUT)?;
        let release = match &self.version {
            Some(version) => {
                let version = version.trim_start_matches('v');
                self.fetch_release(&client, &self.tagged_release_url(version))
                    .await
                    .with_context(|| format!("Release v{} not found", version))?
            }
            None => self
                .fetch_release(&client, &self.latest_release_url())
                .await
                .context("Could not determine the latest release")?,
        };

        // 4. Determine target version
        let target_version = release.tag_name.trim_start_matches('v').to_string();

        println!("Target version:  v{}", target_version);

        // 5. Check if upgrade needed
        if !self.force {
            let current = parse_build_version(&current_version)?;
            let target = parse_build_version(&target_version)?;

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

        // 6. Download the asset this platform needs from that release
        let asset = self.find_asset(&release, &platform.platform_str)?;

        println!("\nDownloading {}...", asset.name);
        let archive = self
            .download_and_check_checksum(&client, &asset.browser_download_url)
            .await?;

        println!("Download complete!");

        // 7. Create backup
        let current_exe = self.install_target()?;
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

        let current_version = self.current_version()?;
        let restore = select_rollback_target(&backups, &current_version)?;
        println!("Rolling back to v{}...", restore.version);

        if self.dry_run {
            println!("\nWould restore: {}", restore.path.display());
            return Ok(());
        }

        // Back up what is about to be replaced - once. Writing a fresh safety
        // backup on every rollback is what made the directory's newest entry
        // the version the previous rollback had just left.
        let current_exe = self.install_target()?;
        if !self.no_backup {
            if backups.iter().any(|b| b.version == current_version) {
                println!("Safety backup skipped: v{} is already backed up.", current_version);
            } else {
                let backup_path = self.create_backup(&current_exe)?;
                println!("Safety backup created: {}", backup_path.display());
            }
        }

        // Confirm the backup runs before it replaces anything
        self.verify_binary(&restore.path)?;

        // Restore from backup: stage beside the target, then rename
        install_binary(&restore.path, &current_exe).context("Failed to restore backup")?;

        println!("✓ Successfully rolled back to v{}!", restore.version);

        Ok(())
    }

    async fn execute_list_versions(&self) -> Result<()> {
        println!("Fetching available versions...");

        let client = build_http_client(CONNECT_TIMEOUT, READ_TIMEOUT)?;
        let releases = self.fetch_releases(&client, &self.releases_url()).await?;

        // The listing is creation order and includes prereleases, so its first
        // entry is not the latest release: ask GitHub which one is, the same
        // way the upgrade path does. A listing is still worth printing when
        // that lookup fails, so a failure drops the marker and warns.
        let latest_tag = match self.fetch_release(&client, &self.latest_release_url()).await {
            Ok(release) => Some(release.tag_name),
            Err(err) => {
                log::warn!("Could not determine the latest release; listing without a marker: {err}");
                None
            }
        };

        println!("\nAvailable versions:");
        for line in self.version_lines(&releases, latest_tag.as_deref()) {
            println!("{}", line);
        }

        let current = self.current_version()?;
        println!("\nCurrent version: v{}", current);

        Ok(())
    }

    /// The steps `--dry-run` says it would take, in order.
    ///
    /// Returned rather than printed so a test can read them: the numbering used
    /// to skip "2." under `--no-backup`, and step 1 named an asset spelling no
    /// release publishes.
    fn dry_run_steps(&self, target_version: &str, platform: &PlatformInfo) -> Result<Vec<String>> {
        let mut steps = vec![format!(
            "Download {}",
            asset_name(target_version, &platform.platform_str)
        )];

        // Checksum before backup, because that is the order `execute_upgrade`
        // runs them in: `download_and_check_checksum` returns before
        // `create_backup` is called. Listed the other way round, the plan told
        // a reader a backup would exist after a checksum failure, and none
        // does; nothing is backed up until the download has been verified.
        steps.push("Check the download against the release's published .sha256 checksum".to_string());

        if !self.no_backup {
            let backup_dir = self.get_backup_dir()?;
            steps.push(format!(
                "Create backup: {}/otto-<current>-<timestamp>.backup",
                backup_dir.display()
            ));
        }

        steps.push("Extract the new binary from the archive".to_string());
        steps.push("Run the new binary's --version to confirm it works".to_string());
        steps.push(format!("Replace {}", self.install_target()?.display()));

        Ok(steps)
    }

    fn show_dry_run_plan(&self, target_version: &str, platform: &PlatformInfo) -> Result<()> {
        println!("\nDry run - would perform the following actions:");
        for (i, step) in self.dry_run_steps(target_version, platform)?.iter().enumerate() {
            println!("  {}. {}", i + 1, step);
        }
        println!("\nRun without --dry-run to perform upgrade.");
        Ok(())
    }

    /// The lines `--list-versions` prints, one per release, newest-created
    /// first. Returned rather than printed so the "(latest)" marker is testable.
    fn version_lines(&self, releases: &[GitHubRelease], latest_tag: Option<&str>) -> Vec<String> {
        releases
            .iter()
            .map(|release| {
                let marker = if latest_tag == Some(release.tag_name.as_str()) { " (latest)" } else { "" };
                format!(
                    "  v{}{:10} - {}",
                    release.tag_name.trim_start_matches('v'),
                    marker,
                    self.format_date(&release.published_at)
                )
            })
            .collect()
    }

    fn current_version(&self) -> Result<String> {
        // Use GIT_DESCRIBE which is set at build time in build.rs
        // This matches what --version displays
        let version = self.current_version.as_deref().unwrap_or(env!("GIT_DESCRIBE"));
        Ok(version.trim_start_matches('v').to_string())
    }

    /// GET one JSON document from the release API, with the token when there is
    /// one. The status check lives here so every endpoint reports a 404 the same
    /// way instead of parsing GitHub's error body as a release.
    async fn get_json<T: DeserializeOwned>(&self, client: &Client, url: &str) -> Result<T> {
        debug!("get_json: url={}", url);
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

        response.json().await.context("Failed to parse GitHub releases")
    }

    /// One release, from either single-release endpoint: see
    /// `latest_release_url` and `tagged_release_url`.
    async fn fetch_release(&self, client: &Client, url: &str) -> Result<GitHubRelease> {
        self.get_json(client, url).await
    }

    /// The whole listing, which only `--list-versions` needs.
    async fn fetch_releases(&self, client: &Client, url: &str) -> Result<Vec<GitHubRelease>> {
        self.get_json(client, url).await
    }

    fn find_asset<'a>(&self, release: &'a GitHubRelease, platform: &str) -> Result<&'a GitHubAsset> {
        let pattern = asset_name(&release.tag_name, platform);

        release
            .assets
            .iter()
            .find(|asset| asset.name == pattern)
            .ok_or_else(|| eyre!("No asset found for platform: {} (looking for {})", platform, pattern))
    }

    /// Download a release archive and check it against the `.sha256` file
    /// published beside it.
    ///
    /// A checksum match, not a signature check: it proves the bytes on disk are
    /// the bytes named by the release's own `.sha256`, which catches a truncated
    /// or corrupted download, and says nothing about who produced either file.
    /// Signing is a non-goal of this command, so the name and the messages say
    /// "checksum" and never "verified".
    ///
    /// The returned `DownloadedArchive` owns the temporary directory, so the
    /// archive stays on disk for as long as the caller holds it.
    async fn download_and_check_checksum(&self, client: &Client, url: &str) -> Result<DownloadedArchive> {
        debug!("download_and_check_checksum: url={}", url);
        let archive = self.download_with_progress(client, url).await?;

        let checksum_url = format!("{}.sha256", url);
        println!("Checking the download against the release's published .sha256 checksum...");
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
        self.download_with_progress_within(client, url, DOWNLOAD_BUDGET).await
    }

    /// `download_with_progress` with the whole-transfer budget injected, so a
    /// test can prove the cap is enforced without waiting ten minutes.
    async fn download_with_progress_within(
        &self,
        client: &Client,
        url: &str,
        budget: Duration,
    ) -> Result<DownloadedArchive> {
        debug!("download_with_progress_within: url={} budget={:?}", url, budget);
        let started = std::time::Instant::now();
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
            let elapsed = started.elapsed();
            if elapsed > budget {
                pb.abandon_with_message("Download exceeded its time budget");
                log::warn!(
                    "download_with_progress_within: budget {:?} exceeded after {:?}, {} bytes",
                    budget,
                    elapsed,
                    downloaded
                );
                return Err(eyre!(
                    "Download of {url} exceeded its {budget:?} budget after {elapsed:?} \
                     ({downloaded} of {total_size} bytes). Nothing was installed; the current \
                     otto is untouched. The connection was delivering data, just far too slowly \
                     to finish."
                ));
            }
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

        // Close the archive before exec'ing what came out of it. `archive` still
        // owns the decoder and the underlying `File` at this point; dropping it
        // removes otto's own descriptors from the set of things that could be
        // holding a write handle when the verify below execs.
        drop(archive);

        // Verify the new binary works
        self.verify_binary(&extracted_binary)?;

        // Stage beside the target and rename: the rename is the atomic step
        install_binary(&extracted_binary, target)
    }

    fn verify_binary(&self, binary_path: &Path) -> Result<()> {
        self.verify_binary_within(binary_path, VERIFY_BUSY_BUDGET)
    }

    /// `verify_binary` with the ETXTBSY budget injected, so a test can prove the
    /// budget is enforced without waiting the production two seconds.
    fn verify_binary_within(&self, binary_path: &Path, busy_budget: Duration) -> Result<()> {
        debug!(
            "verify_binary_within: path={} busy_budget={:?}",
            binary_path.display(),
            busy_budget
        );
        use std::process::Command;

        let started = std::time::Instant::now();
        let mut attempts = 0u32;

        loop {
            attempts += 1;

            match Command::new(binary_path).arg("--version").output() {
                Ok(output) => {
                    if !output.status.success() {
                        return Err(eyre!("New binary failed to run --version"));
                    }
                    debug!("verify_binary_within: ok after {} attempt(s)", attempts);
                    return Ok(());
                }
                Err(err) => {
                    let busy = cfg!(unix) && err.raw_os_error() == Some(ETXTBSY);
                    if !busy {
                        return Err(err).context("Failed to execute new binary");
                    }
                    let elapsed = started.elapsed();
                    if elapsed + VERIFY_RETRY_DELAY <= busy_budget {
                        debug!(
                            "verify_binary_within: ETXTBSY after {:?} (attempt {}), retrying",
                            elapsed, attempts
                        );
                        std::thread::sleep(VERIFY_RETRY_DELAY);
                        continue;
                    }
                    // Budget spent. Say what ETXTBSY actually means, because
                    // "Text file busy" tells a user nothing about what to do.
                    log::warn!(
                        "verify_binary_within: gave up after {:?} and {} attempts, still ETXTBSY",
                        elapsed,
                        attempts
                    );
                    return Err(err).context(format!(
                        "Failed to execute the new binary at {}: still held open for writing by \
                         another process after {:?} ({} attempts). Nothing was installed; the \
                         current otto is untouched. Retry the upgrade, and if it persists check \
                         for a file scanner or backup agent watching that directory.",
                        binary_path.display(),
                        elapsed,
                        attempts
                    ));
                }
            }
        }
    }

    fn get_backup_dir(&self) -> Result<PathBuf> {
        if let Some(ref dir) = self.backup_dir {
            return Ok(crate::executor::layout::expand_tilde(dir));
        }

        // `$OTTO_HOME` moves every other piece of otto's state - run
        // directories, the database - and backups were the one directory that
        // ignored it and rebuilt `$HOME/.otto` itself.
        Ok(crate::executor::layout::resolve_otto_home()?.join("backups"))
    }

    fn format_date(&self, date_str: &str) -> String {
        if let Ok(dt) = DateTime::parse_from_rfc3339(date_str) {
            dt.format("%Y-%m-%d").to_string()
        } else {
            date_str.to_string()
        }
    }
}

#[path = "upgrade_tests.rs"]
mod tests;
