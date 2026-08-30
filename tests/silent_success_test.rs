//! Regression tests for the silent-success criticals in the parse/schedule core.
//!
//! Every test here reproduces a case where otto did the wrong thing and then
//! exited 0, or hung: an unknown task name dropped on the floor, a dependency
//! cycle reported as a clean run, a spawn failure blamed on a task that does
//! not exist, `-j 0` accepted and then spun forever, `exit 7` recorded as 1.
//! See docs/design/2026-06-10-code-review-remediation.md Phase 1.

mod common;

use common::otto_std_cmd;
use std::fs;
use std::path::{Path, PathBuf};

use tempfile::TempDir;

/// Run the real binary in `dir` with an isolated `OTTO_HOME`, returning
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

/// A project directory with `otto.yml` written from `body`, plus its own
/// `OTTO_HOME` so runs never touch the developer's real one.
fn project(body: &str) -> (TempDir, PathBuf) {
    let dir = TempDir::new().expect("tempdir");
    fs::write(dir.path().join("otto.yml"), body).expect("write ottofile");
    let otto_home = dir.path().join("otto-home");
    fs::create_dir_all(&otto_home).expect("create otto home");
    (dir, otto_home)
}

const BUILD_AND_TEST: &str =
    "otto:\n  tasks: [\"*\"]\ntasks:\n  build:\n    action: echo BUILDING\n  test:\n    action: echo TESTING\n";

#[test]
fn unknown_task_fails_and_names_the_task() {
    let (dir, home) = project(BUILD_AND_TEST);
    let (code, stdout, stderr) = run_otto(dir.path(), &home, &["nonexistent"]);

    assert_ne!(code, 0, "otto nonexistent must not exit 0; stdout: {stdout}");
    assert!(
        stderr.contains("nonexistent"),
        "the error must name the unknown task, got: {stderr}"
    );
    assert!(
        !stdout.contains("No tasks to execute"),
        "the unknown name must be an error, not a no-op: {stdout}"
    );
}

#[test]
fn unknown_task_before_a_real_one_does_not_silently_run_it() {
    let (dir, home) = project(BUILD_AND_TEST);
    let (code, stdout, stderr) = run_otto(dir.path(), &home, &["nonexistent", "build"]);

    assert_ne!(code, 0, "the unknown leading arg must fail the run");
    assert!(
        !stdout.contains("BUILDING"),
        "build must not run when an arg was not understood: {stdout}"
    );
    assert!(stderr.contains("nonexistent"), "got: {stderr}");
}

#[test]
fn near_miss_task_name_suggests_the_real_one() {
    let (dir, home) = project(BUILD_AND_TEST);
    let (code, _stdout, stderr) = run_otto(dir.path(), &home, &["buld"]);

    assert_ne!(code, 0);
    assert!(
        stderr.contains("did you mean 'build'"),
        "a one-edit typo must be suggested, got: {stderr}"
    );
}

#[test]
fn bare_otto_runs_the_projects_tasks_not_a_builtin() {
    let (dir, home) = project(BUILD_AND_TEST);
    let (code, stdout, stderr) = run_otto(dir.path(), &home, &[]);

    assert_eq!(code, 0, "bare otto should run the default tasks; stderr: {stderr}");
    assert!(
        stdout.contains("BUILDING") && stdout.contains("TESTING"),
        "got: {stdout}"
    );
    assert!(
        !stdout.contains("Querying database for old runs"),
        "`otto: tasks: [\"*\"]` must not expand to the Clean builtin: {stdout}"
    );
}

#[test]
fn the_dead_lowercase_graph_name_is_no_longer_a_partition_boundary() {
    // `graph` was pushed into the partition name set but no task ever had that
    // name (the builtin is `Graph`), so it split the args and then died with
    // "Task 'graph' not found" much later.
    let (dir, home) = project(BUILD_AND_TEST);
    let (code, _stdout, stderr) = run_otto(dir.path(), &home, &["graph"]);

    assert_ne!(code, 0);
    assert!(
        stderr.contains("unknown task 'graph'") && stderr.contains("did you mean 'Graph'"),
        "got: {stderr}"
    );
}

#[test]
fn dependency_cycle_exits_non_zero() {
    let (dir, home) =
        project("tasks:\n  a:\n    after: [b]\n    action: echo A\n  b:\n    after: [a]\n    action: echo B\n");
    let (code, stdout, stderr) = run_otto(dir.path(), &home, &["a", "b"]);

    assert_ne!(code, 0, "a 2-cycle must not exit 0; stdout: {stdout}");
    assert!(
        stderr.contains("cycle"),
        "the error must say the graph has a cycle, got: {stderr}"
    );
}

#[test]
fn jobs_zero_is_rejected_at_parse() {
    let (dir, home) = project(BUILD_AND_TEST);
    let (code, _stdout, stderr) = run_otto(dir.path(), &home, &["-j", "0", "build"]);

    assert_ne!(code, 0, "-j 0 must be rejected, not accepted and then hung");
    assert!(
        stderr.contains("invalid value '0'"),
        "clap should reject the value by name, got: {stderr}"
    );
}

#[test]
fn empty_default_tasks_still_runs_an_explicitly_requested_task() {
    let (dir, home) = project("otto:\n  tasks: []\ntasks:\n  build:\n    action: echo BUILDING\n");
    let (code, stdout, stderr) = run_otto(dir.path(), &home, &["build"]);

    assert_eq!(code, 0, "stderr: {stderr}");
    assert!(
        stdout.contains("BUILDING"),
        "`otto build` printed nothing and exited 0 before this fix: {stdout}"
    );
}

#[test]
fn unrecognized_help_request_is_an_error_not_a_silent_success() {
    let (dir, home) = project(BUILD_AND_TEST);
    let (code, stdout, stderr) = run_otto(dir.path(), &home, &["help", "build", "extra"]);

    assert_ne!(code, 0, "`otto help build extra` must not exit 0; stdout: {stdout}");
    assert!(stderr.contains("otto help [TASK]"), "got: {stderr}");
}

#[test]
fn unknown_task_flag_is_reported_rather_than_exiting_from_the_parser() {
    let (dir, home) = project(BUILD_AND_TEST);
    let (code, _stdout, stderr) = run_otto(dir.path(), &home, &["build", "--bogus"]);

    // Exit 1 is main's code. Exit 2 would mean clap ended the process from
    // inside the library, which is what `get_matches_from` used to do here.
    assert_eq!(code, 1, "the parser must propagate, not exit; stderr: {stderr}");
    assert!(
        stderr.contains("--bogus"),
        "the unknown flag must be named in the error, got: {stderr}"
    );
}

/// A PATH with `bash` but deliberately no `python3`, so a `python3:` task fails
/// at spawn time - the exact shape that used to hang the scheduler.
fn path_without_python3(dir: &Path) -> PathBuf {
    let bin = dir.join("nopython");
    fs::create_dir_all(&bin).expect("create bin dir");
    for tool in ["bash", "sh", "cat", "rm", "mkdir", "ln"] {
        if let Ok(found) = which(tool) {
            let _ = std::os::unix::fs::symlink(found, bin.join(tool));
        }
    }
    bin
}

fn which(tool: &str) -> Result<PathBuf, ()> {
    for dir in std::env::var("PATH").unwrap_or_default().split(':') {
        let candidate = Path::new(dir).join(tool);
        if candidate.exists() {
            return Ok(candidate);
        }
    }
    Err(())
}

#[test]
fn spawn_failure_names_the_real_task_and_the_run_terminates() {
    // `pytask` is a dependency of `downstream`, so a spawn failure in pytask
    // must both be attributed correctly and stop downstream from running.
    let (dir, home) = project(
        "tasks:\n  pytask:\n    after: [downstream]\n    action: |\n      #!/usr/bin/env python3\n      print(\"hi\")\n  downstream:\n    action: echo DOWNSTREAM\n",
    );
    let bin = path_without_python3(dir.path());

    let output = otto_std_cmd(&home)
        .current_dir(dir.path())
        .env("PATH", &bin)
        .env_remove("OTTOFILE")
        .args(["pytask", "downstream"])
        .output()
        .expect("failed to run otto");

    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();

    assert_ne!(output.status.code(), Some(0), "the run must fail: {stdout}{stderr}");
    assert!(
        stderr.contains("pytask"),
        "the failure must name the task, not a word chopped out of the error text; got: {stderr}"
    );
    assert!(
        !stderr.contains("[such]") && !stdout.contains("[such]"),
        "the task name must not be recovered from the message text: {stdout}{stderr}"
    );
    assert!(
        !stdout.contains("DOWNSTREAM"),
        "downstream must not run after its dependency failed: {stdout}"
    );
}

#[test]
fn failing_task_records_its_real_exit_code() {
    let (dir, home) = project("tasks:\n  boom:\n    action: exit 7\n");
    let (code, stdout, stderr) = run_otto(dir.path(), &home, &["boom"]);
    assert_ne!(code, 0, "{stdout}{stderr}");

    let store = otto::executor::state::StateManager::with_db_path(home.join("otto.db")).expect("open state db");
    let runs = store.get_recent_runs(1, None).expect("recent runs");
    let run = runs.first().expect("the run should be recorded");
    let tasks = store.get_run_tasks(run.id).expect("run tasks");
    let boom = tasks
        .iter()
        .find(|t| t.name == "boom")
        .expect("boom should be recorded");

    assert_eq!(
        boom.exit_code,
        Some(7),
        "the recorded exit code must be the process's own, not a re-parse that falls back to 1"
    );
}

#[test]
fn skipped_task_records_why_it_was_skipped() {
    let (dir, home) =
        project("tasks:\n  first:\n    after: [second]\n    action: exit 3\n  second:\n    action: echo SECOND\n");
    let (code, stdout, stderr) = run_otto(dir.path(), &home, &["first", "second"]);
    assert_ne!(code, 0, "{stdout}{stderr}");

    let store = otto::executor::state::StateManager::with_db_path(home.join("otto.db")).expect("open state db");
    let runs = store.get_recent_runs(1, None).expect("recent runs");
    let run = runs.first().expect("the run should be recorded");
    let tasks = store.get_run_tasks(run.id).expect("run tasks");
    let second = tasks
        .iter()
        .find(|t| t.name == "second")
        .expect("the skipped task should reach the run record");

    let reason = second
        .skip_reason
        .as_deref()
        .expect("skip provenance must be persisted, not built and dropped");
    assert!(
        reason.contains("first"),
        "the reason must name the blocking dep: {reason}"
    );
}

/// A task-level `envs:` that cannot resolve must fail the run, not drop the
/// whole map and run anyway.
///
/// The global env path already failed closed (`cfg::resolver::global_envs`
/// returns Err); the task path warned and substituted an empty map, so one
/// cyclic key silently took every *other* key with it and the task ran with no
/// environment at all, at exit 0. Found by the batched audit, batch 4 of 14.
#[test]
fn a_cyclic_task_env_fails_the_run_instead_of_dropping_every_key() {
    let (dir, home) = project(
        "otto:\n  api: 1\n  tasks: [t]\ntasks:\n  t:\n    envs:\n      GOOD: \"iamfine\"\n      A: \"${B}-a\"\n      B: \"${A}-b\"\n    action: |\n      echo \"GOOD=[${GOOD:-MISSING}]\"\n",
    );
    let (code, stdout, stderr) = run_otto(dir.path(), &home, &["t"]);

    assert_ne!(code, 0, "a cyclic task env must not exit 0; stdout: {stdout}");
    assert!(
        stderr.contains("Circular dependency between environment variables"),
        "the error must name the cycle, got: {stderr}"
    );
    assert!(
        stderr.contains('t'),
        "the error must name the task whose envs failed, got: {stderr}"
    );
    assert!(
        !stdout.contains("finished successfully"),
        "the task must not run with a dropped environment: {stdout}"
    );
    assert!(
        !stdout.contains("GOOD=[MISSING]"),
        "the unrelated healthy key must not be silently dropped: {stdout}"
    );
}

/// The same fail-closed rule for an unresolvable (not cyclic) task env, so the
/// fix is not narrowly pinned to cycle detection.
#[test]
fn an_unresolvable_task_env_fails_the_run() {
    let (dir, home) = project(
        "otto:\n  api: 1\n  tasks: [t]\ntasks:\n  t:\n    envs:\n      GOOD: \"iamfine\"\n      BAD: \"$(exit 3)\"\n    action: |\n      echo \"GOOD=[${GOOD:-MISSING}]\"\n",
    );
    let (code, stdout, _stderr) = run_otto(dir.path(), &home, &["t"]);

    assert_ne!(code, 0, "an unresolvable task env must not exit 0; stdout: {stdout}");
    assert!(
        !stdout.contains("GOOD=[MISSING]"),
        "the healthy key must not be dropped alongside the failing one: {stdout}"
    );
}

/// `otto_get_input <task>.<Key>` returned empty at exit 0 for any key that was
/// not already lowercase.
///
/// The round trip goes through an uppercased shell variable
/// (`OTTO_INPUT_<TASK>_<KEY>`), so the key's original spelling is not
/// recoverable from it. The bash reader lowercased and called that the key,
/// which is a guess: `producer.MIXED_Case` came back empty while
/// `producer.mixed_case` returned the value. Wrong answer, exit 0, no
/// diagnostic - the class this file exists for. The Python generator reads the
/// JSON directly and never had the bug, so the two generators disagreed.
///
/// otto now writes the key's original spelling beside each value and the
/// reader uses it. The JSON key is the contract: `producer.mixed_case` is a
/// miss now, correctly, because that key does not exist.
#[test]
fn input_keys_keep_the_case_the_producer_wrote() {
    let (dir, otto_home) = project(
        r#"
otto:
  api: 1
  tasks: [consumer]
tasks:
  producer:
    output: [MIXED_Case, plain, UPPER]
    bash: |
      otto_set_output "MIXED_Case" "hello"
      otto_set_output "plain" "world"
      otto_set_output "UPPER" "shout"
  consumer:
    before: [producer]
    input: [producer.MIXED_Case, producer.plain, producer.UPPER]
    bash: |
      echo "mixed=[$(otto_get_input producer.MIXED_Case)]"
      echo "plain=[$(otto_get_input producer.plain)]"
      echo "upper=[$(otto_get_input producer.UPPER)]"
      echo "wrongcase=[$(otto_get_input producer.mixed_case)]"
"#,
    );

    let (code, stdout, stderr) = run_otto(dir.path(), &otto_home, &["consumer"]);

    assert_eq!(code, 0, "stderr: {stderr}");
    for expected in ["mixed=[hello]", "plain=[world]", "upper=[shout]"] {
        assert!(
            stdout.contains(expected),
            "expected {expected} in output; got:\n{stdout}"
        );
    }
    assert!(
        stdout.contains("wrongcase=[]"),
        "a key the producer never wrote must miss, not alias onto one it did:\n{stdout}"
    );
    // The miss must also be audible. An earlier version of this test asserted
    // only the empty value, which meant a file whose whole purpose is the
    // silent-success class was defending a silent miss.
    assert!(
        format!("{stdout}{stderr}").contains("no input 'producer.mixed_case'"),
        "the miss must name the key it could not find:\n{stdout}{stderr}"
    );
}

/// A task name containing `.` lost every one of its outputs, silently, at
/// exit 0.
///
/// The writer folds both `-` and `.` into `_` when building
/// `OTTO_INPUT_<TASK>_<KEY>`; the bash reader folded only `-`, so it built the
/// prefix `OTTO_INPUT_PRO.DUCER_`, matched nothing, and left `OTTO_INPUT`
/// empty. Total, silent, whole-task data loss - and it survived the commit that
/// rewrote this very function, because nothing in the tree has a dotted task
/// name. The dashed task is the control: it always worked, which is what
/// isolates the missing `.` fold as the cause.
#[test]
fn a_dotted_task_name_still_delivers_its_outputs() {
    let (dir, otto_home) = project(
        r#"
otto:
  api: 1
  tasks: [consumer]
tasks:
  pro.ducer:
    output: [x]
    bash: otto_set_output "x" "FROM_DOTTED_TASK"
  pro-ducer:
    output: [x]
    bash: otto_set_output "x" "FROM_DASHED_TASK"
  consumer:
    before: [pro.ducer, pro-ducer]
    input: [pro.ducer.x, pro-ducer.x]
    bash: |
      echo "DOTTED=[$(otto_get_input pro.ducer.x)]"
      echo "DASHED=[$(otto_get_input pro-ducer.x)]"
"#,
    );

    let (code, stdout, stderr) = run_otto(dir.path(), &otto_home, &["consumer"]);

    assert_eq!(code, 0, "stderr: {stderr}");
    assert!(
        stdout.contains("DOTTED=[FROM_DOTTED_TASK]"),
        "a dotted task name must not lose its outputs:\n{stdout}"
    );
    assert!(
        stdout.contains("DASHED=[FROM_DASHED_TASK]"),
        "control: the dashed form was never broken:\n{stdout}"
    );
}

/// Distinct output keys that fold onto one shell variable must all survive.
///
/// Shell identifiers hold neither `-` nor `.`, so `a-b`, `a.b` and `A_B` all
/// become `..._A_B`. Every one used to be emitted under that single name, so
/// the last assignment won and the other two values disappeared at exit 0.
/// This qualifies "the JSON key is the contract": it is the contract only if
/// no two keys in a task collapse, so the writer now suffixes a collision and
/// the companion key variable is suffixed with it.
#[test]
fn output_keys_that_collide_as_shell_variables_all_survive() {
    let (dir, otto_home) = project(
        r#"
otto:
  api: 1
  tasks: [consumer]
tasks:
  producer:
    output: [a-b, a.b, A_B]
    bash: |
      otto_set_output "a-b" "DASH"
      otto_set_output "a.b" "DOT"
      otto_set_output "A_B" "UPPER"
  consumer:
    before: [producer]
    input: [producer.a-b, producer.a.b, producer.A_B]
    bash: |
      echo "DASH=[$(otto_get_input producer.a-b)]"
      echo "DOT=[$(otto_get_input producer.a.b)]"
      echo "UPPER=[$(otto_get_input producer.A_B)]"
"#,
    );

    let (code, stdout, stderr) = run_otto(dir.path(), &otto_home, &["consumer"]);

    assert_eq!(code, 0, "stderr: {stderr}");
    for expected in ["DASH=[DASH]", "DOT=[DOT]", "UPPER=[UPPER]"] {
        assert!(
            stdout.contains(expected),
            "three distinct keys must yield three values, not one; missing {expected}:\n{stdout}"
        );
    }
}

/// A key nobody produced is a miss, and it says so on stderr and in the exit
/// code. It used to print an empty line at exit 0, indistinguishable from a key
/// whose value really is empty.
#[test]
fn a_missing_input_key_is_audible_and_non_zero() {
    let (dir, otto_home) = project(
        r#"
otto:
  api: 1
  tasks: [consumer]
tasks:
  producer:
    output: [real]
    bash: otto_set_output "real" "value"
  consumer:
    before: [producer]
    input: [producer.real]
    bash: |
      echo "HIT=[$(otto_get_input producer.real)]"
      # Deliberately NOT redirected: the diagnostic is half of what this pins,
      # and an earlier draft of this test sent it to /dev/null and then
      # asserted on it.
      if otto_get_input producer.nope > /dev/null; then echo "RC=zero"; else echo "RC=nonzero"; fi
"#,
    );

    let (code, stdout, stderr) = run_otto(dir.path(), &otto_home, &["consumer"]);
    let output = format!("{stdout}{stderr}");

    assert_eq!(code, 0, "the task itself still succeeds: {stderr}");
    assert!(stdout.contains("HIT=[value]"), "{stdout}");
    assert!(stdout.contains("RC=nonzero"), "a miss must be non-zero:\n{stdout}");
    assert!(
        output.contains("no input 'producer.nope'") && output.contains("available: producer.real"),
        "the miss must name the key AND what was available:\n{output}"
    );
}
