#![cfg(test)]

use super::*;
use tempfile::TempDir;

#[test]
fn a_directory_with_no_lock_file_is_takeable() -> Result<()> {
    let dir = TempDir::new()?;

    let taken = try_take(dir.path())?.expect("a directory with no lock file is not live");

    assert!(
        !taken.is_held(),
        "there is nothing to hold: every run directory created before the lock existed looks like this"
    );
    assert!(
        !dir.path().join(RUN_LOCK_NAME).exists(),
        "testing the lock must not create it, or --dry-run would write into a directory it only reported on"
    );
    Ok(())
}

#[test]
fn a_held_lock_makes_the_directory_untakeable() -> Result<()> {
    let dir = TempDir::new()?;

    let run = hold(dir.path())?;
    assert!(run.is_held());

    assert!(
        try_take(dir.path())?.is_none(),
        "a run holding its own lock must read as live"
    );

    drop(run);
    assert!(
        try_take(dir.path())?.is_some(),
        "the lock releases when the handle drops, so a finished run leaves nothing unreclaimable"
    );
    Ok(())
}

#[test]
fn a_lock_file_that_is_not_a_file_fails_closed() -> Result<()> {
    let dir = TempDir::new()?;
    // A directory where the lock file should be cannot be opened read-write, so
    // this stands in for the errors that are neither "absent" nor "held".
    std::fs::create_dir(dir.path().join(RUN_LOCK_NAME))?;

    let failure = try_take(dir.path()).expect_err("an untestable lock must not read as deletable");

    assert!(
        failure.to_string().contains("cannot open the run lock"),
        "the failure has to name what could not be done, got: {failure}"
    );
    Ok(())
}

#[test]
fn holding_a_lock_twice_is_reported_rather_than_blocking() -> Result<()> {
    let dir = TempDir::new()?;

    let _first = hold(dir.path())?;
    let failure = hold(dir.path()).expect_err("the second acquisition cannot succeed");

    assert!(
        failure.to_string().contains("already holds the run lock"),
        "got: {failure}"
    );
    Ok(())
}

#[test]
fn holding_fails_when_the_run_directory_is_missing() {
    let dir = TempDir::new().expect("temp dir");
    let missing = dir.path().join("never-created");

    let failure = hold(&missing).expect_err("a run that cannot take its lock aborts rather than running unprotected");

    assert!(
        failure.to_string().contains("cannot open the run lock"),
        "got: {failure}"
    );
}

#[test]
fn an_unheld_handle_holds_nothing() {
    assert!(!RunLock::unheld().is_held());
}
