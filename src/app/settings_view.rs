//! Settings and appearance controls.
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
        let web_link_sender = self.web_link_sender.clone();
        let spec = ChildViewSpec::persistent(
            "tiptoptyp-settings",
            "tiptoptyp Settings",
            [
                METRICS.chrome.settings_width,
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
                            theme::show_logo(ui);
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
                        "Built-in and imported themes share the same semantic colors.",
                        1.0,
                        &web_link_sender,
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
                        &web_link_sender,
                    );
                }
            },
        );
        if close_requested {
            self.settings_visible = false;
            self.staged_ui_font_weight = None;
            self.staged_code_font_weight = None;
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
                        theme::show_logo(ui);
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
        let web_link_sender = self.web_link_sender.clone();
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
                            theme::show_logo(ui);
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
                        &web_link_sender,
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
        if self.snapshot_scene == Some(UiSnapshotScene::SettingsFontPicker) {
            ui.heading("Font selection");
            ui.horizontal(|ui| {
                let id = ui.make_persistent_id(egui::IdSalt::new("ui-font-family"));
                egui::Popup::open_id(ui.ctx(), id.with("popup"));
                show_font_family_picker(
                    ui,
                    "ui-font-family",
                    &self.font_catalog,
                    None,
                    None,
                    "System UI",
                    true,
                );
                ui.vertical(|ui| {
                    if let Some(family) = self.font_catalog.families().first() {
                        ui.label(&family.name);
                        if !crate::font_preview::show(ui, "ui-font-family", family) {
                            self.captures.defer_target("settings");
                        }
                    }
                });
            });
            return;
        }
        ui.set_min_width(ui.available_width());

        ui.horizontal(|ui| {
            ui.label(RichText::new("Search settings").strong());
            ui.add(
                egui::TextEdit::singleline(&mut self.settings_query)
                    .hint_text("Theme, fonts, shortcuts, preview, tools…")
                    .desired_width(f32::INFINITY),
            );
        });
        if !self.settings_query.trim().is_empty() {
            let matches = settings_search_results(&self.settings_query);
            ui.horizontal_wrapped(|ui| {
                ui.label(RichText::new("Matches").weak());
                if matches.is_empty() {
                    ui.label(RichText::new("No settings found").weak());
                }
                for target in matches {
                    let response = ui.button(target.label());
                    let response = settings_hover_text(response, target.section().title());
                    if response.clicked() {
                        self.settings_scroll_target = Some(target);
                    }
                }
            });
            ui.separator();
        }

        let deterministic_settings = matches!(
            self.snapshot_scene,
            Some(
                UiSnapshotScene::SettingsWindow
                    | UiSnapshotScene::SettingsThemePicker
                    | UiSnapshotScene::SettingsDarkThemePicker
                    | UiSnapshotScene::SettingsTooltip
            )
        );
        let mut edited = if deterministic_settings {
            AppSettings::default()
        } else {
            self.pending_settings
                .clone()
                .unwrap_or_else(|| self.settings.clone())
        };
        let settings_scroll = egui::ScrollArea::vertical()
            .id_salt("settings-scroll")
            .auto_shrink([false, false]);
        let settings_scroll = if deterministic_settings {
            // Deterministic scenes describe the complete Settings contract from
            // its first row, independently of persisted egui scroll memory.
            settings_scroll.vertical_scroll_offset(0.0)
        } else {
            settings_scroll
        };
        let mut settings_scroll_target = self.settings_scroll_target.take();
        settings_scroll.show(ui, |ui| {
                settings_heading(ui, SettingsSection::Appearance);
                let system_theme = ui.ctx().system_theme();
                let effective_theme = ui.ctx().theme();
                let theme_overridden = self.theme_override.is_some();
                let theme_picker_enabled = !theme_overridden
                    || matches!(
                        self.snapshot_scene,
                        Some(
                            UiSnapshotScene::SettingsThemePicker
                                | UiSnapshotScene::SettingsDarkThemePicker
                        )
                    );
                settings_target_anchor(
                    ui,
                    SettingsTarget::Appearance,
                    &mut settings_scroll_target,
                );
                ui.horizontal_wrapped(|ui| {
                    theme::apply_compact_control_spacing(ui);
                    ui.add_sized(
                        [
                            METRICS.settings.appearance_label_width,
                            METRICS.icon.button_size.y,
                        ],
                        egui::Label::new(
                            RichText::new(SettingsTarget::Appearance.label()).strong(),
                        ),
                    );
                    ui.add_enabled_ui(theme_picker_enabled, |ui| {
                        for appearance in InterfaceTheme::ALL {
                            ui.selectable_value(
                                &mut edited.interface_theme,
                                appearance,
                                appearance.label(),
                            );
                        }
                    });
                    if theme_overridden {
                        ui.label(
                            RichText::new("QA override")
                                .size(theme::TYPE.supporting)
                                .weak(),
                        );
                    }
                    ui.separator();
                    settings_inline_value(ui, "Active", theme_label(Some(effective_theme)));
                });
                settings_target_anchor(
                    ui,
                    SettingsTarget::TypstSyntax,
                    &mut settings_scroll_target,
                );
                ui.horizontal_wrapped(|ui| {
                    theme::apply_compact_control_spacing(ui);
                    ui.label(RichText::new(SettingsTarget::TypstSyntax.label()).strong());
                    if ui.button("Overrides…").clicked() {
                        self.typst_overrides_dark = effective_theme == egui::Theme::Dark;
                        self.typst_overrides_visible = true;
                    }
                    ui.label(
                        RichText::new("Colours and decorations inherit from the selected theme")
                            .weak(),
                    );
                });

                for (appearance, target, picker_id) in [
                    (
                        egui::Theme::Light,
                        SettingsTarget::LightTheme,
                        "light-color-theme",
                    ),
                    (
                        egui::Theme::Dark,
                        SettingsTarget::DarkTheme,
                        "dark-color-theme",
                    ),
                ] {
                    settings_target_anchor(ui, target, &mut settings_scroll_target);
                    let selected_name = color_theme_choice_label(edited.color_theme(appearance));
                    let imported_path = match edited.color_theme(appearance) {
                        ColorThemeChoice::Sublime(path) => Some(path.clone()),
                        ColorThemeChoice::Builtin(_) => None,
                    };
                    ui.horizontal_wrapped(|ui| {
                        theme::apply_compact_control_spacing(ui);
                        ui.add_sized(
                            [
                                METRICS.settings.appearance_label_width,
                                METRICS.icon.button_size.y,
                            ],
                            egui::Label::new(RichText::new(target.label()).strong()),
                        );
                        ui.add_enabled_ui(theme_picker_enabled, |ui| {
                            let open_picker = matches!(
                                (appearance, self.snapshot_scene),
                                (
                                    egui::Theme::Light,
                                    Some(UiSnapshotScene::SettingsThemePicker)
                                ) | (
                                    egui::Theme::Dark,
                                    Some(UiSnapshotScene::SettingsDarkThemePicker)
                                )
                            );
                            if open_picker {
                                // `ComboBox` stores an already-hashed `IdSalt`, so use
                                // the same representation rather than hashing the raw
                                // string along a different ID path.
                                let id = ui.make_persistent_id(egui::IdSalt::new((
                                    picker_id,
                                    self.snapshot_scene,
                                )));
                                egui::Popup::open_id(ui.ctx(), id.with("popup"));
                            }
                            let choice = edited.color_theme_mut(appearance);
                            let picker = egui::ComboBox::from_id_salt((
                                picker_id,
                                self.snapshot_scene,
                            ))
                                .width(220.0)
                                .height(METRICS.settings.theme_picker_max_height)
                                .selected_text(selected_name)
                                .show_ui(ui, |ui| {
                                    for builtin in builtin_themes::for_mode(
                                        appearance == egui::Theme::Dark,
                                    ) {
                                        let selected = matches!(
                                            choice,
                                            ColorThemeChoice::Builtin(id) if id == builtin.id
                                        );
                                        if ui
                                            .selectable_label(selected, builtin.name)
                                            .clicked()
                                        {
                                            *choice = ColorThemeChoice::builtin(builtin.id);
                                        }
                                    }
                                })
                                .response;
                            if let Some(path) = &imported_path {
                                settings_hover_text(picker, path.clone());
                            }
                            if ui.button("Import…").clicked() {
                                self.choose_tool_binary(
                                    ToolPickerTarget::SublimeTheme {
                                        dark_mode: appearance == egui::Theme::Dark,
                                    },
                                    frame,
                                    ui.ctx(),
                                );
                            }
                        });
                    });
                }

                let (mut displayed_invert, mut displayed_hue_shift) = self
                    .theme_override
                    .as_ref()
                    .map_or((edited.theme_invert, edited.theme_hue_shift_degrees), |profile| {
                        (profile.invert, profile.hue_shift_degrees)
                    });
                settings_target_anchor(
                    ui,
                    SettingsTarget::InvertColors,
                    &mut settings_scroll_target,
                );
                settings_target_anchor(
                    ui,
                    SettingsTarget::HueShift,
                    &mut settings_scroll_target,
                );
                ui.horizontal_wrapped(|ui| {
                    theme::apply_compact_control_spacing(ui);
                    ui.label(RichText::new("Transform").strong());
                    ui.add_enabled_ui(!theme_overridden, |ui| {
                        ui.checkbox(
                            &mut displayed_invert,
                            SettingsTarget::InvertColors.label(),
                        );
                        ui.separator();
                        ui.label(SettingsTarget::HueShift.label());
                        ui.add_sized(
                            [190.0, METRICS.icon.button_size.y],
                            egui::Slider::new(&mut displayed_hue_shift, -180..=180).suffix("°"),
                        );
                        if ui
                            .add_enabled(
                                displayed_invert || displayed_hue_shift != 0,
                                egui::Button::new("Reset"),
                            )
                            .clicked()
                        {
                            displayed_invert = false;
                            displayed_hue_shift = 0;
                        }
                    });
                    ui.label(
                        RichText::new("both themes · invert, then hue")
                            .size(theme::TYPE.supporting)
                            .weak(),
                    );
                });
                if !theme_overridden {
                    edited.theme_invert = displayed_invert;
                    edited.theme_hue_shift_degrees = displayed_hue_shift;
                }
                if !deterministic_settings
                    && edited.interface_theme == InterfaceTheme::System
                    && system_theme.is_none()
                {
                    fallback_notice(
                        ui,
                        "Theme fallback",
                        "System appearance is unavailable; using the configured dark theme",
                    );
                }

                settings_target_anchor(
                    ui,
                    SettingsTarget::PageTheme,
                    &mut settings_scroll_target,
                );
                ui.horizontal_wrapped(|ui| {
                    ui.label(RichText::new(SettingsTarget::PageTheme.label()).strong());
                    for theme in DocumentTheme::ALL {
                        ui.selectable_value(&mut edited.document_theme, theme, theme.label());
                    }
                    ui.separator();
                    settings_inline_value(
                        ui,
                        "Effective",
                        theme_label(Some(edited.document_theme.resolve(effective_theme))),
                    );
                });

                ui.add_space(theme::SPACE.small);
                ui.separator();
                settings_heading(ui, SettingsSection::Editor);
                for target in [
                    SettingsTarget::WrapLines,
                    SettingsTarget::LineNumbers,
                    SettingsTarget::StickyContextRows,
                    SettingsTarget::AutoSave,
                    SettingsTarget::AutoSaveDelay,
                ] {
                    settings_target_anchor(ui, target, &mut settings_scroll_target);
                }
                ui.horizontal_wrapped(|ui| {
                    ui.checkbox(&mut edited.line_wrap, SettingsTarget::WrapLines.label());
                    ui.checkbox(
                        &mut edited.line_numbers,
                        SettingsTarget::LineNumbers.label(),
                    );
                    ui.checkbox(
                        &mut edited.sticky_context_rows,
                        SettingsTarget::StickyContextRows.label(),
                    );
                    ui.checkbox(&mut edited.auto_save, SettingsTarget::AutoSave.label());
                    ui.add_enabled_ui(edited.auto_save, |ui| {
                        ui.label(SettingsTarget::AutoSaveDelay.label());
                        ui.add(
                            egui::Slider::new(&mut edited.auto_save_delay_ms, 250..=5_000)
                                .suffix(" ms")
                                .logarithmic(true),
                        );
                    });
                });
                settings_target_anchor(
                    ui,
                    SettingsTarget::KeyboardShortcuts,
                    &mut settings_scroll_target,
                );
                ui.horizontal_wrapped(|ui| {
                    theme::apply_compact_control_spacing(ui);
                    ui.label(RichText::new("Keyboard").strong());
                    if ui
                        .button(SettingsTarget::KeyboardShortcuts.label())
                        .clicked()
                    {
                        self.shortcut_editor_visible = true;
                    }
                    ui.label(
                        RichText::new("All application, editor, build, preview, and window bindings")
                            .size(theme::TYPE.supporting)
                        .weak(),
                    );
                });
                settings_target_anchor(
                    ui,
                    SettingsTarget::InterfaceScale,
                    &mut settings_scroll_target,
                );
                settings_target_anchor(
                    ui,
                    SettingsTarget::TitleBarMenus,
                    &mut settings_scroll_target,
                );
                ui.horizontal_wrapped(|ui| {
                    theme::apply_compact_control_spacing(ui);
                    ui.label(RichText::new(SettingsTarget::InterfaceScale.label()).strong());
                    ui.add(
                        egui::Slider::new(&mut edited.ui_scale_percent, 75..=150)
                            .suffix("%")
                            .clamping(egui::SliderClamping::Always),
                    );
                    ui.separator();
                    ui.checkbox(
                        &mut edited.titlebar_menus,
                        SettingsTarget::TitleBarMenus.label(),
                    );
                });
                settings_target_anchor(ui, SettingsTarget::UiFont, &mut settings_scroll_target);
                settings_target_anchor(
                    ui,
                    SettingsTarget::UiFontWeight,
                    &mut settings_scroll_target,
                );
                ui.horizontal_wrapped(|ui| {
                    theme::apply_compact_control_spacing(ui);
                    ui.label(RichText::new(SettingsTarget::UiFont.label()).strong());
                    if let Some(selection) = show_font_family_picker(
                        ui,
                        "ui-font-family",
                        &self.font_catalog,
                        edited.ui_font_path.as_deref(),
                        edited.ui_font_family.as_deref(),
                        if edited.ui_font_monospace {
                            "Editor font"
                        } else {
                            "System UI"
                        },
                        true,
                    ) {
                        match selection {
                            FontPickerSelection::Default => {
                                edited.ui_font_path = None;
                                edited.ui_font_family = None;
                                edited.ui_font_face_index = 0;
                                edited.ui_font_monospace = false;
                            }
                            FontPickerSelection::Editor => {
                                edited.ui_font_path = None;
                                edited.ui_font_family = None;
                                edited.ui_font_face_index = 0;
                                edited.ui_font_monospace = true;
                            }
                            FontPickerSelection::Family {
                                name,
                                path,
                                face_index,
                            } => {
                                edited.ui_font_path = Some(path);
                                edited.ui_font_family = Some(name);
                                edited.ui_font_face_index = face_index;
                                edited.ui_font_monospace = false;
                            }
                        }
                    }
                    if ui.button("Choose…").clicked() {
                        self.choose_tool_binary(ToolPickerTarget::UiFont, frame, ui.ctx());
                    }
                    ui.separator();
                    show_font_weight_control(
                        ui,
                        "ui-font-weight",
                        &mut edited.ui_font_weight,
                        &mut self.staged_ui_font_weight,
                        self.font_configuration.ui_weight_support.as_ref(),
                    );
                });
                settings_target_anchor(ui, SettingsTarget::CodeFont, &mut settings_scroll_target);
                settings_target_anchor(
                    ui,
                    SettingsTarget::CodeFontWeight,
                    &mut settings_scroll_target,
                );
                ui.horizontal_wrapped(|ui| {
                    theme::apply_compact_control_spacing(ui);
                    ui.label(RichText::new(SettingsTarget::CodeFont.label()).strong());
                    if let Some(selection) = show_font_family_picker(
                        ui,
                        "code-font-family",
                        &self.font_catalog,
                        edited.code_font_path.as_deref(),
                        edited.code_font_family.as_deref(),
                        "System monospace",
                        false,
                    ) {
                        match selection {
                            FontPickerSelection::Default | FontPickerSelection::Editor => {
                                edited.code_font_path = None;
                                edited.code_font_family = None;
                                edited.code_font_face_index = 0;
                            }
                            FontPickerSelection::Family {
                                name,
                                path,
                                face_index,
                            } => {
                                edited.code_font_path = Some(path);
                                edited.code_font_family = Some(name);
                                edited.code_font_face_index = face_index;
                            }
                        }
                    }
                    if ui.button("Choose…").clicked() {
                        self.choose_tool_binary(ToolPickerTarget::CodeFont, frame, ui.ctx());
                    }
                    ui.separator();
                    show_font_weight_control(
                        ui,
                        "code-font-weight",
                        &mut edited.code_font_weight,
                        &mut self.staged_code_font_weight,
                        self.font_configuration.code_weight_support.as_ref(),
                    );
                    if self.font_catalog_scan.is_running() {
                        ui.label(RichText::new("Scanning fonts…").weak());
                    } else if !self.font_catalog.workspace_directories().is_empty() {
                        ui.label(
                            RichText::new(format!(
                                "{} workspace font folder{}",
                                self.font_catalog.workspace_directories().len(),
                                if self.font_catalog.workspace_directories().len() == 1 {
                                    ""
                                } else {
                                    "s"
                                }
                            ))
                            .weak(),
                        );
                    }
                });
                settings_target_anchor(
                    ui,
                    SettingsTarget::PreviewJump,
                    &mut settings_scroll_target,
                );
                settings_target_anchor(
                    ui,
                    SettingsTarget::HoverDelay,
                    &mut settings_scroll_target,
                );
                settings_target_anchor(
                    ui,
                    SettingsTarget::HoverFade,
                    &mut settings_scroll_target,
                );
                ui.horizontal_wrapped(|ui| {
                    theme::apply_compact_control_spacing(ui);
                    ui.label(RichText::new(SettingsTarget::PreviewJump.label()).strong());
                    let trigger = egui::ComboBox::from_id_salt("source-preview-trigger")
                        .width(METRICS.settings.source_preview_trigger_width)
                        .selected_text(edited.source_preview_trigger.label())
                        .show_ui(ui, |ui| {
                            for trigger in SourcePreviewTrigger::ALL {
                                settings_hover_text(
                                    ui.selectable_value(
                                        &mut edited.source_preview_trigger,
                                        trigger,
                                        trigger.label(),
                                    ),
                                    trigger.description(),
                                );
                            }
                        })
                        .response;
                    settings_hover_text(trigger, edited.source_preview_trigger.description());
                    ui.separator();
                    ui.label(RichText::new("Hovers").strong());
                    ui.label(SettingsTarget::HoverDelay.label());
                    ui.add(
                        egui::DragValue::new(&mut edited.hover_delay_ms)
                            .range(0..=2_000)
                            .speed(10)
                            .suffix(" ms"),
                    );
                    ui.label(SettingsTarget::HoverFade.label());
                    ui.add(
                        egui::DragValue::new(&mut edited.hover_fade_ms)
                            .range(0..=500)
                            .speed(5)
                            .suffix(" ms"),
                    );
                });

                ui.add_space(theme::SPACE.small);
                ui.separator();
                settings_heading(ui, SettingsSection::Tools);
                ui.label(
                    RichText::new(
                        "Packaged builds use pinned sidecars. A custom path overrides one tool without changing the other.",
                    )
                    .size(theme::TYPE.supporting)
                    .color(ui.visuals().weak_text_color()),
                );
                let staged_typst = if edited.typst == self.settings.typst {
                    self.typst_tool.clone()
                } else {
                    resolve_tool(ToolKind::Typst, &edited.typst)
                };
                settings_target_anchor(
                    ui,
                    SettingsTarget::TypstCompiler,
                    &mut settings_scroll_target,
                );
                if tool_preference_editor(
                    ui,
                    SettingsTarget::TypstCompiler.label(),
                    &mut edited.typst,
                    &staged_typst,
                    deterministic_settings,
                ) {
                    self.choose_tool_binary(ToolPickerTarget::Typst, frame, ui.ctx());
                }
                if !deterministic_settings
                    && let Some(reason) = &staged_typst.fallback_reason
                {
                    fallback_notice(ui, "Binary fallback active", reason);
                }
                ui.add_space(METRICS.settings.tool_gap);
                let staged_tinymist = if edited.tinymist == self.settings.tinymist {
                    self.tinymist_tool.clone()
                } else {
                    resolve_tool(ToolKind::Tinymist, &edited.tinymist)
                };
                settings_target_anchor(
                    ui,
                    SettingsTarget::TinymistLanguageServer,
                    &mut settings_scroll_target,
                );
                if tool_preference_editor(
                    ui,
                    SettingsTarget::TinymistLanguageServer.label(),
                    &mut edited.tinymist,
                    &staged_tinymist,
                    deterministic_settings,
                ) {
                    self.choose_tool_binary(ToolPickerTarget::Tinymist, frame, ui.ctx());
                }
                if !deterministic_settings
                    && let Some(reason) = &staged_tinymist.fallback_reason
                {
                    fallback_notice(ui, "Binary fallback active", reason);
                }
                settings_target_anchor(
                    ui,
                    SettingsTarget::RefreshBinaryStatus,
                    &mut settings_scroll_target,
                );
                settings_target_anchor(
                    ui,
                    SettingsTarget::BrowseTypstPackages,
                    &mut settings_scroll_target,
                );
                if ui
                    .button(SettingsTarget::RefreshBinaryStatus.label())
                    .clicked()
                {
                    self.tool_refresh_requested = true;
                    ui.ctx().request_repaint();
                }
                if ui
                    .button(SettingsTarget::BrowseTypstPackages.label())
                    .clicked()
                {
                    self.settings_visible = false;
                    self.open_package_manager(ui.ctx());
                }

                ui.add_space(theme::SPACE.small);
                ui.separator();
                settings_heading(ui, SettingsSection::Preview);
                settings_target_anchor(
                    ui,
                    SettingsTarget::PreviewBackend,
                    &mut settings_scroll_target,
                );
                ui.horizontal_wrapped(|ui| {
                    for preference in PreviewPreference::ALL {
                        ui.selectable_value(
                            &mut edited.preview_preference,
                            preference,
                            preference.label(),
                        );
                    }
                    ui.separator();
                    settings_inline_value(
                        ui,
                        "Effective",
                        if deterministic_settings {
                            "Interactive"
                        } else {
                            self.preview_backend_label()
                        },
                    );
                });
                if !deterministic_settings
                    && let Some(reason) = self.preview_fallback_reason()
                {
                    fallback_notice(ui, "Preview fallback active", &reason);
                }
                if !deterministic_settings
                    && self.preview.requested_backend == PreviewPreference::Interactive
                    && !self.interactive_preview_active()
                    && ui.button("Retry Tinymist").clicked()
                {
                    self.restart_tinymist();
                }

                ui.add_space(theme::SPACE.small);
                ui.separator();
                settings_heading(ui, SettingsSection::Status);
                settings_target_anchor(
                    ui,
                    SettingsTarget::ToolchainStatus,
                    &mut settings_scroll_target,
                );
                ui.horizontal_wrapped(|ui| {
                    let syntax_color = success_color(ui.ctx());
                    if deterministic_settings {
                        show_status_chip(ui, "Typst", "Bundled", "Packaged compiler", syntax_color);
                        show_status_chip(
                            ui,
                            "Tinymist",
                            "Bundled",
                            "Packaged language server",
                            syntax_color,
                        );
                        for name in ["LSP", "Vector", "Watcher", "PDF"] {
                            show_status_chip(ui, name, "Ready", "Ready", syntax_color);
                        }
                    } else {
                        show_tool_status_chip(ui, "Typst", &self.typst_tool);
                        show_tool_status_chip(ui, "Tinymist", &self.tinymist_tool);
                        show_service_status_chip(ui, "LSP", &self.preview.tinymist_state);
                        show_service_status_chip(ui, "Vector", &self.preview.webview_state);
                        show_service_status_chip(ui, "Watcher", &self.compiler_service_state());
                        show_service_status_chip(ui, "PDF", &self.rasterizer_service_state());
                    }
                    show_status_chip(
                        ui,
                        "Syntax",
                        "Ready",
                        "typst-syntax (official parser)",
                        syntax_color,
                    );
                });
                settings_target_anchor(
                    ui,
                    SettingsTarget::ProjectRoot,
                    &mut settings_scroll_target,
                );
                settings_value_row(
                    ui,
                    SettingsTarget::ProjectRoot.label(),
                    &if deterministic_settings {
                        "Theme gallery workspace".to_owned()
                    } else {
                        self.project_root().display().to_string()
                    },
                );
                settings_target_anchor(
                    ui,
                    SettingsTarget::UiScreenshots,
                    &mut settings_scroll_target,
                );
                ui.horizontal_wrapped(|ui| {
                    ui.label(RichText::new(SettingsTarget::UiScreenshots.label()).strong());
                    if ui.button("Main").clicked() {
                        self.captures.queue("main", "main");
                    }
                    if ui.button("Settings").clicked() {
                        self.captures.queue("settings", "settings");
                    }
                    if ui.button("Both").clicked() {
                        self.captures.queue("main", "main");
                        self.captures.queue("settings", "settings");
                    }
                    let screenshot_shortcut = edited
                        .effective_shortcuts()
                        .display(ShortcutAction::CaptureUi)
                        .unwrap_or_else(|| "Unassigned".to_owned());
                    settings_hover_text(
                        ui.label(
                            RichText::new(screenshot_shortcut)
                                .size(theme::TYPE.supporting)
                                .weak(),
                        ),
                        format!(
                            "App-window-only PNGs are saved under {}",
                            self.captures.output_directory().display()
                        ),
                    );
                });
            });

        self.settings_scroll_target = settings_scroll_target;

        if !deterministic_settings {
            self.queue_settings(edited, ui.ctx());
        }
    }
}
