//! What a run actually records about itself, read back through `otto History`.
//!
//! Two columns had production writers that passed a literal `None`, so they were
//! always NULL in every real database while `docs/history.md`'s JSON example
//! promised one of them. Both are checked here end to end - a real run, then
//! `History --json` - because a unit test on the store only proves the column
//! round-trips, not that anything ever fills it.
//!
//! `docs/design/2026-09-02-second-code-review-remediation.md` Phase 9.

mod common;

use common::otto_std_cmd;
use serde_json::Value as JsonValue;
use std::fs;
use std::path::Path;
use tempfile::TempDir;

const OTTOFILE: &str = "otto:\n  api: 1\n  tasks: [build]\ntasks:\n  build:\n    action: echo built\n";

/// Run `otto History <args>` and parse its JSON.
fn history_json(project: &Path, home: &Path, args: &[&str]) -> JsonValue {
    let output = otto_std_cmd(home)
        .current_dir(project)
        .env_remove("OTTOFILE")
        .args(["History"])
        .args(args)
        .args(["--json"])
        .output()
        .expect("failed to run otto History");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success(),
        "otto History exited {}\nstderr:\n{}",
        output.status,
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_str(&stdout).unwrap_or_else(|e| panic!("History --json must emit JSON ({e}); got:\n{stdout}"))
}

/// A run records the host it ran on, and its tasks record the hash of the
/// script that ran.
///
/// `workspace.rs` passed `None // hostname not in ExecutionContext yet` while
/// `RunMetadata::current_system_info()` existed for exactly this and had no
/// production caller at all. `task_execution.rs` passed `None // TODO` for
/// `script_hash` while the `ProcessedAction::Bash { hash, .. }` it was
/// destructuring in the same match carried the hash.
#[test]
fn a_run_records_its_hostname_and_its_tasks_record_a_script_hash() {
    let dir = TempDir::new().expect("tempdir");
    let project = dir.path().join("project");
    let home = dir.path().join("otto-home");
    fs::create_dir_all(&project).expect("create project");
    fs::write(project.join("otto.yml"), OTTOFILE).expect("write ottofile");

    let run = otto_std_cmd(&home)
        .current_dir(&project)
        .env_remove("OTTOFILE")
        .args(["build"])
        .output()
        .expect("failed to run otto");
    assert!(
        run.status.success(),
        "the run itself must succeed, stderr:\n{}",
        String::from_utf8_lossy(&run.stderr)
    );

    let runs = history_json(&project, &home, &[]);
    let hostname = runs[0]
        .get("hostname")
        .unwrap_or_else(|| panic!("the run record must carry a hostname field; got:\n{runs:#}"));
    let hostname = hostname
        .as_str()
        .unwrap_or_else(|| panic!("hostname must be a non-null string, got {hostname}"));
    assert!(!hostname.is_empty(), "hostname must not be empty");

    let tasks = history_json(&project, &home, &["build"]);
    let script_hash = tasks[0]
        .get("script_hash")
        .unwrap_or_else(|| panic!("the task record must carry a script_hash field; got:\n{tasks:#}"));
    let script_hash = script_hash
        .as_str()
        .unwrap_or_else(|| panic!("script_hash must be a non-null string, got {script_hash}"));
    assert!(
        !script_hash.is_empty(),
        "script_hash must be the processor's hash of the rendered script, not an empty string"
    );
}
