#![cfg(test)]

use super::*;

fn key(code: KeyCode, modifiers: KeyModifiers) -> KeyEvent {
    KeyEvent::new(code, modifiers)
}

#[test]
fn ctrl_c_quits_and_cancels_the_run() {
    let mut app = TuiApp::new();
    app.handle_key_event(key(KeyCode::Char('c'), KeyModifiers::CONTROL));
    assert!(app.should_quit, "Ctrl+C closes the dashboard");
    assert!(
        app.cancel_requested(),
        "Ctrl+C cancels the run, it does not just stop watching"
    );
}

#[test]
fn a_bare_c_is_not_ctrl_c() {
    let mut app = TuiApp::new();
    app.handle_key_event(key(KeyCode::Char('c'), KeyModifiers::NONE));
    assert!(!app.should_quit);
    assert!(!app.cancel_requested());
}

#[test]
fn q_quits_without_cancelling() {
    let mut app = TuiApp::new();
    app.handle_key_event(key(KeyCode::Char('q'), KeyModifiers::NONE));
    assert!(app.should_quit);
    assert!(
        !app.cancel_requested(),
        "q closes the dashboard; cancellation is decided by the caller"
    );
}

#[test]
fn f_toggles_fullscreen() {
    let mut app = TuiApp::new();
    app.handle_key_event(key(KeyCode::Char('f'), KeyModifiers::NONE));
    assert!(app.fullscreen_mode);
    app.handle_key_event(key(KeyCode::Char('f'), KeyModifiers::NONE));
    assert!(!app.fullscreen_mode);
}

#[test]
fn running_task_names_is_empty_without_panes() {
    let app = TuiApp::new();
    assert!(app.running_task_names().is_empty());
}
