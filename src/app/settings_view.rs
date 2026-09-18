//! Settings and appearance controls.
use super::*;

impl EditorApp {
    pub(super) fn consume_settings_actions(
        &mut self,
        context: &egui::Context,
        frame: Option<&eframe::Frame>,
    ) {
        let close = if self.settings_window.lock().unwrap().has_actions() {
            let original = self.settings_snapshot();
            let mut edited = original.clone();
            let (actions, close) = self
                .settings_window
                .lock()
                .unwrap()
                .take_actions(&mut edited);
            if edited != original {
                self.queue_settings(edited, context);
            }
            self.apply_settings_actions(actions, context, frame);
            close
        } else {
            false
        };
        if close {
            self.settings_visible = false;
            // Restore focus after native close without waking the owner for
            // ordinary child pointer/scroll frames.
            let target = if self.shortcut_editor_visible {
                scoped_child_viewport_id(context, "tiptoptyp-shortcuts")
            } else {
                context.viewport_id()
            };
            if self.lifecycle.allows_document_work() || self.shortcut_editor_visible {
                context.send_viewport_cmd_to(target, egui::ViewportCommand::Focus);
            }
        }
    }

    pub(super) fn show_settings_window(
        &mut self,
        context: &egui::Context,
        frame: Option<&eframe::Frame>,
    ) {
        use super::{settings_panel::SettingsStatus, settings_window::SettingsWindowInput};
        self.consume_settings_actions(context, frame);
        // The dormant host's surface must survive the first resumed frame,
        // including modal dialogs. Hiding is safe; destroying its current CGL
        // view before eframe switches surfaces is not.
        let retain = self.retain_settings_viewport || !self.lifecycle.allows_document_work();
        let visible = self.settings_visible
            && self.document_workflow.modal().is_none()
            && self.rename_dialog.is_none();
        if !visible && !retain {
            self.settings_window.lock().unwrap().suspend();
            return;
        }
        if !visible
            && retain
            && SettingsWindow::retain_hidden(&self.settings_window, context, &self.captures)
        {
            return;
        }
        let appearance = self
            .imported_theme
            .as_ref()
            .map_or(context.theme(), |theme| {
                if theme.dark_mode {
                    egui::Theme::Dark
                } else {
                    egui::Theme::Light
                }
            });
        let capabilities = self
            .capabilities
            .snapshot(crate::capabilities::CapabilityInputs {
                typst: self.typst_tool.clone(),
                tinymist: self.tinymist_tool.clone(),
                lsp: self.preview.tinymist_state.clone(),
                interactive_preview: self.preview.webview_state.clone(),
                pdf_generation: self.compiler_service_state(),
                rasterization: self.rasterizer_service_state(),
                interactive_preview_supported: cfg!(any(
                    target_os = "macos",
                    target_os = "windows"
                )),
            });
        let input = SettingsWindowInput {
            visible,
            retain_when_closed: retain,
            settings: self.settings_snapshot(),
            theme_override: self.theme_override.clone(),
            snapshot_scene: self.snapshot_scene,
            font_catalog_revision: self.font_catalog_revision,
            font_catalog_scanning: self.font_catalog_scan.is_running(),
            font_configuration: self.font_configuration.clone(),
            typst_tool: self.typst_tool.clone(),
            tinymist_tool: self.tinymist_tool.clone(),
            status: SettingsStatus {
                backend_label: self.preview_backend_label(),
                fallback_reason: self.preview_fallback_reason(),
                requested_backend: self.preview.requested_backend,
                interactive_active: self.interactive_preview_active(),
                capabilities,
            },
            project_root: self.project_root(),
            appearance,
            style: context.style_of(appearance),
        };
        SettingsWindow::show(
            &self.settings_window,
            context,
            &self.captures,
            input,
            &self.font_catalog,
        );
    }

    pub(super) fn show_shortcut_editor_window(&mut self, context: &egui::Context) {
        if !self.shortcut_editor_visible
            || self.document_workflow.modal().is_some()
            || self.rename_dialog.is_some()
        {
            return;
        }

        let appearance = context.theme();
        let style = context.style_of(appearance);
        let captures = self.captures.clone();
        let mut close_requested = false;
        // The editor owns its own native viewport so shortcut capture and
        // text editing cannot be trapped inside the Settings surface.
        let spec = ChildViewSpec::persistent(
            "tiptoptyp-shortcuts",
            "tiptoptyp Keyboard shortcuts",
            [620.0, 540.0],
            [440.0, 300.0],
            "shortcuts",
        );
        ChildViewHost::show(context, &captures, spec, appearance, &style, |ui, input| {
            close_requested |= input.close_requested;
            if ui.ctx().input(|input| input.viewport().fullscreen) == Some(true) {
                ui.ctx()
                    .send_viewport_cmd(egui::ViewportCommand::Fullscreen(false));
            }
            ui.painter()
                .rect_filled(ui.max_rect(), 0.0, ui.visuals().panel_fill);
            let title_rect = Rect::from_min_size(
                ui.max_rect().min,
                egui::vec2(ui.max_rect().width(), METRICS.chrome.toolbar_height),
            );
            let drag = ui.interact(
                title_rect,
                ui.id().with("shortcut-window-drag"),
                Sense::drag(),
            );
            if drag.drag_started() {
                ui.ctx().send_viewport_cmd(egui::ViewportCommand::StartDrag);
            }
            egui::Panel::top("shortcut-titlebar")
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
                        ui.label(RichText::new("Keyboard shortcuts").strong());
                        #[cfg(not(target_os = "macos"))]
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            if icon_button(ui, UiIcon::Close, "Close Keyboard shortcuts").clicked()
                            {
                                close_requested = true;
                            }
                        });
                    });
                });
            egui::CentralPanel::default()
                .frame(theme::settings_content_frame(ui.style()))
                .show(ui, |ui| {
                    show_shortcut_editor_contents(
                        ui,
                        &mut self.shortcut_query,
                        &mut self.shortcut_capture,
                        &mut self.shortcut_notice,
                        &mut self.pending_settings,
                        &self.settings,
                    );
                });
        });
        if close_requested {
            self.shortcut_editor_visible = false;
            self.shortcut_capture = None;
            let target = if self.settings_visible {
                scoped_child_viewport_id(context, "tiptoptyp-settings")
            } else {
                context.viewport_id()
            };
            context.send_viewport_cmd_to(target, egui::ViewportCommand::Focus);
        }
    }

    pub(super) fn show_typst_overrides_window(&mut self, context: &egui::Context) {
        if !self.typst_overrides_visible
            || self.document_workflow.modal().is_some()
            || self.rename_dialog.is_some()
            || self.pending_tool_picker.is_some()
        {
            return;
        }

        let deterministic = self.snapshot_scene == Some(UiSnapshotScene::TypstOverridesWindow);
        let rendered_dark = self.typst_overrides_dark;
        let mut selected_dark = rendered_dark;
        let mut edited = if deterministic {
            AppSettings::default()
        } else {
            self.pending_settings
                .clone()
                .unwrap_or_else(|| self.settings.clone())
        };
        if deterministic {
            let overrides = edited.typst_overrides.for_dark_mut(rendered_dark);
            overrides
                .get_mut_or_default(TypstSyntaxRole::Function)
                .weight = Some(theme::FONT_WEIGHT_BOLD);
        }

        let request = theme_request_for_appearance(&edited, rendered_dark);
        let (preview_theme, fallback) = self
            .imported_theme
            .as_ref()
            .filter(|theme| theme.dark_mode == rendered_dark)
            .map_or_else(
                || load_active_theme_or_fallback(&request),
                |theme| (theme.clone(), None),
            );
        if deterministic {
            let accent = preview_theme.palette.accent;
            let overrides = edited.typst_overrides.for_dark_mut(rendered_dark);
            overrides
                .get_mut_or_default(TypstSyntaxRole::Keyword)
                .foreground = Some(accent);
            overrides
                .get_mut_or_default(TypstSyntaxRole::Raw)
                .background = Some(Rgba {
                a: METRICS.syntax.override_sample_background_alpha,
                ..accent
            });
        }
        let appearance = if preview_theme.dark_mode {
            egui::Theme::Dark
        } else {
            egui::Theme::Light
        };
        let style = theme::style_for_semantic_palette(
            context.style_of(appearance).as_ref(),
            preview_theme.dark_mode,
            preview_theme.palette,
        );
        let native_theme = theme::native_theme(appearance);
        let syntax_palette = theme::syntax_palette_from_semantic(preview_theme.palette);
        let original_overrides = edited.typst_overrides.clone();
        let captures = self.captures.clone();
        let mut close_requested = false;

        let viewport = child_viewport_builder(
            egui::ViewportBuilder::default()
                .with_title("tiptoptyp Typst Overrides")
                .with_inner_size([
                    METRICS.chrome.typst_overrides_width,
                    METRICS.chrome.typst_overrides_height,
                ])
                .with_min_inner_size(METRICS.chrome.typst_overrides_min_size)
                .with_fullsize_content_view(true)
                .with_title_shown(false)
                .with_titlebar_shown(false)
                .with_maximize_button(false)
                .with_maximized(false)
                .with_fullscreen(false),
        );
        crate::viewport_fonts::show_immediate(
            context,
            scoped_child_viewport_id(context, "tiptoptyp-typst-overrides"),
            viewport,
            |ui, _class| {
                captures.begin_viewport(ui.ctx(), "typst-overrides");
                ui.set_style(style.clone());
                ui.ctx()
                    .send_viewport_cmd(egui::ViewportCommand::SetTheme(native_theme));
                let overrides_tooltip_id = typst_overrides_hover_tooltip_id(ui.ctx());
                ui.ctx()
                    .data_mut(|data| data.remove::<HoverTooltipOverlay>(overrides_tooltip_id));
                close_requested |= ui.ctx().input(|input| input.viewport().close_requested());
                if ui.ctx().input(|input| input.viewport().fullscreen) == Some(true) {
                    ui.ctx()
                        .send_viewport_cmd(egui::ViewportCommand::Fullscreen(false));
                }
                ui.painter()
                    .rect_filled(ui.max_rect(), 0.0, ui.visuals().panel_fill);

                let title_rect = Rect::from_min_size(
                    ui.max_rect().min,
                    egui::vec2(ui.max_rect().width(), METRICS.chrome.toolbar_height),
                );
                let drag = ui.interact(
                    title_rect,
                    ui.id().with("typst-overrides-window-drag"),
                    Sense::drag(),
                );
                if drag.drag_started() {
                    ui.ctx().send_viewport_cmd(egui::ViewportCommand::StartDrag);
                }
                egui::Panel::top("typst-overrides-titlebar")
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
                            ui.label(RichText::new("Typst overrides").strong());
                            #[cfg(not(target_os = "macos"))]
                            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                if icon_button(ui, UiIcon::Close, "Close Typst overrides").clicked()
                                {
                                    close_requested = true;
                                }
                            });
                        });
                    });
                egui::CentralPanel::default()
                    .frame(theme::settings_content_frame(ui.style()))
                    .show(ui, |ui| {
                        ui.horizontal_wrapped(|ui| {
                            theme::apply_compact_control_spacing(ui);
                            ui.label(RichText::new("Appearance").strong());
                            ui.selectable_value(&mut selected_dark, false, "Light");
                            ui.selectable_value(&mut selected_dark, true, "Dark");
                            ui.separator();
                            if ui
                                .add_enabled(
                                    !edited.typst_overrides.for_dark(rendered_dark).is_empty(),
                                    egui::Button::new("Reset this appearance"),
                                )
                                .clicked()
                            {
                                edited.typst_overrides.for_dark_mut(rendered_dark).clear();
                            }
                            ui.label(
                                RichText::new("Unset fields inherit the selected theme")
                                    .size(theme::TYPE.supporting)
                                    .weak(),
                            );
                        });
                        if let Some(reason) = &fallback {
                            fallback_notice(ui, "Theme fallback", reason);
                        }
                        ui.add_space(theme::SPACE.small);
                        let scroll_area = egui::ScrollArea::both()
                            .id_salt("typst-overrides-scroll")
                            .auto_shrink([false, false]);
                        let scroll_area = if deterministic {
                            // Snapshot scenes must not inherit the persisted scroll memory of
                            // an earlier interactive window. Otherwise a fresh capture can begin
                            // halfway through the table and hide the headers being verified.
                            scroll_area.scroll_offset(Vec2::ZERO)
                        } else {
                            scroll_area
                        };
                        scroll_area.show(ui, |ui| {
                            show_typst_override_editor(
                                ui,
                                edited.typst_overrides.for_dark_mut(rendered_dark),
                                syntax_palette,
                                &preview_theme.syntect_theme,
                                &self.font_configuration.editor_weight_support,
                            );
                        });
                    });
                if let Some(tooltip) = ui
                    .ctx()
                    .data(|data| data.get_temp::<HoverTooltipOverlay>(overrides_tooltip_id))
                {
                    show_local_tooltip_card(
                        ui.ctx(),
                        tooltip.anchor,
                        &tooltip.detail,
                        tooltip.opacity,
                    );
                }
                captures.end_glow_viewport(ui, "typst-overrides");
            },
        );

        self.typst_overrides_dark = selected_dark;
        if close_requested {
            self.typst_overrides_visible = false;
        }
        if !deterministic && edited.typst_overrides != original_overrides {
            self.queue_settings(edited, context);
        }
    }

    fn apply_settings_actions(
        &mut self,
        actions: Vec<super::settings_panel::SettingsAction>,
        context: &egui::Context,
        frame: Option<&eframe::Frame>,
    ) {
        use super::settings_panel::SettingsAction;
        for action in actions {
            match action {
                SettingsAction::ChooseTool(target) => {
                    self.choose_tool_binary(target, frame, context)
                }
                SettingsAction::ShowOverrides { dark } => {
                    self.typst_overrides_dark = dark;
                    self.typst_overrides_visible = true;
                }
                SettingsAction::ShowShortcuts => self.shortcut_editor_visible = true,
                SettingsAction::RefreshTools => self.tool_refresh_requested = true,
                SettingsAction::ShowPackages => {
                    self.settings_visible = false;
                    self.open_package_manager(context);
                }
                SettingsAction::RetryTinymist => self.restart_tinymist(),
                SettingsAction::Update(edited) => self.queue_settings(*edited, context),
            }
        }
    }
}
