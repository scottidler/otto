//! Pins the shared home-isolation helper (`tests/common/mod.rs`) that every
//! other integration test relies on to avoid writing into the developer's
//! real `~/.otto/otto.db` (`docs/design/2026-06-10-code-review-remediation.md`,
//! Phase 11). If a future edit to `common::otto_cmd` stops pinning
//! `OTTO_HOME` or stops removing `OTTO_DB_PATH`, this test turns that into a
//! loud, named failure instead of a silent DB leak.

mod common;

use common::otto_cmd;
use std::fs;
use tempfile::TempDir;

const FIXTURE: &str = "otto:\n  api: 1\ntasks:\n  hello:\n    action: echo hi\n";

#[test]
fn otto_cmd_creates_the_database_under_the_isolated_home() {
    let home = TempDir::new().unwrap();
    let ottofile = home.path().join("otto.yml");
    fs::write(&ottofile, FIXTURE).unwrap();

    otto_cmd(home.path())
        .arg("-o")
        .arg(&ottofile)
        .arg("hello")
        .assert()
        .success();

    assert!(
        home.path().join("otto.db").exists(),
        "a run through the shared helper must create otto.db under the isolated OTTO_HOME, \
         not the developer's real ~/.otto"
    );
}

/// The stronger claim: even when the ambient environment already exports
/// `OTTO_DB_PATH` (exactly what a developer's real shell might have), the
/// helper's `env_remove` wins and the child never sees it.
#[test]
#[serial_test::serial]
fn otto_cmd_ignores_an_inherited_otto_db_path() {
    // SAFETY: the only env-mutating test in this binary; `#[serial]` protects
    // it against itself if the harness ever gains a second such test here.
    let decoy = TempDir::new().unwrap();
    let decoy_db = decoy.path().join("shared.db");
    unsafe { std::env::set_var("OTTO_DB_PATH", &decoy_db) };

    let home = TempDir::new().unwrap();
    let ottofile = home.path().join("otto.yml");
    fs::write(&ottofile, FIXTURE).unwrap();

    otto_cmd(home.path())
        .arg("-o")
        .arg(&ottofile)
        .arg("hello")
        .assert()
        .success();

    unsafe { std::env::remove_var("OTTO_DB_PATH") };

    assert!(
        home.path().join("otto.db").exists(),
        "OTTO_HOME must win even with OTTO_DB_PATH inherited from the parent process"
    );
    assert!(!decoy_db.exists(), "the inherited OTTO_DB_PATH must never be touched");
}
