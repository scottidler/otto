#![cfg(test)]

use super::*;
use ratatui::layout::Rect;

/// A pane with a fixed name and status; renders nothing.
struct StubPane {
    name: String,
    status: PaneStatus,
}

impl Pane for StubPane {
    fn render(&self, _frame: &mut Frame, _area: Rect, _focused: bool) {}
    fn id(&self) -> &str {
        &self.name
    }
    fn update(&mut self) {}
    fn status(&self) -> PaneStatus {
        self.status.clone()
    }
    fn scroll_up(&mut self) {}
    fn scroll_down(&mut self) {}
    fn reset_scroll(&mut self) {}
}

fn stub(name: &str, status: PaneStatus) -> Box<dyn Pane> {
    Box::new(StubPane {
        name: name.to_string(),
        status,
    })
}

#[test]
fn running_task_names_reports_only_the_running_panes() {
    let mut layout = PaneLayout::new();
    layout.add_pane(stub("build", PaneStatus::Running));
    layout.add_pane(stub("test", PaneStatus::Completed));
    layout.add_pane(stub("lint", PaneStatus::Running));

    assert_eq!(
        layout.running_task_names(),
        vec!["build".to_string(), "lint".to_string()]
    );
}

#[test]
fn running_task_names_is_empty_when_nothing_runs() {
    let mut layout = PaneLayout::new();
    layout.add_pane(stub("build", PaneStatus::Completed));
    assert!(layout.running_task_names().is_empty());
}
