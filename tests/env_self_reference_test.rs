//! End-to-end coverage for an env reading its own inherited value.
//!
//! The idiom `MYVAR: '$(echo "${MYVAR:-fallback}")'` (declare a default, let the invoking
//! shell override it) has to work under the variable's own name, in both the global `otto:`
//! block and a task's `envs:`.

mod common;

use assert_cmd::Command;
use std::path::Path;
use tempfile::TempDir;

const SELF_REF: &str = r#"$(echo "${MYVAR:-fallback}")"#;

/// The fixture's own otto, isolated through `common::otto_cmd` so history and
/// state land in the fixture instead of the developer's `~/.otto`.
fn otto_cmd(work_dir: &Path, ottofile: &Path) -> Command {
    let mut cmd = common::otto_cmd(&work_dir.join(".otto"));
    cmd.current_dir(work_dir);
    cmd.arg("--ottofile").arg(ottofile);
    cmd
}

fn write_ottofile(dir: &Path, body: &str) -> std::path::PathBuf {
    let ottofile = dir.join("otto.yml");
    std::fs::write(&ottofile, body).expect("failed to write ottofile");
    ottofile
}

fn global_env_fixture() -> String {
    format!(
        r#"
otto:
  envs:
    MYVAR: '{SELF_REF}'
tasks:
  show:
    bash: echo "MYVAR=[${{MYVAR}}]"
"#
    )
}

fn task_env_fixture() -> String {
    format!(
        r#"
tasks:
  show:
    envs:
      MYVAR: '{SELF_REF}'
    bash: echo "MYVAR=[${{MYVAR}}]"
"#
    )
}

/// Global scope, value inherited from the invoking shell.
#[test]
fn test_global_env_self_reference_reads_inherited_value() {
    let temp = TempDir::new().unwrap();
    let ottofile = write_ottofile(temp.path(), &global_env_fixture());

    let output = otto_cmd(temp.path(), &ottofile)
        .env("MYVAR", "from-shell")
        .arg("show")
        .output()
        .unwrap();

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("MYVAR=[from-shell]"),
        "expected the inherited value, got stdout:\n{stdout}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

/// Global scope, nothing inherited: the declared default still wins.
#[test]
fn test_global_env_self_reference_falls_back_when_unset() {
    let temp = TempDir::new().unwrap();
    let ottofile = write_ottofile(temp.path(), &global_env_fixture());

    let output = otto_cmd(temp.path(), &ottofile)
        .env_remove("MYVAR")
        .arg("show")
        .output()
        .unwrap();

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("MYVAR=[fallback]"),
        "expected the declared fallback, got stdout:\n{stdout}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

/// Task scope routes through the same evaluator, so the same idiom works there.
#[test]
fn test_task_env_self_reference_reads_inherited_value() {
    let temp = TempDir::new().unwrap();
    let ottofile = write_ottofile(temp.path(), &task_env_fixture());

    let output = otto_cmd(temp.path(), &ottofile)
        .env("MYVAR", "from-shell")
        .arg("show")
        .output()
        .unwrap();

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("MYVAR=[from-shell]"),
        "expected the inherited value, got stdout:\n{stdout}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

/// A cross-reference to a declared key reads the DECLARED value, not the inherited one, even
/// when both keys exist in the invoking environment.
#[test]
fn test_cross_reference_prefers_declared_value_over_inherited() {
    let temp = TempDir::new().unwrap();
    let ottofile = write_ottofile(
        temp.path(),
        r#"
otto:
  envs:
    BASE: 'declared-base'
    DERIVED: '${BASE}/child'
tasks:
  show:
    bash: echo "DERIVED=[${DERIVED}]"
"#,
    );

    let output = otto_cmd(temp.path(), &ottofile)
        .env("BASE", "inherited-base")
        .env("DERIVED", "inherited-derived")
        .arg("show")
        .output()
        .unwrap();

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("DERIVED=[declared-base/child]"),
        "expected the declared value, got stdout:\n{stdout}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

/// Circular self-definition stays loud, and now says *circular*: the message
/// used to be `Failed to resolve environment variable 'A': Environment variable
/// 'B' not found`, naming a variable that is not missing. The exit is non-zero
/// because global env evaluation now fails the run rather than warning and
/// dropping the globals. See 2026-06-10-code-review-remediation.md Phase 3.
#[test]
fn test_circular_env_definition_still_fails_loudly() {
    let temp = TempDir::new().unwrap();
    let ottofile = write_ottofile(
        temp.path(),
        r#"
otto:
  envs:
    A: '${B}'
    B: '${A}'
tasks:
  show:
    bash: echo "A=[${A}] B=[${B}]"
"#,
    );

    let output = otto_cmd(temp.path(), &ottofile)
        .env_remove("A")
        .env_remove("B")
        .arg("show")
        .output()
        .unwrap();

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("Failed to evaluate global environment variables")
            && stderr.contains("Circular dependency between environment variables")
            && stderr.contains("A")
            && stderr.contains("B"),
        "expected the cycle named as a cycle, got stderr:\n{stderr}"
    );
    assert_ne!(output.status.code(), Some(0), "circular env definition must not exit 0");
}
