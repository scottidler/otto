# Docker Release Setup

## Overview

Otto publishes multi-architecture Docker images to GitHub Container Registry (ghcr.io) on every tagged release. This enables Tatari's base-images to include the otto binary via `COPY --from=` without downloading tarballs.

## Registry

**Image:** `ghcr.io/otto-rs/otto`

**Tags for release `v0.6.3`:**
- `ghcr.io/otto-rs/otto:latest`
- `ghcr.io/otto-rs/otto:v0.6.3`
- `ghcr.io/otto-rs/otto:0.6.3`
- `ghcr.io/otto-rs/otto:0.6`
- `ghcr.io/otto-rs/otto:sha-<commit>`

**Platforms:**
- `linux/amd64` (x86_64)
- `linux/arm64` (Apple Silicon, AWS Graviton, etc.)

## Workflow Structure

The release workflow (`.github/workflows/release-and-publish.yml`) triggers on version tags (`v*`):

```
build-linux (matrix: amd64, arm64) ──┬──► docker (multi-arch manifest)
                                     │
                                     ├──► create-release (GitHub Release)
                                     │         ▲
build-macos (matrix: x86_64, arm64) ─┘─────────┘
```

### Jobs

| Job | Runs On | Purpose |
|-----|---------|---------|
| `build-linux` | ubuntu-latest | Matrix build for amd64 (native) and arm64 (cross-compile) |
| `build-macos` | macos-14 | Matrix build for x86_64 and arm64 (Apple Silicon) |
| `create-release` | ubuntu-latest | Creates GitHub Release with all platform binaries |
| `docker` | ubuntu-latest | Builds and pushes multi-arch Docker image to ghcr.io |

### Cross-Compilation

The arm64 Linux binary is cross-compiled on x86_64 using:
- Target: `aarch64-unknown-linux-gnu`
- Toolchain: `gcc-aarch64-linux-gnu`
- Linker: `CARGO_TARGET_AARCH64_UNKNOWN_LINUX_GNU_LINKER=aarch64-linux-gnu-gcc`

To avoid OpenSSL cross-compilation issues, `reqwest` uses `rustls-tls` instead of `native-tls`.

## Dockerfile

The Dockerfile uses `ARG TARGETARCH` (set automatically by buildx) to select the correct binary:

```dockerfile
ARG TARGETARCH
COPY --from=binaries ${TARGETARCH}/otto /usr/local/bin/otto
```

Base image is `debian:bookworm-slim` to match Tatari's base-images GLIBC version (2.36). The Linux builds run inside a `debian:bookworm` container to ensure binary compatibility.

## Usage in base-images

In Tatari's base-images Dockerfiles:

```dockerfile
ARG OTTO_VERSION="0.6.3"
FROM ghcr.io/otto-rs/otto:${OTTO_VERSION} AS otto

FROM python:3.11-slim-bookworm AS base
COPY --from=otto /usr/local/bin/otto /usr/local/bin/otto
```

This pattern is already used for `uv` in `python-uv/Dockerfile`.

## Releasing

```bash
# 1. Commit changes
git add .
git commit -m "Your changes"
git push origin main

# 2. Tag and release
git tag v0.6.4
git push origin v0.6.4
```

The workflow automatically:
1. Builds binaries for all 4 platforms
2. Creates GitHub Release with tarballs + checksums
3. Builds and pushes multi-arch Docker image

## Permissions Required

Repository settings:
- **Actions → General → Workflow permissions:** Read and write permissions

Organization settings (otto-rs):
- **Packages → Package creation:** Public enabled
- **Packages → Default package settings:** Inherit access from source repository

After first publish, ensure the package visibility is set to **Public** at:
`https://github.com/orgs/otto-rs/packages/container/otto/settings`

## Related Files

- `.github/workflows/release-and-publish.yml` - Release workflow
- `.github/workflows/ci.yml` - CI workflow (tests, builds on push/PR)
- `Dockerfile` - Multi-arch Docker image definition
- `.dockerignore` - Excludes unnecessary files from build context
- `Cargo.toml` - Uses `rustls-tls` for cross-compilation compatibility

