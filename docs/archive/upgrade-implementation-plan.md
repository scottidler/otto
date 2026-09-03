# Upgrade Built-in - Implementation Plan

## Overview

This document outlines the plan to implement an `Upgrade` built-in command for Otto that will enable users to upgrade their Otto installation to newer versions with a simple command.

## Current State

Currently, upgrading Otto requires manual steps or a custom shell script (as shown in Patrick Shelby's `.zshrc` example):

```bash
otto-upgrade() {
  gh release -R otto-rs/otto download --pattern 'otto-*-macos-arm64.tar.gz' && \
  export OTTO_VERSION=$(ls otto-*-macos-arm64.tar.gz | cut -d- -f2) && \
  tar -zxf otto-$OTTO_VERSION-macos-arm64.tar.gz && \
  mv otto ~/.local/bin/otto-$OTTO_VERSION && \
  unlink ~/.local/bin/otto 2>/dev/null || true && \
  ln -s ~/.local/bin/otto-$OTTO_VERSION ~/.local/bin/otto && \
  echo "Upgraded Otto version to $OTTO_VERSION"
  unset OTTO_VERSION
}
```

This approach has several limitations:
- Requires `gh` CLI tool to be installed
- Platform-specific (hardcoded for macOS ARM64)
- Not discoverable (requires users to know about and install custom scripts)
- No version validation or rollback capability
- Manual error handling

## Goals

1. **User Experience**: Provide a simple `otto Upgrade` command that works out of the box
2. **Cross-Platform**: Support Linux (x86_64, ARM64) and macOS (Intel, ARM64)
3. **Safety**: Include dry-run mode, version validation, and automatic backup
4. **Flexibility**: Allow upgrading to specific versions or latest
5. **Reliability**: Handle network failures, verify downloads, support rollback
6. **Discoverability**: Integrate with otto's help system

## Proposed Interface

### Basic Usage

```bash
# Upgrade to latest version
otto Upgrade

# Check what would be upgraded (dry-run)
otto Upgrade --dry-run

# Upgrade to specific version
otto Upgrade --version 0.5.6

# Show available versions
otto Upgrade --list-versions

# Rollback to previous version
otto Upgrade --rollback

# Force upgrade even if same version
otto Upgrade --force
```

### Command Options

| Option | Description | Default |
|--------|-------------|---------|
| `--dry-run` | Show what would be upgraded without upgrading | false |
| `--version <VERSION>` | Upgrade to specific version (e.g., "0.5.5") | latest |
| `--list-versions` | List available versions from GitHub releases | - |
| `--force` | Force upgrade even if already on latest/target version | false |
| `--rollback` | Rollback to previously installed version | - |
| `--backup-dir <DIR>` | Directory to store backups | `~/.otto/backups` |
| `--no-backup` | Skip creating backup of current version | false |
| `--github-token <TOKEN>` | GitHub token for API access (avoids rate limits) | env: `GITHUB_TOKEN` |
| `--base-url <URL>` | Base URL for GitHub releases | `https://github.com/otto-rs/otto` |

## Technical Design

### Architecture

```
otto Upgrade
    ↓
UpgradeCommand
    ↓
├── Version Detection (current_version())
├── Release Fetcher (fetch_releases())
├── Platform Detection (detect_platform())
├── Download Manager (download_release())
├── Checksum Verifier (verify_checksum())
├── Backup Manager (backup_current())
└── Installer (install_new_version())
```

### Platform Detection

Detect platform using Rust's standard library:

```rust
fn detect_platform() -> Result<String> {
    let os = std::env::consts::OS;
    let arch = std::env::consts::ARCH;

    match (os, arch) {
        ("linux", "x86_64") => Ok("linux-x86_64"),
        ("linux", "aarch64") => Ok("linux-arm64"),
        ("macos", "x86_64") => Ok("macos-x86_64"),
        ("macos", "aarch64") => Ok("macos-arm64"),
        _ => Err(eyre!("Unsupported platform: {}-{}", os, arch))
    }
}
```

### Version Detection

Read current version from:
1. Binary metadata (`--version` flag output)
2. Fallback to parsing binary itself if needed

```rust
fn current_version() -> Result<String> {
    let output = Command::new(current_exe()?)
        .arg("--version")
        .output()?;

    let version_str = String::from_utf8(output.stdout)?;
    // Parse "otto v0.5.5" -> "0.5.5"
    extract_version(&version_str)
}
```

### Release Fetching

Use GitHub API to fetch releases:

```rust
async fn fetch_releases(token: Option<String>) -> Result<Vec<Release>> {
    let client = reqwest::Client::new();
    let url = "https://api.github.com/repos/otto-rs/otto/releases";

    let mut request = client.get(url);
    if let Some(token) = token {
        request = request.header("Authorization", format!("Bearer {}", token));
    }

    let releases: Vec<Release> = request
        .send().await?
        .json().await?;

    Ok(releases)
}
```

### Download & Verification

```rust
async fn download_release(version: &str, platform: &str) -> Result<PathBuf> {
    let filename = format!("otto-{}-{}.tar.gz", version, platform);
    let url = format!(
        "https://github.com/otto-rs/otto/releases/download/v{}/{}",
        version, filename
    );

    // Download to temporary location
    let temp_dir = TempDir::new()?;
    let temp_file = temp_dir.path().join(&filename);

    let response = reqwest::get(&url).await?;
    let mut file = File::create(&temp_file)?;
    file.write_all(&response.bytes().await?)?;

    // Verify checksum if available
    if let Ok(checksum_url) = get_checksum_url(version, &filename) {
        verify_checksum(&temp_file, &checksum_url).await?;
    }

    Ok(temp_file)
}
```

### Installation Process

```rust
async fn install_new_version(archive: &Path, backup: bool) -> Result<()> {
    let current_exe = current_exe()?;
    let install_dir = current_exe.parent().ok_or(eyre!("No parent dir"))?;

    // Create backup if requested
    if backup {
        backup_current(&current_exe)?;
    }

    // Extract new version
    let temp_dir = TempDir::new()?;
    extract_tarball(archive, temp_dir.path())?;

    let new_binary = temp_dir.path().join("otto");

    // Verify new binary works
    verify_binary(&new_binary)?;

    // Replace current binary (atomic rename)
    let versioned_name = format!("otto-{}", get_version(&new_binary)?);
    let versioned_path = install_dir.join(&versioned_name);

    fs::rename(&new_binary, &versioned_path)?;

    // Update symlink/main binary
    #[cfg(unix)]
    {
        let _ = fs::remove_file(&current_exe);
        std::os::unix::fs::symlink(&versioned_path, &current_exe)?;
    }

    #[cfg(not(unix))]
    {
        fs::rename(&versioned_path, &current_exe)?;
    }

    Ok(())
}
```

### Backup & Rollback

```rust
fn backup_current(exe: &Path) -> Result<PathBuf> {
    let backup_dir = get_backup_dir()?;
    fs::create_dir_all(&backup_dir)?;

    let version = current_version()?;
    let backup_name = format!("otto-{}-{}.backup", version, timestamp());
    let backup_path = backup_dir.join(&backup_name);

    fs::copy(exe, &backup_path)?;

    // Update "latest backup" symlink
    let latest_link = backup_dir.join("otto-latest.backup");
    let _ = fs::remove_file(&latest_link);
    std::os::unix::fs::symlink(&backup_path, &latest_link)?;

    Ok(backup_path)
}

fn rollback_to_previous() -> Result<()> {
    let backup_dir = get_backup_dir()?;
    let latest_backup = backup_dir.join("otto-latest.backup");

    if !latest_backup.exists() {
        return Err(eyre!("No backup found to rollback to"));
    }

    let current_exe = current_exe()?;

    // Backup current before rollback (in case rollback fails)
    backup_current(&current_exe)?;

    // Replace with backup
    fs::copy(&latest_backup, &current_exe)?;

    // Verify rollback worked
    verify_binary(&current_exe)?;

    println!("Successfully rolled back to previous version");
    Ok(())
}
```

## Implementation Steps

### Phase 1: Core Implementation (src/cli/commands/upgrade.rs)

1. **Create Command Structure**
   - [ ] Define `UpgradeCommand` struct with clap parameters
   - [ ] Implement `execute()` method with basic flow
   - [ ] Add platform detection logic
   - [ ] Add current version detection

2. **GitHub Integration**
   - [ ] Implement GitHub API client for releases
   - [ ] Add release parsing logic
   - [ ] Handle authentication (token support)
   - [ ] Implement rate limit handling

3. **Download & Verification**
   - [ ] Implement download manager with progress bar
   - [ ] Add checksum verification
   - [ ] Handle network errors and retries
   - [ ] Add timeout handling

4. **Installation Logic**
   - [ ] Implement backup creation
   - [ ] Add tarball extraction
   - [ ] Implement atomic binary replacement
   - [ ] Add post-install verification

5. **Rollback Support**
   - [ ] Implement backup management
   - [ ] Add rollback command logic
   - [ ] Test rollback scenarios

### Phase 2: Integration (src/cli/*)

1. **Built-in Registration**
   - [ ] Add "Upgrade" to `BUILTIN_COMMANDS` in `src/cli/builtins.rs`
   - [ ] Create `inject_upgrade_meta_task()` in `src/cli/parser.rs`
   - [ ] Add early routing in `src/main.rs`
   - [ ] Add `execute_upgrade_command()` handler in `src/main.rs`

2. **Help Integration**
   - [ ] Ensure help text shows up in `otto --help`
   - [ ] Add `otto Upgrade --help` support
   - [ ] Include examples in help output

### Phase 3: Testing (tests/)

1. **Unit Tests**
   - [ ] Test platform detection
   - [ ] Test version parsing
   - [ ] Test backup/rollback logic
   - [ ] Test URL construction
   - [ ] Test error handling

2. **Integration Tests**
   - [ ] Test upgrade flow with mock releases
   - [ ] Test rollback scenarios
   - [ ] Test network failure handling
   - [ ] Test checksum verification

3. **Manual Testing**
   - [ ] Test on Linux x86_64
   - [ ] Test on Linux ARM64
   - [ ] Test on macOS Intel
   - [ ] Test on macOS ARM64
   - [ ] Test with rate limiting
   - [ ] Test interrupted downloads

### Phase 4: Documentation

1. **User Documentation**
   - [ ] Complete this planning document
   - [ ] Create user-facing guide in docs/commands/upgrade.md
   - [ ] Add upgrade examples to main README
   - [ ] Update migration guide if needed

2. **Developer Documentation**
   - [ ] Document release process for maintainers
   - [ ] Document checksum generation
   - [ ] Document testing procedures

## Example Output

### Successful Upgrade

```
$ otto Upgrade
Checking for updates...
Current version: v0.5.5
Latest version:  v0.5.6

Downloading otto-0.5.6-macos-arm64.tar.gz...
[████████████████████████████] 5.2 MB / 5.2 MB (100%)

Verifying checksum... ✓
Creating backup of current version... ✓
Installing new version... ✓

Successfully upgraded to v0.5.6!
Backup saved to: ~/.otto/backups/otto-0.5.5-1730937600.backup

Run 'otto --version' to verify.
```

### Dry Run

```
$ otto Upgrade --dry-run
Checking for updates...
Current version: v0.5.5
Latest version:  v0.5.6

Would perform the following actions:
  1. Download otto-0.5.6-macos-arm64.tar.gz (5.2 MB)
  2. Verify checksum
  3. Create backup: ~/.otto/backups/otto-0.5.5-1730937600.backup
  4. Install new binary to ~/.local/bin/otto
  5. Update symlink

Run without --dry-run to perform upgrade.
```

### List Versions

```
$ otto Upgrade --list-versions
Available versions:
  v0.5.6 (latest) - 2025-11-06
  v0.5.5         - 2025-11-05
  v0.5.4         - 2025-10-28
  v0.5.3         - 2025-10-15
  v0.5.2         - 2025-09-30
  ...

Current version: v0.5.5
```

### Rollback

```
$ otto Upgrade --rollback
Rolling back to previous version...
Current version: v0.5.6
Backup found:    v0.5.5

Creating safety backup of current version... ✓
Restoring v0.5.5... ✓
Verifying installation... ✓

Successfully rolled back to v0.5.5!

Current backup saved to: ~/.otto/backups/otto-0.5.6-1730937700.backup
```

### Already Up to Date

```
$ otto Upgrade
Checking for updates...
Current version: v0.5.6
Latest version:  v0.5.6

You are already on the latest version!

Use --force to reinstall the current version.
```

## Error Handling

### Network Errors

```
$ otto Upgrade
Checking for updates...
Error: Failed to connect to GitHub API
  Caused by: Connection timeout

Suggestions:
  - Check your internet connection
  - Use --github-token to avoid rate limits
  - Try again later

Run with --verbose for more details.
```

### Permission Errors

```
$ otto Upgrade
...
Error: Permission denied writing to ~/.local/bin/otto

Suggestions:
  - Run with appropriate permissions
  - Ensure ~/.local/bin is writable
  - Check if otto binary is currently in use
```

### Checksum Mismatch

```
$ otto Upgrade
...
Error: Checksum verification failed
  Expected: abc123...
  Got:      def456...

The downloaded file may be corrupted or tampered with.

Suggestions:
  - Try downloading again
  - Report to https://github.com/otto-rs/otto/issues
```

## Dependencies

New Rust crates needed:

```toml
[dependencies]
reqwest = { version = "0.11", features = ["json"] }
tokio = { version = "1", features = ["full"] }
serde = { version = "1.0", features = ["derive"] }
serde_json = "1.0"
sha2 = "0.10"
indicatif = "0.17"  # For progress bars
```

## Security Considerations

1. **Checksum Verification**: Always verify downloaded binaries against checksums
2. **HTTPS Only**: All downloads must use HTTPS
3. **Backup Before Replace**: Always backup current version before upgrading
4. **Binary Verification**: Verify new binary executes `--version` successfully before final install
5. **Atomic Replacement**: Use atomic file operations where possible
6. **Token Security**: Handle GitHub tokens securely, support env vars
7. **Path Validation**: Validate all file paths to prevent directory traversal

## Open Questions

1. **Release Assets**: Do we need to add checksums to GitHub releases?
   - Current releases may not include SHA256 checksums
   - Need to document checksum generation in release process

2. **Auto-Update Checks**: Should otto periodically check for updates?
   - Could add `--check-for-updates` flag to normal runs
   - Privacy considerations (making network requests)

3. **Channel Support**: Should we support release channels (stable/beta/nightly)?
   - Could add `--channel` flag
   - Would need channel metadata in releases

4. **Self-Upgrade on Windows**: How to handle Windows where running binaries can't be replaced?
   - May need launcher/stub approach
   - Or delayed upgrade on next run

5. **Update Notifications**: Should otto notify users when updates are available?
   - Could show message after successful task execution
   - Configurable in user preferences

6. **Build Metadata**: Do we embed version info in binary at compile time?
   - Need to ensure `--version` always returns correct version
   - May need build.rs updates

## Related Documentation

- [Built-in Commands Design](../capitalized-builtins-design.md)
- [Clean Command](clean.md)
- [History Command](history.md)
- [Stats Command](stats.md)

## Success Criteria

The implementation will be considered complete when:

1. ✅ Users can run `otto Upgrade` to upgrade to latest version
2. ✅ All supported platforms (Linux x64/ARM, macOS Intel/ARM) work
3. ✅ Dry-run mode works and shows accurate preview
4. ✅ Rollback functionality works reliably
5. ✅ Network errors are handled gracefully with clear messages
6. ✅ Checksums are verified for all downloads
7. ✅ Backups are created automatically before upgrades
8. ✅ Help text is clear and comprehensive
9. ✅ All tests pass (unit + integration)
10. ✅ Documentation is complete and accurate

## Timeline Estimate

- **Phase 1 (Core)**: 3-4 days
- **Phase 2 (Integration)**: 1 day
- **Phase 3 (Testing)**: 2-3 days
- **Phase 4 (Documentation)**: 1 day

**Total**: ~1-2 weeks for complete implementation

## References

- GitHub Releases API: https://docs.github.com/en/rest/releases
- Cargo Install: https://doc.rust-lang.org/cargo/commands/cargo-install.html
- Rustup Self-Update: https://github.com/rust-lang/rustup (similar problem space)
