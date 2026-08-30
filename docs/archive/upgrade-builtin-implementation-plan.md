# Upgrade Built-in Implementation Plan

## Executive Summary

This document outlines the implementation plan for adding an `Upgrade` built-in command to Otto. The goal is to provide users with a seamless, cross-platform way to upgrade their Otto installation without requiring external tools or manual scripts.

## Problem Statement

Currently, upgrading Otto requires:
- Manual download of releases from GitHub
- Platform-specific scripts (as shown in Patrick Shelby's `.zshrc`)
- External dependencies (like `gh` CLI)
- Knowledge of installation paths and platform names

This creates friction for users and reduces Otto's self-sufficiency.

## Solution Overview

Implement `otto Upgrade` as a first-class built-in command that:
1. Detects the current platform and version
2. Fetches available releases from GitHub
3. Downloads the appropriate binary
4. Verifies integrity (checksum)
5. Backs up the current installation
6. Installs the new version atomically
7. Provides rollback capability

## Implementation Strategy

### 1. Command Structure

Follow the established pattern for Otto built-ins:

```
src/cli/commands/upgrade.rs    # Main implementation
src/cli/builtins.rs             # Register "Upgrade"
src/cli/parser.rs               # inject_upgrade_meta_task()
src/main.rs                     # Early routing & execute_upgrade_command()
docs/commands/upgrade.md        # User documentation
tests/upgrade_test.rs           # Integration tests
```

### 2. Core Components

#### A. Platform Detection Module

```rust
pub struct PlatformInfo {
    pub os: String,           // "linux" or "macos"
    pub arch: String,         // "x86_64" or "arm64"
    pub platform_str: String, // "linux-x86_64", "macos-arm64", etc.
}

impl PlatformInfo {
    pub fn detect() -> Result<Self> {
        // Use std::env::consts::OS and std::env::consts::ARCH
    }
}
```

#### B. Version Manager

```rust
pub struct VersionManager {
    current_version: String,
}

impl VersionManager {
    pub fn current() -> Result<String> {
        // Parse output from `otto --version`
    }

    pub fn compare(v1: &str, v2: &str) -> Ordering {
        // Semantic version comparison
    }
}
```

#### C. Release Fetcher

```rust
pub struct ReleaseFetcher {
    github_token: Option<String>,
    base_url: String,
}

#[derive(Deserialize)]
pub struct Release {
    pub tag_name: String,
    pub name: String,
    pub published_at: String,
    pub assets: Vec<Asset>,
}

impl ReleaseFetcher {
    pub async fn fetch_releases(&self) -> Result<Vec<Release>> {
        // GET https://api.github.com/repos/otto-rs/otto/releases
    }

    pub async fn find_asset(&self, release: &Release, platform: &str) -> Result<Asset> {
        // Find matching asset for platform
    }
}
```

#### D. Download Manager

```rust
pub struct DownloadManager {
    show_progress: bool,
}

impl DownloadManager {
    pub async fn download(&self, url: &str, dest: &Path) -> Result<PathBuf> {
        // Download with progress bar
    }

    pub async fn verify_checksum(&self, file: &Path, expected: &str) -> Result<bool> {
        // SHA256 verification
    }
}
```

#### E. Backup Manager

```rust
pub struct BackupManager {
    backup_dir: PathBuf,
}

impl BackupManager {
    pub fn create_backup(&self, exe_path: &Path) -> Result<PathBuf> {
        // Copy current binary to backup location
        // Update "latest" symlink
    }

    pub fn list_backups(&self) -> Result<Vec<Backup>> {
        // List available backups
    }

    pub fn restore_backup(&self, backup_path: &Path) -> Result<()> {
        // Restore from backup
    }
}
```

#### F. Installer

```rust
pub struct Installer {
    install_dir: PathBuf,
}

impl Installer {
    pub fn install(&self, archive: &Path, version: &str) -> Result<()> {
        // 1. Extract tarball
        // 2. Verify binary
        // 3. Rename to versioned name (otto-0.5.6)
        // 4. Update symlink (unix) or replace (windows)
    }

    fn verify_binary(&self, binary: &Path) -> Result<()> {
        // Execute --version to verify it works
    }
}
```

### 3. Command Implementation

```rust
#[derive(Debug, clap::Parser)]
#[command(name = "Upgrade")]
pub struct UpgradeCommand {
    /// Show what would be done without doing it
    #[arg(long)]
    pub dry_run: bool,

    /// Specific version to upgrade to
    #[arg(long)]
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

    /// Directory for backups
    #[arg(long, default_value = "~/.otto/backups")]
    pub backup_dir: PathBuf,

    /// GitHub token for API access
    #[arg(long, env = "GITHUB_TOKEN")]
    pub github_token: Option<String>,
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
        // 1. Detect platform
        let platform = PlatformInfo::detect()?;

        // 2. Get current version
        let current_version = VersionManager::current()?;
        println!("Current version: {}", current_version);

        // 3. Fetch releases
        let fetcher = ReleaseFetcher::new(self.github_token.clone());
        let releases = fetcher.fetch_releases().await?;

        // 4. Determine target version
        let target_version = self.determine_target_version(&releases)?;
        println!("Target version:  {}", target_version);

        // 5. Check if upgrade needed
        if !self.force && target_version == current_version {
            println!("Already on target version!");
            return Ok(());
        }

        if self.dry_run {
            return self.show_dry_run_plan(&target_version, &platform);
        }

        // 6. Find and download release
        let release = releases.iter()
            .find(|r| r.tag_name == format!("v{}", target_version))
            .ok_or(eyre!("Release not found"))?;

        let asset = fetcher.find_asset(release, &platform.platform_str)?;

        let download_manager = DownloadManager::new(true);
        let archive = download_manager.download(&asset.url, temp_dir()).await?;

        // 7. Verify checksum if available
        if let Some(checksum) = asset.checksum {
            download_manager.verify_checksum(&archive, &checksum).await?;
            println!("Checksum verified ✓");
        }

        // 8. Create backup
        if !self.no_backup {
            let backup_mgr = BackupManager::new(self.backup_dir.clone());
            let backup_path = backup_mgr.create_backup(&current_exe()?)?;
            println!("Backup created: {}", backup_path.display());
        }

        // 9. Install new version
        let installer = Installer::new()?;
        installer.install(&archive, &target_version)?;

        println!("\n✓ Successfully upgraded to v{}!", target_version);
        println!("\nRun 'otto --version' to verify.");

        Ok(())
    }

    async fn execute_rollback(&self) -> Result<()> {
        let backup_mgr = BackupManager::new(self.backup_dir.clone());
        let backups = backup_mgr.list_backups()?;

        let latest = backups.first()
            .ok_or(eyre!("No backups found"))?;

        println!("Rolling back to v{}...", latest.version);

        if self.dry_run {
            println!("\nWould restore: {}", latest.path.display());
            return Ok(());
        }

        // Create backup of current version first
        if !self.no_backup {
            backup_mgr.create_backup(&current_exe()?)?;
        }

        backup_mgr.restore_backup(&latest.path)?;

        println!("✓ Successfully rolled back to v{}!", latest.version);

        Ok(())
    }

    async fn execute_list_versions(&self) -> Result<()> {
        let fetcher = ReleaseFetcher::new(self.github_token.clone());
        let releases = fetcher.fetch_releases().await?;

        println!("Available versions:");
        for (i, release) in releases.iter().enumerate() {
            let version = release.tag_name.trim_start_matches('v');
            let latest_tag = if i == 0 { " (latest)" } else { "" };
            println!("  {}{} - {}", version, latest_tag, release.published_at);
        }

        let current = VersionManager::current()?;
        println!("\nCurrent version: {}", current);

        Ok(())
    }
}
```

### 4. Integration Points

#### A. Register in builtins.rs

```rust
pub const BUILTIN_COMMANDS: &[&str] = &[
    "Clean",
    "Convert",
    "Graph",
    "History",
    "Stats",
    "Upgrade",  // Add this
];
```

#### B. Add injection in parser.rs

```rust
fn inject_upgrade_meta_task(&mut self) {
    use crate::cfg::param::{Nargs, ParamType};

    let upgrade_task = TaskSpec {
        name: "Upgrade".to_string(),
        help: Some("[built-in] Upgrade Otto to a newer version".to_string()),
        after: vec![],
        before: vec![],
        input: vec![],
        output: vec![],
        envs: HashMap::new(),
        params: {
            let mut params = HashMap::new();

            params.insert(
                "dry-run".to_string(),
                ParamSpec {
                    name: "dry-run".to_string(),
                    short: None,
                    long: Some("dry-run".to_string()),
                    param_type: ParamType::FLG,
                    // ... rest of param spec
                },
            );

            params.insert(
                "version".to_string(),
                ParamSpec {
                    name: "version".to_string(),
                    short: Some('v'),
                    long: Some("version".to_string()),
                    param_type: ParamType::OPT,
                    // ... rest of param spec
                },
            );

            // Add other parameters...

            params
        },
        action: "# Built-in upgrade command".to_string(),
    };

    self.config_spec.tasks.insert("Upgrade".to_string(), upgrade_task);
}

fn inject_builtin_commands(&mut self) {
    self.inject_clean_meta_task();
    self.inject_convert_meta_task();
    self.inject_graph_meta_task();
    self.inject_history_meta_task();
    self.inject_stats_meta_task();
    self.inject_upgrade_meta_task();  // Add this
}
```

#### C. Add routing in main.rs

```rust
#[tokio::main]
async fn main() {
    // ... setup ...

    if args.len() > 1 {
        match args[1].as_str() {
            "Clean" => {
                if let Err(e) = execute_clean_command(&args[1..]).await {
                    eprintln!("{e}");
                    std::process::exit(1);
                }
                return;
            }
            // ... other commands ...
            "Upgrade" => {
                if let Err(e) = execute_upgrade_command(&args[1..]).await {
                    eprintln!("{e}");
                    std::process::exit(1);
                }
                return;
            }
            _ => {}
        }
    }

    // ... rest of main ...
}

async fn execute_upgrade_command(args: &[String]) -> Result<(), Report> {
    use clap::Parser;

    let upgrade_cmd = UpgradeCommand::parse_from(args);
    upgrade_cmd.execute().await?;
    Ok(())
}
```

## Testing Strategy

### Unit Tests

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_platform_detection() {
        let platform = PlatformInfo::detect().unwrap();
        assert!(!platform.platform_str.is_empty());
        assert!(platform.os == "linux" || platform.os == "macos");
    }

    #[test]
    fn test_version_comparison() {
        assert_eq!(
            VersionManager::compare("0.5.5", "0.5.6"),
            Ordering::Less
        );
        assert_eq!(
            VersionManager::compare("0.5.6", "0.5.6"),
            Ordering::Equal
        );
    }

    #[test]
    fn test_backup_creation() {
        let temp_dir = TempDir::new().unwrap();
        let backup_mgr = BackupManager::new(temp_dir.path().to_path_buf());

        // Create fake binary
        let fake_bin = temp_dir.path().join("otto");
        fs::write(&fake_bin, b"fake binary").unwrap();

        let backup = backup_mgr.create_backup(&fake_bin).unwrap();
        assert!(backup.exists());
    }
}
```

### Integration Tests

```rust
#[tokio::test]
async fn test_upgrade_dry_run() {
    let cmd = UpgradeCommand {
        dry_run: true,
        version: Some("0.5.5".to_string()),
        force: false,
        // ... other fields
    };

    // Should not fail and should show plan
    assert!(cmd.execute().await.is_ok());
}

#[tokio::test]
async fn test_upgrade_with_mock_release() {
    // Use mock HTTP server for testing
    // Verify download, extraction, installation
}

#[tokio::test]
async fn test_rollback_functionality() {
    // Create backup, modify binary, rollback
    // Verify original binary is restored
}
```

### Manual Testing Checklist

- [ ] Test on Linux x86_64
- [ ] Test on Linux ARM64
- [ ] Test on macOS Intel
- [ ] Test on macOS ARM64
- [ ] Test with GitHub token
- [ ] Test without GitHub token (rate limiting)
- [ ] Test dry-run mode
- [ ] Test rollback
- [ ] Test upgrade to specific version
- [ ] Test upgrade to latest
- [ ] Test force upgrade
- [ ] Test interrupted download (kill mid-download)
- [ ] Test corrupted archive
- [ ] Test with no internet connection
- [ ] Test with invalid version
- [ ] Test when already up-to-date
- [ ] Test backup creation and restoration

## Dependencies

Add to `Cargo.toml`:

```toml
[dependencies]
# HTTP client for GitHub API and downloads
reqwest = { version = "0.11", features = ["json", "stream"] }

# Async runtime (already present but ensure "full" features)
tokio = { version = "1", features = ["full"] }

# JSON parsing for GitHub API
serde = { version = "1.0", features = ["derive"] }
serde_json = "1.0"

# For checksum verification
sha2 = "0.10"
hex = "0.4"

# Progress bars for downloads
indicatif = "0.17"

# Tarball extraction
tar = "0.4"
flate2 = "1.0"

# Semantic versioning
semver = "1.0"
```

## Rollout Plan

### Phase 1: Basic Implementation (Week 1)
- Core upgrade functionality
- Platform detection
- GitHub API integration
- Download and install
- Basic tests

### Phase 2: Safety Features (Week 1)
- Backup/rollback
- Checksum verification
- Error handling
- Dry-run mode

### Phase 3: Polish (Week 2)
- Progress bars
- Better error messages
- List versions
- Comprehensive tests

### Phase 4: Documentation (Week 2)
- User documentation
- Code documentation
- Release process docs

## Risk Mitigation

| Risk | Mitigation |
|------|------------|
| Binary replacement fails on Windows | Implement delayed upgrade (next run) for Windows |
| Network failures during download | Add retry logic with exponential backoff |
| Corrupted downloads | Always verify checksums before installation |
| GitHub rate limiting | Support authentication tokens, clear error messages |
| Backup fails | Check disk space and permissions before starting |
| New version doesn't work | Always create backup, provide rollback |
| Breaking file permissions | Preserve executable permissions on Unix |

## Success Metrics

- ✅ Users can upgrade with a single command
- ✅ Works on all supported platforms
- ✅ Zero external dependencies (no `gh`, `curl`, etc.)
- ✅ Safe (backups, verification, rollback)
- ✅ Fast (parallel downloads, efficient extraction)
- ✅ Clear feedback (progress bars, helpful errors)
- ✅ Well-tested (>80% coverage)
- ✅ Well-documented (user and developer docs)

## Future Enhancements

Ideas for future iterations:

1. **Auto-update checks**: Notify users when updates available
2. **Release channels**: Support stable/beta/nightly channels
3. **Delta updates**: Download only changed bytes (smaller updates)
4. **Plugins**: Upgrade system for otto plugins
5. **Update scheduling**: Automatic updates in background
6. **Telemetry**: Opt-in usage stats for version adoption
7. **Proxy support**: HTTP proxy configuration
8. **Offline mode**: Upgrade from local files

## Appendix: File Structure

```
otto/
├── src/
│   ├── cli/
│   │   ├── builtins.rs           (register Upgrade)
│   │   ├── commands/
│   │   │   ├── clean.rs
│   │   │   ├── upgrade.rs        (NEW - main implementation)
│   │   │   └── ...
│   │   ├── parser.rs              (add inject_upgrade_meta_task)
│   │   └── ...
│   ├── main.rs                    (add routing)
│   └── ...
├── tests/
│   ├── upgrade_test.rs            (NEW - integration tests)
│   └── ...
├── docs/
│   ├── commands/
│   │   ├── upgrade.md             (NEW - user docs)
│   │   └── ...
│   ├── upgrade-builtin-implementation-plan.md  (THIS FILE)
│   └── ...
└── Cargo.toml                     (add dependencies)
```

## References

- [GitHub Releases API Documentation](https://docs.github.com/en/rest/releases)
- [Rustup Self-Update Implementation](https://github.com/rust-lang/rustup/blob/master/src/cli/self_update.rs)
- [Cargo Install Implementation](https://github.com/rust-lang/cargo/blob/master/src/cargo/ops/cargo_install.rs)
- [Clean Command Documentation](commands/clean.md)
- [Built-in Commands Design](capitalized-builtins-design.md)
