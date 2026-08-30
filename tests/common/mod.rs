//! Shared home-isolation helper for integration tests.
//!
//! `docs/design/2026-06-10-code-review-remediation.md`, Phase 11: a full
//! `cargo test` with neither `OTTO_HOME` nor `OTTO_DB_PATH` set used to write
//! a project row per temp-dir fixture into the developer's real
//! `~/.otto/otto.db` (1040 of 1264 rows on the live db were `.tmp*` test
//! artifacts, measured 2026-08-29). Every integration test that spawns the
//! `otto` binary or builds a `StateManager` must isolate its home through
//! this module instead of rolling its own `env(...)` calls, so the isolation
//! can't drift file by file.
//!
//! `StateManager::default_db_path()` (`src/executor/state/db.rs`) checks
//! `$OTTO_DB_PATH` first, then falls back to `resolve_otto_home()`
//! (`src/executor/layout.rs`), which checks `$OTTO_HOME` before falling back
//! to `$HOME/.otto`. Setting `HOME` alone, without also pinning `OTTO_HOME`
//! and removing `OTTO_DB_PATH`, is not isolation: a developer's shell that
//! exports either one would still steer the child at the real store.
//!
//! Not every test file that includes this module uses every item in it;
//! that's expected, since each `tests/*.rs` file is its own compiled binary.

#![allow(dead_code)]

use assert_cmd::cargo::cargo_bin_cmd;
use std::path::Path;

/// Build an `otto` command isolated to `home`.
///
/// `OTTO_HOME` is pinned to `home` and `OTTO_DB_PATH` is removed from the
/// child's environment, so the spawned binary cannot read or write the
/// developer's real database no matter what the ambient environment
/// exports. Pinned by `tests/db_isolation_test.rs`.
pub fn otto_cmd(home: &Path) -> assert_cmd::Command {
    let mut cmd = cargo_bin_cmd!("otto");
    cmd.env("OTTO_HOME", home).env_remove("OTTO_DB_PATH");
    cmd
}

/// Build a `StateManager` rooted at `home`'s `otto.db`, for tests that talk
/// to the store directly rather than through the binary.
pub fn isolated_state_manager(home: &Path) -> otto::executor::state::StateManager {
    otto::executor::state::StateManager::with_db_path(home.join("otto.db"))
        .expect("failed to open scratch StateManager")
}
