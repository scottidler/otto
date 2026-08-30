use super::pane::{Pane, PaneStatus};
use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout, Rect},
};

/// Manages dynamic pane layout
pub struct PaneLayout {
    panes: Vec<Box<dyn Pane>>,
    focused_index: usize,
    page: usize,
    panes_per_page: usize,
}

impl PaneLayout {
    pub fn new() -> Self {
        Self {
            panes: Vec::new(),
            focused_index: 0,
            page: 0,
            panes_per_page: 16,
        }
    }

    pub fn next_page(&mut self) {
        let total_pages = self.panes.len().div_ceil(self.panes_per_page);
        if total_pages > 1 {
            self.page = (self.page + 1) % total_pages;
            self.focused_index = 0;
        }
    }

    pub fn prev_page(&mut self) {
        let total_pages = self.panes.len().div_ceil(self.panes_per_page);
        if total_pages > 1 {
            self.page = if self.page == 0 { total_pages - 1 } else { self.page - 1 };
            self.focused_index = 0;
        }
    }

    pub fn current_page(&self) -> usize {
        self.page
    }

    pub fn total_pages(&self) -> usize {
        self.panes.len().div_ceil(self.panes_per_page)
    }

    pub fn add_pane(&mut self, pane: Box<dyn Pane>) {
        self.panes.push(pane);
    }

    pub fn update_all(&mut self) {
        for pane in &mut self.panes {
            pane.update();
        }
    }

    pub fn render(&self, frame: &mut Frame, area: Rect) {
        if self.panes.is_empty() {
            return;
        }

        // Calculate which panes to show on current page
        let start_idx = self.page * self.panes_per_page;
        let end_idx = (start_idx + self.panes_per_page).min(self.panes.len());
        let visible_panes: Vec<&Box<dyn Pane>> = self.panes[start_idx..end_idx].iter().collect();

        if visible_panes.is_empty() {
            return;
        }

        let grid_areas = self.calculate_grid_for_count(area, visible_panes.len());

        for (i, pane) in visible_panes.iter().enumerate() {
            if let Some(pane_area) = grid_areas.get(i) {
                let focused = i == self.focused_index;
                pane.render(frame, *pane_area, focused);
            }
        }
    }

    pub fn render_fullscreen(&self, frame: &mut Frame, area: Rect) {
        if self.panes.is_empty() {
            return;
        }

        // Render only the focused pane in fullscreen (accounting for pagination)
        let start_idx = self.page * self.panes_per_page;
        let absolute_index = start_idx + self.focused_index;
        if let Some(pane) = self.panes.get(absolute_index) {
            pane.render(frame, area, true);
        }
    }

    fn calculate_grid_for_count(&self, area: Rect, num_panes: usize) -> Vec<Rect> {
        if num_panes == 0 {
            return vec![];
        }

        // Determine grid dimensions based on pane count
        let (rows, cols) = match num_panes {
            1 => (1, 1),
            2 => (1, 2),
            3..=4 => (2, 2),
            5..=6 => (2, 3),
            7..=9 => (3, 3),
            10..=12 => (3, 4),
            _ => (4, 4), // Max 16 visible panes per page
        };

        let row_constraints: Vec<Constraint> = (0..rows).map(|_| Constraint::Percentage(100 / rows as u16)).collect();

        let row_layout = Layout::default()
            .direction(Direction::Vertical)
            .constraints(row_constraints)
            .split(area);

        let col_constraints: Vec<Constraint> = (0..cols).map(|_| Constraint::Percentage(100 / cols as u16)).collect();

        let mut grid_areas = Vec::new();
        for row_area in row_layout.iter() {
            let col_layout = Layout::default()
                .direction(Direction::Horizontal)
                .constraints(&col_constraints)
                .split(*row_area);

            grid_areas.extend(col_layout.iter().copied());
        }

        // Return only as many areas as we have panes
        grid_areas.truncate(num_panes);
        grid_areas
    }

    pub fn focus_next(&mut self) {
        let start_idx = self.page * self.panes_per_page;
        let end_idx = (start_idx + self.panes_per_page).min(self.panes.len());
        let visible_count = end_idx - start_idx;

        if visible_count > 0 {
            self.focused_index = (self.focused_index + 1) % visible_count;
        }
    }

    pub fn focus_prev(&mut self) {
        let start_idx = self.page * self.panes_per_page;
        let end_idx = (start_idx + self.panes_per_page).min(self.panes.len());
        let visible_count = end_idx - start_idx;

        if visible_count > 0 {
            self.focused_index = if self.focused_index == 0 { visible_count - 1 } else { self.focused_index - 1 };
        }
    }

    /// The tasks whose panes are still showing Running.
    ///
    /// The panes mirror the scheduler's status broadcasts, so this is what the
    /// user was looking at when they closed the dashboard - and what the
    /// "waiting for N running task(s)" message has to name.
    pub fn running_task_names(&self) -> Vec<String> {
        self.panes
            .iter()
            .filter(|pane| pane.status() == PaneStatus::Running)
            .map(|pane| pane.id().to_string())
            .collect()
    }

    pub fn focused_pane_mut(&mut self) -> Option<&mut Box<dyn Pane>> {
        let start_idx = self.page * self.panes_per_page;
        let absolute_index = start_idx + self.focused_index;
        self.panes.get_mut(absolute_index)
    }
}

impl Default for PaneLayout {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
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
}
