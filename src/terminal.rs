//! Window-owned terminal: libghostty runs on one worker, egui sees immutable grids.
mod emoji;
mod engine;
mod input;
mod session;
mod view;

pub(crate) use view::{TerminalPane, terminal_id};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum PanelTab {
    #[default]
    Problems,
    Terminal,
    Activity,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct BottomPanel {
    tab: PanelTab,
    visible: bool,
    maximized: bool,
}

impl BottomPanel {
    pub(crate) fn selected(self) -> Option<PanelTab> {
        self.visible.then_some(self.tab)
    }

    pub(crate) fn is_visible(self) -> bool {
        self.visible
    }

    pub(crate) fn is_maximized(self) -> bool {
        self.visible && self.maximized
    }

    pub(crate) fn toggle_maximized(&mut self) {
        self.maximized = !self.is_maximized();
        self.visible = true;
    }

    pub(crate) fn select(&mut self, tab: PanelTab) {
        self.tab = tab;
        self.visible = true;
    }

    pub(crate) fn hide(&mut self) {
        self.visible = false;
    }

    pub(crate) fn toggle_visibility(&mut self) {
        self.visible = !self.visible;
    }

    pub(crate) fn toggle(&mut self, tab: PanelTab) {
        if self.selected() == Some(tab) {
            self.hide();
        } else {
            self.select(tab);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maximizing_a_closed_panel_opens_its_tab_and_hide_preserves_maximize_state() {
        let mut panel = BottomPanel::default();
        panel.select(PanelTab::Terminal);
        panel.hide();
        panel.toggle_maximized();
        assert_eq!(panel.selected(), Some(PanelTab::Terminal));
        assert!(panel.is_maximized());
        panel.hide();
        assert!(!panel.is_maximized());
        panel.toggle_visibility();
        assert!(panel.is_maximized());
        panel.select(PanelTab::Problems);
        assert!(panel.is_maximized());
        panel.toggle_maximized();
        assert!(!panel.is_maximized());
        assert_eq!(panel.selected(), Some(PanelTab::Problems));
    }

    #[test]
    fn bottom_panel_switches_without_an_intermediate_hidden_state() {
        let mut panel = BottomPanel::default();
        panel.toggle(PanelTab::Terminal);
        assert_eq!(panel.selected(), Some(PanelTab::Terminal));
        panel.toggle(PanelTab::Problems);
        assert_eq!(panel.selected(), Some(PanelTab::Problems));
        panel.toggle(PanelTab::Problems);
        assert_eq!(panel.selected(), None);
        panel.toggle_visibility();
        assert_eq!(panel.selected(), Some(PanelTab::Problems));
        panel.select(PanelTab::Terminal);
        panel.toggle_visibility();
        panel.toggle_visibility();
        assert_eq!(panel.selected(), Some(PanelTab::Terminal));
    }
}
