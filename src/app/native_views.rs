//! Owned child-window views and native preview composition.
use super::*;

#[cfg(any(target_os = "macos", target_os = "windows"))]
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct WebviewStyle {
    pub(super) palette: (Color32, Color32),
    accent: Color32,
    hover: Stroke,
    focus: Stroke,
    zoom_snapshot: bool,
    reveal_animation: crate::settings::RevealAnimation,
}

#[cfg(any(target_os = "macos", target_os = "windows"))]
impl WebviewStyle {
    fn script(self) -> String {
        let css = |stroke: Stroke| {
            format!(
                "{}px solid rgba({},{},{},{})",
                stroke.width,
                stroke.color.r(),
                stroke.color.g(),
                stroke.color.b(),
                f32::from(stroke.color.a()) / 255.0
            )
        };
        format!(
            "window.tiptoptypSetPalette?.({}, {}, {}, {}, {}); window.tiptoptypZoomSnapshot?.configure({}); window.tiptoptypSetRevealOptions?.({});",
            serde_json::json!([self.palette.0.r(), self.palette.0.g(), self.palette.0.b()]),
            serde_json::json!([self.palette.1.r(), self.palette.1.g(), self.palette.1.b()]),
            serde_json::json!(css(self.hover)),
            serde_json::json!(css(self.focus)),
            serde_json::json!([self.accent.r(), self.accent.g(), self.accent.b()]),
            self.zoom_snapshot,
            serde_json::json!(self.reveal_animation),
        )
    }
}

#[cfg(any(target_os = "macos", target_os = "windows"))]
impl EditorApp {
    pub(super) fn preview_webview_style(&self, context: &egui::Context) -> WebviewStyle {
        let style = context.style_of(context.theme());
        let visuals = &style.visuals;
        WebviewStyle {
            palette: self.preview_palette(context, self.preview.dark),
            accent: theme::palette(context).accent,
            hover: visuals.widgets.hovered.bg_stroke,
            focus: visuals.selection.stroke,
            zoom_snapshot: self.settings.preview_zoom_snapshot && cfg!(target_os = "macos"),
            reveal_animation: self.settings.preview_reveal_animation.normalized(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct WebviewAppliedState {
    bounds: NativeRect,
    background: Color32,
    pub(super) visible: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct WebviewPropertyDiff {
    bounds: bool,
    background: bool,
    visible: bool,
}

fn webview_property_diff(
    previous: Option<WebviewAppliedState>,
    next: WebviewAppliedState,
) -> WebviewPropertyDiff {
    WebviewPropertyDiff {
        bounds: previous.is_none_or(|previous| previous.bounds != next.bounds),
        background: previous.is_none_or(|previous| previous.background != next.background),
        visible: previous.is_none_or(|previous| previous.visible != next.visible),
    }
}

fn table_window_size(scene: Option<UiSnapshotScene>) -> [f32; 2] {
    match scene {
        Some(UiSnapshotScene::TableEditorNarrow | UiSnapshotScene::TableEditorTexNarrow) => {
            [620.0, 600.0]
        }
        Some(UiSnapshotScene::TableEditor | UiSnapshotScene::TableEditorTex) => [940.0, 700.0],
        _ => [860.0, 560.0],
    }
}

impl EditorApp {
    pub(super) fn open_web_preview_context_menu(
        &mut self,
        context: &egui::Context,
        request: preview_jump::WebMenuRequest,
    ) {
        if self.document_flow_busy()
            || self.process_close_pending
            || self.tinymist_sync.generation != Some(request.generation)
            || !self.interactive_preview_active()
        {
            return;
        }
        #[cfg(any(target_os = "macos", target_os = "windows"))]
        if let Some(applied) = self.webview_applied.filter(|applied| applied.visible)
            && let Some(window) = self.webview_parent
            && let Some(anchor) =
                preview_jump::web_menu_anchor(applied.bounds, request.point, context.zoom_factor())
        {
            self.open_app_popup(AppPopup::PreviewSource {
                anchor,
                window,
                target: preview_jump::WebSourceLocation {
                    generation: request.generation,
                    token: request.token,
                    path: self.preview_document_path(),
                    url: self.webview_url.clone().unwrap_or_default(),
                    window,
                },
            });
            context.request_repaint();
        }
    }

    pub(super) fn reveal_web_preview_source(&mut self, target: preview_jump::WebSourceLocation) {
        let trace = std::env::var_os("TIPTOPTYP_UI_TRACE").is_some();
        let current = self.interactive_preview_active()
            && self.tinymist_sync.generation == Some(target.generation)
            && self.preview_document_path() == target.path;
        if trace {
            eprintln!(
                "ui.preview.source-menu token={} current={current}",
                target.token
            );
        }
        if !current {
            return;
        }
        #[cfg(any(target_os = "macos", target_os = "windows"))]
        if self.webview_parent == Some(target.window)
            && self.webview_url.as_ref() == Some(&target.url)
            && let Some(webview) = self.webview.as_ref()
        {
            let script = format!("window.tiptoptypRevealSource?.({});", target.token);
            if trace {
                let _ = webview.evaluate_script_with_callback(&script, |result| {
                    eprintln!("ui.preview.source-menu dispatched={result}");
                });
            } else {
                let _ = webview.evaluate_script(&script);
            }
        } else if trace {
            eprintln!("ui.preview.source-menu frontend-retired");
        }
    }

    /// Native objects stay on the owning UI thread; discard their cached
    /// presentation identity together so a recreated view cannot inherit it.
    pub(super) fn discard_webview(&mut self) {
        self.pending_web_preview_menu = None;
        if matches!(self.app_popup, Some(AppPopup::PreviewSource { .. })) {
            self.close_app_popup();
        }
        if matches!(
            self.pending_app_popup_action,
            Some(AppPopupAction::RevealPreviewSource(_))
        ) {
            self.pending_app_popup_action = None;
        }
        #[cfg(any(target_os = "macos", target_os = "windows"))]
        self.release_webview_focus();
        #[cfg(target_os = "macos")]
        self.webview_snapshot.invalidate();
        #[cfg(all(feature = "desktop-ui-tests", target_os = "macos"))]
        crate::desktop_test::reset_renderer();
        #[cfg(any(target_os = "macos", target_os = "windows"))]
        {
            self.webview = None;
            self.webview_applied = None;
            self.webview_url = None;
            self.webview_parent = None;
            self.webview_navigation = None;
            self.webview_reload_pending = false;
            self.webview_style = None;
        }
    }

    pub(super) fn show_package_manager_window(&mut self, context: &egui::Context) {
        if !self.packages_visible
            || self.document_workflow.modal().is_some()
            || self.rename_dialog.is_some()
        {
            return;
        }
        let active_theme = context.theme();
        let style = context.style_of(active_theme);
        let captures = self.captures.clone();
        let catalog = self.package_catalog.as_ref();
        let loading = self.package_catalog_job.is_running() || self.package_uninstall.is_running();
        let mut close_requested = false;
        let mut refresh_requested = false;
        let mut browser_action = PackageBrowserAction::default();
        let spec = ChildViewSpec::persistent(
            "tiptoptyp-packages",
            "tiptoptyp Packages",
            [760.0, 620.0],
            [520.0, 360.0],
            "packages",
        );
        ChildViewHost::show(
            context,
            &captures,
            spec,
            active_theme,
            &style,
            |ui, input| {
                close_requested |= input.close_requested;
                if ui.ctx().input(|input| input.viewport().fullscreen) == Some(true) {
                    ui.ctx()
                        .send_viewport_cmd(egui::ViewportCommand::Fullscreen(false));
                }
                ui.painter()
                    .rect_filled(ui.max_rect(), 0.0, ui.visuals().panel_fill);
                egui::Panel::top("packages-titlebar")
                    .exact_size(METRICS.chrome.toolbar_height)
                    .frame(theme::settings_title_frame(ui.style()))
                    .show(ui, |ui| {
                        ui.horizontal_centered(|ui| {
                            #[cfg(target_os = "macos")]
                            theme::reserve_window_controls(ui);
                            crate::window_logo::show(ui, &captures);
                            ui.label(RichText::new("Typst packages").strong());
                            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                refresh_requested |= crate::app::icons::action_button_enabled(
                                    ui, !loading, "Refresh",
                                )
                                .clicked();
                                #[cfg(not(target_os = "macos"))]
                                if icon_button(ui, UiIcon::Close, "Close Packages").clicked() {
                                    close_requested = true;
                                }
                            });
                        });
                    });
                egui::CentralPanel::default()
                    .frame(theme::settings_content_frame(ui.style()))
                    .show(ui, |ui| {
                        show_package_browser_ui(
                            ui,
                            &mut self.package_query,
                            &mut self.package_filter,
                            catalog,
                            loading,
                            &mut browser_action,
                        );
                    });
            },
        );
        if close_requested {
            self.packages_visible = false;
            crate::window_host::return_focus(context);
        }
        if refresh_requested {
            self.request_package_catalog(context);
        }
        if let Some(import) = browser_action.copied {
            context.copy_text(import);
            self.notice = Some(Notice {
                message: "Copied package import".to_owned(),
                kind: NoticeKind::Success,
            });
        }
        if let Some(target) = browser_action.open_link {
            self.follow_preview_link(&target);
        }
        if let Some(installation) = browser_action.uninstall {
            self.document_workflow.set_modal(AppModal::UninstallPackage {
                message: format!(
                    "Permanently uninstall this package version?\n{}\nLocal changes in this directory will be removed.",
                    installation.package_path.display()
                ),
                installation,
            });
        }
    }

    pub(super) fn show_table_editor_window(&mut self, context: &egui::Context) {
        if self.table_editor.is_none() || self.document_workflow.modal().is_some() {
            return;
        }
        self.prepare_table_samples(context);
        let theme = context.theme();
        let style = context.style_of(theme);
        let shortcuts = self.settings.effective_shortcuts();
        let child = scoped_child_viewport_id(context, "tiptoptyp-table-editor");
        let allow_requested_paste = self
            .pending_widget_paste
            .is_some_and(|pending| pending.admits(child, context.cumulative_frame_nr()));
        let mut paste_requested = false;
        let mut requested_paste_admitted = false;
        let Some(dialog) = &mut self.table_editor else {
            return;
        };
        let mut requested_action = None;
        let captures = self.captures.clone();
        let size = table_window_size(self.snapshot_scene);
        let math = dialog.is_math();
        let spec = ChildViewSpec::persistent(
            "tiptoptyp-table-editor",
            if math {
                "tiptoptyp Math editor"
            } else {
                "tiptoptyp Table editor"
            },
            size,
            [620.0, 420.0],
            "table-editor",
        );
        ChildViewHost::show(context, &captures, spec, theme, &style, |ui, input| {
            let physical = table_workbench::prepare_child_keyboard(
                ui,
                dialog,
                &shortcuts,
                allow_requested_paste,
            );
            paste_requested |= physical.paste_requested;
            requested_paste_admitted |= physical.requested_paste_admitted;
            let _queued =
                crate::child_view::begin_queued_edit_events(ui.ctx(), &mut self.table_edit_events);
            if input.close_requested || input.escape_pressed {
                requested_action = Some(TableEditorUiAction::Cancel);
            }
            ui.painter()
                .rect_filled(ui.max_rect(), 0.0, ui.visuals().panel_fill);
            egui::Panel::top("table-editor-titlebar")
                .exact_size(METRICS.chrome.toolbar_height)
                .frame(theme::settings_title_frame(ui.style()))
                .show(ui, |ui| {
                    ui.horizontal_centered(|ui| {
                        #[cfg(target_os = "macos")]
                        theme::reserve_window_controls(ui);
                        crate::window_logo::show(ui, &captures);
                        ui.strong(if math {
                            "Math workbench"
                        } else {
                            "Table workbench"
                        });
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            ui.weak("Unsaved draft");
                        });
                    });
                });
            egui::CentralPanel::default()
                .frame(theme::settings_content_frame(ui.style()))
                .show(ui, |ui| {
                    let width = ui.available_width();
                    let height = ui.available_height();
                    if let Some(action) =
                        table_workbench::show(ui, dialog, width, height, &self.settings)
                    {
                        requested_action = Some(action);
                    }
                });
        });

        if requested_paste_admitted {
            self.pending_widget_paste = None;
        }
        if paste_requested && requested_action.is_none() {
            self.request_widget_paste(context, child);
        }
        match requested_action {
            Some(TableEditorUiAction::Cancel) => self.cancel_table_editor(context),
            Some(TableEditorUiAction::Apply) => {
                if let Err(error) = self.validate_table_sample_locks() {
                    self.table_editor.as_mut().unwrap().set_error(error);
                    return;
                }
                let mut dialog = self.table_editor.take().expect("table editor exists");
                match table_workbench::prepare_source_edit(
                    self.document().source(),
                    self.document().key(),
                    &dialog,
                ) {
                    Ok(edit) => {
                        let snapshot = self.editor_snapshot(context);
                        self.document_mut().edit(snapshot.cursor, |source| {
                            source.replace_range(edit.byte_range, &edit.replacement)
                        });
                        self.pending_editor_selection =
                            Some(EditorSelection::Focus(edit.cursor..edit.cursor));
                        self.find_bar.search.clear();
                        self.mark_edited();
                        self.notice = Some(Notice {
                            message: "Updated table".to_owned(),
                            kind: NoticeKind::Success,
                        });
                        self.table_edit_events.clear();
                        self.table_samples.reset();
                        crate::table_sampling::clear(context, child);
                        ChildViewHost::close(context, "tiptoptyp-table-editor");
                        crate::window_host::return_focus(context);
                    }
                    Err(error) => {
                        dialog.set_error(error);
                        self.table_editor = Some(dialog);
                    }
                }
            }
            None => {}
        }
    }

    pub(super) fn show_rename_dialog(&mut self, context: &egui::Context) {
        if self.rename_dialog.is_none()
            || self.document_workflow.modal().is_some()
            || self.rename_overlay_suspended
        {
            return;
        }
        let Some(window_rect) = context.input(|input| input.viewport().inner_rect) else {
            return;
        };
        let dialog_width = (window_rect.width() - METRICS.popup.rename_window_inset)
            .clamp(1.0, METRICS.popup.rename_max_width);
        let theme = context.theme();
        let style = context.style_of(theme);
        let Some(dialog) = &mut self.rename_dialog else {
            return;
        };
        let mut submit = false;
        let mut cancel = false;
        let mut overlay_had_focus = self.rename_overlay_had_focus;
        let mut suspend_overlay = false;
        let captures = self.captures.clone();
        let spec =
            ChildViewSpec::modal("tiptoptyp-rename-overlay", "Rename", window_rect, "rename");
        ChildViewHost::show(context, &captures, spec, theme, &style, |ui, input| {
            cancel |= {
                if input.focused == Some(true) {
                    overlay_had_focus = true;
                }
                suspend_overlay |= overlay_had_focus && input.focused == Some(false);
                input.close_requested || input.escape_pressed
            };
            if suspend_overlay {
                ui.ctx()
                    .send_viewport_cmd(egui::ViewportCommand::Visible(false));
            }
            egui::Area::new(viewport_scoped_id(ui.ctx(), "rename-document-dialog"))
                .anchor(egui::Align2::CENTER_CENTER, Vec2::ZERO)
                .show(ui.ctx(), |ui| {
                    theme::dialog_card_frame(&style).show(ui, |ui| {
                        theme::apply_compact_tool_spacing(ui);
                        ui.set_min_width(dialog_width);
                        ui.set_max_width(dialog_width);
                        ui.label(RichText::new("Rename").strong());
                        let response = ui.add_sized(
                            [dialog_width, METRICS.popup.rename_input_height],
                            egui::TextEdit::singleline(&mut dialog.name).id_salt("rename-name"),
                        );
                        if dialog.focus {
                            response.request_focus();
                            dialog.focus = false;
                        }
                        submit |= response.lost_focus()
                            && ui.input(|input| input.key_pressed(egui::Key::Enter));
                        match dialog_footer(ui, "Rename", None, true, false) {
                            Some(AppModalChoice::Primary) => submit = true,
                            Some(AppModalChoice::Cancel) => cancel = true,
                            _ => {}
                        }
                    });
                });
        });
        self.rename_overlay_had_focus = overlay_had_focus;
        if suspend_overlay {
            self.rename_overlay_suspended = true;
        }
        if cancel {
            self.rename_dialog = None;
            self.rename_overlay_had_focus = false;
            self.rename_overlay_suspended = false;
            crate::window_host::return_focus(context);
        } else if submit {
            let dialog = self.rename_dialog.take().expect("rename dialog exists");
            self.rename_overlay_had_focus = false;
            self.rename_overlay_suspended = false;
            crate::window_host::return_focus(context);
            self.commit_rename(dialog.path, dialog.name, context);
        }
    }

    pub(super) fn show_workspace_chooser(&mut self, context: &egui::Context) {
        if !self.workspace_chooser_visible || self.document_workflow.modal().is_some() {
            return;
        }
        let Some(window_rect) = context.input(|input| input.viewport().inner_rect) else {
            return;
        };
        let style = context.style_of(context.theme());
        let native_theme = theme::native_theme(context.theme());
        let card_width = (window_rect.width() - METRICS.popup.modal_window_inset)
            .clamp(1.0, METRICS.popup.modal_max_width);
        let recent_workspaces = if self.snapshot_scene == Some(UiSnapshotScene::WorkspaceChooser) {
            vec![
                PathBuf::from("Recent/Annual report"),
                PathBuf::from("Recent/Research notes"),
            ]
        } else {
            self.settings.existing_recent_workspaces()
        };
        let mut selected = None;
        let mut removed = None;
        let mut choose_folder = false;
        let mut cancel = false;
        let mut dismiss = false;
        let captures = self.captures.clone();

        crate::viewport_fonts::show_immediate(
            context,
            scoped_child_viewport_id(context, "tiptoptyp-workspace-chooser"),
            theme::popup_viewport_builder("Change workspace root")
                .with_position(window_rect.min)
                .with_inner_size(window_rect.size())
                .with_min_inner_size(window_rect.size())
                .with_max_inner_size(window_rect.size())
                .with_active(true),
            |ui, _class| {
                captures.begin_viewport(ui.ctx(), "workspace");
                ui.set_style(style.clone());
                ui.ctx()
                    .send_viewport_cmd(egui::ViewportCommand::SetTheme(native_theme));
                dismiss |= ui.ctx().input(|input| {
                    input.viewport().close_requested() || input.key_pressed(egui::Key::Escape)
                });
                egui::Area::new(viewport_scoped_id(ui.ctx(), "workspace-chooser-card"))
                    .order(egui::Order::Foreground)
                    .anchor(egui::Align2::CENTER_CENTER, Vec2::ZERO)
                    .show(ui.ctx(), |ui| {
                        theme::dialog_card_frame(&style).show(ui, |ui| {
                            theme::apply_compact_tool_spacing(ui);
                            ui.set_width(card_width);
                            let content_top = ui.next_widget_position().y;
                            ui.label(RichText::new("Change workspace root").strong());
                            if !recent_workspaces.is_empty() {
                                ui.add_space(theme::SPACE.small);
                                ui.label(
                                    RichText::new("Recent")
                                        .size(theme::TYPE.supporting)
                                        .strong(),
                                );
                                let used = ui.next_widget_position().y - content_top;
                                let footer = dialog_footer_height(
                                    ui,
                                    card_width,
                                    "Choose Folder…",
                                    None,
                                    true,
                                );
                                let reserved = used
                                    + footer
                                    + theme::SPACE.control
                                    + theme::dialog_card_frame(&style).total_margin().sum().y
                                    + theme::SPACE.content * 2.0;
                                egui::ScrollArea::vertical()
                                    .id_salt("recent-workspaces")
                                    .min_scrolled_height(0.0)
                                    .max_height(dialog_scroll_height(
                                        window_rect.height(),
                                        reserved,
                                    ))
                                    .show(ui, |ui| {
                                        for path in &recent_workspaces {
                                            match show_recent_workspace_row(ui, path, card_width) {
                                                Some(RecentWorkspaceAction::Open(path)) => {
                                                    selected = Some(path);
                                                }
                                                Some(RecentWorkspaceAction::Remove(path)) => {
                                                    removed = Some(path);
                                                }
                                                None => {}
                                            }
                                        }
                                    });
                            }
                            ui.add_space(theme::SPACE.control);
                            match dialog_footer(ui, "Choose Folder…", None, true, false) {
                                Some(AppModalChoice::Primary) => choose_folder = true,
                                Some(AppModalChoice::Cancel) => cancel = true,
                                _ => {}
                            }
                        });
                    });
                captures.end_glow_viewport(ui, "workspace");
            },
        );

        if let Some(path) = removed {
            self.forget_workspace(&path);
            context.request_repaint();
        } else if let Some(path) = selected {
            self.workspace_chooser_visible = false;
            crate::window_host::return_focus(context);
            self.queue_open_path(path);
        } else if choose_folder {
            self.workspace_chooser_visible = false;
            crate::window_host::return_focus(context);
            self.open_folder_dialog();
        } else if cancel || dismiss {
            self.workspace_chooser_visible = false;
            crate::window_host::return_focus(context);
        }
    }

    pub(super) fn show_app_modal_window(&mut self, context: &egui::Context) {
        if self.document_workflow.modal_suspended {
            return;
        }
        let Some(window_rect) = context.input(|input| input.viewport().inner_rect) else {
            return;
        };
        let Some(modal) = self.document_workflow.modal() else {
            return;
        };
        let theme = context.theme();
        let style = context.style_of(theme);
        let native_theme = theme::native_theme(theme);
        let card_width = (window_rect.width() - METRICS.popup.modal_window_inset)
            .clamp(1.0, METRICS.popup.modal_max_width);

        let mut choice = None;
        let mut overlay_had_focus = self.document_workflow.modal_had_focus;
        let mut suspend_overlay = false;
        let captures = self.captures.clone();

        crate::viewport_fonts::show_immediate(
            context,
            scoped_child_viewport_id(context, "tiptoptyp-modal-overlay"),
            theme::popup_viewport_builder("tiptoptyp")
                .with_position(window_rect.min)
                .with_inner_size(window_rect.size())
                .with_min_inner_size(window_rect.size())
                .with_max_inner_size(window_rect.size())
                .with_active(true),
            |ui, _class| {
                captures.begin_viewport(ui.ctx(), "modal");
                ui.set_style(style.clone());
                ui.ctx()
                    .send_viewport_cmd(egui::ViewportCommand::SetTheme(native_theme));
                if ui.ctx().input(|input| {
                    if input.viewport().focused == Some(true) {
                        overlay_had_focus = true;
                    }
                    suspend_overlay |= overlay_had_focus && input.viewport().focused == Some(false);
                    input.viewport().close_requested() || input.key_pressed(egui::Key::Escape)
                }) {
                    choice = Some(AppModalChoice::Cancel);
                }
                if suspend_overlay {
                    ui.ctx()
                        .send_viewport_cmd(egui::ViewportCommand::Visible(false));
                }
                egui::Area::new(viewport_scoped_id(ui.ctx(), "app-modal-card"))
                    .order(egui::Order::Foreground)
                    .anchor(egui::Align2::CENTER_CENTER, Vec2::ZERO)
                    .show(ui.ctx(), |ui| {
                        theme::dialog_card_frame(&style).show(ui, |ui| {
                            theme::apply_compact_tool_spacing(ui);
                            ui.set_max_width(card_width);
                            ui.set_min_width(card_width);
                            let content_top = ui.next_widget_position().y;
                            let (title, message, color) = match &modal {
                                AppModal::Alert {
                                    title,
                                    message,
                                    kind,
                                } => (
                                    title.as_str(),
                                    message.as_str(),
                                    notice_color(*kind, ui.ctx()),
                                ),
                                AppModal::Unsaved { message, .. } => {
                                    ("Save changes?", message.as_str(), warning_color(ui.ctx()))
                                }
                                AppModal::UninstallPackage { message, .. } => (
                                    "Uninstall package?",
                                    message.as_str(),
                                    warning_color(ui.ctx()),
                                ),
                                AppModal::RevertGit { message, .. } => {
                                    ("Revert changes?", message.as_str(), warning_color(ui.ctx()))
                                }
                                AppModal::DeleteFile { message, .. } => {
                                    ("Delete file?", message.as_str(), warning_color(ui.ctx()))
                                }
                                AppModal::Overwrite { message, .. } => (
                                    "Replace existing file?",
                                    message.as_str(),
                                    warning_color(ui.ctx()),
                                ),
                            };
                            ui.label(RichText::new(title).strong().color(color));
                            let (primary, secondary, cancel) = match &modal {
                                AppModal::Alert { .. } => ("OK", None, false),
                                AppModal::Unsaved { .. } => ("Save", Some("Discard"), true),
                                AppModal::UninstallPackage { .. } => ("Uninstall", None, true),
                                AppModal::DeleteFile { .. } => ("Delete", None, true),
                                AppModal::RevertGit { .. } => ("Revert", None, true),
                                AppModal::Overwrite { .. } => ("Overwrite", None, true),
                            };
                            let used = ui.next_widget_position().y - content_top;
                            let footer =
                                dialog_footer_height(ui, card_width, primary, secondary, cancel);
                            let reserved = used
                                + footer
                                + theme::SPACE.control
                                + theme::dialog_card_frame(&style).total_margin().sum().y
                                + theme::SPACE.content * 2.0;
                            egui::ScrollArea::vertical()
                                .min_scrolled_height(0.0)
                                .max_height(dialog_scroll_height(window_rect.height(), reserved))
                                .show(ui, |ui| {
                                    ui.label(message);
                                });
                            ui.add_space(theme::SPACE.control);
                            if let Some(clicked) = dialog_footer(
                                ui,
                                primary,
                                secondary,
                                cancel,
                                matches!(modal, AppModal::Unsaved { .. }),
                            ) {
                                choice = Some(clicked);
                            }
                        });
                    });
                captures.end_glow_viewport(ui, "modal");
            },
        );
        self.document_workflow.modal_had_focus = overlay_had_focus;
        if suspend_overlay {
            self.document_workflow.modal_suspended = true;
        }

        let Some(choice) = choice else {
            return;
        };
        let modal = self
            .document_workflow
            .take_modal()
            .expect("modal exists while handling its choice");
        self.document_workflow.clear_modal();
        crate::window_host::return_focus(context);
        match (modal, choice) {
            (AppModal::Alert { .. }, _) => {}
            (AppModal::RevertGit { request, .. }, AppModalChoice::Primary) => {
                let dirty = self.tabs.has_unsaved_changes();
                self.git
                    .confirm_revert(context, request, &self.workspace_root, dirty);
            }
            (AppModal::RevertGit { .. }, _) => {}
            (AppModal::UninstallPackage { installation, .. }, AppModalChoice::Primary) => {
                self.package_catalog_job.supersede();
                if let Err(error) = self.package_uninstall.start_and_repaint(
                    "package-uninstall",
                    context,
                    move || {
                        crate::package_catalog::uninstall(&installation)?;
                        Ok(format!(
                            "Uninstalled {}",
                            installation.package_path.display()
                        ))
                    },
                ) {
                    self.show_file_error(error);
                }
            }
            (AppModal::UninstallPackage { .. }, _) => {}
            (AppModal::DeleteFile { path, .. }, AppModalChoice::Primary) => {
                let deletes_preview = self.designated_preview_path().as_ref() == Some(&path);
                match crate::workspace::WorkspaceRoot::open(&self.workspace_root)
                    .and_then(|root| root.file(&path))
                    .and_then(|file| file.delete())
                {
                    Ok(()) => {
                        if deletes_preview {
                            self.restart_tinymist_preserving_preview();
                            self.schedule_compile_now();
                        }
                        if self.document().path().as_ref() == Some(&path) {
                            self.reset_untitled_document();
                        }
                        self.refresh_workspace();
                        self.notice = Some(Notice {
                            message: format!("Deleted {}", path.display()),
                            kind: NoticeKind::Success,
                        });
                    }
                    Err(error) => self
                        .show_file_error(format!("Could not delete {}: {error}", path.display())),
                }
            }
            (AppModal::DeleteFile { .. }, _) => self.schedule_autosave_if_needed(),
            (AppModal::Unsaved { pending, .. }, AppModalChoice::Primary) => {
                self.document_workflow.queue_action(PendingDocumentAction {
                    action: DeferredDocumentAction::SaveThen(Box::new(pending)),
                    key: self.document().key(),
                    allow_discard: true,
                    description: "saving the current document".to_owned(),
                });
            }
            (AppModal::Unsaved { mut pending, .. }, AppModalChoice::Secondary) => {
                self.document_workflow.cancel_continuation();
                pending.key = self.document().key();
                pending.allow_discard = true;
                self.document_workflow.queue_action(pending);
            }
            (AppModal::Unsaved { .. }, AppModalChoice::Cancel) => {
                self.document_workflow.cancel_continuation();
                self.schedule_autosave_if_needed();
            }
            (
                AppModal::Overwrite {
                    path,
                    key,
                    expected_disk_fingerprint,
                    observed_disk_fingerprint,
                    ..
                },
                AppModalChoice::Primary,
            ) => {
                self.document_workflow.queue_action(PendingDocumentAction {
                    action: DeferredDocumentAction::ForceSave {
                        path,
                        key,
                        expected_disk_fingerprint,
                        observed_disk_fingerprint,
                    },
                    key: self.document().key(),
                    allow_discard: true,
                    description: "overwriting the current file".to_owned(),
                });
            }
            (AppModal::Overwrite { .. }, _) => {
                self.document_workflow.cancel_continuation();
                self.schedule_autosave_if_needed();
            }
        }
        self.document_workflow.finish_dispatch();
        context.request_repaint();
    }

    pub(super) fn show_asset_hover_window(&mut self, context: &egui::Context) {
        if self.snapshot_scene == Some(UiSnapshotScene::AssetPreview) && self.asset_hover.is_none()
        {
            let rgba =
                image::load_from_memory(include_bytes!("../../assets/icons/tiptoptyp-256.png"))
                    .expect("QA preview image")
                    .into_rgba8();
            let size = [rgba.width() as usize, rgba.height() as usize];
            let texture = context.load_texture(
                "qa-asset-preview",
                ColorImage::from_rgba_unmultiplied(size, &rgba),
                TextureOptions::LINEAR,
            );
            self.asset_hover = Some(AssetHoverState {
                origin: Rect::from_min_size(Pos2::new(80.0, 140.0), Vec2::new(130.0, 24.0)),
                anchor: Pos2::new(220.0, 140.0),
                placement: TooltipPlacement::Right,
                path: PathBuf::from("sample.png"),
                kind: DocumentKind::Image,
                opacity: 1.0,
                token: Default::default(),
                content: AssetHoverContent::Ready {
                    texture,
                    source_size: size,
                },
            });
        }
        let Some(identity) = self
            .asset_hover
            .as_ref()
            .map(|hover| asset_tooltip_identity(hover.origin, &hover.path))
        else {
            return;
        };
        let interaction_id = tooltip_interaction_id(context);
        let geometry_id = tooltip_geometry_id(context);
        let interaction = context.data(|data| {
            data.get_temp::<TooltipInteractionState>(interaction_id)
                .filter(|state| state.identity == identity)
                .unwrap_or(TooltipInteractionState::new(identity))
        });
        if interaction.dismissed {
            self.clear_asset_hover();
            context.data_mut(|data| {
                data.remove::<TooltipGeometry>(geometry_id);
                data.remove::<TooltipInteractionState>(interaction_id);
            });
            return;
        }
        let root_focused = context.input(|input| {
            input.viewport().focused == Some(true) && input.viewport().visible() != Some(false)
        });
        let geometry = context.data(|data| data.get_temp::<TooltipGeometry>(geometry_id));
        let handoff_active = native_tooltip_handoff_active(context, false);
        let blocked_by_overlay = self.settings_visible
            || self.packages_visible
            || self.rename_dialog.is_some()
            || self.table_editor.is_some()
            || self.latexdiff.is_some()
            || matches!(self.app_popup, Some(ref popup) if !matches!(popup, AppPopup::GitChunk { .. }))
            || self.document_workflow.modal().is_some();
        if blocked_by_overlay {
            self.clear_asset_hover();
            context.data_mut(|data| {
                if data
                    .get_temp::<TooltipGeometry>(geometry_id)
                    .is_some_and(|geometry| geometry.identity == identity)
                {
                    data.remove::<TooltipGeometry>(geometry_id);
                    data.remove::<TooltipInteractionState>(interaction_id);
                }
            });
            return;
        }
        if !tooltip_viewport_should_render(
            self.snapshot_scene == Some(UiSnapshotScene::AssetPreview),
            root_focused,
            handoff_active,
            identity,
            geometry,
            Some(interaction),
        ) {
            return;
        }
        let Some(window_rect) = context.input(|input| input.viewport().inner_rect) else {
            return;
        };
        let Some(hover) = self.asset_hover.as_ref() else {
            return;
        };
        let theme = context.theme();
        let style = context.style_of(theme);
        let mut tooltip_frame = theme::tooltip_card_frame(&style);
        if hover.kind == DocumentKind::Pdf {
            tooltip_frame = tooltip_frame
                .inner_margin(egui::Margin::same(METRICS.popup.card_inner_margin / 2))
                .outer_margin(egui::Margin::same(theme::SPACE.content as i8 / 2));
        }
        let frame_margin = tooltip_frame.total_margin().sum();
        let size = asset_hover_card_size(
            &hover.content,
            window_rect.size(),
            frame_margin,
            METRICS.popup.viewport_edge,
        );
        let root_local_card = place_native_tooltip_card(
            Rect::from_min_size(Pos2::ZERO, window_rect.size()),
            hover.origin,
            hover.anchor,
            size,
            hover.placement,
            METRICS.popup.viewport_edge,
        );
        let position = window_rect.min + root_local_card.min.to_vec2();
        let now = context.input(|input| input.time);
        let previous = context.data(|data| {
            data.get_temp::<TooltipGeometry>(geometry_id)
                .filter(|geometry| geometry.identity == identity)
        });
        let handoff_apex = previous.map_or_else(
            || {
                context
                    .pointer_hover_pos()
                    .or_else(|| context.pointer_latest_pos())
                    .filter(|pointer| hover.origin.contains(*pointer))
                    .unwrap_or_else(|| hover.origin.center())
            },
            |geometry| geometry.handoff_apex,
        );
        context.data_mut(|data| {
            data.insert_temp(
                geometry_id,
                TooltipGeometry {
                    identity,
                    origin: hover.origin,
                    card: root_local_card,
                    handoff_apex,
                    pointer_inside_viewport: previous
                        .is_some_and(|geometry| geometry.pointer_inside_viewport),
                    handoff_until: previous.map_or_else(
                        || tooltip_handoff_deadline(now),
                        |geometry| geometry.handoff_until,
                    ),
                },
            );
        });

        let spec = ChildViewSpec::tooltip(
            "asset-hover-overlay",
            "tiptoptyp asset preview",
            position,
            size,
            interaction.focus_requested || interaction.focused,
            "asset-hover",
        );
        let content_size = frame_content_size(size, frame_margin);
        let revision = match &hover.content {
            AssetHoverContent::Loading => 0,
            AssetHoverContent::Ready { .. } => 1,
            AssetHoverContent::Error(_) => 2,
        };
        repaint_tooltip_on_change(context, "asset-hover-overlay", identity, revision);
        let hover = hover.clone();
        let parent = context.viewport_id();
        let context = context.clone();
        ChildViewHost::show_deferred(
            &context.clone(),
            &self.captures,
            spec,
            theme,
            &style.clone(),
            move |ui, input| {
                let interaction = context
                    .data(|data| data.get_temp::<TooltipInteractionState>(interaction_id))
                    .filter(|state| state.identity == identity)
                    .unwrap_or(TooltipInteractionState::new(identity));
                let dismiss_requested = input.escape_pressed;
                let frame = if interaction.focused {
                    tooltip_frame.stroke(Stroke::new(
                        1.0,
                        style.visuals.widgets.active.bg_stroke.color,
                    ))
                } else {
                    tooltip_frame
                };
                let frame_response = frame.show(ui, |ui| {
                    show_asset_hover_contents(
                        ui,
                        &hover.path,
                        hover.kind,
                        &hover.content,
                        content_size,
                    );
                });
                let card_rect = frame_response.response.rect;
                let pointer_inside_viewport = ui.rect_contains_pointer(ui.max_rect());
                let pointer_inside_card = ui.rect_contains_pointer(card_rect);
                let popup_interacted =
                    pointer_inside_card && ui.input(|input| input.pointer.any_pressed());
                let interaction = update_tooltip_interaction_state(
                    interaction,
                    identity,
                    popup_interacted,
                    input.focused,
                );
                let interaction = if dismiss_requested {
                    TooltipInteractionState {
                        focused: false,
                        focus_requested: false,
                        dismissed: true,
                        ..interaction
                    }
                } else {
                    interaction
                };
                publish_tooltip_interaction(
                    &context,
                    parent,
                    geometry_id,
                    interaction_id,
                    interaction,
                    pointer_inside_viewport,
                );
                if popup_interacted {
                    crate::window_host::focus(
                        ui.ctx(),
                        ui.ctx().viewport_id(),
                        crate::window_host::FocusCause::UserAction,
                    );
                }
            },
        );
    }

    pub(super) fn show_diagnostic_tooltip_window(&mut self, context: &egui::Context) {
        if self.asset_hover.is_some() {
            return;
        }
        let native_tooltip_id = native_hover_tooltip_id(context);
        let (origin, anchor, detail, severity, placement) = if self.snapshot_scene
            == Some(UiSnapshotScene::DiagnosticTooltip)
        {
            (
                Rect::from_min_size(
                    Pos2::new(420.0, METRICS.chrome.toolbar_height + 86.0),
                    Vec2::splat(1.0),
                ),
                Pos2::new(420.0, METRICS.chrome.toolbar_height + 86.0),
                Arc::<str>::from(
                    "The character `#` is not valid in code\nHint: you are already in code mode\nHint: try removing the `#`",
                ),
                Some(DiagnosticSeverity::Error),
                TooltipPlacement::Right,
            )
        } else if self.snapshot_scene == Some(UiSnapshotScene::FunctionTooltip) {
            (
                Rect::from_min_size(
                    Pos2::new(420.0, METRICS.chrome.toolbar_height + 86.0),
                    Vec2::splat(1.0),
                ),
                Pos2::new(420.0, METRICS.chrome.toolbar_height + 86.0),
                Arc::<str>::from(
                    "```typc\ntext(body, size: length = 1em, fill: color = black)\n```\nDisplays content as text with the selected size and fill.",
                ),
                None,
                TooltipPlacement::Below,
            )
        } else if let Some(tooltip) = self.diagnostic_tooltip.as_ref() {
            (
                tooltip.origin,
                tooltip.anchor,
                Arc::from(tooltip.detail.as_str()),
                Some(tooltip.severity),
                TooltipPlacement::Right,
            )
        } else if let Some(tooltip) =
            context.data(|data| data.get_temp::<HoverTooltipOverlay>(native_tooltip_id))
        {
            (
                tooltip.origin,
                tooltip.anchor,
                tooltip.detail,
                None,
                TooltipPlacement::Below,
            )
        } else {
            return;
        };
        let interaction_id = tooltip_interaction_id(context);
        let geometry_id = tooltip_geometry_id(context);
        let identity = cached_tooltip_identity(context, origin, &detail);
        let interaction =
            context.data(|data| data.get_temp::<TooltipInteractionState>(interaction_id));
        if interaction.is_some_and(|state| state.identity == identity && state.dismissed) {
            self.diagnostic_tooltip = None;
            context.data_mut(|data| {
                data.remove::<HoverTooltipOverlay>(native_tooltip_id);
                data.remove::<TooltipGeometry>(geometry_id);
                data.remove::<TooltipInteractionState>(interaction_id);
            });
            return;
        }
        let deterministic_scene = matches!(
            self.snapshot_scene,
            Some(UiSnapshotScene::DiagnosticTooltip | UiSnapshotScene::FunctionTooltip)
        );
        let root_focused = context.input(|input| {
            input.viewport().focused == Some(true) && input.viewport().visible() != Some(false)
        });
        let geometry = context.data(|data| data.get_temp::<TooltipGeometry>(geometry_id));
        let handoff_active = native_tooltip_handoff_active(context, false);
        let root_ready = tooltip_viewport_should_render(
            deterministic_scene,
            root_focused,
            handoff_active,
            identity,
            geometry,
            interaction,
        );
        if !root_ready
            || self.settings_visible
            || self.packages_visible
            || self.rename_dialog.is_some()
            || matches!(self.app_popup, Some(ref popup) if !matches!(popup, AppPopup::GitChunk { .. }))
            || self.document_workflow.modal().is_some()
        {
            return;
        }
        let git_diff = severity.is_none()
            && context.data(|data| {
                data.get_temp::<HoverTooltipOverlay>(native_tooltip_id)
                    .is_some_and(|tip| tip.git_diff)
            });
        show_native_tooltip_card(
            context,
            "diagnostic-tooltip-overlay",
            anchor,
            origin,
            detail,
            git_diff,
            placement,
            &self.captures,
            &self.web_link_sender,
        );
    }

    pub(super) fn show_app_popup_window(&mut self, context: &egui::Context) {
        if app_popup_blocked_by_root_overlay(
            self.document_workflow.modal().is_some(),
            self.typst_overrides_visible,
            self.workspace_chooser_visible,
            self.rename_dialog.is_some(),
        ) {
            self.close_app_popup();
            return;
        }
        if matches!(&self.app_popup, Some(AppPopup::PreviewSource { window, .. }) if *window != context.viewport_id())
        {
            return;
        }
        if matches!(self.app_popup, Some(AppPopup::GitChunk { .. })) {
            return;
        }
        let Some(popup) = self.app_popup.take() else {
            return;
        };
        let popup_owner = match &popup {
            AppPopup::PreviewSource { window, .. } => *window,
            _ => context.viewport_id(),
        };
        let Some(window_rect) = context.input_for(popup_owner, |input| input.viewport().inner_rect)
        else {
            self.close_app_popup();
            return;
        };
        let theme = context.theme();
        let style = context.style_of(theme);
        let rename_path = self.document().path().clone();
        let shortcuts = self.settings.effective_shortcuts();
        let (anchor, desired_size) = match &popup {
            AppPopup::CommandPalette { anchor } => (*anchor, Vec2::new(600.0, 260.0)),
            AppPopup::PreviewSource { anchor, .. } => {
                let margin = theme::menu_card_frame(&style).total_margin().sum();
                (
                    *anchor,
                    Vec2::new(
                        popup_layout::menu_row_content_width(
                            context,
                            &style,
                            "Reveal in Source",
                            "",
                        ) + margin.x,
                        editor_menu_content_height(1, 0) + margin.y,
                    ),
                )
            }
            AppPopup::File { anchor } => (*anchor, command_popup_size(CommandMenu::File, &style)),
            AppPopup::Edit { anchor } => (*anchor, command_popup_size(CommandMenu::Edit, &style)),
            AppPopup::View { anchor } => (*anchor, command_popup_size(CommandMenu::View, &style)),
            AppPopup::Layout { anchor } => {
                (*anchor, layout_popup_size(context, &style, &shortcuts))
            }
            AppPopup::Workspace {
                anchor, is_file, ..
            } => (*anchor, workspace_context_menu_size(*is_file, &style)),
            AppPopup::Editor {
                anchor,
                link,
                table,
                math,
                ..
            } => {
                let can_format = self.document().kind().typesetting_language().is_some();
                // Only TeX keeps the choice flyout; a Typst target sizes as
                // one direct row on a plain card.
                let has_flyout = math
                    .as_ref()
                    .is_some_and(|target| target.single_alternative().is_none());
                let rows = editor_menu_row_specs(
                    link.is_some(),
                    table.is_some(),
                    math.as_ref(),
                    can_format,
                    &shortcuts,
                    &|chord| context.format_shortcut(&chord),
                );
                let choices: &[&str] = math
                    .as_ref()
                    .filter(|_| has_flyout)
                    .map(|target| target.choices())
                    .unwrap_or(&[]);
                let separators =
                    editor_menu_separators(link.is_some(), table.is_some(), can_format);
                let geometry = editor_menu_geometry(context, &style, &rows, choices, separators);
                (
                    *anchor,
                    editor_context_menu_size(&style, &geometry, has_flyout),
                )
            }
            AppPopup::GitChunk { anchor, .. } => (*anchor, Vec2::new(720.0, 440.0)),
            AppPopup::StatusLog { anchor } => {
                (*anchor, status_log_popup_size(self.status_log.len()))
            }
            AppPopup::FontSelector { anchor, .. } => (*anchor, METRICS.menu.font_selector_size),
        };
        let estimated_size = egui::vec2(
            desired_size
                .x
                .min((window_rect.width() - METRICS.popup.viewport_edge).max(1.0)),
            desired_size
                .y
                .min((window_rect.height() - METRICS.popup.viewport_edge).max(1.0)),
        );
        let menu_frame = theme::menu_card_frame(&style);
        // A flyout menu draws its own parent card, so there is no outer frame
        // consuming margins: content fills the viewport exactly. Subtracting
        // frame margins here (as plain menus do) would shrink the scroll area
        // below the column and clip the last rows.
        let is_flyout_menu = match &popup {
            AppPopup::Editor {
                math: Some(target), ..
            } => target.single_alternative().is_none(),
            _ => false,
        };
        let menu_content_size = if is_flyout_menu {
            estimated_size
        } else {
            let frame_margin = menu_frame.total_margin().sum();
            frame_content_size(estimated_size, frame_margin)
        };
        let menu_width = menu_content_size.x;
        let menu_height = menu_content_size.y;
        let anchor = if matches!(popup, AppPopup::StatusLog { .. }) {
            clamp_popup_above_anchor(anchor, estimated_size, window_rect.size())
        } else {
            clamp_popup_anchor(anchor, estimated_size, window_rect.size())
        };
        let (can_undo, can_redo) = self.editor_history_availability(context);
        let has_selection = self.selected_editor_chars(context).is_some();
        let can_format = self.document().kind().typesetting_language().is_some();
        let can_export_pdf = self.source_preview_available();
        let can_sync_preview = (self.document().kind().is_typst()
            && self.interactive_preview_active())
            || (self.document().kind() == DocumentKind::Tex
                && self.synctex.artifact.is_some()
                && self.tex_tools.synctex.is_some());
        let edit_availability = CommandAvailability {
            can_undo,
            can_redo,
            typesetting_document: can_format,
            interactive_preview: can_sync_preview,
            new_table: self.native_command_enabled(AppCommand::NewTable),
            edit_table: self.native_command_enabled(AppCommand::EditTable),
            source_read_only: self.table_editor.is_some(),
            ..Default::default()
        };
        let layout_state = self.layout_menu_state();
        let mut close = false;
        let mut action = None;
        let mut had_focus = self.app_popup_had_focus;
        let mut blur_started = self.app_popup_blur_started;
        let popup_generation = self.app_popup_generation;
        let captures = self.captures.clone();
        let is_git_chunk_popup = matches!(&popup, AppPopup::GitChunk { .. });
        let is_palette = matches!(&popup, AppPopup::CommandPalette { .. });
        let palette_entries = is_palette.then(|| self.command_palette_entries(context));
        let palette_origin = self.command_palette_origin;
        let palette_viewport = scoped_child_viewport_id(context, "tiptoptyp-popup-overlay");
        let owner_frame = context.cumulative_frame_nr();
        let allow_paste = self
            .pending_widget_paste
            .is_some_and(|paste| paste.admits(palette_viewport, owner_frame));
        let mut palette_paste_requested = false;
        let mut palette_paste_admitted = false;
        let mut explicit_palette_cancel = false;
        let popup_title = if is_palette {
            "tiptoptyp Command Palette"
        } else if is_git_chunk_popup {
            "tiptoptyp Diff"
        } else {
            "tiptoptyp menu"
        };
        let spec = ChildViewSpec::dismiss_on_blur(
            "tiptoptyp-popup-overlay",
            popup_title,
            window_rect.min + anchor.to_vec2(),
            estimated_size,
            "popup",
        );

        ChildViewHost::show(context, &captures, spec, theme, &style, |ui, input| {
            // The palette owns Escape so an IME composition can consume it.
            close |= input.close_requested || (!is_palette && input.escape_pressed);
            let now = Instant::now();
            close |=
                popup_focus_should_close(&mut had_focus, &mut blur_started, input.focused, now);
            if let Some(started) = blur_started {
                let elapsed = now.saturating_duration_since(started);
                if elapsed < POPUP_BLUR_GRACE {
                    ui.ctx().request_repaint_after(POPUP_BLUR_GRACE - elapsed);
                }
            }

            egui::Area::new(viewport_scoped_id(ui.ctx(), "app-popup-card"))
                .order(egui::Order::Foreground)
                .fixed_pos(Pos2::ZERO)
                .show(ui.ctx(), |ui| {
                    // A flyout menu draws its own parent and flyout cards so
                    // the window never shows an empty reserved column.
                    let frame = if is_flyout_menu {
                        egui::Frame::NONE
                    } else {
                        menu_frame
                    };
                    frame.show(ui, |ui| {
                        show_popup_contents(
                            ui,
                            Vec2::new(menu_width, menu_height),
                            popup_generation,
                            |ui| match &popup {
                                AppPopup::PreviewSource { target, .. } => {
                                    if menu_item(ui, "Reveal in Source", None).clicked() {
                                        action = Some(AppPopupAction::RevealPreviewSource(
                                            target.clone(),
                                        ));
                                    }
                                }
                                AppPopup::CommandPalette { .. } => {
                                    let edits = prepare_child_widget_edit_events(
                                        ui.ctx(),
                                        &shortcuts,
                                        allow_paste,
                                    );
                                    // Native menu edits are already resolved commands;
                                    // inject after normalizing physical key events.
                                    let _queued = crate::child_view::begin_queued_edit_events(
                                        ui.ctx(),
                                        &mut self.command_palette_edit_events,
                                    );
                                    palette_paste_requested |= edits.paste_requested;
                                    palette_paste_admitted |= edits.requested_paste_admitted;
                                    match self
                                        .command_palette
                                        .show(ui, palette_entries.as_deref().unwrap_or_default())
                                    {
                                        Some(command_palette::Action::Execute(selected)) => {
                                            if let Some(origin) = palette_origin {
                                                action = Some(AppPopupAction::Palette {
                                                    action: selected,
                                                    origin,
                                                });
                                            }
                                        }
                                        Some(command_palette::Action::Dismiss) => {
                                            close = true;
                                            explicit_palette_cancel = true;
                                        }
                                        None => {}
                                    }
                                }
                                AppPopup::File { .. }
                                | AppPopup::Edit { .. }
                                | AppPopup::View { .. }
                                    if self.tabs.is_empty() =>
                                {
                                    let menu = match popup {
                                        AppPopup::File { .. } => CommandMenu::File,
                                        AppPopup::Edit { .. } => CommandMenu::Edit,
                                        _ => CommandMenu::View,
                                    };
                                    show_command_popup_ui(
                                        ui,
                                        menu,
                                        CommandAvailability {
                                            empty_workspace: true,
                                            ..Default::default()
                                        },
                                        &shortcuts,
                                        &mut action,
                                    );
                                }
                                AppPopup::File { .. } => {
                                    show_file_popup_ui(
                                        ui,
                                        can_export_pdf,
                                        rename_path.as_deref(),
                                        &shortcuts,
                                        &mut action,
                                    );
                                }
                                AppPopup::Edit { .. } => {
                                    show_edit_popup_ui(
                                        ui,
                                        edit_availability,
                                        &shortcuts,
                                        &mut action,
                                    );
                                }
                                AppPopup::View { .. } => {
                                    show_view_popup_ui(
                                        ui,
                                        self.document().kind().typesetting_language().is_some(),
                                        &shortcuts,
                                        &mut action,
                                    );
                                }
                                AppPopup::Layout { .. } => {
                                    show_layout_popup_ui(ui, layout_state, &shortcuts, &mut action);
                                }
                                AppPopup::Workspace { path, is_file, .. } => {
                                    let preview_selected = self
                                        .designated_preview_path()
                                        .is_some_and(|preview| same_path(&preview, path));
                                    show_workspace_popup_ui(
                                        ui,
                                        path,
                                        *is_file,
                                        preview_selected,
                                        &mut action,
                                    );
                                }
                                AppPopup::Editor {
                                    source,
                                    link,
                                    table,
                                    math,
                                    ..
                                } => {
                                    let mut editor_action = None;
                                    show_editor_context_menu_ui(
                                        ui,
                                        EditorContextMenuOptions {
                                            can_undo,
                                            can_redo,
                                            has_selection,
                                            can_format,
                                            can_sync_preview: can_sync_preview
                                                && source.is_some_and(|target| {
                                                    target.key == self.document().key()
                                                }),
                                            link: link.as_deref(),
                                            table: table.as_ref(),
                                            math: math.as_ref(),
                                        },
                                        &shortcuts,
                                        &mut editor_action,
                                    );
                                    if let Some(editor_action) =
                                        editor_action.and_then(|action| action.at_source(*source))
                                    {
                                        action = Some(AppPopupAction::Editor(editor_action));
                                    }
                                }
                                AppPopup::StatusLog { .. } => {
                                    show_status_log_popup_ui(ui, &self.status_log);
                                }
                                AppPopup::FontSelector { target, .. } => {
                                    show_document_font_selector_ui(
                                        ui,
                                        &self.font_catalog,
                                        target,
                                        &mut action,
                                    );
                                }
                                AppPopup::GitChunk { .. } => {
                                    unreachable!("hunks belong to the code panel")
                                }
                            },
                        );
                    });
                });
        });
        self.app_popup_had_focus = had_focus;
        self.app_popup_blur_started = blur_started;

        if palette_paste_admitted {
            self.pending_widget_paste = None;
        }
        if palette_paste_requested && !close && action.is_none() {
            self.request_widget_paste(context, palette_viewport);
        }
        let action_selected = action.is_some();
        if close || action_selected {
            if is_palette && !action_selected {
                self.dismiss_command_palette(context, explicit_palette_cancel);
            } else {
                self.close_app_popup();
            }
            if is_git_chunk_popup {
                self.git_editor.chunk = None;
            }
        } else {
            self.app_popup = Some(popup);
        }
        // A popup losing focus usually means the user activated another app.
        // Only an in-app menu selection should return keyboard focus to the
        // editor; reclaiming it on blur makes tiptoptyp steal activation.
        if action_selected && !is_palette {
            crate::window_host::return_focus(context);
        }
        if let Some(action) = action {
            self.pending_app_popup_action = Some(action);
            context.request_repaint();
        }
    }

    #[cfg(any(target_os = "macos", target_os = "windows"))]
    pub(super) fn update_webview(
        &mut self,
        context: &egui::Context,
        frame: Option<&eframe::Frame>,
        _rect: Rect,
        native_rect: NativeRect,
        background: Color32,
        visible: bool,
    ) -> bool {
        use wry::dpi::{LogicalPosition, LogicalSize};

        if self
            .webview_parent
            .is_some_and(|parent| parent != context.viewport_id())
        {
            self.discard_webview();
        }
        let Some(url) = self.preview.connection.endpoint().map(ToString::to_string) else {
            self.hide_webview();
            return false;
        };
        let navigation_state = PreviewNavigationContext {
            base_url: url.clone(),
            project_root: self.project_root(),
            source_dir: self.preview_document_path().parent().map(Path::to_path_buf),
        };
        if let Some(shared) = &self.webview_navigation
            && let Ok(mut current) = shared.lock()
        {
            *current = navigation_state.clone();
        }
        let bounds = wry::Rect {
            position: LogicalPosition::new(native_rect.left() as f64, native_rect.top() as f64)
                .into(),
            size: LogicalSize::new(
                native_rect.width().max(1.0) as f64,
                native_rect.height().max(1.0) as f64,
            )
            .into(),
        };
        let next_applied = WebviewAppliedState {
            bounds: native_rect,
            background,
            visible,
        };
        if self.webview.is_none() {
            let focused = eframe::window_host::window(context, context.viewport_id())
                .map(|window| window.has_focus())
                .or_else(|| context.input(|input| input.viewport().focused));
            let may_create =
                may_create_window_webview(self.window_host, cfg!(target_os = "macos"), focused);
            if !may_create {
                // Wry currently activates NSApplication while constructing a
                // WKWebView, including child web views. Defer creation until
                // the user next activates this window so background service
                // changes can never pull focus from another application.
                self.preview.webview_state = ServiceState::Starting(
                    "Interactive preview will resume when the window is active".to_owned(),
                );
                return false;
            }
            let navigation_sender = self.web_link_sender.clone();
            let navigation_repaint = crate::worker::RepaintTarget::current(context);
            let popup_sender = self.web_link_sender.clone();
            let popup_repaint = crate::worker::RepaintTarget::current(context);
            let load_sender = self.web_action_sender.clone();
            let load_repaint = crate::worker::RepaintTarget::current(context);
            let action_sender = self.web_action_sender.clone();
            let action_repaint = crate::worker::RepaintTarget::current(context);
            let shared_navigation = Arc::new(Mutex::new(navigation_state));
            let navigation_handler_state = Arc::clone(&shared_navigation);
            let popup_handler_state = Arc::clone(&shared_navigation);
            let rgba = (
                background.r(),
                background.g(),
                background.b(),
                background.a(),
            );
            let palette_script = self.preview_webview_style(context).script();
            let builder = wry::WebViewBuilder::new()
                .with_focused(false)
                .with_url(&url)
                .with_initialization_script("window.tiptoptypCaptureZoomSnapshot = request => window.ipc?.postMessage(JSON.stringify({type:'preview-zoom-snapshot', ...request}));")
                .with_initialization_script(include_str!("../preview_zoom_snapshot.js"))
                .with_initialization_script(include_str!("../preview_snapshot_buffer.js"))
                .with_initialization_script(include_str!("../preview_navigation.js"))
                .with_initialization_script(palette_script)
                .with_on_page_load_handler(move |event, _url| {
                    // evaluate_script returning Ok only means queued. A navigation
                    // can discard it, then replay the creation-time palette.
                    // Re-send the current palette after the new document is ready.
                    if matches!(event, wry::PageLoadEvent::Finished)
                        && load_sender
                            .send(r#"{"type":"preview-loaded"}"#.to_owned())
                            .is_ok()
                    {
                        load_repaint.request_repaint();
                    }
                })
                .with_ipc_handler(move |request| {
                    if action_sender.send(request.body().clone()).is_ok() {
                        action_repaint.request_repaint();
                    }
                })
                .with_bounds(bounds)
                .with_visible(visible)
                .with_background_color(rgba)
                .with_background_throttling(wry::BackgroundThrottlingPolicy::Disabled)
                // The SVG adapter owns keyboard zoom; AppKit still handles
                // native pinch magnification without a second hotkey scale.
                .with_hotkeys_zoom(false)
                .with_navigation_handler(move |candidate| {
                    let action = navigation_handler_state
                        .lock()
                        .map(|state| preview_navigation_action(&state, &candidate))
                        .unwrap_or_else(|_| PreviewNavigationAction::Dispatch(candidate));
                    match action {
                        PreviewNavigationAction::Embed => true,
                        PreviewNavigationAction::Dispatch(target) => {
                            if navigation_sender.send(target).is_ok() {
                                navigation_repaint.request_repaint();
                            }
                            false
                        }
                    }
                })
                .with_new_window_req_handler(move |candidate, _features| {
                    let target = popup_handler_state
                        .lock()
                        .ok()
                        .and_then(|state| preview_new_window_target(&state, &candidate));
                    if let Some(target) = target
                        && popup_sender.send(target).is_ok()
                    {
                        popup_repaint.request_repaint();
                    }
                    wry::NewWindowResponse::Deny
                });
            let parent = crate::native_window::for_viewport(context, context.viewport_id())
                .or_else(|| frame.and_then(crate::native_window::NativeWindowHandle::from_owner));
            let Some(parent) = parent else {
                return false;
            };
            let built = builder.build_as_child(&parent);
            match built {
                Ok(webview) => {
                    // Native preview creation is background rendering work.
                    // with_focused(false) is unsupported on macOS, so explicitly
                    // leave the parent as the keyboard owner after construction.
                    let _ = webview.focus_parent();
                    #[cfg(target_os = "macos")]
                    {
                        crate::native_window::enable_native_webview_magnification(&webview);
                        self.webview_snapshot.bind(&webview);
                    }
                    self.webview = Some(webview);
                    self.webview_parent = Some(context.viewport_id());
                    self.webview_url = Some(url.clone());
                    self.webview_reload_pending = false;
                    self.webview_navigation = Some(shared_navigation);
                    self.webview_applied = Some(next_applied);
                    self.preview.webview_state =
                        ServiceState::Ready("Tinymist vector frontend is embedded".to_owned());
                }
                Err(error) => {
                    self.fail_local_webview(format!(
                        "Could not embed the Tinymist preview: {error}"
                    ));
                    return false;
                }
            }
        }
        if webview_navigation_required(
            self.webview_url.as_deref(),
            &url,
            self.webview_reload_pending,
        ) {
            #[cfg(target_os = "macos")]
            self.webview_snapshot.allowed(false);
            if let Some(webview) = &self.webview
                && let Err(error) = webview.load_url(&url)
            {
                self.fail_local_webview(format!("Could not load the Tinymist preview: {error}"));
                return false;
            }
            self.webview_url = Some(url);
            self.webview_reload_pending = false;
            self.webview_style = None;
        }
        let style = self.preview_webview_style(context);
        #[cfg(target_os = "macos")]
        self.webview_snapshot
            .allowed(visible && style.zoom_snapshot);
        if let Some(webview) = &self.webview {
            if self.webview_style != Some(style) {
                #[cfg(target_os = "macos")]
                if self
                    .webview_style
                    .is_some_and(|previous| previous.palette != style.palette)
                {
                    self.webview_snapshot.retire_image();
                }
                let script = style.script();
                if webview.evaluate_script(&script).is_ok() {
                    self.webview_style = Some(style);
                }
            }
            let diff = webview_property_diff(self.webview_applied, next_applied);
            if diff.background {
                let _ = webview.set_background_color((
                    background.r(),
                    background.g(),
                    background.b(),
                    background.a(),
                ));
            }
            if diff.bounds
                && let Err(error) = webview.set_bounds(bounds)
            {
                self.fail_local_webview(format!("Could not position the vector preview: {error}"));
                return false;
            }
            #[cfg(target_os = "macos")]
            if diff.bounds {
                self.webview_snapshot.layout(webview);
            }
            if diff.visible
                && let Err(error) = webview.set_visible(visible)
            {
                self.fail_local_webview(format!("Could not show the vector preview: {error}"));
                return false;
            }
            if std::env::var_os("TIPTOPTYP_UI_TRACE").is_some()
                && (diff.bounds || diff.background || diff.visible)
            {
                eprintln!(
                    "ui.preview.properties bounds={} background={} visible={}",
                    diff.bounds, diff.background, diff.visible
                );
            }
            self.webview_applied = Some(next_applied);
        }
        if self.preview.tinymist_state.is_ready() {
            self.preview.webview_state =
                ServiceState::Ready("Tinymist vector frontend is embedded".to_owned());
        }
        true
    }

    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    pub(super) fn update_webview(
        &mut self,
        _context: &egui::Context,
        _frame: Option<&eframe::Frame>,
        _rect: Rect,
        _native_rect: NativeRect,
        _background: Color32,
        _visible: bool,
    ) -> bool {
        false
    }

    #[cfg(any(target_os = "macos", target_os = "windows"))]
    fn release_webview_focus(&mut self) {
        if std::mem::take(&mut self.preview_controls.web.focused)
            && let Some(webview) = &self.webview
        {
            let _ = webview.focus_parent();
        }
    }

    #[cfg(any(target_os = "macos", target_os = "windows"))]
    pub(super) fn hide_webview(&mut self) {
        #[cfg(target_os = "macos")]
        self.webview_snapshot.allowed(false);
        if self.webview_applied.is_some_and(|applied| !applied.visible) {
            return;
        }
        self.release_webview_focus();
        if let Some(webview) = &self.webview {
            let _ = webview.set_visible(false);
            if let Some(applied) = &mut self.webview_applied {
                applied.visible = false;
            }
        }
    }

    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    pub(super) fn hide_webview(&mut self) {}
}

fn dialog_scroll_height(owner_height: f32, reserved: f32) -> f32 {
    (owner_height - reserved).clamp(0.0, METRICS.popup.modal_message_max_height)
}

fn dialog_footer_height(
    ui: &egui::Ui,
    width: f32,
    primary: &str,
    secondary: Option<&str>,
    cancel: bool,
) -> f32 {
    let mut used = 0.0;
    let mut height = 0.0;
    let mut row_height = 0.0_f32;
    let padding = ui.spacing().button_padding * 2.0
        + Vec2::splat(ui.visuals().widgets.inactive.bg_stroke.width * 2.0);
    for label in [Some(primary), secondary, cancel.then_some("Cancel")]
        .into_iter()
        .flatten()
    {
        let size = egui::WidgetText::from(label)
            .into_galley(
                ui,
                Some(egui::TextWrapMode::Wrap),
                (width - padding.x).max(1.0),
                egui::TextStyle::Button,
            )
            .size()
            + padding;
        if used > 0.0 && used + ui.spacing().item_spacing.x + size.x > width {
            height += row_height + ui.spacing().item_spacing.y;
            row_height = 0.0;
            used = 0.0;
        }
        used += if used == 0.0 {
            size.x
        } else {
            ui.spacing().item_spacing.x + size.x
        };
        row_height = row_height.max(size.y.max(ui.spacing().interact_size.y));
    }
    height + row_height
}

fn dialog_footer(
    ui: &mut egui::Ui,
    primary: &str,
    secondary: Option<&str>,
    cancel: bool,
    observe_modal: bool,
) -> Option<AppModalChoice> {
    let mut choice = None;
    ui.allocate_ui_with_layout(
        Vec2::new(ui.available_width(), ui.spacing().interact_size.y),
        Layout::right_to_left(Align::Center).with_main_wrap(true),
        |ui| {
            if ui
                .add(egui::Button::new(primary).fill(ui.visuals().selection.bg_fill))
                .clicked()
            {
                choice = Some(AppModalChoice::Primary);
            }
            if let Some(label) = secondary {
                let response = ui.button(label);
                #[cfg(all(feature = "desktop-ui-tests", target_os = "macos"))]
                if observe_modal {
                    crate::desktop_test::observe("modal.discard", &response);
                }
                if response.clicked() {
                    choice = Some(AppModalChoice::Secondary);
                }
            }
            if cancel {
                let response = ui.button("Cancel");
                #[cfg(all(feature = "desktop-ui-tests", target_os = "macos"))]
                if observe_modal {
                    crate::desktop_test::observe("modal.cancel", &response);
                }
                if response.clicked() {
                    choice = Some(AppModalChoice::Cancel);
                }
            }
        },
    );
    let _ = observe_modal;
    choice
}

#[cfg(test)]
mod property_tests {
    use super::*;

    #[test]
    fn dialog_scroll_body_reserves_title_and_footer_space_inside_short_owners() {
        assert_eq!(dialog_scroll_height(160.0, 140.0), 20.0);
        assert_eq!(dialog_scroll_height(160.0, 160.0), 0.0);
        assert_eq!(dialog_scroll_height(160.0, 180.0), 0.0);
        assert_eq!(dialog_scroll_height(900.0, 140.0), 180.0);
    }

    #[test]
    fn labeled_dialog_footers_wrap_inside_narrow_cards_with_monospace_ui() {
        use egui_kittest::{Harness, kittest::Queryable as _};
        for (primary, secondary) in [
            ("Save", Some("Discard")),
            ("Choose Folder…", None),
            ("Rename", None),
        ] {
            let mut harness = Harness::builder()
                .with_size(Vec2::new(168.0, 100.0))
                .build_ui_state(
                    move |ui, choice| {
                        theme::apply_compact_tool_spacing(ui);
                        for kind in [egui::TextStyle::Body, egui::TextStyle::Button] {
                            ui.style_mut()
                                .text_styles
                                .insert(kind, egui::FontId::monospace(18.0));
                        }
                        *choice = dialog_footer(ui, primary, secondary, true, false);
                    },
                    None,
                );
            harness.run();
            let cancel = harness.get_by_label("Cancel").rect();
            let first = harness.get_by_label(primary).rect();
            assert!(cancel.top() > first.top(), "{primary}: footer did not wrap");
            for label in [Some(primary), secondary, Some("Cancel")]
                .into_iter()
                .flatten()
            {
                let rect = harness.get_by_label(label).rect();
                assert!(
                    rect.left() >= 0.0 && rect.right() <= 168.0 && rect.bottom() <= 100.0,
                    "{primary} / {label}: {rect:?}"
                );
            }
            harness.get_by_label("Cancel").click();
            harness.step();
            assert_eq!(*harness.state(), Some(AppModalChoice::Cancel));
        }
    }

    #[test]
    fn table_window_starts_compact_and_preserves_comparable_qa_sizes() {
        assert_eq!(table_window_size(None), [860.0, 560.0]);
        assert_eq!(
            table_window_size(Some(UiSnapshotScene::TableEditor)),
            [940.0, 700.0]
        );
        assert_eq!(
            table_window_size(Some(UiSnapshotScene::TableEditorTexNarrow)),
            [620.0, 600.0]
        );
    }

    fn rect(left: f32) -> NativeRect {
        ViewportTransform::new([0.0, 0.0], 1.0)
            .unwrap()
            .to_native(EguiRect::new([left, 0.0], [left + 100.0, 100.0]).unwrap())
            .unwrap()
    }

    #[cfg(any(target_os = "macos", target_os = "windows"))]
    #[test]
    fn webview_receives_theme_accent_even_when_page_colors_stay_the_same() {
        let context = egui::Context::default();
        let directory = tempfile::tempdir().unwrap();
        let app = EditorApp::dormant_for_tests(&context, directory.path().into());
        let before = app.preview_webview_style(&context);
        let theme = crate::builtin_themes::find("catppuccin-mocha").unwrap();
        theme::set_imported_palette(&context, Some((theme.dark_mode, theme.palette)));
        let after = app.preview_webview_style(&context);
        assert_eq!(
            after.palette, before.palette,
            "page appearance stays independent"
        );
        assert_ne!(
            before, after,
            "accent-only changes must reach an existing WebView"
        );
        let script = after.script();
        let arguments = script
            .strip_prefix("window.tiptoptypSetPalette?.(")
            .unwrap()
            .split_once(");")
            .unwrap()
            .0;
        let values: serde_json::Value = serde_json::from_str(&format!("[{arguments}]")).unwrap();
        assert_eq!(
            values[4],
            serde_json::json!([
                theme.palette.accent.r,
                theme.palette.accent.g,
                theme.palette.accent.b
            ])
        );
    }

    #[cfg(any(target_os = "macos", target_os = "windows"))]
    #[test]
    fn reveal_preferences_reach_existing_webviews_without_changing_the_palette_or_bounds() {
        let context = egui::Context::default();
        let directory = tempfile::tempdir().unwrap();
        let mut app = EditorApp::dormant_for_tests(&context, directory.path().into());
        let before = app.preview_webview_style(&context);
        app.settings.preview_reveal_animation = crate::settings::RevealAnimation {
            enabled: false,
            style: crate::settings::RevealStyle::Spotlight,
            duration_ms: 1500,
            size_percent: 150,
        };
        let after = app.preview_webview_style(&context);
        assert_ne!(before, after);
        assert_eq!(before.palette, after.palette);
        assert_eq!(before.hover, after.hover);
        assert_eq!(before.focus, after.focus);
        assert_eq!(before.zoom_snapshot, after.zoom_snapshot);
        assert_eq!(app.preview_webview_style(&context), after);
        let script = after.script();
        let options = script
            .split("window.tiptoptypSetRevealOptions?.(")
            .nth(1)
            .unwrap()
            .strip_suffix(");")
            .unwrap();
        let options: crate::settings::RevealAnimation = serde_json::from_str(options).unwrap();
        assert_eq!(options, app.settings.preview_reveal_animation);
    }

    #[test]
    fn stable_frames_emit_no_native_property_updates_and_recreation_invalidates_all() {
        let state = WebviewAppliedState {
            bounds: rect(0.0),
            background: Color32::BLACK,
            visible: true,
        };
        let first = webview_property_diff(None, state);
        assert_eq!(
            first,
            WebviewPropertyDiff {
                bounds: true,
                background: true,
                visible: true,
            }
        );
        for _ in 0..100 {
            assert_eq!(
                webview_property_diff(Some(state), state),
                WebviewPropertyDiff {
                    bounds: false,
                    background: false,
                    visible: false,
                }
            );
        }
        let moved = WebviewAppliedState {
            bounds: rect(1.0),
            ..state
        };
        assert_eq!(
            webview_property_diff(Some(state), moved),
            WebviewPropertyDiff {
                bounds: true,
                background: false,
                visible: false,
            }
        );
        assert_eq!(webview_property_diff(None, moved), first);
    }
}
