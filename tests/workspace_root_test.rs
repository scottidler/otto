//! Regression: tasks must execute with cwd = the directory containing the
//! ottofile, not the directory otto was invoked from.
//!
//! Previously, running `otto <task>` from a subdirectory of the project
//! resolved relative paths in task bodies against the invocation cwd, so a
//! task that ran `cargo install --path borg` from `<root>/borg` looked for
//! `<root>/borg/borg/Cargo.toml` and failed.

use assert_cmd::cargo::cargo_bin_cmd;
use std::fs;
use std::io::Write;
use tempfile::tempdir;

#[test]
fn task_cwd_anchors_to_ottofile_parent_when_invoked_from_subdir() {
    let temp = tempdir().unwrap();
    let root = temp.path();

    let ottofile = root.join(".otto.yml");
    let mut f = fs::File::create(&ottofile).unwrap();
    writeln!(
        f,
        r#"
otto:
  api: 1
  tasks: [where]
tasks:
  where:
    help: Record the working directory at execution time
    bash: |
      pwd > sentinel.txt
"#
    )
    .unwrap();

    let subdir = root.join("borg");
    fs::create_dir(&subdir).unwrap();

    // Isolate ~/.otto writes so concurrent test runs don't collide.
    let otto_home = tempdir().unwrap();

    let mut cmd = cargo_bin_cmd!("otto");
    cmd.current_dir(&subdir).env("OTTO_HOME", otto_home.path()).arg("where");

    let assert = cmd.assert().success();
    let _ = assert; // surface output if the assertion above fails

    // The sentinel must land at <root>/sentinel.txt - proving the task ran
    // with cwd = root, not <root>/borg.
    let root_sentinel = root.join("sentinel.txt");
    let subdir_sentinel = subdir.join("sentinel.txt");
    assert!(
        root_sentinel.exists(),
        "expected sentinel at {}, but it was not created there",
        root_sentinel.display()
    );
    assert!(
        !subdir_sentinel.exists(),
        "sentinel leaked into invocation dir at {} - task ran with the wrong cwd",
        subdir_sentinel.display()
    );

    let recorded = fs::read_to_string(&root_sentinel).unwrap();
    let recorded_path = std::path::PathBuf::from(recorded.trim());
    let expected = fs::canonicalize(root).unwrap();
    let actual = fs::canonicalize(&recorded_path).unwrap();
    assert_eq!(
        actual,
        expected,
        "task pwd was {}, expected {}",
        actual.display(),
        expected.display()
    );
}
