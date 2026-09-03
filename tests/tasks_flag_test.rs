//! Integration tests for `otto --tasks` (design doc
//! docs/design/2026-08-28-boundary-fixes-and-dynamic-foreach.md, Phase 5).
//!
//! These spawn the real `otto` binary (assert_cmd) rather than calling
//! `Parser::parse()` in-process, because `--tasks` exits the process on
//! success/failure.

mod common;

use serde_json::Value as JsonValue;
use std::fs;
use tempfile::TempDir;

fn write_ottofile(dir: &std::path::Path, contents: &str) -> std::path::PathBuf {
    let path = dir.join("otto.yml");
    fs::write(&path, contents).unwrap();
    path
}

const FIXTURE: &str = r#"
otto:
  api: 1
  tasks: [up]

tasks:
  up:
    help: "Build + start each service in scope"
    foreach:
      items: [alpha, beta]
      parallel: false
    params:
      -s|--svc:
        help: "service name"
    bash: |
      touch "${OTTO_SENTINEL}"
      echo "ran up:${item}"

  down:
    help: "Stop each service"
    after: [up]
    bash: echo "down"
"#;

/// (a) piped default is JSON: `otto --tasks | jq -e 'type == "object" and (keys | length > 0)'`.
#[test]
fn tasks_piped_default_is_json_nonempty_object() {
    let temp = TempDir::new().unwrap();
    let ottofile = write_ottofile(temp.path(), FIXTURE);

    let output = common::otto_cmd(temp.path())
        .arg("--tasks")
        .arg("-o")
        .arg(&ottofile)
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    let json: JsonValue = serde_json::from_str(&stdout).expect("stdout must be valid JSON");
    let obj = json.as_object().expect("top-level value must be an object");
    assert!(!obj.is_empty(), "task map must not be empty");
}

/// (a) `--format yaml` emits YAML whose top-level keys are the same task names
/// as the piped JSON default (one logical shape, two encodings).
#[test]
fn tasks_yaml_format_has_same_keys_as_json_default() {
    let temp = TempDir::new().unwrap();
    let ottofile = write_ottofile(temp.path(), FIXTURE);

    let json_out = common::otto_cmd(temp.path())
        .arg("--tasks")
        .arg("-o")
        .arg(&ottofile)
        .output()
        .unwrap();
    assert!(json_out.status.success());
    let json_stdout = String::from_utf8_lossy(&json_out.stdout).to_string();
    let json: JsonValue = serde_json::from_str(&json_stdout).unwrap();
    let mut json_keys: Vec<String> = json.as_object().unwrap().keys().cloned().collect();
    json_keys.sort();

    let yaml_out = common::otto_cmd(temp.path())
        .arg("--tasks")
        .arg("--format")
        .arg("yaml")
        .arg("-o")
        .arg(&ottofile)
        .output()
        .unwrap();
    assert!(yaml_out.status.success());
    let yaml_stdout = String::from_utf8_lossy(&yaml_out.stdout).to_string();
    let yaml: serde_yaml_ng::Value = serde_yaml_ng::from_str(&yaml_stdout).expect("stdout must be valid YAML");
    let mut yaml_keys: Vec<String> = yaml
        .as_mapping()
        .expect("top-level value must be a mapping")
        .keys()
        .map(|k| k.as_str().unwrap().to_string())
        .collect();
    yaml_keys.sort();

    assert_eq!(json_keys, yaml_keys, "JSON and YAML must expose the identical key set");

    // Also prove the JSON default really is JSON and the yaml override really
    // is YAML (not just "both parse everything"): JSON starts with `{`.
    assert!(json_stdout.trim_start().starts_with('{'));
    assert!(!yaml_stdout.trim_start().starts_with('{'));
}

/// (b) subtask ids present in `subtasks` arrays, AND no builtin keys leak in.
#[test]
fn tasks_reports_subtask_ids_and_excludes_builtins() {
    let temp = TempDir::new().unwrap();
    let ottofile = write_ottofile(temp.path(), FIXTURE);

    let output = common::otto_cmd(temp.path())
        .arg("--tasks")
        .arg("-o")
        .arg(&ottofile)
        .output()
        .unwrap();
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    let json: JsonValue = serde_json::from_str(&stdout).unwrap();
    let obj = json.as_object().unwrap();

    let keys: Vec<&str> = obj.keys().map(String::as_str).collect();
    for builtin in otto::cli::BUILTIN_COMMANDS {
        assert!(
            !keys.contains(builtin),
            "builtin '{builtin}' leaked into --tasks: {keys:?}"
        );
    }

    let up = obj.get("up").expect("'up' task must be present");
    let subtasks: Vec<String> = up["subtasks"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap().to_string())
        .collect();
    assert_eq!(subtasks, vec!["up:alpha".to_string(), "up:beta".to_string()]);

    // Subtasks must not appear as separate top-level entries.
    assert!(!obj.contains_key("up:alpha"));
    assert!(!obj.contains_key("up:beta"));
}

/// (c) sentinel test: no task body runs during `--tasks`.
#[test]
fn tasks_executes_no_task_body() {
    let temp = TempDir::new().unwrap();
    let ottofile = write_ottofile(temp.path(), FIXTURE);
    let sentinel = temp.path().join("sentinel");

    let output = common::otto_cmd(temp.path())
        .arg("--tasks")
        .arg("-o")
        .arg(&ottofile)
        .env("OTTO_SENTINEL", &sentinel)
        .output()
        .unwrap();

    assert!(output.status.success());
    assert!(!sentinel.exists(), "--tasks must not execute any task body");
}

/// (c) stdout is parseable in the selected format even when a notice fires;
/// notices land on stderr only.
#[test]
fn tasks_notice_goes_to_stderr_stdout_stays_pure_data() {
    let temp = TempDir::new().unwrap();
    let fixture = r#"
otto:
  api: 1
  tasks: [orphan]

tasks:
  orphan:
    foreach:
      glob: "no-such-file-*.xyz"
    bash: echo "never runs"
"#;
    let ottofile = write_ottofile(temp.path(), fixture);

    let output = common::otto_cmd(temp.path())
        .arg("--tasks")
        .arg("-o")
        .arg(&ottofile)
        .output()
        .unwrap();

    assert!(output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("Notice:"), "expected a notice on stderr, got: {stderr}");

    let stdout = String::from_utf8_lossy(&output.stdout);
    let json: JsonValue = serde_json::from_str(&stdout).expect("stdout must stay parseable despite the notice");
    assert_eq!(json["orphan"]["subtasks"].as_array().unwrap().len(), 0);
}

/// TTY-branch verification: under a real pty, `--tasks` with no `--format`
/// override defaults to YAML, not JSON. Runs `otto` under `script -qec` so
/// stdout is genuinely a tty from the process's point of view.
#[test]
fn tasks_defaults_to_yaml_on_a_real_tty() {
    let temp = TempDir::new().unwrap();
    let ottofile = write_ottofile(temp.path(), FIXTURE);
    let ottofile_arg = ottofile.display().to_string();
    let mut cmd = common::pty_cmd(&[common::OTTO_BIN, "--tasks", "-o", &ottofile_arg]);
    let output = common::isolate(&mut cmd, temp.path())
        .output()
        .expect("failed to run `script` for the pty test");

    assert!(
        output.status.success(),
        "script exited non-zero; stdout: {}, stderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = common::pty_stdout(&output.stdout);
    // A tty must yield YAML: it must not parse as JSON's `{...}` object shape,
    // and it must parse as YAML with the same key set --tasks always reports.
    assert!(
        !stdout.trim_start().starts_with('{'),
        "expected YAML on a tty, got JSON-shaped output: {stdout}"
    );
    // On a parse failure, show the bytes. A pty run picks up whatever the host's
    // `script` writes around the child's output, and "control characters are not
    // allowed" with no bytes attached is unactionable from a CI log.
    let yaml: serde_yaml_ng::Value = serde_yaml_ng::from_str(&stdout).unwrap_or_else(|e| {
        panic!(
            "tty output must be valid YAML: {e}\nfirst 120 bytes: {:?}",
            &output.stdout[..output.stdout.len().min(120)]
        )
    });
    let mut keys: Vec<String> = yaml
        .as_mapping()
        .expect("top-level value must be a mapping")
        .keys()
        .map(|k| k.as_str().unwrap().to_string())
        .collect();
    keys.sort();
    assert_eq!(keys, vec!["down".to_string(), "up".to_string()]);
}
