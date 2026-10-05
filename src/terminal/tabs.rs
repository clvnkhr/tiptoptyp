//! Multiple terminal panes in the bottom panel; exactly one is shown at a time.
//! Closing the active pane removes it. With no panes left the terminal area
//! is cleared and reports itself off until another pane opens.
use std::path::Path;

use eframe::egui;

use super::view::TerminalPane;

/// Bound the number of live panes; each hidden pane can still own a shell.
pub(crate) const MAX_TERMINAL_PANES: usize = 8;

pub(crate) struct TerminalTabs {
    panes: Vec<TerminalPane>,
    active: usize,
    visible: bool,
}

impl Default for TerminalTabs {
    fn default() -> Self {
        Self {
            panes: vec![TerminalPane::default()],
            active: 0,
            visible: true,
        }
    }
}

impl TerminalTabs {
    fn pane(&self) -> Option<&TerminalPane> {
        self.panes.get(self.active_index())
    }

    fn pane_mut(&mut self) -> Option<&mut TerminalPane> {
        let index = self.active_index();
        self.panes.get_mut(index)
    }

    pub(crate) fn len(&self) -> usize {
        self.panes.len()
    }

    pub(crate) fn active_index(&self) -> usize {
        self.active.min(self.panes.len().saturating_sub(1))
    }

    /// Appends one pane and shows it. A full set keeps the current pane.
    pub(crate) fn add(&mut self) {
        if self.panes.len() >= MAX_TERMINAL_PANES {
            return;
        }
        self.panes.push(TerminalPane::default());
        self.select(self.panes.len() - 1);
    }

    /// Shows another pane, parking the previous one.
    pub(crate) fn select(&mut self, index: usize) {
        if self.panes.is_empty() {
            return;
        }
        let index = index.min(self.panes.len() - 1);
        if index != self.active {
            let previous = self.active.min(self.panes.len() - 1);
            self.panes[previous].set_visible(false);
            self.active = index;
        }
        let visible = self.visible;
        if let Some(pane) = self.pane_mut() {
            pane.set_visible(visible);
            pane.request_focus();
        }
    }

    /// Removes the shown pane, reaping its shell. The next pane takes its
    /// place; an emptied terminal reports itself off until another opens.
    pub(crate) fn close_active(&mut self) {
        if self.panes.is_empty() {
            return;
        }
        let index = self.active_index();
        self.panes.remove(index);
        self.active = index.min(self.panes.len().saturating_sub(1));
        let visible = self.visible;
        if let Some(pane) = self.pane_mut() {
            pane.set_visible(visible);
            pane.request_focus();
        }
    }

    pub(crate) fn request_focus(&mut self) {
        if let Some(pane) = self.pane_mut() {
            pane.request_focus();
        }
    }

    /// Restarts the shown shell, opening a first pane from a cleared window.
    pub(crate) fn restart(&mut self) {
        if self.panes.is_empty() {
            self.panes.push(TerminalPane::default());
            self.active = 0;
            return;
        }
        if let Some(pane) = self.pane_mut() {
            pane.restart();
        }
    }

    pub(crate) fn is_off(&self) -> bool {
        // Closing the last pane clears the window; emptiness is the off state.
        self.panes.is_empty()
    }

    pub(crate) fn set_visible(&mut self, visible: bool) {
        self.visible = visible;
        let active = self.active_index();
        for (index, pane) in self.panes.iter_mut().enumerate() {
            pane.set_visible(visible && index == active);
        }
    }

    pub(crate) fn prepare_fixture(&mut self, output: &[u8], cwd: &Path) {
        if let Some(pane) = self.pane_mut() {
            pane.prepare_fixture(output, cwd);
        }
    }

    pub(crate) fn copy(&self, context: &egui::Context) {
        if let Some(pane) = self.pane() {
            pane.copy(context);
        }
    }

    pub(crate) fn select_all(&mut self) {
        if let Some(pane) = self.pane_mut() {
            pane.select_all();
        }
    }

    pub(crate) fn sync_snapshot(&mut self) {
        if let Some(pane) = self.pane_mut() {
            pane.sync_snapshot();
        }
    }

    pub(crate) fn activity(&mut self) -> crate::activity::Activity {
        self.pane_mut().map_or(
            crate::activity::Activity::Inactive("Turned off"),
            TerminalPane::activity,
        )
    }

    pub(crate) fn status_text(&self) -> Option<&str> {
        self.pane().map_or(Some("Off"), TerminalPane::status_text)
    }

    pub(crate) fn starting_directory<'a>(&'a self, workspace: &'a Path) -> &'a Path {
        self.pane()
            .map_or(workspace, |pane| pane.starting_directory(workspace))
    }

    pub(crate) fn show(&mut self, ui: &mut egui::Ui, cwd: &Path) {
        let Some(pane) = self.pane_mut() else {
            // A cleared window keeps the panel background and nothing else.
            let colors = super::view::terminal_panel_colors(ui);
            let (_, outer) = ui.allocate_space(ui.available_size().max(egui::Vec2::ZERO));
            ui.painter().rect_filled(outer, 0.0, colors.background);
            return;
        };
        pane.show(ui, cwd);
    }

    #[cfg(test)]
    pub(crate) fn replace_with_fixture_panes(&mut self, outputs: &[&[u8]], cwd: &Path) {
        self.panes = outputs
            .iter()
            .map(|output| {
                let mut pane = TerminalPane::default();
                pane.prepare_fixture(output, cwd);
                pane
            })
            .collect();
        self.active = 0;
    }

    #[cfg(test)]
    pub(crate) fn pane_focus_requested(&self, index: usize) -> bool {
        self.panes
            .get(index)
            .is_some_and(TerminalPane::focus_requested_for_test)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn adding_terminals_appends_and_switches_up_to_the_bound() {
        let mut tabs = TerminalTabs::default();
        assert_eq!(tabs.len(), 1);
        assert_eq!(tabs.active_index(), 0);
        for expected in 1..MAX_TERMINAL_PANES {
            tabs.add();
            assert_eq!(tabs.len(), expected + 1);
            assert_eq!(tabs.active_index(), expected);
            assert!(tabs.pane_focus_requested(expected));
        }
        tabs.add();
        assert_eq!(tabs.len(), MAX_TERMINAL_PANES);
        assert_eq!(tabs.active_index(), MAX_TERMINAL_PANES - 1);
    }

    #[test]
    fn selecting_parks_the_previous_pane_and_clamps_its_index() {
        let mut tabs = TerminalTabs::default();
        tabs.replace_with_fixture_panes(&[b"one", b"two", b"three"], Path::new("/project"));
        assert_eq!(
            tabs.starting_directory(Path::new("/other")),
            Path::new("/project")
        );
        tabs.select(2);
        assert_eq!(tabs.active_index(), 2);
        assert!(tabs.pane_focus_requested(2));
        assert!(!tabs.pane_focus_requested(0));
        tabs.select(99);
        assert_eq!(tabs.active_index(), 2);
        tabs.select(0);
        assert_eq!(tabs.active_index(), 0);
        // Re-selecting the shown pane is harmless and still focuses it.
        assert!(tabs.pane_focus_requested(0));
    }

    #[test]
    fn closing_removes_the_shown_pane_and_empties_to_a_cleared_window() {
        let mut tabs = TerminalTabs::default();
        tabs.replace_with_fixture_panes(&[b"one", b"two", b"three"], Path::new("/project"));
        tabs.select(1);
        tabs.close_active();
        assert_eq!(tabs.len(), 2);
        assert_eq!(tabs.active_index(), 1);
        tabs.close_active();
        assert_eq!(tabs.len(), 1);
        assert_eq!(tabs.active_index(), 0);
        assert!(!tabs.is_off());
        tabs.close_active();
        assert!(tabs.is_off());
        assert_eq!(tabs.status_text(), Some("Off"));
        assert!(matches!(
            tabs.activity(),
            crate::activity::Activity::Inactive("Turned off")
        ));
        assert_eq!(
            tabs.starting_directory(Path::new("/other")),
            Path::new("/other")
        );
        // Closing, selecting, and restarting an empty set stay harmless, and
        // restart on an empty set opens a first pane again.
        tabs.close_active();
        tabs.select(0);
        tabs.restart();
        assert_eq!(tabs.len(), 1);
        assert!(!tabs.is_off());
    }
}
