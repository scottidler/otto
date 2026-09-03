//! Builtin routing: what happens when a builtin name meets a user task.
//!
//! `docs/design/2026-09-02-second-code-review-remediation.md`, Phase 5. Every
//! test here reproduces an invocation that used to exit 0 having done less than
//! it was asked, or having done something else entirely:
//!
//! - `otto build Clean` ran `Clean` and dropped `build`.
//! - a task named `Clean` was replaced by the builtin at injection.
//! - `otto build -t` died on the declared short of a global flag.
//! - an unknown default task warned and exited 0.
//! - a task declaring `-h|--host` could never be given a host.

mod common;

use common::otto_cmd;
use predicates::prelude::*;
use serial_test::serial;
use std::fs;
use tempfile::TempDir;

/// A temp dir holding `otto.yml` with the given body, plus its otto home.
fn fixture(body: &str) -> TempDir {
    let temp = TempDir::new().expect("tempdir");
    fs::write(temp.path().join("otto.yml"), body).expect("write ottofile");
    temp
}

fn otto_home(temp: &TempDir) -> std::path::PathBuf {
    temp.path().join(".otto")
}

const BUILD_ONLY: &str = r#"
otto:
  api: 1
tasks:
  build:
    action: echo BUILD-RAN
"#;

#[test]
#[serial]
fn a_builtin_named_with_a_user_task_fails_naming_both() {
    let temp = fixture(BUILD_ONLY);
    otto_cmd(&otto_home(&temp))
        .current_dir(temp.path())
        .args(["build", "Clean", "--dry-run"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("'build'").and(predicate::str::contains("'Clean'")))
        // The builtin must not have run on the way to the error.
        .stdout(predicate::str::contains("Querying database").not());
}

#[test]
#[serial]
fn a_lone_builtin_still_runs() {
    // The guard rejects a mixed list, not a builtin: `otto Clean` is the whole
    // point of a builtin having a name.
    let temp = fixture(BUILD_ONLY);
    otto_cmd(&otto_home(&temp))
        .current_dir(temp.path())
        .args(["Clean", "--dry-run"])
        .assert()
        .success();
}

#[test]
#[serial]
fn a_task_named_like_a_builtin_fails_at_load() {
    let temp = fixture(
        r#"
otto:
  api: 1
tasks:
  Clean:
    action: echo USER-CLEAN-RAN
"#,
    );
    // Not `otto Clean`, which `main` routes to the builtin's own clap parser
    // before an ottofile is read: `--tasks` is the cheapest surface that loads
    // the file and nothing else, so it proves the rejection is at load time.
    otto_cmd(&otto_home(&temp))
        .current_dir(temp.path())
        .arg("--tasks")
        .assert()
        .failure()
        .stderr(predicate::str::contains("reserved builtin command name"));
}

#[test]
#[serial]
fn the_tui_short_flag_reaches_the_tui() {
    // `-t` is `--tui`'s declared short. Without a tty the TUI cannot start, so
    // the one thing this must never say again is "unexpected argument '-t'".
    let temp = fixture(BUILD_ONLY);
    let output = otto_cmd(&otto_home(&temp))
        .current_dir(temp.path())
        .args(["build", "-t"])
        .output()
        .expect("run otto");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        !stderr.contains("unexpected argument"),
        "-t must not reach the task's arg parser, got: {stderr}"
    );
}

#[test]
#[serial]
fn an_unknown_default_task_fails_with_a_suggestion() {
    let temp = fixture(
        r#"
otto:
  api: 1
  tasks: [bild]
tasks:
  build:
    action: echo BUILD-RAN
"#,
    );
    otto_cmd(&otto_home(&temp))
        .current_dir(temp.path())
        .assert()
        .failure()
        .stderr(predicate::str::contains("bild").and(predicate::str::contains("build")));
}

#[test]
#[serial]
fn a_task_can_declare_the_h_short_for_itself() {
    let temp = fixture(
        r#"
otto:
  api: 1
tasks:
  build:
    params:
      -h|--host:
        help: target host
    action: echo "HOST=$host"
"#,
    );
    otto_cmd(&otto_home(&temp))
        .current_dir(temp.path())
        .args(["build", "-h", "example.com"])
        .assert()
        .success()
        .stdout(predicate::str::contains("HOST=example.com"));
}

#[test]
#[serial]
fn a_task_declaring_h_still_gets_help_from_the_long_flag() {
    let temp = fixture(
        r#"
otto:
  api: 1
tasks:
  build:
    params:
      -h|--host:
        help: target host
    action: echo "HOST=$host"
"#,
    );
    otto_cmd(&otto_home(&temp))
        .current_dir(temp.path())
        .args(["build", "--help"])
        .assert()
        .success()
        .stdout(predicate::str::contains("--host"));
}

#[test]
#[serial]
fn an_ottofile_with_no_user_tasks_prints_help() {
    // `tasks: ["*"]` expands to nothing, and the six injected builtins used to
    // make `tasks.is_empty()` false, so this printed "No tasks to execute".
    let temp = fixture(
        r#"
otto:
  api: 1
  tasks: ["*"]
tasks: {}
"#,
    );
    otto_cmd(&otto_home(&temp))
        .current_dir(temp.path())
        .assert()
        .success()
        .stdout(predicate::str::contains("Usage").and(predicate::str::contains("No tasks to execute").not()))
        // The ottofile exists, so the not-found epilogue must not appear.
        .stdout(predicate::str::contains("No ottofile found").not());
}
