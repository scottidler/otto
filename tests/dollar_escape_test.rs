//! End-to-end coverage for `$$`, the literal-dollar escape, and for the
//! interpolation boundary around it.
//!
//! `$$` was implemented and explained only inside a design doc, so no user of
//! otto could find it and nothing pinned it. Every assertion here corresponds to
//! a claim on the "Variable interpolation" section of
//! `docs/commands/ottofile-reference.md`; if one of these goes red, that section
//! is wrong and must change with it.
//!
//! The `action:` case is the one that matters most. Writing that doc, the first
//! draft asserted `awk '{print $$1}'` worked inside an action. It does not:
//! action bodies reach the shell verbatim, so the doubled form is wrong there.
//! The claim was caught by running it, which is why it is a test now.

mod common;

use assert_cmd::Command;
use std::path::Path;
use tempfile::TempDir;

fn otto_cmd(work_dir: &Path, ottofile: &Path) -> Command {
    let mut cmd = common::otto_cmd(&work_dir.join(".otto"));
    cmd.current_dir(work_dir);
    cmd.arg("--ottofile").arg(ottofile);
    cmd
}

fn write_ottofile(dir: &Path, body: &str) -> std::path::PathBuf {
    let ottofile = dir.join("otto.yml");
    std::fs::write(&ottofile, body).expect("failed to write ottofile");
    ottofile
}

/// The four forms the reference documents, in one run: `$$` is a literal `$`,
/// it beats `${...}` and `$(...)`, and an unterminated `${` is literal text.
#[test]
fn dollar_dollar_is_a_literal_dollar_and_beats_every_other_form() {
    let temp = TempDir::new().expect("tempdir");
    let ottofile = write_ottofile(
        temp.path(),
        r#"
otto:
  api: 1
  envs:
    PRICE: "$$4.99"
    BRACED: "$${VAR}"
    CMDSUB: "$$(echo hi)"
    UNTERM: "${"
tasks:
  show:
    bash: |
      echo "PRICE=[$PRICE]"
      echo "BRACED=[$BRACED]"
      echo "CMDSUB=[$CMDSUB]"
      echo "UNTERM=[$UNTERM]"
"#,
    );

    let output = otto_cmd(temp.path(), &ottofile).arg("show").output().expect("otto run");
    let stdout = String::from_utf8_lossy(&output.stdout).to_string();

    assert!(output.status.success(), "otto show failed:\n{stdout}");
    // A literal dollar reaches the task, and bash does not then expand it.
    assert!(
        stdout.contains("PRICE=[$4.99]"),
        "`$$4.99` must reach the task as `$4.99`:\n{stdout}"
    );
    // `$$` is consumed first, so the braces are text, not a reference. `VAR` is
    // not even declared: if this expanded, the run would have failed instead.
    assert!(
        stdout.contains("BRACED=[${VAR}]"),
        "`$${{VAR}}` must be a literal `$` then `{{VAR}}`:\n{stdout}"
    );
    // Same for command substitution: otto must not run `echo hi`.
    assert!(
        stdout.contains("CMDSUB=[$(echo hi)]"),
        "`$$(echo hi)` must not be a command substitution:\n{stdout}"
    );
    assert!(
        !stdout.contains("CMDSUB=[hi]"),
        "`$$(echo hi)` ran the command:\n{stdout}"
    );
    // An unterminated `${` is literal text rather than an error.
    assert!(
        stdout.contains("UNTERM=[${]"),
        "an unterminated `${{` must pass through literally:\n{stdout}"
    );
}

/// `action:` bodies are handed to the shell verbatim. `$$` there is bash's PID,
/// not an escape, so the reference tells users to write `$1` and not `$$1`.
///
/// Both halves are asserted, because the discriminating fact is the *difference*
/// between the two: a build that started interpolating action bodies would make
/// these two agree, and that is the regression this test exists to catch.
#[test]
fn action_bodies_are_not_interpolated_so_dollar_dollar_is_not_an_escape_there() {
    let temp = TempDir::new().expect("tempdir");
    // Field 1 is a NUMBER on purpose. `$$1` is awk's `$($1)`, and that is only
    // well defined across awks when field 1 reads as a field index: gawk coerces
    // a non-numeric "alpha" to 0 and prints `$0`, while the BWK awk macOS ships
    // rejects it outright ("illegal field $(alpha)"). With `2` there, both awks
    // agree that `$($1)` is field 2, and the two task bodies still differ
    // observably, which is the whole point of the test.
    std::fs::write(temp.path().join("data.txt"), "2 alpha beta\n2 gamma delta\n").expect("write data");
    let ottofile = write_ottofile(
        temp.path(),
        r#"
otto:
  api: 1
tasks:
  doubled:
    bash: awk '{print $$1}' data.txt
  single:
    bash: awk '{print $1}' data.txt
"#,
    );

    let doubled = otto_cmd(temp.path(), &ottofile)
        .arg("doubled")
        .output()
        .expect("otto doubled");
    let doubled_out = String::from_utf8_lossy(&doubled.stdout).to_string();
    let single = otto_cmd(temp.path(), &ottofile)
        .arg("single")
        .output()
        .expect("otto single");
    let single_out = String::from_utf8_lossy(&single.stdout).to_string();

    // Both stdout AND stderr: otto sends task failure detail to stderr, so a
    // bare stdout dump reports an empty string for exactly the case that needs
    // explaining.
    assert!(
        doubled.status.success(),
        "doubled failed:\nstdout:\n{doubled_out}\nstderr:\n{}",
        String::from_utf8_lossy(&doubled.stderr)
    );
    assert!(
        single.status.success(),
        "single failed:\nstdout:\n{single_out}\nstderr:\n{}",
        String::from_utf8_lossy(&single.stderr)
    );

    // `$1` is the first field, which is what a user wants.
    assert!(single_out.contains('2'), "`$1` must print field 1:\n{single_out}");
    assert!(
        !single_out.contains("alpha"),
        "`$1` must print field 1 and nothing else:\n{single_out}"
    );

    // `$$1` reaches awk verbatim and is read as `$($1)`, so it prints field 2.
    // If otto ever interpolated action bodies, `$$` would collapse to `$` and
    // this would print field 1, matching `single` instead. The difference is
    // the assertion.
    assert!(
        doubled_out.contains("alpha"),
        "`$$1` in an action must reach awk verbatim, not be escaped by otto:\n{doubled_out}"
    );
    assert_ne!(
        doubled_out.trim(),
        single_out.trim(),
        "an interpolated action body would make these agree; they must not"
    );
}

/// Otto does not implement shell parameter expansion, so `${VAR:-default}` is
/// read as a variable *named* `VAR:-default` and fails naming it. The reference
/// quotes this message; the test pins the part of it a user would search for.
#[test]
fn shell_style_defaults_are_not_supported_and_the_error_names_the_whole_reference() {
    let temp = TempDir::new().expect("tempdir");
    let ottofile = write_ottofile(
        temp.path(),
        r#"
otto:
  api: 1
  envs:
    G: "${MYVAR:-fallback}"
tasks:
  show:
    bash: echo "$G"
"#,
    );

    let output = otto_cmd(temp.path(), &ottofile).arg("show").output().expect("otto run");
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();

    assert!(
        !output.status.success(),
        "an unresolved reference must not exit 0:\n{stderr}"
    );
    assert!(
        stderr.contains("Environment variable 'MYVAR:-fallback' not found"),
        "the error must name the whole reference so the user sees why:\n{stderr}"
    );
}

/// The escape hatch the reference points users at instead: defer the default to
/// the shell inside a command substitution, which otto passes through.
#[test]
fn a_command_substitution_carries_a_shell_default_through() {
    let temp = TempDir::new().expect("tempdir");
    let ottofile = write_ottofile(
        temp.path(),
        r#"
otto:
  api: 1
  envs:
    G: '$(echo "${MYVAR:-fallback}")'
tasks:
  show:
    bash: echo "G=[$G]"
"#,
    );

    let output = otto_cmd(temp.path(), &ottofile).arg("show").output().expect("otto run");
    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    assert!(output.status.success(), "otto show failed:\n{stdout}");
    assert!(
        stdout.contains("G=[fallback]"),
        "the shell default must resolve:\n{stdout}"
    );
}
