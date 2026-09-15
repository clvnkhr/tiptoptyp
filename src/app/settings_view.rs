//! Settings and appearance controls.
use super::settings_panel::LUMINOSITY_HINT;
use super::*;

impl EditorApp {
    pub(super) fn show_settings_window(&mut self, context: &egui::Context, frame: &eframe::Frame) {
        if !self.settings_visible
            || self.document_workflow.modal().is_some()
            || self.rename_dialog.is_some()
        {
            return;
        }
        let active_theme = self
            .imported_theme
            .as_ref()
            .map_or(context.theme(), |theme| {
                if theme.dark_mode {
                    egui::Theme::Dark
                } else {
                    egui::Theme::Light
                }
            });
        let style = context.style_of(active_theme);
        let mut close_requested = false;
        let captures = self.captures.clone();
        let spec = ChildViewSpec::persistent(
            "tiptoptyp-settings",
            "tiptoptyp Settings",
            [
                if matches!(
                    self.snapshot_scene,
                    Some(
                        UiSnapshotScene::SettingsColors
                            | UiSnapshotScene::SettingsEditor
                            | UiSnapshotScene::SettingsStatus
                    )
                ) {
                    METRICS.chrome.settings_min_size.x
                } else {
                    METRICS.chrome.settings_width
                },
                METRICS.chrome.settings_height,
            ],
            METRICS.chrome.settings_min_size,
            "settings",
        );
        ChildViewHost::show(
            context,
            &captures,
            spec,
            active_theme,
            &style,
            |ui, input| {
                let settings_tooltip_id = settings_hover_tooltip_id(ui.ctx());
                ui.ctx()
                    .data_mut(|data| data.remove::<HoverTooltipOverlay>(settings_tooltip_id));
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
                    ui.id().with("settings-window-drag"),
                    Sense::drag(),
                );
                if drag.drag_started() {
                    ui.ctx().send_viewport_cmd(egui::ViewportCommand::StartDrag);
                }
                egui::Panel::top("settings-titlebar")
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
                            ui.label(RichText::new("Settings").strong());
                            #[cfg(not(target_os = "macos"))]
                            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                if icon_button(ui, UiIcon::Close, "Close Settings").clicked() {
                                    close_requested = true;
                                }
                            });
                        });
                    });
                egui::CentralPanel::default()
                    .frame(theme::settings_content_frame(ui.style()))
                    .show(ui, |ui| self.show_settings(ui, frame));
                if self.snapshot_scene == Some(UiSnapshotScene::SettingsTooltip) {
                    show_local_tooltip_card(
                        ui.ctx(),
                        Pos2::new(
                            theme::SPACE.content * 2.0,
                            METRICS.chrome.toolbar_height + theme::SPACE.content * 2.0,
                        ),
                        LUMINOSITY_HINT,
                        1.0,
                    );
                } else if let Some(tooltip) = ui
                    .ctx()
                    .data(|data| data.get_temp::<HoverTooltipOverlay>(settings_tooltip_id))
                {
                    show_local_tooltip_card(
                        ui.ctx(),
                        tooltip.anchor,
                        &tooltip.detail,
                        tooltip.opacity,
                    );
                }
            },
        );
        if close_requested {
            self.settings_visible = false;
            self.settings_ui.staged_ui_font_weight = None;
            self.settings_ui.staged_code_font_weight = None;
            // macOS does not consistently reactivate an owned window after a
            // child closes. Return focus to the still-open shortcut window or
            // to the document so the next click is actionable.
            let target = if self.shortcut_editor_visible {
                scoped_child_viewport_id(context, "tiptoptyp-shortcuts")
            } else {
                context.viewport_id()
            };
            context.send_viewport_cmd_to(target, egui::ViewportCommand::Focus);
        }
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

    pub(super) fn show_settings(&mut self, ui: &mut egui::Ui, frame: &eframe::Frame) {
        use super::settings_panel::{SettingsAction, SettingsPanel, SettingsStatus};
        let status = SettingsStatus {
            backend_label: self.preview_backend_label(),
            fallback_reason: self.preview_fallback_reason(),
            requested_backend: self.preview.requested_backend,
            interactive_active: self.interactive_preview_active(),
            tinymist: self.preview.tinymist_state.clone(),
            webview: self.preview.webview_state.clone(),
            compiler: self.compiler_service_state(),
            rasterizer: self.rasterizer_service_state(),
        };
        let project_root = self.project_root();
        let mut actions = Vec::new();
        SettingsPanel {
            state: &mut self.settings_ui,
            settings: &self.settings,
            pending_settings: self.pending_settings.as_ref(),
            theme_override: self.theme_override.as_ref(),
            snapshot_scene: self.snapshot_scene,
            font_catalog: &self.font_catalog,
            font_catalog_scanning: self.font_catalog_scan.is_running(),
            font_configuration: &self.font_configuration,
            typst_tool: &self.typst_tool,
            tinymist_tool: &self.tinymist_tool,
            status,
            project_root: &project_root,
            captures: &self.captures,
            actions: &mut actions,
        }
        .show(ui);
        for action in actions {
            match action {
                SettingsAction::ChooseTool(target) => {
                    self.choose_tool_binary(target, frame, ui.ctx())
                }
                SettingsAction::ShowOverrides { dark } => {
                    self.typst_overrides_dark = dark;
                    self.typst_overrides_visible = true;
                }
                SettingsAction::ShowShortcuts => self.shortcut_editor_visible = true,
                SettingsAction::RefreshTools => self.tool_refresh_requested = true,
                SettingsAction::ShowPackages => {
                    self.settings_visible = false;
                    self.open_package_manager(ui.ctx());
                }
                SettingsAction::RetryTinymist => self.restart_tinymist(),
                SettingsAction::Update(edited) => self.queue_settings(*edited, ui.ctx()),
            }
        }
    }
}
