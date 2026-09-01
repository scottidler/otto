//! Success criterion (f) of Phase 4 (design doc
//! `docs/design/2026-08-31-buffered-foreach-computed-envs-required-params.md`):
//! the cancellation flush is ordered but never stops early.
//!
//! Two things force the unusual shape of this file.
//!
//! First, the run has to be cancelled from inside the process: otto installs a
//! signal handler only on the `--tui` path (`src/app.rs:579-588`), where the
//! cursor is deliberately inert, so a SIGINT at a plain `otto` does not reach
//! `abandon_run` at all. `cancel_signal()` is the one trigger the scheduler
//! actually has, and it is what the existing cancellation tests drive too
//! (`src/executor/scheduler_tests_a.rs:91-146`).
//!
//! Second, replay writes through a locked `io::stdout()`, which the test
//! harness's `print!` capture does not intercept, so the real file descriptors
//! are redirected for the duration of the run. That is process-global, which is
//! why this is the ONLY test in this file: an integration test target is its own
//! binary, so nothing else is running alongside it.

mod common;

use std::collections::HashMap;
use std::os::fd::AsRawFd;
use std::path::PathBuf;
use std::sync::Arc;

use eyre::Result;
use tempfile::TempDir;

use otto::cfg::edge::When;
use otto::executor::task::TaskEdge;
use otto::executor::workspace::ExecutionContext;
use otto::{Task, TaskScheduler, Workspace};

/// A buffered foreach subtask, as the parser would hand one to the scheduler.
fn subtask(parent: &str, item: &str, action: &str) -> Task {
    let mut task = Task::new(
        format!("{parent}:{item}"),
        Some(parent.to_string()),
        vec![],
        vec![],
        vec![],
        HashMap::new(),
        HashMap::new(),
        action.to_string(),
    );
    task.buffered = true;
    task
}

/// The virtual parent, carrying `When::Always` edges to every subtask and the
/// Phase 3 display-order map.
fn parent(name: &str, items: &[&str]) -> Task {
    let order: Vec<String> = items.iter().map(|item| format!("{name}:{item}")).collect();
    let edges = order.iter().map(|n| TaskEdge::new(n.clone(), When::Always)).collect();
    let mut task = Task::new(
        name.to_string(),
        None,
        edges,
        vec![],
        vec![],
        HashMap::new(),
        HashMap::new(),
        String::new(),
    );
    task.is_virtual_parent = true;
    task.buffered = true;
    task.foreach_display_order = Some(order);
    task
}

/// Redirect the process's real stdout and stderr into `path` for the duration
/// of `body`, then restore them and hand back what was written.
async fn capture_terminal<Fut>(path: &std::path::Path, body: impl FnOnce() -> Fut) -> String
where
    Fut: std::future::Future<Output = ()>,
{
    let file = std::fs::File::create(path).expect("capture file");
    // SAFETY: single-test binary, so no other thread is writing to fd 1 or 2,
    // and both descriptors are restored before this function returns.
    let (saved_out, saved_err) = unsafe {
        let saved_out = libc::dup(1);
        let saved_err = libc::dup(2);
        libc::dup2(file.as_raw_fd(), 1);
        libc::dup2(file.as_raw_fd(), 2);
        (saved_out, saved_err)
    };

    body().await;

    unsafe {
        libc::dup2(saved_out, 1);
        libc::dup2(saved_err, 2);
        libc::close(saved_out);
        libc::close(saved_err);
    }
    std::fs::read_to_string(path).expect("capture file is readable")
}

/// The position of `needle` in `text`, as a failure-reporting assertion.
fn at(text: &str, needle: &str) -> usize {
    text.find(needle)
        .unwrap_or_else(|| panic!("expected to find {needle:?} in the cancelled run's output:\n{text}"))
}

/// A cancelled buffered group emits exactly one thing per item, in item order,
/// and never stops at the first non-terminal one.
///
/// The group is wide enough to hold, at the moment of cancellation: a killed
/// child that had already written a partial log (alpha), a finished subtask
/// whose block is held behind it (beta), a second killed child (gamma), and two
/// items that never started (delta, epsilon). Stopping at alpha would lose
/// beta's complete block, which is the exact stall this flush exists to
/// prevent.
#[tokio::test]
async fn test_a_cancelled_buffered_group_flushes_in_item_order_without_stopping_early() -> Result<()> {
    let temp_dir = TempDir::new()?;
    let work_dir = PathBuf::from(temp_dir.path());
    let otto_home = work_dir.join(".otto");
    // SAFETY: one test in this binary, and the variables are set before any
    // state store is opened.
    unsafe {
        std::env::remove_var("OTTO_DB_PATH");
        std::env::set_var("OTTO_HOME", &otto_home);
    }

    let items = ["alpha", "beta", "gamma", "delta", "epsilon"];
    let mut tasks = vec![
        // A partial log on disk, so "its logs are partial and are not replayed"
        // is asserted against real bytes rather than an empty file.
        subtask("say", "alpha", "echo 'alpha partial line'\nsleep 30"),
        subtask("say", "beta", "echo 'beta produced this'"),
        subtask("say", "gamma", "sleep 30"),
        subtask("say", "delta", "sleep 30"),
        subtask("say", "epsilon", "sleep 30"),
    ];
    tasks.push(parent("say", &items));

    let workspace = Workspace::new(work_dir).await?;
    workspace.init().await?;
    // Two permits: alpha and beta start, beta finishes and gamma takes its
    // place, delta and epsilon never leave the ready queue.
    let scheduler = TaskScheduler::new(tasks, Arc::new(workspace), ExecutionContext::new(), 2, false).await?;
    let cancel = scheduler.cancel_signal();

    let capture_path = temp_dir.path().join("terminal.txt");
    let captured = capture_terminal(&capture_path, || async move {
        tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_millis(1500)).await;
            cancel.cancel();
        });
        let outcome = tokio::time::timeout(std::time::Duration::from_secs(20), scheduler.execute_all())
            .await
            .expect("the cancelled run must not hang");
        assert!(outcome.is_err(), "a cancelled run must not report success");
    })
    .await;

    // One thing per item, in item order, with beta's finished block emitted
    // behind a killed alpha and ahead of a killed gamma and two unstarted items.
    let alpha = at(&captured, "say:alpha was killed mid-run");
    let beta = at(&captured, "beta produced this");
    let beta_status = at(&captured, "] finished successfully");
    let gamma = at(&captured, "say:gamma was killed mid-run");
    let delta = at(&captured, "say:delta did not start");
    let epsilon = at(&captured, "say:epsilon did not start");
    assert!(
        alpha < beta && beta < beta_status && beta_status < gamma && gamma < delta && delta < epsilon,
        "the flush must walk the whole item list in order:\n{captured}"
    );

    // No partial block anywhere: alpha wrote a line to its log before it was
    // killed, and that line must not be replayed.
    assert!(
        !captured.contains("alpha partial line"),
        "a killed subtask's partial log must never be replayed:\n{captured}"
    );
    assert!(
        captured.contains("say:alpha/stdout.log") && captured.contains("say:alpha/stderr.log"),
        "its run-dir paths are printed instead, so nothing is silently discarded:\n{captured}"
    );
    assert!(
        captured.contains("run cancelled"),
        "the cancellation itself is still reported:\n{captured}"
    );
    Ok(())
}
