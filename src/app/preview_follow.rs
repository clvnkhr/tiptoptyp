//! One coalesced source jump per edit burst; idle frames never schedule work.
use super::*;
use crate::tinymist::{CompileStatus, Generation};

const FOLLOW_DEBOUNCE: Duration = Duration::from_millis(200);

#[derive(Default)]
pub(super) struct PreviewFollow {
    pending: Option<PendingFollow>,
}

struct PendingFollow {
    key: DocumentKey,
    generation: Generation,
    edited_at: Instant,
    char_index: Option<usize>,
    compiled: bool,
}

#[derive(Debug, PartialEq, Eq)]
enum FollowAction {
    Idle,
    Wait(Duration),
    Jump(usize),
}

impl PreviewFollow {
    pub(super) fn clear(&mut self) {
        self.pending = None;
    }

    fn queue(&mut self, key: DocumentKey, generation: Generation, now: Instant) {
        self.pending = Some(PendingFollow {
            key,
            generation,
            edited_at: now,
            char_index: None,
            compiled: false,
        });
    }

    pub(super) fn compiled(&mut self, generation: Generation, status: CompileStatus, at: Instant) {
        if let Some(pending) = &mut self.pending
            && pending.generation == generation
            && at >= pending.edited_at
        {
            // Compile reports have no document version. Ignore reports already
            // received before this edit and wait through any subsequent compile.
            pending.compiled = status == CompileStatus::CompileSuccess;
        }
    }

    fn poll(
        &mut self,
        key: DocumentKey,
        generation: Option<Generation>,
        ready: bool,
        now: Instant,
    ) -> FollowAction {
        let Some(pending) = &self.pending else {
            return FollowAction::Idle;
        };
        if pending.key != key || Some(pending.generation) != generation {
            self.clear();
            return FollowAction::Idle;
        }
        let Some(char_index) = pending.char_index else {
            return FollowAction::Idle;
        };
        if !ready || !pending.compiled {
            // Compilation/readiness events wake the UI. No polling timer.
            return FollowAction::Idle;
        }
        let deadline = pending.edited_at + FOLLOW_DEBOUNCE;
        if now < deadline {
            return FollowAction::Wait(deadline - now);
        }
        self.clear();
        FollowAction::Jump(char_index)
    }
}

impl EditorApp {
    fn preview_follows_edits(&self) -> bool {
        self.settings.preview_follow_edits
            && self.settings.preview_preference == PreviewPreference::Interactive
            && self.snapshot_scene.is_none()
            && !self.compilation_paused
            && self.document().kind().is_typst()
            && self.preview_visible()
            && self.interactive_preview_requested()
    }

    pub(super) fn queue_preview_follow(&mut self) {
        self.preview_follow.clear();
        if self.preview_follows_edits()
            && let Some(generation) = self.tinymist_sync.generation
        {
            self.preview_follow
                .queue(self.document().key(), generation, Instant::now());
        }
    }

    pub(super) fn tick_preview_follow(&mut self, context: &egui::Context) {
        if self.preview_follow.pending.is_none() {
            return;
        }
        if !self.preview_follows_edits() {
            self.preview_follow.clear();
            return;
        }
        if let Some(pending) = &mut self.preview_follow.pending
            && pending.char_index.is_none()
        {
            // Transactions can update the selection after mark_edited. Capture
            // it at the end of that frame, once, so later cursor-only movement
            // cannot redirect the delayed jump. Dialog/completion edits may
            // still have a selection waiting for the next editor frame.
            pending.char_index = Some(
                self.pending_editor_selection
                    .as_ref()
                    .map(|selection| selection.range().start)
                    .or_else(|| {
                        egui::text_edit::TextEditState::load(context, source_editor_id(context))
                            .and_then(|state| state.cursor.char_range())
                            .map(|cursor| cursor.primary.index.0)
                    })
                    .unwrap_or(0),
            );
        }
        match self.preview_follow.poll(
            self.document().key(),
            self.tinymist_sync.generation,
            self.interactive_preview_active(),
            Instant::now(),
        ) {
            FollowAction::Idle => {}
            FollowAction::Wait(delay) => context.request_repaint_after(delay),
            FollowAction::Jump(char_index) => self.jump_source_to_preview(char_index),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tiptoptyp_core::document::WindowSessionId;

    fn key() -> DocumentKey {
        DocumentKey::new(WindowSessionId::new(1), 2, 3)
    }

    fn queue(follow: &mut PreviewFollow, key: DocumentKey, at: Instant, cursor: usize) {
        follow.queue(key, Generation(1), at);
        follow.pending.as_mut().unwrap().char_index = Some(cursor);
    }

    #[test]
    fn edit_bursts_coalesce_and_wait_for_compile_and_debounce() {
        let now = Instant::now();
        let mut follow = PreviewFollow::default();
        queue(&mut follow, key(), now, 4);
        let latest = now + Duration::from_millis(100);
        queue(&mut follow, key().after_edit(), latest, 5);
        let poll = |follow: &mut PreviewFollow, at| {
            follow.poll(key().after_edit(), Some(Generation(1)), true, at)
        };
        // Neither an old report nor another server can release this edit.
        follow.compiled(Generation(1), CompileStatus::CompileSuccess, now);
        follow.compiled(Generation(2), CompileStatus::CompileSuccess, latest);
        assert_eq!(poll(&mut follow, latest), FollowAction::Idle);
        follow.compiled(Generation(1), CompileStatus::CompileSuccess, latest);
        assert_eq!(
            poll(&mut follow, latest),
            FollowAction::Wait(FOLLOW_DEBOUNCE)
        );
        assert_eq!(
            poll(&mut follow, latest + FOLLOW_DEBOUNCE),
            FollowAction::Jump(5)
        );
        for _ in 0..100 {
            assert_eq!(
                poll(&mut follow, latest + FOLLOW_DEBOUNCE),
                FollowAction::Idle
            );
        }
    }

    #[test]
    fn slow_or_failed_compiles_and_unready_views_do_not_poll_or_jump() {
        let now = Instant::now();
        let later = now + Duration::from_secs(10);
        let mut follow = PreviewFollow::default();
        queue(&mut follow, key(), now, 42);
        for status in [CompileStatus::Compiling, CompileStatus::CompileError] {
            follow.compiled(Generation(1), status, later);
            assert_eq!(
                follow.poll(key(), Some(Generation(1)), true, later),
                FollowAction::Idle
            );
        }
        follow.compiled(Generation(1), CompileStatus::CompileSuccess, later);
        assert_eq!(
            follow.poll(key(), Some(Generation(1)), false, later),
            FollowAction::Idle
        );
        assert_eq!(
            follow.poll(key(), Some(Generation(1)), true, later),
            FollowAction::Jump(42)
        );
    }

    #[test]
    fn replacing_a_document_revision_tab_or_server_discards_the_jump() {
        let now = Instant::now();
        for (current, generation) in [
            (key().after_edit(), Some(Generation(1))),
            (DocumentKey { epoch: 9, ..key() }, Some(Generation(1))),
            (
                DocumentKey {
                    owner: WindowSessionId::new(2),
                    ..key()
                },
                Some(Generation(1)),
            ),
            (key(), Some(Generation(2))),
            (key(), None),
        ] {
            let mut follow = PreviewFollow::default();
            queue(&mut follow, key(), now, 42);
            follow.compiled(Generation(1), CompileStatus::CompileSuccess, now);
            assert_eq!(
                follow.poll(current, generation, true, now + FOLLOW_DEBOUNCE),
                FollowAction::Idle
            );
            assert!(follow.pending.is_none());
        }
    }

    #[cfg(any(target_os = "macos", target_os = "windows"))]
    fn app(context: &egui::Context, root: &Path) -> EditorApp {
        let mut app = EditorApp::dormant_for_tests(context, root.into());
        app.snapshot_scene = None;
        app.settings.auto_save = false;
        app.document_mut()
            .replace_unprojected_untitled("one\nα🙂two");
        app.view_mode = ViewMode::Split;
        app.tinymist_sync.generation = Some(Generation(1));
        assert!(app.preview_follows_edits());
        app
    }

    #[test]
    #[cfg(any(target_os = "macos", target_os = "windows"))]
    fn edits_capture_the_final_caret_once_without_moving_editor_focus() {
        let root = tempfile::tempdir().unwrap();
        let context = egui::Context::default();
        let mut app = app(&context, root.path());
        app.document_mut()
            .edit(CCursorRange::default(), |source| source.push('!'));
        app.mark_edited();
        // Source commands can place the caret after mark_edited.
        app.store_editor_cursor(&context, CCursorRange::one(CCursor::new(10)));
        let focus = egui::Id::new("editor-focus-test");
        context.memory_mut(|memory| memory.request_focus(focus));
        app.tick_preview_follow(&context);
        assert_eq!(
            app.preview_follow.pending.as_ref().unwrap().char_index,
            Some(10)
        );

        app.store_editor_cursor(&context, CCursorRange::one(CCursor::new(1)));
        app.mark_edited(); // No transaction: cursor movement must not requeue.
        app.tick_preview_follow(&context);
        assert_eq!(
            app.preview_follow.pending.as_ref().unwrap().char_index,
            Some(10)
        );
        assert_eq!(context.memory(|memory| memory.focused()), Some(focus));
        assert_eq!(app.view_mode, ViewMode::Split);

        // A table dialog or completion can supply the next selection directly.
        app.document_mut()
            .edit(CCursorRange::default(), |source| source.push('?'));
        app.mark_edited();
        app.pending_editor_selection = Some(EditorSelection::Focus(6..8));
        app.tick_preview_follow(&context);
        assert_eq!(
            app.preview_follow.pending.as_ref().unwrap().char_index,
            Some(6)
        );
    }

    #[test]
    #[cfg(any(target_os = "macos", target_os = "windows"))]
    fn disabled_hidden_paused_and_other_backends_cancel_pending_follows() {
        let root = tempfile::tempdir().unwrap();
        let context = egui::Context::default();
        for change in [
            |app: &mut EditorApp| app.settings.preview_follow_edits = false,
            |app: &mut EditorApp| app.view_mode = ViewMode::Code,
            |app: &mut EditorApp| app.compilation_paused = true,
            |app: &mut EditorApp| app.settings.preview_preference = PreviewPreference::PdfJs,
            |app: &mut EditorApp| app.settings.preview_preference = PreviewPreference::Native,
            |app: &mut EditorApp| {
                app.document_mut().replace_loaded_unprojected(
                    "plain".into(),
                    PathBuf::from("notes.txt"),
                    DocumentKind::Text,
                    None,
                );
            },
        ] {
            let mut app = app(&context, root.path());
            app.queue_preview_follow();
            assert!(app.preview_follow.pending.is_some());
            change(&mut app);
            let view_mode = app.view_mode;
            app.tick_preview_follow(&context);
            assert!(app.preview_follow.pending.is_none());
            app.queue_preview_follow();
            assert!(app.preview_follow.pending.is_none());
            assert_eq!(app.view_mode, view_mode);
        }
    }

    #[test]
    #[cfg(any(target_os = "macos", target_os = "windows"))]
    fn manual_jumps_supersede_automatic_jumps() {
        let root = tempfile::tempdir().unwrap();
        let context = egui::Context::default();
        let mut app = app(&context, root.path());
        app.queue_preview_follow();
        app.jump_source_to_preview(2);
        assert!(app.preview_follow.pending.is_none());
    }
}
