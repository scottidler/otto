//! The scheduler honors `foreach.jobs` (design doc
//! `docs/design/2026-09-01-cancellation-reaping-and-foreach-concurrency.md`,
//! Phase 3).
//!
//! Two gates stand between a foreach item and a running process, and a group
//! carrying `jobs:` is exempt from BOTH: the outer launch cap
//! (`while active_tasks.capped_len() < max_concurrent`) and the shared
//! `Semaphore::new(max_parallel)` each body acquires from. Exempting only the
//! semaphore changes nothing measurable - the control test below runs the same
//! fixture without the key and still sees exactly `-j` items start.
//!
//! **Concurrency here is proved with barriers, never with a stopwatch.** Every
//! item blocks on a fifo of its own until this test writes to it, so a run only
//! makes progress when the test says so, and "all ten started" is a fact about
//! ten marker files rather than about how long anything took. The one place a
//! duration appears is [`SETTLE`], where the claim is negative ("this must NOT
//! have started"), and even there the positive half of the same claim is a
//! barrier: the deferred task starts immediately after the thing blocking it is
//! released.
//!
//! Break-the-code, criterion (e): deleting either arm of `may_admit` in
//! `src/executor/scheduler.rs` makes one of the tty tests below fail. The
//! recorded output is in the implementation notes.

#![cfg(unix)]

mod common;

use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use tempfile::TempDir;

/// How long any single wait-for-a-marker step gets before the test gives up.
/// A bound on failure, not a measurement: every step it guards is released by
/// this test itself.
const STEP_TIMEOUT: Duration = Duration::from_secs(30);

/// How long a "this must NOT have happened" claim is watched before it is
/// believed. Only the negative halves use it.
const SETTLE: Duration = Duration::from_millis(750);

/// How often a barrier is polled.
const POLL: Duration = Duration::from_millis(25);

/// Create the fifo an item blocks on. A fifo, not a sleep: the item is
/// released by this test writing to it, so the run's progress is driven rather
/// than waited out.
fn mkfifo(path: &Path) {
    let status = Command::new("mkfifo")
        .arg(path)
        .status()
        .expect("mkfifo must be available on a unix host");
    assert!(status.success(), "mkfifo failed for {}", path.display());
}

/// Unblock whatever is reading `path`, and say whether anything was.
///
/// Opened `O_NONBLOCK`, which is what makes this safe to call on a fifo nobody
/// is reading: the open fails with `ENXIO` instead of blocking this test
/// forever. That is also how cleanup can release every fifo unconditionally,
/// including the ones belonging to items a regression never started.
fn release(path: &Path) -> bool {
    match OpenOptions::new().write(true).custom_flags(libc::O_NONBLOCK).open(path) {
        Ok(mut fifo) => fifo.write_all(b"go\n").is_ok(),
        Err(_) => false,
    }
}

/// Poll `condition` until it holds or [`STEP_TIMEOUT`] runs out.
fn wait_for(label: &str, condition: impl Fn() -> bool) -> bool {
    let deadline = Instant::now() + STEP_TIMEOUT;
    while Instant::now() < deadline {
        if condition() {
            return true;
        }
        thread::sleep(POLL);
    }
    eprintln!("wait_for({label}) timed out");
    false
}

/// Watch `condition` for [`SETTLE`] and report whether it ever became true.
///
/// Polled rather than sampled once at the end: the claim is that the thing
/// never happened during the window, not that it had stopped by the end of it.
fn ever_true(condition: impl Fn() -> bool) -> bool {
    let deadline = Instant::now() + SETTLE;
    while Instant::now() < deadline {
        if condition() {
            return true;
        }
        thread::sleep(POLL);
    }
    false
}

/// The scratch directory a fixture's markers and fifos live in, plus the otto
/// run driving them.
///
/// Dropping it releases every fifo before killing otto: a task body blocked on
/// a fifo read outlives a SIGKILL to otto (otto's `kill_on_drop` cannot fire
/// for a process that never runs its own teardown), so a failed assertion
/// would otherwise leave blocked shells behind.
struct Fixture {
    _temp: TempDir,
    dir: PathBuf,
    home: PathBuf,
    log: PathBuf,
    otto: Option<Child>,
}

impl Fixture {
    /// Write `ottofile` (with `@DIR@` replaced by the scratch path) and create
    /// one fifo per name in `fifos`.
    fn new(ottofile: &str, fifos: &[&str]) -> Self {
        let temp = TempDir::new().unwrap();
        let dir = temp.path().to_path_buf();
        let home = dir.join("otto-home");
        fs::create_dir_all(&home).unwrap();
        fs::write(dir.join("otto.yml"), ottofile.replace("@DIR@", dir.to_str().unwrap())).unwrap();
        for name in fifos {
            mkfifo(&dir.join(format!("fifo.{name}")));
        }
        Self {
            _temp: temp,
            dir: dir.clone(),
            home,
            log: dir.join("otto.out"),
            otto: None,
        }
    }

    /// Start otto on this fixture, with its output captured to a file so a
    /// test can assert on otto's own per-task status lines (Rust's stdout is
    /// line buffered, so a line is readable as soon as it is written).
    fn run(&mut self, args: &[&str]) {
        let log = File::create(&self.log).unwrap();
        let mut cmd = Command::new(common::OTTO_BIN);
        common::isolate(&mut cmd, &self.home);
        cmd.arg("-o")
            .arg(self.dir.join("otto.yml"))
            .args(args)
            .current_dir(&self.dir)
            .stdin(Stdio::null())
            .stdout(Stdio::from(log.try_clone().unwrap()))
            .stderr(Stdio::from(log));
        self.otto = Some(cmd.spawn().expect("otto must start"));
    }

    /// Whether the named ordinary task has written its `task.<name>` marker.
    /// Items are counted separately, by [`Self::started_items`], so a test's
    /// "no item started" claim can never be satisfied or spoiled by a task that
    /// is not one of the group's items.
    fn started(&self, name: &str) -> bool {
        self.dir.join(format!("task.{name}")).exists()
    }

    /// Which foreach items have written their `item.<name>` marker, sorted.
    fn started_items(&self) -> Vec<String> {
        let mut names: Vec<String> = fs::read_dir(&self.dir)
            .unwrap()
            .filter_map(|entry| {
                let name = entry.ok()?.file_name().to_string_lossy().to_string();
                name.strip_prefix("item.").map(str::to_string)
            })
            .collect();
        names.sort();
        names
    }

    fn release(&self, name: &str) -> bool {
        release(&self.dir.join(format!("fifo.{name}")))
    }

    fn output(&self) -> String {
        fs::read_to_string(&self.log).unwrap_or_default()
    }

    /// Release every fifo, then wait for otto to exit, and return its output.
    fn finish(&mut self) -> String {
        self.release_everything();
        if let Some(mut child) = self.otto.take() {
            let deadline = Instant::now() + STEP_TIMEOUT;
            loop {
                match child.try_wait() {
                    Ok(Some(_)) => break,
                    _ if Instant::now() >= deadline => {
                        let _ = child.kill();
                        let _ = child.wait();
                        break;
                    }
                    _ => {
                        self.release_everything();
                        thread::sleep(POLL);
                    }
                }
            }
        }
        self.output()
    }

    fn release_everything(&self) {
        let Ok(entries) = fs::read_dir(&self.dir) else {
            return;
        };
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            if name.starts_with("fifo.") {
                release(&entry.path());
            }
        }
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        self.release_everything();
        if let Some(mut child) = self.otto.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
        self.release_everything();
    }
}

/// Ten items that block forever on a fifo apiece, under a global cap of two.
/// `@JOBS@` is the line under test; the control run deletes it.
const TEN_BLOCKING_ITEMS: &str = r#"
otto:
  api: 1

tasks:
  tail:
    foreach:
      items: [s01, s02, s03, s04, s05, s06, s07, s08, s09, s10]
      parallel: true
@JOBS@
    bash: |
      touch @DIR@/item.${item}
      read line < @DIR@/fifo.${item}
"#;

const TEN_ITEM_FIFOS: &[&str] = &["s01", "s02", "s03", "s04", "s05", "s06", "s07", "s08", "s09", "s10"];

/// Success criterion (a): ten never-exiting items under `-j 2` all start.
#[test]
fn jobs_all_starts_every_item_past_the_global_launch_cap() {
    let mut fixture = Fixture::new(&TEN_BLOCKING_ITEMS.replace("@JOBS@", "      jobs: all"), TEN_ITEM_FIFOS);
    fixture.run(&["-j", "2", "tail"]);

    let all_started = wait_for("all ten items start", || fixture.started_items().len() == 10);
    let started = fixture.started_items();
    assert!(
        all_started,
        "jobs: all must start every item regardless of the global cap; started {} of 10: {started:?}",
        started.len()
    );

    let output = fixture.finish();
    assert!(
        output.contains("[tail] finished successfully"),
        "the group must complete once released: {output}"
    );
}

/// The control for the test above, and the reason it proves anything: the same
/// fixture WITHOUT `jobs:` starts exactly `-j` items and leaves the other eight
/// silently unstarted. This is the defect the key exists to fix, measured
/// in-tree rather than quoted from the design doc.
#[test]
fn without_jobs_the_same_fixture_starts_only_the_cap() {
    let mut fixture = Fixture::new(&TEN_BLOCKING_ITEMS.replace("@JOBS@\n", ""), TEN_ITEM_FIFOS);
    fixture.run(&["-j", "2", "tail"]);

    assert!(
        wait_for("the cap's worth of items start", || fixture.started_items().len() == 2),
        "two items must start under -j 2: {:?}",
        fixture.started_items()
    );
    assert!(
        !ever_true(|| fixture.started_items().len() > 2),
        "without jobs:, nothing past the cap may start while the first two block: {:?}",
        fixture.started_items()
    );

    fixture.finish();
}

/// Success criterion (b): `jobs: 4` starts exactly four, and the fifth only
/// after one of the four exits.
#[test]
fn jobs_fixed_starts_exactly_n_and_the_next_only_after_one_exits() {
    let mut fixture = Fixture::new(&TEN_BLOCKING_ITEMS.replace("@JOBS@", "      jobs: 4"), TEN_ITEM_FIFOS);
    fixture.run(&["-j", "2", "tail"]);

    assert!(
        wait_for("four items start", || fixture.started_items().len() == 4),
        "jobs: 4 must start four items: {:?}",
        fixture.started_items()
    );
    let four = fixture.started_items();
    assert!(
        !ever_true(|| fixture.started_items().len() > 4),
        "a fifth item must not start while all four permits are held: {:?}",
        fixture.started_items()
    );

    // Release exactly one of the four. Its permit is the only thing that can
    // let a fifth item in, so a fifth marker appearing is the permit moving,
    // not time passing.
    assert!(fixture.release(&four[0]), "the first item must be blocked on its fifo");
    assert!(
        wait_for("a fifth item starts", || fixture.started_items().len() == 5),
        "the fifth item must start once a permit is freed: {:?}",
        fixture.started_items()
    );

    fixture.finish();
}

/// A `tty: true` task and a `jobs: all` group, both ready from the start, at
/// `-j 1`.
const TTY_BESIDE_GROUP: &str = r#"
otto:
  api: 1

tasks:
  term:
    tty: true
    bash: |
      touch @DIR@/task.term
      read line < @DIR@/fifo.term
  tail:
    foreach:
      items: [s1, s2, s3]
      parallel: true
      jobs: all
    bash: |
      touch @DIR@/item.${item}
      read line < @DIR@/fifo.${item}
"#;

/// Success criterion (c): a tty task and an exempt group never overlap, at
/// `max_parallel = 1`.
///
/// `-j 1` is the case that would have looked correct under the dead "the group
/// holds one shared permit" design: with a single permit, a tty task and a
/// group holding "the" permit could not have overlapped for reasons that had
/// nothing to do with the mechanism. Here nothing but the admission rules keeps
/// them apart - an exempt item does not touch the shared semaphore at all.
///
/// Which side the launch loop admits first is not fixed (both are ready in the
/// first pass), so the test asserts the property for whichever side won: the
/// other one must not have started, and must start as soon as the first is
/// released.
#[test]
fn a_tty_task_and_an_exempt_group_never_overlap_at_one_job() {
    let mut fixture = Fixture::new(TTY_BESIDE_GROUP, &["term", "s1", "s2", "s3"]);
    fixture.run(&["-j", "1", "term", "tail"]);

    assert!(
        wait_for("either side starts", || fixture.started("term")
            || !fixture.started_items().is_empty()),
        "neither the tty task nor the group ever started"
    );

    if fixture.started("term") {
        assert!(
            !ever_true(|| !fixture.started_items().is_empty()),
            "no exempt item may start while the tty task owns the terminal: {:?}",
            fixture.started_items()
        );
        assert!(fixture.release("term"), "the tty task must be blocked on its fifo");
        assert!(
            wait_for("the group starts after the tty task", || fixture.started_items().len()
                == 3),
            "the group must start once the tty task is released: {:?}",
            fixture.started_items()
        );
    } else {
        assert!(
            !ever_true(|| fixture.started("term")),
            "the tty task must not start beside exempt items: {:?}",
            fixture.started_items()
        );
        assert!(
            wait_for("all three items start", || fixture.started_items().len() == 3),
            "the exempt group runs one permit per item: {:?}",
            fixture.started_items()
        );
        for item in ["s1", "s2", "s3"] {
            assert!(fixture.release(item), "item {item} must be blocked on its fifo");
        }
        assert!(
            wait_for("the tty task starts after the group", || fixture.started("term")),
            "the tty task must start once the group is done"
        );
    }

    let output = fixture.finish();
    assert!(
        output.contains("[term] finished successfully"),
        "both sides must finish: {output}"
    );
}

/// A gate task that returns only once all three exempt items are running, so
/// the tty task behind it becomes ready WHILE the group is in flight. The gate
/// is an ordinary capped task and the group is exempt, so it can run beside
/// them even at `-j 1`.
const TTY_READY_DURING_GROUP: &str = r#"
otto:
  api: 1

tasks:
  gate:
    bash: |
      while [ "$(ls @DIR@/item.s* 2>/dev/null | wc -l)" -lt 3 ]; do sleep 0.05; done
  term:
    tty: true
    before: [gate]
    bash: |
      touch @DIR@/task.term
      read line < @DIR@/fifo.term
  tail:
    foreach:
      items: [s1, s2, s3]
      parallel: true
      jobs: all
    bash: |
      touch @DIR@/item.${item}
      read line < @DIR@/fifo.${item}
"#;

/// Success criterion (d), first half: a tty task that becomes ready while an
/// exempt group is in flight waits for the group instead of starting beside it.
#[test]
fn a_tty_task_becoming_ready_during_an_exempt_group_waits_for_it() {
    let mut fixture = Fixture::new(TTY_READY_DURING_GROUP, &["term", "s1", "s2", "s3"]);
    fixture.run(&["-j", "1", "tail", "term"]);

    assert!(
        wait_for("all three items start", || fixture.started_items().len() == 3),
        "the exempt group must start under -j 1: {:?}",
        fixture.started_items()
    );
    // The gate returning is what makes the tty task ready, and it can only
    // return once every item is running: this is the "becomes ready DURING"
    // half of the criterion, and it is a fact about otto's own status line
    // rather than about elapsed time.
    assert!(
        wait_for("the gate finishes", || fixture
            .output()
            .contains("[gate] finished successfully")),
        "the gate must finish while the group is in flight: {}",
        fixture.output()
    );
    assert!(
        !ever_true(|| fixture.started("term")),
        "a tty task that became ready during an exempt group must wait for it"
    );

    for item in ["s1", "s2", "s3"] {
        assert!(fixture.release(item), "item {item} must be blocked on its fifo");
    }
    assert!(
        wait_for("the tty task starts", || fixture.started("term")),
        "the tty task must start once the group drains: {}",
        fixture.output()
    );

    let output = fixture.finish();
    assert!(
        output.contains("[term] finished successfully"),
        "the run must complete: {output}"
    );
}

/// The mirror fixture. `hold` is an ordinary task that keeps one shared permit
/// while `term` (tty, released by `pre`) is admitted and left queuing for the
/// whole semaphore; the exempt group becomes ready when `hold` finishes, which
/// is during the window where the tty task is admitted but has not acquired a
/// single permit yet.
const GROUP_READY_DURING_TTY: &str = r#"
otto:
  api: 1

tasks:
  pre:
    bash: |
      echo pre done
  hold:
    bash: |
      touch @DIR@/task.hold
      read line < @DIR@/fifo.hold
  term:
    tty: true
    before: [pre]
    bash: |
      touch @DIR@/task.term
      read line < @DIR@/fifo.term
  tail:
    before: [hold]
    foreach:
      items: [s1, s2, s3]
      parallel: true
      jobs: all
    bash: |
      touch @DIR@/item.${item}
      read line < @DIR@/fifo.${item}
"#;

/// Success criterion (d), second half: an exempt group that becomes ready while
/// a tty task is in flight waits for it.
///
/// The sequencing is structural rather than timed. `pre` reporting is what
/// makes the tty task ready, and the launch pass that admits it runs before the
/// loop can consume the NEXT report - so releasing `hold` only after `pre`'s
/// status line has been printed guarantees the group becomes ready with the tty
/// task already counted in flight. That the tty task is in flight while still
/// queuing for its permits is `ActiveTasks::spawn`'s doing, and it is the whole
/// reason this rule can be decided in the loop.
#[test]
fn an_exempt_group_becoming_ready_during_a_tty_task_waits_for_it() {
    let mut fixture = Fixture::new(GROUP_READY_DURING_TTY, &["term", "hold", "s1", "s2", "s3"]);
    fixture.run(&["-j", "2", "hold", "term", "tail"]);

    assert!(
        wait_for("hold starts", || fixture.started("hold")),
        "the permit holder must start: {}",
        fixture.output()
    );
    assert!(
        wait_for("pre finishes", || fixture
            .output()
            .contains("[pre] finished successfully")),
        "pre must finish so the tty task becomes ready: {}",
        fixture.output()
    );
    assert!(
        !fixture.started("term"),
        "the tty task cannot have started: hold still holds a permit"
    );

    // Freeing the permit holder is what makes the group ready.
    assert!(fixture.release("hold"), "hold must be blocked on its fifo");
    assert!(
        wait_for("hold finishes", || fixture
            .output()
            .contains("[hold] finished successfully")),
        "hold must finish, which is what readies the group: {}",
        fixture.output()
    );
    assert!(
        wait_for("the tty task starts", || fixture.started("term")),
        "the tty task must take the terminal once the permit frees: {}",
        fixture.output()
    );
    assert!(
        !ever_true(|| !fixture.started_items().is_empty()),
        "the group became ready during the tty task and must wait for it: {:?}",
        fixture.started_items()
    );

    assert!(fixture.release("term"), "the tty task must be blocked on its fifo");
    assert!(
        wait_for("the group starts", || fixture.started_items().len() == 3),
        "the group must start once the tty task is released: {:?}",
        fixture.started_items()
    );

    let output = fixture.finish();
    assert!(
        output.contains("[tail] finished successfully"),
        "the run must complete: {output}"
    );
}

/// A group of two under `jobs: 1`, so the second item is admitted by the loop
/// and then sits queuing on the group's single permit, plus a task that can
/// only be launched after that queued item was popped from the ready queue.
const QUEUED_ITEM_DOES_NOT_BLOCK_THE_LOOP: &str = r#"
otto:
  api: 1

tasks:
  gate:
    bash: |
      while [ "$(ls @DIR@/item.s* 2>/dev/null | wc -l)" -lt 1 ]; do sleep 0.05; done
  after:
    before: [gate]
    bash: |
      touch @DIR@/task.after
      read line < @DIR@/fifo.after
  tail:
    foreach:
      items: [s1, s2]
      parallel: true
      jobs: 1
    bash: |
      touch @DIR@/item.${item}
      read line < @DIR@/fifo.${item}
"#;

/// **The other half of criterion (f): the permit is acquired INSIDE the
/// spawned body, in `execute_task`.**
///
/// Criterion (f)'s unit test
/// (`spawn_counts_a_task_in_flight_before_its_body_acquires_a_permit`) builds
/// its own `ActiveTasks` and its own async block, so it pins that
/// `ActiveTasks::spawn` inserts before its body runs and nothing else. Move
/// `semaphore.acquire_many` from inside the body
/// (`src/executor/scheduler/task_execution.rs`) up into `execute_task` above
/// `active.spawn` and that test stays green while the design's load-bearing
/// property is gone: the single-threaded launch loop would then block inside
/// `execute_task`, waiting on a permit, instead of going on to admit the rest
/// of the queue. Until this test, only a code comment stood on that site.
///
/// The shape, all barriers and no stopwatch: `s1` takes the group's only
/// permit and blocks on its fifo; `s2` is admitted and queues for that permit;
/// `gate` returns once an item is running, which makes `after` ready. `after`
/// can only be launched by a pass that got past `s2`, so its marker is the
/// proof that popping `s2` did not park the loop.
///
/// Break-the-code: hoist the acquire above `active.spawn` in `execute_task`
/// and `after` never starts, because the loop is parked on a permit `s1` will
/// not release until this test releases `s1`'s fifo - which it does not do
/// until after the assertion.
#[test]
fn a_queued_exempt_item_does_not_park_the_launch_loop() {
    let mut fixture = Fixture::new(QUEUED_ITEM_DOES_NOT_BLOCK_THE_LOOP, &["after", "s1", "s2"]);
    fixture.run(&["-j", "1", "tail", "after"]);

    assert!(
        wait_for("one item starts", || fixture.started_items().len() == 1),
        "jobs: 1 must start exactly one item: {:?}",
        fixture.started_items()
    );
    assert!(
        !ever_true(|| fixture.started_items().len() > 1),
        "the second item must still be queuing for the group's only permit: {:?}",
        fixture.started_items()
    );
    assert!(
        wait_for("the task behind the queued item starts", || fixture.started("after")),
        "the launch loop must keep admitting while an admitted item queues for its permit; \
         otto's output was: {}",
        fixture.output()
    );

    // And the queued item is genuinely queued, not dropped: releasing the
    // running one hands it the permit.
    let running = fixture.started_items();
    assert!(
        fixture.release(&running[0]),
        "the running item must be blocked on its fifo"
    );
    assert!(
        wait_for("the queued item starts", || fixture.started_items().len() == 2),
        "the queued item must start once the permit is freed: {:?}",
        fixture.started_items()
    );

    let output = fixture.finish();
    assert!(
        output.contains("[tail] finished successfully"),
        "the group must complete once released: {output}"
    );
}
