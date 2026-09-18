use super::*;
use crate::git::{
    editor::ChunkDiff,
    repository::{
        Repository,
        hunks::{self, Action},
    },
};

impl EditorApp {
    pub(super) fn handle_hunk_shortcuts(
        &mut self,
        context: &egui::Context,
        viewport: egui::ViewportId,
        shortcuts: &ShortcutBindings,
    ) {
        if self.document_flow_busy() {
            return;
        }
        let action = Action::ALL.into_iter().find(|action| {
            shortcuts.egui(action.shortcut()).is_some_and(|shortcut| {
                context.input_mut_for(viewport, |input| input.consume_shortcut(&shortcut))
            })
        });
        let Some(action) = action else {
            return;
        };
        if matches!(action, Action::Next | Action::Previous) {
            self.navigate_hunk(context, action == Action::Previous);
        } else if let Some(chunk) = self.git_editor.chunk.clone() {
            self.perform_hunk_action(context, action, self.document().key(), chunk);
        } else {
            // Action shortcuts select the hunk under the caret, never an
            // arbitrary first change elsewhere in the document.
            let cursor = self.editor_snapshot(context).cursor.primary.index.0;
            let line = self
                .document()
                .source()
                .chars()
                .take(cursor)
                .filter(|c| *c == '\n')
                .count();
            if let Some(index) = self.git_editor.hunks.iter().position(|h| {
                h.changes.iter().any(|c| {
                    c.lines.contains(&line) || (c.lines.is_empty() && c.lines.start == line)
                })
            }) && let Some(path) = self.document().path().clone()
            {
                self.git_editor.open_chunk(index, &path);
                let chunk = self.git_editor.chunk.clone().unwrap();
                self.perform_hunk_action(context, action, self.document().key(), chunk);
            }
        }
    }

    fn navigate_hunk(&mut self, context: &egui::Context, previous: bool) {
        let Some(path) = self.document().path().clone() else {
            return;
        };
        let cursor = self.editor_snapshot(context).cursor.primary.index.0;
        let line = self
            .document()
            .source()
            .chars()
            .take(cursor)
            .filter(|c| *c == '\n')
            .count();
        if let Some(line) = self.git_editor.navigate(&path, line, previous) {
            self.apply_editor_location(None, Some((line + 1, 1)));
            if let Some(chunk) = self.git_editor.chunk.clone() {
                let anchor = self
                    .last_editor_caret
                    .map_or(Pos2::new(180.0, 80.0), |caret| caret.rect.left_bottom());
                self.open_app_popup(AppPopup::GitChunk { anchor, chunk });
            }
        }
    }

    pub(super) fn perform_hunk_action(
        &mut self,
        context: &egui::Context,
        action: Action,
        key: DocumentKey,
        chunk: ChunkDiff,
    ) {
        if self.document_flow_busy() || self.git_hunk_job.is_running() {
            return;
        }
        if key != self.document().key() || !self.git_editor.selection_is_current(key, &chunk) {
            self.show_file_error("This hunk is stale; reopen it before making changes".into());
            return;
        }
        if matches!(action, Action::Next | Action::Previous) {
            self.git_editor.chunk = Some(chunk);
            self.navigate_hunk(context, action == Action::Previous);
            return;
        }
        let source = match self.canonical_document_source() {
            Ok(source) => source,
            Err(error) => {
                self.show_file_error(error);
                return;
            }
        };
        if action == Action::Revert {
            let replacement = hunks::revert(&source, &chunk.hunk).and_then(|text| {
                self.document()
                    .project_canonical_change(
                        key,
                        tiptoptyp_core::text::AppliedTextEdits {
                            text,
                            mapped_offsets: [ScalarOffset::new(0); 2],
                        },
                    )
                    .map_err(|error| error.to_string())
            });
            match replacement {
                Ok(replacement) => {
                    let cursor = self.editor_snapshot(context).cursor;
                    self.document_mut()
                        .edit(cursor, |source| *source = replacement.text);
                    self.search.clear();
                    self.mark_edited();
                    self.close_app_popup();
                    self.git_editor.chunk = None;
                    self.notice = Some(Notice {
                        message: "Reverted hunk in editor · Undo to restore".into(),
                        kind: NoticeKind::Success,
                    });
                }
                Err(error) => self.show_file_error(error),
            }
        } else {
            let root = self.workspace_root.clone();
            if let Err(error) =
                self.git_hunk_job
                    .start_and_repaint("git-hunk", context, move || {
                        Repository::new(&root).change_index(
                            &chunk.path,
                            &source,
                            &chunk.hunk,
                            action,
                        )
                    })
            {
                self.show_file_error(error);
            }
        }
    }

    pub(super) fn poll_hunk_action(&mut self) {
        match self.git_hunk_job.poll() {
            LatestJobPoll::Ready(message) => {
                self.git.request_refresh();
                self.git_editor.request_refresh();
                self.notice = Some(Notice {
                    message,
                    kind: NoticeKind::Success,
                });
            }
            LatestJobPoll::Failed(error) => self.show_file_error(error),
            LatestJobPoll::Idle | LatestJobPoll::Pending => {}
        }
    }
}
