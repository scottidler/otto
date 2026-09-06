//! A stop signal must reach the scheduler, on both the plain and the `--tui` path.
//!
//! Three signals mean "stop this run": SIGINT from the keyboard, SIGTERM from a
//! supervisor or a bare `kill`, and SIGHUP from a terminal closing. All three
//! are fatal by default, and until 2026-09-02 only SIGINT was handled, so the
//! other two killed otto outright: `abandon_run` never ran, and every non-`tty`
//! task's subtree was orphaned, because those children run in their own session
//! and the signal aimed at otto never reaches them.
//!
//! Through v2.0.5 otto installed a signal handler only on the `--tui` path
//! (`src/app.rs`, `execute_with_tui`). A plain `otto <task>` had no handler, so
//! the terminal's default SIGINT disposition killed the process outright:
//! `abandon_run` never ran, and a buffered foreach group lost every block that
//! had completed but not yet been replayed. `tests/foreach_buffer_cancel_test.rs`
//! pins the ORDER of that flush by driving `cancel_signal()` in-process, which
//! is the only trigger it had; this file pins the WIRING, which is the part
//! that was missing: that a real Ctrl+C at a real terminal reaches that same
//! signal.
//!
//! The interrupt has to arrive the way a user's does, or it proves nothing. A
//! `kill -INT <pid>` aimed at otto would pass even with no pty involved and
//! would not exercise the process-group question at all. So the run happens
//! under `script`, which allocates a pty, and the test writes a literal `0x03`
//! byte into `script`'s stdin. `script` forwards it to the pty master, the line
//! discipline turns it into SIGINT for the pty's foreground process group, and
//! otto is in that group. That also demonstrates the session design holds: the
//! task children run in their own sessions
//! (`src/executor/scheduler/task_execution.rs`, `setsid`), so they do not
//! receive the group signal and cannot race otto's teardown.
//!
//! SIGTERM and SIGHUP are the opposite case and are driven the opposite way: a
//! `kill -TERM <otto pid>` IS how a user or a supervisor sends them, so the
//! tests below aim them at otto's own pid, which the task body records from
//! `$PPID`. They still run under a pty, because the `--tui` case needs one and
//! because the plain case must not accidentally be measuring a pipe.
//!
//! Break-the-code check, run 2026-09-01 before the Ctrl+C test was committed:
//! with the signal-handler installation commented out, the same drive exits
//! 130, prints no cancellation notice, no killed-log-path lines, and no
//! did-not-start line. With it, 5 runs out of 5 produced identical output. The
//! 2026-09-02 break-the-test runs for the SIGTERM, SIGHUP and `--tui` SIGHUP
//! tests are recorded in the Phase 1 implementation notes.

mod common;

use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use tempfile::TempDir;

/// The foreach items, in the order the replay cursor must use.
const ITEMS: [&str; 4] = ["alpha", "beta", "gamma", "delta"];

/// How long any single wait-for-a-marker step gets before the test gives up.
const STEP_TIMEOUT: Duration = Duration::from_secs(30);

/// Each subtask announces itself, then blocks until the test releases it, so
/// the test decides which item completes and which ones are still running when
/// the interrupt lands. Nothing here sleeps for a fixed duration to "be running
/// by now": every step is gated on a file the subtask itself created.
const OTTOFILE: &str = r#"
otto:
  api: 1

tasks:
  say:
    foreach:
      items: [alpha, beta, gamma, delta]
      parallel: true
      buffer: true
    bash: |
      touch "${MARKERS}/start-${item}"
      while [ ! -e "${MARKERS}/go-${item}" ]; do sleep 0.05; done
      echo "${item} ran to completion"
      touch "${MARKERS}/done-${item}"
"#;

/// The items that have written their start marker, in `ITEMS` order.
fn started(markers: &Path) -> Vec<&'static str> {
    ITEMS
        .iter()
        .copied()
        .filter(|item| markers.join(format!("start-{item}")).exists())
        .collect()
}

/// Poll `predicate` until it holds, or fail naming `what`.
fn wait_for(predicate: impl Fn() -> bool, what: &str, child: &mut Child) {
    let deadline = Instant::now() + STEP_TIMEOUT;
    while Instant::now() < deadline {
        if predicate() {
            return;
        }
        thread::sleep(Duration::from_millis(50));
    }
    let _ = child.kill();
    panic!("timed out waiting for {what}");
}

/// Reap `child` within `STEP_TIMEOUT`, returning its exit code.
fn wait_for_exit(child: &mut Child) -> i32 {
    let deadline = Instant::now() + STEP_TIMEOUT;
    while Instant::now() < deadline {
        match child.try_wait().expect("try_wait") {
            Some(status) => return status.code().unwrap_or(-1),
            None => thread::sleep(Duration::from_millis(50)),
        }
    }
    let _ = child.kill();
    panic!("otto did not exit within {STEP_TIMEOUT:?} of the interrupt");
}

#[test]
fn a_terminal_ctrl_c_on_a_plain_run_flushes_the_buffered_group() {
    let temp = TempDir::new().expect("temp dir");
    let home = temp.path().join("home");
    let proj = temp.path().join("proj");
    let markers = proj.join("markers");
    fs::create_dir_all(&home).expect("home");
    fs::create_dir_all(&markers).expect("markers");
    fs::write(proj.join("otto.yml"), OTTOFILE).expect("ottofile");

    // `-j 2` is what creates the states this test needs: two items run, a third
    // takes the freed slot when the first finishes, and the fourth never
    // starts.
    let mut cmd = common::pty_cmd(&[common::OTTO_BIN, "-j", "2", "say"]);
    common::isolate(&mut cmd, &home);
    cmd.current_dir(&proj)
        .env("MARKERS", &markers)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = cmd.spawn().expect("script should run otto under a pty");

    // Drain both pipes from their own threads: the pty and the run both have to
    // keep making progress while the test drives the markers, and a full pipe
    // buffer would stall the run instead of the test.
    let (tx, rx) = mpsc::channel::<Vec<u8>>();
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

    // Both slots busy.
    wait_for(|| started(&markers).len() >= 2, "two subtasks to start", &mut child);

    // Release a started item that is NOT the first item. `alpha` completing
    // would let the replay cursor emit its block the instant it finished, which
    // is ordinary buffered replay and would prove nothing about cancellation.
    // Held behind an unfinished `alpha`, the released item's block can only
    // reach the terminal through the cancellation flush.
    let released = *started(&markers)
        .iter()
        .find(|item| **item != ITEMS[0])
        .expect("with -j 2, at least one of the two started items is not the first item");
    fs::write(markers.join(format!("go-{released}")), "").expect("release marker");

    // The released item is done and its freed slot has been taken, so at the
    // moment of the interrupt the group holds a completed-but-unreplayed block,
    // two live children, and one item that never started.
    wait_for(
        || markers.join(format!("done-{released}")).exists() && started(&markers).len() >= 3,
        "the released subtask to finish and a third to start",
        &mut child,
    );
    // The subtask writes its done marker before the scheduler has necessarily
    // read its report. Both sides of that race are covered by design (the
    // report-unread state is exactly what `abandon_run` drains first), so this
    // pause is not load-bearing for correctness, only for keeping the test
    // exercising the common path.
    thread::sleep(Duration::from_millis(300));

    // The interrupt itself: a literal Ctrl+C byte into the pty.
    child
        .stdin
        .as_mut()
        .expect("stdin")
        .write_all(&[0x03])
        .expect("write Ctrl+C to the pty");
    child.stdin.as_mut().expect("stdin").flush().expect("flush");

    let code = wait_for_exit(&mut child);
    let mut output = String::new();
    while let Ok(bytes) = rx.recv() {
        output.push_str(&common::pty_stdout(&bytes));
    }

    assert_ne!(code, 0, "an interrupted run must not exit 0:\n{output}");
    assert!(
        output.contains("run cancelled"),
        "the interrupt must reach abandon_run, which announces the cancellation:\n{output}"
    );
    assert!(
        output.contains(&format!("{released} ran to completion")),
        "the completed block held behind an unfinished earlier item must be flushed:\n{output}"
    );
    let unstarted: Vec<&str> = ITEMS
        .iter()
        .copied()
        .filter(|item| !markers.join(format!("start-{item}")).exists())
        .collect();
    assert!(
        !unstarted.is_empty(),
        "the fixture must leave at least one item unstarted, or it is not testing the did-not-start line"
    );
    for item in unstarted {
        assert!(
            output.contains(&format!("say:{item} did not start")),
            "an item that never started must say so rather than vanish:\n{output}"
        );
    }
    assert!(
        output.contains("was killed mid-run"),
        "a killed child's log paths must be printed, not its partial log:\n{output}"
    );
    assert!(
        !output.contains("should never"),
        "no unreleased subtask may have produced output:\n{output}"
    );
}

// ---------------------------------------------------------------------------
// SIGTERM, SIGHUP, and the `--tui` path.
// ---------------------------------------------------------------------------

/// crossterm's `EnterAlternateScreen` / `LeaveAlternateScreen`, as
/// `tests/tui_panic_test.rs` spells them.
const ENTER_ALT_SCREEN: &str = "\x1b[?1049h";
const LEAVE_ALT_SCREEN: &str = "\x1b[?1049l";

/// One non-`tty` task that forks a grandchild, records that pid and otto's own,
/// and then blocks. Nothing releases it: the signal is the only way this run
/// ends. `$PPID` inside the body IS otto, because otto is what spawned the
/// interpreter, and `setsid` in the child changes its session, never its parent.
const SIGNAL_OTTOFILE: &str = r#"
otto:
  api: 1

tasks:
  hold:
    bash: |
      sleep 603 &
      echo $! > "${MARKERS}/gc"
      echo $PPID > "${MARKERS}/otto-pid"
      touch "${MARKERS}/start"
      wait
"#;

/// The grandchild's identity in `/proc/<pid>/cmdline`. Distinct from the marks
/// `cancel_reaping_test.rs` uses so the two suites can never read each other's
/// processes when cargo runs them at the same time.
const GRANDCHILD_MARK: &str = "sleep 603";

/// A pty-driven otto run whose signals are aimed at otto's recorded pid.
struct SignalRun {
    child: Child,
    output: mpsc::Receiver<Vec<u8>>,
    markers: PathBuf,
}

impl SignalRun {
    fn start(args: &[&str], proj: &Path, home: &Path, markers: &Path) -> Self {
        let mut argv = vec![common::OTTO_BIN];
        argv.extend_from_slice(args);
        let mut cmd = common::pty_cmd(&argv);
        common::isolate(&mut cmd, home);
        cmd.current_dir(proj)
            .env("MARKERS", markers)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child = cmd.spawn().expect("script should run otto under a pty");

        // Both pipes drained from their own threads: a full pipe buffer would
        // stall the run instead of the test.
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
        Self {
            child,
            output,
            markers: markers.to_path_buf(),
        }
    }

    /// Poll `predicate` until it holds, or fail naming `what`.
    fn wait_for(&mut self, predicate: impl Fn() -> bool, what: &str) {
        let deadline = Instant::now() + STEP_TIMEOUT;
        while Instant::now() < deadline {
            if predicate() {
                return;
            }
            thread::sleep(Duration::from_millis(50));
        }
        let _ = self.child.kill();
        panic!("timed out waiting for {what}");
    }

    /// Send `signal` to otto itself, the way a supervisor or a closing terminal
    /// does. Not to `script`, and not to the group: aiming at the group would
    /// hit the test's own processes and would not test otto's handler at all.
    fn signal_otto(&mut self, signal: &str) {
        let pid = recorded(&self.markers, "otto-pid");
        let sent = Command::new("kill")
            .arg(format!("-{signal}"))
            .arg(&pid)
            .status()
            .expect("kill");
        assert!(sent.success(), "could not send {signal} to otto (pid {pid})");
    }

    /// Reap otto within `STEP_TIMEOUT`, returning its exit code and output.
    fn finish(mut self) -> (i32, String) {
        let deadline = Instant::now() + STEP_TIMEOUT;
        let code = loop {
            match self.child.try_wait().expect("try_wait") {
                Some(status) => break status.code().unwrap_or(-1),
                None if Instant::now() < deadline => thread::sleep(Duration::from_millis(50)),
                None => {
                    let _ = self.child.kill();
                    panic!("otto did not exit within {STEP_TIMEOUT:?} of the signal");
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

/// The value a task body recorded in `<markers>/<name>`.
fn recorded(markers: &Path, name: &str) -> String {
    fs::read_to_string(markers.join(name))
        .unwrap_or_else(|e| panic!("task body never recorded {name}: {e}"))
        .trim()
        .to_string()
}

/// A pid's command line as the kernel reports it, or `None` if the pid is gone
/// or is a zombie (a zombie's `cmdline` is empty).
#[cfg(target_os = "linux")]
fn cmdline(pid: &str) -> Option<String> {
    let raw = fs::read(format!("/proc/{pid}/cmdline")).ok()?;
    let text = String::from_utf8_lossy(&raw).replace('\0', " ").trim().to_string();
    (!text.is_empty()).then_some(text)
}

/// Whether `pid` is still running the command it was recorded running. Pid
/// existence alone answers a different question: the kernel reissues pids, so a
/// bare `/proc/<pid>` check turns "somebody else got this number" into "the
/// grandchild survived".
#[cfg(target_os = "linux")]
fn still_running(pid: &str, mark: &str) -> bool {
    cmdline(pid).is_some_and(|line| line.contains(mark))
}

/// Write the signal fixture into a fresh temp project.
fn signal_project(temp: &TempDir) -> (PathBuf, PathBuf, PathBuf) {
    let home = temp.path().join("home");
    let proj = temp.path().join("proj");
    let markers = proj.join("markers");
    fs::create_dir_all(&home).expect("home");
    fs::create_dir_all(&markers).expect("markers");
    fs::write(proj.join("otto.yml"), SIGNAL_OTTOFILE).expect("ottofile");
    (home, proj, markers)
}

/// Start the fixture and wait until its grandchild is identifiable in `/proc`.
///
/// This WAITS rather than asserting outright: the marker carries `$!`, which
/// bash knows at fork, so the pid is recorded before the child has exec'd
/// `sleep`, and until that exec lands `/proc/<pid>/cmdline` still reports the
/// forked shell.
#[cfg(target_os = "linux")]
fn started_run(args: &[&str], proj: &Path, home: &Path, markers: &Path) -> (SignalRun, String) {
    let mut run = SignalRun::start(args, proj, home, markers);
    let gc_marker = markers.join("gc");
    let pid_marker = markers.join("otto-pid");
    run.wait_for(
        || gc_marker.exists() && pid_marker.exists(),
        "the task to fork its grandchild and record both pids",
    );
    let grandchild = recorded(markers, "gc");
    let probe = grandchild.clone();
    run.wait_for(
        || still_running(&probe, GRANDCHILD_MARK),
        "the grandchild to be identifiable in /proc",
    );
    (run, grandchild)
}

/// Wait out the gap between SIGKILL delivery and the kernel tearing the process
/// down, then report whether the grandchild is still there. SIGKILL anything
/// that is, so a failing assertion does not leak a ten-minute `sleep`.
#[cfg(target_os = "linux")]
fn survivor(grandchild: &str) -> Option<String> {
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline && still_running(grandchild, GRANDCHILD_MARK) {
        thread::sleep(Duration::from_millis(50));
    }
    let survived = still_running(grandchild, GRANDCHILD_MARK).then(|| {
        let line = cmdline(grandchild).unwrap_or_default();
        format!("{grandchild} -> {line}")
    });
    let _ = Command::new("kill").arg("-9").arg(grandchild).status();
    survived
}

/// SIGTERM is what a supervisor, a CI runner, and a bare `kill` send. Before
/// 2026-09-02 it killed otto on its default disposition and left the whole task
/// subtree running.
#[test]
#[cfg(target_os = "linux")]
fn a_sigterm_on_a_plain_run_reaps_the_task_subtree() {
    let temp = TempDir::new().expect("temp dir");
    let (home, proj, markers) = signal_project(&temp);
    let (mut run, grandchild) = started_run(&["hold"], &proj, &home, &markers);

    run.signal_otto("TERM");
    let (code, output) = run.finish();
    let survived = survivor(&grandchild);

    assert_ne!(code, 0, "a signalled run must not exit 0:\n{output}");
    assert!(
        output.contains("run cancelled"),
        "SIGTERM must reach abandon_run, which announces the cancellation:\n{output}"
    );
    assert!(
        survived.is_none(),
        "SIGTERM must reap the non-tty task's whole subtree, but this grandchild \
         outlived it: {survived:?}\n{output}"
    );
}

/// SIGHUP is a terminal closing. It reaches otto and the pty's foreground group,
/// and it does NOT reach a child in its own session, so without a handler it is
/// the same orphaning bug as SIGTERM with the same one-line cause.
#[test]
#[cfg(target_os = "linux")]
fn a_sighup_on_a_plain_run_reaps_the_task_subtree() {
    let temp = TempDir::new().expect("temp dir");
    let (home, proj, markers) = signal_project(&temp);
    let (mut run, grandchild) = started_run(&["hold"], &proj, &home, &markers);

    run.signal_otto("HUP");
    let (code, output) = run.finish();
    let survived = survivor(&grandchild);

    assert_ne!(code, 0, "a signalled run must not exit 0:\n{output}");
    assert!(
        output.contains("run cancelled"),
        "SIGHUP must reach abandon_run, which announces the cancellation:\n{output}"
    );
    assert!(
        survived.is_none(),
        "SIGHUP must reap the non-tty task's whole subtree, but this grandchild \
         outlived it: {survived:?}\n{output}"
    );
}

/// The `--tui` path had its own handler and it was not enough: it only set a
/// quit flag, which `TuiApp::run` reads after drawing, and after a hangup every
/// draw fails EIO so the flag is never read. The signal task cancels the run
/// directly now, and the dashboard hands the terminal back on its way out.
#[test]
#[cfg(target_os = "linux")]
fn a_sighup_during_a_tui_run_reaps_the_task_subtree_and_restores_the_terminal() {
    let temp = TempDir::new().expect("temp dir");
    let (home, proj, markers) = signal_project(&temp);
    let (mut run, grandchild) = started_run(&["--tui", "hold"], &proj, &home, &markers);

    run.signal_otto("HUP");
    let (code, output) = run.finish();
    let survived = survivor(&grandchild);

    // Vacuous-pass guard: if the dashboard never took the terminal, this test is
    // measuring the plain path under a different name.
    assert!(
        output.contains(ENTER_ALT_SCREEN),
        "the run never entered the alternate screen, so --tui was not exercised:\n{output:?}"
    );
    assert_ne!(code, 0, "a signalled run must not exit 0:\n{output:?}");
    assert!(
        survived.is_none(),
        "SIGHUP under --tui must reap the non-tty task's whole subtree, but this \
         grandchild outlived it: {survived:?}\n{output:?}"
    );
    let entered = output.find(ENTER_ALT_SCREEN).expect("checked above");
    let left = output.rfind(LEAVE_ALT_SCREEN);
    assert!(
        left.is_some_and(|left| left > entered),
        "SIGHUP under --tui left the user on the alternate screen:\n{output:?}"
    );
}
