//! Cancelling a run must kill the whole task subtree, not just the direct child.
//!
//! Through v2.1.0 `abandon_run` aborted the task bodies and relied on
//! `kill_on_drop(true)`, which is a SIGKILL to the process otto spawned and to
//! nothing below it. Every task child was already made a process-group leader
//! (`task_execution.rs`, `cmd.process_group(0)`) with a comment saying the
//! reason was group reachability, and nothing in `src/` ever signalled a group.
//! Measured on 2026-09-01, a parallel foreach whose bodies forked `sleep 600`
//! left both grandchildren running after a real Ctrl+C. otto-dev's `logs` task
//! is `bash -> otto -> docker compose logs`, so that leaked two processes per
//! service on every interrupt.
//!
//! The interrupt has to arrive the way a user's does, so these run under
//! `script` (a real pty) and write a literal `0x03` byte, exactly as
//! `sigint_cancel_test.rs` does. Every step is gated on a marker file the task
//! body itself wrote; nothing here sleeps for a fixed duration to "be running
//! by now".
//!
//! **Liveness is read from `/proc/<pid>/cmdline` CONTENT, never from pid
//! existence.** Pids recycle, and a zombie still has a `/proc/<pid>` directory:
//! only the recorded command line tells the difference between "still running"
//! and "that number belongs to something else now". That is what makes these
//! tests Linux-only; the reaping itself is `#[cfg(unix)]` and the unit tests in
//! `src/executor/scheduler_tests_b.rs` pin the mechanism on any unix.
//!
//! Break-the-code check, run 2026-09-01 before this file was committed: with
//! the SIGTERM/SIGKILL `signal_snapshot` calls removed from
//! `reap_live_children`, `a_cancelled_run_reaps_every_task_bodys_grandchildren`
//! fails with both grandchildren still reporting `sleep 601`. See the
//! implementation notes for the recorded output.

mod common;

use std::fs;
use std::io::{Read, Write};
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use tempfile::TempDir;

/// How long any single wait-for-a-marker step gets before the test gives up.
const STEP_TIMEOUT: Duration = Duration::from_secs(30);

/// The sleep every fixture's grandchild runs. The duration is the identity: it
/// is what `/proc/<pid>/cmdline` is matched against, so it must not collide
/// with anything else the machine happens to be running.
const GRANDCHILD_MARK: &str = "sleep 601";

/// A pty-driven otto run, with both pipes drained from their own threads.
///
/// The pty and the run both have to keep making progress while the test drives
/// the markers, and a full pipe buffer would stall the run instead of the test.
struct PtyRun {
    child: Child,
    output: mpsc::Receiver<Vec<u8>>,
}

impl PtyRun {
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

    /// The interrupt itself: a literal Ctrl+C byte into the pty.
    fn interrupt(&mut self) {
        let stdin = self.child.stdin.as_mut().expect("stdin");
        stdin.write_all(&[0x03]).expect("write Ctrl+C to the pty");
        stdin.flush().expect("flush");
    }

    /// Reap otto within `STEP_TIMEOUT`, returning its exit code and output.
    ///
    /// A `-1` code means the process died of a signal rather than exiting, which
    /// is itself an assertable outcome here: it is what otto signalling its own
    /// process group would look like from outside.
    fn finish(mut self) -> (i32, String) {
        let deadline = Instant::now() + STEP_TIMEOUT;
        let code = loop {
            match self.child.try_wait().expect("try_wait") {
                Some(status) => break status.code().unwrap_or(-1),
                None if Instant::now() < deadline => thread::sleep(Duration::from_millis(50)),
                None => {
                    let _ = self.child.kill();
                    panic!("otto did not exit within {STEP_TIMEOUT:?} of the interrupt");
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

/// A pid's command line as the kernel reports it, or `None` if the pid is gone
/// or is a zombie (a zombie's `cmdline` is empty).
#[cfg(target_os = "linux")]
fn cmdline(pid: &str) -> Option<String> {
    let raw = fs::read(format!("/proc/{pid}/cmdline")).ok()?;
    let text = String::from_utf8_lossy(&raw).replace('\0', " ").trim().to_string();
    (!text.is_empty()).then_some(text)
}

/// Whether `pid` is still running the command it was recorded running.
///
/// Pid existence alone would answer a different question: the kernel reissues
/// pids, so a bare `/proc/<pid>` check turns "somebody else got this number"
/// into "the grandchild survived".
#[cfg(target_os = "linux")]
fn still_running(pid: &str, mark: &str) -> bool {
    cmdline(pid).is_some_and(|line| line.contains(mark))
}

/// The pid a task body recorded in `<markers>/<name>`.
fn recorded_pid(markers: &Path, name: &str) -> String {
    fs::read_to_string(markers.join(name))
        .unwrap_or_else(|e| panic!("task body never recorded a pid in {name}: {e}"))
        .trim()
        .to_string()
}

/// SIGKILL anything the run left behind, so a failing assertion does not leak a
/// ten-minute `sleep` into the machine running the suite.
#[cfg(target_os = "linux")]
fn cleanup(pids: &[String]) {
    for pid in pids {
        let _ = Command::new("kill").arg("-9").arg(pid).status();
    }
}

fn project(temp: &TempDir, ottofile: &str) -> (std::path::PathBuf, std::path::PathBuf, std::path::PathBuf) {
    let home = temp.path().join("home");
    let proj = temp.path().join("proj");
    let markers = proj.join("markers");
    fs::create_dir_all(&home).expect("home");
    fs::create_dir_all(&markers).expect("markers");
    fs::write(proj.join("otto.yml"), ottofile).expect("ottofile");
    (home, proj, markers)
}

/// Each item forks a grandchild, records its pid, announces itself, and then
/// blocks in `wait`. Nothing releases them: the interrupt is the only way this
/// run ends.
const GRANDCHILD_OTTOFILE: &str = r#"
otto:
  api: 1

tasks:
  hold:
    foreach:
      items: [alpha, beta]
      parallel: true
    bash: |
      sleep 601 &
      echo $! > "${MARKERS}/gc-${item}"
      touch "${MARKERS}/start-${item}"
      wait
"#;

/// Phase 1 success criterion (a): every grandchild of every task body is gone
/// after a real Ctrl+C.
#[test]
#[cfg(target_os = "linux")]
fn a_cancelled_run_reaps_every_task_bodys_grandchildren() {
    let temp = TempDir::new().expect("temp dir");
    let (home, proj, markers) = project(&temp, GRANDCHILD_OTTOFILE);

    let mut run = PtyRun::start(&["-j", "4", "hold"], &proj, &home, &markers);
    run.wait_for(
        || {
            ["alpha", "beta"]
                .iter()
                .all(|i| markers.join(format!("gc-{i}")).exists())
        },
        "both items to fork a grandchild",
    );

    let pids: Vec<String> = ["alpha", "beta"]
        .iter()
        .map(|item| recorded_pid(&markers, &format!("gc-{item}")))
        .collect();
    // The fixture has to be doing what it claims BEFORE the interrupt, or an
    // "everything is dead" assertion afterwards proves nothing.
    //
    // This WAITS rather than asserting outright: the marker carries `$!`, which
    // bash knows at fork, so the pid is recorded before the child has exec'd
    // `sleep`. Until that exec lands, `/proc/<pid>/cmdline` still reports the
    // forked shell. Asserting here read the wrapper's command line on a loaded
    // machine and failed the precondition before an interrupt was ever sent.
    run.wait_for(
        || pids.iter().all(|pid| still_running(pid, GRANDCHILD_MARK)),
        "both grandchildren to be identifiable in /proc",
    );

    run.interrupt();
    let (code, output) = run.finish();

    // The reaping happens inside `abandon_run`, before otto returns, so the
    // grandchildren are already gone here. The poll only absorbs the gap
    // between SIGKILL delivery and the kernel tearing the process down.
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline && pids.iter().any(|pid| still_running(pid, GRANDCHILD_MARK)) {
        thread::sleep(Duration::from_millis(50));
    }
    let survivors: Vec<&String> = pids.iter().filter(|pid| still_running(pid, GRANDCHILD_MARK)).collect();
    let survivor_lines: Vec<String> = survivors
        .iter()
        .map(|pid| format!("{pid} -> {:?}", cmdline(pid)))
        .collect();
    cleanup(&pids);

    assert_ne!(code, 0, "an interrupted run must not exit 0:\n{output}");
    assert!(
        output.contains("run cancelled"),
        "the interrupt must reach abandon_run:\n{output}"
    );
    assert!(
        survivors.is_empty(),
        "cancellation must reap the whole task subtree, but these grandchildren outlived it: {survivor_lines:?}\n{output}"
    );
}

/// The direct child dies on SIGTERM; the grandchild ignores it and does not. The
/// body therefore removes its own registry entry DURING the grace window, which
/// is the case a SIGKILL pass reading the live registry would miss entirely.
///
/// `trap "" TERM` is what makes the grandchild SIGTERM-proof, and it keeps
/// working after bash execs the last command of a `-c` string over itself: an
/// ignored disposition survives `execve`, so the recorded pid ends up BEING the
/// `sleep`, still ignoring SIGTERM. That exec is also why the marker below is
/// the sleep's own command line rather than the wrapper shell's.
const GRACE_WINDOW_OTTOFILE: &str = r#"
otto:
  api: 1

tasks:
  hold:
    foreach:
      items: [only]
      parallel: true
    bash: |
      bash -c 'trap "" TERM; sleep 602' &
      echo $! > "${MARKERS}/gc"
      touch "${MARKERS}/start"
      wait
"#;

/// The grace-window fixture's grandchild, distinct from [`GRANDCHILD_MARK`] so
/// the two tests can never read each other's processes.
const IGNORING_GRANDCHILD_MARK: &str = "sleep 602";

/// Phase 1 success criterion (d): the grace-window case.
#[test]
#[cfg(target_os = "linux")]
fn a_grandchild_that_ignores_sigterm_is_reaped_after_the_grace_window() {
    let temp = TempDir::new().expect("temp dir");
    let (home, proj, markers) = project(&temp, GRACE_WINDOW_OTTOFILE);

    let mut run = PtyRun::start(&["hold"], &proj, &home, &markers);
    run.wait_for(|| markers.join("gc").exists(), "the item to fork its grandchild");

    let pid = recorded_pid(&markers, "gc");
    let mark = IGNORING_GRANDCHILD_MARK;
    run.wait_for(
        || still_running(&pid, mark),
        "the grandchild to be identifiable in /proc",
    );

    run.interrupt();
    let (code, output) = run.finish();

    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline && still_running(&pid, mark) {
        thread::sleep(Duration::from_millis(50));
    }
    let survived = still_running(&pid, mark);
    let survivor = cmdline(&pid);
    cleanup(&[pid]);

    assert_ne!(code, 0, "an interrupted run must not exit 0:\n{output}");
    assert!(
        !survived,
        "a SIGTERM-ignoring grandchild must still be reaped by the SIGKILL pass, \
         which only works if that pass walks the snapshot rather than the live \
         registry the body emptied during the grace window; saw {survivor:?}\n{output}"
    );
}

/// The task owns the terminal, so it stays in otto's own process group. It
/// ignores SIGINT, so it is still registered when `abandon_run` runs, and blocks
/// on a fifo rather than on a child so nothing is left holding the pty open.
const TTY_OTTOFILE: &str = r#"
otto:
  api: 1

tasks:
  hold:
    tty: true
    bash: |
      trap "" INT
      touch "${MARKERS}/start"
      read -r _ < "${MARKERS}/gate"
"#;

/// Phase 1 success criterion (c): cancelling a `tty: true` task signals the
/// task, never otto's own process group.
///
/// The tty child is deliberately NOT a group leader, so a `killpg` aimed at its
/// pgid would land on otto (and on `script`, and on the shell between them).
/// From outside that is visible as otto dying of a signal instead of exiting:
/// the assertion is that otto reaches its ordinary cancelled exit and prints its
/// ordinary cancellation notice.
#[test]
#[cfg(unix)]
fn cancelling_a_tty_task_signals_the_task_and_not_otto_itself() {
    let temp = TempDir::new().expect("temp dir");
    let (home, proj, markers) = project(&temp, TTY_OTTOFILE);
    let gate = markers.join("gate");
    let made = Command::new("mkfifo").arg(&gate).status().expect("mkfifo");
    assert!(made.success(), "the fixture needs a fifo to block the tty task on");

    let mut run = PtyRun::start(&["hold"], &proj, &home, &markers);
    run.wait_for(|| markers.join("start").exists(), "the tty task to start");

    run.interrupt();
    let (code, output) = run.finish();

    assert_ne!(
        code, -1,
        "otto must not die of a signal: a -1 here is otto having signalled its own \
         process group, which is exactly what the tty carve-out exists to prevent:\n{output}"
    );
    assert_ne!(
        code,
        128 + libc_sigterm(),
        "otto must not report having been SIGTERMed by its own cancellation:\n{output}"
    );
    assert_ne!(code, 0, "an interrupted run must not exit 0:\n{output}");
    assert!(
        output.contains("run cancelled"),
        "otto must survive its own reaping and print the ordinary cancellation notice:\n{output}"
    );
}

/// SIGTERM's number, spelled out rather than pulled in as a dependency for one
/// integer. It is 15 on every unix otto builds for.
#[cfg(unix)]
fn libc_sigterm() -> i32 {
    15
}
