//! Command-palette admission and execution reuse the ordinary command paths.
use super::*;

#[derive(Clone, Copy, Debug)]
pub(super) struct Origin {
    viewport: egui::ViewportId,
    focus: Option<egui::Id>,
    terminal: bool,
    preview: Option<preview_controls::Target>,
    tex_mode: bool,
}

const EXTRA: &[ShortcutAction] = &[
    ShortcutAction::Compile,
    ShortcutAction::ToggleCompilation,
    ShortcutAction::KeyboardShortcuts,
    ShortcutAction::NextTab,
    ShortcutAction::PreviousTab,
    ShortcutAction::ToggleTexMode,
    ShortcutAction::UseTabForPreview,
    ShortcutAction::ToggleFold,
    ShortcutAction::CollapseAll,
    ShortcutAction::ExpandAll,
    ShortcutAction::FindNext,
    ShortcutAction::FindPrevious,
    ShortcutAction::ReplaceOne,
    ShortcutAction::ReplaceAll,
    ShortcutAction::ToggleFindCase,
    ShortcutAction::ToggleFindRegex,
    ShortcutAction::ExplorerSearch,
    ShortcutAction::RefreshWorkspace,
    ShortcutAction::StatusHistory,
    ShortcutAction::ToggleLineWrap,
    ShortcutAction::ToggleLineNumbers,
    ShortcutAction::ToggleStickyContext,
    ShortcutAction::UiScaleIn,
    ShortcutAction::UiScaleOut,
    ShortcutAction::UiScaleReset,
    ShortcutAction::PreviewZoomIn,
    ShortcutAction::PreviewZoomOut,
    ShortcutAction::PreviewZoomReset,
    ShortcutAction::PreviewPreviousPage,
    ShortcutAction::PreviewNextPage,
    ShortcutAction::PreviewFitWidth,
    ShortcutAction::PreviousHunk,
    ShortcutAction::NextHunk,
    ShortcutAction::Minimize,
    ShortcutAction::ToggleFullscreen,
];

impl EditorApp {
    pub(super) fn command_palette_visible(&self) -> bool {
        matches!(self.app_popup, Some(AppPopup::CommandPalette { .. }))
    }

    pub(super) fn palette_holds_find(&self, context: &egui::Context) -> bool {
        self.command_palette_visible()
            && self.command_palette_origin.is_some_and(|origin| {
                origin.viewport == scoped_child_viewport_id(context, find_bar::VIEWPORT_SALT)
            })
            && focused_input_viewport(context)
                == scoped_child_viewport_id(context, "tiptoptyp-popup-overlay")
    }

    fn palette_origin(&self, context: &egui::Context) -> Origin {
        let mut viewport = focused_input_viewport(context);
        if viewport == scoped_child_viewport_id(context, "tiptoptyp-popup-overlay") {
            viewport = context.viewport_id();
        }
        Origin {
            viewport,
            focus: (viewport == context.viewport_id())
                .then(|| context.memory(|memory| memory.focused()))
                .flatten(),
            terminal: self.terminal_focused(context, viewport),
            preview: self.focused_preview_target(context),
            tex_mode: self.document().config().is_some(),
        }
    }

    fn palette_admitted(&self, context: &egui::Context) -> bool {
        let focused = focused_input_viewport(context);
        !self.document_flow_busy()
            && !self.process_close_pending
            && !self.typst_overrides_visible
            && !self.workspace_chooser_visible
            && (focused == context.viewport_id()
                || focused == scoped_child_viewport_id(context, "preview-window")
                || focused == scoped_child_viewport_id(context, find_bar::VIEWPORT_SALT)
                || focused == scoped_child_viewport_id(context, preview_controls::SALT)
                || focused
                    == crate::child_view::child_viewport_id(
                        scoped_child_viewport_id(context, "preview-window"),
                        preview_controls::SALT,
                    )
                || focused == scoped_child_viewport_id(context, "tiptoptyp-popup-overlay"))
    }

    pub(super) fn toggle_command_palette(&mut self, context: &egui::Context) {
        if self.command_palette_visible() {
            self.dismiss_command_palette(context, true);
            return;
        }
        if !self.palette_admitted(context) {
            return;
        }
        let mut origin = self.palette_origin(context);
        origin.tex_mode = self.tex_mode_available();
        let bounds = context.content_rect();
        let anchor = Pos2::new(
            ((bounds.width() - 600.0) / 2.0).max(8.0),
            bounds.top() + METRICS.chrome.toolbar_height + 16.0,
        );
        self.editor_completion = None;
        self.clear_editor_hover();
        self.clear_asset_hover();
        self.command_palette.open();
        self.command_palette_edit_events.clear();
        self.open_app_popup(AppPopup::CommandPalette { anchor });
        self.command_palette_origin = Some(origin);
        context.request_repaint();
    }

    pub(super) fn dismiss_command_palette(&mut self, context: &egui::Context, restore: bool) {
        let origin = self.command_palette_origin.take();
        self.close_app_popup();
        if restore && let Some(origin) = origin {
            self.restore_palette_origin(context, origin);
        }
    }

    fn restore_palette_origin_field(&self, context: &egui::Context, origin: Origin) {
        if origin.viewport == context.viewport_id()
            && let Some(focus) = origin.focus
        {
            context.memory_mut(|memory| memory.request_focus(focus));
        }
    }

    fn restore_palette_origin(&mut self, context: &egui::Context, origin: Origin) {
        self.restore_palette_origin_field(context, origin);
        crate::window_host::focus(
            context,
            origin.viewport,
            crate::window_host::FocusCause::ReturnFromChild,
        );
    }

    /// Admit before terminal/raw editor input, and keep all query keys in the
    /// child while open. Window-management commands still target its parent.
    pub(super) fn handle_palette_shortcuts(
        &mut self,
        context: &egui::Context,
        viewport: egui::ViewportId,
        shortcuts: &ShortcutBindings,
    ) -> bool {
        let toggle = context
            .input_mut_for(viewport, |input| {
                let index = input.events.iter().position(|event| {
                    let egui::Event::Key {
                        key,
                        modifiers,
                        pressed: true,
                        ..
                    } = event
                    else {
                        return false;
                    };
                    shortcuts.action_for_key_event(*key, *modifiers)
                        == Some(ShortcutAction::CommandPalette)
                })?;
                input.events.remove(index);
                Some(())
            })
            .is_some();
        if toggle {
            self.toggle_command_palette(context);
            return true;
        }
        if !self.command_palette_visible() {
            return false;
        }
        handle_window_management_shortcuts(context, viewport, shortcuts);
        if shortcuts
            .egui(ShortcutAction::CaptureUi)
            .is_some_and(|chord| {
                context.input_mut_for(viewport, |input| input.consume_shortcut(&chord))
            })
        {
            self.captures.queue_for_viewport(viewport);
        }
        true
    }

    pub(super) fn palette_native_command(
        &mut self,
        command: AppCommand,
        context: &egui::Context,
    ) -> bool {
        if !self.command_palette_visible() {
            return false;
        }
        let viewport = scoped_child_viewport_id(context, "tiptoptyp-popup-overlay");
        if matches!(command, AppCommand::CommandPalette | AppCommand::CloseTab) {
            self.dismiss_command_palette(context, true);
        } else {
            let event = match command {
                AppCommand::Cut => Some(egui::Event::Cut),
                AppCommand::Copy => Some(egui::Event::Copy),
                AppCommand::Paste => {
                    self.request_widget_paste(context, viewport);
                    None
                }
                _ => standard_text_edit_shortcut(command).map(|chord| egui::Event::Key {
                    key: chord.logical_key,
                    physical_key: None,
                    pressed: true,
                    repeat: false,
                    modifiers: chord.modifiers,
                }),
            };
            if let Some(event) = event {
                self.command_palette_edit_events.push(event);
                context.request_repaint_of(viewport);
            }
        }
        true
    }

    pub(super) fn command_palette_entries(
        &self,
        context: &egui::Context,
    ) -> Vec<command_palette::Entry> {
        let origin = self
            .command_palette_origin
            .unwrap_or_else(|| self.palette_origin(context));
        let bindings = self.settings.effective_shortcuts();
        crate::native_menu::COMMAND_SPECS
            .iter()
            .filter(|spec| spec.command != AppCommand::CommandPalette)
            .map(|spec| spec.shortcut_action)
            .chain(EXTRA.iter().copied())
            .map(|action| command_palette::Entry {
                action,
                label: if action == ShortcutAction::PreviewFitWidth {
                    "Fit preview to width"
                } else {
                    action.label()
                },
                category: action.group(),
                shortcut: bindings
                    .egui(action)
                    .map(|chord| context.format_shortcut(&chord)),
                enabled: self.palette_action_enabled(action, origin, context),
            })
            .collect()
    }

    fn palette_preview_target(&self, origin: Origin) -> Option<preview_controls::Target> {
        let target = origin
            .preview
            .or_else(|| self.main_preview_controls_target())
            .or_else(|| {
                self.preview_controls
                    .popout
                    .map(|_| self.preview_controls.popout_target)
            })?;
        match target {
            preview_controls::Target::Source if self.source_preview_available() => Some(target),
            preview_controls::Target::Asset(key)
                if self.pdfium_asset_requested() && key == self.document().key() =>
            {
                Some(target)
            }
            _ => None,
        }
    }

    fn palette_action_enabled(
        &self,
        action: ShortcutAction,
        origin: Origin,
        context: &egui::Context,
    ) -> bool {
        use ShortcutAction as A;
        if self.document_flow_busy() || self.process_close_pending {
            return false;
        }
        let has_source = !self.tabs.is_empty() && self.document().kind().is_editable();
        let source_focus = has_source
            && !origin.terminal
            && origin.preview.is_none()
            && origin.viewport == context.viewport_id()
            && origin
                .focus
                .is_none_or(|id| id == source_editor_id(context));
        if let Some(spec) = crate::native_menu::COMMAND_SPECS
            .iter()
            .find(|spec| spec.shortcut_action == action)
        {
            return match action {
                A::Undo => {
                    source_focus
                        && !self.overleaf_source_locked()
                        && self.editor_history_availability(context).0
                }
                A::Redo => {
                    source_focus
                        && !self.overleaf_source_locked()
                        && self.editor_history_availability(context).1
                }
                A::ToggleComment | A::Format | A::NewTable | A::EditTable => {
                    source_focus && self.native_command_enabled(spec.command)
                }
                A::Cut | A::Paste if source_focus && self.overleaf_source_locked() => false,
                A::Cut | A::Copy | A::Paste | A::SelectAll => {
                    origin.terminal
                        || source_focus
                        || origin.viewport
                            == scoped_child_viewport_id(context, find_bar::VIEWPORT_SALT)
                        || origin.viewport
                            == scoped_child_viewport_id(context, preview_controls::SALT)
                        || origin.viewport
                            == crate::child_view::child_viewport_id(
                                scoped_child_viewport_id(context, "preview-window"),
                                preview_controls::SALT,
                            )
                        || (origin.viewport == context.viewport_id()
                            && origin.preview.is_none()
                            && origin.focus.is_some())
                }
                A::Find | A::FindReplace if origin.terminal => true,
                A::FindReplace if origin.preview.is_some() => false,
                _ => self.native_command_enabled(spec.command),
            };
        }
        match action {
            A::Compile => self.source_preview_available(),
            A::ToggleCompilation => {
                self.source_preview_available()
                    && compilation_pause_button_visible(
                        self.preview_language_support().build,
                        self.interactive_preview_requested(),
                    )
            }
            A::KeyboardShortcuts
            | A::RefreshWorkspace
            | A::ExplorerSearch
            | A::StatusHistory
            | A::UiScaleIn
            | A::UiScaleOut
            | A::UiScaleReset
            | A::Minimize
            | A::ToggleFullscreen => true,
            A::NextTab | A::PreviousTab => self.tabs.len() > 1,
            A::ToggleTexMode => source_focus && !self.overleaf_source_locked() && origin.tex_mode,
            A::UseTabForPreview => {
                has_source && self.document().kind().typesetting_language().is_some()
            }
            A::ToggleFold | A::CollapseAll | A::ExpandAll => {
                source_focus && self.document().kind().is_typst() && self.settings.line_numbers
            }
            A::FindNext | A::FindPrevious => source_focus || origin.terminal,
            A::ReplaceOne | A::ReplaceAll => source_focus && !self.overleaf_source_locked(),
            A::ToggleFindCase | A::ToggleFindRegex => source_focus,
            A::ToggleLineWrap | A::ToggleLineNumbers => has_source,
            A::ToggleStickyContext => has_source && self.document().kind().is_typst(),
            A::PreviewZoomIn | A::PreviewZoomOut | A::PreviewZoomReset => {
                !self.tabs.is_empty() && self.preview_visible()
            }
            A::PreviewPreviousPage | A::PreviewNextPage | A::PreviewFitWidth => {
                self.preview_visible() && self.palette_preview_target(origin).is_some()
            }
            A::PreviousHunk | A::NextHunk => source_focus && !self.git_editor.hunks.is_empty(),
            _ => false,
        }
    }

    pub(super) fn execute_palette_action(
        &mut self,
        action: ShortcutAction,
        origin: Origin,
        context: &egui::Context,
        frame: Option<&eframe::Frame>,
    ) {
        if !self.palette_action_enabled(action, origin, context) {
            return;
        }
        let child = match action {
            ShortcutAction::LatexDiff => Some(latexdiff::SALT),
            ShortcutAction::TexDocumentation => Some(tex_documentation::SALT),
            ShortcutAction::ImportCitation => Some(citation::SALT),
            ShortcutAction::NewTable | ShortcutAction::EditTable => Some("tiptoptyp-table-editor"),
            ShortcutAction::NewFromTemplate => Some("tiptoptyp-templates"),
            _ => None,
        };
        if child.is_some() {
            // Preserve the source selection for the tool, but let its new child
            // activate. Returning native focus to the owner here races that
            // activation and leaves the modal's focused field without input.
            self.restore_palette_origin_field(context, origin);
        } else {
            self.restore_palette_origin(context, origin);
        }
        if let Some(target) = origin.preview
            && action == ShortcutAction::Find
        {
            self.preview_controls.select(target);
            self.preview_controls.open = true;
            self.preview_controls.focus_find = true;
            context.request_repaint();
            return;
        }
        // A View command must not be consumed by the text-field router's
        // suppression of source edits while Find or another field owns input.
        if action == ShortcutAction::ComfyPreview {
            self.execute_app_command(AppCommand::ComfyPreview, context, frame);
            return;
        }
        if let Some(spec) = crate::native_menu::COMMAND_SPECS
            .iter()
            .find(|spec| spec.shortcut_action == action)
        {
            if !self.route_edit_command_to_focused_widget(spec.command, context, origin.viewport) {
                self.execute_app_command(spec.command, context, frame);
            }
            if let Some(child) = child {
                let open = match child {
                    "tiptoptyp-table-editor" => self.table_editor.is_some(),
                    "tiptoptyp-templates" => self.template_picker.is_some(),
                    _ => self.tool_dialog_salt() == Some(child),
                };
                if open {
                    ChildViewHost::request_focus_on_show(context, child);
                } else {
                    // A refused command (for example New Table inside math)
                    // has no child to receive input; return to its source.
                    self.restore_palette_origin(context, origin);
                }
            }
            return;
        }
        use ShortcutAction as A;
        match action {
            A::Compile => self.compile_pdf(frame),
            A::ToggleCompilation => self.toggle_compilation_paused(),
            A::NextTab | A::PreviousTab => {
                let active = self.tabs.active_index().unwrap();
                let count = self.tabs.len();
                let index = if action == A::PreviousTab {
                    (active + count - 1) % count
                } else {
                    (active + 1) % count
                };
                self.activate_tab(self.tabs.id_at(index).unwrap(), context);
            }
            A::UiScaleIn => self.adjust_ui_scale(5, context),
            A::UiScaleOut => self.adjust_ui_scale(-5, context),
            A::PreviewZoomIn | A::PreviewZoomOut | A::PreviewZoomReset => {
                let zoom = match action {
                    A::PreviewZoomIn => PreviewZoomAction::In,
                    A::PreviewZoomOut => PreviewZoomAction::Out,
                    _ => PreviewZoomAction::Reset,
                };
                if let Some(target) = origin.preview {
                    self.preview_control_action_for(
                        target,
                        match zoom {
                            PreviewZoomAction::In => preview_controls::Action::ZoomIn,
                            PreviewZoomAction::Out => preview_controls::Action::ZoomOut,
                            PreviewZoomAction::Reset => preview_controls::Action::Fit,
                        },
                    );
                } else if !self.route_preview_zoom(context, zoom) {
                    let preview = if self.document().kind().preview_only() {
                        &mut self.asset_preview
                    } else {
                        &mut self.preview
                    };
                    image_view::request_zoom(preview, zoom);
                }
            }
            A::PreviewPreviousPage | A::PreviewNextPage | A::PreviewFitWidth => {
                let Some(target) = self.palette_preview_target(origin) else {
                    return;
                };
                let snapshot = match target {
                    preview_controls::Target::Source if self.interactive_preview_active() => {
                        self.preview_controls.web.clone()
                    }
                    preview_controls::Target::Source => self.pdfium_preview.controls_snapshot(),
                    preview_controls::Target::Asset(_) => self.pdfium_asset.controls_snapshot(),
                };
                if let Some(action) = preview_page_action(action, &snapshot) {
                    self.preview_control_action_for(target, action);
                }
            }
            A::PreviousHunk | A::NextHunk => {
                if self.document_diff.is_some() {
                    self.navigate_document_diff(context, action == A::PreviousHunk);
                } else {
                    self.navigate_hunk(
                        context,
                        action == A::PreviousHunk,
                        git_actions::HunkNavigation::Editor,
                    );
                }
            }
            A::Minimize => context.send_viewport_cmd_to(
                crate::window_host::management_target(context, origin.viewport),
                egui::ViewportCommand::Minimized(true),
            ),
            A::ToggleFullscreen => {
                let target = crate::window_host::management_target(context, origin.viewport);
                let fullscreen = context
                    .input_for(target, |input| input.viewport().fullscreen)
                    .unwrap_or(false);
                context
                    .send_viewport_cmd_to(target, egui::ViewportCommand::Fullscreen(!fullscreen));
            }
            _ => self.execute_extra_shortcut(action, context),
        }
        context.request_repaint();
    }
}

fn preview_page_action(
    action: ShortcutAction,
    snapshot: &preview_controls::Snapshot,
) -> Option<preview_controls::Action> {
    use preview_controls::Action;
    match action {
        ShortcutAction::PreviewPreviousPage => Some(Action::Page(snapshot.page.saturating_sub(1))),
        ShortcutAction::PreviewNextPage => Some(Action::Page(
            snapshot
                .page
                .saturating_add(1)
                .min(snapshot.count.saturating_sub(1)),
        )),
        ShortcutAction::PreviewFitWidth => Some(Action::Fit),
        _ => None,
    }
}

#[cfg(test)]
#[path = "palette_actions_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "palette_target_tests.rs"]
mod target_tests;
