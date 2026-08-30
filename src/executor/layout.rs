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
/// Not `dirs::data_local_dir()`: that honors `$XDG_DATA_HOME` on Linux only and
/// returns `~/Library/Application Support` on macOS, so otto's logs landed
/// somewhere its own `--help` never mentions.
pub fn xdg_data_dir() -> Option<PathBuf> {
    if let Ok(dir) = std::env::var("XDG_DATA_HOME") {
        let path = PathBuf::from(dir);
        if path.is_absolute() {
            return Some(path);
        }
    }
    dirs::home_dir().map(|h| h.join(".local").join("share"))
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
    run_root(otto_home, name, hash).join(timestamp.to_string())
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

#[cfg(test)]
mod tests {
    use super::*;
    use serial_test::serial;
    use std::path::PathBuf;

    #[test]
    fn test_project_dir_name_is_name_then_hash() {
        assert_eq!(project_dir_name("otto", "6b20a2e4"), "otto-6b20a2e4");
    }

    #[test]
    fn test_run_dir_joins_home_project_timestamp() {
        let dir = run_dir(Path::new("/home/u/.otto"), "otto", "6b20a2e4", 1700000000);
        assert_eq!(dir, PathBuf::from("/home/u/.otto/otto-6b20a2e4/1700000000"));
    }

    #[test]
    fn test_parse_round_trips_what_the_workspace_builds() {
        let name = project_dir_name("my-project", "0123abcd");
        assert_eq!(parse_project_dir_name(&name), Some(("my-project", "0123abcd")));
    }

    #[test]
    fn test_parse_rejects_non_project_entries() {
        // The things that actually sit next to run roots in the otto home.
        assert_eq!(parse_project_dir_name("otto.db"), None);
        assert_eq!(parse_project_dir_name(".cache"), None);
        assert_eq!(parse_project_dir_name(".last_prune"), None);
        // A hash of the wrong length or the wrong alphabet is not a run root.
        assert_eq!(parse_project_dir_name("proj-abc123"), None);
        assert_eq!(parse_project_dir_name("proj-ABCDEF12"), None);
        assert_eq!(parse_project_dir_name("proj-6b20a2eg"), None);
        assert_eq!(parse_project_dir_name("-6b20a2e4"), None);
    }

    #[test]
    #[serial]
    fn test_resolve_otto_home_prefers_otto_home() {
        // SAFETY: serialized against every other env-mutating test in the crate.
        unsafe {
            std::env::set_var("OTTO_HOME", "/tmp/otto-home-probe");
        }
        let home = resolve_otto_home().unwrap();
        unsafe {
            std::env::remove_var("OTTO_HOME");
        }
        assert_eq!(home, PathBuf::from("/tmp/otto-home-probe"));
    }

    #[test]
    #[serial]
    fn test_resolve_otto_home_falls_back_to_dot_otto() {
        // SAFETY: serialized against every other env-mutating test in the crate.
        unsafe {
            std::env::remove_var("OTTO_HOME");
        }
        let home = resolve_otto_home().unwrap();
        let expected = PathBuf::from(std::env::var("HOME").unwrap()).join(".otto");
        assert_eq!(home, expected);
    }

    #[test]
    #[serial]
    fn xdg_data_dir_honors_the_env_and_falls_back() {
        let prior = std::env::var("XDG_DATA_HOME").ok();

        let dir = tempfile::tempdir().expect("tempdir");
        unsafe { std::env::set_var("XDG_DATA_HOME", dir.path()) };
        assert_eq!(xdg_data_dir().as_deref(), Some(dir.path()));
        assert_eq!(log_dir(), Some(dir.path().join("otto").join("logs")));

        // A relative value is not a usable data dir; fall back rather than
        // scattering logs relative to whatever the cwd happened to be.
        unsafe { std::env::set_var("XDG_DATA_HOME", "relative/path") };
        assert!(xdg_data_dir().expect("a home dir").ends_with(".local/share"));

        unsafe { std::env::remove_var("XDG_DATA_HOME") };
        assert!(xdg_data_dir().expect("a home dir").ends_with(".local/share"));

        match prior {
            Some(v) => unsafe { std::env::set_var("XDG_DATA_HOME", v) },
            None => unsafe { std::env::remove_var("XDG_DATA_HOME") },
        }
    }
}
