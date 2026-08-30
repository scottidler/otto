#![cfg(test)]

use super::*;
use serial_test::serial;
use std::cell::RefCell;
use std::rc::Rc;

/// Counts restores instead of touching a terminal.
struct CountingRestore {
    calls: Rc<RefCell<usize>>,
    fail: bool,
}

impl TerminalRestore for CountingRestore {
    fn restore(&mut self) -> io::Result<()> {
        *self.calls.borrow_mut() += 1;
        if self.fail { Err(io::Error::other("no terminal")) } else { Ok(()) }
    }
}

fn guard(calls: &Rc<RefCell<usize>>, fail: bool) -> TerminalGuard<CountingRestore> {
    TerminalGuard::new(CountingRestore {
        calls: Rc::clone(calls),
        fail,
    })
}

#[test]
fn drop_restores_the_terminal() {
    let calls = Rc::new(RefCell::new(0));
    drop(guard(&calls, false));
    assert_eq!(*calls.borrow(), 1, "Drop must restore exactly once");
}

#[test]
fn an_explicit_restore_is_not_repeated_by_drop() {
    let calls = Rc::new(RefCell::new(0));
    let mut g = guard(&calls, false);
    g.restore().expect("restore succeeds");
    g.restore().expect("second restore is a no-op");
    drop(g);
    assert_eq!(*calls.borrow(), 1, "restore must happen exactly once in total");
}

#[test]
fn a_failing_restore_reports_the_error_and_is_not_retried() {
    let calls = Rc::new(RefCell::new(0));
    let mut g = guard(&calls, true);
    let err = g.restore().expect_err("the fake restore fails");
    assert_eq!(err.to_string(), "no terminal");
    drop(g);
    assert_eq!(*calls.borrow(), 1, "a failed restore is not retried on Drop");
}

#[test]
#[serial]
fn the_restore_claim_is_taken_exactly_once() {
    claim_terminal_takeover();
    assert!(claim_terminal_restore(), "the first claimant restores");
    assert!(!claim_terminal_restore(), "a second claimant must not restore again");
}

#[test]
#[serial]
fn the_panic_restore_is_a_noop_without_a_takeover() {
    // The claim is already false here unless a takeover happened, which is
    // the whole point: a panic in a non-TUI run writes nothing.
    TERMINAL_TAKEOVER.store(false, Ordering::SeqCst);
    restore_terminal_on_panic();
    assert!(!claim_terminal_restore());
}
