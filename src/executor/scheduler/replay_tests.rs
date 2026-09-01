//! Unit tests for buffered-foreach replay (design doc
//! `docs/design/2026-08-31-buffered-foreach-computed-envs-required-params.md`,
//! Phase 4). The end-to-end emission-order properties live in
//! `tests/foreach_buffer_test.rs`; what is pinned here is the machinery those
//! properties rest on: the cursor's item-order rule, the bounded reader, and
//! the cancellation plan's six-state table.

#![cfg(test)]

use super::*;

use std::collections::{HashMap, HashSet};
use std::io::Cursor as IoCursor;

fn names(items: &[&str]) -> Vec<String> {
    items.iter().map(|s| (*s).to_string()).collect()
}

fn set(items: &[&str]) -> HashSet<String> {
    items.iter().map(|s| (*s).to_string()).collect()
}

fn pending(line: &str) -> PendingBlock {
    PendingBlock {
        status_line: format!("{line}\n"),
        status_to_stderr: false,
        drain: Vec::new(),
    }
}

fn group(items: &[&str]) -> ReplayGroup {
    ReplayGroup {
        items: names(items),
        cursor: 0,
        pending: HashMap::new(),
        emitted: HashSet::new(),
    }
}

fn cursor_over(parent: &str, items: &[&str]) -> ReplayCursor {
    let mut cursor = ReplayCursor::default();
    for item in items {
        cursor.parent_of.insert((*item).to_string(), parent.to_string());
    }
    cursor.parents.push(parent.to_string());
    cursor.groups.insert(parent.to_string(), group(items));
    cursor
}

// =========================================================================
// The cursor: item order, and the skipped-item stall it exists to prevent
// =========================================================================

/// The whole point of the feature: a block is held until its slot comes up,
/// however early it finished.
#[test]
fn take_ready_holds_a_finished_item_until_its_slot_comes_up() {
    let mut cursor = cursor_over("say", &["say:alpha", "say:beta", "say:gamma"]);

    assert!(cursor.record("say:gamma", pending("gamma done")));
    assert!(
        cursor.take_ready("say").is_empty(),
        "gamma finished first, but alpha's slot is still the cursor's"
    );

    assert!(cursor.record("say:beta", pending("beta done")));
    assert!(cursor.take_ready("say").is_empty(), "beta is still behind alpha");

    assert!(cursor.record("say:alpha", pending("alpha done")));
    let emitted: Vec<String> = cursor.take_ready("say").into_iter().map(|(name, _)| name).collect();
    assert_eq!(
        emitted,
        names(&["say:alpha", "say:beta", "say:gamma"]),
        "the cursor item drains every already-finished successor behind it, in item order"
    );
}

/// A task that is not a buffered subtask is not the cursor's business: the
/// caller prints it immediately.
#[test]
fn record_declines_a_task_that_is_not_buffered() {
    let mut cursor = cursor_over("say", &["say:alpha"]);
    assert!(!cursor.record("chatty", pending("chatty done")));
    assert!(!cursor.record("say", pending("parent done")));
}

/// The cursor advances on ANY terminal state, so a skipped first item cannot
/// stall the group. This is the case a report-channel-only cursor would hang
/// on: both skip paths send no `TaskReport`.
#[test]
fn a_skipped_first_item_does_not_stall_its_successors() {
    let mut cursor = cursor_over("say", &["say:alpha", "say:beta"]);
    assert!(cursor.record("say:alpha", pending("alpha skipped (up to date)")));
    assert!(cursor.record("say:beta", pending("beta done")));
    let emitted: Vec<String> = cursor.take_ready("say").into_iter().map(|(name, _)| name).collect();
    assert_eq!(emitted, names(&["say:alpha", "say:beta"]));
}

/// A second transition for an item whose block already printed is dropped
/// rather than printing the block twice.
#[test]
fn an_already_emitted_item_is_never_emitted_again() {
    let mut cursor = cursor_over("say", &["say:alpha"]);
    assert!(cursor.record("say:alpha", pending("alpha done")));
    assert_eq!(cursor.take_ready("say").len(), 1);
    assert!(cursor.record("say:alpha", pending("alpha done again")));
    assert!(cursor.take_ready("say").is_empty());
    assert!(cursor.take_remaining("say").is_empty());
}

/// The end-of-group backstop runs the cursor to the end and emits whatever the
/// four transition sites left behind, without inventing blocks for items that
/// recorded nothing.
#[test]
fn take_remaining_flushes_the_backlog_and_ends_the_group() {
    let mut cursor = cursor_over("say", &["say:alpha", "say:beta", "say:gamma"]);
    assert!(cursor.record("say:gamma", pending("gamma done")));
    let emitted: Vec<String> = cursor.take_remaining("say").into_iter().map(|(name, _)| name).collect();
    assert_eq!(emitted, names(&["say:gamma"]));
    assert!(cursor.take_remaining("say").is_empty(), "the group is finished");
}

/// Under `--tui` the terminal leg is already suppressed, so the cursor is built
/// empty and every hook returns early.
#[test]
fn the_cursor_is_inert_under_tui() {
    let parent = buffered_parent("say", &["say:alpha"]);
    let subtask = buffered_subtask("say:alpha");
    let tasks = vec![parent, subtask];
    assert!(ReplayCursor::new(&tasks, true).is_empty());
    assert!(!ReplayCursor::new(&tasks, false).is_empty());
}

/// An item the run set never reached would stall its group's cursor forever, so
/// the item list is filtered to the tasks actually in the run.
#[test]
fn the_cursor_drops_items_that_are_not_in_the_run_set() {
    let parent = buffered_parent("say", &["say:alpha", "say:ghost"]);
    let tasks = vec![parent, buffered_subtask("say:alpha")];
    let cursor = ReplayCursor::new(&tasks, false);
    assert_eq!(cursor.groups["say"].items, names(&["say:alpha"]));
    assert!(cursor.parent_of("say:ghost").is_none());
}

/// A foreach that did NOT set `buffer: true` carries a display-order map all
/// the same (Phase 3 builds it for every expansion); it must not be buffered.
#[test]
fn an_unbuffered_foreach_group_is_not_tracked() {
    let mut parent = buffered_parent("say", &["say:alpha"]);
    parent.buffered = false;
    let mut subtask = buffered_subtask("say:alpha");
    subtask.buffered = false;
    assert!(ReplayCursor::new(&[parent, subtask], false).is_empty());
}

fn bare_task(name: &str) -> Task {
    Task::new(
        name.to_string(),
        None,
        vec![],
        vec![],
        vec![],
        HashMap::new(),
        HashMap::new(),
        String::new(),
    )
}

fn buffered_parent(name: &str, items: &[&str]) -> Task {
    let mut task = bare_task(name);
    task.is_virtual_parent = true;
    task.buffered = true;
    task.foreach_display_order = Some(names(items));
    task
}

fn buffered_subtask(name: &str) -> Task {
    let mut task = bare_task(name);
    task.buffered = true;
    task
}

// =========================================================================
// Cancellation: ordered, but never stopping early
// =========================================================================

/// The design doc's six-state table, one row at a time, plus the property the
/// table exists for: a killed or unstarted item does not swallow the completed
/// block of a LATER item behind it.
#[test]
fn the_cancellation_plan_is_ordered_and_never_stops_early() {
    let items = names(&[
        "say:alpha",   // already emitted during the run
        "say:beta",    // active body, child launched and killed
        "say:gamma",   // ready-queued, never started
        "say:delta",   // active body, child never launched
        "say:epsilon", // terminal, or a report sent and never consumed
    ]);
    let emitted = set(&["say:alpha"]);
    let recorded = set(&["say:epsilon"]);
    let mut statuses = HashMap::new();
    statuses.insert("say:beta".to_string(), TaskStatus::Running);
    statuses.insert("say:delta".to_string(), TaskStatus::Running);
    statuses.insert("say:gamma".to_string(), TaskStatus::Pending);
    // Only beta's child ever opened a log.
    let log_exists = |name: &str| name == "say:beta";

    let plan = plan_cancelled_group(&items, &emitted, &recorded, &statuses, &log_exists);

    assert_eq!(
        plan,
        vec![
            ("say:alpha", CancelledItem::AlreadyEmitted),
            ("say:beta", CancelledItem::KilledChild),
            ("say:gamma", CancelledItem::NeverStarted),
            ("say:delta", CancelledItem::NeverStarted),
            ("say:epsilon", CancelledItem::Block),
        ],
        "exactly one outcome per item, in item order, with epsilon's finished block \
         still emitted behind a killed beta and two unstarted items"
    );
}

/// A body that is `Running` but never opened a log has no path to print, so it
/// must not be handed one. This is the state `ActiveTasks` cannot tell apart on
/// its own: it tracks spawned bodies, not children.
#[test]
fn a_running_body_with_no_log_did_not_start() {
    let items = names(&["say:alpha"]);
    let mut statuses = HashMap::new();
    statuses.insert("say:alpha".to_string(), TaskStatus::Running);
    let plan = plan_cancelled_group(&items, &HashSet::new(), &HashSet::new(), &statuses, &|_| false);
    assert_eq!(plan, vec![("say:alpha", CancelledItem::NeverStarted)]);
}

/// A recorded transition wins over every status signal: the logs are complete,
/// so the block prints in full even though the task is still `Running` in the
/// status map (the report was sent and never consumed).
#[test]
fn a_report_sent_but_never_consumed_still_prints_its_block() {
    let items = names(&["say:alpha"]);
    let mut statuses = HashMap::new();
    statuses.insert("say:alpha".to_string(), TaskStatus::Running);
    let plan = plan_cancelled_group(&items, &HashSet::new(), &set(&["say:alpha"]), &statuses, &|_| true);
    assert_eq!(plan, vec![("say:alpha", CancelledItem::Block)]);
}

// =========================================================================
// Bounded streaming
// =========================================================================

#[test]
fn read_bounded_chunk_yields_one_line_at_a_time() {
    let mut reader = IoCursor::new(b"one\ntwo\n".to_vec());
    let mut chunk = Vec::new();
    assert!(read_bounded_chunk(&mut reader, &mut chunk).unwrap());
    assert_eq!(chunk, b"one\n");
    chunk.clear();
    assert!(read_bounded_chunk(&mut reader, &mut chunk).unwrap());
    assert_eq!(chunk, b"two\n");
    chunk.clear();
    assert!(!read_bounded_chunk(&mut reader, &mut chunk).unwrap());
    assert!(chunk.is_empty(), "end of file yields nothing");
}

#[test]
fn read_bounded_chunk_reports_a_final_line_with_no_newline() {
    let mut reader = IoCursor::new(b"tail".to_vec());
    let mut chunk = Vec::new();
    assert!(!read_bounded_chunk(&mut reader, &mut chunk).unwrap());
    assert_eq!(chunk, b"tail");
}

/// A log line longer than the chunk cap is emitted in pieces rather than
/// accumulated whole: peak memory during replay is the buffer, never the log.
#[test]
fn read_bounded_chunk_caps_an_endless_line() {
    let huge = vec![b'x'; REPLAY_CHUNK_BYTES * 2 + 7];
    let mut reader = IoCursor::new(huge.clone());
    let mut chunk = Vec::new();
    let mut pieces = 0;
    let mut total = 0;
    loop {
        chunk.clear();
        let complete = read_bounded_chunk(&mut reader, &mut chunk).unwrap();
        if chunk.is_empty() {
            break;
        }
        assert!(chunk.len() <= REPLAY_CHUNK_BYTES, "a chunk must never exceed the cap");
        assert!(!complete, "there is no newline anywhere in this log");
        total += chunk.len();
        pieces += 1;
    }
    assert_eq!(total, huge.len());
    assert_eq!(pieces, 3);
}

// =========================================================================
// Block rendering
// =========================================================================

fn read_back(dir: &std::path::Path, name: &str, contents: &str, no_prefix: bool) -> String {
    let path = dir.join(name);
    std::fs::write(&path, contents).unwrap();
    let mut out: Vec<u8> = Vec::new();
    stream_log(&mut out, &path, "say:alpha", no_prefix).unwrap();
    String::from_utf8(out).unwrap()
}

#[test]
fn stream_log_prefixes_every_line_and_honors_no_prefix() {
    let dir = tempfile::TempDir::new().unwrap();
    let prefixed = read_back(dir.path(), "stdout.log", "one\ntwo\n", false);
    assert_eq!(prefixed.lines().count(), 2);
    assert!(
        prefixed.lines().all(|line| line.contains("say:alpha")),
        "every replayed line keeps its attribution: {prefixed}"
    );
    assert_eq!(read_back(dir.path(), "raw.log", "one\ntwo\n", true), "one\ntwo\n");
}

/// A log whose last line has no newline must not leave the status line that
/// follows it stranded mid-line.
#[test]
fn stream_log_closes_an_unterminated_last_line() {
    let dir = tempfile::TempDir::new().unwrap();
    let out = read_back(dir.path(), "stdout.log", "tail", true);
    assert_eq!(out, "tail\n");
}

/// A subtask that printed nothing, or was skipped before it could, contributes
/// nothing but its status line.
#[test]
fn stream_log_is_a_no_op_for_an_empty_or_missing_log() {
    let dir = tempfile::TempDir::new().unwrap();
    assert_eq!(read_back(dir.path(), "empty.log", "", false), "");
    let mut out: Vec<u8> = Vec::new();
    stream_log(&mut out, &dir.path().join("absent.log"), "say:alpha", false).unwrap();
    assert!(out.is_empty(), "a missing log is not an error");
}

/// The marker names the stream, the condition, and the log path: a bool could
/// not have said any of those.
#[test]
fn the_truncation_marker_names_the_condition_and_the_log() {
    let block = ReplayBlock {
        task_name: "say:alpha".to_string(),
        kind: BlockKind::Logs,
        stdout_log: PathBuf::from("/run/tasks/say:alpha/stdout.log"),
        stderr_log: PathBuf::from("/run/tasks/say:alpha/stderr.log"),
        status_line: None,
        status_to_stderr: false,
        drain: Vec::new(),
    };
    let marker = truncation_marker(
        &block,
        &DrainIssue {
            stream: OutputType::Stdout,
            condition: DrainCondition::Timeout,
        },
    );
    assert!(marker.contains("say:alpha"), "{marker}");
    assert!(marker.contains("truncated"), "{marker}");
    assert!(marker.contains("output processing timed out"), "{marker}");
    assert!(marker.contains("/run/tasks/say:alpha/stdout.log"), "{marker}");

    let join = truncation_marker(
        &block,
        &DrainIssue {
            stream: OutputType::Stderr,
            condition: DrainCondition::JoinError,
        },
    );
    assert!(join.contains("stderr"), "{join}");
    assert!(join.contains("did not join"), "{join}");
    assert!(join.contains("/run/tasks/say:alpha/stderr.log"), "{join}");
}
