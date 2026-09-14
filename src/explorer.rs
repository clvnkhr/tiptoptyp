//! Stable Explorer identities and a validated, user-configurable display order.
use serde::{Deserialize, Serialize};

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
