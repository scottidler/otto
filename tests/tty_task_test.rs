//! Integration tests for `tty: true` (design doc
//! docs/design/2026-08-28-boundary-fixes-and-dynamic-foreach.md, Phase 7).
//!
//! These spawn the real `otto` binary. The whole feature is about what the task's
//! stdout/stderr are connected to and about what else is allowed to run at the
//! same time, and neither is observable from inside the process.
//!
//! Every fixture here is non-interactive and self-terminating: a task that waits
//! for input would wedge the suite.

mod common;

use common::{OTTO_BIN, isolate, otto_cmd};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command as StdCommand, Output};
use tempfile::TempDir;

fn write_ottofile(dir: &Path, contents: &str) -> PathBuf {
    let path = dir.join("otto.yml");
    fs::write(&path, contents).unwrap();
    path
}

fn otto(ottofile: &Path, otto_home: &Path, args: &[&str]) -> Output {
    otto_cmd(otto_home).arg("-o").arg(ottofile).args(args).output().unwrap()
}

fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).to_string()
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).to_string()
}

/// The run directory is `$OTTO_HOME/<project>-<hash>/<timestamp>/`, so the log
/// for a task is found by walking for `tasks/<name>/<file>`.
fn find_log(otto_home: &Path, task: &str, file: &str) -> String {
    fn walk(dir: &Path, needle: &Path, found: &mut Vec<PathBuf>) {
        let Ok(entries) = fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                walk(&path, needle, found);
            } else if path.ends_with(needle) {
                found.push(path);
            }
        }
    }
    let needle = Path::new("tasks").join(task).join(file);
    let mut found = Vec::new();
    walk(otto_home, &needle, &mut found);
    assert_eq!(
        found.len(),
        1,
        "expected exactly one {} for task {task} under {}, found {found:?}",
        file,
        otto_home.display()
    );
    fs::read_to_string(&found[0]).unwrap()
}

const MARKER: &str = "otto: tty task, output not captured";

/// A `tty: true` task that reports what its stdin/stdout are attached to, beside
/// an ordinary task. Both self-terminate.
const TTY_FIXTURE: &str = r#"
otto:
  api: 1

tasks:
  interactive:
    help: "owns the terminal"
    tty: true
    bash: |
      if [ -t 0 ]; then echo "STDIN: tty"; else echo "STDIN: NOT a tty"; fi
      if [ -t 1 ]; then echo "STDOUT: tty"; else echo "STDOUT: NOT a tty"; fi
      echo "hello from tty"

  plain:
    help: "an ordinary captured task"
    bash: |
      echo "hello from plain"
"#;

/// Criterion (a): under a real pty the tty task's stdout IS a tty, and its
/// recorded logs carry the marker line rather than being empty.
///
/// On main the same fixture printed `STDIN: tty` / `STDOUT: NOT a tty`: stdin
/// was always inherited, stdout was the piece that was piped away.
#[test]
fn tty_task_sees_a_real_tty_on_stdout_under_a_pty() {
    let temp = TempDir::new().unwrap();
    let otto_home = temp.path().join("otto-home");
    let ottofile = write_ottofile(temp.path(), TTY_FIXTURE);
    // `script` runs the command through a shell, so otto inherits `script`'s
    // environment: isolating the outer process isolates the inner otto.
    let inner_cmd = format!("{OTTO_BIN} -o {} interactive", ottofile.display());
    let mut script = StdCommand::new("script");
    isolate(&mut script, &otto_home);
    let output = script
        .arg("-qec")
        .arg(&inner_cmd)
        .arg("/dev/null")
        .output()
        .expect("failed to run `script` (util-linux) for the pty test");

    let out = stdout(&output);
    assert!(output.status.success(), "otto failed under the pty: {out}");
    assert!(out.contains("STDOUT: tty"), "tty task did not get a tty stdout:\n{out}");
    assert!(out.contains("STDIN: tty"), "tty task did not get a tty stdin:\n{out}");

    let log = find_log(&otto_home, "interactive", "stdout.log");
    assert_eq!(log, format!("{MARKER}\n"), "tty logs must carry the marker");
    assert_eq!(find_log(&otto_home, "interactive", "stderr.log"), format!("{MARKER}\n"));
}

/// A tty task's output reaches the terminal unprefixed and uncaptured, while an
/// ordinary task in the same run still gets its `[task]` prefix and its log.
#[test]
fn tty_output_is_unprefixed_while_plain_tasks_keep_their_prefix() {
    let temp = TempDir::new().unwrap();
    let otto_home = temp.path().join("otto-home");
    let ottofile = write_ottofile(temp.path(), TTY_FIXTURE);

    let output = otto(&ottofile, &otto_home, &["interactive", "plain"]);
    let out = stdout(&output);
    assert!(output.status.success(), "run failed: {out}{}", stderr(&output));

    assert!(
        out.lines().any(|l| l.trim_end() == "hello from tty"),
        "tty output must be unprefixed:\n{out}"
    );
    // otto's own per-task status lines stay prefixed for every task, tty or not:
    // that is otto speaking, not the task's output. What must never be prefixed is
    // a line the task itself wrote.
    for line in ["hello from tty", "STDIN:", "STDOUT:"] {
        assert!(
            !out.contains(&format!("[interactive] {line}")),
            "tty task output must never be prefixed ({line}):\n{out}"
        );
    }
    assert!(
        out.contains("[plain] hello from plain"),
        "an ordinary task must keep its prefix:\n{out}"
    );

    assert_eq!(find_log(&otto_home, "interactive", "stdout.log"), format!("{MARKER}\n"));
    let plain_log = find_log(&otto_home, "plain", "stdout.log");
    assert!(
        plain_log.contains("hello from plain"),
        "an ordinary task must still be captured: {plain_log}"
    );
}

/// Fixture for the exclusivity test: four independent tasks, each stamping its
/// start and end into one shared timeline file. The task carrying `tty: true` is
/// named by `tty_task`; `None` produces the control fixture.
///
/// Stamps come from bash's `EPOCHREALTIME` (fixed-width microseconds), not from
/// `date`: this box ships uutils coreutils 0.8.0, whose `date` ignores the `%3N`
/// width modifier and emits variable-length nanoseconds, so the stamps would not
/// be comparable as integers.
fn timeline_fixture(timeline: &Path, tty_task: Option<&str>) -> String {
    let body = |name: &str, sleep: &str| {
        let tty_line = if tty_task == Some(name) { "    tty: true\n" } else { "" };
        format!(
            r#"  {name}:
{tty_line}    bash: |
      echo "{name} start ${{EPOCHREALTIME/[.,]/}}" >> {timeline}
      sleep {sleep}
      echo "{name} end ${{EPOCHREALTIME/[.,]/}}" >> {timeline}
"#,
            timeline = timeline.display()
        )
    };
    format!(
        "otto:\n  api: 1\n\ntasks:\n{}{}{}{}",
        body("interactive", "0.6"),
        body("alpha", "0.4"),
        body("beta", "0.4"),
        body("gamma", "0.4"),
    )
}

/// (task, start_us, end_us) parsed out of the shared timeline file.
fn intervals(timeline: &Path) -> Vec<(String, u64, u64)> {
    let text = fs::read_to_string(timeline).unwrap_or_else(|e| panic!("no timeline at {}: {e}", timeline.display()));
    let mut starts = std::collections::HashMap::new();
    let mut out = Vec::new();
    for line in text.lines() {
        let parts: Vec<&str> = line.split_whitespace().collect();
        assert_eq!(parts.len(), 3, "malformed timeline line {line:?}");
        let stamp: u64 = parts[2]
            .parse()
            .unwrap_or_else(|e| panic!("bad stamp in {line:?}: {e}"));
        match parts[1] {
            "start" => {
                starts.insert(parts[0].to_string(), stamp);
            }
            "end" => {
                let start = starts.remove(parts[0]).expect("end before start");
                out.push((parts[0].to_string(), start, stamp));
            }
            other => panic!("unexpected timeline verb {other:?}"),
        }
    }
    out
}

fn overlaps(a: &(String, u64, u64), b: &(String, u64, u64)) -> bool {
    a.1 < b.2 && b.1 < a.2
}

/// Criterion (b): while a tty task runs, nothing else does. Asserted from real
/// wall-clock intervals the tasks themselves stamped, not from a status field.
///
/// The control half of the test runs the identical fixture with no `tty:` and
/// requires overlap, so a harness that could never observe overlap fails here.
#[test]
fn a_tty_task_runs_exclusively_while_the_same_tasks_otherwise_overlap() {
    let temp = TempDir::new().unwrap();

    // Control: same four tasks, no tty. With -j 4 they must overlap.
    let control_home = temp.path().join("control-home");
    let control_timeline = temp.path().join("control-timeline");
    let control_file = temp.path().join("control.yml");
    fs::write(&control_file, timeline_fixture(&control_timeline, None)).unwrap();
    let control = otto(
        &control_file,
        &control_home,
        &["-j", "4", "interactive", "alpha", "beta", "gamma"],
    );
    assert!(control.status.success(), "control run failed: {}", stderr(&control));
    let control_intervals = intervals(&control_timeline);
    assert_eq!(control_intervals.len(), 4);
    let control_overlap = control_intervals
        .iter()
        .enumerate()
        .any(|(i, a)| control_intervals[i + 1..].iter().any(|b| overlaps(a, b)));
    assert!(
        control_overlap,
        "control run never overlapped, so this test could not detect a lost exclusivity: {control_intervals:?}"
    );

    // Subject: the same fixture with `tty: true` on `interactive`.
    let home = temp.path().join("otto-home");
    let timeline = temp.path().join("timeline");
    let ottofile = temp.path().join("tty.yml");
    fs::write(&ottofile, timeline_fixture(&timeline, Some("interactive"))).unwrap();
    let output = otto(&ottofile, &home, &["-j", "4", "interactive", "alpha", "beta", "gamma"]);
    assert!(output.status.success(), "tty run failed: {}", stderr(&output));

    let observed = intervals(&timeline);
    assert_eq!(observed.len(), 4, "all four tasks must have run: {observed:?}");
    let tty = observed
        .iter()
        .find(|(name, _, _)| name == "interactive")
        .expect("tty task missing from timeline")
        .clone();
    for other in observed.iter().filter(|(name, _, _)| name != "interactive") {
        assert!(
            !overlaps(&tty, other),
            "{} overlapped the tty task: tty={tty:?} other={other:?}",
            other.0
        );
    }
}

/// Criterion (c): `--tui` plus any tty task in the run set is a loud error raised
/// before anything executes. The sentinels prove nothing ran, not merely that the
/// process exited non-zero.
#[test]
fn tui_with_a_tty_task_errors_before_executing_anything() {
    let temp = TempDir::new().unwrap();
    let otto_home = temp.path().join("otto-home");
    let tty_sentinel = temp.path().join("tty-ran");
    let plain_sentinel = temp.path().join("plain-ran");
    let fixture = format!(
        r#"
otto:
  api: 1

tasks:
  interactive:
    tty: true
    bash: |
      touch {tty}

  plain:
    bash: |
      touch {plain}
"#,
        tty = tty_sentinel.display(),
        plain = plain_sentinel.display()
    );
    let ottofile = write_ottofile(temp.path(), &fixture);

    let output = otto(&ottofile, &otto_home, &["--tui", "interactive", "plain"]);

    assert!(!output.status.success(), "--tui with a tty task must fail");
    let err = stderr(&output);
    assert!(
        err.contains("--tui cannot run alongside a tty task") && err.contains("interactive"),
        "error must name the flag and the offending task, got: {err}"
    );
    assert!(!tty_sentinel.exists(), "the tty task ran despite the conflict error");
    assert!(
        !plain_sentinel.exists(),
        "a sibling task ran despite the conflict error; the check is not before execution"
    );

    // The same run without --tui is fine, which is what makes the error a
    // conflict rather than a rejection of the ottofile.
    let ok = otto(&ottofile, &otto_home, &["interactive", "plain"]);
    assert!(ok.status.success(), "run without --tui failed: {}", stderr(&ok));
    assert!(tty_sentinel.exists() && plain_sentinel.exists());
}

/// A failing tty task still reports its exit code and its log paths, so the
/// marker-bearing logs are reachable from the error a user sees.
#[test]
fn a_failing_tty_task_reports_exit_code_and_log_paths() {
    let temp = TempDir::new().unwrap();
    let otto_home = temp.path().join("otto-home");
    let ottofile = write_ottofile(
        temp.path(),
        r#"
otto:
  api: 1

tasks:
  interactive:
    tty: true
    bash: |
      echo "about to fail"
      exit 3
"#,
    );

    let output = otto(&ottofile, &otto_home, &["interactive"]);

    assert!(!output.status.success());
    let err = stderr(&output);
    assert!(
        err.contains("Task interactive failed with exit code Some(3)"),
        "error must name the task and exit code: {err}"
    );
    assert!(
        err.contains("stdout.log") && err.contains("stderr.log"),
        "error must point at the logs: {err}"
    );
    assert_eq!(find_log(&otto_home, "interactive", "stdout.log"), format!("{MARKER}\n"));
}

/// `tty:` on a foreach task means "give each of these the terminal": every
/// subtask inherits it, and the exclusivity gate serializes them even though the
/// group is declared `parallel: true`. Documented behavior, not an error.
#[test]
fn foreach_subtasks_inherit_tty_and_are_serialized_by_exclusivity() {
    let temp = TempDir::new().unwrap();
    let otto_home = temp.path().join("otto-home");
    let timeline = temp.path().join("timeline");
    let ottofile = write_ottofile(
        temp.path(),
        &format!(
            r#"
otto:
  api: 1

tasks:
  login:
    tty: true
    foreach:
      items: [alpha, beta, gamma]
      as: svc
      parallel: true
    bash: |
      echo "${{svc}} start ${{EPOCHREALTIME/[.,]/}}" >> {timeline}
      sleep 0.3
      echo "${{svc}} end ${{EPOCHREALTIME/[.,]/}}" >> {timeline}
"#,
            timeline = timeline.display()
        ),
    );

    let output = otto(&ottofile, &otto_home, &["-j", "4", "login"]);
    assert!(output.status.success(), "foreach tty run failed: {}", stderr(&output));

    let observed = intervals(&timeline);
    assert_eq!(observed.len(), 3, "every subtask must run: {observed:?}");
    for (i, a) in observed.iter().enumerate() {
        for b in &observed[i + 1..] {
            assert!(!overlaps(a, b), "tty subtasks overlapped: {a:?} and {b:?}");
        }
    }
    for svc in ["alpha", "beta", "gamma"] {
        assert_eq!(
            find_log(&otto_home, &format!("login:{svc}"), "stdout.log"),
            format!("{MARKER}\n")
        );
    }
}
