//! Integration tests for `foreach.jobs` schema and load-time validation
//! (design doc `docs/design/2026-09-01-cancellation-reaping-and-foreach-
//! concurrency.md`, Phase 2). Schema and validation only: nothing here
//! exercises scheduler concurrency, which is Phase 3.

mod common;

use serde_json::Value as JsonValue;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Output;
use tempfile::TempDir;

fn write_ottofile(dir: &Path, contents: &str) -> PathBuf {
    let path = dir.join("otto.yml");
    fs::write(&path, contents).unwrap();
    path
}

fn otto(ottofile: &Path, args: &[&str]) -> Output {
    let home = ottofile.parent().expect("ottofile must live in a directory");
    common::otto_cmd(home)
        .arg("-o")
        .arg(ottofile)
        .args(args)
        .output()
        .unwrap()
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).to_string()
}

/// Success criterion (a), shape 1: `jobs` combined with `parallel: false` is
/// incoherent (serial already means one item at a time) and fails to load,
/// naming both keys and the task.
#[test]
fn jobs_with_parallel_false_fails_to_load_naming_both_keys_and_task() {
    let temp_dir = TempDir::new().unwrap();
    let ottofile = write_ottofile(
        temp_dir.path(),
        r#"
otto:
  api: 1

tasks:
  logs:
    foreach:
      items: [alpha, beta]
      parallel: false
      jobs: all
    bash: echo ${item}
"#,
    );

    let output = otto(&ottofile, &["--tasks"]);
    assert!(!output.status.success(), "jobs + parallel: false must fail to load");
    let err = stderr(&output);
    assert!(err.contains("jobs"), "error must name 'jobs': {err}");
    assert!(err.contains("parallel"), "error must name 'parallel': {err}");
    assert!(err.contains("logs"), "error must name the task 'logs': {err}");
}

/// Success criterion (a), shape 2: `jobs` written as a sibling of `foreach:`
/// on the task, instead of nested inside it, fails to load naming the key and
/// the task path. This is `deny_unknown_fields` on the task's own helper
/// struct doing the work (no custom validator needed): `jobs` is not one of
/// its declared fields.
#[test]
fn jobs_outside_foreach_fails_to_load_naming_the_key_and_task_path() {
    let temp_dir = TempDir::new().unwrap();
    let ottofile = write_ottofile(
        temp_dir.path(),
        r#"
otto:
  api: 1

tasks:
  logs:
    jobs: all
    foreach:
      items: [alpha, beta]
    bash: echo ${item}
"#,
    );

    let output = otto(&ottofile, &["--tasks"]);
    assert!(!output.status.success(), "task-level 'jobs' must fail to load");
    let err = stderr(&output);
    assert!(err.contains("jobs"), "error must name 'jobs': {err}");
    assert!(
        err.contains("tasks.logs") || err.contains("'logs'"),
        "error must name the task path: {err}"
    );
}

/// Success criterion (a), shape 3: `jobs: 0` is rejected in favor of the
/// literal `all`, naming the task path.
#[test]
fn jobs_zero_fails_to_load_and_names_all_as_the_replacement() {
    let temp_dir = TempDir::new().unwrap();
    let ottofile = write_ottofile(
        temp_dir.path(),
        r#"
otto:
  api: 1

tasks:
  logs:
    foreach:
      items: [alpha, beta]
      jobs: 0
    bash: echo ${item}
"#,
    );

    let output = otto(&ottofile, &["--tasks"]);
    assert!(!output.status.success(), "jobs: 0 must fail to load");
    let err = stderr(&output);
    assert!(err.contains('0'), "error must name the rejected value 0: {err}");
    assert!(err.contains("all"), "error must name 'all' as the replacement: {err}");
    assert!(
        err.contains("tasks.logs") || err.contains("'logs'"),
        "error must name the task path: {err}"
    );
}

/// Success criterion (a), shape 4: a negative or non-integer `jobs` value is
/// a loud serde type error - already loud without any custom validation -
/// and, like the other three shapes, none of these panic.
#[test]
fn jobs_negative_or_non_integer_fails_to_load_without_panicking() {
    let temp_dir = TempDir::new().unwrap();
    for bad_value in ["-3", "1.5", "sometimes"] {
        let ottofile = write_ottofile(
            temp_dir.path(),
            &format!(
                r#"
otto:
  api: 1

tasks:
  logs:
    foreach:
      items: [alpha, beta]
      jobs: {bad_value}
    bash: echo ${{item}}
"#
            ),
        );

        let output = otto(&ottofile, &["--tasks"]);
        assert!(!output.status.success(), "jobs: {bad_value} must fail to load");
        // A panic surfaces as a stderr backtrace banner rather than a plain
        // eyre/serde error line; ruling it out directly rather than trusting
        // a non-zero exit alone (a panic in a subprocess is also non-zero).
        let err = stderr(&output);
        assert!(
            !err.contains("panicked"),
            "jobs: {bad_value} must fail cleanly, not panic: {err}"
        );
    }
}

/// `jobs: all` combined with `buffer: true` is legal: buffering is a display
/// policy, concurrency is a scheduling policy, and the two do not conflict.
#[test]
fn jobs_all_with_buffer_true_is_legal() {
    let temp_dir = TempDir::new().unwrap();
    let ottofile = write_ottofile(
        temp_dir.path(),
        r#"
otto:
  api: 1

tasks:
  logs:
    foreach:
      items: [alpha, beta]
      buffer: true
      jobs: all
    bash: echo ${item}
"#,
    );

    let output = otto(&ottofile, &["--tasks"]);
    assert!(
        output.status.success(),
        "jobs: all + buffer: true must load: {}",
        stderr(&output)
    );
}

/// A fixed positive `jobs` count also loads cleanly (the non-`all` half of
/// the enum).
#[test]
fn jobs_fixed_count_is_legal() {
    let temp_dir = TempDir::new().unwrap();
    let ottofile = write_ottofile(
        temp_dir.path(),
        r#"
otto:
  api: 1

tasks:
  logs:
    foreach:
      items: [alpha, beta]
      jobs: 4
    bash: echo ${item}
"#,
    );

    let output = otto(&ottofile, &["--tasks"]);
    assert!(output.status.success(), "jobs: 4 must load: {}", stderr(&output));
}

/// Success criterion (b), the regression guard: `foreach.jobs` never reaches
/// `--tasks --format json`. `TaskView` (`src/cli/commands/tasks.rs`) has
/// exactly four fields - `help`, `params`, `edges`, `subtasks` - and none of
/// them derive from `ForeachSpec`, so an ottofile setting `jobs` produces the
/// identical view shape as one that does not; `jobs` itself never appears in
/// the output.
#[test]
fn tasks_json_view_never_mentions_foreach_jobs() {
    let temp_dir = TempDir::new().unwrap();
    let ottofile = write_ottofile(
        temp_dir.path(),
        r#"
otto:
  api: 1

tasks:
  logs:
    foreach:
      items: [alpha, beta]
      jobs: all
    bash: echo ${item}
"#,
    );

    let output = otto(&ottofile, &["--tasks"]);
    assert!(output.status.success(), "stderr: {}", stderr(&output));
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        !stdout.contains("jobs"),
        "--tasks output must never mention 'jobs': {stdout}"
    );

    let json: JsonValue = serde_json::from_str(&stdout).expect("stdout must be valid JSON");
    let logs = json.get("logs").expect("'logs' task must be present");
    let mut keys: Vec<&str> = logs.as_object().unwrap().keys().map(String::as_str).collect();
    keys.sort_unstable();
    assert_eq!(
        keys,
        vec!["edges", "help", "params", "subtasks"],
        "TaskView's shape must stay exactly these four fields regardless of foreach.jobs"
    );
}
