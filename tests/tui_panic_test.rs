//! A panic while the TUI owns the terminal must hand the terminal back.
//!
//! `install_panic_hook` existed with no end-to-end coverage: the only thing
//! referencing it was `src/tui/mod.rs` itself, and the audit that found this
//! recorded it as a known gap rather than closing it. The failure it guards
//! against is not subtle - a panic that unwinds past the guard's `Drop` leaves
//! the user in the alternate screen with raw mode on and no echo, and the only
//! way out is `reset(1)` blind.
//!
//! Testing it needs three things the ordinary harness does not give: a real
//! terminal (crossterm's raw mode is a `tcsetattr` on a tty and no-ops or errors
//! otherwise), an actual panic, and a way to observe the escape sequences the
//! process emitted on its way out. This file gets them by re-executing its own
//! test binary under `script`, which allocates a pty, with an env var that
//! selects the child role.

use std::process::Command;

/// Selects the child role. Set only by the parent test below.
const CHILD_ROLE_VAR: &str = "OTTO_TEST_TUI_PANIC_CHILD";

/// crossterm's `EnterAlternateScreen` / `LeaveAlternateScreen`.
const ENTER_ALT_SCREEN: &str = "\x1b[?1049h";
const LEAVE_ALT_SCREEN: &str = "\x1b[?1049l";

/// The child. Takes the terminal the way otto does, deliberately leaks the
/// guard so its `Drop` cannot do the restoring, then panics.
///
/// Leaking the guard is the whole point: with it in scope, `Drop` restores the
/// terminal during unwind and the test would pass whether or not the panic hook
/// exists. `claim_terminal_restore` hands the restore right to exactly one
/// claimant, so removing the guard from the race leaves the hook as the only
/// thing that can emit the sequence this test looks for.
fn child_takes_the_terminal_and_panics() -> ! {
    let guard = otto::tui::init_terminal().expect("init_terminal under a pty");
    std::mem::forget(guard);
    panic!("deliberate panic with the terminal taken over");
}

#[test]
fn a_panic_with_the_terminal_taken_over_restores_it() {
    // Child role: do the thing, never return.
    if std::env::var(CHILD_ROLE_VAR).is_ok() {
        child_takes_the_terminal_and_panics();
    }

    // Parent role: re-run this same test, as a child, under a pty.
    let exe = std::env::current_exe().expect("current test binary");
    let inner = format!(
        "{} --exact a_panic_with_the_terminal_taken_over_restores_it --nocapture",
        exe.display()
    );

    let output = Command::new("script")
        .args(["-q", "-e", "-c", &inner, "/dev/null"])
        .env(CHILD_ROLE_VAR, "1")
        // The child panics on purpose; its backtrace is noise here.
        .env("RUST_BACKTRACE", "0")
        .output()
        .expect("script should run the child under a pty");

    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    let combined = format!("{stdout}{stderr}");

    // If the child never got the terminal, the rest proves nothing. This is the
    // vacuous-pass guard: without a real pty `init_terminal` fails and the
    // child dies before entering the alternate screen.
    assert!(
        combined.contains(ENTER_ALT_SCREEN),
        "the child never entered the alternate screen, so this test asserted nothing. \
         Is `script` present and allocating a pty?\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );

    // It really did panic, rather than exiting cleanly through some other path.
    assert!(
        combined.contains("deliberate panic with the terminal taken over"),
        "the child did not panic as intended:\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );

    // The claim under test: the terminal was handed back on the way out.
    assert!(
        combined.contains(LEAVE_ALT_SCREEN),
        "a panic with the terminal taken over left the user in the alternate screen; \
         the panic hook did not run\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );

    // Ordering matters: leaving must come after entering. A stray `1049l` from
    // somewhere earlier in the stream would otherwise satisfy the check above.
    let entered = combined.find(ENTER_ALT_SCREEN).expect("checked above");
    let left = combined.rfind(LEAVE_ALT_SCREEN).expect("checked above");
    assert!(
        left > entered,
        "the alternate screen was left before it was entered, which means the restore \
         did not come from the panic\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
}
