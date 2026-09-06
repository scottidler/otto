#![cfg(test)]

use super::*;
use crate::tui::pane::{Pane, PaneStatus, ScrollState};
use std::cell::Cell;
use std::rc::Rc;

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

/// A pane whose scroll keys drive a real [`ScrollState`], so a key binding and
/// the scroll behaviour it triggers are checked together rather than through a
/// "was I called" flag that could be satisfied by the wrong call.
struct ScrollSpy {
    scroll: Rc<Cell<ScrollState>>,
    total: usize,
    visible: usize,
}

impl Pane for ScrollSpy {
    fn render(&self, _frame: &mut ratatui::Frame, _area: ratatui::layout::Rect, _focused: bool) {}
    fn id(&self) -> &str {
        "spy"
    }
    fn update(&mut self) {}
    fn status(&self) -> PaneStatus {
        PaneStatus::Running
    }
    fn scroll_up(&mut self) {
        let mut scroll = self.scroll.get();
        scroll.up(self.total, self.visible);
        self.scroll.set(scroll);
    }
    fn scroll_down(&mut self) {
        let mut scroll = self.scroll.get();
        scroll.down(self.total, self.visible);
        self.scroll.set(scroll);
    }
    fn reset_scroll(&mut self) {
        let mut scroll = self.scroll.get();
        scroll.top();
        self.scroll.set(scroll);
    }
    fn scroll_to_bottom(&mut self) {
        let mut scroll = self.scroll.get();
        scroll.bottom();
        self.scroll.set(scroll);
    }
}

fn spy(scroll: &Rc<Cell<ScrollState>>) -> Box<dyn Pane> {
    Box::new(ScrollSpy {
        scroll: Rc::clone(scroll),
        total: 100,
        visible: 20,
    })
}

fn app_with_spy() -> (TuiApp, Rc<Cell<ScrollState>>) {
    let scroll = Rc::new(Cell::new(ScrollState::new()));
    let mut app = TuiApp::new();
    app.layout_mut().add_pane(spy(&scroll));
    (app, scroll)
}

/// Nothing resumed following once the user scrolled: `Home` jumped to the top
/// and `Down` only re-enables follow at the very bottom, one line at a time.
#[test]
fn end_and_g_resume_following_after_scrolling_up() {
    for code in [KeyCode::End, KeyCode::Char('G')] {
        let (mut app, scroll) = app_with_spy();
        app.handle_key_event(key(KeyCode::Char('k'), KeyModifiers::NONE));
        assert!(!scroll.get().is_following(), "k stops following");

        // `G` arrives from a real terminal with Shift held; the binding must
        // not depend on the modifier either way.
        let modifiers = if code == KeyCode::Char('G') {
            KeyModifiers::SHIFT
        } else {
            KeyModifiers::NONE
        };
        app.handle_key_event(key(code, modifiers));

        assert!(scroll.get().is_following(), "{code:?} returns the pane to live output");
        assert_eq!(scroll.get().start_line(100, 20), 80, "{code:?} jumps to the newest row");
    }
}

/// The status bar has to name the binding in all three of its shapes -
/// single page, multi page, fullscreen - or a key that exists is a key nobody
/// finds.
#[test]
fn the_status_bar_names_the_follow_binding() {
    // Wide enough that the longest of the three lines is not clipped.
    let mut terminal =
        ratatui::Terminal::new(ratatui::backend::TestBackend::new(200, 3)).expect("the test backend needs no terminal");

    for (shape, panes, fullscreen) in [("single page", 1, false), ("fullscreen", 1, true), ("paged", 17, false)] {
        let scroll = Rc::new(Cell::new(ScrollState::new()));
        let mut app = TuiApp::new();
        for _ in 0..panes {
            app.layout_mut().add_pane(spy(&scroll));
        }
        app.fullscreen_mode = fullscreen;

        let completed = terminal
            .draw(|frame| {
                let area = frame.area();
                app.render_status_bar(frame, area);
            })
            .expect("drawing into the test backend succeeds");
        let text: String = (0..200)
            .map(|x| completed.buffer.cell((x, 1)).expect("inside the buffer").symbol())
            .collect();

        assert!(
            text.contains("End/G: Follow"),
            "the {shape} status bar must name End/G: {text:?}"
        );
    }
}
