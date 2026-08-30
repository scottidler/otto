//! Phase 8 (`docs/design/2026-06-10-code-review-remediation.md`): the CLI
//! surface, exercised through the real binary.
//!
//! These pin the behaviours that only show up end to end: which ottofile
//! `--help` reads, how an argument that spells a task name is partitioned,
//! and whether a builtin behind a global flag still routes.

use assert_cmd::cargo::cargo_bin_cmd;
use predicates::prelude::*;
use std::fs;
use std::path::Path;
use tempfile::{TempDir, tempdir};

/// A workspace with `otto.yml` (tasks: build/test/fmt) and `sub/other.yml`
/// (task: ZZZuniquetask), so "which file did help read" has an unambiguous answer.
fn fixture() -> TempDir {
    let temp = tempdir().expect("tempdir");
    write(
        &temp.path().join("otto.yml"),
        r#"
otto:
  api: 1
  tasks: [build]
tasks:
  build:
    help: Build the thing
    params:
      -m|--msg:
        help: A message
        default: none
    bash: |
      echo "build msg=${msg}"
  test:
    help: Test the thing
    bash: |
      echo "TEST"
  fmt:
    help: Format
    params:
      --format:
        choices: [ascii, unicode]
        default: ascii
        help: Output format
    bash: |
      echo "fmt format=${format}"
"#,
    );
    fs::create_dir_all(temp.path().join("sub")).expect("sub dir");
    write(
        &temp.path().join("sub").join("other.yml"),
        r#"
otto:
  api: 1
  tasks: [ZZZuniquetask]
tasks:
  ZZZuniquetask:
    help: Only in other.yml
    bash: |
      echo other
"#,
    );
    temp
}

fn write(path: &Path, content: &str) {
    fs::write(path, content).expect("write fixture");
}

#[test]
fn help_reads_the_ottofile_the_flag_names() {
    let temp = fixture();
    let mut cmd = cargo_bin_cmd!("otto");
    cmd.current_dir(temp.path()).args(["-o", "sub/other.yml", "--help"]);

    cmd.assert()
        .success()
        .stdout(predicate::str::contains("ZZZuniquetask"))
        .stdout(predicate::str::contains("build").not());
}

#[test]
fn help_reads_the_ottofile_the_env_names() {
    let temp = fixture();
    let mut cmd = cargo_bin_cmd!("otto");
    cmd.current_dir(temp.path())
        .env("OTTOFILE", "sub/other.yml")
        .arg("--help");

    cmd.assert()
        .success()
        .stdout(predicate::str::contains("ZZZuniquetask"))
        .stdout(predicate::str::contains("build").not());
}

#[test]
fn a_flag_value_that_spells_a_task_reaches_the_task() {
    let temp = fixture();
    let mut cmd = cargo_bin_cmd!("otto");
    cmd.current_dir(temp.path())
        .env("OTTO_HOME", temp.path().join("otto-home"))
        .args(["build", "--msg", "test"]);

    cmd.assert()
        .success()
        .stdout(predicate::str::contains("build msg=test"));
}

#[test]
fn a_double_dash_hands_the_rest_to_the_task() {
    let temp = fixture();
    let mut cmd = cargo_bin_cmd!("otto");
    cmd.current_dir(temp.path())
        .env("OTTO_HOME", temp.path().join("otto-home"))
        .args(["build", "--", "--msg=x"]);

    cmd.assert().success().stdout(predicate::str::contains("build msg=x"));
}

#[test]
fn a_choice_is_accepted_in_any_case_and_stored_canonically() {
    let temp = fixture();
    let mut cmd = cargo_bin_cmd!("otto");
    cmd.current_dir(temp.path())
        .env("OTTO_HOME", temp.path().join("otto-home"))
        .args(["fmt", "--format", "ASCII"]);

    cmd.assert()
        .success()
        .stdout(predicate::str::contains("fmt format=ascii"));
}

#[test]
fn a_task_requested_twice_is_an_error() {
    let temp = fixture();
    let mut cmd = cargo_bin_cmd!("otto");
    cmd.current_dir(temp.path())
        .env("OTTO_HOME", temp.path().join("otto-home"))
        .args(["build", "--msg=a", "build", "--msg=b"]);

    cmd.assert()
        .failure()
        .stderr(predicate::str::contains("was requested more than once"));
}

#[test]
fn an_unknown_task_flag_fails_without_panicking() {
    let temp = fixture();
    let mut cmd = cargo_bin_cmd!("otto");
    cmd.current_dir(temp.path())
        .env("OTTO_HOME", temp.path().join("otto-home"))
        .args(["build", "--bogus"]);

    cmd.assert()
        .failure()
        .stderr(predicate::str::contains("unexpected argument"))
        .stderr(predicate::str::contains("panicked").not());
}

#[test]
fn a_builtin_routes_from_behind_a_global_flag() {
    let temp = fixture();
    write(&temp.path().join("Makefile"), "all:\n\techo hi\n");

    let mut cmd = cargo_bin_cmd!("otto");
    cmd.current_dir(temp.path())
        .env("OTTO_HOME", temp.path().join("otto-home"))
        .args(["-j", "2", "Convert"])
        .write_stdin("all:\n\techo hi\n");

    cmd.assert().success().stdout(predicate::str::contains("bash: echo hi"));
}

#[test]
fn a_builtin_routes_from_behind_the_tui_flag() {
    // The TUI path dispatches the same builtin table as the terminal path, so
    // this converts rather than printing "No tasks to execute".
    let temp = fixture();
    let mut cmd = cargo_bin_cmd!("otto");
    cmd.current_dir(temp.path())
        .env("OTTO_HOME", temp.path().join("otto-home"))
        .args(["--tui", "Convert"])
        .write_stdin("all:\n\techo hi\n");

    cmd.assert()
        .success()
        .stdout(predicate::str::contains("bash: echo hi"))
        .stdout(predicate::str::contains("No tasks to execute").not());
}

#[test]
fn an_invalid_history_status_is_rejected() {
    let temp = fixture();
    let mut cmd = cargo_bin_cmd!("otto");
    cmd.current_dir(temp.path())
        .env("OTTO_HOME", temp.path().join("otto-home"))
        .args(["History", "--status", "bogus"]);

    cmd.assert()
        .failure()
        .stderr(predicate::str::contains("invalid value 'bogus'"));
}

#[test]
fn a_missing_ottofile_names_the_path_it_looked_for() {
    let temp = fixture();
    let mut cmd = cargo_bin_cmd!("otto");
    cmd.current_dir(temp.path()).args(["-o", "/nope/nothere.yml", "build"]);

    cmd.assert()
        .failure()
        .stderr(predicate::str::contains("/nope/nothere.yml"));
}

#[test]
fn the_tui_flag_is_accepted_after_a_task_name() {
    // `--tui` is global, and clap does not push global flags into external
    // subcommands, so this used to fail with "unexpected argument '--tui'".
    // Not a tty under test, so the TUI falls back to terminal output.
    let temp = fixture();
    let mut cmd = cargo_bin_cmd!("otto");
    cmd.current_dir(temp.path())
        .env("OTTO_HOME", temp.path().join("otto-home"))
        .args(["test", "--tui"]);

    cmd.assert()
        .success()
        .stderr(predicate::str::contains("--tui requires a TTY"));
}

#[test]
fn an_unknown_log_level_is_rejected() {
    let temp = fixture();
    let mut cmd = cargo_bin_cmd!("otto");
    cmd.current_dir(temp.path())
        .env("OTTO_HOME", temp.path().join("otto-home"))
        .args(["--log-level", "bogus", "test"]);

    cmd.assert()
        .failure()
        .stderr(predicate::str::contains("invalid --log-level 'bogus'"));
}

#[test]
fn no_prefix_drops_the_prefix_from_status_lines_too() {
    let temp = fixture();
    let mut cmd = cargo_bin_cmd!("otto");
    cmd.current_dir(temp.path())
        .env("OTTO_HOME", temp.path().join("otto-home"))
        .args(["--no-prefix", "test"]);

    cmd.assert()
        .success()
        .stdout(predicate::str::contains("test finished successfully"))
        .stdout(predicate::str::contains("[test]").not());
}
