//! Window-owned terminal: libghostty runs on one worker, egui sees immutable grids.
mod engine;
mod input;
mod session;
mod view;

pub(crate) use view::{TerminalPane, terminal_id};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum BottomPanel {
    #[default]
    Hidden,
    Problems,
    Terminal,
}

impl BottomPanel {
    pub(crate) fn toggle(&mut self, panel: Self) {
        *self = if *self == panel { Self::Hidden } else { panel };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bottom_panel_switches_without_an_intermediate_hidden_state() {
        let mut panel = BottomPanel::default();
        panel.toggle(BottomPanel::Terminal);
        assert_eq!(panel, BottomPanel::Terminal);
        panel.toggle(BottomPanel::Problems);
        assert_eq!(panel, BottomPanel::Problems);
        panel.toggle(BottomPanel::Problems);
        assert_eq!(panel, BottomPanel::Hidden);
    }
}
