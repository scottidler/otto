mod app;
mod layout;
mod pane;

pub use app::TuiApp;
pub use layout::PaneLayout;
pub use pane::{Pane, TaskPane, wrap_line};

use crossterm::{
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use ratatui::{Terminal, backend::CrosstermBackend};
use std::io;
use std::sync::Once;
use std::sync::atomic::{AtomicBool, Ordering};

/// True while the TUI owns the terminal (raw mode + alternate screen).
///
/// The panic hook is process-global and fires for every panic, TUI or not;
/// this flag is what stops a panic in a plain `otto build` from writing
/// alternate-screen escape sequences at a terminal that never entered one.
static TERMINAL_TAKEOVER: AtomicBool = AtomicBool::new(false);

/// Only install the panic hook once, however many times the TUI starts.
static PANIC_HOOK: Once = Once::new();

/// Record that the TUI has taken the terminal over.
fn claim_terminal_takeover() {
    TERMINAL_TAKEOVER.store(true, Ordering::SeqCst);
}

/// Take the "terminal needs restoring" right, if it is still outstanding.
///
/// Returns `true` exactly once per takeover: whoever gets it does the restore.
/// Both the guard's `Drop` and the panic hook race for it, and a double
/// `LeaveAlternateScreen` would drop the user out of a screen they are legitimately
/// in (the shell's own pager, say), so the claim has to be exclusive.
fn claim_terminal_restore() -> bool {
    TERMINAL_TAKEOVER.swap(false, Ordering::SeqCst)
}

/// Something that put the terminal into a state it has to be taken back out of.
///
/// A trait rather than a concrete terminal so [`TerminalGuard`]'s exactly-once
/// semantics are testable without a live TTY.
pub trait TerminalRestore {
    fn restore(&mut self) -> io::Result<()>;
}

/// Restores the terminal on the way out, whatever the way out is.
///
/// Every `?` between `init_terminal()` and the end of the TUI run used to leak
/// raw mode and the alternate screen: the only restore call sat after all of
/// them. Owning the restore in `Drop` is what makes an early error, a panic
/// unwinding through the run, and a clean quit all land in the same place.
pub struct TerminalGuard<T: TerminalRestore> {
    inner: T,
    restored: bool,
}

impl<T: TerminalRestore> TerminalGuard<T> {
    pub fn new(inner: T) -> Self {
        Self { inner, restored: false }
    }

    pub fn get_mut(&mut self) -> &mut T {
        &mut self.inner
    }

    /// Restore now. Idempotent: later calls, `Drop` included, do nothing.
    ///
    /// Callers restore explicitly when they have something to print afterwards -
    /// a message written before the alternate screen is left is a message nobody
    /// ever sees.
    pub fn restore(&mut self) -> io::Result<()> {
        if self.restored {
            return Ok(());
        }
        self.restored = true;
        self.inner.restore()
    }
}

impl<T: TerminalRestore> Drop for TerminalGuard<T> {
    fn drop(&mut self) {
        if let Err(e) = self.restore() {
            // Best effort: a restore fails when the terminal is gone, and
            // then this write fails too. `eprintln!` would panic inside a
            // destructor for a message nobody can see.
            use std::io::Write;
            let _ = writeln!(io::stderr(), "otto: failed to restore the terminal: {e}");
        }
    }
}

/// The real terminal the TUI draws on.
pub struct TuiTerminal {
    terminal: Terminal<CrosstermBackend<io::Stdout>>,
}

impl TuiTerminal {
    pub fn terminal_mut(&mut self) -> &mut Terminal<CrosstermBackend<io::Stdout>> {
        &mut self.terminal
    }
}

impl TerminalRestore for TuiTerminal {
    fn restore(&mut self) -> io::Result<()> {
        // Whoever restores first wins the claim; the loser does nothing. This
        // used to call the claim and throw the answer away, then restore
        // regardless, which is the double `LeaveAlternateScreen` the claim
        // exists to prevent: a panic (hook restores, then the guard's `Drop`
        // restores again during unwind) or a second signal
        // (`restore_terminal_best_effort` then `Drop`) dropped the user out of
        // whatever screen they had legitimately gone back to.
        if !claim_terminal_restore() {
            return Ok(());
        }
        disable_raw_mode()?;
        execute!(self.terminal.backend_mut(), LeaveAlternateScreen)?;
        self.terminal.show_cursor()?;
        Ok(())
    }
}

impl TerminalGuard<TuiTerminal> {
    pub fn terminal_mut(&mut self) -> &mut Terminal<CrosstermBackend<io::Stdout>> {
        self.inner.terminal_mut()
    }
}

/// Best-effort terminal restore for the paths that never reach the guard's
/// `Drop`: a panic unwinding past it, and the second-signal `process::exit`
/// (`app.rs`, `install_stop_handler`), which runs no destructors at all.
///
/// Errors are dropped on purpose: this runs while the process is already dying,
/// and a `?` here would replace the message the user needs with an io error.
pub fn restore_terminal_best_effort() {
    if !claim_terminal_restore() {
        return;
    }
    let _ = disable_raw_mode();
    let _ = execute!(io::stdout(), LeaveAlternateScreen, crossterm::cursor::Show);
}

/// Chain a terminal restore onto the process panic hook.
///
/// Without it a panic inside the TUI unwinds straight past the guard's `Drop`
/// on the abort path and leaves the user with a terminal that echoes nothing.
pub fn install_panic_hook() {
    PANIC_HOOK.call_once(|| {
        let previous = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            restore_terminal_best_effort();
            previous(info);
        }));
    });
}

/// Initialize the terminal for TUI mode.
///
/// The returned guard restores the terminal when it is dropped, so callers can
/// use `?` freely after this point.
pub fn init_terminal() -> io::Result<TerminalGuard<TuiTerminal>> {
    install_panic_hook();
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    if let Err(e) = execute!(stdout, EnterAlternateScreen) {
        let _ = disable_raw_mode();
        return Err(e);
    }
    claim_terminal_takeover();
    let backend = CrosstermBackend::new(stdout);
    match Terminal::new(backend) {
        Ok(terminal) => Ok(TerminalGuard::new(TuiTerminal { terminal })),
        Err(e) => {
            restore_terminal_best_effort();
            Err(e)
        }
    }
}

#[path = "mod_tests.rs"]
mod tests;
