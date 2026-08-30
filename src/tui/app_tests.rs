#![cfg(test)]

use super::*;

fn key(code: KeyCode, modifiers: KeyModifiers) -> KeyEvent {
    KeyEvent::new(code, modifiers)
}

/// Ctrl+C closes the dashboard. It used to also set a `cancel_requested`
/// flag, and this test used to assert that flag - but no production code read
/// it: `execute_with_tui` cancels unconditionally once the dashboard is gone.
/// The pair of tests asserting a "quit vs. cancel" distinction was pinning
/// something production does not do, so they now assert what it does.
#[test]
fn ctrl_c_closes_the_dashboard() {
    let mut app = TuiApp::new();
    app.handle_key_event(key(KeyCode::Char('c'), KeyModifiers::CONTROL));
    assert!(app.should_quit, "Ctrl+C closes the dashboard");
}

#[test]
fn a_bare_c_is_not_ctrl_c() {
    let mut app = TuiApp::new();
    app.handle_key_event(key(KeyCode::Char('c'), KeyModifiers::NONE));
    assert!(!app.should_quit);
}

/// `q`, Esc and Ctrl+C are the same key as far as the run is concerned:
/// closing the dashboard cancels, because nothing is watching any more.
#[test]
fn q_and_esc_close_the_dashboard_the_same_way_ctrl_c_does() {
    for code in [KeyCode::Char('q'), KeyCode::Esc] {
        let mut app = TuiApp::new();
        app.handle_key_event(key(code, KeyModifiers::NONE));
        assert!(app.should_quit, "{code:?} closes the dashboard");
    }
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
