//! Stable Explorer identities and a validated, user-configurable display order.
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ExplorerSection {
    Files,
    Git,
    Contents,
    Subfiles,
    Symbols,
    Packages,
    Tags,
    References,
}

impl ExplorerSection {
    pub(crate) const ALL: [Self; 8] = [
        Self::Files,
        Self::Git,
        Self::Contents,
        Self::Subfiles,
        Self::Symbols,
        Self::Packages,
        Self::Tags,
        Self::References,
    ];

    pub(crate) const fn index(self) -> usize {
        self as usize
    }

    pub(crate) const fn title(self) -> &'static str {
        match self {
            Self::Files => "Files",
            Self::Git => "Git",
            Self::Contents => "Contents",
            Self::Subfiles => "Subfiles",
            Self::Symbols => "Symbols",
            Self::Packages => "Packages",
            Self::Tags => "Tags",
            Self::References => "References",
        }
    }

    pub(crate) const fn id(self) -> &'static str {
        match self {
            Self::Files => "workspace-files",
            Self::Git => "workspace-git",
            Self::Contents => "workspace-contents",
            Self::Subfiles => "workspace-subfiles",
            Self::Symbols => "workspace-symbols",
            Self::Packages => "workspace-packages",
            Self::Tags => "workspace-tags",
            Self::References => "workspace-references",
        }
    }

    pub(crate) const fn default_open(self) -> bool {
        matches!(self, Self::Files | Self::Contents)
    }
}

/// Every section occurs exactly once. Neither settings nor a reorder operation
/// can create duplicate panels or omit one; UI state remains keyed by identity.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(try_from = "[ExplorerSection; 8]", into = "[ExplorerSection; 8]")]
pub(crate) struct ExplorerOrder([ExplorerSection; 8]);

impl Default for ExplorerOrder {
    fn default() -> Self {
        Self(ExplorerSection::ALL)
    }
}

impl TryFrom<[ExplorerSection; 8]> for ExplorerOrder {
    type Error = &'static str;

    fn try_from(sections: [ExplorerSection; 8]) -> Result<Self, Self::Error> {
        let mut seen = [false; 8];
        for section in sections {
            if std::mem::replace(&mut seen[section.index()], true) {
                return Err("Explorer order must contain each panel exactly once");
            }
        }
        Ok(Self(sections))
    }
}

impl From<ExplorerOrder> for [ExplorerSection; 8] {
    fn from(order: ExplorerOrder) -> Self {
        order.0
    }
}

impl ExplorerOrder {
    pub(crate) fn sections(self) -> [ExplorerSection; 8] {
        self.0
    }

    pub(crate) fn move_to(&mut self, section: ExplorerSection, destination: usize) -> bool {
        let source = self.0.iter().position(|entry| *entry == section).unwrap();
        if destination >= self.0.len() || destination == source {
            return false;
        }
        if source < destination {
            self.0[source..=destination].rotate_left(1);
        } else {
            self.0[destination..=source].rotate_right(1);
        }
        true
    }

    pub(crate) fn next_open(
        self,
        open: [bool; 8],
        after: ExplorerSection,
    ) -> Option<ExplorerSection> {
        self.0
            .into_iter()
            .skip_while(|section| *section != after)
            .skip(1)
            .find(|section| open[section.index()])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_move_preserves_each_panel_once_and_roundtrips() {
        for section in ExplorerSection::ALL {
            for destination in 0..8 {
                let mut order = ExplorerOrder::default();
                order.move_to(section, destination);
                assert_eq!(order.sections()[destination], section);
                assert_eq!(ExplorerOrder::try_from(order.sections()), Ok(order));
                let encoded = serde_json::to_string(&order).unwrap();
                assert_eq!(
                    serde_json::from_str::<ExplorerOrder>(&encoded).unwrap(),
                    order
                );
            }
        }
        let mut order = ExplorerOrder::default();
        assert!(!order.move_to(ExplorerSection::Files, usize::MAX));
        assert_eq!(order.sections()[1], ExplorerSection::Git);
    }

    #[test]
    fn duplicate_or_missing_panels_are_rejected() {
        let mut sections = ExplorerSection::ALL;
        sections[7] = ExplorerSection::Files;
        assert!(ExplorerOrder::try_from(sections).is_err());
        let encoded = serde_json::to_string(&sections).unwrap();
        assert!(serde_json::from_str::<ExplorerOrder>(&encoded).is_err());
        assert!(serde_json::from_str::<ExplorerOrder>("[\"files\"]").is_err());
    }

    #[test]
    fn resize_neighbor_follows_display_order_and_skips_closed_panels() {
        let mut order = ExplorerOrder::default();
        order.move_to(ExplorerSection::References, 0);
        order.move_to(ExplorerSection::Tags, 2);
        let mut open = [false; 8];
        for section in [
            ExplorerSection::References,
            ExplorerSection::Tags,
            ExplorerSection::Git,
        ] {
            open[section.index()] = true;
        }
        assert_eq!(
            order.next_open(open, ExplorerSection::References),
            Some(ExplorerSection::Tags)
        );
        assert_eq!(
            order.next_open(open, ExplorerSection::Tags),
            Some(ExplorerSection::Git)
        );
        assert_eq!(order.next_open(open, ExplorerSection::Git), None);
    }
}
/// Visibility and size restoration belong to the panel, not keyboard/menu adapters.
///
/// This is also the smallest useful width to persist. The live panel can be
/// temporarily constrained by a narrow viewport, but retaining that transient
/// constraint would make a later normal-sized window reopen with a nearly
/// invisible Explorer.
pub(crate) const EXPLORER_MIN_WIDTH: f32 = 160.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ExplorerPanelPhase {
    Open,
    HideContents,
    Closed,
}

impl ExplorerPanelPhase {
    pub(crate) fn toggle(self) -> Self {
        match self {
            Self::Open => Self::HideContents,
            Self::HideContents | Self::Closed => Self::Open,
        }
    }
    pub(crate) fn panel_visible(self) -> bool {
        self != Self::Closed
    }
    pub(crate) fn contents_visible(self) -> bool {
        self == Self::Open
    }
    pub(crate) fn finish_frame(self) -> Self {
        if self == Self::HideContents {
            Self::Closed
        } else {
            self
        }
    }
}

pub(crate) struct ExplorerPanelState {
    phase: ExplorerPanelPhase,
    width: Option<f32>,
    restore_pending: bool,
    startup_pending: bool,
    query: String,
    focus_search: bool,
    reveal_git: bool,
    selected_path: Option<PathBuf>,
}
impl Default for ExplorerPanelState {
    fn default() -> Self {
        Self {
            phase: ExplorerPanelPhase::Open,
            width: None,
            restore_pending: false,
            startup_pending: true,
            query: String::new(),
            focus_search: false,
            reveal_git: true,
            selected_path: None,
        }
    }
}
impl ExplorerPanelState {
    pub(crate) fn focus_search(&mut self) {
        self.focus_search = true;
    }
    pub(crate) fn take_search_focus(&mut self) -> bool {
        std::mem::take(&mut self.focus_search)
    }
    pub(crate) fn set_git_reveal(&mut self, reveal: bool) {
        self.reveal_git = reveal;
    }
    pub(crate) fn take_git_reveal(&mut self) -> bool {
        std::mem::take(&mut self.reveal_git)
    }
    pub(crate) fn select_path(&mut self, path: PathBuf) {
        self.selected_path = Some(path);
    }
    pub(crate) fn selected_path(&self) -> Option<&Path> {
        self.selected_path.as_deref()
    }
    pub(crate) fn clear_selection(&mut self) {
        self.selected_path = None;
    }
    pub(crate) fn startup_width(&mut self, persisted: f32, default: f32) -> Option<f32> {
        if std::mem::take(&mut self.startup_pending)
            && (!persisted.is_finite() || persisted < EXPLORER_MIN_WIDTH)
        {
            Some(default)
        } else {
            None
        }
    }
    pub(crate) fn open(&mut self) {
        self.restore_pending |= !self.phase.panel_visible();
        self.phase = ExplorerPanelPhase::Open;
    }
    pub(crate) fn hide(&mut self) {
        self.phase = ExplorerPanelPhase::Closed;
    }
    pub(crate) fn toggle(&mut self) {
        self.restore_pending |= !self.phase.panel_visible();
        self.phase = self.phase.toggle();
    }
    pub(crate) fn panel_visible(&self) -> bool {
        self.phase.panel_visible()
    }
    pub(crate) fn contents_visible(&self) -> bool {
        self.phase.contents_visible()
    }
    pub(crate) fn query(&self) -> &str {
        &self.query
    }
    pub(crate) fn query_mut(&mut self) -> &mut String {
        &mut self.query
    }
    pub(crate) fn remember_width(&mut self, width: f32) {
        if self.contents_visible() && width.is_finite() && width >= EXPLORER_MIN_WIDTH {
            self.width = Some(width);
        }
    }
    pub(crate) fn take_restored_width(&mut self) -> Option<f32> {
        std::mem::take(&mut self.restore_pending)
            .then_some(self.width)
            .flatten()
    }
    pub(crate) fn finish_frame(&mut self) -> bool {
        let next = self.phase.finish_frame();
        let changed = next != self.phase;
        self.phase = next;
        changed
    }
}

#[cfg(test)]
mod panel_tests {
    use super::*;

    #[test]
    fn presentation_requests_are_owner_local_and_consumed_once() {
        let mut first = ExplorerPanelState::default();
        let mut second = ExplorerPanelState::default();
        first.focus_search();
        assert!(first.take_search_focus());
        assert!(!first.take_search_focus());
        assert!(!second.take_search_focus());
        assert!(first.take_git_reveal());
        assert!(!first.take_git_reveal());
        assert!(second.take_git_reveal());
        first.set_git_reveal(true);
        first.set_git_reveal(false);
        assert!(!first.take_git_reveal());
    }

    #[test]
    fn startup_repairs_tiny_persisted_sizes_without_overwriting_valid_sizes() {
        for width in [12.0, 96.0, f32::NAN] {
            let mut state = ExplorerPanelState::default();
            assert_eq!(state.startup_width(width, 230.0), Some(230.0));
            assert_eq!(state.startup_width(100.0, 230.0), None);
        }
        let mut state = ExplorerPanelState::default();
        assert_eq!(state.startup_width(310.0, 230.0), None);
    }
    #[test]
    fn closing_frames_cannot_replace_saved_width_and_every_open_path_restores_it() {
        let mut state = ExplorerPanelState::default();
        state.remember_width(260.0);
        state.toggle();
        state.remember_width(12.0);
        assert!(state.panel_visible());
        assert!(!state.contents_visible());
        assert!(state.finish_frame());
        assert!(!state.panel_visible());
        state.toggle();
        assert_eq!(state.take_restored_width(), Some(260.0));
        assert_eq!(state.take_restored_width(), None);
        state.remember_width(330.0);
        state.toggle();
        state.finish_frame();
        state.open();
        assert_eq!(state.take_restored_width(), Some(330.0));
        state.remember_width(EXPLORER_MIN_WIDTH - 1.0);
        state.hide();
        state.open();
        assert_eq!(state.take_restored_width(), Some(330.0));
        state.remember_width(f32::NAN);
        state.hide();
        state.open();
        assert_eq!(state.take_restored_width(), Some(330.0));
    }

    #[test]
    fn imported_file_selection_is_owner_local_and_can_be_replaced() {
        let mut state = ExplorerPanelState::default();
        let first = PathBuf::from("/workspace/first.typ");
        let second = PathBuf::from("/workspace/second.typ");
        state.select_path(first.clone());
        assert_eq!(state.selected_path(), Some(first.as_path()));
        state.select_path(second.clone());
        assert_eq!(state.selected_path(), Some(second.as_path()));
        state.clear_selection();
        assert_eq!(state.selected_path(), None);
    }
}
