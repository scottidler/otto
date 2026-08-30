//! End-to-end coverage for `$(...)` boundary finding in `envs:`.
//!
//! The old regex stopped at the first `)`, so a nested substitution or a `)` inside quotes
//! ran a truncated command, failed, and took the whole env map down with it. These fixtures
//! pin the shapes an ottofile can now write, and the loud error for the one shape it can't.

use assert_cmd::Command;
use predicates::prelude::PredicateBooleanExt;
use std::path::Path;
use tempfile::TempDir;

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

const FIXTURE: &str = r#"
otto:
  envs:
    NESTED: '$(echo "$(basename /a/b)")'
    PARENQ: '$(echo ")")'
    SIBLING: 'plain-value'
tasks:
  show:
    bash: |
      echo "NESTED=[${NESTED:-UNSET}]"
      echo "PARENQ=[${PARENQ:-UNSET}]"
      echo "SIBLING=[${SIBLING:-UNSET}]"
"#;

/// A nested `$()` resolves through to the inner command's output, and a `)` inside double
/// quotes is data rather than the end of the substitution. The sibling key proves one
/// awkward value no longer takes the whole map with it.
#[test]
fn test_nested_and_quoted_substitutions_resolve() {
    let temp = TempDir::new().unwrap();
    let ottofile = write_ottofile(temp.path(), FIXTURE);

    otto_cmd(temp.path(), &ottofile)
        .arg("show")
        .assert()
        .success()
        .stdout(predicates::str::contains("NESTED=[b]"))
        .stdout(predicates::str::contains("PARENQ=[)]"))
        .stdout(predicates::str::contains("SIBLING=[plain-value]"));
}

/// A `)` inside single quotes, a backslash-escaped `)`, and a subshell group all survive to
/// sh intact.
#[test]
fn test_quoting_and_escaping_edge_cases_resolve() {
    let temp = TempDir::new().unwrap();
    let ottofile = write_ottofile(
        temp.path(),
        r#"
otto:
  envs:
    SINGLE: "$(echo 'a)b')"
    ESCAPED: '$(echo \))'
    GROUP: '$( (echo grouped) )'
tasks:
  show:
    bash: |
      echo "SINGLE=[${SINGLE:-UNSET}]"
      echo "ESCAPED=[${ESCAPED:-UNSET}]"
      echo "GROUP=[${GROUP:-UNSET}]"
"#,
    );

    otto_cmd(temp.path(), &ottofile)
        .arg("show")
        .assert()
        .success()
        .stdout(predicates::str::contains("SINGLE=[a)b]"))
        .stdout(predicates::str::contains("ESCAPED=[)]"))
        .stdout(predicates::str::contains("GROUP=[grouped]"));
}

/// An unmatched `$(` is reported by key and by value, never passed through as a
/// literal - and it now stops the run. It used to be a warning: the globals were
/// dropped and the task ran anyway with an environment nobody configured
/// (`BROKEN=[UNSET]`, exit 0). See 2026-06-10-code-review-remediation.md Phase 3.
#[test]
fn test_unmatched_substitution_is_reported_with_key_and_value() {
    let temp = TempDir::new().unwrap();
    let ottofile = write_ottofile(
        temp.path(),
        r#"
otto:
  envs:
    BROKEN: '$(echo hello'
tasks:
  show:
    bash: echo "BROKEN=[${BROKEN:-UNSET}]"
"#,
    );

    otto_cmd(temp.path(), &ottofile)
        .arg("show")
        .assert()
        .failure()
        .stderr(predicates::str::contains("BROKEN"))
        .stderr(predicates::str::contains("unmatched '$('"))
        .stderr(predicates::str::contains("$(echo hello"))
        .stdout(predicates::str::contains("BROKEN=").not());
}

/// Task-scoped envs go through the same scanner as global ones.
#[test]
fn test_task_scoped_env_nested_substitution_resolves() {
    let temp = TempDir::new().unwrap();
    let ottofile = write_ottofile(
        temp.path(),
        r#"
tasks:
  show:
    envs:
      NESTED: '$(echo "$(basename /a/b)")'
    bash: echo "NESTED=[${NESTED:-UNSET}]"
"#,
    );

    otto_cmd(temp.path(), &ottofile)
        .arg("show")
        .assert()
        .success()
        .stdout(predicates::str::contains("NESTED=[b]"));
}
