//! UI labels and shortcuts for repository hunk actions.
use crate::git::repository::hunks::Action;

impl Action {
    pub(crate) const ALL: [Self; 5] = [
        Self::Previous,
        Self::Next,
        Self::Stage,
        Self::Unstage,
        Self::Revert,
    ];
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Stage => "Stage",
            Self::Unstage => "Unstage",
            Self::Revert => "Revert",
            Self::Previous => "Previous",
            Self::Next => "Next",
        }
    }
    pub(crate) fn shortcut(self) -> crate::shortcuts::ShortcutAction {
        use crate::shortcuts::ShortcutAction as S;
        match self {
            Self::Stage => S::StageHunk,
            Self::Unstage => S::UnstageHunk,
            Self::Revert => S::RevertHunk,
            Self::Previous => S::PreviousHunk,
            Self::Next => S::NextHunk,
        }
    }
}
