//! The docked diff is a live view of the same document and undo history.
use super::*;
use crate::git::editor::{document_diff::DocumentDiff, full_view};
impl EditorApp {
    pub(super) fn prepare_document_diff(
        &mut self,
        chunk: &crate::git::editor::ChunkDiff,
    ) -> Result<(), String> {
        let canonical = self.canonical_document_source()?;
        let baseline = DocumentDiff::baseline(&canonical, &self.git_editor.hunks)?;
        let baseline = self
            .document()
            .project_canonical_change(
                self.document().key(),
                tiptoptyp_core::text::AppliedTextEdits {
                    text: baseline,
                    mapped_offsets: [ScalarOffset::new(0); 2],
                },
            )
            .map_err(|error| error.to_string())?
            .text;
        self.document_diff = Some(DocumentDiff::new(baseline, self.document().source()));
        let line = chunk.hunk.changes.first().map_or(0, |c| c.lines.start);
        let at = self
            .document()
            .source()
            .split_inclusive('\n')
            .take(line)
            .map(|s| s.chars().count())
            .sum();
        self.editor_attention = Some(EditorAttention::arm(at));
        self.pending_editor_selection = Some(EditorSelection::Focus(at..at));
        Ok(())
    }
    pub(super) fn navigate_document_diff(&mut self, context: &egui::Context, previous: bool) {
        let cursor = self.editor_snapshot(context).cursor.primary.index.0;
        let line = self
            .document()
            .source()
            .chars()
            .take(cursor)
            .filter(|&c| c == '\n')
            .count();
        if let Some(diff) = self.document_diff.as_mut() {
            diff.refresh(self.tabs.current_record().document.source());
            if let Some(line) = diff.destination(line, previous) {
                let at = diff
                    .current
                    .split_inclusive('\n')
                    .take(line)
                    .map(|s| s.chars().count())
                    .sum();
                self.editor_attention = Some(EditorAttention::arm(at));
                self.pending_editor_selection = Some(EditorSelection::Focus(at..at));
                self.store_editor_cursor(context, CCursorRange::one(CCursor::new(at)));
                let id = source_editor_id(context);
                context.memory_mut(|m| m.request_focus(id));
                context.request_repaint();
            }
        }
    }
    pub(super) fn show_document_diff(&mut self, ui: &mut egui::Ui) {
        ui.set_min_width(ui.available_width());
        let editor_bounds = ui.available_rect_before_wrap();
        let path = self.document().path().clone().unwrap_or_default();
        let pending_selection = self.pending_editor_selection.take();
        let takes_focus = pending_selection
            .as_ref()
            .is_some_and(EditorSelection::takes_focus);
        let jump = pending_selection.map(|selection| selection.range().clone());
        if let Some(range) = &jump {
            self.store_editor_cursor(
                ui.ctx(),
                CCursorRange::two(CCursor::new(range.start), CCursor::new(range.end)),
            );
            if takes_focus {
                let id = source_editor_id(ui.ctx());
                ui.memory_mut(|m| m.request_focus(id));
            }
        }
        let snapshot = self.editor_snapshot(ui.ctx());
        let key = self.document().key();
        let cursor_line = snapshot
            .source
            .chars()
            .take(snapshot.cursor.primary.index.0)
            .filter(|&c| c == '\n')
            .count();
        let selected = self
            .git_editor
            .hunks
            .iter()
            .find(|h| {
                h.changes.iter().any(|c| {
                    c.lines.contains(&cursor_line)
                        || (c.lines.is_empty() && c.lines.start == cursor_line)
                })
            })
            .cloned();
        let repository_ready = selected.is_some()
            && self.git_editor.activity() == crate::activity::Activity::Idle
            && !self.git_hunk_job.is_running();
        let mitex_dollars = self.document().config().is_some();
        let settings = &self.settings;
        let source_id = source_editor_id(ui.ctx());
        let diff = self.document_diff.as_mut().unwrap();
        let auto_pair = &mut self.auto_pair_syntax;
        let kind = self.tabs.current_record().document.kind();
        if settings.ascii_punctuation && ui.memory(|m| m.has_focus(source_id)) {
            ui.input_mut(|input| crate::auto_pairs::ascii_punctuation(&mut input.events));
        }
        let particle_settings = settings.editor_effects;
        let typing_intent = editor_particles::capture(
            ui,
            source_id,
            editor_particles::input_enabled(particle_settings),
        );
        let effect_paint = editor_particles::prepare_shake(ui, source_id, particle_settings);
        let mut output = self
            .tabs
            .current_record_mut()
            .document
            .edit(snapshot.cursor, |source| {
                let mut buffer = ui.input(|input| {
                    crate::auto_pairs::PairingBuffer::new(
                        source,
                        auto_pair,
                        settings.auto_pair_delimiters && kind.is_typst(),
                        &input.events,
                    )
                    .with_surround(settings.auto_pair_delimiters, &input.events)
                    .with_indentation(settings.indent_spaces)
                });
                full_view::show(
                    ui,
                    diff,
                    &mut buffer,
                    full_view::ViewConfig {
                        id: source_id,
                        path: &path,
                        settings,
                        repository_ready,
                        mitex_dollars,
                        jump,
                    },
                )
            });
        let context_click = super::editor_view::secondary_click_location(
            ui,
            &mut output.edit,
            Some(snapshot.cursor),
            &mut self.secondary_press_guard,
        );
        let cursor = output
            .edit
            .state
            .cursor
            .char_range()
            .unwrap_or(snapshot.cursor);
        // Keep menu, Undo/Redo, preview following and inspection on the source
        // editor's existing identity, even while it is presented as a diff.
        if self.document().key() != key {
            self.find_bar.search.clear();
            self.mark_edited();
        }
        let caret = output
            .edit
            .galley
            .pos_from_cursor(cursor.primary)
            .translate(output.edit.galley_pos.to_vec2());
        self.last_editor_caret = Some(EditorCaretState {
            key: self.document().key(),
            char_index: cursor.primary.index.0,
            rect: caret,
        });
        #[cfg(all(feature = "desktop-ui-tests", target_os = "macos"))]
        if cursor.is_empty() {
            let mut target = output.edit.response.clone();
            target.interact_rect = Rect::from_center_size(caret.center(), Vec2::splat(1.0));
            crate::desktop_test::observe("editor.caret", &target);
        }
        if let Some((anchor, character)) = context_click {
            self.prepare_editor_data();
            let popup = super::editor_view::editor_context_popup(
                &mut self.editor_data,
                self.tabs.current_record().document.source(),
                kind,
                anchor,
                character,
                self.tabs.current_record().document.key(),
            )
            .with_liquid_selection(self.document().snapshot(), kind, Some(cursor));
            self.open_app_popup(popup);
        }
        let attention_viewport = Rect::from_min_max(
            Pos2::new(
                output.edit.response.rect.left().max(output.viewport.left()),
                output.viewport.top(),
            ),
            output.viewport.right_bottom(),
        );
        let effect_paint = effect_paint.map(|marker| marker.finish(ui));
        reveal::paint_editor(
            ui,
            attention_viewport,
            &output.edit.galley,
            output.edit.galley_pos,
            &mut self.editor_attention,
            self.settings.source_reveal_animation,
        );
        editor_particles::paint(
            ui,
            editor_particles::Surface {
                edit: &output.edit,
                document: self.document().key(),
                viewport: attention_viewport,
                view: editor_particles::View::Diff,
                paint_range: effect_paint,
                shake_viewport: output.viewport,
            },
            particle_settings,
            typing_intent,
            self.document().key() != key,
        );
        match output.action {
            Some(full_view::Action::Close) => {
                self.close_document_diff();
                let range = cursor.as_sorted_char_range();
                self.pending_editor_selection =
                    Some(EditorSelection::Focus(range.start.0..range.end.0));
                self.focus_source_editor(ui.ctx());
            }
            Some(full_view::Action::Previous) => self.navigate_document_diff(ui.ctx(), true),
            Some(full_view::Action::Next) => self.navigate_document_diff(ui.ctx(), false),
            Some(full_view::Action::RevertLine(index)) => {
                if let Some(text) = self
                    .document_diff
                    .as_ref()
                    .and_then(|d| d.revert_line(index))
                {
                    self.document_mut().edit(cursor, |source| *source = text);
                    let source = self.document().source().to_owned();
                    self.document_diff.as_mut().unwrap().refresh(&source);
                    self.store_editor_cursor(ui.ctx(), cursor);
                    self.mark_edited();
                    ui.memory_mut(|m| m.request_focus(source_id));
                }
            }
            Some(action) => {
                if let Some(hunk) = selected {
                    let action = match action {
                        full_view::Action::Stage => crate::git::repository::hunks::Action::Stage,
                        full_view::Action::Unstage => {
                            crate::git::repository::hunks::Action::Unstage
                        }
                        _ => crate::git::repository::hunks::Action::Revert,
                    };
                    self.perform_hunk_action(
                        ui.ctx(),
                        action,
                        self.document().key(),
                        crate::git::editor::ChunkDiff {
                            path: path.clone(),
                            hunk,
                        },
                    );
                }
            }
            None => {}
        }
        if let Some(diff) = self.document_diff.as_ref() {
            crate::git::editor::overview::paint(
                ui,
                output.viewport,
                output.line_count,
                &diff.hunks,
            );
        }
        self.show_find_overlay(ui, editor_bounds);
    }
}
