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

pub struct Workflow<A, M, D> {
    phase: Phase<A, M, D>,
    after_save: Option<A>,
}
impl<A, M, D> Default for Workflow<A, M, D> {
    fn default() -> Self {
        Self {
            phase: Phase::Idle,
            after_save: None,
        }
    }
}
impl<A, M, D> Workflow<A, M, D> {
    /// Consume the receipt before releasing a continuation. Newer edits and
    /// uncertain durability keep the window open even though bytes were saved.
    pub fn complete_save<C>(
        &mut self,
        document: &mut crate::document::DocumentSession<C>,
        receipt: crate::document::SaveReceipt,
        synchronized: bool,
    ) -> Result<Option<A>, &'static str> {
        let continuation = self.after_save.take();
        if matches!(
            document.record_save(receipt)?,
            crate::document::SaveStatus::Stale
        ) {
            // Keep a close/open continuation alive until the authoritative
            // save completion arrives. An older completion proves only that
            // older bytes reached the destination.
            self.after_save = continuation;
            return Ok(None);
        }
        Ok(
            if synchronized && document.source() == document.saved_source() {
                continuation
            } else {
                None
            },
        )
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
        self.after_save = Some(action);
        Ok(())
    }
    pub fn take_continuation(&mut self) -> Option<A> {
        self.after_save.take()
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
            flow.complete_save(&mut document, older.committed(1), true)
                .unwrap(),
            None
        );
        assert!(flow.has_continuation());
        let authoritative = document.prepare_save("draft.txt".into(), DocumentKind::Text);
        assert_eq!(
            flow.complete_save(&mut document, authoritative.committed(2), true)
                .unwrap(),
            Some("close")
        );
        assert!(!flow.has_continuation());
    }
}
