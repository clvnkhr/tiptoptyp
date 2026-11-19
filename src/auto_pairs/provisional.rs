//! A typed prefix temporarily borrows a real closer until typing resolves it.
use eframe::egui;
use std::{collections::VecDeque, ops::Range};
use tiptoptyp_core::document::DocumentKey;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Owner {
    pub(crate) document: DocumentKey,
    pub(crate) tab: u64,
    pub(crate) editor: egui::Id,
}

#[derive(Default)]
pub(crate) enum State {
    #[default]
    Idle,
    Borrowing(CloserPrefix),
}

pub(crate) struct CloserPrefix {
    owner: Owner,
    span: Range<usize>,
    token: String,
    start_scalar: usize,
    prefix_bytes: usize,
    prefix_scalars: usize,
    painted_frame: u64,
}

pub(super) enum TextAction {
    Ordinary,
    Advance(usize),
    Complete { advance: usize, tail: usize },
    Restore { insertion: String, advance: usize },
    AtLimit,
}

impl CloserPrefix {
    fn admits(&self, owner: Owner, source: &str, caret: usize) -> bool {
        self.owner == owner
            && caret == self.start_scalar + self.prefix_scalars
            && source.get(self.span.clone()) == Some(self.token.as_str())
    }
}

impl State {
    pub(crate) fn cancel(&mut self) {
        *self = Self::Idle;
    }

    /// A source surface must be observed on consecutive owning viewport frames.
    /// Missing paint retires the prefix without polling or repainting.
    pub(crate) fn begin_frame(&mut self, frame: u64) {
        if matches!(self, Self::Borrowing(prefix) if prefix.painted_frame != frame && prefix.painted_frame.checked_add(1) != Some(frame))
        {
            self.cancel();
        }
    }

    pub(crate) fn end_frame(&mut self, frame: u64) {
        if matches!(self, Self::Borrowing(prefix) if prefix.painted_frame != frame) {
            self.cancel();
        }
    }

    #[cfg(any(test, feature = "desktop-ui-tests"))]
    pub(crate) fn phase(&self) -> &'static str {
        match self {
            Self::Idle => "idle",
            Self::Borrowing(_) => "borrowing",
        }
    }

    pub(crate) fn reconcile(
        &mut self,
        owner: Owner,
        source: &str,
        caret: Option<usize>,
        allowed: bool,
    ) {
        if !allowed
            || matches!(self, Self::Borrowing(prefix) if !caret.is_some_and(|caret| prefix.admits(owner, source, caret)))
        {
            self.cancel();
        }
    }

    /// Admit only this editor transaction's new revision. The borrowed token
    /// and final caret must still agree after TextEdit and linked edits finish.
    pub(crate) fn finish(
        &mut self,
        before: Owner,
        after: DocumentKey,
        source: &str,
        caret: Option<usize>,
        allowed: bool,
        frame: u64,
    ) {
        if before.document.owner != after.owner || before.document.epoch != after.epoch {
            self.cancel();
            return;
        }
        self.reconcile(before, source, caret, allowed);
        if let Self::Borrowing(prefix) = self {
            prefix.owner.document = after;
            prefix.painted_frame = frame;
        }
    }

    pub(super) fn begin(
        &mut self,
        owner: Owner,
        source: &str,
        span: Range<usize>,
        start_scalar: usize,
    ) {
        let Some(token) = source
            .get(span.clone())
            .filter(|token| token.starts_with('\\') && token.len() > 1)
        else {
            self.cancel();
            return;
        };
        *self = Self::Borrowing(CloserPrefix {
            owner,
            span,
            token: token.into(),
            start_scalar,
            prefix_bytes: 1,
            prefix_scalars: 1,
            painted_frame: u64::MAX,
        });
    }

    pub(super) fn continue_text(
        &mut self,
        owner: Owner,
        source: &str,
        caret: usize,
        text: &str,
        limit: usize,
    ) -> TextAction {
        let Self::Borrowing(prefix) = self else {
            return TextAction::Ordinary;
        };
        if !prefix.admits(owner, source, caret) || text.is_empty() {
            self.cancel();
            return TextAction::Ordinary;
        }
        let advance = text.chars().count();
        if prefix.token[prefix.prefix_bytes..].starts_with(text) {
            prefix.prefix_bytes += text.len();
            prefix.prefix_scalars += advance;
            if prefix.prefix_bytes == prefix.token.len() {
                self.cancel();
            }
            return TextAction::Advance(advance);
        }
        let remaining = &prefix.token[prefix.prefix_bytes..];
        if text.starts_with(remaining) {
            let completed = TextAction::Complete {
                advance: remaining.chars().count(),
                tail: remaining.len(),
            };
            self.cancel();
            return completed;
        }
        if advance.saturating_add(prefix.prefix_scalars)
            > limit.saturating_sub(source.chars().count())
        {
            return TextAction::AtLimit;
        }
        let insertion = format!("{text}{}", &prefix.token[..prefix.prefix_bytes]);
        self.cancel();
        TextAction::Restore { insertion, advance }
    }

    /// Backspace reverses the virtual overstep rather than deleting the
    /// document's closing command. Repeated backspaces retire its prefix.
    pub(super) fn rewind(&mut self, owner: Owner, source: &str, caret: usize) -> Option<usize> {
        let Self::Borrowing(prefix) = self else {
            return None;
        };
        if !prefix.admits(owner, source, caret) {
            self.cancel();
            return None;
        }
        let previous = prefix.token[..prefix.prefix_bytes].chars().next_back()?;
        prefix.prefix_bytes -= previous.len_utf8();
        prefix.prefix_scalars -= 1;
        if prefix.prefix_bytes == 0 {
            self.cancel();
        }
        Some(caret - 1)
    }
}

enum InputStep {
    Text(String),
    Synthetic(&'static str),
    Backspace,
    Cancel,
}

/// Bounded by the current native event batch, never retained between frames.
/// Barriers are consumed in order, including motion away and back to one caret.
pub(super) struct InputOrder(VecDeque<InputStep>);
impl InputOrder {
    pub(super) fn new(events: &[egui::Event]) -> Self {
        let mut steps = VecDeque::new();
        for event in events {
            match event {
                egui::Event::Text(text) => steps.push_back(InputStep::Text(text.clone())),
                egui::Event::Key {
                    key,
                    pressed: true,
                    modifiers,
                    ..
                } if modifiers.command
                    || modifiers.ctrl
                    || matches!(
                        key,
                        egui::Key::ArrowLeft
                            | egui::Key::ArrowRight
                            | egui::Key::ArrowUp
                            | egui::Key::ArrowDown
                            | egui::Key::Home
                            | egui::Key::End
                            | egui::Key::PageUp
                            | egui::Key::PageDown
                            | egui::Key::Escape
                            | egui::Key::Delete
                    ) =>
                {
                    steps.push_back(InputStep::Cancel);
                }
                egui::Event::Key {
                    key: egui::Key::Enter,
                    pressed: true,
                    ..
                } => {
                    steps.push_back(InputStep::Synthetic("\n"));
                }
                egui::Event::Key {
                    key: egui::Key::Tab,
                    pressed: true,
                    ..
                } => {
                    steps.push_back(InputStep::Synthetic("\t"));
                }
                egui::Event::Key {
                    key: egui::Key::Backspace,
                    pressed: true,
                    ..
                } => {
                    steps.push_back(InputStep::Backspace);
                }
                egui::Event::PointerButton { pressed: true, .. }
                | egui::Event::WindowFocused(false)
                | egui::Event::Cut
                | egui::Event::Paste(_)
                | egui::Event::Ime(_) => steps.push_back(InputStep::Cancel),
                _ => {}
            }
        }
        Self(steps)
    }

    /// True identifies an actual committed Text event, not a programmatic edit.
    pub(super) fn before_text(&mut self, state: &mut State, text: &str) -> bool {
        while let Some(step) = self.0.pop_front() {
            match step {
                InputStep::Cancel | InputStep::Backspace => state.cancel(),
                InputStep::Text(expected) => {
                    if expected == text {
                        return true;
                    }
                    state.cancel();
                    return false;
                }
                InputStep::Synthetic(expected) => {
                    state.cancel();
                    if expected == text {
                        return false;
                    }
                }
            }
        }
        false
    }

    pub(super) fn before_backspace(&mut self, state: &mut State) -> bool {
        while let Some(step) = self.0.pop_front() {
            match step {
                InputStep::Cancel => state.cancel(),
                InputStep::Backspace => return true,
                InputStep::Text(_) | InputStep::Synthetic(_) => {
                    state.cancel();
                    return false;
                }
            }
        }
        state.cancel();
        false
    }

    pub(super) fn finish(&mut self, state: &mut State) {
        if !self.0.is_empty() {
            state.cancel();
        }
        self.0.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tiptoptyp_core::document::WindowSessionId;

    fn owner() -> Owner {
        Owner {
            document: DocumentKey::new(WindowSessionId::new(1), 2, 3),
            tab: 4,
            editor: egui::Id::new("source"),
        }
    }
    fn borrow(source: &str) -> (State, usize) {
        let start = source.rfind('\\').unwrap();
        let scalar = source[..start].chars().count();
        let mut state = State::default();
        state.begin(owner(), source, start..source.len(), scalar);
        (state, scalar + 1)
    }
    fn key(key: egui::Key) -> egui::Event {
        egui::Event::Key {
            key,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        }
    }

    #[test]
    fn divergent_text_restores_once_and_matching_text_completes_without_editing() {
        for source in [r"\(\)", r"\[\]", r"\begin{env}x\end{env}"] {
            let (mut state, caret) = borrow(source);
            let TextAction::Restore { insertion, advance } =
                state.continue_text(owner(), source, caret, "a", usize::MAX)
            else {
                panic!("typed command must preserve closer")
            };
            assert_eq!(insertion, "a\\");
            assert_eq!(advance, 1);
            assert_eq!(state.phase(), "idle");
            assert!(matches!(
                state.continue_text(owner(), source, caret, "b", usize::MAX),
                TextAction::Ordinary
            ));
            let (mut state, caret) = borrow(source);
            let suffix = &source[source.rfind('\\').unwrap() + 1..];
            assert!(matches!(
                state.continue_text(owner(), source, caret, suffix, usize::MAX),
                TextAction::Advance(_)
            ));
            assert_eq!(state.phase(), "idle");
        }
    }

    #[test]
    fn partial_unicode_environment_prefix_restores_exact_token_and_rewinds() {
        let source = r"\begin{文}x\end{文}";
        let (mut state, mut caret) = borrow(source);
        assert!(matches!(
            state.continue_text(owner(), source, caret, "end{文", usize::MAX),
            TextAction::Advance(5)
        ));
        caret += 5;
        let TextAction::Restore { insertion, advance } =
            state.continue_text(owner(), source, caret, "章", usize::MAX)
        else {
            panic!("divergence must restore the Unicode prefix")
        };
        assert_eq!(insertion, "章\\end{文");
        assert_eq!(advance, 1);

        let (mut state, caret) = borrow(source);
        assert_eq!(state.rewind(owner(), source, caret), Some(caret - 1));
        assert_eq!(state.phase(), "idle");
        assert_eq!(state.rewind(owner(), source, caret - 1), None);
    }

    #[test]
    fn exact_owner_token_caret_focus_and_revision_fence_the_prefix() {
        let source = r"\(\)";
        for changed in 0..7 {
            let (mut state, caret) = borrow(source);
            let mut current = owner();
            let mut text = source;
            let mut position = Some(caret);
            let mut allowed = true;
            match changed {
                0 => current.document = current.document.after_edit(),
                1 => current.tab += 1,
                2 => current.editor = egui::Id::new("diff"),
                3 => text = r"\(\]",
                4 => position = Some(caret - 1),
                5 => position = None,
                6 => allowed = false,
                _ => unreachable!(),
            }
            state.reconcile(current, text, position, allowed);
            assert_eq!(state.phase(), "idle", "case{changed}");
        }
        let (mut state, caret) = borrow(source);
        let next = owner().document.after_edit();
        state.finish(owner(), next, source, Some(caret), true, 10);
        let current = Owner {
            document: next,
            ..owner()
        };
        assert!(matches!(
            state.continue_text(current, source, caret, "a", usize::MAX),
            TextAction::Restore { .. }
        ));
    }

    #[test]
    fn hidden_or_unpainted_frames_cannot_revive_the_same_caret() {
        let source = r"\(\)";
        let (mut state, caret) = borrow(source);
        state.finish(owner(), owner().document, source, Some(caret), true, 10);
        state.end_frame(10);
        state.begin_frame(11);
        assert_eq!(state.phase(), "borrowing");
        state.end_frame(11);
        assert_eq!(state.phase(), "idle");
        let (mut state, caret) = borrow(source);
        state.finish(owner(), owner().document, source, Some(caret), true, 10);
        state.begin_frame(12);
        assert_eq!(state.phase(), "idle");
    }

    #[test]
    fn barriers_between_text_receipts_cancel_even_after_manual_return() {
        let source = r"\(\)";
        let mut input = InputOrder::new(&[
            egui::Event::Text("\\".into()),
            key(egui::Key::ArrowLeft),
            key(egui::Key::ArrowRight),
            egui::Event::Text("a".into()),
        ]);
        let mut state = State::default();
        assert!(input.before_text(&mut state, "\\"));
        state.begin(owner(), source, 2..4, 2);
        assert!(input.before_text(&mut state, "a"));
        assert!(matches!(
            state.continue_text(owner(), source, 3, "a", usize::MAX),
            TextAction::Ordinary
        ));
        let (mut state, _) = borrow(source);
        let mut input = InputOrder::new(&[key(egui::Key::ArrowLeft), key(egui::Key::ArrowRight)]);
        input.finish(&mut state);
        assert_eq!(state.phase(), "idle");
    }

    #[test]
    fn capacity_failure_is_atomic_and_the_original_closer_can_still_complete() {
        let source = r"\(\)";
        let (mut state, caret) = borrow(source);
        assert!(matches!(
            state.continue_text(owner(), source, caret, "a", source.chars().count() + 1),
            TextAction::AtLimit
        ));
        assert_eq!(state.phase(), "borrowing");
        assert!(matches!(
            state.continue_text(owner(), source, caret, ")", 0),
            TextAction::Advance(1)
        ));
        assert_eq!(state.phase(), "idle");
    }

    #[test]
    fn completed_closer_and_unicode_tail_share_one_receipt() {
        for (source, text, expected_advance) in
            [(r"\(\)", ")文", 1), (r"\begin{文}x\end{文}", "end{文}→", 6)]
        {
            let (mut state, caret) = borrow(source);
            let TextAction::Complete { advance, tail } =
                state.continue_text(owner(), source, caret, text, 0)
            else {
                panic!("completion must precede the remaining text")
            };
            assert_eq!(advance, expected_advance);
            assert_eq!(
                &text[tail..],
                if text.starts_with(')') { "文" } else { "→" }
            );
            assert_eq!(state.phase(), "idle");
        }
    }

    #[test]
    fn manual_return_before_backspace_retires_virtual_rewind() {
        let source = r"\(\)";
        let (mut state, caret) = borrow(source);
        let mut input = InputOrder::new(&[
            key(egui::Key::ArrowLeft),
            key(egui::Key::ArrowRight),
            key(egui::Key::Backspace),
        ]);
        assert!(input.before_backspace(&mut state));
        assert_eq!(state.rewind(owner(), source, caret), None);
        assert_eq!(state.phase(), "idle");
    }

    #[test]
    fn synthetic_newline_does_not_consume_later_committed_text() {
        let mut state = State::default();
        let mut input = InputOrder::new(&[
            key(egui::Key::Enter),
            egui::Event::Text("\\".into()),
            egui::Event::Text("a".into()),
        ]);
        assert!(!input.before_text(&mut state, "\n"));
        assert!(input.before_text(&mut state, "\\"));
        let source = r"\(\)";
        state.begin(owner(), source, 2..4, 2);
        assert!(input.before_text(&mut state, "a"));
        assert!(matches!(
            state.continue_text(owner(), source, 3, "a", usize::MAX),
            TextAction::Restore { .. }
        ));
    }
}
