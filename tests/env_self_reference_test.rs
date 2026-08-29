//! End-to-end coverage for an env reading its own inherited value.
//!
//! The idiom `MYVAR: '$(echo "${MYVAR:-fallback}")'` (declare a default, let the invoking
//! shell override it) has to work under the variable's own name, in both the global `otto:`
//! block and a task's `envs:`.

use assert_cmd::Command;
use std::path::Path;
use tempfile::TempDir;

const SELF_REF: &str = r#"$(echo "${MYVAR:-fallback}")"#;

#[allow(deprecated)]
fn otto_cmd(work_dir: &Path, ottofile: &Path) -> Command {
    let mut cmd = Command::cargo_bin("otto").expect("Failed to find otto binary");
    cmd.current_dir(work_dir);
    cmd.arg("--ottofile").arg(ottofile);
    // Keep history/state inside the fixture instead of the developer's ~/.otto
    cmd.env("OTTO_HOME", work_dir.join(".otto"));
    cmd.env("OTTO_DB_PATH", work_dir.join("otto.db"));
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

/// Circular self-definition stays loud: a warning naming the unresolvable variable, and a
/// non-zero exit because the task body then hits an unbound variable.
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
            && stderr.contains("Failed to resolve environment variable")
            && stderr.contains("not found"),
        "expected a loud circular-reference warning, got stderr:\n{stderr}"
    );
    assert_ne!(output.status.code(), Some(0), "circular env definition must not exit 0");
}
