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
