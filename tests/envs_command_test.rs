//! Integration tests for `otto.envs-command` and the `global_envs()` cwd
//! contract (design doc
//! `docs/design/2026-08-31-buffered-foreach-computed-envs-required-params.md`,
//! Phase 2).
//!
//! These spawn the real `otto` binary: the contract is about *where* a
//! command runs, *when* it runs, and *what environment* the task body
//! finally sees, none of which is observable from inside the process (the
//! help paths call `std::process::exit`, and a marker file must be touched by
//! a real subprocess).

mod common;

use std::fs;
use std::path::Path;
use std::process::Output;
use tempfile::TempDir;

fn write_ottofile(dir: &Path, contents: &str) -> std::path::PathBuf {
    let path = dir.join("otto.yml");
    fs::write(&path, contents).unwrap();
    path
}

/// Run otto with `-o <ottofile>` from a process cwd that is NOT the
/// ottofile's directory. `-o` is the shape the cwd defect was verified
/// failing on, so it is this file's default.
fn otto_from(cwd: &Path, ottofile: &Path, args: &[&str]) -> Output {
    let home = ottofile.parent().expect("ottofile must live in a directory");
    common::otto_cmd(home)
        .current_dir(cwd)
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

/// A relative-path helper script beside the ottofile, exactly the shape
/// otto-dev's `envs:` block uses (`$(scripts/svc.sh root philo)`).
fn write_svc_script(dir: &Path) {
    let scripts = dir.join("scripts");
    fs::create_dir_all(&scripts).unwrap();
    let script = scripts.join("svc.sh");
    fs::write(&script, "#!/bin/sh\necho /srv/philo\n").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&script, fs::Permissions::from_mode(0o755)).unwrap();
    }
}

/// `envs:` with a relative command, the otto-dev shape. Verified on main to
/// fail with `sh: 1: scripts/svc.sh: not found`, exit 127, on every
/// invocation whose process cwd is not the ottofile's directory.
const RELATIVE_ENVS_FIXTURE: &str = r#"
otto:
  api: 1
  envs:
    PHILO_ROOT: "$(scripts/svc.sh root philo)"

tasks:
  profiles:
    bash: |
      echo "PHILO_ROOT=[${PHILO_ROOT}]"
"#;

// ----------------------------------------------------------------------
// (e) command sources share one cwd: `envs:` `$(...)` resolves against the
//     ottofile's directory, not the process cwd.
// ----------------------------------------------------------------------

/// The `-o`-from-elsewhere case. This is the break-the-code check the design
/// doc names: reverting `global_envs()` to `self.cwd` must fail here.
#[test]
fn envs_substitution_resolves_against_the_ottofile_dir_under_dash_o() {
    let temp = TempDir::new().unwrap();
    let elsewhere = TempDir::new().unwrap();
    write_svc_script(temp.path());
    let ottofile = write_ottofile(temp.path(), RELATIVE_ENVS_FIXTURE);

    let output = otto_from(elsewhere.path(), &ottofile, &["profiles"]);

    assert!(
        output.status.success(),
        "relative `envs:` command must resolve against the ottofile dir; stderr: {}",
        stderr(&output)
    );
    assert!(
        stdout(&output).contains("PHILO_ROOT=[/srv/philo]"),
        "stdout: {}\nstderr: {}",
        stdout(&output),
        stderr(&output)
    );
}

/// The no-flag shape: plain `otto <task>` from a SUBDIRECTORY, where upward
/// discovery finds the ottofile but the process cwd is the subdirectory.
/// Verified failing with exit 127 on main.
#[test]
fn envs_substitution_resolves_from_a_subdirectory_with_no_flags() {
    let temp = TempDir::new().unwrap();
    write_svc_script(temp.path());
    write_ottofile(temp.path(), RELATIVE_ENVS_FIXTURE);
    let sub = temp.path().join("deep/nested");
    fs::create_dir_all(&sub).unwrap();

    let output = common::otto_cmd(temp.path())
        .current_dir(&sub)
        .arg("profiles")
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "plain `otto <task>` from a subdirectory must resolve the relative command; stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        String::from_utf8_lossy(&output.stdout).contains("PHILO_ROOT=[/srv/philo]"),
        "stdout: {}",
        String::from_utf8_lossy(&output.stdout)
    );
}

/// `-C` aimed anywhere but the ottofile's own directory: the third of the
/// four shapes the defect was verified on. `-C` changes the process cwd, so
/// before the fix this failed the same way `-o` did.
#[test]
fn envs_substitution_resolves_when_dash_c_points_elsewhere() {
    let temp = TempDir::new().unwrap();
    let elsewhere = TempDir::new().unwrap();
    write_svc_script(temp.path());
    let ottofile = write_ottofile(temp.path(), RELATIVE_ENVS_FIXTURE);

    let output = common::otto_cmd(temp.path())
        .arg("-C")
        .arg(elsewhere.path())
        .arg("-o")
        .arg(&ottofile)
        .arg("profiles")
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "`-C` elsewhere must not change where a relative `envs:` command runs; stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        String::from_utf8_lossy(&output.stdout).contains("PHILO_ROOT=[/srv/philo]"),
        "stdout: {}",
        String::from_utf8_lossy(&output.stdout)
    );
}

// ----------------------------------------------------------------------
// (a) the command's KEY=VALUE stdout reaches task bodies and foreach.command
// ----------------------------------------------------------------------

/// The design doc's acceptance-criteria fixture, verbatim, plus the two other
/// consumers of `global_envs()`: a `foreach.command` and (below) a
/// `choices-command`.
const COMPUTED_FIXTURE: &str = r#"
otto:
  api: 1
  envs-command: "printf 'FOO=bar\nBAZ=qux\n'"

tasks:
  show:
    bash: |
      echo "FOO=[${FOO}] BAZ=[${BAZ}]"

  each:
    foreach:
      command: 'printf "%s\n" "$FOO" "$BAZ"'
      as: item
      parallel: false
    bash: |
      echo "item=[${item}]"
"#;

#[test]
fn computed_envs_reach_a_task_body() {
    let temp = TempDir::new().unwrap();
    let ottofile = write_ottofile(temp.path(), COMPUTED_FIXTURE);

    let output = otto_from(temp.path(), &ottofile, &["show"]);

    assert!(output.status.success(), "stderr: {}", stderr(&output));
    assert!(
        stdout(&output).contains("FOO=[bar] BAZ=[qux]"),
        "stdout: {}",
        stdout(&output)
    );
}

/// `foreach.command` is handed `global_envs()`, so it sees the computed
/// variables too - the second of the three consumers.
#[test]
fn computed_envs_reach_a_foreach_command() {
    let temp = TempDir::new().unwrap();
    let ottofile = write_ottofile(temp.path(), COMPUTED_FIXTURE);

    let output = otto_from(temp.path(), &ottofile, &["each"]);

    assert!(output.status.success(), "stderr: {}", stderr(&output));
    let out = stdout(&output);
    assert!(out.contains("item=[bar]"), "stdout: {out}");
    assert!(out.contains("item=[qux]"), "stdout: {out}");
}

/// An ottofile with `envs-command` and no literal `envs:` at all still gets
/// the computed map: the both-empty fast path must not swallow it.
#[test]
fn computed_envs_work_without_any_literal_envs_block() {
    let temp = TempDir::new().unwrap();
    let ottofile = write_ottofile(
        temp.path(),
        r#"
otto:
  api: 1
  envs-command: "printf 'ONLY=computed\n'"

tasks:
  show:
    bash: echo "ONLY=[${ONLY}]"
"#,
    );

    let output = otto_from(temp.path(), &ottofile, &["show"]);

    assert!(output.status.success(), "stderr: {}", stderr(&output));
    assert!(
        stdout(&output).contains("ONLY=[computed]"),
        "stdout: {}",
        stdout(&output)
    );
}

/// Whitespace inside a value survives the whole pipeline, which is what the
/// `run_command_stdout` extraction bought: `run_lines_command`'s per-line
/// `str::trim` would have eaten it.
#[test]
fn whitespace_in_a_computed_value_survives() {
    let temp = TempDir::new().unwrap();
    let ottofile = write_ottofile(
        temp.path(),
        r#"
otto:
  api: 1
  envs-command: "printf 'PADDED=  spaced value  \n'"

tasks:
  show:
    bash: echo "PADDED=[${PADDED}]"
"#,
    );

    let output = otto_from(temp.path(), &ottofile, &["show"]);

    assert!(output.status.success(), "stderr: {}", stderr(&output));
    assert!(
        stdout(&output).contains("PADDED=[  spaced value  ]"),
        "stdout: {}",
        stdout(&output)
    );
}

// ----------------------------------------------------------------------
// (b) a literal `envs:` entry beats the command's value for the same key
// ----------------------------------------------------------------------

#[test]
fn a_literal_envs_entry_beats_the_computed_one() {
    let temp = TempDir::new().unwrap();
    let ottofile = write_ottofile(
        temp.path(),
        r#"
otto:
  api: 1
  envs-command: "printf 'FOO=computed\nBAZ=qux\n'"
  envs:
    FOO: explicit

tasks:
  show:
    bash: echo "FOO=[${FOO}] BAZ=[${BAZ}]"
"#,
    );

    let output = otto_from(temp.path(), &ottofile, &["show"]);

    assert!(output.status.success(), "stderr: {}", stderr(&output));
    let out = stdout(&output);
    assert!(out.contains("FOO=[explicit]"), "the literal must win; stdout: {out}");
    assert!(
        out.contains("BAZ=[qux]"),
        "an unshadowed computed key must survive; stdout: {out}"
    );
}

// ----------------------------------------------------------------------
// (c) laziness: never for `--help`, run for a real run
// ----------------------------------------------------------------------

fn marker_fixture(marker: &Path) -> String {
    format!(
        r#"
otto:
  api: 1
  envs-command: "touch {marker} && printf 'FOO=bar\n'"

tasks:
  show:
    bash: echo "FOO=[${{FOO}}]"
"#,
        marker = marker.display(),
    )
}

#[test]
fn help_does_not_run_the_envs_command() {
    let temp = TempDir::new().unwrap();
    let marker = temp.path().join("envs-command-marker");
    let ottofile = write_ottofile(temp.path(), &marker_fixture(&marker));

    let output = otto_from(temp.path(), &ottofile, &["--help"]);

    assert!(output.status.success(), "stderr: {}", stderr(&output));
    assert!(!marker.exists(), "`--help` must never run the envs-command");
}

/// Positive control for the assertion above: the same fixture, actually run.
#[test]
fn a_real_run_does_run_the_envs_command() {
    let temp = TempDir::new().unwrap();
    let marker = temp.path().join("envs-command-marker");
    let ottofile = write_ottofile(temp.path(), &marker_fixture(&marker));

    let output = otto_from(temp.path(), &ottofile, &["show"]);

    assert!(output.status.success(), "stderr: {}", stderr(&output));
    assert!(marker.exists(), "a real run must resolve the env map");
    assert!(stdout(&output).contains("FOO=[bar]"), "stdout: {}", stdout(&output));
}

/// Once per invocation, not once per asking surface: two tasks that both read
/// the map, plus a `foreach.command`, still run the command exactly once. The
/// `OnceCell` in `DynamicResolver` is what gives this; the command counts its
/// own executions by appending a line.
#[test]
fn the_envs_command_runs_at_most_once_per_invocation() {
    let temp = TempDir::new().unwrap();
    let counter = temp.path().join("runs");
    let ottofile = write_ottofile(
        temp.path(),
        &format!(
            r#"
otto:
  api: 1
  envs-command: "echo run >> {counter} && printf 'FOO=bar\n'"

tasks:
  each:
    foreach:
      command: 'printf "%s\n" "$FOO"'
      as: item
      parallel: false
    bash: echo "item=[${{item}}]"

  show:
    after: [each]
    bash: echo "FOO=[${{FOO}}]"
"#,
            counter = counter.display(),
        ),
    );

    let output = otto_from(temp.path(), &ottofile, &["show"]);

    assert!(output.status.success(), "stderr: {}", stderr(&output));
    let runs = fs::read_to_string(&counter).unwrap();
    assert_eq!(runs.lines().count(), 1, "envs-command ran more than once: {runs:?}");
}

// ----------------------------------------------------------------------
// (d) invalid output fails the load, naming the line
// ----------------------------------------------------------------------

#[test]
fn a_line_that_is_not_key_equals_value_fails_naming_the_line() {
    let temp = TempDir::new().unwrap();
    let ottofile = write_ottofile(
        temp.path(),
        r#"
otto:
  api: 1
  envs-command: "printf 'not-a-kv\n'"

tasks:
  show:
    bash: echo hi
"#,
    );

    let output = otto_from(temp.path(), &ottofile, &["show"]);

    assert!(!output.status.success(), "stdout: {}", stdout(&output));
    let err = stderr(&output);
    assert!(err.contains("otto.envs-command"), "must name the key, got: {err}");
    assert!(err.contains("line 1"), "must name the line number, got: {err}");
    assert!(err.contains("not-a-kv"), "must quote the line, got: {err}");
}

#[test]
fn an_invalid_key_fails_naming_the_line() {
    let temp = TempDir::new().unwrap();
    let ottofile = write_ottofile(
        temp.path(),
        r#"
otto:
  api: 1
  envs-command: "printf 'GOOD=1\n1BAD=2\n'"

tasks:
  show:
    bash: echo hi
"#,
    );

    let output = otto_from(temp.path(), &ottofile, &["show"]);

    assert!(!output.status.success(), "stdout: {}", stdout(&output));
    let err = stderr(&output);
    assert!(err.contains("line 2"), "must name the line number, got: {err}");
    assert!(err.contains("1BAD"), "must name the offending key, got: {err}");
}

/// A non-zero exit is loud, naming the command and its stderr - the same
/// contract `foreach.command` and `choices-command` already have.
#[test]
fn a_failing_envs_command_is_a_loud_error() {
    let temp = TempDir::new().unwrap();
    let ottofile = write_ottofile(
        temp.path(),
        r#"
otto:
  api: 1
  envs-command: "echo boom >&2; exit 7"

tasks:
  show:
    bash: echo hi
"#,
    );

    let output = otto_from(temp.path(), &ottofile, &["show"]);

    assert!(!output.status.success(), "stdout: {}", stdout(&output));
    let err = stderr(&output);
    assert!(err.contains("otto.envs-command"), "got: {err}");
    assert!(err.contains("exit code 7"), "got: {err}");
    assert!(err.contains("boom"), "got: {err}");
}

// ----------------------------------------------------------------------
// (f) layering, not merging: a shadowing self-reference sees the COMPUTED
//     value, not the fallback and not the OS value
// ----------------------------------------------------------------------

#[test]
fn a_shadowing_self_reference_reads_the_computed_value() {
    let temp = TempDir::new().unwrap();
    let ottofile = write_ottofile(
        temp.path(),
        r#"
otto:
  api: 1
  envs-command: "printf 'FOO=computed\n'"
  envs:
    FOO: '$(echo "${FOO:-fallback}")'

tasks:
  show:
    bash: echo "FOO=[${FOO}]"
"#,
    );

    // The OS value is set too, so a merge (which discards the computed value)
    // would show `from-the-os` and a missing layer would show `fallback`.
    // Only layering gives `computed`.
    let home = ottofile.parent().unwrap();
    let output = common::otto_cmd(home)
        .current_dir(temp.path())
        .env("FOO", "from-the-os")
        .arg("-o")
        .arg(&ottofile)
        .arg("show")
        .output()
        .unwrap();

    assert!(output.status.success(), "stderr: {}", stderr(&output));
    assert!(
        stdout(&output).contains("FOO=[computed]"),
        "stdout: {}",
        stdout(&output)
    );
}

// ----------------------------------------------------------------------
// (g) the third consumer of global_envs(): a `choices-command`
// ----------------------------------------------------------------------

const CHOICES_FIXTURE: &str = r#"
otto:
  api: 1
  envs-command: "printf 'ALLOWED=alpha\n'"

tasks:
  sw:
    params:
      svc:
        choices-command: 'printf "%s\n" "$ALLOWED"'
    bash: echo "svc=[${svc}]"
"#;

#[test]
fn a_choices_command_reads_a_computed_variable() {
    let temp = TempDir::new().unwrap();
    let ottofile = write_ottofile(temp.path(), CHOICES_FIXTURE);

    let accepted = otto_from(temp.path(), &ottofile, &["sw", "alpha"]);
    assert!(accepted.status.success(), "stderr: {}", stderr(&accepted));
    assert!(
        stdout(&accepted).contains("svc=[alpha]"),
        "stdout: {}",
        stdout(&accepted)
    );

    let rejected = otto_from(temp.path(), &ottofile, &["sw", "beta"]);
    assert!(!rejected.status.success(), "stdout: {}", stdout(&rejected));
    assert!(
        stderr(&rejected).contains("alpha"),
        "the rejection must name the computed value set, got: {}",
        stderr(&rejected)
    );
}

// ----------------------------------------------------------------------
// The command runs in the ottofile's directory, like every other command
// source. Same contract the cwd fix above established for `envs:`.
// ----------------------------------------------------------------------

#[test]
fn the_envs_command_runs_in_the_ottofile_directory() {
    let temp = TempDir::new().unwrap();
    let elsewhere = TempDir::new().unwrap();
    write_svc_script(temp.path());
    let ottofile = write_ottofile(
        temp.path(),
        r#"
otto:
  api: 1
  envs-command: "printf 'ROOT=%s\n' \"$(scripts/svc.sh root philo)\""

tasks:
  show:
    bash: echo "ROOT=[${ROOT}]"
"#,
    );

    let output = otto_from(elsewhere.path(), &ottofile, &["show"]);

    assert!(output.status.success(), "stderr: {}", stderr(&output));
    assert!(
        stdout(&output).contains("ROOT=[/srv/philo]"),
        "stdout: {}",
        stdout(&output)
    );
}
