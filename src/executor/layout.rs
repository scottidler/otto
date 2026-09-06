//! Where otto keeps its state on disk.
//!
//! One place decides what the otto home is, what a project's run root is called,
//! and how to read that name back. Before this, four call sites each built or
//! parsed the name themselves and two of them were wrong: `Workspace` created
//! `<name>-<hash>` while the database cleanup rebuilt `otto-<hash>`, so
//! DB-driven `Clean` reported success and left the directories behind.

use eyre::{Context, Result};
use std::path::{Path, PathBuf};

/// Number of characters of the project-path hash that otto appends to a project
/// directory name. Kept here because both the writer and the readers need it.
pub const PROJECT_HASH_LEN: usize = 8;

/// Resolve the otto home directory.
///
/// Uses `$OTTO_HOME` if set, otherwise `$HOME/.otto`. This is the single knob
/// that moves otto's state: run directories and the database both derive from
/// it.
pub fn resolve_otto_home() -> Result<PathBuf> {
    if let Ok(otto_home) = std::env::var("OTTO_HOME") {
        Ok(PathBuf::from(otto_home))
    } else {
        let home = std::env::var("HOME").context("Failed to get HOME")?;
        Ok(PathBuf::from(home).join(".otto"))
    }
}

/// XDG data dir, honoring `$XDG_DATA_HOME` and falling back to `$HOME/.local/share`.
///
/// Not a cross-platform data-dir crate: that would honor `$XDG_DATA_HOME` on
/// Linux only and return `~/Library/Application Support` on macOS, so otto's
/// logs landed somewhere its own `--help` never mentions.
pub fn xdg_data_dir() -> Option<PathBuf> {
    if let Ok(dir) = std::env::var("XDG_DATA_HOME") {
        let path = PathBuf::from(dir);
        if path.is_absolute() {
            return Some(path);
        }
    }
    std::env::home_dir().map(|h| h.join(".local").join("share"))
}

/// Expands a leading `~` (bare, or followed by `/...`) to the current user's
/// home directory. Any other path, including `~user` (someone else's home),
/// passes through unchanged: otto never resolves another user's home, so
/// supporting it would mean carrying a `/etc/passwd` lookup crate for a case
/// that never fires. A path that can't be expanded (no home directory found)
/// also passes through unchanged, deferring the failure to whatever tries to
/// use the path next.
pub fn expand_tilde(path: impl AsRef<Path>) -> PathBuf {
    let path = path.as_ref();
    match path.strip_prefix("~") {
        Ok(rest) => match std::env::home_dir() {
            Some(home) => home.join(rest),
            None => path.to_path_buf(),
        },
        Err(_) => path.to_path_buf(),
    }
}

/// Where otto writes `otto.log`: `<xdg-data>/otto/logs`.
pub fn log_dir() -> Option<PathBuf> {
    xdg_data_dir().map(|d| d.join("otto").join("logs"))
}

/// The directory name otto gives a project's run root: `<name>-<hash>`.
pub fn project_dir_name(name: &str, hash: &str) -> String {
    format!("{name}-{hash}")
}

/// The directory every run of one project lives under: `<otto_home>/<name>-<hash>`.
pub fn run_root(otto_home: &Path, name: &str, hash: &str) -> PathBuf {
    otto_home.join(project_dir_name(name, hash))
}

/// The directory one run writes into: `<otto_home>/<name>-<hash>/<timestamp>`.
pub fn run_dir(otto_home: &Path, name: &str, hash: &str, timestamp: u64) -> PathBuf {
    run_root(otto_home, name, hash).join(run_dir_name(timestamp, 0))
}

/// The directory name for the `seq`th run to start in one second: `<timestamp>`
/// for the first, `<timestamp>-<seq>` for each one after it.
///
/// The timestamp alone was the whole name, so every run starting in the same
/// second shared one directory. They overwrote each other's `tasks/<name>/`
/// output while running, raced each other creating it (`File exists (os error
/// 17)`), and - once the `UNIQUE(runs.timestamp)` constraint was dropped so the
/// rows no longer collided - cleaning any one of them deleted the directory the
/// others were still pointing at.
///
/// The unsuffixed first name is deliberate: it is what every existing run
/// directory on disk is already called, and the overwhelmingly common case is
/// one run per second.
pub fn run_dir_name(timestamp: u64, seq: u32) -> String {
    if seq == 0 { timestamp.to_string() } else { format!("{timestamp}-{seq}") }
}

/// The start time encoded in a run directory name, or `None` if the name is not
/// one otto produced.
///
/// Accepts both shapes [`run_dir_name`] emits. A cleanup path that parsed only
/// the bare timestamp would walk straight past every disambiguated directory and
/// leak it forever.
pub fn parse_run_dir_name(dir_name: &str) -> Option<u64> {
    match dir_name.split_once('-') {
        Some((timestamp, seq)) => {
            // Both halves must be numeric, or this is not a run directory.
            seq.parse::<u32>().ok()?;
            timestamp.parse().ok()
        }
        None => dir_name.parse().ok(),
    }
}

/// Split a directory name under the otto home back into `(name, hash)`, or
/// `None` if it is not a project run root.
///
/// The hash is the trailing segment and is always [`PROJECT_HASH_LEN`] lowercase
/// hex characters, which is what separates a run root from `.cache`, `otto.db`,
/// `.last_prune`, and anything else a user drops in the otto home. Project names
/// may themselves contain `-`, so the split is from the right.
pub fn parse_project_dir_name(dir_name: &str) -> Option<(&str, &str)> {
    let (name, hash) = dir_name.rsplit_once('-')?;
    if name.is_empty() || hash.len() != PROJECT_HASH_LEN {
        return None;
    }
    if !hash.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)) {
        return None;
    }
    Some((name, hash))
}

/// Total size in bytes of the files a directory owns.
///
/// Symlinks are not followed and are not counted. Every symlink under the otto
/// home points into the project's shared `.cache/`, so following them charges
/// one blob to every run that references it: the run directory is reported as
/// larger than it is, and deleting it frees a fraction of what was reported.
/// `is_dir()` and `metadata()` both follow symlinks, so the decision is made
/// from the entry's `file_type()` instead.
///
/// `Clean` and `Workspace` each carried a copy of this and disagreed on exactly
/// that point, so `otto Clean` and the size recorded at run completion were two
/// different numbers for the same directory.
pub fn directory_size(path: &Path) -> Result<u64> {
    let mut total = 0u64;

    if path.is_dir() {
        for entry in std::fs::read_dir(path)? {
            let entry = entry?;
            let file_type = entry.file_type()?;

            if file_type.is_symlink() {
                continue;
            }

            if file_type.is_dir() {
                total += directory_size(&entry.path())?;
            } else {
                total += entry.metadata()?.len();
            }
        }
    }

    Ok(total)
}

#[path = "layout_tests.rs"]
mod tests;
