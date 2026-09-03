//! A consumer of a foreach subtask's output.
//!
//! A subtask is named `<parent>:<item>` (`src/naming.rs`), and a variable name
//! is `OTTO_INPUT_<TASK>_<KEY>` with the task name folded into the alphabet a
//! shell identifier is made of. That fold enumerated `-` and `.` and nothing
//! else, so the `:` survived it, and otto wrote the line
//! `OTTO_INPUT_UP:ALPHA_K='v-alpha'` into the consumer's input file. bash read
//! it as a command:
//!
//! ```text
//! input.up:alpha.env: line 4: OTTO_INPUT_UP:ALPHA_K=v-alpha: command not found
//! ```
//!
//! The consumer then failed. See
//! `docs/design/2026-09-02-second-code-review-remediation.md`, Phase 2.

mod common;

use std::fs;
use std::path::{Path, PathBuf};

use tempfile::TempDir;

/// Run the real binary in `dir` with an isolated `OTTO_HOME`, returning
/// (exit code, stdout, stderr).
fn run_otto(dir: &Path, otto_home: &Path, args: &[&str]) -> (i32, String, String) {
    let output = common::otto_std_cmd(otto_home)
        .current_dir(dir)
        .env_remove("OTTOFILE")
        .args(args)
        .output()
        .expect("failed to run otto");

    (
        output.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&output.stdout).to_string(),
        String::from_utf8_lossy(&output.stderr).to_string(),
    )
}

/// A project directory with `otto.yml` written from `body`, plus its own
/// `OTTO_HOME` so runs never touch the developer's real one.
fn project(body: &str) -> (TempDir, PathBuf) {
    let dir = TempDir::new().expect("tempdir");
    fs::write(dir.path().join("otto.yml"), body).expect("write ottofile");
    let otto_home = dir.path().join("otto-home");
    fs::create_dir_all(&otto_home).expect("create otto home");
    (dir, otto_home)
}

const SUBTASK_PRODUCER: &str = r#"
otto:
  api: 1
  tasks: [use]
tasks:
  up:
    foreach:
      items: [alpha]
      as: item
    output: [k]
    bash: otto_set_output k "v-${item}"
  use:
    before: ["up:alpha"]
    input: ["up:alpha.k"]
    bash: echo "got=[$(otto_get_input up:alpha.k)]"
"#;

#[test]
fn a_consumer_reads_a_foreach_subtasks_output() {
    let (dir, home) = project(SUBTASK_PRODUCER);
    let (code, stdout, stderr) = run_otto(dir.path(), &home, &["use"]);

    assert_eq!(code, 0, "stdout:\n{stdout}\nstderr:\n{stderr}");
    assert!(
        stdout.contains("got=[v-alpha]"),
        "the subtask's value must reach its consumer:\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    // The failure mode was audible but not fatal-looking, so pin the noise too:
    // a `command not found` from the input file means the fold let a byte
    // through that a shell assignment cannot hold.
    assert!(
        !stderr.contains("command not found"),
        "the input file must be sourceable, not executable text:\n{stderr}"
    );
}

/// The written file itself, not just the consumer's reading of it: every
/// assignment in an input `.env` has an identifier on its left.
#[test]
fn the_generated_input_file_assigns_only_to_identifiers() {
    let (dir, home) = project(SUBTASK_PRODUCER);
    let (code, stdout, stderr) = run_otto(dir.path(), &home, &["use"]);
    assert_eq!(code, 0, "stdout:\n{stdout}\nstderr:\n{stderr}");

    let input_file = find_file(&home, "input.up:alpha.env")
        .unwrap_or_else(|| panic!("no input.up:alpha.env under {}", home.display()));
    let contents = fs::read_to_string(&input_file).expect("read input env");

    let mut assignments = 0;
    for line in contents.lines() {
        if line.trim().is_empty() || line.starts_with('#') {
            continue;
        }
        let Some((name, _)) = line.split_once('=') else {
            continue;
        };
        assignments += 1;
        assert!(
            name.chars().next().is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
                && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_'),
            "'{name}' is not a shell identifier, so bash runs the line instead of \
             assigning it:\n{contents}"
        );
    }
    assert!(
        assignments >= 3,
        "expected the value plus its two companions:\n{contents}"
    );
    assert!(
        contents.contains("OTTO_INPUT_UP_ALPHA_K='v-alpha'"),
        "the `:` folds to `_` like every other byte outside [A-Za-z0-9_]:\n{contents}"
    );
}

/// Depth-first search for a file by name; the run directory is timestamped, so
/// the path cannot be spelled out.
fn find_file(root: &Path, name: &str) -> Option<PathBuf> {
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else if path.file_name().is_some_and(|f| f == name) {
                return Some(path);
            }
        }
    }
    None
}
