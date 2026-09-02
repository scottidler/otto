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

/// `foreach.jobs` combined with `tty: true` on the same task fails to load,
/// naming both keys and the task. Same rejection, and the same reason, as
/// `foreach.buffer` + `tty` eleven lines above it in
/// `src/cli/parser/config.rs`: a tty task owns the terminal exclusively, so a
/// per-group concurrency override cannot be honored. Rejected at load rather
/// than resolved at run time, so the answer arrives when the ottofile is read
/// and one file does not give two answers to one question.
#[test]
fn jobs_with_tty_fails_to_load_naming_both_keys_and_task() {
    let temp_dir = TempDir::new().unwrap();
    let ottofile = write_ottofile(
        temp_dir.path(),
        r#"
otto:
  api: 1

tasks:
  logs:
    tty: true
    foreach:
      items: [alpha, beta]
      jobs: all
    bash: echo ${item}
"#,
    );

    let output = otto(&ottofile, &["--tasks"]);
    assert!(!output.status.success(), "jobs + tty must fail to load");
    let err = stderr(&output);
    assert!(err.contains("jobs"), "error must name 'jobs': {err}");
    assert!(err.contains("tty"), "error must name 'tty': {err}");
    assert!(err.contains("logs"), "error must name the task 'logs': {err}");
}

/// The task name is what the error names, not an expanded item. `tty:` is
/// inherited by every subtask, so a rejection written against the expansion
/// would name `logs:alpha` - a name the user never wrote and cannot edit.
#[test]
fn the_tty_rejection_names_the_authored_task_not_an_item() {
    let temp_dir = TempDir::new().unwrap();
    let ottofile = write_ottofile(
        temp_dir.path(),
        r#"
otto:
  api: 1

tasks:
  logs:
    tty: true
    foreach:
      items: [alpha, beta]
      jobs: all
    bash: echo ${item}
"#,
    );

    let err = stderr(&otto(&ottofile, &["--tasks"]));
    assert!(
        !err.contains("logs:alpha") && !err.contains("logs:beta"),
        "the error must name the authored task, not an expanded item: {err}"
    );
}

/// `--Serial` with `foreach.jobs` is rejected, matching the `parallel: false`
/// rejection: the flag and the key ask the same incoherent thing, and which
/// entry point the serial request came in through does not change the answer.
/// Checked by running the task, since `--Serial` is a per-task CLI partition
/// and is not visible at config-load time.
#[test]
fn serial_flag_with_jobs_is_rejected_like_parallel_false() {
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
      parallel: true
      jobs: all
    bash: echo RAN-${item}
"#,
    );

    let output = otto(&ottofile, &["logs", "--Serial"]);
    assert!(!output.status.success(), "--Serial + jobs must be rejected");
    let err = stderr(&output);
    assert!(err.contains("Serial"), "error must name '--Serial': {err}");
    assert!(err.contains("jobs"), "error must name 'jobs': {err}");
    assert!(err.contains("logs"), "error must name the task 'logs': {err}");

    // Rejected before anything runs, like every other shape in this file.
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        !stdout.contains("RAN-") && !err.contains("RAN-"),
        "no item may execute once the combination is rejected: {stdout}{err}"
    );
}

/// The same ottofile without `--Serial` still runs: the rejection above is
/// about the flag, not about the `jobs:` key it was checked beside.
#[test]
fn the_same_jobs_task_runs_without_the_serial_flag() {
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
      parallel: true
      jobs: all
    bash: echo RAN-${item}
"#,
    );

    let output = otto(&ottofile, &["logs"]);
    assert!(
        output.status.success(),
        "jobs: all without --Serial must run: {}",
        stderr(&output)
    );
}
