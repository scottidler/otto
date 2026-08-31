use super::layout::PaneLayout;
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::{
    Terminal,
    backend::Backend,
    layout::{Constraint, Direction, Layout},
    style::{Color, Style},
    text::Line,
    widgets::{Block, Borders, Paragraph},
};
use std::io;
use std::time::{Duration, Instant};

const TUI_TICK_RATE_MS: u64 = 100; // 10 FPS

/// Main TUI application
pub struct TuiApp {
    layout: PaneLayout,
    should_quit: bool,
    last_tick: Instant,
    tick_rate: Duration,
    fullscreen_mode: bool,
    shutdown_flag: Option<std::sync::Arc<std::sync::atomic::AtomicBool>>,
}

impl Default for TuiApp {
    fn default() -> Self {
        Self::new()
    }
}

impl TuiApp {
    pub fn new() -> Self {
        Self {
            layout: PaneLayout::new(),
            should_quit: false,
            last_tick: Instant::now(),
            tick_rate: Duration::from_millis(TUI_TICK_RATE_MS),
            fullscreen_mode: false,
            shutdown_flag: None,
        }
    }

    /// Names of the tasks still running when the dashboard closed.
    ///
    /// Read from the panes because the scheduler has already moved into its
    /// own task by then; the panes mirror its status broadcasts.
    pub fn running_task_names(&self) -> Vec<String> {
        self.layout.running_task_names()
    }

    pub fn set_shutdown_flag(&mut self, flag: std::sync::Arc<std::sync::atomic::AtomicBool>) {
        self.shutdown_flag = Some(flag);
    }

    pub fn layout_mut(&mut self) -> &mut PaneLayout {
        &mut self.layout
    }

    pub fn run<B: Backend>(&mut self, terminal: &mut Terminal<B>) -> io::Result<()> {
        loop {
            // Draw UI
            terminal.draw(|f| {
                let chunks = Layout::default()
                    .direction(Direction::Vertical)
                    .constraints([
                        Constraint::Min(1),    // Main content area
                        Constraint::Length(3), // Status bar (3 lines with border)
                    ])
                    .split(f.area());

                // Render main content
                if self.fullscreen_mode {
                    self.layout.render_fullscreen(f, chunks[0]);
                } else {
                    self.layout.render(f, chunks[0]);
                }

                // Render status bar
                self.render_status_bar(f, chunks[1]);
            })
            .map_err(|e| io::Error::other(e.to_string()))?;

            // Handle events with timeout
            let timeout = self
                .tick_rate
                .checked_sub(self.last_tick.elapsed())
                .unwrap_or_else(|| Duration::from_secs(0));

            if crossterm::event::poll(timeout)?
                && let Event::Key(key) = event::read()?
                && key.kind == KeyEventKind::Press
            {
                self.handle_key_event(key);
            }

            if self.last_tick.elapsed() >= self.tick_rate {
                self.on_tick();
                self.last_tick = Instant::now();
            }

            if let Some(ref flag) = self.shutdown_flag
                && flag.load(std::sync::atomic::Ordering::SeqCst)
            {
                self.should_quit = true;
            }

            if self.should_quit {
                break;
            }
        }

        Ok(())
    }

    fn on_tick(&mut self) {
        // Update all panes (receive from broadcast channels)
        self.layout.update_all();
    }

    fn render_status_bar(&self, frame: &mut ratatui::Frame, area: ratatui::layout::Rect) {
        let total_pages = self.layout.total_pages();
        let current_page = self.layout.current_page() + 1; // 1-indexed for display

        let page_info = if total_pages > 1 {
            format!(" [Page {}/{}] ", current_page, total_pages)
        } else {
            String::new()
        };

        let help_text = if self.fullscreen_mode {
            format!(
                "{}f/Enter: Exit Fullscreen | ↑↓/jk: Scroll | Home: Top | q/Esc: Quit | ^C: Cancel run",
                page_info
            )
        } else if total_pages > 1 {
            format!(
                "{}PgUp/PgDn: Change Page | f/Enter: Fullscreen | Tab/←→: Switch | ↑↓/jk: Scroll | q/Esc: Quit | ^C: Cancel run",
                page_info
            )
        } else {
            format!(
                "{}f/Enter: Fullscreen | Tab/←→: Switch Pane | ↑↓/jk: Scroll | Home: Top | q/Esc: Quit | ^C: Cancel run",
                page_info
            )
        };

        let status_line = Line::from(help_text);
        let paragraph = Paragraph::new(status_line).block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::DarkGray)),
        );

        frame.render_widget(paragraph, area);
    }

    fn handle_key_event(&mut self, key: KeyEvent) {
        // Ctrl+C first, and on the full event: the call site used to pass only
        // `key.code`, so the modifier never reached this match and Ctrl+C was
        // dead for as long as the TUI owned the terminal (raw mode means the
        // kernel never turns it into SIGINT either).
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
            self.should_quit = true;
            return;
        }

        // `q`, Esc and Ctrl+C all do the same thing, and that is deliberate.
        // There used to be a `cancel_requested` flag distinguishing "stop the
        // run" from "stop watching it", with two tests asserting the
        // difference - but no production code ever read it: `execute_with_tui`
        // (`app.rs`) cancels unconditionally once the dashboard closes,
        // because nothing is watching the run any more and leaving invisible
        // children running is the bug that change fixed. The flag asserted a
        // distinction production does not make, so it is gone rather than
        // left to imply one.
        match key.code {
            KeyCode::Char('q') | KeyCode::Esc => {
                self.should_quit = true;
            }
            KeyCode::Char('f') | KeyCode::Enter => {
                self.fullscreen_mode = !self.fullscreen_mode;
            }
            KeyCode::Tab | KeyCode::Right => {
                self.layout.focus_next();
            }
            KeyCode::BackTab | KeyCode::Left => {
                self.layout.focus_prev();
            }
            KeyCode::Up | KeyCode::Char('k') => {
                if let Some(pane) = self.layout.focused_pane_mut() {
                    pane.scroll_up();
                }
            }
            KeyCode::Down | KeyCode::Char('j') => {
                if let Some(pane) = self.layout.focused_pane_mut() {
                    pane.scroll_down();
                }
            }
            KeyCode::Home => {
                if let Some(pane) = self.layout.focused_pane_mut() {
                    pane.reset_scroll();
                }
            }
            KeyCode::PageDown => {
                self.layout.next_page();
            }
            KeyCode::PageUp => {
                self.layout.prev_page();
            }
            _ => {}
        }
    }
}

#[path = "app_tests.rs"]
mod tests;
