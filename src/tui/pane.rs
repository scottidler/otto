use crate::executor::output::{TaskMessage, TaskOutput, TuiTaskStatus};
use ratatui::{
    Frame,
    layout::Rect,
    style::{Color, Style},
    text::Line,
    widgets::{Block, Borders, Paragraph},
};
use std::cell::Cell;
use std::collections::VecDeque;
use std::time::{Duration, SystemTime};
use tokio::sync::broadcast;

/// Status of a task displayed in a pane
#[derive(Debug, Clone, PartialEq)]
pub enum PaneStatus {
    Pending,
    Running,
    Completed,
    Failed,
    Skipped,
}

impl PaneStatus {
    pub fn symbol(&self) -> &str {
        match self {
            PaneStatus::Pending => "○",
            PaneStatus::Running => "●",
            PaneStatus::Completed => "✓",
            PaneStatus::Failed => "✗",
            PaneStatus::Skipped => "⊘",
        }
    }

    pub fn color(&self) -> Color {
        match self {
            PaneStatus::Pending => Color::Gray,
            PaneStatus::Running => Color::Green,
            PaneStatus::Completed => Color::Green,
            PaneStatus::Failed => Color::Red,
            PaneStatus::Skipped => Color::Yellow,
        }
    }
}

/// Split `line` into pieces no wider than `max_width` characters.
///
/// `max_width` is clamped to at least 1: a pane narrow enough to report an
/// inner width of 0 (any pane in a 1-2 column terminal, and every pane while
/// the terminal is being resized) used to loop forever here, because a chunk
/// of width 0 never shortens the remainder.
pub fn wrap_line(line: &str, max_width: usize) -> Vec<&str> {
    let width = max_width.max(1);
    if line.is_empty() {
        return vec![""];
    }

    let mut pieces = Vec::new();
    let mut remaining = line;
    while !remaining.is_empty() {
        let end = remaining
            .char_indices()
            .nth(width)
            .map(|(idx, _)| idx)
            .unwrap_or(remaining.len());
        pieces.push(&remaining[..end]);
        remaining = &remaining[end..];
    }
    pieces
}

/// Where a pane's viewport sits in its output buffer.
///
/// Pulled out of `TaskPane` as plain data so the scroll behaviour is testable
/// without a terminal. `follow` (auto-scroll) and `offset` used to disagree:
/// the pane rendered the bottom while `offset` still said 0, so the first Up
/// keypress jumped to the top of the buffer instead of up one line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ScrollState {
    offset: usize,
    follow: bool,
}

impl Default for ScrollState {
    fn default() -> Self {
        Self::new()
    }
}

impl ScrollState {
    pub fn new() -> Self {
        Self {
            offset: 0,
            follow: true,
        }
    }

    /// The highest first-line that still fills the viewport.
    fn max_offset(total: usize, visible: usize) -> usize {
        total.saturating_sub(visible)
    }

    /// The first buffer line to render.
    pub fn start_line(&self, total: usize, visible: usize) -> usize {
        let max = Self::max_offset(total, visible);
        if self.follow { max } else { self.offset.min(max) }
    }

    /// True while the pane sticks to the newest output.
    pub fn is_following(&self) -> bool {
        self.follow
    }

    /// Scroll up one line, starting from what the user is actually looking at.
    pub fn up(&mut self, total: usize, visible: usize) {
        let current = self.start_line(total, visible);
        self.follow = false;
        self.offset = current.saturating_sub(1);
    }

    /// Scroll down one line; reaching the bottom re-enables follow.
    pub fn down(&mut self, total: usize, visible: usize) {
        let max = Self::max_offset(total, visible);
        let current = self.start_line(total, visible);
        if current >= max {
            self.follow = true;
            self.offset = max;
            return;
        }
        self.offset = current + 1;
        self.follow = self.offset >= max;
    }

    /// Jump to the top and stay there.
    pub fn top(&mut self) {
        self.offset = 0;
        self.follow = false;
    }
}

/// Trait for renderable panes
pub trait Pane {
    fn render(&self, frame: &mut Frame, area: Rect, focused: bool);

    /// Get the pane's identifier (task name)
    fn id(&self) -> &str;

    fn update(&mut self);

    /// Get current status
    fn status(&self) -> PaneStatus;

    /// Scroll up one line.
    fn scroll_up(&mut self);

    /// Scroll down one line.
    ///
    /// No height argument: the pane remembers the viewport height it last
    /// rendered at, which is the only number that is actually right. The
    /// hardcoded 20 this replaces scrolled by a screenful that matched no pane
    /// on any terminal except by accident.
    fn scroll_down(&mut self);

    fn reset_scroll(&mut self);
}

/// A pane that displays output from a single task
pub struct TaskPane {
    task_name: String,
    status: PaneStatus,
    output_rx: broadcast::Receiver<TaskOutput>,
    message_rx: Option<broadcast::Receiver<TaskMessage>>,
    output_buffer: VecDeque<String>,
    scroll: ScrollState,
    /// Viewport height from the last render, so scrolling moves by what the
    /// user can see. `Cell` because `render` takes `&self`.
    visible_height: Cell<u16>,
    max_buffer_lines: usize,
    start_time: Option<SystemTime>,
    duration: Option<Duration>,
}

impl TaskPane {
    pub fn new(task_name: String, output_tx: broadcast::Sender<TaskOutput>) -> Self {
        Self {
            task_name: task_name.clone(),
            status: PaneStatus::Pending,
            output_rx: output_tx.subscribe(),
            message_rx: None,
            output_buffer: VecDeque::new(),
            scroll: ScrollState::new(),
            visible_height: Cell::new(0),
            max_buffer_lines: 1000, // Ring buffer
            start_time: None,
            duration: None,
        }
    }

    pub fn set_message_channel(&mut self, message_tx: broadcast::Sender<TaskMessage>) {
        self.message_rx = Some(message_tx.subscribe());
    }

    fn tui_status_to_pane_status(status: &TuiTaskStatus) -> PaneStatus {
        match status {
            TuiTaskStatus::Pending => PaneStatus::Pending,
            TuiTaskStatus::Running => PaneStatus::Running,
            TuiTaskStatus::Completed => PaneStatus::Completed,
            TuiTaskStatus::Failed => PaneStatus::Failed,
            TuiTaskStatus::Skipped => PaneStatus::Skipped,
        }
    }

    /// Append one line, honoring the ring buffer.
    fn push_line(&mut self, line: String) {
        self.output_buffer.push_back(line);
        if self.output_buffer.len() > self.max_buffer_lines {
            self.output_buffer.pop_front();
        }
    }

    /// Drain one task message into the buffer.
    fn apply_message(&mut self, message: TaskMessage) {
        match message {
            TaskMessage::Output(output) => {
                if output.task_name == self.task_name {
                    for line in output.content.lines() {
                        self.push_line(line.to_string());
                    }
                }
            }
            TaskMessage::StatusChange { task_name, status, .. } => {
                if task_name == self.task_name {
                    self.status = Self::tui_status_to_pane_status(&status);
                    let status_msg = match status {
                        TuiTaskStatus::Skipped => "○ Task skipped (up to date)",
                        TuiTaskStatus::Running => "● Task running",
                        TuiTaskStatus::Completed => "✓ Task completed",
                        TuiTaskStatus::Failed => "✗ Task failed",
                        TuiTaskStatus::Pending => "◌ Task pending",
                    };
                    self.push_line(status_msg.to_string());
                }
            }
            TaskMessage::Started { task_name, timestamp } => {
                if task_name == self.task_name {
                    self.status = PaneStatus::Running;
                    self.start_time = Some(timestamp);
                    self.push_line("● Task started".to_string());
                }
            }
            TaskMessage::Finished {
                task_name,
                status,
                duration_ms,
                ..
            } => {
                if task_name == self.task_name {
                    self.status = Self::tui_status_to_pane_status(&status);
                    self.duration = Some(Duration::from_millis(duration_ms));
                    let status_msg = match status {
                        TuiTaskStatus::Completed => "✓ Task completed successfully",
                        TuiTaskStatus::Failed => "✗ Task failed",
                        TuiTaskStatus::Skipped => "○ Task skipped",
                        _ => "Task finished",
                    };
                    self.push_line(status_msg.to_string());
                }
            }
        }
    }

    /// The marker a pane shows when the broadcast channel outran it.
    ///
    /// A lagged receiver is not an empty one: `while let Ok(..)` treated the
    /// two the same and stopped draining for the rest of the run, so a pane
    /// that fell behind once went silent permanently.
    fn lagged_marker(dropped: u64) -> String {
        format!("… {dropped} line(s) dropped: output arrived faster than the dashboard could read it")
    }
}

impl Pane for TaskPane {
    fn render(&self, frame: &mut Frame, area: Rect, focused: bool) {
        let mut title = format!(" {} {} ", self.task_name, self.status.symbol());

        if let Some(dur) = &self.duration {
            title.push_str(&format!(" ({:.1}s) ", dur.as_secs_f64()));
        } else if let Some(start) = &self.start_time
            && let Ok(elapsed) = SystemTime::now().duration_since(*start)
        {
            title.push_str(&format!(" ({:.1}s) ", elapsed.as_secs_f64()));
        }

        // A pane the user scrolled off the bottom stops showing new output; say
        // so in the title rather than looking like a task that stopped talking.
        if !self.scroll.is_following() {
            title.push_str("[scroll paused] ");
        }

        let border_color = if focused { Color::Yellow } else { self.status.color() };

        let block = Block::default()
            .title(title)
            .borders(Borders::ALL)
            .border_style(Style::default().fg(border_color));

        let inner_area = block.inner(area);
        frame.render_widget(block, area);

        // Remember the viewport height so key handling scrolls by a real screenful.
        self.visible_height.set(inner_area.height);

        let visible_height = inner_area.height as usize;
        let total_lines = self.output_buffer.len();
        let start_line = self.scroll.start_line(total_lines, visible_height);
        let end_line = (start_line + visible_height).min(total_lines);

        let max_width = inner_area.width as usize;
        let mut wrapped_lines: Vec<Line> = Vec::new();
        for line in self
            .output_buffer
            .iter()
            .skip(start_line)
            .take(end_line.saturating_sub(start_line))
        {
            wrapped_lines.extend(wrap_line(line, max_width).into_iter().map(Line::from));
        }

        let paragraph = Paragraph::new(wrapped_lines);
        frame.render_widget(paragraph, inner_area);
    }

    fn id(&self) -> &str {
        &self.task_name
    }

    fn update(&mut self) {
        // Drain the status-message channel. Lagged is a skip, not a stop.
        let mut messages: Vec<TaskMessage> = Vec::new();
        let mut dropped: u64 = 0;
        if let Some(rx) = &mut self.message_rx {
            loop {
                match rx.try_recv() {
                    Ok(message) => messages.push(message),
                    Err(broadcast::error::TryRecvError::Lagged(n)) => dropped += n,
                    Err(_) => break,
                }
            }
        }
        if dropped > 0 {
            self.push_line(Self::lagged_marker(dropped));
        }
        for message in messages {
            self.apply_message(message);
        }

        // Drain the per-task output channel, same rule.
        let mut outputs: Vec<TaskOutput> = Vec::new();
        let mut dropped: u64 = 0;
        loop {
            match self.output_rx.try_recv() {
                Ok(output) => outputs.push(output),
                Err(broadcast::error::TryRecvError::Lagged(n)) => dropped += n,
                Err(_) => break,
            }
        }
        if dropped > 0 {
            self.push_line(Self::lagged_marker(dropped));
        }
        for output in outputs {
            if output.task_name != self.task_name {
                continue;
            }
            for line in output.content.lines() {
                self.push_line(line.to_string());
            }
        }
    }

    fn status(&self) -> PaneStatus {
        self.status.clone()
    }

    fn scroll_up(&mut self) {
        let visible = self.visible_height.get() as usize;
        self.scroll.up(self.output_buffer.len(), visible);
    }

    fn scroll_down(&mut self) {
        let visible = self.visible_height.get() as usize;
        self.scroll.down(self.output_buffer.len(), visible);
    }

    fn reset_scroll(&mut self) {
        self.scroll.top();
    }
}

#[path = "pane_tests.rs"]
mod tests;
