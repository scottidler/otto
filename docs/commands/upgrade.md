# `otto Upgrade` - Upgrade the Otto Binary

`Upgrade` replaces the running `otto` binary with a release fetched from GitHub, in place.

> **Note**: Built-in commands are capitalized (e.g., `Upgrade`, `Clean`) to avoid namespace conflicts with user-defined tasks.

Regenerated from `otto Upgrade --help` and live runs against the real GitHub releases API (`otto-rs/otto`); every example below is observed output. The prior version of this page was a pre-implementation planning document (checklists, a releases-host override flag that was never built, an effort estimate); it has been moved to [`docs/archive/upgrade-implementation-plan.md`](../archive/upgrade-implementation-plan.md).

## Usage

```bash
otto Upgrade [OPTIONS]
```

## Options

```
$ otto Upgrade --help
Upgrade Otto to a newer version

Usage: otto Upgrade [OPTIONS]

Options:
      --dry-run                      Show what would be done without doing it
  -v, --version <VERSION>            Specific version to upgrade to (e.g., "0.5.6")
      --list-versions                List available versions
      --rollback                     Rollback to previous version
      --force                        Force upgrade even if already on target version
      --no-backup                    Skip creating backup
      --backup-dir <BACKUP_DIR>      Directory for backups (default: ~/.otto/backups)
      --github-token <GITHUB_TOKEN>  GitHub token for API access (avoids rate limits) [env: GITHUB_TOKEN]
  -h, --help                         Print help
```

There is no flag to point at a different releases host: every release comes from `otto-rs/otto` on GitHub. `--github-token`'s value is deliberately never echoed by `--help` (clap would otherwise print `[env: GITHUB_TOKEN=<the live token>]`), so setting `$GITHUB_TOKEN` and running `--help` cannot leak it into a terminal, CI log, or transcript.

## Examples

### Check and upgrade

```bash
otto Upgrade --dry-run
otto Upgrade
otto Upgrade --version 2.1.0
otto Upgrade --force        # reinstall the current version, or downgrade
```

### List and roll back

```bash
otto Upgrade --list-versions
otto Upgrade --rollback
otto Upgrade --rollback --dry-run
```

### Backups

```bash
otto Upgrade --no-backup
otto Upgrade --backup-dir /var/backups/otto
```

## Observed output

### Dry run

```
$ otto Upgrade --dry-run
Checking for updates...
Platform: linux-amd64
Current version: v2.2.1-<commits-since-tag>-g<short-sha>
Target version:  v2.2.1

Dry run - would perform the following actions:
  1. Download otto-v2.2.1-linux-amd64.tar.gz
  2. Create backup: /home/user/.otto/backups/otto-<current>-<timestamp>.backup
  3. Check the download against the release's published .sha256 checksum
  4. Extract the new binary from the archive
  5. Run the new binary's --version to confirm it works
  6. Replace /path/to/otto

Run without --dry-run to perform upgrade.
```

The `Current version` line is `git describe --tags --always` of the running binary (`build.rs` bakes it in as `GIT_DESCRIBE`), so it changes on every commit; it is shown here as a placeholder rather than a pasted value, which would be stale one commit later.

With `--no-backup`, step 2 is omitted and the rest renumber — the plan is generated, not a fixed template, so the step count always matches what will actually run.

### Downgrading without `--force`

```
$ otto Upgrade --dry-run --version 2.1.0
Checking for updates...
Platform: linux-amd64
Current version: v2.2.1-<commits-since-tag>-g<short-sha>
Target version:  v2.1.0

Current version is newer than target version.
Use --force to downgrade.
```

### `--list-versions`

```
$ otto Upgrade --list-versions
Fetching available versions...

Available versions:
  v2.2.1 (latest)  - 2026-09-02
  v2.2.0           - 2026-09-02
  v2.1.0           - 2026-09-01
  ...
```

The `(latest)` marker is attached to whichever release GitHub's `/releases/latest` endpoint actually returns, not to the first (newest-created) entry in the list — the two can differ for a pre-release or a release created out of order.

### `--rollback`

Rollback restores **the newest backup whose version is strictly older than the version currently running** — not simply "the newest backup file". A rollback itself writes a safety backup of the binary it is about to replace, so "newest file in the directory" would usually be the backup the *previous* rollback just wrote, and repeated rollbacks would toggle between two versions instead of walking backward. A backup whose filename doesn't parse as a semver is skipped, since it can't be ordered against the current version.

```
$ otto Upgrade --rollback --dry-run
Rolling back to v2.0.0...

Would restore: /home/user/.otto/backups/otto-2.0.0-1000.backup
```

With no eligible backup:

```
$ otto Upgrade --rollback
No backup directory found at /home/user/.otto/backups
```

(exit code 1)

### Backup location and `$OTTO_HOME`

Backups live under `$OTTO_HOME/backups` (default `$HOME/.otto/backups`) unless `--backup-dir` overrides it. If `$OTTO_HOME` points somewhere non-default, a backup made under `$HOME/.otto/backups` from before that change is invisible to `--rollback` — it looks in the current `$OTTO_HOME` only, not every directory a backup was ever written to.

## Safety

- **Checksum**: every download is checked against the release's published `.sha256` file before the archive is trusted.
- **Backup by default**: `--no-backup` is required to skip it.
- **Staged install**: the new binary is staged beside the target and its own `--version` is run to confirm it works before the running binary is replaced.

## Related Commands

- [`otto Clean`](clean.md) - Manage disk usage
- [`otto History`](history.md) - View execution history
