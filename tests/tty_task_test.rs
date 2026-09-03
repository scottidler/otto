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
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Output, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};
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

/// A microsecond wall-clock stamp for the timeline fixtures. See
/// `timeline_fixture` for why it is neither `date` nor `EPOCHREALTIME`.
const STAMP: &str = r#"$(perl -MTime::HiRes=time -e 'printf "%.0f", time()*1000000')"#;

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
    let ottofile_arg = ottofile.display().to_string();
    let mut cmd = common::pty_cmd(&[OTTO_BIN, "-o", &ottofile_arg, "interactive"]);
    let output = isolate(&mut cmd, &otto_home)
        .output()
        .expect("failed to run `script` for the pty test");

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
/// Stamps come from `perl -MTime::HiRes`, not from `date` and not from bash's
/// `EPOCHREALTIME`. `date` is out because this box ships uutils coreutils 0.8.0,
/// whose `date` ignores the `%3N` width modifier and emits variable-length
/// nanoseconds, so the stamps would not be comparable as integers.
/// `EPOCHREALTIME` is out because it is bash 5.0+, and macOS runs these tasks
/// under /bin/bash 3.2.57, where it expands to nothing and every timeline line
/// arrives with a missing field. perl is on the base install of both.
fn timeline_fixture(timeline: &Path, tty_task: Option<&str>) -> String {
    let stamp = STAMP;
    let body = |name: &str, sleep: &str| {
        let tty_line = if tty_task == Some(name) { "    tty: true\n" } else { "" };
        format!(
            r#"  {name}:
{tty_line}    bash: |
      echo "{name} start {stamp}" >> {timeline}
      sleep {sleep}
      echo "{name} end {stamp}" >> {timeline}
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
      echo "${{svc}} start {stamp}" >> {timeline}
      sleep 0.3
      echo "${{svc}} end {stamp}" >> {timeline}
"#,
            timeline = timeline.display(),
            stamp = STAMP,
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

// ---------------------------------------------------------------------------
// The stdin half of the same rule: a non-`tty` task cannot read the terminal,
// by either route, and `tty: true` is how a task gets one.
//
// Both routes need a real terminal to test, so these run under `script`. The
// reads use `head -c1`, a real `read(2)`, deliberately: bash's `read -t N` polls
// before it reads and times out cleanly, so it never reproduces the hang these
// tests exist to keep out.
// ---------------------------------------------------------------------------

/// How long a pty run gets before the test gives up. The behavior it guards
/// against is an UNBOUNDED hang - before 2026-09-02 the `head -c1` fixtures ran
/// until an external `timeout` killed them - so the bound only has to be finite
/// and slack enough not to flake on a loaded machine.
const PTY_TIMEOUT: Duration = Duration::from_secs(30);

/// A pty-driven otto run with its stdin held open, so a test can feed the
/// terminal. Both pipes are drained from their own threads: a full pipe buffer
/// would stall the run instead of the test.
struct PtyRun {
    child: Child,
    output: mpsc::Receiver<Vec<u8>>,
}

impl PtyRun {
    fn start(args: &[&str], proj: &Path, home: &Path, markers: &Path) -> Self {
        let mut argv = vec![OTTO_BIN];
        argv.extend_from_slice(args);
        let mut cmd = common::pty_cmd(&argv);
        isolate(&mut cmd, home);
        cmd.current_dir(proj)
            .env("MARKERS", markers)
            // `bash -ic` sources the developer's own startup files, which print
            // whatever they print. Pointing HOME at the fixture keeps the
            // interactive-shell case reading the same nothing everywhere.
            .env("HOME", home)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child = cmd.spawn().expect("script should run otto under a pty");

        let (tx, output) = mpsc::channel::<Vec<u8>>();
        let pipes: [Box<dyn Read + Send>; 2] = [
            Box::new(child.stdout.take().expect("stdout")),
            Box::new(child.stderr.take().expect("stderr")),
        ];
        for mut pipe in pipes {
            let tx = tx.clone();
            thread::spawn(move || {
                let mut buf = Vec::new();
                let _ = pipe.read_to_end(&mut buf);
                let _ = tx.send(buf);
            });
        }
        drop(tx);
        Self { child, output }
    }

    /// Poll `predicate` until it holds, or fail naming `what`.
    fn wait_for(&mut self, predicate: impl Fn() -> bool, what: &str) {
        let deadline = Instant::now() + PTY_TIMEOUT;
        while Instant::now() < deadline {
            if predicate() {
                return;
            }
            thread::sleep(Duration::from_millis(50));
        }
        let _ = self.child.kill();
        panic!("timed out waiting for {what}");
    }

    /// Type `line` at the terminal. The pty is in canonical mode, so the newline
    /// is what hands the bytes to the reader.
    fn type_line(&mut self, line: &str) {
        let stdin = self.child.stdin.as_mut().expect("stdin");
        stdin.write_all(line.as_bytes()).expect("write to the pty");
        stdin.write_all(b"\n").expect("newline");
        stdin.flush().expect("flush");
    }

    /// Reap otto within [`PTY_TIMEOUT`], returning its exit code and output.
    ///
    /// The timeout is the assertion for the hang cases: reaching it means the
    /// task is still blocked on a read that must not have been possible.
    fn finish(mut self) -> (i32, String) {
        let deadline = Instant::now() + PTY_TIMEOUT;
        let code = loop {
            match self.child.try_wait().expect("try_wait") {
                Some(status) => break status.code().unwrap_or(-1),
                None if Instant::now() < deadline => thread::sleep(Duration::from_millis(50)),
                None => {
                    let _ = self.child.kill();
                    panic!("otto did not exit within {PTY_TIMEOUT:?}; the task is still blocked on its read");
                }
            }
        };
        let mut text = String::new();
        while let Ok(bytes) = self.output.recv() {
            text.push_str(&common::pty_stdout(&bytes));
        }
        (code, text)
    }
}

/// Write `ottofile` into a fresh temp project and return (home, proj, markers).
fn pty_project(temp: &TempDir, ottofile: &str) -> (PathBuf, PathBuf, PathBuf) {
    let home = temp.path().join("home");
    let proj = temp.path().join("proj");
    let markers = proj.join("markers");
    fs::create_dir_all(&home).expect("home");
    fs::create_dir_all(&markers).expect("markers");
    fs::write(proj.join("otto.yml"), ottofile).expect("ottofile");
    (home, proj, markers)
}

const READ_STDIN_OTTOFILE: &str = r#"
otto:
  api: 1

tasks:
  rd:
    bash: |
      echo before-read
      head -c1
      echo after-read
"#;

/// Route one: a non-`tty` task's fd 0 is `/dev/null` when otto's own stdin is a
/// terminal, so a real read gets EOF instead of blocking on keystrokes nobody is
/// routing to it. Before this the same fixture printed `before-read` and nothing
/// else, forever.
#[test]
fn a_non_tty_task_reading_stdin_under_a_pty_gets_eof() {
    let temp = TempDir::new().unwrap();
    let (home, proj, markers) = pty_project(&temp, READ_STDIN_OTTOFILE);

    let run = PtyRun::start(&["rd"], &proj, &home, &markers);
    let (code, output) = run.finish();

    assert_eq!(
        code, 0,
        "the read should have hit EOF and the task succeeded:\n{output}"
    );
    assert!(
        output.contains("after-read"),
        "the task never got past its read:\n{output}"
    );
}

const DEV_TTY_OTTOFILE: &str = r#"
otto:
  api: 1

tasks:
  rd:
    bash: |
      echo before-read
      head -c1 </dev/tty
      echo after-read
"#;

/// Route two: nulling fd 0 does nothing about `/dev/tty`, which reopens the
/// controlling terminal by name. A non-`tty` child has no controlling terminal
/// (`setsid`), so the open fails with the program's own error text instead of the
/// task stopping on SIGTTIN.
#[test]
fn a_non_tty_task_opening_dev_tty_fails_instead_of_hanging() {
    let temp = TempDir::new().unwrap();
    let (home, proj, markers) = pty_project(&temp, DEV_TTY_OTTOFILE);

    let run = PtyRun::start(&["rd"], &proj, &home, &markers);
    let (code, output) = run.finish();

    assert_ne!(code, 0, "opening /dev/tty must fail the task:\n{output}");
    assert!(
        output.contains("/dev/tty"),
        "the failure must name /dev/tty, so the user knows to set tty: true:\n{output}"
    );
    assert!(
        !output.contains("after-read"),
        "the read must not have succeeded:\n{output}"
    );
}

const INTERACTIVE_SHELL_OTTOFILE: &str = r#"
otto:
  api: 1

tasks:
  rd:
    bash: |
      echo before-read
      bash -ic 'head -c1 </dev/tty'
      echo after-read
"#;

/// The hole that sank the `SIG_IGN` mechanism: an interactive shell resets the
/// job-control dispositions and stops anyway. `setsid` is not defeatable that
/// way, because there is no controlling terminal left to stop on.
///
/// Asserted on a substring, not on the whole stream: `bash -ic` without a
/// terminal also prints a job-control warning of its own.
#[test]
fn a_non_tty_task_opening_dev_tty_from_an_interactive_shell_fails_too() {
    let temp = TempDir::new().unwrap();
    let (home, proj, markers) = pty_project(&temp, INTERACTIVE_SHELL_OTTOFILE);

    let run = PtyRun::start(&["rd"], &proj, &home, &markers);
    let (code, output) = run.finish();

    assert_ne!(code, 0, "opening /dev/tty must fail the task:\n{output}");
    assert!(
        output.contains("/dev/tty"),
        "an interactive shell must fail the same way a non-interactive one does:\n{output}"
    );
    assert!(
        !output.contains("after-read"),
        "the read must not have succeeded:\n{output}"
    );
}

const TTY_READ_OTTOFILE: &str = r#"
otto:
  api: 1

tasks:
  ask:
    tty: true
    bash: |
      touch "${MARKERS}/start"
      printf 'read=[%s]\n' "$(head -c1 </dev/tty)"
"#;

/// The other side of the policy: `tty: true` is how a task gets a terminal, and
/// it still has one. Without this the two tests above would be satisfied by
/// breaking terminal access for every task.
#[test]
fn a_tty_task_reads_the_byte_typed_at_the_terminal() {
    let temp = TempDir::new().unwrap();
    let (home, proj, markers) = pty_project(&temp, TTY_READ_OTTOFILE);

    let mut run = PtyRun::start(&["ask"], &proj, &home, &markers);
    let start = markers.join("start");
    run.wait_for(|| start.exists(), "the tty task to start");
    run.type_line("x");
    let (code, output) = run.finish();

    assert_eq!(
        code, 0,
        "the tty task should have read its byte and succeeded:\n{output}"
    );
    assert!(
        output.contains("read=[x]"),
        "a tty task must still be able to read the terminal:\n{output}"
    );
}

const CAT_OTTOFILE: &str = r#"
otto:
  api: 1

tasks:
  rd:
    bash: |
      cat
"#;

/// The rule is terminal-only. A pipe never produces SIGTTIN and never blocks on
/// a terminal nobody is typing at, so `echo hi | otto rd` keeps working: nulling
/// stdin unconditionally would have broken every CI job that feeds a task.
#[test]
fn a_non_tty_task_with_piped_stdin_reads_the_pipe() {
    let temp = TempDir::new().unwrap();
    let (home, proj, _markers) = pty_project(&temp, CAT_OTTOFILE);

    let mut child = common::otto_std_cmd(&home)
        .current_dir(&proj)
        .arg("rd")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("otto should start");
    child
        .stdin
        .take()
        .expect("stdin")
        .write_all(b"hi\n")
        .expect("write the pipe");
    let output = child.wait_with_output().expect("otto should finish");

    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(output.status.success(), "a piped-stdin task must succeed:\n{combined}");
    assert!(
        combined.contains("hi"),
        "a pipe on stdin must still reach the task:\n{combined}"
    );
}
