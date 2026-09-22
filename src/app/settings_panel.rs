//! Settings renderer: borrowed presentation inputs and explicit application actions.
//! This module cannot access the document, worker handles, or EditorApp.
use super::settings_controls::{
    FontPickerSelection, color_theme_choice_label, fallback_notice, settings_inline_value,
    settings_value_row, show_font_family_picker, show_font_weight_control,
    show_service_status_chip, show_status_chip, show_tool_status_chip, tool_preference_editor,
};
use super::{
    SettingsSection, SettingsTarget, ToolPickerTarget, settings_heading, settings_hover_text,
    settings_search_results, settings_target_anchor, success_color, theme_label,
};
use crate::{
    builtin_themes,
    capabilities::CapabilitySnapshot,
    explorer::{ExplorerOrder, ExplorerSection},
    font_catalog::FontCatalog,
    screenshot::{CaptureController, CaptureThemeProfile, UiSnapshotScene},
    settings::{
        AppSettings, ColorThemeChoice, DocumentTheme, GitDiffStyle, InterfaceTheme,
        PreviewPreference, SourcePreviewTrigger,
    },
    shortcuts::ShortcutAction,
    theme::{self, METRICS},
    toolchain::ToolResolution,
};
use eframe::egui::{self, Align, Layout, Rect, RichText, Vec2};
use std::path::Path;

#[derive(Default)]
pub(super) struct SettingsUiState {
    pub(super) query: String,
    pub(super) scroll_target: Option<SettingsTarget>,
    pub(super) staged_ui_font_weight: Option<u16>,
    pub(super) staged_code_font_weight: Option<u16>,
}

pub(super) enum SettingsAction {
    ChooseTool(ToolPickerTarget),
    ShowOverrides { dark: bool },
    ShowShortcuts,
    RefreshTools,
    ShowPackages,
    RetryTinymist,
    Update(Box<AppSettings>),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct SettingsStatus {
    pub(super) backend_label: &'static str,
    pub(super) fallback_reason: Option<String>,
    pub(super) requested_backend: PreviewPreference,
    pub(super) interactive_active: bool,
    pub(super) capabilities: CapabilitySnapshot,
}

pub(super) struct SettingsPanel<'a> {
    pub(super) state: &'a mut SettingsUiState,
    pub(super) settings: &'a AppSettings,
    pub(super) pending_settings: Option<&'a AppSettings>,
    pub(super) theme_override: Option<&'a CaptureThemeProfile>,
    pub(super) snapshot_scene: Option<UiSnapshotScene>,
    pub(super) font_catalog: &'a FontCatalog,
    pub(super) font_catalog_scanning: bool,
    pub(super) font_configuration: &'a theme::FontConfiguration,
    pub(super) typst_tool: &'a ToolResolution,
    pub(super) tinymist_tool: &'a ToolResolution,
    pub(super) status: SettingsStatus,
    pub(super) project_root: &'a Path,
    pub(super) captures: &'a CaptureController,
    pub(super) actions: &'a mut Vec<SettingsAction>,
}
impl SettingsPanel<'_> {
    pub(super) fn show(&mut self, ui: &mut egui::Ui) {
        let _span = crate::performance::span("ui.settings");
        if self.snapshot_scene == Some(UiSnapshotScene::SettingsFontPicker) {
            ui.heading("Font selection");
            ui.horizontal(|ui| {
                let id = ui.make_persistent_id(egui::IdSalt::new("ui-font-family"));
                egui::Popup::open_id(ui.ctx(), id.with("popup"));
                show_font_family_picker(
                    ui,
                    "ui-font-family",
                    self.font_catalog,
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
                egui::TextEdit::singleline(&mut self.state.query)
                    .hint_text("Theme, fonts, shortcuts, preview, tools…")
                    .desired_width(f32::INFINITY),
            );
        });
        if !self.state.query.trim().is_empty() {
            let matches = settings_search_results(&self.state.query);
            ui.horizontal_wrapped(|ui| {
                ui.label(RichText::new("Matches").weak());
                if matches.is_empty() {
                    ui.label(RichText::new("No settings found").weak());
                }
                for target in matches {
                    let response = ui.button(target.label());
                    if response.clicked() {
                        self.state.scroll_target = Some(target);
                    }
                }
            });
            ui.separator();
        }

        let deterministic_settings = matches!(
            self.snapshot_scene,
            Some(
                UiSnapshotScene::SettingsWindow
                    | UiSnapshotScene::SettingsColors
                    | UiSnapshotScene::SettingsEditor
                    | UiSnapshotScene::SettingsStatus
                    | UiSnapshotScene::BracketSettings
                    | UiSnapshotScene::SettingsThemePicker
                    | UiSnapshotScene::SettingsDarkThemePicker
                    | UiSnapshotScene::SettingsTooltip
            )
        );
        if deterministic_settings {
            ui.style_mut().scroll_animation = egui::style::ScrollAnimation::none();
        }
        let original = self.pending_settings.unwrap_or(self.settings);
        let mut edited = if deterministic_settings {
            AppSettings::default()
        } else {
            original.clone()
        };
        let settings_scroll = egui::ScrollArea::vertical()
            .id_salt("settings-scroll")
            .auto_shrink([false, false]);
        let settings_scroll = if deterministic_settings
            && !matches!(
                self.snapshot_scene,
                Some(
                    UiSnapshotScene::BracketSettings
                        | UiSnapshotScene::SettingsColors
                        | UiSnapshotScene::SettingsEditor
                        | UiSnapshotScene::SettingsStatus
                )
            ) {
            // Deterministic scenes describe the complete Settings contract from
            // its first row, independently of persisted egui scroll memory.
            settings_scroll.vertical_scroll_offset(0.0)
        } else {
            settings_scroll
        };
        let mut settings_scroll_target = self.state.scroll_target.take();
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
                        self.actions.push(SettingsAction::ShowOverrides { dark: effective_theme == egui::Theme::Dark });
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
                                    show_theme_choices(ui, choice, appearance);
                                })
                                .response;
                            if let Some(path) = &imported_path {
                                settings_hover_text(picker, path.clone());
                            }
                            if ui.button("Import…").clicked() {
                                self.actions.push(SettingsAction::ChooseTool(
                                    ToolPickerTarget::SublimeTheme {
                                        dark_mode: appearance == egui::Theme::Dark,
                                    }));
                            }
                        });
                    });
                }

                let (displayed_invert, displayed_hue_shift) = self
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
                settings_target_anchor(ui, SettingsTarget::ThemeColors, &mut settings_scroll_target);
                // Capture themes intentionally ignore persisted color adjustments.
                let colors = if theme_overridden { Default::default() } else { edited.theme_colors };
                let mut transform = crate::theme_transform::ThemeTransform::new(
                    displayed_invert, f32::from(displayed_hue_shift),
                ).with_colors(colors);
                ui.add_enabled_ui(!theme_overridden, |ui| show_theme_color_controls(ui, &mut transform));
                if !theme_overridden {
                    edited.theme_invert = transform.invert;
                    edited.theme_hue_shift_degrees = transform.hue_shift_degrees as i16;
                    edited.theme_colors = transform.colors;
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
                        settings_slider_enabled(ui, SettingsTarget::AutoSaveDelay.label(), edited.auto_save,
                            egui::Slider::new(&mut edited.auto_save_delay_ms, 250..=5_000)
                                .suffix(" ms")
                                .logarithmic(true),
                        );
                });
                settings_target_anchor(ui, SettingsTarget::AutoPairDelimiters, &mut settings_scroll_target);
                ui.checkbox(&mut edited.auto_pair_delimiters, SettingsTarget::AutoPairDelimiters.label());
                settings_target_anchor(ui, SettingsTarget::MitexDollars, &mut settings_scroll_target);
                ui.checkbox(&mut edited.mitex_auto_enable, SettingsTarget::MitexDollars.label());
                ui.horizontal_wrapped(|ui| {
                    ui.label("MiTeX version");
                    ui.add(egui::TextEdit::singleline(&mut edited.mitex_version).desired_width(65.0))
                        .on_hover_text("Pinned package version used when enabling TeX mode. Existing imports are not rewritten.");
                });
                ui.label("$x$ → mi · $ x $ → mitex. Native Typst math prevents enabling.");
                settings_target_anchor(ui, SettingsTarget::GitDiffStyle, &mut settings_scroll_target);
                ui.horizontal_wrapped(|ui| {
                    theme::apply_compact_control_spacing(ui);
                    ui.label(RichText::new(SettingsTarget::GitDiffStyle.label()).strong());
                    for style in GitDiffStyle::ALL {
                        ui.selectable_value(&mut edited.git_diff_style, style, style.label());
                    }
                });
                settings_target_anchor(ui, SettingsTarget::RainbowBrackets, &mut settings_scroll_target);
                show_bracket_controls(ui, &mut edited.rainbow_brackets);
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
                        self.actions.push(SettingsAction::ShowShortcuts);
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
                settings_target_anchor(
                    ui,
                    SettingsTarget::FixedTabWidth,
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
                    ui.checkbox(
                        &mut edited.fixed_tab_width,
                        SettingsTarget::FixedTabWidth.label(),
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
                        self.font_catalog,
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
                        self.actions.push(SettingsAction::ChooseTool(ToolPickerTarget::UiFont));
                    }
                    ui.separator();
                    show_font_weight_control(
                        ui,
                        "ui-font-weight",
                        &mut edited.ui_font_weight,
                        &mut self.state.staged_ui_font_weight,
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
                        self.font_catalog,
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
                        self.actions.push(SettingsAction::ChooseTool(ToolPickerTarget::CodeFont));
                    }
                    ui.separator();
                    show_font_weight_control(
                        ui,
                        "code-font-weight",
                        &mut edited.code_font_weight,
                        &mut self.state.staged_code_font_weight,
                        self.font_configuration.code_weight_support.as_ref(),
                    );
                    if self.font_catalog_scanning {
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
                ui.horizontal_wrapped(|ui| {
                    theme::apply_compact_control_spacing(ui);
                    ui.label(RichText::new(SettingsTarget::PreviewJump.label()).strong());
                    egui::ComboBox::from_id_salt("source-preview-trigger")
                        .width(METRICS.settings.source_preview_trigger_width)
                        .selected_text(edited.source_preview_trigger.label())
                        .show_ui(ui, |ui| {
                            for trigger in SourcePreviewTrigger::ALL {
                                ui.selectable_value(
                                    &mut edited.source_preview_trigger,
                                    trigger,
                                    trigger.label(),
                                );
                            }
                        });
                    ui.separator();
                    ui.label(RichText::new("Hovers").strong());
                    ui.label(SettingsTarget::HoverDelay.label());
                    ui.add(
                        egui::DragValue::new(&mut edited.hover_delay_ms)
                            .range(0..=2_000)
                            .speed(10)
                            .suffix(" ms"),
                    );
                });

                ui.add_space(theme::SPACE.small);
                ui.separator();
                settings_target_anchor(
                    ui,
                    SettingsTarget::ExplorerOrder,
                    &mut settings_scroll_target,
                );
                show_explorer_order_controls(ui, &mut edited.explorer_order);

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
                let staged_typst = self.typst_tool;
                settings_target_anchor(
                    ui,
                    SettingsTarget::TypstCompiler,
                    &mut settings_scroll_target,
                );
                if tool_preference_editor(
                    ui,
                    SettingsTarget::TypstCompiler.label(),
                    &mut edited.typst,
                    staged_typst,
                    deterministic_settings,
                ) {
                    self.actions.push(SettingsAction::ChooseTool(ToolPickerTarget::Typst));
                }
                if !deterministic_settings
                    && let Some(reason) = &staged_typst.fallback_reason
                {
                    fallback_notice(ui, "Binary fallback active", reason);
                }
                ui.add_space(METRICS.settings.tool_gap);
                let staged_tinymist = self.tinymist_tool;
                settings_target_anchor(
                    ui,
                    SettingsTarget::TinymistLanguageServer,
                    &mut settings_scroll_target,
                );
                if tool_preference_editor(
                    ui,
                    SettingsTarget::TinymistLanguageServer.label(),
                    &mut edited.tinymist,
                    staged_tinymist,
                    deterministic_settings,
                ) {
                    self.actions.push(SettingsAction::ChooseTool(ToolPickerTarget::Tinymist));
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
                    self.actions.push(SettingsAction::RefreshTools);
                    ui.ctx().request_repaint();
                }
                if ui
                    .button(SettingsTarget::BrowseTypstPackages.label())
                    .clicked()
                {
                    self.actions.push(SettingsAction::ShowPackages);
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
                            self.status.backend_label
                        },
                    );
                });
                if !deterministic_settings
                    && let Some(reason) = self.status.fallback_reason.as_ref()
                {
                    fallback_notice(ui, "Preview fallback active", reason);
                }
                if edited.preview_preference == PreviewPreference::PdfJs {
                    ui.add(egui::Label::new(if cfg!(any(target_os = "macos", target_os = "windows")) {
                        "PDF.js supports scrolling, zooming, text selection and PDF links. Source-to-preview jumps require the Tinymist mode."
                    } else {
                        "PDF.js is available on macOS and Windows. This platform uses the rasterised PDF preview."
                    }).wrap());
                }
                settings_target_anchor(
                    ui,
                    SettingsTarget::PreviewFollowEdits,
                    &mut settings_scroll_target,
                );
                ui.checkbox(
                    &mut edited.preview_follow_edits,
                    SettingsTarget::PreviewFollowEdits.label(),
                )
                .on_hover_text(
                    "Automatically scroll the Tinymist preview to your edit after compilation. Does not move the editor cursor. PDF.js and raster previews do not support source jumps.",
                );
                if !deterministic_settings
                    && self.status.requested_backend == PreviewPreference::Interactive
                    && !self.status.interactive_active
                    && ui.button("Retry Tinymist").clicked()
                {
                    self.actions.push(SettingsAction::RetryTinymist);
                }

                ui.add_space(theme::SPACE.small);
                ui.separator();
                settings_heading(ui, SettingsSection::Status);
                let status = if deterministic_settings {
                    format!(
                        "{} (deterministic QA build)",
                        crate::build_info::APP_NAME
                    )
                } else {
                    crate::build_info::VERSION.to_owned()
                };
                ui.add(egui::Label::new(status).wrap());
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
                        for name in ["Editing", "LSP", "Interactive", "PDF", "Raster", "Links"] {
                            show_status_chip(ui, name, "Ready", "Ready", syntax_color);
                        }
                    } else {
                        show_tool_status_chip(ui, "Typst", self.typst_tool);
                        show_tool_status_chip(ui, "Tinymist", self.tinymist_tool);
                        let capabilities = &self.status.capabilities;
                        show_service_status_chip(ui, "Editing", &capabilities.editing);
                        show_service_status_chip(ui, "LSP", &capabilities.lsp);
                        show_service_status_chip(
                            ui,
                            "Interactive",
                            &capabilities.interactive_preview,
                        );
                        show_service_status_chip(ui, "PDF", &capabilities.pdf_generation);
                        show_service_status_chip(ui, "Raster", &capabilities.rasterization);
                        show_service_status_chip(ui, "Links", &capabilities.link_extraction);
                    }
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
                        self.project_root.display().to_string()
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

        self.state.scroll_target = settings_scroll_target;

        if !deterministic_settings && &edited != original {
            self.actions.push(SettingsAction::Update(Box::new(edited)));
        }
    }
}

/// Reserve the complete label/control width before egui places the group in a
/// wrapping row. A non-wrapping child alone starts at the old cursor position.
pub(super) fn settings_slider(
    ui: &mut egui::Ui,
    label: &str,
    slider: egui::Slider<'_>,
) -> egui::Response {
    settings_slider_enabled(ui, label, true, slider)
}

fn settings_slider_enabled(
    ui: &mut egui::Ui,
    label: &str,
    enabled: bool,
    slider: egui::Slider<'_>,
) -> egui::Response {
    let label_width = ui.fonts_mut(|fonts| {
        fonts
            .layout_no_wrap(
                label.to_owned(),
                egui::TextStyle::Body.resolve(ui.style()),
                ui.visuals().text_color(),
            )
            .size()
            .x
    });
    let gap = ui.spacing().item_spacing.x;
    let width = (label_width + gap + 172.0).min(ui.max_rect().width());
    ui.allocate_ui_with_layout(
        Vec2::new(width, ui.spacing().interact_size.y),
        Layout::left_to_right(Align::Center),
        |ui| {
            if !enabled {
                ui.disable();
            }
            ui.spacing_mut().slider_width = (width - label_width - gap - 72.0).max(32.0);
            let label = ui.add(egui::Label::new(label).extend());
            ui.add(slider).labelled_by(label.id)
        },
    )
    .inner
}

pub(super) fn show_theme_choices(
    ui: &mut egui::Ui,
    choice: &mut ColorThemeChoice,
    appearance: egui::Theme,
) {
    let preferred_dark = appearance == egui::Theme::Dark;
    for dark in [preferred_dark, !preferred_dark] {
        if dark != preferred_dark {
            ui.separator();
        }
        ui.label(RichText::new(if dark { "Dark themes" } else { "Light themes" }).strong());
        for builtin in builtin_themes::for_mode(dark) {
            let selected = matches!(choice, ColorThemeChoice::Builtin(id) if id == builtin.id);
            if ui.selectable_label(selected, builtin.name).clicked() {
                *choice = ColorThemeChoice::builtin(builtin.id);
            }
        }
    }
}

pub(super) const LUMINOSITY_HINT: &str = "Adjust midtones while preserving black and white.";

pub(super) fn show_theme_color_controls(
    ui: &mut egui::Ui,
    transform: &mut crate::theme_transform::ThemeTransform,
) {
    ui.horizontal_wrapped(|ui| {
        ui.label(RichText::new(SettingsTarget::ThemeColors.label()).strong());
        if ui
            .add_enabled(
                *transform != Default::default(),
                egui::Button::new("Reset colors"),
            )
            .clicked()
        {
            *transform = Default::default();
        }
    });
    ui.horizontal_wrapped(|ui| {
        ui.checkbox(&mut transform.invert, SettingsTarget::InvertColors.label());
        settings_slider(
            ui,
            SettingsTarget::HueShift.label(),
            egui::Slider::new(&mut transform.hue_shift_degrees, -180.0..=180.0)
                .integer()
                .suffix("°"),
        );
        let colors = &mut transform.colors;
        settings_hover_text(
            settings_slider(
                ui,
                "Luminosity",
                egui::Slider::new(&mut colors.luminosity, -100..=100).suffix("%"),
            ),
            LUMINOSITY_HINT,
        );
        settings_slider(
            ui,
            "Brightness",
            egui::Slider::new(&mut colors.brightness, -50..=50).suffix("%"),
        );
        settings_slider(
            ui,
            "Contrast",
            egui::Slider::new(&mut colors.contrast, 50..=150).suffix("%"),
        );
        settings_hover_text(
            settings_slider(
                ui,
                "Saturation",
                egui::Slider::new(&mut colors.saturation, 0..=200).suffix("%"),
            ),
            "0% makes the theme grayscale; 100% keeps its original saturation.",
        );
    });
}

pub(super) fn show_explorer_order_controls(ui: &mut egui::Ui, order: &mut ExplorerOrder) {
    // Earlier Settings rows can have wide intrinsic content. Anchor these
    // right-aligned controls to the visible window, not that overflow width.
    let top = ui.next_widget_position();
    let width = ui
        .available_width()
        .min((ui.clip_rect().right() - top.x).max(1.0));
    let bounds = Rect::from_min_size(top, Vec2::new(width, ui.available_height()));
    ui.scope_builder(egui::UiBuilder::new().max_rect(bounds), |ui| {
        ui.horizontal(|ui| {
            ui.label(RichText::new(SettingsTarget::ExplorerOrder.label()).strong());
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if ui
                    .add_enabled(
                        *order != ExplorerOrder::default(),
                        egui::Button::new("Reset panel order"),
                    )
                    .clicked()
                {
                    *order = ExplorerOrder::default();
                }
            });
        });
        for (position, section) in order.sections().into_iter().enumerate() {
            ui.push_id(section.id(), |ui| {
                ui.horizontal(|ui| {
                    ui.label(section.title());
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        for (symbol, direction, destination) in [
                            (
                                "↓",
                                "down",
                                (position + 1 < ExplorerSection::ALL.len()).then_some(position + 1),
                            ),
                            ("↑", "up", position.checked_sub(1)),
                        ] {
                            let response = ui.add_enabled(
                                destination.is_some(),
                                egui::Button::new(symbol).min_size(Vec2::splat(24.0)),
                            );
                            let label = format!("Move {} {direction}", section.title());
                            response.widget_info(|| {
                                egui::WidgetInfo::labeled(
                                    egui::WidgetType::Button,
                                    response.enabled(),
                                    &label,
                                )
                            });
                            if settings_hover_text(response, label).clicked()
                                && let Some(destination) = destination
                            {
                                order.move_to(section, destination);
                            }
                        }
                    });
                });
            });
        }
    });
}

/// Keep each palette on its own wrapping row so the controls remain usable in
/// narrow Settings windows. The preview uses the actual light/dark colors.
pub(super) fn show_bracket_controls(
    ui: &mut egui::Ui,
    settings: &mut crate::rainbow::RainbowBrackets,
) {
    use crate::rainbow::{BracketFamily, BracketPalette};
    ui.checkbox(
        &mut settings.enabled,
        SettingsTarget::RainbowBrackets.label(),
    );
    ui.add_enabled_ui(settings.enabled, |ui| {
        for family in BracketFamily::ALL {
            let selected = &mut settings.palettes[family as usize];
            ui.horizontal_wrapped(|ui| {
                let label = ui.label(family.label());
                egui::ComboBox::from_id_salt(("bracket-palette", family as usize))
                    .selected_text(selected.label())
                    .width(120.0)
                    .show_ui(ui, |ui| {
                        for palette in BracketPalette::ALL {
                            ui.selectable_value(selected, palette, palette.label());
                        }
                    })
                    .response
                    .labelled_by(label.id);
                for color in selected.colors(ui.visuals().dark_mode) {
                    ui.colored_label(color, "●");
                }
            });
        }
    });
}

#[cfg(test)]
mod tests {
    use super::super::settings_controls::settings_status_has_detail;
    use super::super::{
        HoverTooltipOverlay, install_hover_runtime_config, settings_hover_tooltip_id,
        show_local_tooltip_card,
    };
    use super::*;
    use egui::{Color32, Pos2};
    use egui_kittest::{Harness, kittest::Queryable as _};
    use std::time::Duration;

    #[test]
    fn settings_renderer_emits_actions_without_mutating_live_preferences_or_services() {
        use crate::preview::ServiceState;
        use crate::toolchain::{ToolKind, ToolOrigin};
        let captures = CaptureController::disabled_for_tests();
        let fonts = FontCatalog::default();
        let font_configuration = theme::FontConfiguration {
            custom_ui_loaded: false,
            custom_editor_loaded: false,
            weighted_ui_loaded: false,
            ui_weight_support: None,
            code_weight_support: None,
            editor_weight_support: theme::FontWeightSupport::Discrete {
                values: vec![400],
                default: 400,
            },
        };
        let typst = ToolResolution {
            kind: ToolKind::Typst,
            program: "typst".into(),
            origin: ToolOrigin::Bundled,
            fallback_reason: None,
        };
        let tinymist = ToolResolution {
            kind: ToolKind::Tinymist,
            program: "tinymist".into(),
            origin: ToolOrigin::Bundled,
            fallback_reason: None,
        };
        let mut harness = Harness::builder()
            .with_size(Vec2::new(700.0, 4000.0))
            .build_ui_state(
                move |ui,
                      state: &mut (
                    SettingsUiState,
                    AppSettings,
                    Vec<SettingsAction>,
                    Option<AppSettings>,
                )| {
                    SettingsPanel {
                        state: &mut state.0,
                        settings: &state.1,
                        pending_settings: state.3.as_ref(),
                        theme_override: None,
                        snapshot_scene: None,
                        font_catalog: &fonts,
                        font_catalog_scanning: false,
                        font_configuration: &font_configuration,
                        typst_tool: &typst,
                        tinymist_tool: &tinymist,
                        status: SettingsStatus {
                            backend_label: "Rasterised PDF",
                            fallback_reason: None,
                            requested_backend: PreviewPreference::Interactive,
                            interactive_active: false,
                            capabilities: CapabilitySnapshot {
                                editing: ServiceState::Ready("ready".into()),
                                lsp: ServiceState::Failed("failure".into()),
                                interactive_preview: ServiceState::Failed("failure".into()),
                                pdf_generation: ServiceState::Ready("ready".into()),
                                rasterization: ServiceState::Failed("pdftoppm unavailable".into()),
                                link_extraction: ServiceState::Ready("ready".into()),
                            },
                        },
                        project_root: Path::new("."),
                        captures: &captures,
                        actions: &mut state.2,
                    }
                    .show(ui);
                },
                (
                    SettingsUiState::default(),
                    AppSettings::default(),
                    Vec::new(),
                    None,
                ),
            );
        harness.run();
        for capability in ["Editing", "LSP", "Interactive", "PDF", "Raster", "Links"] {
            harness.get_by_label(capability);
        }
        assert!(
            harness.state().2.is_empty(),
            "idle Settings must not emit updates"
        );
        harness.get_by_label("Follow edits in preview").click();
        harness.run();
        assert!(harness.state().1.preview_follow_edits);
        assert!(harness.state().2.iter().any(|action| {
            matches!(action, SettingsAction::Update(settings) if !settings.preview_follow_edits)
        }));
        harness.state_mut().2.clear();
        harness.get_by_label("Retry Tinymist").click();
        harness.run();
        assert!(
            harness
                .state()
                .2
                .iter()
                .any(|a| matches!(a, SettingsAction::RetryTinymist))
        );
        harness.state_mut().2.clear();
        harness.get_by_label("Invert colors").click();
        harness.run();
        assert!(!harness.state().1.theme_invert);
        assert!(
            harness
                .state()
                .2
                .iter()
                .any(|a| matches!(a, SettingsAction::Update(settings) if settings.theme_invert))
        );
        harness.state_mut().2.clear();
        harness.get_by_label("Fixed tab width").click();
        harness.run();
        assert!(
            harness
                .state()
                .2
                .iter()
                .any(|a| matches!(a, SettingsAction::Update(settings) if settings.fixed_tab_width))
        );
        harness.state_mut().2.clear();
        harness.get_by_label("Side-by-side").click();
        harness.run();
        assert!(harness.state().2.iter().any(
            |a| matches!(a, SettingsAction::Update(settings) if settings.git_diff_style
                == crate::settings::GitDiffStyle::SideBySide)
        ));
        harness.state_mut().2.clear();
        harness.get_by_label("Overrides…").click();
        harness.run();
        assert!(
            harness
                .state()
                .2
                .iter()
                .any(|a| matches!(a, SettingsAction::ShowOverrides { .. }))
        );
        // A pending edit is already queued: idle frames must not enqueue it again.
        harness.state_mut().2.clear();
        let mut pending = harness.state().1.clone();
        pending.theme_invert = true;
        harness.state_mut().3 = Some(pending);
        harness.run();
        assert!(harness.state().2.is_empty());
        // Changing back to the live value must still cancel that pending edit.
        harness.get_by_label("Invert colors").click();
        harness.run();
        assert!(harness.state().2.iter().any(|action| matches!(action,
            SettingsAction::Update(settings) if !settings.theme_invert)));
    }

    #[test]
    fn delay_label_and_slider_wrap_as_one_control() {
        for width in [320.0, 360.0, 500.0, 620.0] {
            for delay in [250, 750, 5_000] {
                let mut harness = Harness::builder()
                    .with_size(Vec2::new(width, 220.0))
                    .build_ui_state(
                        |ui, delay| {
                            ui.horizontal_wrapped(|ui| {
                                ui.add_sized([width - 180.0, 20.0], egui::Button::new("Before"));
                                settings_slider_enabled(
                                    ui,
                                    "Auto-save delay",
                                    true,
                                    egui::Slider::new(delay, 250..=5_000)
                                        .suffix(" ms")
                                        .logarithmic(true),
                                );
                            });
                        },
                        delay,
                    );
                harness.run();
                let before = harness
                    .get_by_role_and_label(egui::accesskit::Role::Button, "Before")
                    .rect();
                let label = harness
                    .get_by_role_and_label(egui::accesskit::Role::Label, "Auto-save delay")
                    .rect();
                let slider = harness
                    .get_by_role_and_label(egui::accesskit::Role::Slider, "Auto-save delay")
                    .rect();
                assert!(
                    label.top() >= before.bottom(),
                    "width={width} label={label:?} before={before:?}"
                );
                assert!((label.center().y - slider.center().y).abs() < 1.0);
                assert!(label.right() <= slider.left());
                assert!(slider.right() <= width);
            }
        }
    }

    #[test]
    fn status_chips_wrap_intact_and_stay_within_the_window() {
        for width in [320.0, 360.0, 500.0, 620.0] {
            let mut harness = Harness::builder()
                .with_size(Vec2::new(width, 260.0))
                .build_ui(|ui| {
                    ui.horizontal_wrapped(|ui| {
                        ui.add_sized([width - 75.0, 20.0], egui::Button::new("Before"));
                        for (name, status) in [
                            ("Tinymist", "Bundled"),
                            ("Watcher", "Starting"),
                            ("Syntax", "Ready"),
                        ] {
                            show_status_chip(ui, name, status, "status detail", Color32::GREEN);
                        }
                    });
                });
            harness.run();
            for (name, status) in [
                ("Tinymist", "Bundled"),
                ("Watcher", "Starting"),
                ("Syntax", "Ready"),
            ] {
                let name = harness
                    .get_by_role_and_label(egui::accesskit::Role::Label, name)
                    .rect();
                let status = harness
                    .get_by_role_and_label(egui::accesskit::Role::Label, status)
                    .rect();
                assert!(
                    (name.center().y - status.center().y).abs() < 1.0,
                    "width={width}: {name:?} {status:?}"
                );
                assert!(status.right() <= width && name.left() >= 0.0);
            }
            assert!(
                harness.get_by_label("Tinymist").rect().top()
                    >= harness.get_by_label("Before").rect().bottom()
            );
        }
    }

    #[test]
    fn each_theme_slot_lists_matching_themes_first_and_accepts_the_other_mode() {
        for appearance in [egui::Theme::Light, egui::Theme::Dark] {
            let mut harness = Harness::builder()
                .with_size(Vec2::new(360.0, 1600.0))
                .build_ui_state(
                    |ui, choice| show_theme_choices(ui, choice, appearance),
                    ColorThemeChoice::builtin("tiptop-light"),
                );
            harness.run();
            let light = harness.get_by_label("Light themes").rect();
            let dark = harness.get_by_label("Dark themes").rect();
            assert_eq!(light.top() < dark.top(), appearance == egui::Theme::Light);
            let target = builtin_themes::default_for_mode(appearance == egui::Theme::Light);
            harness.get_by_label(target.name).click();
            harness.run();
            assert_eq!(*harness.state(), ColorThemeChoice::builtin(target.id));
        }
    }

    #[test]
    fn color_controls_edit_and_reset_without_overflow() {
        for width in [320.0, 500.0, 700.0] {
            let mut harness = Harness::builder()
                .with_size(Vec2::new(width, 360.0))
                .build_ui_state(
                    show_theme_color_controls,
                    crate::theme_transform::ThemeTransform::default(),
                );
            harness.run();
            harness.get_by_label("Invert colors").click();
            harness.run_steps(4);
            assert!(harness.state().invert);
            for label in [
                "Hue shift",
                "Luminosity",
                "Brightness",
                "Contrast",
                "Saturation",
            ] {
                let slider = harness.get_by_role_and_label(egui::accesskit::Role::Slider, label);
                let label_rect = harness
                    .get_by_role_and_label(egui::accesskit::Role::Label, label)
                    .rect();
                assert!((slider.rect().center().y - label_rect.center().y).abs() < 1.0);
                assert!(slider.rect().right() <= width);
                let pos = Pos2::new(
                    slider.rect().left() + slider.rect().width() * 0.75,
                    slider.rect().center().y,
                );
                harness.event(egui::Event::PointerMoved(pos));
                for pressed in [true, false] {
                    harness.event(egui::Event::PointerButton {
                        pos,
                        button: egui::PointerButton::Primary,
                        pressed,
                        modifiers: egui::Modifiers::NONE,
                    });
                }
                // Hover cards deliberately request animation frames.
                harness.run_steps(4);
            }
            assert_ne!(*harness.state(), Default::default());
            assert_ne!(harness.state().hue_shift_degrees, 0.0);
            assert_ne!(harness.state().colors.luminosity, 0);
            assert_ne!(harness.state().colors.brightness, 0);
            assert_ne!(harness.state().colors.contrast, 100);
            assert_ne!(harness.state().colors.saturation, 100);
            harness.get_by_label("Reset colors").click();
            harness.run_steps(4);
            assert_eq!(*harness.state(), Default::default());
        }
    }

    #[test]
    fn unified_color_reset_also_handles_only_inversion_or_hue() {
        for transform in [
            crate::theme_transform::ThemeTransform::new(true, 0.0),
            crate::theme_transform::ThemeTransform::new(false, 30.0),
        ] {
            let mut harness =
                Harness::builder().build_ui_state(show_theme_color_controls, transform);
            harness.run();
            harness.get_by_label("Reset colors").click();
            harness.run_steps(4);
            assert_eq!(*harness.state(), Default::default());
        }
    }

    #[test]
    fn only_nonobvious_color_controls_offer_hints() {
        let mut harness = Harness::builder()
            .with_size(Vec2::new(500.0, 400.0))
            .build_ui(|ui| {
                install_hover_runtime_config(ui.ctx(), Duration::ZERO);
                let id = settings_hover_tooltip_id(ui.ctx());
                ui.ctx()
                    .data_mut(|data| data.remove::<HoverTooltipOverlay>(id));
                show_theme_color_controls(ui, &mut Default::default());
            });
        for (label, expected) in [
            ("Luminosity", true),
            ("Brightness", false),
            ("Contrast", false),
            ("Saturation", true),
            ("Hue shift", false),
        ] {
            let rect = harness
                .get_by_role_and_label(egui::accesskit::Role::Slider, label)
                .rect();
            harness.hover_at(rect.center());
            harness.run_steps(4);
            let id = settings_hover_tooltip_id(&harness.ctx);
            let hint = harness
                .ctx
                .data(|data| data.get_temp::<HoverTooltipOverlay>(id));
            assert_eq!(hint.is_some(), expected, "{label}");
        }
    }

    #[test]
    fn status_hints_skip_repeated_labels_but_keep_diagnostics() {
        for detail in [
            "",
            "Ready",
            " ready. ",
            "Watcher",
            "Watcher Ready",
            "Watcher: Ready",
        ] {
            assert!(
                !settings_status_has_detail("Watcher", "Ready", detail),
                "{detail}"
            );
        }
        for detail in [
            "Watching 4 files",
            "Bundled 0.15.1 · /path/to/typst",
            "Connection timed out; retrying",
        ] {
            assert!(settings_status_has_detail("Watcher", "Ready", detail));
        }
        let mut harness = Harness::builder().build_ui_state(
            |ui, detail| {
                install_hover_runtime_config(ui.ctx(), Duration::ZERO);
                let id = settings_hover_tooltip_id(ui.ctx());
                ui.ctx()
                    .data_mut(|data| data.remove::<HoverTooltipOverlay>(id));
                show_status_chip(ui, "Watcher", "Ready", detail, Color32::GREEN);
            },
            "Ready".to_owned(),
        );
        harness.hover_at(harness.get_by_label("Watcher").rect().center());
        harness.run_steps(4);
        let id = settings_hover_tooltip_id(&harness.ctx);
        assert!(
            harness
                .ctx
                .data(|data| data.get_temp::<HoverTooltipOverlay>(id))
                .is_none()
        );
        *harness.state_mut() = "Watching 4 files".to_owned();
        harness.run_steps(4);
        assert_eq!(
            harness
                .ctx
                .data(|data| data.get_temp::<HoverTooltipOverlay>(id))
                .unwrap()
                .detail,
            "Watching 4 files".into()
        );
    }

    #[test]
    fn local_hints_fit_content_and_shrink_after_long_messages() {
        for width in [320.0, 360.0, 500.0, 700.0] {
            let mut harness = Harness::builder()
                .with_size(Vec2::new(width, 400.0))
                .build_ui_state(
                    |ui, state: &mut (String, Rect)| {
                        state.1 = show_local_tooltip_card(
                            ui.ctx(),
                            Pos2::new(width - 30.0, 365.0),
                            &state.0,
                            1.0,
                        );
                    },
                    ("Move Files up".to_owned(), Rect::NOTHING),
                );
            harness.run();
            let short = harness.state().1;
            assert!(short.width() < 150.0 && short.height() < 50.0, "{short:?}");
            for detail in [
                LUMINOSITY_HINT.to_owned(),
                format!("/{}tool.typ", "a-long-directory/".repeat(25)),
                "A useful diagnostic detail.\n".repeat(40),
                "Move Files up".to_owned(),
            ] {
                harness.state_mut().0 = detail;
                harness.run();
                let card = harness.state().1;
                let bounds = harness.ctx.content_rect().shrink(theme::SPACE.small);
                assert!(
                    bounds.expand(1.0).contains_rect(card),
                    "width={width} card={card:?} bounds={bounds:?}"
                );
            }
            assert_eq!(harness.state().1.size(), short.size());
        }
    }
}
