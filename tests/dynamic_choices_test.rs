//! Integration tests for dynamic param `choices` (design doc
//! docs/design/2026-08-28-boundary-fixes-and-dynamic-foreach.md, Phase 6b).
//!
//! These spawn the real `otto` binary for the same reason the `foreach:
//! command:` tests do: the contract is about *which invocations execute the
//! command*, and that is only observable from outside the process (help paths
//! call `std::process::exit`, and the counter must count real subprocesses).

use assert_cmd::cargo::cargo_bin_cmd;
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

fn otto(ottofile: &Path, args: &[&str]) -> Output {
    cargo_bin_cmd!("otto")
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
  switch:
    help: "Switch the active service"
    params:
      -s|--svc:
        choices-command: "printf 'alpha\nbeta\n'"
        help: Service to switch to
    bash: echo "switched to ${svc}"

  build:
    help: "an unrelated task"
    bash: echo built
"#;

/// A dependency (`prep`) owns the dynamic set; the task the user invokes
/// (`deploy`) just has a plain param whose value propagates into it. This is the
/// second bind trigger: `prep`'s command runs because `prep`'s param must
/// validate, even though the user never typed `prep`.
const PROPAGATION_FIXTURE: &str = r#"
otto:
  api: 1

tasks:
  prep:
    help: "dependency carrying the dynamic value set"
    params:
      -s|--svc:
        choices-command: "printf 'alpha\nbeta\n'"
        help: Service to prepare
      -t|--tag:
        help: an ordinary param, used to give prep its own CLI partition
    bash: echo "prep ${svc}"

  deploy:
    help: "the task the user invokes"
    params:
      -s|--svc:
        help: Service to deploy
    before: [prep]
    bash: echo "deploy ${svc}"
"#;

/// A fixture whose choices command appends one line to `counter` per execution.
/// Single-line command so the `[dynamic choices: ...]` help marker is greppable.
fn counter_fixture(counter: &Path) -> String {
    format!(
        r#"
otto:
  api: 1

tasks:
  switch:
    help: "Switch the active service"
    params:
      -s|--svc:
        choices-command: "echo ran >> {counter}; echo alpha; echo beta"
        help: Service to switch to
    bash: echo "switched to ${{svc}}"

  build:
    help: "an unrelated task"
    bash: echo built
"#,
        counter = counter.display()
    )
}

/// The propagation fixture, counting executions the same way.
fn counter_propagation_fixture(counter: &Path) -> String {
    format!(
        r#"
otto:
  api: 1

tasks:
  prep:
    help: "dependency carrying the dynamic value set"
    params:
      -s|--svc:
        choices-command: "echo ran >> {counter}; echo alpha; echo beta"
        help: Service to prepare
      -t|--tag:
        help: an ordinary param
    bash: echo "prep ${{svc}}"

  deploy:
    help: "the task the user invokes"
    params:
      -s|--svc:
        help: Service to deploy
    before: [prep]
    bash: echo "deploy ${{svc}}"
"#,
        counter = counter.display()
    )
}

fn count(counter: &Path) -> usize {
    fs::read_to_string(counter).map(|s| s.lines().count()).unwrap_or(0)
}

// ----------------------------------------------------------------------
// (a) the value set actually validates, directly and through propagation
// ----------------------------------------------------------------------

#[test]
fn a_value_in_the_dynamic_set_is_accepted() {
    let temp = TempDir::new().unwrap();
    let ottofile = write_ottofile(temp.path(), FIXTURE);

    let output = otto(&ottofile, &["switch", "--svc", "beta"]);

    assert!(output.status.success(), "stderr: {}", stderr(&output));
    assert!(stdout(&output).contains("switched to beta"), "{}", stdout(&output));
}

#[test]
fn a_value_outside_the_dynamic_set_is_rejected_and_lists_the_values() {
    let temp = TempDir::new().unwrap();
    let ottofile = write_ottofile(temp.path(), FIXTURE);

    let output = otto(&ottofile, &["switch", "--svc", "nosuch"]);

    assert!(!output.status.success(), "must not run: {}", stdout(&output));
    let err = stderr(&output);
    assert!(err.contains("nosuch"), "must name the bad value: {err}");
    assert!(err.contains("alpha"), "must list the valid values: {err}");
    assert!(err.contains("beta"), "must list the valid values: {err}");
    assert!(
        !stdout(&output).contains("switched to"),
        "task body must not run: {}",
        stdout(&output)
    );
}

#[test]
fn a_propagated_value_outside_the_dependencys_dynamic_set_is_rejected() {
    let temp = TempDir::new().unwrap();
    let ottofile = write_ottofile(temp.path(), PROPAGATION_FIXTURE);

    let output = otto(&ottofile, &["deploy", "--svc", "nosuch"]);

    assert!(!output.status.success(), "must not run: {}", stdout(&output));
    let err = stderr(&output);
    assert!(err.contains("not in allowed choices"), "{err}");
    assert!(err.contains("nosuch"), "must name the value: {err}");
    assert!(err.contains("prep"), "must name the task that rejected it: {err}");
    assert!(err.contains("alpha, beta"), "must list the resolved values: {err}");
}

#[test]
fn a_propagated_value_inside_the_dependencys_dynamic_set_is_accepted() {
    let temp = TempDir::new().unwrap();
    let ottofile = write_ottofile(temp.path(), PROPAGATION_FIXTURE);

    let output = otto(&ottofile, &["deploy", "--svc", "beta"]);

    assert!(output.status.success(), "stderr: {}", stderr(&output));
    let out = stdout(&output);
    assert!(out.contains("prep beta"), "{out}");
    assert!(out.contains("deploy beta"), "{out}");
}

// ----------------------------------------------------------------------
// (b) counter: zero executions for every help/enumeration surface,
//     exactly one across direct binding plus propagation
// ----------------------------------------------------------------------

#[test]
fn help_surfaces_never_execute_the_command_and_render_the_marker() {
    let temp = TempDir::new().unwrap();
    let counter = temp.path().join("counter");
    let ottofile = write_ottofile(temp.path(), &counter_fixture(&counter));

    // Global help: the params aren't rendered here, but it must still not run.
    // NOTE: the global-help path re-divines the ottofile from `.` instead of
    // honoring `-o` (a pre-existing bug, assigned to the remediation doc), so
    // this one runs from the fixture directory.
    let output = cargo_bin_cmd!("otto")
        .current_dir(temp.path())
        .arg("--help")
        .output()
        .unwrap();
    assert_eq!(count(&counter), 0, "otto --help executed the choices command");

    // `otto help switch` renders the params, and names the command in place of
    // the values it refuses to go and fetch.
    let output_help_task = otto(&ottofile, &["help", "switch"]);
    let rendered = stdout(&output_help_task);
    assert!(
        rendered.contains("[dynamic choices: echo ran >>"),
        "otto help switch must name the command: {rendered}"
    );
    assert_eq!(count(&counter), 0, "otto help switch executed the choices command");

    // `<task> --help` is the same posture.
    let output_task_help = otto(&ottofile, &["switch", "--help"]);
    assert!(
        stdout(&output_task_help).contains("[dynamic choices:"),
        "{}",
        stdout(&output_task_help)
    );
    assert_eq!(count(&counter), 0, "otto switch --help executed the choices command");

    // Sanity: the global help really did render.
    assert!(stdout(&output).contains("switch"), "{}", stdout(&output));
}

#[test]
fn tasks_reports_provenance_without_executing() {
    let temp = TempDir::new().unwrap();
    let counter = temp.path().join("counter");
    let ottofile = write_ottofile(temp.path(), &counter_fixture(&counter));

    let output = otto(&ottofile, &["--tasks"]);
    assert!(output.status.success(), "stderr: {}", stderr(&output));
    assert_eq!(count(&counter), 0, "--tasks executed the choices command");

    let view: JsonValue = serde_json::from_str(&stdout(&output)).expect("--tasks must emit JSON when piped");
    let param = &view["switch"]["params"][0];
    assert_eq!(param["name"], "svc", "{view}");
    assert!(
        param["choices-command"]
            .as_str()
            .expect("dynamic param must carry choices-command")
            .contains("echo alpha"),
        "{view}"
    );
    assert!(
        param.get("choices").is_none(),
        "choices-command replaces choices, never sits beside it: {view}"
    );
}

#[test]
fn an_unrelated_task_never_executes_the_command() {
    let temp = TempDir::new().unwrap();
    let counter = temp.path().join("counter");
    let ottofile = write_ottofile(temp.path(), &counter_fixture(&counter));

    let output = otto(&ottofile, &["build"]);

    assert!(output.status.success(), "stderr: {}", stderr(&output));
    assert_eq!(count(&counter), 0, "an unrelated task executed the choices command");
}

#[test]
fn direct_binding_executes_the_command_exactly_once() {
    let temp = TempDir::new().unwrap();
    let counter = temp.path().join("counter");
    let ottofile = write_ottofile(temp.path(), &counter_fixture(&counter));

    let output = otto(&ottofile, &["switch", "--svc", "beta"]);

    assert!(output.status.success(), "stderr: {}", stderr(&output));
    assert_eq!(count(&counter), 1, "direct binding must resolve exactly once");
}

#[test]
fn propagation_alone_executes_the_command_exactly_once() {
    let temp = TempDir::new().unwrap();
    let counter = temp.path().join("counter");
    let ottofile = write_ottofile(temp.path(), &counter_propagation_fixture(&counter));

    let output = otto(&ottofile, &["deploy", "--svc", "beta"]);

    assert!(output.status.success(), "stderr: {}", stderr(&output));
    assert!(stdout(&output).contains("prep beta"), "{}", stdout(&output));
    assert_eq!(count(&counter), 1, "propagation validation must resolve exactly once");
}

/// The cache's whole point: `prep` gets its own CLI partition (so the bind path
/// builds its clap command AND resolves) and also inherits `--svc` from
/// `deploy` (so propagation validates it). Two triggers, one execution.
#[test]
fn direct_binding_plus_propagation_still_executes_the_command_exactly_once() {
    let temp = TempDir::new().unwrap();
    let counter = temp.path().join("counter");
    let ottofile = write_ottofile(temp.path(), &counter_propagation_fixture(&counter));

    let output = otto(&ottofile, &["deploy", "--svc", "beta", "prep", "--tag", "t1"]);

    assert!(output.status.success(), "stderr: {}", stderr(&output));
    assert!(stdout(&output).contains("prep beta"), "{}", stdout(&output));
    assert_eq!(
        count(&counter),
        1,
        "both bind triggers must share one cached resolution"
    );
}

// ----------------------------------------------------------------------
// (c) failure modes, all loud
// ----------------------------------------------------------------------

#[test]
fn a_nonzero_exit_fails_loudly_naming_task_param_and_command() {
    let temp = TempDir::new().unwrap();
    let ottofile = write_ottofile(
        temp.path(),
        r#"
otto:
  api: 1

tasks:
  switch:
    params:
      -s|--svc:
        choices-command: "echo boom >&2; exit 3"
    bash: echo "switched to ${svc}"
"#,
    );

    let output = otto(&ottofile, &["switch", "--svc", "beta"]);

    assert!(!output.status.success(), "must not run: {}", stdout(&output));
    let err = stderr(&output);
    assert!(err.contains("switch"), "must name the task: {err}");
    assert!(err.contains("svc"), "must name the param: {err}");
    assert!(err.contains("echo boom >&2; exit 3"), "must name the command: {err}");
    assert!(err.contains("exit code 3"), "{err}");
    assert!(stdout(&output).is_empty(), "nothing on stdout: {}", stdout(&output));
}

#[test]
fn zero_output_lines_fail_loudly_unlike_an_empty_foreach_scope() {
    let temp = TempDir::new().unwrap();
    let ottofile = write_ottofile(
        temp.path(),
        r#"
otto:
  api: 1

tasks:
  switch:
    params:
      -s|--svc:
        choices-command: "true"
    bash: echo "switched to ${svc}"
"#,
    );

    let output = otto(&ottofile, &["switch", "--svc", "beta"]);

    assert!(!output.status.success(), "must not run: {}", stdout(&output));
    let err = stderr(&output);
    assert!(err.contains("switch"), "must name the task: {err}");
    assert!(err.contains("svc"), "must name the param: {err}");
    assert!(err.contains("no values"), "must say why: {err}");
}

#[test]
fn choices_and_choices_command_together_is_a_config_load_error() {
    let temp = TempDir::new().unwrap();
    let ottofile = write_ottofile(
        temp.path(),
        r#"
otto:
  api: 1

tasks:
  switch:
    params:
      -s|--svc:
        choices: [alpha, beta]
        choices-command: "printf 'alpha\n'"
    bash: echo "switched to ${svc}"
"#,
    );

    // Config-shape errors are load-time: they fire before anything executes,
    // and they fire on a surface that executes nothing at all.
    for args in [&["switch", "--svc", "beta"][..], &["--tasks"][..]] {
        let output = otto(&ottofile, args);
        let err = stderr(&output);
        assert!(
            !output.status.success(),
            "{args:?} must not succeed: {}",
            stdout(&output)
        );
        assert!(err.contains("svc"), "{args:?} must name the param: {err}");
        assert!(err.contains("cannot be combined"), "{args:?}: {err}");
        assert!(err.contains("printf"), "{args:?} must name the command: {err}");
        assert!(
            err.contains("alpha, beta"),
            "{args:?} must name the static choices: {err}"
        );
    }
}

// ----------------------------------------------------------------------
// Recursion guard, symmetric with foreach's
// ----------------------------------------------------------------------

#[test]
fn a_nested_otto_resolving_the_same_key_errors_loudly() {
    let temp = TempDir::new().unwrap();
    let ottofile = write_ottofile(temp.path(), FIXTURE);

    let output = cargo_bin_cmd!("otto")
        .arg("-o")
        .arg(&ottofile)
        .args(["switch", "--svc", "beta"])
        // Stands in for an outer otto that is already resolving this key.
        .env("OTTO_CHOICES_COMMAND", "switch:svc")
        .output()
        .unwrap();

    assert!(!output.status.success(), "must not run: {}", stdout(&output));
    let err = stderr(&output);
    assert!(err.contains("recursion detected"), "{err}");
    assert!(err.contains("switch:svc"), "{err}");
}
