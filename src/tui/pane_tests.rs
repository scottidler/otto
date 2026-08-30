#![cfg(test)]

use super::*;

// =====================================================================
// wrap_line
// =====================================================================

#[test]
fn wrap_line_leaves_a_short_line_alone() {
    assert_eq!(wrap_line("abc", 10), vec!["abc"]);
}

#[test]
fn wrap_line_splits_at_the_width() {
    assert_eq!(wrap_line("abcdef", 2), vec!["ab", "cd", "ef"]);
}

#[test]
fn wrap_line_terminates_at_width_zero() {
    // The hang this closes: width 0 chunks never shortened the remainder.
    assert_eq!(wrap_line("abc", 0), vec!["a", "b", "c"]);
}

#[test]
fn wrap_line_splits_on_characters_not_bytes() {
    assert_eq!(wrap_line("héllo", 2), vec!["hé", "ll", "o"]);
}

#[test]
fn wrap_line_keeps_a_blank_line() {
    assert_eq!(wrap_line("", 10), vec![""]);
}

// =====================================================================
// ScrollState
// =====================================================================

#[test]
fn a_following_pane_renders_the_bottom() {
    let scroll = ScrollState::new();
    assert!(scroll.is_following());
    assert_eq!(scroll.start_line(100, 20), 80);
}

#[test]
fn the_first_up_moves_one_line_not_to_the_top() {
    let mut scroll = ScrollState::new();
    scroll.up(100, 20);
    assert_eq!(
        scroll.start_line(100, 20),
        79,
        "Up from the bottom is one line, not a jump"
    );
    assert!(!scroll.is_following());
}

#[test]
fn up_stops_at_the_top() {
    let mut scroll = ScrollState::new();
    scroll.top();
    scroll.up(100, 20);
    assert_eq!(scroll.start_line(100, 20), 0);
}

#[test]
fn down_returns_to_following_at_the_bottom() {
    let mut scroll = ScrollState::new();
    scroll.up(100, 20);
    scroll.down(100, 20);
    assert_eq!(scroll.start_line(100, 20), 80);
    assert!(scroll.is_following(), "reaching the bottom re-enables auto-scroll");
}

#[test]
fn down_from_the_top_advances_one_line() {
    let mut scroll = ScrollState::new();
    scroll.top();
    scroll.down(100, 20);
    assert_eq!(scroll.start_line(100, 20), 1);
    assert!(!scroll.is_following());
}

#[test]
fn a_buffer_shorter_than_the_viewport_always_starts_at_zero() {
    let mut scroll = ScrollState::new();
    assert_eq!(scroll.start_line(3, 20), 0);
    scroll.up(3, 20);
    assert_eq!(scroll.start_line(3, 20), 0);
    scroll.down(3, 20);
    assert_eq!(scroll.start_line(3, 20), 0);
}

#[test]
fn a_zero_height_viewport_does_not_panic() {
    let mut scroll = ScrollState::new();
    scroll.up(10, 0);
    scroll.down(10, 0);
    assert_eq!(scroll.start_line(10, 0), 10);
}

// =====================================================================
// TaskPane drain
// =====================================================================

fn pane_with_channel(capacity: usize) -> (TaskPane, broadcast::Sender<TaskOutput>) {
    let (tx, _) = broadcast::channel::<TaskOutput>(capacity);
    (TaskPane::new("build".to_string(), tx.clone()), tx)
}

#[test]
fn a_lagged_pane_keeps_draining() {
    let (mut pane, tx) = pane_with_channel(2);
    // Overrun the channel: the receiver is now behind by one.
    for i in 0..3 {
        tx.send(TaskOutput {
            task_name: "build".to_string(),
            content: format!("line {i}\n"),
            stream_type: crate::executor::output::OutputType::Stdout,
            timestamp: SystemTime::now(),
        })
        .expect("the pane's receiver keeps the channel open");
    }

    pane.update();

    let lines: Vec<&String> = pane.output_buffer.iter().collect();
    assert!(
        lines.iter().any(|l| l.contains("dropped")),
        "the drop is reported, not silent: {lines:?}"
    );
    assert!(
        lines.iter().any(|l| l.as_str() == "line 2"),
        "draining continues past the lag: {lines:?}"
    );
}

#[test]
fn output_for_another_task_is_ignored() {
    let (mut pane, tx) = pane_with_channel(8);
    tx.send(TaskOutput {
        task_name: "test".to_string(),
        content: "not mine\n".to_string(),
        stream_type: crate::executor::output::OutputType::Stdout,
        timestamp: SystemTime::now(),
    })
    .expect("send succeeds");

    pane.update();

    assert!(pane.output_buffer.is_empty());
}
