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
fn bottom_resumes_following_from_anywhere() {
    let mut scroll = ScrollState::new();
    scroll.top();
    assert!(!scroll.is_following());
    scroll.bottom();
    assert!(scroll.is_following(), "End/G resumes auto-scroll");
    assert_eq!(scroll.start_line(100, 20), 80);
}

#[test]
fn bottom_after_one_up_follows_again() {
    let mut scroll = ScrollState::new();
    scroll.up(100, 20);
    assert!(!scroll.is_following());
    scroll.bottom();
    assert!(scroll.is_following());
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

// =====================================================================
// TaskPane render window
// =====================================================================

/// Render `pane` into a `width` x `height` terminal and return the rows
/// *inside* the pane's border, trailing blanks trimmed.
fn render_rows(pane: &TaskPane, width: u16, height: u16) -> Vec<String> {
    let mut terminal = ratatui::Terminal::new(ratatui::backend::TestBackend::new(width, height))
        .expect("the test backend needs no terminal");
    let completed = terminal
        .draw(|frame| {
            let area = frame.area();
            pane.render(frame, area, false);
        })
        .expect("drawing into the test backend succeeds");
    let buffer = completed.buffer;
    (1..height - 1)
        .map(|y| {
            (1..width - 1)
                .map(|x| buffer.cell((x, y)).expect("inside the buffer").symbol())
                .collect::<String>()
                .trim_end()
                .to_string()
        })
        .collect()
}

/// The clipping bug: the window used to be `visible_height` *unwrapped* lines,
/// wrapped afterwards, so a wrapped line pushed the newest output past the
/// bottom of the pane and `Paragraph` silently threw it away. A pane in follow
/// mode must show the end of the buffer, whatever the lines wrap to.
#[test]
fn a_following_pane_shows_the_newest_line_even_when_a_line_wraps() {
    // 12x6 terminal -> a 10x4 inner area. One line three inner-widths long
    // (3 wrapped rows) plus three short ones is 6 rows for a 4-row viewport.
    let (mut pane, _tx) = pane_with_channel(8);
    pane.push_line("A".repeat(30));
    for i in 1..=3 {
        pane.push_line(format!("L{i}"));
    }

    let rows = render_rows(&pane, 12, 6);

    assert_eq!(rows.len(), 4, "the inner area is four rows: {rows:?}");
    assert_eq!(
        rows.last().map(String::as_str),
        Some("L3"),
        "follow mode must end on the newest buffer line: {rows:?}"
    );
    assert_eq!(
        rows,
        vec![
            "AAAAAAAAAA".to_string(),
            "L1".to_string(),
            "L2".to_string(),
            "L3".to_string()
        ],
        "the window is the last four wrapped rows: {rows:?}"
    );
}

/// The scroll offset counts wrapped rows too, or Up from the bottom of a
/// wrapping buffer would skip a whole wrapped line's worth of output.
#[test]
fn scrolling_up_moves_one_wrapped_row() {
    let (mut pane, _tx) = pane_with_channel(8);
    pane.push_line("A".repeat(30));
    for i in 1..=3 {
        pane.push_line(format!("L{i}"));
    }

    // Renders once so the pane knows its viewport, then scrolls up one row.
    let _ = render_rows(&pane, 12, 6);
    pane.scroll_up();
    let rows = render_rows(&pane, 12, 6);

    assert_eq!(
        rows,
        vec![
            "AAAAAAAAAA".to_string(),
            "AAAAAAAAAA".to_string(),
            "L1".to_string(),
            "L2".to_string()
        ],
        "one Up is one wrapped row: {rows:?}"
    );
}
