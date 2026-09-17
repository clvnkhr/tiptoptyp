//! Exclusive operation phases, independent of dialog and platform types.
#[derive(Default)]
enum Phase<A, M, D> {
    #[default]
    Idle,
    Queued(A),
    Executing,
    Prompt(M),
    Dialog(D),
}

/// Identifies one save-dependent action within its owning workflow.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SaveContinuationToken(u64);

pub struct Workflow<A, M, D> {
    phase: Phase<A, M, D>,
    after_save: Option<(SaveContinuationToken, A)>,
    next_continuation: u64,
}
impl<A, M, D> Default for Workflow<A, M, D> {
    fn default() -> Self {
        Self {
            phase: Phase::Idle,
            after_save: None,
            next_continuation: 0,
        }
    }
}
impl<A, M, D> Workflow<A, M, D> {
    /// Observe the recorded save before releasing a continuation. Newer edits and
    /// uncertain durability keep the window open even though bytes were saved.
    pub fn complete_save(
        &mut self,
        token: Option<SaveContinuationToken>,
        status: Result<crate::document::SaveStatus, &'static str>,
        dirty: bool,
        synchronized: bool,
    ) -> Result<Option<A>, &'static str> {
        if token != self.continuation_token() {
            // A previous save may update persisted state, but must not release
            // or cancel an action installed after it was submitted.
            status?;
            return Ok(None);
        }
        let continuation = self.after_save.take();
        if matches!(status?, crate::document::SaveStatus::Stale) {
            // Keep a close/open continuation alive until the authoritative
            // save completion arrives. An older completion proves only that
            // older bytes reached the destination.
            self.after_save = continuation;
            return Ok(None);
        }
        Ok(if synchronized && !dirty {
            continuation.map(|(_, action)| action)
        } else {
            None
        })
    }
    pub fn is_busy(&self) -> bool {
        !matches!(self.phase, Phase::Idle) || self.after_save.is_some()
    }
    pub fn prompt(&self) -> Option<&M> {
        if let Phase::Prompt(value) = &self.phase {
            Some(value)
        } else {
            None
        }
    }
    pub fn has_dialog(&self) -> bool {
        matches!(self.phase, Phase::Dialog(_))
    }
    pub fn has_continuation(&self) -> bool {
        self.after_save.is_some()
    }
    pub fn continuation_token(&self) -> Option<SaveContinuationToken> {
        self.after_save.as_ref().map(|(token, _)| *token)
    }
    pub fn queue(&mut self, action: A) -> Result<(), A> {
        if !matches!(self.phase, Phase::Idle | Phase::Executing) {
            return Err(action);
        }
        self.phase = Phase::Queued(action);
        Ok(())
    }
    pub fn take_action(&mut self) -> Option<A> {
        if !matches!(self.phase, Phase::Queued(_)) {
            return None;
        }
        let Phase::Queued(action) = std::mem::replace(&mut self.phase, Phase::Executing) else {
            unreachable!()
        };
        Some(action)
    }
    pub fn show_prompt(&mut self, modal: M) -> Result<(), M> {
        if !matches!(
            self.phase,
            Phase::Idle | Phase::Executing | Phase::Prompt(_)
        ) {
            return Err(modal);
        }
        self.phase = Phase::Prompt(modal);
        Ok(())
    }
    pub fn clear_prompt(&mut self) {
        if matches!(self.phase, Phase::Prompt(_)) {
            self.phase = Phase::Executing;
        }
    }
    pub fn take_prompt(&mut self) -> Option<M> {
        if !matches!(self.phase, Phase::Prompt(_)) {
            return None;
        }
        let Phase::Prompt(prompt) = std::mem::replace(&mut self.phase, Phase::Executing) else {
            unreachable!()
        };
        Some(prompt)
    }
    pub fn choose(&mut self, dialog: D) -> Result<(), D> {
        if !matches!(self.phase, Phase::Idle | Phase::Executing) {
            return Err(dialog);
        }
        self.phase = Phase::Dialog(dialog);
        Ok(())
    }
    pub fn take_dialog(&mut self) -> Option<D> {
        if !self.has_dialog() {
            return None;
        }
        let Phase::Dialog(dialog) = std::mem::replace(&mut self.phase, Phase::Executing) else {
            unreachable!()
        };
        Some(dialog)
    }
    pub fn continue_after_save(&mut self, action: A) -> Result<(), A> {
        if self.after_save.is_some() {
            return Err(action);
        }
        let Some(next) = self.next_continuation.checked_add(1) else {
            return Err(action);
        };
        self.next_continuation = next;
        self.after_save = Some((SaveContinuationToken(next), action));
        Ok(())
    }
    pub fn take_continuation(&mut self) -> Option<A> {
        self.after_save.take().map(|(_, action)| action)
    }
    pub fn cancel_continuation(&mut self) {
        self.after_save = None;
    }
    pub fn finish_dispatch(&mut self) {
        if matches!(self.phase, Phase::Executing) {
            self.phase = Phase::Idle;
            // No write, dialog or prompt was scheduled. A failed attempt to
            // launch a chooser cannot leave a close continuation hanging.
            self.after_save = None;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_old_or_unrelated_save_cannot_release_or_cancel_a_new_close() {
        use crate::document::SaveStatus;
        let mut flow = Workflow::<&str, (), ()>::default();
        flow.continue_after_save("first close").unwrap();
        let old = flow.continuation_token();
        flow.cancel_continuation();
        flow.continue_after_save("second close").unwrap();
        let current = flow.continuation_token();
        assert_ne!(old, current);
        assert!(flow.continue_after_save("duplicate").is_err());
        assert_eq!(flow.continuation_token(), current);
        for token in [None, old] {
            for (status, synchronized) in [
                (Ok(SaveStatus::Applied), true),
                (Ok(SaveStatus::Applied), false),
                (Ok(SaveStatus::Stale), true),
                (Err("wrong document"), true),
            ] {
                let result = flow.complete_save(token, status, false, synchronized);
                if status.is_err() {
                    assert!(result.is_err());
                } else {
                    assert_eq!(result.unwrap(), None);
                }
                assert_eq!(flow.continuation_token(), current);
            }
        }
        assert_eq!(
            flow.complete_save(current, Ok(SaveStatus::Applied), false, true)
                .unwrap(),
            Some("second close")
        );
        assert_eq!(
            flow.complete_save(current, Ok(SaveStatus::Applied), false, true)
                .unwrap(),
            None
        );
    }

    #[test]
    fn exhausted_continuation_tokens_are_not_reused() {
        let mut flow = Workflow::<&str, (), ()> {
            next_continuation: u64::MAX,
            ..Default::default()
        };
        assert!(flow.continue_after_save("close").is_err());
        assert_eq!(flow.continuation_token(), None);
    }
    #[test]
    fn save_before_close_has_exclusive_phases_and_one_continuation() {
        let mut flow = Workflow::<&str, &str, &str>::default();
        flow.queue("close").unwrap();
        assert!(flow.choose("unrelated dialog").is_err());
        assert_eq!(flow.take_action(), Some("close"));
        flow.show_prompt("save changes?").unwrap();
        assert!(flow.queue("second close").is_err());
        flow.clear_prompt();
        flow.continue_after_save("close").unwrap();
        flow.choose("save destination").unwrap();
        assert!(flow.show_prompt("unrelated prompt").is_err());
        assert_eq!(flow.take_dialog(), Some("save destination"));
        assert_eq!(flow.take_dialog(), None);
        let next = flow.take_continuation().unwrap();
        flow.queue(next).unwrap();
        assert_eq!(flow.take_continuation(), None);
        assert_eq!(flow.take_action(), Some("close"));
        assert_eq!(flow.take_action(), None);
        flow.finish_dispatch();
        assert!(!flow.is_busy());
    }
    #[test]
    fn canceled_save_cannot_release_close() {
        let mut flow = Workflow::<&str, &str, &str>::default();
        flow.continue_after_save("close").unwrap();
        flow.choose("save").unwrap();
        flow.take_dialog();
        flow.cancel_continuation();
        flow.finish_dispatch();
        assert_eq!(flow.take_action(), None);
        assert_eq!(flow.take_continuation(), None);
        assert!(!flow.is_busy());
    }

    #[test]
    fn stale_save_completion_keeps_close_continuation_until_newer_save() {
        use crate::document::{DocumentKind, DocumentSession, WindowSessionId};

        let mut document =
            DocumentSession::<usize>::new(WindowSessionId::new(1), "saved", DocumentKind::Text);
        document.replace_loaded(
            "saved".to_owned(),
            "draft.txt".into(),
            DocumentKind::Text,
            Some(1),
        );
        let older = document.prepare_save("draft.txt".into(), DocumentKind::Text);
        document.edit(0, |source| source.push_str(" newer"));
        let newer = document.prepare_save("draft.txt".into(), DocumentKind::Text);
        document.record_save(newer.committed(2)).unwrap();

        let mut flow = Workflow::<&str, &str, &str>::default();
        flow.continue_after_save("close").unwrap();
        assert_eq!(
            flow.complete_save(
                flow.continuation_token(),
                document.record_save(older.committed(1)),
                document.is_dirty(),
                true
            )
            .unwrap(),
            None
        );
        assert!(flow.has_continuation());
        let authoritative = document.prepare_save("draft.txt".into(), DocumentKind::Text);
        assert_eq!(
            flow.complete_save(
                flow.continuation_token(),
                document.record_save(authoritative.committed(2)),
                document.is_dirty(),
                true
            )
            .unwrap(),
            Some("close")
        );
        assert!(!flow.has_continuation());
    }
}
