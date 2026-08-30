# Upgrade Command Fix

## Problem Description

The `otto Upgrade` command fails to find release assets and reports incorrect version information. When executed, it shows:

```
Current version: v0.1.3
Target version:  v0.5.7
No asset found for platform: linux-x86_64
```

However, the actual current version should be `v0.5.7` (from `git describe`), and the platform string doesn't match the GitHub release asset naming convention.

## Root Causes

### 1. Incorrect Version Detection

**Location:** `src/cli/commands/upgrade.rs` lines 278-287

The `current_version()` method uses `CARGO_PKG_VERSION` which pulls from `Cargo.toml` (hardcoded to `0.1.3`), but the actual version displayed by `otto --version` uses `GIT_DESCRIBE` (set at build time in `build.rs`).

```rust
fn current_version(&self) -> Result<String> {
    // Try to get version from environment variable (set at build time)
    if let Ok(version) = env::var("OTTO_VERSION") {
        return Ok(version.trim_start_matches('v').to_string());
    }

    // Fallback: parse from cargo package version
    let version = env!("CARGO_PKG_VERSION");  // <-- WRONG: Returns 0.1.3
    Ok(version.to_string())
}
```

The build system sets `GIT_DESCRIBE` at compile time (via `build.rs`), and this is what's used for `--version` output. The upgrade command should use the same source.

### 2. Incorrect Asset Name Pattern

**Location:** `src/cli/commands/upgrade.rs` lines 314-322

The `find_asset()` method searches for `otto-{platform}.tar.gz`, but GitHub releases use the pattern `otto-v{version}-{platform}.tar.gz`.

```rust
fn find_asset<'a>(&self, release: &'a GitHubRelease, platform: &str) -> Result<&'a GitHubAsset> {
    let pattern = format!("otto-{}.tar.gz", platform);  // <-- WRONG: Missing version prefix
    // ...
}
```

Actual asset names on GitHub v0.5.7 release:
- `otto-v0.5.7-linux.tar.gz`
- `otto-v0.5.7-macos-arm64.tar.gz`
- `otto-v0.5.7-macos-x86_64.tar.gz`

### 3. Incorrect Platform String Mapping

**Location:** `src/cli/commands/upgrade.rs` lines 65-71

The platform detection maps `("linux", "x86_64")` to `"linux-x86_64"`, but GitHub releases use just `"linux"` for x86_64.

```rust
let platform_str = match (os, arch) {
    ("linux", "x86_64") => "linux-x86_64",  // <-- WRONG: Should be "linux"
    ("linux", "aarch64") => "linux-arm64",
    ("macos", "x86_64") => "macos-x86_64",
    ("macos", "aarch64") => "macos-arm64",
    _ => return Err(eyre!("Unsupported platform: {}-{}", os, arch)),
};
```

## Solution

### Fix 1: Use GIT_DESCRIBE for Version Detection

Update `current_version()` to use `GIT_DESCRIBE` environment variable set at build time:

```rust
fn current_version(&self) -> Result<String> {
    // Use GIT_DESCRIBE which is set at build time in build.rs
    // This matches what --version displays
    let version = env!("GIT_DESCRIBE");
    Ok(version.trim_start_matches('v').to_string())
}
```

This ensures consistency between `otto --version` and the upgrade command's version detection.

### Fix 2: Update Asset Name Pattern

Update `find_asset()` to include the version in the pattern:

```rust
fn find_asset<'a>(&self, release: &'a GitHubRelease, platform: &str) -> Result<&'a GitHubAsset> {
    // Extract version from release tag
    let version = release.tag_name.trim_start_matches('v');
    let pattern = format!("otto-v{}-{}.tar.gz", version, platform);

    release
        .assets
        .iter()
        .find(|asset| asset.name == pattern)  // Use exact match instead of contains
        .ok_or_else(|| eyre!("No asset found for platform: {} (looking for {})", platform, pattern))
}
```

Using exact match instead of `contains` is more robust and provides better error messages.

### Fix 3: Correct Platform String Mapping

Update `PlatformInfo::detect()` to match actual GitHub release asset names:

```rust
let platform_str = match (os, arch) {
    ("linux", "x86_64") => "linux",           // Changed from "linux-x86_64"
    ("linux", "aarch64") => "linux-arm64",    // No change
    ("macos", "x86_64") => "macos-x86_64",    // No change
    ("macos", "aarch64") => "macos-arm64",    // No change
    _ => return Err(eyre!("Unsupported platform: {}-{}", os, arch)),
};
```

## Testing Considerations

After implementing these fixes, test with:

1. **Version Detection Test:**
   ```bash
   otto --version  # Should show git describe output
   otto Upgrade --dry-run  # Should show same version as current
   ```

2. **Asset Pattern Test:**
   ```bash
   otto Upgrade --dry-run  # Should find the correct asset
   otto Upgrade --list-versions  # Should display available versions
   ```

3. **Platform Detection Test:**
   - Test on Linux x86_64
   - Test on macOS (both Intel and ARM if available)

4. **Edge Cases:**
   - Test when already on latest version
   - Test with `--force` flag
   - Test rollback functionality after upgrade

## Implementation Notes

- All three fixes are in `src/cli/commands/upgrade.rs`
- No changes needed to `build.rs` or version display logic
- The fixes ensure consistency with existing version display behavior
- Error messages should be improved to show what pattern was searched for

## Related Files

- `src/cli/commands/upgrade.rs` - Main upgrade command implementation
- `build.rs` - Sets `GIT_DESCRIBE` at compile time
- `src/cli/parser.rs` - Uses `GIT_DESCRIBE` for `--version` output
- `Cargo.toml` - Contains package version (0.1.3, not used for display)
