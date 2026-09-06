//! Load-time validation of an ottofile's shape (design doc
//! docs/design/2026-09-02-second-code-review-remediation.md, Phase 4).
//!
//! These spawn the real `otto` binary because "at load" is the claim under
//! test: the error must reach the user before anything expands, executes, or
//! allocates, and that ordering is only observable from outside the process.

mod common;

use std::fs;
use std::path::Path;
use std::process::Output;
use std::time::{Duration, Instant};
use tempfile::TempDir;

fn write_ottofile(dir: &Path, contents: &str) -> std::path::PathBuf {
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

fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).to_string()
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).to_string()
}

/// Two static sources used to load: `validate_sources` returned `Ok` whenever
/// `command` was absent, and `resolve_items`'s `else if` chain then expanded
/// the glob and dropped the items without a word. `--help` is the surface
/// because it is the cheapest one that used to succeed.
#[test]
fn a_glob_and_items_foreach_fails_at_load_naming_both_sources() {
    let temp = TempDir::new().unwrap();
    let ottofile = write_ottofile(
        temp.path(),
        r#"
otto: {api: 1}
tasks:
  multi:
    foreach: {glob: "*.yml", items: [x, y], as: svc}
    bash: echo "${svc}"
"#,
    );

    let output = otto(&ottofile, &["--help"]);

    assert!(!output.status.success(), "must not load: {}", stdout(&output));
    let err = stderr(&output);
    assert!(err.contains("Task 'multi'"), "must name the task: {err}");
    assert!(err.contains("glob"), "must name glob: {err}");
    assert!(err.contains("items"), "must name items: {err}");
    assert!(err.contains("exactly one source"), "{err}");
    assert!(!stdout(&output).contains("items]"), "must not render a count");
}

/// A `foreach:` with no source at all used to load and fail much later, at
/// expansion.
#[test]
fn a_sourceless_foreach_fails_at_load() {
    let temp = TempDir::new().unwrap();
    let ottofile = write_ottofile(
        temp.path(),
        r#"
otto: {api: 1}
tasks:
  empty:
    foreach: {}
    bash: echo hi
"#,
    );

    let output = otto(&ottofile, &["--help"]);

    assert!(!output.status.success());
    let err = stderr(&output);
    assert!(err.contains("Task 'empty'"), "must name the task: {err}");
    assert!(err.contains("no source"), "{err}");
}

/// `as: "my item"` used to load and fail at run time in `executor::action`,
/// whose message names "environment variable name" - not the field the author
/// wrote. Now it fails at load naming `foreach.as`.
#[test]
fn a_non_identifier_foreach_as_fails_at_load_naming_the_field() {
    let temp = TempDir::new().unwrap();
    let ottofile = write_ottofile(
        temp.path(),
        r#"
otto: {api: 1}
tasks:
  up:
    foreach: {items: [a], as: "my item"}
    bash: echo hi
"#,
    );

    let output = otto(&ottofile, &["--help"]);

    assert!(!output.status.success(), "must not load: {}", stdout(&output));
    let err = stderr(&output);
    assert!(err.contains("Task 'up'"), "must name the task: {err}");
    assert!(err.contains("foreach.as"), "must name the field: {err}");
    assert!(err.contains("my item"), "must quote the value: {err}");
}

/// The whole `usize` space as a range used to be counted by building it: the
/// `max_items` guard ran against an already-materialized `Vec`, so the check
/// could only fire after the allocation it exists to prevent. Now the count is
/// computed with checked arithmetic at load.
#[test]
fn a_range_spanning_the_usize_space_fails_at_load_without_allocating() {
    let temp = TempDir::new().unwrap();
    let ottofile = write_ottofile(
        temp.path(),
        r#"
otto: {api: 1}
tasks:
  huge:
    foreach: {range: "0-18446744073709551615", as: n}
    bash: echo "${n}"
"#,
    );

    let started = Instant::now();
    let output = otto(&ottofile, &["--help"]);
    let elapsed = started.elapsed();

    assert!(!output.status.success(), "must not load: {}", stdout(&output));
    let err = stderr(&output);
    assert!(err.contains("Task 'huge'"), "must name the task: {err}");
    assert!(err.contains("max_items"), "must name the limit: {err}");
    assert!(
        elapsed < Duration::from_secs(1),
        "must fail without expanding the range, took {elapsed:?}"
    );
}

/// A range that fits `max_items` still expands.
#[test]
fn a_range_inside_max_items_still_loads_and_expands() {
    let temp = TempDir::new().unwrap();
    let ottofile = write_ottofile(
        temp.path(),
        r#"
otto: {api: 1}
tasks:
  count:
    foreach: {range: "1-3", as: n}
    bash: echo "n=${n}"
"#,
    );

    let output = otto(&ottofile, &["count"]);

    assert!(output.status.success(), "{}", stderr(&output));
    let out = stdout(&output);
    for expected in ["n=1", "n=2", "n=3"] {
        assert!(out.contains(expected), "missing {expected}: {out}");
    }
}

/// A task keyed `2024:` loads, because YAML hands the task map an integer
/// scalar and the key is stringified. An edge naming it used to fail with
/// "invalid type: integer" - two signals for one meaning. Both are the same
/// name written in the same place, so both are accepted.
#[test]
fn a_numeric_edge_target_loads_and_runs_the_numerically_named_task() {
    let temp = TempDir::new().unwrap();
    let ottofile = write_ottofile(
        temp.path(),
        r#"
otto: {api: 1}
tasks:
  2024:
    bash: echo "ran-2024"
  report:
    after: [2024]
    bash: echo "ran-report"
"#,
    );

    let output = otto(&ottofile, &["report"]);

    assert!(output.status.success(), "{}", stderr(&output));
    let out = stdout(&output);
    assert!(out.contains("ran-2024"), "the numerically named task must run: {out}");
    assert!(out.contains("ran-report"), "{out}");
}

/// The boolean twin of the case above: `true:` is a YAML boolean key, and an
/// edge naming it arrives as a boolean scalar.
#[test]
fn a_boolean_edge_target_loads_and_runs_the_booleanly_named_task() {
    let temp = TempDir::new().unwrap();
    let ottofile = write_ottofile(
        temp.path(),
        r#"
otto: {api: 1}
tasks:
  true:
    bash: echo "ran-true"
  report:
    after: [true]
    bash: echo "ran-report"
"#,
    );

    let output = otto(&ottofile, &["report"]);

    assert!(output.status.success(), "{}", stderr(&output));
    let out = stdout(&output);
    assert!(out.contains("ran-true"), "the booleanly named task must run: {out}");
    assert!(out.contains("ran-report"), "{out}");
}
