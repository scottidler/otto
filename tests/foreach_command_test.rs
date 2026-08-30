//! Integration tests for `foreach: command:` (design doc
//! docs/design/2026-08-28-boundary-fixes-and-dynamic-foreach.md, Phase 6).
//!
//! These spawn the real `otto` binary: the feature's whole contract is about
//! *which invocations execute the command*, and that is only observable from
//! outside the process (help paths call `std::process::exit`, and the counter
//! must count real subprocesses).

mod common;

use serde_json::Value as JsonValue;
use std::fs;
use std::path::Path;
use std::process::Output;
use tempfile::TempDir;

fn write_ottofile(dir: &Path, contents: &str) -> std::path::PathBuf {
    let path = dir.join("otto.yml");
    fs::write(&path, contents).unwrap();
    path
}

/// The ottofile's own directory doubles as the isolated `OTTO_HOME`: every
/// fixture here already lives in its own `TempDir`, so there's no need for a
/// second scratch dir.
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

/// The design doc's acceptance-criteria fixture, verbatim.
const FIXTURE: &str = r#"
otto:
  api: 1

tasks:
  up:
    help: "Bring up each service"
    foreach:
      command: "printf 'alpha\nbeta\n'"
      as: svc
      parallel: false
    bash: |
      echo "up ${svc}"

  build:
    help: "an unrelated task"
    bash: echo built
"#;

/// A fixture whose command appends one line to $COUNTER_FILE per execution.
fn counter_fixture(counter: &Path) -> String {
    format!(
        r#"
otto:
  api: 1

tasks:
  up:
    help: "Bring up each service"
    foreach:
      command: "echo ran >> {counter}; printf 'alpha\nbeta\n'"
      as: svc
      parallel: false
    bash: echo "up ${{svc}}"

  build:
    help: "an unrelated task"
    bash: echo built
"#,
        counter = counter.display()
    )
}

fn count(counter: &Path) -> usize {
    fs::read_to_string(counter).map(|s| s.lines().count()).unwrap_or(0)
}

// ----------------------------------------------------------------------
// (a) the doc's YAML example expands and runs
// ----------------------------------------------------------------------

#[test]
fn command_source_expands_and_runs_subtasks() {
    let temp = TempDir::new().unwrap();
    let ottofile = write_ottofile(temp.path(), FIXTURE);

    let output = otto(&ottofile, &["up"]);

    assert!(output.status.success(), "stderr: {}", stderr(&output));
    let out = stdout(&output);
    assert!(out.contains("[up:alpha] up alpha"), "{out}");
    assert!(out.contains("[up:beta] up beta"), "{out}");
}

/// Phase 4's serial-group ordering is what makes `parallel: false` + `command:`
/// safe: targeting one subtask must run exactly that subtask.
#[test]
fn serial_command_source_targets_one_subtask_without_its_predecessor() {
    let temp = TempDir::new().unwrap();
    let ottofile = write_ottofile(temp.path(), FIXTURE);

    let output = otto(&ottofile, &["up:beta"]);

    assert!(output.status.success(), "stderr: {}", stderr(&output));
    let out = stdout(&output);
    assert!(out.contains("up beta"), "{out}");
    assert!(!out.contains("up alpha"), "predecessor must not be pulled in: {out}");
}

/// And a full serial run keeps the declared order.
#[test]
fn serial_command_source_runs_subtasks_in_command_order() {
    let temp = TempDir::new().unwrap();
    let ottofile = write_ottofile(temp.path(), FIXTURE);

    let output = otto(&ottofile, &["up"]);

    let out = stdout(&output);
    let alpha = out.find("up alpha").expect("alpha ran");
    let beta = out.find("up beta").expect("beta ran");
    assert!(alpha < beta, "serial subtasks run in the command's line order: {out}");
}

// ----------------------------------------------------------------------
// (b) counter test: exactly once when needed, zero times otherwise
// ----------------------------------------------------------------------

#[test]
fn command_runs_exactly_once_for_an_invocation_that_needs_it() {
    let temp = TempDir::new().unwrap();
    let counter = temp.path().join("counter");
    let ottofile = write_ottofile(temp.path(), &counter_fixture(&counter));

    let output = otto(&ottofile, &["up"]);
    assert!(output.status.success(), "stderr: {}", stderr(&output));
    assert_eq!(count(&counter), 1, "resolution is cached per invocation");

    // A subtask-shaped token resolves too (validating the token costs one run).
    fs::write(&counter, "").unwrap();
    let output = otto(&ottofile, &["up:beta"]);
    assert!(output.status.success(), "stderr: {}", stderr(&output));
    assert_eq!(count(&counter), 1);
}

#[test]
fn help_never_executes_the_command_and_renders_dynamic() {
    let temp = TempDir::new().unwrap();
    let counter = temp.path().join("counter");
    let ottofile = write_ottofile(temp.path(), &counter_fixture(&counter));

    // NOTE: the global-help path re-divines the ottofile from `.` instead of
    // honoring `-o` (a pre-existing bug, assigned to the remediation doc), so
    // this one runs from the fixture directory.
    let output = common::otto_cmd(temp.path())
        .current_dir(temp.path())
        .arg("--help")
        .output()
        .unwrap();
    let out = stdout(&output);
    assert!(out.contains("[dynamic]"), "global help must render [dynamic]: {out}");
    assert!(!out.contains("items]"), "{out}");
    assert_eq!(count(&counter), 0, "otto --help must never execute a command source");

    let output = otto(&ottofile, &["help", "up"]);
    assert!(stdout(&output).contains("[dynamic]"), "{}", stdout(&output));
    assert_eq!(count(&counter), 0);

    let output = otto(&ottofile, &["up", "--help"]);
    assert!(stdout(&output).contains("[dynamic]"), "{}", stdout(&output));
    assert_eq!(count(&counter), 0);
}

#[test]
fn targeting_an_unrelated_task_never_executes_the_command() {
    let temp = TempDir::new().unwrap();
    let counter = temp.path().join("counter");
    let ottofile = write_ottofile(temp.path(), &counter_fixture(&counter));

    let output = otto(&ottofile, &["build"]);

    assert!(output.status.success(), "stderr: {}", stderr(&output));
    assert!(stdout(&output).contains("built"));
    assert_eq!(count(&counter), 0, "an unrelated task must not resolve `up`");
}

/// The enumeration surfaces DO resolve: giving the real list is their job.
#[test]
fn enumeration_surfaces_resolve_the_command_once() {
    let temp = TempDir::new().unwrap();
    let counter = temp.path().join("counter");
    let ottofile = write_ottofile(temp.path(), &counter_fixture(&counter));

    let output = otto(&ottofile, &["--list-subtasks"]);
    assert!(output.status.success(), "stderr: {}", stderr(&output));
    let out = stdout(&output);
    assert!(out.contains("up:alpha") && out.contains("up:beta"), "{out}");
    assert_eq!(count(&counter), 1);

    fs::write(&counter, "").unwrap();
    let output = otto(&ottofile, &["--tasks"]);
    assert!(output.status.success(), "stderr: {}", stderr(&output));
    let json: JsonValue = serde_json::from_str(&stdout(&output)).expect("stdout must be valid JSON");
    let subtasks = json["up"]["subtasks"].as_array().expect("subtasks array");
    assert_eq!(
        subtasks.iter().filter_map(JsonValue::as_str).collect::<Vec<_>>(),
        vec!["up:alpha", "up:beta"]
    );
    assert_eq!(count(&counter), 1);
}

// ----------------------------------------------------------------------
// (c) failure modes are loud, with nothing on stdout
// ----------------------------------------------------------------------

#[test]
fn nonzero_exit_fails_loudly_naming_task_and_command_with_empty_stdout() {
    let temp = TempDir::new().unwrap();
    let ottofile = write_ottofile(
        temp.path(),
        r#"
otto: {api: 1}
tasks:
  up:
    foreach: {command: "echo boom >&2; exit 7", as: svc}
    bash: echo "up ${svc}"
"#,
    );

    let output = otto(&ottofile, &["up"]);

    assert!(!output.status.success(), "must exit non-zero");
    assert_eq!(stdout(&output), "", "nothing on stdout");
    let err = stderr(&output);
    assert!(err.contains("up"), "{err}");
    assert!(err.contains("echo boom >&2; exit 7"), "{err}");
    assert!(err.contains("exit code 7"), "{err}");
}

#[test]
fn a_failing_command_is_loud_on_the_enumeration_surfaces_too() {
    let temp = TempDir::new().unwrap();
    let ottofile = write_ottofile(
        temp.path(),
        r#"
otto: {api: 1}
tasks:
  up:
    foreach: {command: "exit 3", as: svc}
    bash: echo "up ${svc}"
"#,
    );

    for surface in [["--tasks"], ["--list-subtasks"]] {
        let output = otto(&ottofile, &surface);
        assert!(!output.status.success(), "{surface:?} must exit non-zero");
        assert_eq!(stdout(&output), "", "{surface:?} must put nothing on stdout");
        assert!(stderr(&output).contains("exit code 3"), "{}", stderr(&output));
    }
}

#[test]
fn zero_lines_is_an_empty_scope_with_a_stderr_notice() {
    let temp = TempDir::new().unwrap();
    let ottofile = write_ottofile(
        temp.path(),
        r#"
otto: {api: 1}
tasks:
  up:
    foreach: {command: "true", as: svc}
    bash: echo "up ${svc}"
"#,
    );

    let output = otto(&ottofile, &["up"]);

    assert!(
        output.status.success(),
        "an empty scope is legitimate: {}",
        stderr(&output)
    );
    assert!(!stdout(&output).contains("up:"), "no subtasks: {}", stdout(&output));
    let err = stderr(&output);
    assert!(err.contains("produced no items"), "{err}");
    assert!(err.contains("up"), "{err}");
}

#[test]
fn command_combined_with_a_static_source_is_a_config_error() {
    let temp = TempDir::new().unwrap();
    let ottofile = write_ottofile(
        temp.path(),
        r#"
otto: {api: 1}
tasks:
  up:
    foreach: {command: "printf 'a\n'", items: [x], as: svc}
    bash: echo "up ${svc}"
"#,
    );

    let output = otto(&ottofile, &["up"]);

    assert!(!output.status.success());
    assert_eq!(stdout(&output), "");
    let err = stderr(&output);
    assert!(err.contains("Task 'up'"), "{err}");
    assert!(err.contains("cannot be combined with items"), "{err}");
}

#[test]
fn duplicate_lines_produce_a_duplicate_identifier_error() {
    let temp = TempDir::new().unwrap();
    let ottofile = write_ottofile(
        temp.path(),
        r#"
otto: {api: 1}
tasks:
  up:
    foreach: {command: "printf 'a\na\n'", as: svc}
    bash: echo "up ${svc}"
"#,
    );

    let output = otto(&ottofile, &["up"]);

    assert!(!output.status.success());
    assert!(
        stderr(&output).contains("duplicate subtask name 'up:a'"),
        "{}",
        stderr(&output)
    );
}

#[test]
fn recursion_guard_errors_when_the_task_is_already_being_resolved() {
    let temp = TempDir::new().unwrap();
    let ottofile = write_ottofile(
        temp.path(),
        r#"
otto: {api: 1}
tasks:
  up:
    foreach: {command: "printf 'alpha\n'", as: svc}
    bash: echo "up ${svc}"
  build:
    bash: echo built
"#,
    );

    // Standing in for an inner otto: the outer resolution set the guard.
    let output = common::otto_cmd(temp.path())
        .arg("-o")
        .arg(&ottofile)
        .arg("up")
        .env("OTTO_FOREACH_COMMAND", "up")
        .output()
        .unwrap();

    assert!(!output.status.success());
    assert_eq!(stdout(&output), "");
    let err = stderr(&output);
    assert!(err.contains("recursion detected"), "{err}");
    assert!(err.contains("cycle: up -> up"), "{err}");

    // An inner otto targeting an ordinary task is unaffected.
    let output = common::otto_cmd(temp.path())
        .arg("-o")
        .arg(&ottofile)
        .arg("build")
        .env("OTTO_FOREACH_COMMAND", "up")
        .output()
        .unwrap();
    assert!(output.status.success(), "stderr: {}", stderr(&output));
    assert!(stdout(&output).contains("built"));
}

// ----------------------------------------------------------------------
// execution contract: sh -c, cwd = ottofile dir, inherited env + global envs
// ----------------------------------------------------------------------

#[test]
fn command_runs_in_the_ottofile_directory_with_the_resolved_global_envs() {
    let temp = TempDir::new().unwrap();
    let nested = temp.path().join("nested");
    fs::create_dir(&nested).unwrap();
    let ottofile = write_ottofile(
        temp.path(),
        r#"
otto:
  api: 1
  envs:
    SVC_LIST: "$(printf 'alpha beta')"
tasks:
  up:
    foreach:
      command: "printf '%s\n' ${SVC_LIST}; basename \"$(pwd)\" > marker; printf '%s\n' \"${FROM_SHELL}\""
      as: svc
    bash: echo "up ${svc}"
"#,
    );

    // Invoked from a *different* directory, with an inherited shell variable.
    // OTTO_HOME still isolates to the fixture's own temp dir, not the cwd.
    let output = common::otto_cmd(temp.path())
        .current_dir(&nested)
        .arg("-o")
        .arg(&ottofile)
        .arg("up")
        .env("FROM_SHELL", "inherited")
        .output()
        .unwrap();

    assert!(output.status.success(), "stderr: {}", stderr(&output));
    let out = stdout(&output);
    // global envs reached the command...
    assert!(out.contains("up alpha") && out.contains("up beta"), "{out}");
    // ...as did the inherited environment...
    assert!(out.contains("up inherited"), "{out}");
    // ...and the command's cwd was the ottofile's directory, not otto's cwd.
    let marker = fs::read_to_string(temp.path().join("marker")).expect("marker written beside the ottofile");
    assert_eq!(
        marker.trim(),
        temp.path().file_name().unwrap().to_string_lossy(),
        "command cwd must be the ottofile's directory"
    );
    assert!(!nested.join("marker").exists());
}

/// Task params are deliberately NOT available: params resolve after expansion.
#[test]
fn task_params_are_not_visible_to_the_command() {
    let temp = TempDir::new().unwrap();
    let ottofile = write_ottofile(
        temp.path(),
        r#"
otto: {api: 1}
tasks:
  up:
    params:
      -s|--svc:
        help: "service name"
        default: "from-param"
    foreach:
      command: "printf 'seen-%s\n' \"${svc:-unset}\""
      as: item
    bash: echo "up ${item}"
"#,
    );

    let output = otto(&ottofile, &["up", "--svc", "web"]);

    assert!(output.status.success(), "stderr: {}", stderr(&output));
    let out = stdout(&output);
    assert!(
        out.contains("up seen-unset"),
        "params must not reach the command: {out}"
    );
}
