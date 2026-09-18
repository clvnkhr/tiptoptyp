//! Less-frequent workspace/editor commands, resolved only for actual key events.
use super::*;
use ShortcutAction as A;

fn is_extra(action: ShortcutAction) -> bool {
    matches!(
        action,
        A::KeyboardShortcuts
            | A::ToggleTexMode
            | A::UseTabForPreview
            | A::ToggleFold
            | A::CollapseAll
            | A::ExpandAll
            | A::FindNext
            | A::FindPrevious
            | A::ReplaceOne
            | A::ReplaceAll
            | A::ToggleFindCase
            | A::ToggleFindRegex
            | A::PreviewPreviousPage
            | A::PreviewNextPage
            | A::PreviewFitWidth
            | A::ExplorerSearch
            | A::RefreshWorkspace
            | A::StatusHistory
            | A::ToggleLineWrap
            | A::ToggleLineNumbers
            | A::ToggleStickyContext
            | A::UiScaleReset
            | A::WindowColor
            | A::FileMenu
            | A::EditMenu
            | A::ViewMenu
    )
}

impl EditorApp {
    pub(super) fn handle_extra_shortcuts(
        &mut self,
        context: &egui::Context,
        viewport: egui::ViewportId,
        bindings: &ShortcutBindings,
    ) {
        let action = context.input_mut_for(viewport, |input| {
            let (index, action) = input.events.iter().enumerate().find_map(|(index, event)| {
                let egui::Event::Key {
                    key,
                    modifiers,
                    pressed: true,
                    ..
                } = event
                else {
                    return None;
                };
                // Resolve against ALL effective bindings before filtering. A
                // more-specific legacy command must not become a new base chord.
                let action = bindings.action_for_key_event(*key, *modifiers)?;
                is_extra(action).then_some((index, action))
            })?;
            input.events.remove(index);
            Some(action)
        });
        let Some(action) = action else {
            return;
        };
        if action == A::KeyboardShortcuts {
            self.shortcut_editor_visible = !self.shortcut_editor_visible;
            self.shortcut_capture = None;
            context.request_repaint();
            return;
        }
        if action == A::UiScaleReset {
            let scale = self
                .pending_settings
                .as_ref()
                .unwrap_or(&self.settings)
                .ui_scale_percent;
            self.adjust_ui_scale(100 - scale as i16, context);
            return;
        }
        // Never act on a document behind another native child or a file dialog.
        if viewport != context.viewport_id()
            || self.document_flow_busy()
            || self.process_close_pending
        {
            return;
        }
        self.execute_extra_shortcut(action, context);
    }

    fn execute_extra_shortcut(&mut self, action: ShortcutAction, context: &egui::Context) {
        let anchor =
            context.content_rect().left_top() + Vec2::new(90.0, METRICS.chrome.toolbar_height);
        match action {
            A::FileMenu | A::EditMenu | A::ViewMenu => {
                let popup = match action {
                    A::FileMenu => AppPopup::File { anchor },
                    A::EditMenu => AppPopup::Edit { anchor },
                    _ => AppPopup::View { anchor },
                };
                self.open_app_popup(popup);
            }
            A::WindowColor => crate::window_logo::toggle(context),
            A::StatusHistory => self.open_app_popup(AppPopup::StatusLog {
                anchor: context.content_rect().left_bottom() - Vec2::new(0.0, 40.0),
            }),
            A::ExplorerSearch => {
                self.explorer.open();
                self.focus_explorer_search = true;
            }
            A::RefreshWorkspace => self.refresh_workspace(),
            _ if self.tabs.is_empty() => return,
            A::ToggleTexMode if self.tex_mode_available() => {
                self.set_tex_mode(self.document().config().is_none(), context);
            }
            A::UseTabForPreview => {
                if let Some(id) = self.tabs.active_id() {
                    self.select_preview_tab(id, context);
                }
            }
            A::ToggleFold | A::CollapseAll | A::ExpandAll => self.fold_shortcut(action, context),
            A::FindNext | A::FindPrevious | A::ReplaceOne | A::ReplaceAll
                if self.document().kind().is_editable() =>
            {
                if self.find_query.is_empty() {
                    self.open_find(matches!(action, A::ReplaceOne | A::ReplaceAll));
                } else if !matches!(action, A::ReplaceOne | A::ReplaceAll)
                    || (self.find_visible && self.replace_visible)
                {
                    self.apply_find_actions(
                        context,
                        action == A::FindPrevious,
                        action == A::FindNext,
                        action == A::ReplaceOne,
                        action == A::ReplaceAll,
                    );
                } else {
                    // Never perform a hidden, stale replacement. First expose
                    // the query and replacement for review, like the UI button.
                    self.open_find(true);
                }
            }
            A::ToggleFindCase | A::ToggleFindRegex if self.document().kind().is_editable() => {
                self.find_visible = true;
                if action == A::ToggleFindCase {
                    self.find_case_sensitive = !self.find_case_sensitive;
                } else {
                    self.find_regex = !self.find_regex;
                }
                self.search.clear();
            }
            A::PreviewPreviousPage | A::PreviewNextPage | A::PreviewFitWidth => {
                if !self.preview_visible()
                    || (!self.document().kind().preview_only() && self.interactive_preview_active())
                {
                    return;
                }
                let preview = if self.document().kind().preview_only() {
                    &mut self.asset_preview
                } else {
                    &mut self.preview
                };
                raster_shortcut(preview, action);
            }
            A::ToggleLineWrap | A::ToggleLineNumbers | A::ToggleStickyContext => {
                let mut settings = self
                    .pending_settings
                    .clone()
                    .unwrap_or_else(|| self.settings.clone());
                match action {
                    A::ToggleLineWrap => settings.line_wrap = !settings.line_wrap,
                    A::ToggleLineNumbers => settings.line_numbers = !settings.line_numbers,
                    _ => settings.sticky_context_rows = !settings.sticky_context_rows,
                }
                self.queue_settings(settings, context);
            }
            _ => return,
        }
        context.request_repaint();
    }

    fn fold_shortcut(&mut self, action: ShortcutAction, context: &egui::Context) {
        if !self.document().kind().is_typst() || !self.settings.line_numbers {
            return;
        }
        if context.egui_wants_keyboard_input()
            && context.memory(|m| m.focused()) != Some(source_editor_id(context))
        {
            return;
        }
        self.prepare_editor_source_data();
        let key = self.document().key();
        let source = self.editor_data.source_snapshot();
        let contexts = self.editor_data.context_regions();
        self.folding_mut().prepare(key, source, &contexts);
        let cursor = self.editor_snapshot(context).cursor.primary.index.0;
        let line = self
            .document()
            .source()
            .chars()
            .take(cursor)
            .filter(|&c| c == '\n')
            .count();
        let target = match action {
            A::ToggleFold => {
                let Some(region) = self.folding().region_at(line).cloned() else {
                    return;
                };
                self.folding_mut().toggle(region.line);
                self.folding()
                    .is_collapsed(region.line)
                    .then_some(region.header)
            }
            A::CollapseAll => {
                self.folding_mut().collapse_all();
                self.folding()
                    .regions
                    .iter()
                    .find(|r| r.line <= line && line < r.end_line)
                    .map(|r| r.header)
            }
            _ => {
                self.folding_mut().expand_all();
                None
            }
        };
        if let Some(target) = target {
            self.pending_editor_selection = Some(EditorSelection::Focus(target..target));
            let id = source_editor_id(context);
            let mut state = egui::text_edit::TextEditState::load(context, id).unwrap_or_default();
            state
                .cursor
                .set_char_range(Some(CCursorRange::one(CCursor::new(target))));
            state.store(context, id);
        }
    }
}

fn raster_shortcut(preview: &mut PreviewController, action: ShortcutAction) {
    let count = preview.content.pages().len();
    if count == 0 {
        return;
    }
    let page = preview.requested_page.unwrap_or(preview.visible_page);
    match action {
        A::PreviewPreviousPage => preview.requested_page = Some(page.saturating_sub(1)),
        A::PreviewNextPage => preview.requested_page = Some((page + 1).min(count - 1)),
        A::PreviewFitWidth => preview.fit_width = !preview.fit_width,
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn press(app: &mut EditorApp, context: &egui::Context, action: A) {
        let bindings = app.settings.effective_shortcuts();
        let chord = bindings.egui(action).unwrap();
        context
            .run_ui(
                egui::RawInput {
                    events: vec![egui::Event::Key {
                        key: chord.logical_key,
                        physical_key: None,
                        pressed: true,
                        repeat: false,
                        modifiers: chord.modifiers,
                    }],
                    ..Default::default()
                },
                |ui| app.handle_extra_shortcuts(ui.ctx(), ui.ctx().viewport_id(), &bindings),
            )
            .drop_without_applying_deltas();
    }

    fn app(context: &egui::Context, root: &Path) -> EditorApp {
        let mut app = EditorApp::dormant_for_tests(context, root.into());
        app.document_mut().replace_loaded_unprojected(
            "#let f(x) = {\n  α + x\n}\n".into(),
            root.join("first.typ"),
            DocumentKind::Typst,
            None,
        );
        app.document_mut().set_history_reset(false);
        app
    }

    #[test]
    fn added_bindings_toggle_workspace_controls_and_respect_close_guards() {
        let context = egui::Context::default();
        let root = tempfile::tempdir().unwrap();
        let mut app = app(&context, root.path());
        app.explorer.hide();
        press(&mut app, &context, A::ExplorerSearch);
        assert!(app.explorer.panel_visible());
        assert!(app.focus_explorer_search);
        press(&mut app, &context, A::KeyboardShortcuts);
        assert!(app.shortcut_editor_visible);
        app.process_close_pending = true;
        press(&mut app, &context, A::ToggleLineWrap);
        assert!(app.pending_settings.is_none());
        app.process_close_pending = false;
        press(&mut app, &context, A::ToggleLineWrap);
        assert_eq!(
            app.pending_settings.as_ref().unwrap().line_wrap,
            !app.settings.line_wrap
        );
        app.empty_workspace(&context);
        press(&mut app, &context, A::CollapseAll);
        assert!(app.tabs.is_empty());
        assert!(app.folding().regions.is_empty());
    }

    #[test]
    fn modified_legacy_chord_does_not_trigger_new_base_action() {
        let context = egui::Context::default();
        let root = tempfile::tempdir().unwrap();
        let mut app = app(&context, root.path());
        let bindings = app.settings.effective_shortcuts();
        let chord = bindings.egui(A::Packages).unwrap();
        context
            .run_ui(
                egui::RawInput {
                    events: vec![egui::Event::Key {
                        key: chord.logical_key,
                        physical_key: None,
                        pressed: true,
                        repeat: false,
                        modifiers: chord.modifiers,
                    }],
                    ..Default::default()
                },
                |ui| {
                    app.handle_extra_shortcuts(ui.ctx(), ui.ctx().viewport_id(), &bindings);
                    assert_eq!(
                        ui.input_mut(|input| consume_shortcut(input, &bindings, |_| true)),
                        Some(AppCommand::Packages)
                    );
                    assert!(app.compile_deadline.is_none());
                },
            )
            .drop_without_applying_deltas();
    }

    #[test]
    fn fold_chords_move_caret_to_visible_header_without_editing_source() {
        let context = egui::Context::default();
        let root = tempfile::tempdir().unwrap();
        let mut app = app(&context, root.path());
        app.settings.line_numbers = true;
        let key = app.document().key();
        let source = app.document().source().clone();
        let cursor = source[..source.find('α').unwrap()].chars().count();
        let mut state = egui::text_edit::TextEditState::default();
        state
            .cursor
            .set_char_range(Some(CCursorRange::one(CCursor::new(cursor))));
        state.store(&context, source_editor_id(&context));
        press(&mut app, &context, A::CollapseAll);
        assert!(app.folding().is_collapsed(0));
        assert!(app.pending_editor_selection.is_some());
        press(&mut app, &context, A::ExpandAll);
        assert!(!app.folding().is_collapsed(0));
        press(&mut app, &context, A::ToggleFold);
        assert!(app.folding().is_collapsed(0));
        assert_eq!(app.document().key(), key);
        assert_eq!(*app.document().source(), source);
    }

    #[test]
    fn replacement_chords_expose_hidden_fields_before_changing_text() {
        let context = egui::Context::default();
        let root = tempfile::tempdir().unwrap();
        let mut app = app(&context, root.path());
        app.find_query = "α".into();
        app.replacement = "β".into();
        let source = app.document().source().clone();
        press(&mut app, &context, A::ReplaceAll);
        assert_eq!(*app.document().source(), source);
        assert!(app.find_visible && app.replace_visible);
        press(&mut app, &context, A::FindNext);
        assert!(app.pending_editor_selection.is_some());
        press(&mut app, &context, A::ReplaceAll);
        assert_eq!(*app.document().source(), source.replace('α', "β"));
    }

    #[test]
    fn raster_shortcuts_are_bounded_and_do_not_touch_the_other_controller() {
        let context = egui::Context::default();
        let key = ArtifactKey::unversioned(0);
        let mut asset = PreviewController::new(false, PreviewPreference::Native);
        let pinned = PreviewController::new(false, PreviewPreference::Native);
        asset.replace_asset(
            key,
            None,
            (0..2)
                .map(|i| {
                    make_preview_texture(
                        &context,
                        tiptoptyp_core::document::WindowSessionId::new(1),
                        key,
                        i,
                        PreviewPage {
                            size: [2, 2],
                            rgba: vec![255; 16],
                            links: vec![],
                        },
                        false,
                    )
                })
                .collect(),
        );
        for _ in 0..10 {
            raster_shortcut(&mut asset, A::PreviewNextPage);
        }
        assert_eq!(asset.requested_page, Some(1));
        for _ in 0..10 {
            raster_shortcut(&mut asset, A::PreviewPreviousPage);
        }
        assert_eq!(asset.requested_page, Some(0));
        raster_shortcut(&mut asset, A::PreviewFitWidth);
        assert!(!asset.fit_width);
        assert!(pinned.fit_width);
        assert!(pinned.requested_page.is_none());
    }
}
