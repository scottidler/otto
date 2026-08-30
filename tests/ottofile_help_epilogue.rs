mod common;

use predicates::prelude::*;
use std::fs;
use std::io::Write;
use tempfile::tempdir;

#[test]
fn test_help_epilogue_when_ottofile_missing() {
    let temp = tempdir().unwrap();
    let mut cmd = common::otto_cmd(temp.path());
    cmd.current_dir(&temp).arg("--help");
    cmd.assert()
        .failure()
        .code(2)
        .stdout(predicate::str::contains(
            "ERROR: No ottofile found in this directory or any parent directory!",
        ))
        .stdout(predicate::str::contains("Otto looks for one of the following files"))
        .stdout(predicate::str::contains("otto.yml"));
}

#[test]
fn test_help_epilogue_not_present_when_ottofile_exists() {
    let temp = tempdir().unwrap();
    let ottofile_path = temp.path().join("otto.yml");
    let mut file = fs::File::create(&ottofile_path).unwrap();
    writeln!(file, "otto:\n  api: 1\ntasks:\n  test:\n    action: echo test").unwrap();

    let mut cmd = common::otto_cmd(temp.path());
    cmd.current_dir(&temp).arg("--help");
    cmd.assert()
        .success()
        .stdout(predicate::str::contains("otto").and(predicate::str::contains("ERROR: No ottofile found").not()));
}

// =============================================================================
// Truthful config-error help fallback
// (design doc 2026-08-29-strict-ottofile-schema, Phase 3b)
//
// Two config-failure states that used to render the same epilogue: no ottofile
// anywhere up the tree, and an ottofile that exists and will not parse. The
// assertions below name the STREAM for every claim - global `--help` writes its
// help to stdout and the parse diagnostic to stderr, so a test that watched
// only stdout would pass while the operator saw nothing new.
// =============================================================================

/// An ottofile that parses as YAML and fails the typed parse: `before:` wants a
/// sequence and gets a map, so serde reports the field path plus line/column.
fn write_parse_failing_ottofile(dir: &std::path::Path) {
    let content = "otto:\n  api: 1\n\ntasks:\n  up:\n    before:\n      key: value\n    action: echo up\n";
    fs::write(dir.join(".otto.yml"), content).unwrap();
}

#[test]
fn test_help_reports_parse_error_and_does_not_claim_missing_ottofile() {
    let temp = tempdir().unwrap();
    write_parse_failing_ottofile(temp.path());

    let mut cmd = common::otto_cmd(temp.path());
    cmd.current_dir(&temp).arg("--help");
    cmd.assert()
        .failure()
        .code(2)
        // The real diagnostic, on the stream that carries it.
        .stderr(predicate::str::contains("failed to parse ottofile"))
        .stderr(predicate::str::contains(".otto.yml"))
        .stderr(predicate::str::contains(
            "tasks.up.before: invalid type: map, expected a sequence",
        ))
        // The lie is gone from the stream that used to carry it.
        .stdout(predicate::str::contains("No ottofile found").not())
        // The global flag list still renders, per the phase's scope discipline.
        .stdout(predicate::str::contains("--ottofile"));
}

#[test]
fn test_help_still_claims_missing_when_no_ottofile_anywhere() {
    let temp = tempdir().unwrap();

    let mut cmd = common::otto_cmd(temp.path());
    cmd.current_dir(&temp).arg("--help");
    cmd.assert()
        .failure()
        .code(2)
        .stdout(predicate::str::contains(
            "ERROR: No ottofile found in this directory or any parent directory!",
        ))
        // Nothing was found, so nothing can have failed to parse.
        .stderr(predicate::str::contains("failed to parse ottofile").not());
}

/// NON-REGRESSION, and labelled as such because it already passed before this
/// phase: the usertask help path shares `load_config_from_path` with the global
/// `--help` path and diverges only at the `Err` branch this phase changed.
/// Enriching, wrapping, or re-typing the error INSIDE `load_config_from_path`
/// would move this path too; this is what catches that.
#[test]
fn test_usertask_help_still_reports_parse_error_on_stderr() {
    let temp = tempdir().unwrap();
    write_parse_failing_ottofile(temp.path());

    let mut cmd = common::otto_cmd(temp.path());
    cmd.current_dir(&temp).args(["up", "--help"]);
    cmd.assert()
        .failure()
        .code(1)
        .stderr(predicate::str::contains(
            "tasks.up.before: invalid type: map, expected a sequence",
        ))
        // Unchanged: this path never rendered the global-help fallback.
        .stderr(predicate::str::contains("failed to parse ottofile").not());
}
