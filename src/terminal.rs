//! Window-owned terminal: libghostty runs on one worker, egui sees immutable grids.
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
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct BottomPanel {
    tab: PanelTab,
    visible: bool,
}

impl BottomPanel {
    pub(crate) fn selected(self) -> Option<PanelTab> {
        self.visible.then_some(self.tab)
    }

    pub(crate) fn is_visible(self) -> bool {
        self.visible
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
