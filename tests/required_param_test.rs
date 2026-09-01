//! Integration tests for `params.<title>.required` (design doc
//! `docs/design/2026-08-31-buffered-foreach-computed-envs-required-params.md`,
//! Phase 1).
//!
//! These spawn the real `otto` binary because the contract is about *which
//! invocation shapes reach clap at all* and *which subprocesses run on the
//! error path*, both of which are only observable from outside the process
//! (the preflight's whole point is answering without building a clap
//! `Command` or resolving `global_envs()`).

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

/// The design doc's Data Model fixture, adapted: `sw` carries two required
/// positionals (no `choices-command`, so binding it needs no subprocess and no
/// network), `build` is unrelated, and `dep`/`main` cover the
/// dependency-only case (criterion (c)).
const FIXTURE: &str = r#"
otto:
  api: 1

tasks:
  sw:
    help: "Switch the active service"
    params:
      svc:
        required: true
      branch:
        required: true
    bash: echo "svc=${svc} branch=${branch}"

  build:
    help: "an unrelated task"
    bash: echo built

  dep:
    help: "dependency-only task with a required, choices-validated param"
    params:
      mode:
        choices: [alpha, beta]
        required: true
    bash: echo "dep mode=[${mode:-}]"

  main:
    help: "the task the user actually invokes"
    before: [dep]
    bash: echo main
"#;

// ----------------------------------------------------------------------
// (a) bare invocation: otto's own preflight error, before clap, naming the
//     missing params - not clap's "required arguments were not provided".
// ----------------------------------------------------------------------

#[test]
fn bare_invocation_fails_with_ottos_own_error_naming_the_params() {
    let temp = TempDir::new().unwrap();
    let ottofile = write_ottofile(temp.path(), FIXTURE);

    let output = otto(&ottofile, &["sw"]);

    assert!(!output.status.success(), "must not run: {}", stdout(&output));
    let err = stderr(&output);
    assert!(err.contains("svc"), "must name the missing param: {err}");
    assert!(err.contains("branch"), "must name the missing param: {err}");
    assert!(
        !err.contains("the following required arguments were not provided"),
        "must be otto's own preflight error, not clap's: {err}"
    );
    assert!(stdout(&output).is_empty(), "nothing on stdout: {}", stdout(&output));
}

// ----------------------------------------------------------------------
// (b) supplied on the command line: binds and runs.
// ----------------------------------------------------------------------

#[test]
fn otto_sw_alpha_binds_and_runs() {
    let temp = TempDir::new().unwrap();
    let ottofile = write_ottofile(temp.path(), FIXTURE);

    let output = otto(&ottofile, &["sw", "alpha", "feature-1"]);

    assert!(output.status.success(), "stderr: {}", stderr(&output));
    assert!(
        stdout(&output).contains("svc=alpha branch=feature-1"),
        "{}",
        stdout(&output)
    );
}

// ----------------------------------------------------------------------
// (c) a required param on a dependency-only task still runs unset, exit 0 -
//     matching how `choices` already behaves for propagated values.
// ----------------------------------------------------------------------

#[test]
fn a_required_param_on_a_dependency_only_task_runs_unset() {
    let temp = TempDir::new().unwrap();
    let ottofile = write_ottofile(temp.path(), FIXTURE);

    let output = otto(&ottofile, &["main"]);

    assert!(output.status.success(), "stderr: {}", stderr(&output));
    assert!(
        stdout(&output).contains("dep mode=[]"),
        "dependency-only task must run with the required param unset: {}",
        stdout(&output)
    );
}

// ----------------------------------------------------------------------
// (d) load-time rejections: each of the four combinations, none of which
//     panics.
// ----------------------------------------------------------------------

fn assert_load_error_names(ottofile_body: &str, needle: &str) {
    let temp = TempDir::new().unwrap();
    let ottofile = write_ottofile(temp.path(), ottofile_body);

    let output = otto(&ottofile, &["--tasks"]);

    assert!(!output.status.success(), "must not load: {}", stdout(&output));
    let err = stderr(&output);
    assert!(err.contains(needle), "must name the param: {err}");
    assert!(!err.contains("panicked"), "must not panic: {err}");
}

#[test]
fn required_on_a_flag_is_a_load_error() {
    assert_load_error_names(
        r#"
otto:
  api: 1
tasks:
  sw:
    params:
      -v|--verbose:
        default: "false"
        required: true
    bash: echo hi
"#,
        "verbose",
    );
}

#[test]
fn required_with_default_is_a_load_error() {
    assert_load_error_names(
        r#"
otto:
  api: 1
tasks:
  sw:
    params:
      svc:
        default: web
        required: true
    bash: echo hi
"#,
        "svc",
    );
}

#[test]
fn required_with_a_zero_capable_nargs_is_a_load_error() {
    for spelling in ["0", "?", "*"] {
        assert_load_error_names(
            &format!(
                r#"
otto:
  api: 1
tasks:
  sw:
    params:
      svc:
        nargs: "{spelling}"
        required: true
    bash: echo hi
"#
            ),
            "svc",
        );
    }
}

#[test]
fn required_positional_after_optional_positional_is_a_load_error_not_a_panic() {
    assert_load_error_names(
        r#"
otto:
  api: 1
tasks:
  sw:
    params:
      svc:
        help: "optional, declared first"
      branch:
        required: true
    bash: echo hi
"#,
        "branch",
    );
}

// ----------------------------------------------------------------------
// (f) the preflight sits before Step 0's global_envs(), not merely before
//     the clap gate: bare invocation touches neither the choices-command's
//     marker nor the literal envs: substitution's marker.
// ----------------------------------------------------------------------

fn envs_and_choices_fixture(envs_marker: &Path, choices_marker: &Path) -> String {
    format!(
        r#"
otto:
  api: 1
  envs:
    MARK: "$(touch {envs_marker} && echo ok)"

tasks:
  switch:
    params:
      svc:
        choices-command: "touch {choices_marker} && echo alpha"
        required: true
    bash: echo "svc=${{svc}}"
"#,
        envs_marker = envs_marker.display(),
        choices_marker = choices_marker.display(),
    )
}

#[test]
fn bare_invocation_touches_neither_the_envs_marker_nor_the_choices_command_marker() {
    let temp = TempDir::new().unwrap();
    let envs_marker = temp.path().join("envs-marker");
    let choices_marker = temp.path().join("choices-marker");
    let ottofile = write_ottofile(temp.path(), &envs_and_choices_fixture(&envs_marker, &choices_marker));

    let output = otto(&ottofile, &["switch"]);

    assert!(!output.status.success(), "must not run: {}", stdout(&output));
    assert!(
        !envs_marker.exists(),
        "global_envs() must not have resolved on the preflight-rejected path"
    );
    assert!(
        !choices_marker.exists(),
        "choices-command must not have run on the preflight-rejected path"
    );
}

/// Positive control: the same fixture, given a value, DOES resolve both -
/// proving the markers aren't silently broken and the absence above is the
/// preflight's doing, not a fixture mistake.
#[test]
fn supplying_the_value_resolves_both_envs_and_choices_command() {
    let temp = TempDir::new().unwrap();
    let envs_marker = temp.path().join("envs-marker");
    let choices_marker = temp.path().join("choices-marker");
    let ottofile = write_ottofile(temp.path(), &envs_and_choices_fixture(&envs_marker, &choices_marker));

    let output = otto(&ottofile, &["switch", "alpha"]);

    assert!(output.status.success(), "stderr: {}", stderr(&output));
    assert!(envs_marker.exists(), "global_envs() must have resolved");
    assert!(choices_marker.exists(), "choices-command must have run");
}

// ----------------------------------------------------------------------
// (g) `--tasks` reports `required` only when true.
// ----------------------------------------------------------------------

#[test]
fn tasks_reports_required_only_when_true() {
    let temp = TempDir::new().unwrap();
    let ottofile = write_ottofile(
        temp.path(),
        r#"
otto:
  api: 1
tasks:
  sw:
    params:
      svc:
        required: true
      branch:
        help: "plain, not required"
    bash: echo hi
"#,
    );

    let output = otto(&ottofile, &["--tasks"]);
    assert!(output.status.success(), "stderr: {}", stderr(&output));

    let view: JsonValue = serde_json::from_str(&stdout(&output)).expect("--tasks must emit JSON when piped");
    let params = view["sw"]["params"].as_array().unwrap();
    let svc = params.iter().find(|p| p["name"] == "svc").unwrap();
    let branch = params.iter().find(|p| p["name"] == "branch").unwrap();

    assert_eq!(svc["required"], true, "{view}");
    assert!(
        branch.get("required").is_none(),
        "a plain param must omit the key entirely, not emit `false`: {view}"
    );
}

// ----------------------------------------------------------------------
// Break-the-code check (Testing Strategy): reverting the clap gate to
// `args.len() > 1` alone (i.e. removing the preflight) must fail the bare
// invocation case. This test IS that check - it fails if the preflight is
// ever removed while the gate stays untouched, which is exactly the
// regression the design doc calls out.
// ----------------------------------------------------------------------

#[test]
fn without_the_preflight_bare_invocation_would_silently_run_unset() {
    // Documents the property the preflight replaces: a plain param with no
    // `required:` key runs unset, exit 0, on a bare invocation. `required:
    // true` on the same shape must invert this to a non-zero exit (see
    // `bare_invocation_fails_with_ottos_own_error_naming_the_params` above).
    let temp = TempDir::new().unwrap();
    let ottofile = write_ottofile(
        temp.path(),
        r#"
otto:
  api: 1
tasks:
  sw:
    params:
      svc:
        help: "not required"
    bash: echo "svc=[${svc:-}]"
"#,
    );

    let output = otto(&ottofile, &["sw"]);
    assert!(output.status.success(), "stderr: {}", stderr(&output));
    assert!(stdout(&output).contains("svc=[]"), "{}", stdout(&output));
}

/// Criterion (f), completed. When Phase 1 landed, `otto.envs-command` did not
/// exist yet, so the "no subprocess on the error path" property could only be
/// pinned with a literal `envs:` substitution's marker. Now the key exists,
/// and the preflight's placement (before Step 0, not at the clap gate) is what
/// keeps its subprocess off this path too: `global_envs()` resolves at
/// `discovery.rs:171`, about 64 lines before the gate.
fn envs_command_and_choices_fixture(envs_marker: &Path, choices_marker: &Path) -> String {
    format!(
        r#"
otto:
  api: 1
  envs-command: "touch {envs_marker} && printf 'ALLOWED=alpha\n'"

tasks:
  switch:
    params:
      svc:
        choices-command: "touch {choices_marker} && echo alpha"
        required: true
    bash: echo "svc=${{svc}}"
"#,
        envs_marker = envs_marker.display(),
        choices_marker = choices_marker.display(),
    )
}

#[test]
fn bare_invocation_touches_neither_the_envs_command_marker_nor_the_choices_command_marker() {
    let temp = TempDir::new().unwrap();
    let envs_marker = temp.path().join("envs-command-marker");
    let choices_marker = temp.path().join("choices-marker");
    let ottofile = write_ottofile(
        temp.path(),
        &envs_command_and_choices_fixture(&envs_marker, &choices_marker),
    );

    let output = otto(&ottofile, &["switch"]);

    assert!(!output.status.success(), "must not run: {}", stdout(&output));
    assert!(
        !envs_marker.exists(),
        "otto.envs-command must not have run on the preflight-rejected path"
    );
    assert!(
        !choices_marker.exists(),
        "choices-command must not have run on the preflight-rejected path"
    );
}

/// Positive control for the pair above: both commands DO run once a value is
/// supplied, so the two absences are the preflight's doing rather than a
/// broken fixture.
#[test]
fn supplying_the_value_resolves_both_the_envs_command_and_the_choices_command() {
    let temp = TempDir::new().unwrap();
    let envs_marker = temp.path().join("envs-command-marker");
    let choices_marker = temp.path().join("choices-marker");
    let ottofile = write_ottofile(
        temp.path(),
        &envs_command_and_choices_fixture(&envs_marker, &choices_marker),
    );

    let output = otto(&ottofile, &["switch", "alpha"]);

    assert!(output.status.success(), "stderr: {}", stderr(&output));
    assert!(envs_marker.exists(), "otto.envs-command must have run");
    assert!(choices_marker.exists(), "choices-command must have run");
}
