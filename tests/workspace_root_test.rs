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

/// The project directory reached through a symlink: the task must still see
/// cwd as the project root, whether that root is reported through the
/// symlink path or its canonical target (both are correct; what would be
/// wrong is landing in the invocation directory, or in the symlink's parent).
#[test]
fn task_cwd_anchors_correctly_when_the_project_dir_is_reached_through_a_symlink() {
    let temp = tempdir().unwrap();
    let real_root = temp.path().join("real-project");
    fs::create_dir(&real_root).unwrap();

    let ottofile = real_root.join(".otto.yml");
    let mut f = fs::File::create(&ottofile).unwrap();
    writeln!(
        f,
        r#"
otto:
  api: 1
  tasks: [where]
tasks:
  where:
    bash: |
      pwd > sentinel.txt
"#
    )
    .unwrap();

    let link_root = temp.path().join("linked-project");
    #[cfg(unix)]
    std::os::unix::fs::symlink(&real_root, &link_root).unwrap();

    let subdir = link_root.join("borg");
    fs::create_dir(&subdir).unwrap();

    let otto_home = tempdir().unwrap();
    let mut cmd = cargo_bin_cmd!("otto");
    cmd.current_dir(&subdir).env("OTTO_HOME", otto_home.path()).arg("where");
    cmd.assert().success();

    // The sentinel lands somewhere under the project root (through the link
    // or its canonical target - both are the same directory); it must not
    // leak into the subdirectory otto was invoked from.
    assert!(
        !subdir.join("sentinel.txt").exists(),
        "sentinel leaked into the invocation dir; task ran with the wrong cwd"
    );
    let recorded = fs::read_to_string(real_root.join("sentinel.txt"))
        .expect("sentinel must land in the project root, reached through the symlink or its target");
    let recorded_path = std::path::PathBuf::from(recorded.trim());
    assert_eq!(
        fs::canonicalize(&recorded_path).unwrap(),
        fs::canonicalize(&real_root).unwrap(),
        "task pwd must resolve to the project root"
    );
}

/// A project directory that looks like a git worktree (`.git` is a *file*
/// naming the real gitdir elsewhere, not a directory) rather than a normal
/// clone. Otto has no git-aware logic at all, so this must behave exactly
/// like any other project directory; the only way that could break is if
/// directory-walking code assumed `.git` is always a directory.
#[test]
fn task_cwd_anchors_correctly_in_a_directory_that_looks_like_a_git_worktree() {
    let temp = tempdir().unwrap();
    let root = temp.path();

    fs::write(root.join(".git"), "gitdir: /elsewhere/.git/worktrees/branch\n").unwrap();

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
    bash: |
      pwd > sentinel.txt
"#
    )
    .unwrap();

    let otto_home = tempdir().unwrap();
    let mut cmd = cargo_bin_cmd!("otto");
    cmd.current_dir(root).env("OTTO_HOME", otto_home.path()).arg("where");
    cmd.assert().success();

    let recorded = fs::read_to_string(root.join("sentinel.txt")).expect("sentinel must be written");
    assert_eq!(
        fs::canonicalize(std::path::PathBuf::from(recorded.trim())).unwrap(),
        fs::canonicalize(root).unwrap(),
    );
}
