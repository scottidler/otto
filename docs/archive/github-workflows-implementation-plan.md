# GitHub Workflows Implementation Plan

**Date**: November 5, 2025
**Status**: Ready for Implementation
**Author**: AI Assistant

## Overview

This document outlines the plan to implement GitHub Actions workflows for the `otto` project, including binary release automation and continuous integration. Additionally, we'll update all existing Rust projects to use the latest stable Rust version.

## Current State

### Otto Project
- **Version**: 0.1.3 (currently at v0.5.0 tag on GitHub)
- **Status**: No `.github/workflows/` directory exists
- **Needs**: Binary release workflow and CI workflow

### Existing Projects with Workflows
Based on analysis of sister projects under `~/repos/scottidler/`:

1. **aka** (`.github/workflows/binary-release.yml`)
   - Current Rust: 1.83.0
   - Platforms: Linux + macOS
   - Includes shell scripts in release

2. **git-tools** (`.github/workflows/binary-release.yml`)
   - Current Rust: 1.83.0
   - Platforms: Linux only
   - Multi-binary workspace

3. **gx** (`.github/workflows/binary-release.yml` + `ci.yml`)
   - Current Rust: 1.88.0
   - Platforms: Linux + macOS
   - Single binary (cleanest pattern)
   - Has CI workflow for testing

## Goals

1. ✅ Create binary release workflow for `otto` based on the `gx` pattern (cleanest)
2. ✅ Create CI workflow for `otto` to test on PRs and pushes
3. ✅ Update all projects to use **Rust 1.91.0** (latest stable as of Nov 2025)
4. ✅ Support both Linux and macOS builds for `otto`
5. ✅ Generate downloadable tar.gz artifacts

## Latest Rust Version

**Target Version**: `1.91.0` (latest stable)

## Implementation Plan

### Phase 1: Otto Project Setup

#### 1.1 Create Directory Structure
```bash
mkdir -p /home/saidler/repos/otto-rs/otto/.github/workflows
```

#### 1.2 Create `binary-release.yml`

**File**: `.github/workflows/binary-release.yml`

**Key Features**:
- Triggers on tags matching `v*` pattern (e.g., `v0.5.0`, `v0.6.0`)
- Matrix build for `ubuntu-latest` and `macos-latest`
- Rust version: `1.91.0`
- Cargo caching for faster builds
- Git describe for version tracking
- Two-job structure:
  - `build-and-package`: Builds binaries for each platform
  - `create-release`: Creates GitHub release with all artifacts
- Permissions: `contents: write` for release creation

**Artifact Naming**:
- `otto-{tag}-linux.tar.gz` (e.g., `otto-v0.6.0-linux.tar.gz`)
- `otto-{tag}-macos.tar.gz` (e.g., `otto-v0.6.0-macos.tar.gz`)

**Based On**: `gx/.github/workflows/binary-release.yml` (cleanest implementation)

#### 1.3 Create `ci.yml`

**File**: `.github/workflows/ci.yml`

**Key Features**:
- Triggers on:
  - Push to `main` and `makefile` branches
  - All pull requests
- Runs on `ubuntu-latest`
- Rust version: `1.91.0`
- Cargo caching
- Jobs:
  - `test`: Runs `cargo test --all-features`
  - `clippy`: Runs `cargo clippy -- -D warnings`
  - `fmt`: Checks `cargo fmt --all -- --check`
  - `build`: Verifies release build succeeds

**Based On**: `gx/.github/workflows/ci.yml` with enhancements

### Phase 2: Update Existing Projects

#### 2.1 Update `aka` Project
**File**: `/home/saidler/repos/scottidler/aka/.github/workflows/binary-release.yml`
- Change `RUST_VERSION` from `1.83.0` to `1.91.0`

#### 2.2 Update `git-tools` Project
**File**: `/home/saidler/repos/scottidler/git-tools/.github/workflows/binary-release.yml`
- Change `RUST_VERSION` from `1.83.0` to `1.91.0`

#### 2.3 Update `gx` Project
**File**: `/home/saidler/repos/scottidler/gx/.github/workflows/binary-release.yml`
- Change `RUST_VERSION` from `1.88.0` to `1.91.0`

**File**: `/home/saidler/repos/scottidler/gx/.github/workflows/ci.yml`
- Change `RUST_VERSION` to `1.91.0` (if applicable)

## Detailed Workflow Specifications

### Binary Release Workflow Structure

```yaml
name: binary-release

on:
  push:
    tags:
      - 'v*'

env:
  RUST_VERSION: 1.91.0
  CARGO_TERM_COLOR: always

permissions:
  contents: write

jobs:
  build-and-package:
    runs-on: ${{ matrix.os }}
    strategy:
      matrix:
        include:
          - os: ubuntu-latest
            target: x86_64-unknown-linux-gnu
            suffix: linux
          - os: macos-latest
            target: x86_64-apple-darwin
            suffix: macos

    steps:
      - Checkout with full history and tags
      - Set GIT_DESCRIBE environment variable
      - Cache cargo registry
      - Set up Rust toolchain (1.91.0)
      - Build for target (release mode)
      - Package binary into artifacts/
      - Create tar.gz archive
      - Upload artifacts

  create-release:
    needs: build-and-package
    runs-on: ubuntu-latest

    steps:
      - Download all artifacts
      - Create GitHub Release using softprops/action-gh-release@v2
```

### CI Workflow Structure

```yaml
name: ci

on:
  push:
    branches: [main, makefile]
  pull_request:

env:
  RUST_VERSION: 1.91.0
  CARGO_TERM_COLOR: always

jobs:
  test:
    runs-on: ubuntu-latest
    steps:
      - Checkout
      - Cache cargo
      - Set up Rust
      - Run tests with all features

  clippy:
    runs-on: ubuntu-latest
    steps:
      - Checkout
      - Cache cargo
      - Set up Rust with clippy
      - Run clippy with warnings as errors

  fmt:
    runs-on: ubuntu-latest
    steps:
      - Checkout
      - Set up Rust with rustfmt
      - Check formatting

  build:
    runs-on: ubuntu-latest
    steps:
      - Checkout
      - Cache cargo
      - Set up Rust
      - Build in release mode
```

## Testing Plan

### Pre-Implementation Testing
1. Review workflow files locally for syntax errors
2. Validate YAML structure using yamllint or online validator

### Post-Implementation Testing
1. **Test Binary Release Workflow**:
   - Create a test tag (e.g., `v0.5.1-test`)
   - Push tag and verify workflow runs
   - Check artifact generation
   - Verify GitHub release creation
   - Download and test binaries on both platforms

2. **Test CI Workflow**:
   - Create a test branch
   - Push commit and verify CI runs
   - Create a PR and verify CI runs on PR
   - Intentionally break test/fmt/clippy to verify failures are caught

### Rollback Plan
If workflows fail:
1. Delete problematic tag
2. Fix workflow files
3. Re-tag and push

## Benefits

1. **Automated Releases**: Push a tag, get platform-specific binaries automatically
2. **Quality Gates**: CI catches issues before merge
3. **Consistent Tooling**: All projects on same Rust version
4. **Easy Distribution**: Users can download pre-built binaries
5. **Version Tracking**: Git describe provides build provenance

## Post-Implementation Tasks

1. Update README.md with download instructions
2. Document release process for maintainers
3. Consider adding Windows support in future
4. Consider adding ARM64 (Apple Silicon) support
5. Add release notes template

## Example Usage

### Creating a Release
```bash
# Tag the release
git tag v0.6.0
git push origin v0.6.0

# Workflow automatically:
# - Builds for Linux and macOS
# - Creates tar.gz files
# - Creates GitHub release
# - Uploads artifacts
```

### Installing from Release
```bash
# Download the appropriate tar.gz for your platform
wget https://github.com/otto-rs/otto/releases/download/v0.6.0/otto-v0.6.0-linux.tar.gz

# Extract
tar -xzf otto-v0.6.0-linux.tar.gz

# Move to PATH
sudo mv otto /usr/local/bin/
```

## Dependencies

- GitHub Actions (free for public repos)
- No external services required
- Uses official GitHub actions:
  - `actions/checkout@v4`
  - `actions/cache@v4`
  - `actions/upload-artifact@v4`
  - `actions/download-artifact@v4`
  - `softprops/action-gh-release@v2`

## Timeline

- **Phase 1** (Otto workflows): ~30 minutes
- **Phase 2** (Update existing projects): ~15 minutes
- **Testing**: ~30 minutes
- **Total**: ~1.5 hours

## Success Criteria

✅ Otto has working binary-release.yml
✅ Otto has working ci.yml
✅ All projects use Rust 1.91.0
✅ Test release succeeds with downloadable artifacts
✅ CI catches test/clippy/fmt failures
✅ Workflows follow established patterns from gx project

---

**Ready to Execute**: Yes
**Approval Status**: Pending user confirmation
**Next Step**: Create workflow files and update existing projects
