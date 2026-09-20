//! Owned child-window views and native preview composition.
use super::*;

#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct WebviewAppliedState {
    bounds: NativeRect,
    background: Color32,
    visible: bool,
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

impl EditorApp {
    /// Native objects stay on the owning UI thread; discard their cached
    /// presentation identity together so a recreated view cannot inherit it.
    pub(super) fn discard_webview(&mut self) {
        #[cfg(any(target_os = "macos", target_os = "windows"))]
        {
            self.webview = None;
            self.webview_applied = None;
            self.webview_url = None;
            self.webview_navigation = None;
            self.webview_reload_pending = false;
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
                            ui.add_space(
                                METRICS.toolbar.traffic_lights_fallback_width
                                    + METRICS.toolbar.traffic_lights_gap,
                            );
                            crate::window_logo::show(ui, &captures);
                            ui.label(RichText::new("Typst packages").strong());
                            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                refresh_requested |= ui
                                    .add_enabled(!loading, egui::Button::new("Refresh"))
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
            context.send_viewport_cmd(egui::ViewportCommand::Focus);
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
        if self.table_editor.is_none()
            || self.document_workflow.modal().is_some()
            || self.table_editor_suspended
        {
            return;
        }
        let Some(window_rect) = context.input(|input| input.viewport().inner_rect) else {
            return;
        };
        let card_width = (window_rect.width() - METRICS.popup.modal_window_inset).clamp(1.0, 900.0);
        let cells_height = (window_rect.height() - 190.0).clamp(80.0, 420.0);
        let theme = context.theme();
        let style = context.style_of(theme);
        let Some(dialog) = &mut self.table_editor else {
            return;
        };
        let mut requested_action = None;
        let mut overlay_had_focus = self.table_editor_had_focus;
        let mut suspend_overlay = false;
        let captures = self.captures.clone();
        let spec = ChildViewSpec::modal(
            "tiptoptyp-table-editor-overlay",
            "Table editor",
            window_rect,
            "table-editor",
        );
        ChildViewHost::show(context, &captures, spec, theme, &style, |ui, input| {
            if input.focused == Some(true) {
                overlay_had_focus = true;
            }
            suspend_overlay |= overlay_had_focus && input.focused == Some(false);
            if input.close_requested || input.escape_pressed {
                requested_action = Some(TableEditorUiAction::Cancel);
            }
            if suspend_overlay {
                ui.ctx()
                    .send_viewport_cmd(egui::ViewportCommand::Visible(false));
            }
            egui::Area::new(viewport_scoped_id(ui.ctx(), "table-editor-dialog"))
                .order(egui::Order::Foreground)
                .anchor(egui::Align2::CENTER_CENTER, Vec2::ZERO)
                .show(ui.ctx(), |ui| {
                    theme::dialog_card_frame(&style).show(ui, |ui| {
                        ui.set_width(card_width);
                        if let Some(action) =
                            show_table_editor_ui(ui, dialog, card_width, cells_height)
                        {
                            requested_action = Some(action);
                        }
                    });
                });
        });
        self.table_editor_had_focus = overlay_had_focus;
        if suspend_overlay {
            self.table_editor_suspended = true;
        }

        match requested_action {
            Some(TableEditorUiAction::Cancel) => {
                self.table_editor = None;
                self.table_editor_had_focus = false;
                self.table_editor_suspended = false;
                context.send_viewport_cmd(egui::ViewportCommand::Focus);
            }
            Some(TableEditorUiAction::Apply) => {
                let mut dialog = self.table_editor.take().expect("table editor exists");
                match prepare_table_source_edit(
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
                        self.table_editor_had_focus = false;
                        self.table_editor_suspended = false;
                        context.send_viewport_cmd(egui::ViewportCommand::Focus);
                    }
                    Err(error) => {
                        dialog.error = Some(error);
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
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            submit |= ui.button("Rename").clicked();
                            cancel |= ui.button("Cancel").clicked();
                        });
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
            context.send_viewport_cmd(egui::ViewportCommand::Focus);
        } else if submit {
            let dialog = self.rename_dialog.take().expect("rename dialog exists");
            self.rename_overlay_had_focus = false;
            self.rename_overlay_suspended = false;
            context.send_viewport_cmd(egui::ViewportCommand::Focus);
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
                            ui.set_width(card_width);
                            ui.label(RichText::new("Change workspace root").strong());
                            ui.label(
                                RichText::new(
                                    "Choose a recent workspace or select another folder.",
                                )
                                .color(ui.visuals().weak_text_color()),
                            );
                            if !recent_workspaces.is_empty() {
                                ui.add_space(theme::SPACE.small);
                                ui.label(
                                    RichText::new("Recent")
                                        .size(theme::TYPE.supporting)
                                        .strong(),
                                );
                                egui::ScrollArea::vertical()
                                    .id_salt("recent-workspaces")
                                    .max_height(METRICS.popup.modal_message_max_height)
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
                            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                choose_folder |= ui.button("Choose Folder…").clicked();
                                cancel |= ui.button("Cancel").clicked();
                            });
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
            context.send_viewport_cmd(egui::ViewportCommand::Focus);
            self.queue_open_path(path);
        } else if choose_folder {
            self.workspace_chooser_visible = false;
            context.send_viewport_cmd(egui::ViewportCommand::Focus);
            self.open_folder_dialog();
        } else if cancel || dismiss {
            self.workspace_chooser_visible = false;
            context.send_viewport_cmd(egui::ViewportCommand::Focus);
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
        let message_height = (window_rect.height() - METRICS.popup.modal_height_inset).clamp(
            METRICS.popup.modal_message_min_height,
            METRICS.popup.modal_message_max_height,
        );
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
                            ui.set_max_width(card_width);
                            ui.set_min_width(card_width);
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
                                    ("warning", message.as_str(), warning_color(ui.ctx()))
                                }
                                AppModal::UninstallPackage { message, .. }
                                | AppModal::DeleteFile { message, .. }
                                | AppModal::Overwrite { message, .. } => {
                                    ("warning", message.as_str(), warning_color(ui.ctx()))
                                }
                            };
                            ui.label(RichText::new(title).strong().color(color));
                            egui::ScrollArea::vertical()
                                .max_height(message_height)
                                .show(ui, |ui| {
                                    ui.label(message);
                                });
                            ui.add_space(theme::SPACE.control);
                            ui.with_layout(
                                Layout::right_to_left(Align::Center),
                                |ui| match &modal {
                                    AppModal::Alert { .. } => {
                                        if ui.button("OK").clicked() {
                                            choice = Some(AppModalChoice::Primary);
                                        }
                                    }
                                    AppModal::Unsaved { .. } => {
                                        if ui.button("Save").clicked() {
                                            choice = Some(AppModalChoice::Primary);
                                        }
                                        if ui.button("Discard").clicked() {
                                            choice = Some(AppModalChoice::Secondary);
                                        }
                                        if ui.button("Cancel").clicked() {
                                            choice = Some(AppModalChoice::Cancel);
                                        }
                                    }
                                    AppModal::UninstallPackage { .. } => {
                                        if ui.button("Uninstall").clicked() {
                                            choice = Some(AppModalChoice::Primary);
                                        }
                                        if ui.button("Cancel").clicked() {
                                            choice = Some(AppModalChoice::Cancel);
                                        }
                                    }
                                    AppModal::DeleteFile { .. } => {
                                        if ui.button("Delete").clicked() {
                                            choice = Some(AppModalChoice::Primary);
                                        }
                                        if ui.button("Cancel").clicked() {
                                            choice = Some(AppModalChoice::Cancel);
                                        }
                                    }
                                    AppModal::Overwrite { .. } => {
                                        if ui.button("Overwrite").clicked() {
                                            choice = Some(AppModalChoice::Primary);
                                        }
                                        if ui.button("Cancel").clicked() {
                                            choice = Some(AppModalChoice::Cancel);
                                        }
                                    }
                                },
                            );
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
        context.send_viewport_cmd(egui::ViewportCommand::Focus);
        match (modal, choice) {
            (AppModal::Alert { .. }, _) => {}
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
            || self.git_editor.chunk.is_some()
            || self.rename_dialog.is_some()
            || self.table_editor.is_some()
            || self.app_popup.is_some()
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
        let tooltip_frame = theme::tooltip_card_frame(&style);
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
                    ui.ctx().send_viewport_cmd(egui::ViewportCommand::Focus);
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
            || self.git_editor.chunk.is_some()
            || self.rename_dialog.is_some()
            || self.app_popup.is_some()
            || self.document_workflow.modal().is_some()
        {
            return;
        }
        show_native_tooltip_card(
            context,
            "diagnostic-tooltip-overlay",
            anchor,
            origin,
            detail,
            severity,
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
            self.table_editor.is_some(),
        ) {
            self.close_app_popup();
            return;
        }
        let Some(popup) = self.app_popup.take() else {
            return;
        };
        let Some(window_rect) = context.input(|input| input.viewport().inner_rect) else {
            self.close_app_popup();
            return;
        };
        let theme = context.theme();
        let style = context.style_of(theme);
        let rename_path = self.document().path().clone();
        let (anchor, desired_size) = match &popup {
            AppPopup::File { anchor } => (*anchor, command_popup_size(CommandMenu::File, &style)),
            AppPopup::Edit { anchor } => (*anchor, command_popup_size(CommandMenu::Edit, &style)),
            AppPopup::View { anchor } => (*anchor, command_popup_size(CommandMenu::View, &style)),
            AppPopup::Workspace {
                anchor, is_file, ..
            } => (*anchor, workspace_context_menu_size(*is_file, &style)),
            AppPopup::Editor {
                anchor,
                link,
                table,
            } => (
                *anchor,
                editor_context_menu_size(
                    link.is_some(),
                    table.is_some(),
                    self.document().kind().is_typst(),
                    &style,
                ),
            ),
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
        let frame_margin = menu_frame.total_margin().sum();
        let menu_content_size = frame_content_size(estimated_size, frame_margin);
        let menu_width = menu_content_size.x;
        let menu_height = menu_content_size.y;
        let anchor = if matches!(popup, AppPopup::StatusLog { .. }) {
            clamp_popup_above_anchor(anchor, estimated_size, window_rect.size())
        } else {
            clamp_popup_anchor(anchor, estimated_size, window_rect.size())
        };
        let (can_undo, can_redo) = self.editor_history_availability(context);
        let shortcuts = self.settings.effective_shortcuts();
        let has_selection = self.selected_editor_chars(context).is_some();
        let can_format = self.document().kind().is_typst();
        let can_export_pdf = self.typst_preview_available();
        let can_sync_preview =
            self.document().kind().is_typst() && self.interactive_preview_active();
        let mut close = false;
        let mut action = None;
        let mut had_focus = self.app_popup_had_focus;
        let mut blur_started = self.app_popup_blur_started;
        let popup_generation = self.app_popup_generation;
        let captures = self.captures.clone();
        let is_git_chunk_popup = matches!(&popup, AppPopup::GitChunk { .. });
        let popup_title = if is_git_chunk_popup {
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
            close |= input.close_requested || input.escape_pressed;
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
                    menu_frame.show(ui, |ui| {
                        show_popup_contents(
                            ui,
                            Vec2::new(menu_width, menu_height),
                            popup_generation,
                            |ui| match &popup {
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
                                        can_undo,
                                        can_redo,
                                        can_format,
                                        can_sync_preview,
                                        &shortcuts,
                                        &mut action,
                                    );
                                }
                                AppPopup::View { .. } => {
                                    show_view_popup_ui(
                                        ui,
                                        self.document().kind().is_typst(),
                                        &shortcuts,
                                        &mut action,
                                    );
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
                                AppPopup::Editor { link, table, .. } => {
                                    let mut editor_action = None;
                                    show_editor_context_menu_ui(
                                        ui,
                                        EditorContextMenuOptions {
                                            can_undo,
                                            can_redo,
                                            has_selection,
                                            can_format,
                                            can_sync_preview,
                                            link: link.as_deref(),
                                            table: table.as_ref(),
                                        },
                                        &shortcuts,
                                        &mut editor_action,
                                    );
                                    if let Some(editor_action) = editor_action {
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
                                AppPopup::GitChunk { chunk, .. } => {
                                    if let Some(crate::git::editor::view::Action::RunHunk(
                                        selected,
                                    )) = crate::git::editor::view::show_chunk(
                                        ui,
                                        chunk,
                                        &shortcuts,
                                        self.git_hunk_job.is_running(),
                                        self.settings.git_diff_style,
                                    ) {
                                        action = Some(AppPopupAction::GitHunk(
                                            selected,
                                            self.document().key(),
                                            chunk.clone(),
                                        ));
                                    }
                                }
                            },
                        );
                    });
                });
        });
        self.app_popup_had_focus = had_focus;
        self.app_popup_blur_started = blur_started;

        let action_selected = action.is_some();
        if close || action_selected {
            self.close_app_popup();
            if is_git_chunk_popup {
                self.git_editor.chunk = None;
            }
        } else {
            self.app_popup = Some(popup);
        }
        // A popup losing focus usually means the user activated another app.
        // Only an in-app menu selection should return keyboard focus to the
        // editor; reclaiming it on blur makes tiptoptyp steal activation.
        if action_selected {
            context.send_viewport_cmd(egui::ViewportCommand::Focus);
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
            let focused = context.input(|input| input.viewport().focused);
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
            let shared_navigation = Arc::new(Mutex::new(navigation_state));
            let navigation_handler_state = Arc::clone(&shared_navigation);
            let popup_handler_state = Arc::clone(&shared_navigation);
            let rgba = (
                background.r(),
                background.g(),
                background.b(),
                background.a(),
            );
            let builder = wry::WebViewBuilder::new()
                .with_url(&url)
                .with_bounds(bounds)
                .with_background_color(rgba)
                .with_background_throttling(wry::BackgroundThrottlingPolicy::Disabled)
                // Enable WKWebView/WebView2's platform zoom gestures and
                // standard zoom shortcuts instead of emulating trackpads in
                // the egui layer.
                .with_hotkeys_zoom(true)
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
            let built = if self.window_host.is_root() {
                let Some(window) = frame.and_then(eframe::Frame::winit_window) else {
                    self.preview.webview_state = ServiceState::Degraded(
                        "The native window handle is temporarily unavailable".to_owned(),
                    );
                    return false;
                };
                builder.build_as_child(window.as_ref())
            } else {
                let Some(window) = self.native_window_parent.as_ref() else {
                    self.preview.webview_state = ServiceState::Starting(
                        "Waiting for this document window's native handle".to_owned(),
                    );
                    return false;
                };
                builder.build_as_child(window)
            };
            match built {
                Ok(webview) => {
                    #[cfg(target_os = "macos")]
                    crate::native_window::enable_native_webview_magnification(&webview);
                    self.webview = Some(webview);
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
            if let Some(webview) = &self.webview
                && let Err(error) = webview.load_url(&url)
            {
                self.fail_local_webview(format!("Could not load the Tinymist preview: {error}"));
                return false;
            }
            self.webview_url = Some(url);
            self.webview_reload_pending = false;
        }
        if let Some(webview) = &self.webview {
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
    pub(super) fn hide_webview(&mut self) {
        if self.webview_applied.is_some_and(|applied| !applied.visible) {
            return;
        }
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

#[cfg(test)]
mod property_tests {
    use super::*;

    fn rect(left: f32) -> NativeRect {
        ViewportTransform::new([0.0, 0.0], 1.0)
            .unwrap()
            .to_native(EguiRect::new([left, 0.0], [left + 100.0, 100.0]).unwrap())
            .unwrap()
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
