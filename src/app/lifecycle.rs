//! Document services have a lifetime distinct from their retained native host.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub(super) enum DocumentLifecycle {
    #[default]
    Active,
    Dormant,
    ResumePending,
}

impl DocumentLifecycle {
    pub(super) fn suspend(&mut self) -> bool {
        if *self == Self::Dormant {
            return false;
        }
        *self = Self::Dormant;
        true
    }

    pub(super) fn request_resume(&mut self) {
        if *self == Self::Dormant {
            *self = Self::ResumePending;
        }
    }

    pub(super) fn needs_visible_window(self) -> bool {
        self == Self::ResumePending
    }

    pub(super) fn allows_document_work(self) -> bool {
        self == Self::Active
    }

    /// Called after queued replacements/preferences, only in a visible owner.
    pub(super) fn activate(&mut self) -> bool {
        if *self != Self::ResumePending {
            return false;
        }
        *self = Self::Active;
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn repeated_close_reopen_and_settings_changes_cannot_duplicate_service_starts() {
        let mut state = DocumentLifecycle::default();
        assert!(!state.activate());
        for _ in 0..3 {
            assert!(state.suspend());
            assert!(!state.suspend());
            for _ in 0..100 {
                assert!(!state.allows_document_work());
                assert!(
                    !state.activate(),
                    "host/settings frames do not reopen documents"
                );
            }
            state.request_resume();
            state.request_resume();
            assert!(state.needs_visible_window());
            assert!(
                !state.allows_document_work(),
                "queued changes must settle before starting services"
            );
            assert!(state.activate());
            assert!(!state.activate());
            assert!(state.allows_document_work());
        }
    }

    #[test]
    fn close_before_pending_resume_cancels_the_start() {
        let mut state = DocumentLifecycle::Dormant;
        state.request_resume();
        assert!(state.suspend());
        assert!(!state.activate());
        assert!(!state.needs_visible_window());
    }
}
