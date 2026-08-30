//! Regression tests for the containment defects in
//! docs/design/2026-06-10-code-review-remediation.md Phase 3.
//!
//! Four generators templated values straight into the script they generate, so
//! an env value, a parameter, a foreach item and a dynamic `choices-command`
//! value were each a shell injection; a foreach item containing path separators
//! became a directory name and wrote outside the run's `tasks/` tree. Every test
//! here runs the real binary with a payload that creates a marker file, and
//! asserts the marker was NOT created.

mod common;

use common::otto_std_cmd;
use std::fs;
use std::path::{Path, PathBuf};

use tempfile::TempDir;

/// Run the real binary in `dir` isolated through `common::otto_std_cmd`, which
/// pins `OTTO_HOME` and removes any inherited `OTTO_DB_PATH` (`OTTO_HOME` alone
/// is not isolation: `OTTO_DB_PATH` wins over it). Returns
/// (exit code, stdout, stderr).
fn run_otto(dir: &Path, otto_home: &Path, args: &[&str]) -> (i32, String, String) {
    let output = otto_std_cmd(otto_home)
        .current_dir(dir)
        .env_remove("OTTOFILE")
        .args(args)
        .output()
        .expect("failed to run otto");

    (
        output.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&output.stdout).to_string(),
        String::from_utf8_lossy(&output.stderr).to_string(),
    )
}

/// A project directory with `otto.yml` written from `body`, its own `OTTO_HOME`,
/// and the path a successful injection would create.
struct Fixture {
    _dir: TempDir,
    project: PathBuf,
    otto_home: PathBuf,
    marker: PathBuf,
}

impl Fixture {
    fn new(body_template: &str) -> Self {
        let dir = TempDir::new().expect("tempdir");
        let project = dir.path().join("project");
        let otto_home = dir.path().join("otto-home");
        let marker = dir.path().join("PWNED");
        fs::create_dir_all(&project).expect("create project");
        fs::create_dir_all(&otto_home).expect("create otto home");

        let body = body_template.replace("{MARKER}", marker.to_str().expect("utf-8 tempdir"));
        fs::write(project.join("otto.yml"), body).expect("write ottofile");

        Self {
            _dir: dir,
            project,
            otto_home,
            marker,
        }
    }

    fn run(&self, args: &[&str]) -> (i32, String, String) {
        run_otto(&self.project, &self.otto_home, args)
    }

    fn marker_arg(&self, template: &str) -> String {
        template.replace("{MARKER}", self.marker.to_str().expect("utf-8 tempdir"))
    }

    fn assert_not_pwned(&self, what: &str, stdout: &str) {
        assert!(
            !self.marker.exists(),
            "{what}: the payload executed and created {}\nstdout:\n{stdout}",
            self.marker.display()
        );
    }
}

/// The design doc's acceptance criterion, verbatim: an ottofile env value that
/// closes the generated `export PAYLOAD="..."` and runs `touch`.
#[test]
fn an_env_value_reaches_the_task_as_literal_text() {
    let fixture = Fixture::new(
        r#"
otto:
  name: injection
  envs:
    PAYLOAD: 'x"; touch {MARKER}; echo "y'
tasks:
  hello:
    bash: |
      echo "payload=$PAYLOAD"
"#,
    );

    let (code, stdout, stderr) = fixture.run(&["hello"]);

    assert_eq!(code, 0, "stdout:\n{stdout}\nstderr:\n{stderr}");
    fixture.assert_not_pwned("env value", &stdout);
    assert!(
        stdout.contains(r#"payload=x"; touch"#),
        "the value must arrive whole, as text:\n{stdout}"
    );
}

/// The same payload class through the *other* script generator: `python:`
/// sugar builds `os.environ['PAYLOAD'] = '...'` via `python_quote`, a
/// separate quoting function from `bash_quote`. A payload that breaks out of
/// a Python single-quoted string (unescaped `'`) would let the rest of the
/// line run as Python source rather than sit inside the string.
#[test]
fn an_env_value_reaches_a_python_task_as_literal_text() {
    let fixture = Fixture::new(
        r#"
otto:
  name: injection
  envs:
    PAYLOAD: "x'; import os; os.system('touch {MARKER}'); y = 'z"
tasks:
  hello:
    action: |
      #!/usr/bin/env python3
      import os
      print("payload=" + os.environ["PAYLOAD"])
"#,
    );

    let (code, stdout, stderr) = fixture.run(&["hello"]);

    assert_eq!(code, 0, "stdout:\n{stdout}\nstderr:\n{stderr}");
    fixture.assert_not_pwned("env value (python generator)", &stdout);
    assert!(
        stdout.contains("payload=x'; import os; os.system"),
        "the value must arrive whole, as text:\n{stdout}"
    );
}

/// Same payload through `--name`, which lands in the parameter generator.
#[test]
fn a_param_value_reaches_the_task_as_literal_text() {
    let fixture = Fixture::new(
        r#"
otto:
  name: injection
tasks:
  hello:
    params:
      --name:
        default: world
    bash: |
      echo "name=$name"
"#,
    );

    let payload = fixture.marker_arg(r#"x"; touch {MARKER}; echo "y"#);
    let (code, stdout, stderr) = fixture.run(&["hello", "--name", &payload]);

    assert_eq!(code, 0, "stdout:\n{stdout}\nstderr:\n{stderr}");
    fixture.assert_not_pwned("param value", &stdout);
    assert!(
        stdout.contains(r#"name=x"; touch"#),
        "the value must arrive whole, as text:\n{stdout}"
    );
}

/// A `foreach: command:` item is command output, so it is data twice over.
#[test]
fn a_foreach_item_reaches_the_task_as_literal_text() {
    let fixture = Fixture::new(
        r#"
otto:
  name: injection
tasks:
  fe:
    foreach:
      command: 'printf ''a"b\n'''
      as: pkg
    bash: |
      echo "pkg=$pkg"
"#,
    );

    let (code, stdout, stderr) = fixture.run(&["fe"]);

    assert_eq!(code, 0, "stdout:\n{stdout}\nstderr:\n{stderr}");
    assert!(
        stdout.contains(r#"pkg=a"b"#),
        "the item must arrive whole, as text:\n{stdout}\n{stderr}"
    );
}

/// A `choices-command` value, the fourth generator input, with the doc's payload.
#[test]
fn a_dynamic_choices_value_reaches_the_task_as_literal_text() {
    let fixture = Fixture::new(
        r#"
otto:
  name: injection
tasks:
  ch:
    params:
      --pick:
        choices-command: 'printf ''a";touch {MARKER};echo"b\n'''
    bash: |
      echo "pick=$pick"
"#,
    );

    let payload = fixture.marker_arg(r#"a";touch {MARKER};echo"b"#);
    let (code, stdout, stderr) = fixture.run(&["ch", "--pick", &payload]);

    assert_eq!(code, 0, "stdout:\n{stdout}\nstderr:\n{stderr}");
    fixture.assert_not_pwned("choices value", &stdout);
    assert!(
        stdout.contains(r#"pick=a";touch"#),
        "the value must arrive whole, as text:\n{stdout}"
    );
}

/// A foreach item containing `/` became the subtask's directory name, so
/// `../../../ESCAPED` wrote a directory beside the run instead of inside its
/// `tasks/` tree.
#[test]
fn a_foreach_item_cannot_name_a_directory_outside_tasks() {
    let fixture = Fixture::new(
        r#"
otto:
  name: injection
tasks:
  fe:
    foreach:
      command: 'printf ''../../../ESCAPED\n'''
      as: pkg
    bash: |
      echo "pkg=$pkg"
"#,
    );

    let (code, stdout, stderr) = fixture.run(&["fe"]);
    assert_eq!(code, 0, "stdout:\n{stdout}\nstderr:\n{stderr}");

    // Everything the run created lives under the run's own tasks/ directory.
    let mut escaped: Vec<PathBuf> = Vec::new();
    let mut stack = vec![fixture.otto_home.clone()];
    while let Some(dir) = stack.pop() {
        for entry in fs::read_dir(&dir).expect("read otto home") {
            let entry = entry.expect("dir entry");
            let path = entry.path();
            if path.file_name().and_then(|n| n.to_str()) == Some("ESCAPED") {
                escaped.push(path.clone());
            }
            if entry.file_type().expect("file type").is_dir() {
                stack.push(path);
            }
        }
    }
    assert!(
        escaped.is_empty(),
        "a foreach item escaped its tasks/ tree: {escaped:?}"
    );

    // The item still reaches the script intact.
    assert!(
        stdout.contains("pkg=../../../ESCAPED"),
        "the item must arrive whole, as text:\n{stdout}\n{stderr}"
    );
}

/// Six runs of one unchanged ottofile produced six cache files, because the
/// generators walked a HashMap.
#[test]
fn repeated_runs_of_one_ottofile_produce_one_cache_entry() {
    let fixture = Fixture::new(
        r#"
otto:
  name: injection
  envs:
    A: one
    B: two
    C: three
    D: four
    E: five
    F: six
tasks:
  hello:
    bash: |
      echo "$A$B$C$D$E$F"
"#,
    );

    for _ in 0..6 {
        let (code, stdout, stderr) = fixture.run(&["hello"]);
        assert_eq!(code, 0, "stdout:\n{stdout}\nstderr:\n{stderr}");
    }

    let mut caches: Vec<PathBuf> = Vec::new();
    for project in fs::read_dir(&fixture.otto_home).expect("read otto home") {
        let cache_dir = project.expect("dir entry").path().join(".cache");
        if let Ok(entries) = fs::read_dir(&cache_dir) {
            caches.extend(entries.map(|e| e.expect("cache entry").path()));
        }
    }

    assert_eq!(caches.len(), 1, "one script, one cache entry, got: {caches:?}");
}
