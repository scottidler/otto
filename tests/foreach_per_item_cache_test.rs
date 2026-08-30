//! Per-item incremental builds for `foreach`, and the fail-closed rules around
//! the paths that make them work.
//!
//! Phase 6 bullet 10's criterion is "a foreach whose per-item outputs are current
//! skips exactly those items and re-runs exactly the one whose input was
//! touched". Nothing pinned it: what existed was a unit test asserting the
//! interpolated string, which is not the skip behavior. Found by the batched
//! audit, batch 7 of 14.

mod common;

use common::otto_std_cmd;
use std::fs;
use std::path::{Path, PathBuf};

use tempfile::TempDir;

fn run(project: &Path, home: &Path, args: &[&str]) -> (i32, String, String) {
    let out = otto_std_cmd(home)
        .current_dir(project)
        .env_remove("OTTOFILE")
        .args(args)
        .output()
        .expect("failed to run otto");
    (
        out.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&out.stdout).to_string(),
        String::from_utf8_lossy(&out.stderr).to_string(),
    )
}

fn project(body: &str) -> (TempDir, PathBuf, PathBuf) {
    let dir = TempDir::new().expect("tempdir");
    let project = dir.path().join("project");
    let home = dir.path().join("otto-home");
    fs::create_dir_all(project.join("src")).expect("create src");
    fs::create_dir_all(&home).expect("create home");
    fs::write(project.join("otto.yml"), body).expect("write ottofile");
    (dir, project, home)
}

const PER_ITEM: &str = "\
otto:
  api: 1
  tasks: [compile]
tasks:
  compile:
    foreach: {items: [a, b], as: item}
    input: [\"src/${item}.txt\"]
    output: [\"out/${item}.o\"]
    action: |
      mkdir -p out
      echo built > out/${item}.o
      echo COMPILING ${item}
";

/// Touching one item's input re-runs exactly that item.
#[test]
fn a_foreach_reruns_only_the_item_whose_input_changed() {
    let (_dir, proj, home) = project(PER_ITEM);
    fs::write(proj.join("src/a.txt"), "a1").expect("write a");
    fs::write(proj.join("src/b.txt"), "b1").expect("write b");

    let (code, stdout, stderr) = run(&proj, &home, &["compile"]);
    assert_eq!(code, 0, "cold run failed\nstdout:\n{stdout}\nstderr:\n{stderr}");
    assert!(stdout.contains("COMPILING a"), "cold run must build a:\n{stdout}");
    assert!(stdout.contains("COMPILING b"), "cold run must build b:\n{stdout}");

    let (code, stdout, _) = run(&proj, &home, &["compile"]);
    assert_eq!(code, 0);
    assert!(
        !stdout.contains("COMPILING"),
        "a warm run must rebuild nothing:\n{stdout}"
    );

    // Only a's input changes.
    std::thread::sleep(std::time::Duration::from_millis(1100));
    fs::write(proj.join("src/a.txt"), "a2").expect("touch a");

    let (code, stdout, _) = run(&proj, &home, &["compile"]);
    assert_eq!(code, 0);
    assert!(stdout.contains("COMPILING a"), "a must rebuild:\n{stdout}");
    assert!(
        !stdout.contains("COMPILING b"),
        "b must stay up to date - this is the whole point of per-item paths:\n{stdout}"
    );
}

/// A variable nothing will expand fails the task rather than silently matching
/// no file.
///
/// Checked when the task is built, which is the first point its own `envs:` are
/// merged with the globals - `--help` and `--list-subtasks` do not build tasks
/// and so cannot report it. Recorded deliberately: validating earlier would have
/// to guess at task-level envs and would reject valid ottofiles.
#[test]
fn an_unexpandable_foreach_path_variable_is_rejected() {
    let (_dir, proj, home) = project(
        "otto:\n  api: 1\n  tasks: [fe]\ntasks:\n  fe:\n    foreach: {items: [a], as: item}\n    input: [\"src/${item}-${NOPE}.txt\"]\n    action: echo hi\n",
    );

    let (code, stdout, stderr) = run(&proj, &home, &["fe"]);
    assert_ne!(code, 0, "otto fe must fail\nstdout:\n{stdout}");
    assert!(
        stderr.contains("NOPE"),
        "the error must name the unexpandable variable, got:\n{stderr}"
    );
}

/// A global variable in a plain task's path resolves and the task caches on it.
///
/// This was the silent half, and the worse one. Paths expanded nothing, so
/// `${SRCDIR}/a.txt` matched no file, the task tracked no inputs and re-ran on
/// every invocation while reporting success. Measured before the fix: the
/// literal path skipped as up to date, the variable path ran forever - and
/// `examples/environment-variables/otto.yml` ships
/// `output: ["${BUILD_DIR}/${PROJECT_NAME}"]`, so this is a documented feature
/// that silently did not work.
#[test]
fn a_global_variable_in_a_plain_task_path_resolves_and_caches() {
    let (_dir, proj, home) = project(
        "otto:\n  api: 1\n  tasks: [plain]\n  envs:\n    SRCDIR: src\n    OUTDIR: out\ntasks:\n  plain:\n    input: [\"${SRCDIR}/a.txt\"]\n    output: [\"${OUTDIR}/plain.o\"]\n    action: |\n      mkdir -p out\n      echo done > out/plain.o\n      echo PLAIN-RAN\n",
    );
    fs::write(proj.join("src/a.txt"), "x").expect("write a");

    let (code, stdout, stderr) = run(&proj, &home, &["plain"]);
    assert_eq!(code, 0, "cold run failed\nstdout:\n{stdout}\nstderr:\n{stderr}");
    assert!(stdout.contains("PLAIN-RAN"), "cold run must execute:\n{stdout}");

    let (code, stdout, _) = run(&proj, &home, &["plain"]);
    assert_eq!(code, 0);
    assert!(
        !stdout.contains("PLAIN-RAN"),
        "a variable path must resolve so the task can go up to date:\n{stdout}"
    );
}

/// A variable no environment defines is still an error, in either shape.
#[test]
fn an_undefined_path_variable_is_an_error() {
    for body in [
        "otto:\n  api: 1\n  tasks: [plain]\ntasks:\n  plain:\n    input: [\"${NOPE}/a.txt\"]\n    action: echo hi\n",
        "otto:\n  api: 1\n  tasks: [fe]\ntasks:\n  fe:\n    foreach: {items: [a], as: item}\n    input: [\"${NOPE}/${item}.txt\"]\n    action: echo hi\n",
    ] {
        let (_dir, proj, home) = project(body);
        let task = if body.contains("foreach") { "fe" } else { "plain" };
        let (code, stdout, stderr) = run(&proj, &home, &[task]);
        assert_ne!(code, 0, "an undefined path variable must fail\nstdout:\n{stdout}");
        assert!(stderr.contains("NOPE"), "the error must name it, got:\n{stderr}");
    }
}
