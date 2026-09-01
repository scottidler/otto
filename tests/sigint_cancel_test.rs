//! A terminal Ctrl+C on a plain (non-TUI) run must reach the scheduler.
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
//! otto is in that group. That also demonstrates the process-group design
//! holds: the task children were put in their own groups
//! (`src/executor/scheduler/task_execution.rs`, `cmd.process_group(0)`), so
//! they do not receive the group signal and cannot race otto's teardown.
//!
//! Break-the-code check, run 2026-09-01 before this file was committed: with
//! the `install_interrupt_handler` call commented out, the same drive exits
//! 130, prints no cancellation notice, no killed-log-path lines, and no
//! did-not-start line. With it, 5 runs out of 5 produced identical output.

mod common;

use std::fs;
use std::io::{Read, Write};
use std::path::Path;
use std::process::{Child, Stdio};
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
