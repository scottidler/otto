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

/// What the terminal did within one wait.
enum TerminalWait {
    /// Input is available; crossterm can be asked to read it.
    Ready,
    /// The timeout passed with nothing to read.
    Idle,
    /// The terminal is gone: the pty master closed, or the descriptor errored.
    HungUp,
}

/// Wait for terminal input without entering crossterm's reader on a terminal
/// that is gone.
///
/// crossterm 0.29's event source (`UnixInternalEventSource::try_read`) loops on
/// its tty read until it has an event or sees `WouldBlock`. A read that returns
/// `Ok(0)` or `Err(EIO)`, which is what a pty whose master has closed returns,
/// never breaks that loop and never re-checks the timeout, so `event::poll`
/// never returns. Reproduced by closing the pty master under `otto --tui`: the
/// main thread spun at 100% CPU, the shutdown flag two lines below the poll was
/// never read, and the run row stayed `running`.
///
/// So the wait happens here, with `poll(2)` on the terminal's own descriptor,
/// and a hangup (`POLLHUP`, `POLLERR`, `POLLNVAL`) is reported before crossterm
/// is asked to read anything. crossterm is entered only once input is known to
/// be present, and then with a zero timeout. What remains is a hangup landing
/// between this poll and that read, which needs a keystroke and the hangup in
/// the same instant.
///
/// When stdin is not a terminal (crossterm then opens `/dev/tty` itself), or
/// off unix, the check is skipped and crossterm's own poll runs as before.
fn wait_for_terminal_input(timeout: Duration) -> io::Result<TerminalWait> {
    #[cfg(unix)]
    {
        use std::io::IsTerminal;
        if io::stdin().is_terminal() {
            return poll_stdin(timeout);
        }
    }
    Ok(if crossterm::event::poll(timeout)? {
        TerminalWait::Ready
    } else {
        TerminalWait::Idle
    })
}

#[cfg(unix)]
fn poll_stdin(timeout: Duration) -> io::Result<TerminalWait> {
    poll_terminal_fd(libc::STDIN_FILENO, timeout)
}

/// `poll(2)` one descriptor for input or hangup. The whole hangup detection is
/// here, split out from [`poll_stdin`] so it can be tested against a pty whose
/// master is closed without touching the process's real stdin.
#[cfg(unix)]
fn poll_terminal_fd(fd: libc::c_int, timeout: Duration) -> io::Result<TerminalWait> {
    let mut fds = [libc::pollfd {
        fd,
        events: libc::POLLIN,
        revents: 0,
    }];
    let millis = libc::c_int::try_from(timeout.as_millis()).unwrap_or(libc::c_int::MAX);
    // SAFETY: `fds` is one initialized `pollfd` that outlives the call, and the
    // count passed is its length.
    let ready = unsafe { libc::poll(fds.as_mut_ptr(), 1, millis) };
    if ready < 0 {
        let err = io::Error::last_os_error();
        // A signal landed during the wait (SIGHUP itself, or a resize). The
        // loop comes back around and reads the shutdown flag the handler set.
        if err.kind() == io::ErrorKind::Interrupted {
            return Ok(TerminalWait::Idle);
        }
        return Err(err);
    }
    if ready == 0 {
        return Ok(TerminalWait::Idle);
    }
    if fds[0].revents & (libc::POLLHUP | libc::POLLERR | libc::POLLNVAL) != 0 {
        return Ok(TerminalWait::HungUp);
    }
    Ok(TerminalWait::Ready)
}

#[cfg(all(test, unix))]
mod hangup_tests {
    use super::{TerminalWait, poll_terminal_fd};
    use std::io::Write;
    use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
    use std::time::Duration;

    /// A pty pair, so a test can be the terminal and then close it. This is the
    /// one condition that made `otto --tui` spin forever, and the only way to
    /// reproduce it is to own the master and close it.
    fn open_pty() -> (OwnedFd, OwnedFd) {
        let mut master = -1;
        let mut slave = -1;
        // SAFETY: both out-params are valid; the three optional args are null.
        //
        // `null_mut()` for all three, not `null()`: Apple's libc declares the
        // `termios` and `winsize` parameters `*mut` where Linux declares them
        // `*const`, so `null()` is a type error on macOS and compiles fine
        // here. `*mut T` coerces to `*const T`, so one spelling satisfies both.
        // Caught by the macOS bash-3.2 CI job, which is the only place in this
        // repo that compiles the test tree against Apple's headers.
        let rc = unsafe {
            libc::openpty(
                &mut master,
                &mut slave,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
            )
        };
        assert_eq!(rc, 0, "openpty: {}", std::io::Error::last_os_error());
        // SAFETY: two fresh descriptors openpty just returned, owned by nothing else.
        unsafe { (OwnedFd::from_raw_fd(master), OwnedFd::from_raw_fd(slave)) }
    }

    #[test]
    fn a_closed_master_reports_a_hangup_rather_than_blocking() {
        let (master, slave) = open_pty();
        drop(master); // the terminal window closes
        // Would have blocked forever inside crossterm's reader; here it returns.
        let waited = poll_terminal_fd(slave.as_raw_fd(), Duration::from_secs(5)).expect("poll");
        assert!(
            matches!(waited, TerminalWait::HungUp),
            "a closed pty master must read as a hangup"
        );
    }

    #[test]
    fn pending_input_reads_as_ready() {
        let (master, slave) = open_pty();
        // SAFETY: `master` is an open, owned writable descriptor.
        let mut writer = unsafe { std::fs::File::from_raw_fd(master.as_raw_fd()) };
        std::mem::forget(master); // `writer` owns the fd now
        // A full line: the slave is in canonical mode here, where poll does not
        // report input until a line is complete. otto puts the real terminal in
        // raw mode; this test only needs poll to see POLLIN.
        writer.write_all(b"q\n").expect("write to pty");
        let waited = poll_terminal_fd(slave.as_raw_fd(), Duration::from_secs(5)).expect("poll");
        assert!(
            matches!(waited, TerminalWait::Ready),
            "pending input must read as ready"
        );
    }

    #[test]
    fn a_quiet_terminal_times_out_idle() {
        let (_master, slave) = open_pty();
        let waited = poll_terminal_fd(slave.as_raw_fd(), Duration::from_millis(50)).expect("poll");
        assert!(
            matches!(waited, TerminalWait::Idle),
            "a quiet terminal must time out idle"
        );
    }
}

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
            terminal
                .draw(|f| {
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

            match wait_for_terminal_input(timeout)? {
                TerminalWait::HungUp => {
                    return Err(io::Error::other("the terminal hung up"));
                }
                TerminalWait::Ready => {
                    if crossterm::event::poll(Duration::ZERO)?
                        && let Event::Key(key) = event::read()?
                        && key.kind == KeyEventKind::Press
                    {
                        self.handle_key_event(key);
                    }
                }
                TerminalWait::Idle => {}
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
                "{}f/Enter: Exit Fullscreen | ↑↓/jk: Scroll | Home: Top | End/G: Follow | q/Esc: Quit | ^C: Cancel run",
                page_info
            )
        } else if total_pages > 1 {
            format!(
                "{}PgUp/PgDn: Change Page | f/Enter: Fullscreen | Tab/←→: Switch | ↑↓/jk: Scroll | End/G: Follow | q/Esc: Quit | ^C: Cancel run",
                page_info
            )
        } else {
            format!(
                "{}f/Enter: Fullscreen | Tab/←→: Switch Pane | ↑↓/jk: Scroll | Home: Top | End/G: Follow | q/Esc: Quit | ^C: Cancel run",
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
            // The way back to live output. `Home` had no counterpart, and
            // `Down` only resumes following once the user has walked all the
            // way back to the bottom a line at a time, so a scrolled pane was
            // effectively stuck showing old output for the rest of the run.
            KeyCode::End | KeyCode::Char('G') => {
                if let Some(pane) = self.layout.focused_pane_mut() {
                    pane.scroll_to_bottom();
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
