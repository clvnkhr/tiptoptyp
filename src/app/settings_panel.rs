//! Settings renderer: borrowed presentation inputs and explicit application actions.
//! This module cannot access the document, worker handles, or EditorApp.
use super::settings_controls::{
    FontPickerSelection, color_theme_choice_label, fallback_notice, settings_control_row,
    settings_inline_value, settings_value_row, show_font_family_picker, show_font_weight_control,
    show_page_appearance_sample, show_service_status_chip, show_status_chip,
    tool_preference_editor,
};
use super::{
    SettingsSection, SettingsTarget, ToolPickerTarget, settings_heading, settings_hover_text,
    settings_search_results, settings_target_anchor, success_color,
};
use crate::{
    builtin_themes,
    capabilities::CapabilitySnapshot,
    explorer::{ExplorerOrder, ExplorerSection},
    font_catalog::FontCatalog,
    screenshot::{CaptureController, CaptureThemeProfile, UiSnapshotScene},
    settings::{
        AppSettings, ColorThemeChoice, DocumentTheme, EditorParticleSettings, GitDiffStyle,
        InterfaceTheme, ParticleDensity, ParticleStyle, PreviewPreference, RevealAnimation,
        RevealStyle, SourcePreviewTrigger,
    },
    shortcuts::ShortcutAction,
    theme::{self, METRICS},
    toolchain::ToolResolution,
};
use eframe::egui::{self, Align, Layout, Rect, RichText, Vec2};
use std::path::Path;

#[derive(Default)]
pub(super) struct SettingsUiState {
    pub(super) category: SettingsSection,
    pub(super) colors_open: bool,
    pub(super) json_mode: bool,
    pub(super) help_mode: bool,
    pub(super) json_search: bool,
    pub(super) json_query: String,
    pub(super) json_match: Option<usize>,
    pub(super) json_draft: Option<crate::settings_json::Draft>,
    pub(super) query: String,
    /// The owner sets this when a shortcut should return focus to the search
    /// field. The flag is consumed after the widget is built for this frame.
    pub(super) focus_search: bool,
    pub(super) scroll_target: Option<SettingsTarget>,
    pub(super) snippet_library: super::settings_snippets::LibraryState,
    harper_dictionary_draft: Option<(String, String)>,
    pub(super) staged_ui_font_weight: Option<u16>,
    pub(super) staged_code_font_weight: Option<u16>,
}

pub(super) enum SettingsAction {
    OpenDocumentation(String),
    ChooseTool(ToolPickerTarget),
    ShowOverrides { dark: bool },
    ShowShortcuts,
    ShowSnippets,
    EditSnippet(super::settings_snippets::Selection),
    RefreshTools,
    ShowPackages,
    RetryTinymist,
    Update(Box<AppSettings>),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct SettingsStatus {
    pub(super) backend_label: &'static str,
    pub(super) preview_failure: Option<String>,
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
    pub(super) tex_tools: &'a crate::tex::tools::TexTools,
    pub(super) status: SettingsStatus,
    pub(super) project_root: &'a Path,
    pub(super) captures: &'a CaptureController,
    pub(super) actions: &'a mut Vec<SettingsAction>,
}
impl SettingsPanel<'_> {
    fn show_json(&mut self, ui: &mut egui::Ui) {
        let draft = self
            .state
            .json_draft
            .get_or_insert_with(|| crate::settings_json::Draft::new(self.settings));
        if draft.base != *self.settings
            && draft
                .validation
                .as_ref()
                .is_ok_and(|value| value == &draft.base)
        {
            *draft = crate::settings_json::Draft::new(self.settings);
        }
        ui.horizontal_wrapped(|ui| {
            let save = ui.add_enabled(
                draft.validation.is_ok() && !draft.saved,
                egui::Button::new("Save JSON settings"),
            );
            #[cfg(all(feature = "desktop-ui-tests", target_os = "macos"))]
            crate::desktop_test::observe("settings.json.save", &save);
            if save.clicked() {
                let mut merged = self.settings.clone();
                merged.apply_edits(&draft.base, draft.validation.as_ref().unwrap().clone());
                self.actions
                    .push(SettingsAction::Update(Box::new(merged.clone())));
                *draft = crate::settings_json::Draft::new(&merged);
                draft.saved = true;
            }
            let reload = ui.button("Reload current settings");
            #[cfg(all(feature = "desktop-ui-tests", target_os = "macos"))]
            crate::desktop_test::observe("settings.json.reload", &reload);
            if reload.clicked() {
                *draft = crate::settings_json::Draft::new(self.settings);
            }
            if draft.validation.is_ok() {
                ui.weak(if draft.saved {
                    "Settings saved"
                } else {
                    "Valid settings"
                });
            }
        });
        if let Err(error) = &draft.validation {
            ui.colored_label(ui.visuals().error_fg_color, error);
        }
        let mut find = false;
        let mut focus_match = false;
        if self.state.focus_search {
            self.state.json_search = true;
        }
        if self.state.json_search {
            let row_height = ui
                .text_style_height(&egui::TextStyle::Body)
                .max(ui.text_style_height(&egui::TextStyle::Button))
                + ui.spacing().button_padding.y * 2.0;
            ui.allocate_ui_with_layout(
                Vec2::new(
                    ui.available_width(),
                    row_height.max(ui.spacing().interact_size.y),
                ),
                Layout::right_to_left(Align::Center),
                |ui| {
                    if ui.small_button("Close search").clicked() {
                        self.state.json_search = false;
                    }
                    focus_match |= ui.button("Next match").clicked();
                    let query = ui.add(
                        egui::TextEdit::singleline(&mut self.state.json_query)
                            .id_salt("settings-json-find")
                            .hint_text("Find in JSON")
                            .horizontal_align(Align::Min)
                            .desired_width(ui.available_width().max(1.0)),
                    );
                    query.widget_info(|| {
                        egui::WidgetInfo::labeled(egui::WidgetType::TextEdit, true, "Find in JSON")
                    });
                    #[cfg(all(feature = "desktop-ui-tests", target_os = "macos"))]
                    crate::desktop_test::observe("settings.json.find", &query);
                    if self.state.focus_search {
                        query.request_focus();
                        self.state.focus_search = false;
                    }
                    focus_match |= (query.has_focus() || query.lost_focus())
                        && ui.input(|i| i.key_pressed(egui::Key::Enter));
                    find = query.changed() || focus_match;
                    if query.changed() {
                        self.state.json_match = None;
                    }
                },
            );
        }
        ui.separator();
        egui::ScrollArea::vertical()
            .id_salt("settings-json-scroll")
            .auto_shrink([false, false])
            .min_scrolled_height(0.0)
            .max_height(ui.available_height().max(1.0))
            .show(ui, |ui| {
                let response = super::code_editor::editor(
                    ui,
                    "settings-json",
                    &mut draft.text,
                    "json",
                    self.settings,
                    30,
                );
                if find && !self.state.json_query.is_empty() {
                    let start = self.state.json_match.map_or(0, |i| {
                        draft.text.floor_char_boundary(
                            (i + self.state.json_query.len()).min(draft.text.len()),
                        )
                    });
                    let found = draft.text[start..]
                        .find(&self.state.json_query)
                        .map(|i| i + start)
                        .or_else(|| draft.text[..start].find(&self.state.json_query));
                    self.state.json_match = found;
                    if let Some(found) = found {
                        let first = draft.text[..found].chars().count();
                        let last = first + self.state.json_query.chars().count();
                        if let Some(mut state) = egui::TextEdit::load_state(ui.ctx(), response.id) {
                            state
                                .cursor
                                .set_char_range(Some(egui::text::CCursorRange::two(
                                    egui::text::CCursor::new(first),
                                    egui::text::CCursor::new(last),
                                )));
                            state.store(ui.ctx(), response.id);
                        }
                        if focus_match {
                            response.request_focus();
                        }
                    }
                }
                #[cfg(all(feature = "desktop-ui-tests", target_os = "macos"))]
                crate::desktop_test::observe("settings.json", &response);
                if response.changed() {
                    self.state.json_match = None;
                    draft.validate();
                }
            });
    }

    pub(super) fn show(&mut self, ui: &mut egui::Ui) {
        let documentation_id = egui::Id::new(("settings-documentation", ui.ctx().viewport_id()));
        if let Some(url) = ui
            .ctx()
            .data_mut(|data| data.remove_temp::<String>(documentation_id))
        {
            self.actions.push(SettingsAction::OpenDocumentation(url));
        }
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
        if self.state.help_mode {
            super::help::show(ui, self.settings);
            return;
        }
        if self.state.json_mode {
            self.show_json(ui);
            return;
        }

        let wide = super::settings_navigation::uses_rail(ui.available_width());
        ui.horizontal(|ui| {
            if !wide {
                super::settings_navigation::selector(ui, &mut self.state.category);
            }
            let search = ui.add(
                egui::TextEdit::singleline(&mut self.state.query)
                    .id_salt("settings-search")
                    .hint_text("Search settings")
                    .desired_width(f32::INFINITY),
            );
            search.widget_info(|| {
                egui::WidgetInfo::labeled(egui::WidgetType::TextEdit, true, "Search settings")
            });
            if self.state.focus_search {
                self.state.focus_search = false;
                search.request_focus();
            }
            #[cfg(all(feature = "desktop-ui-tests", target_os = "macos"))]
            crate::desktop_test::observe("settings.search", &search);
        });
        if !self.state.query.trim().is_empty() {
            let matches = settings_search_results(&self.state.query);
            egui::ScrollArea::vertical()
                .id_salt("settings-search-results")
                .max_height(96.0)
                .show(ui, |ui| {
                    ui.horizontal_wrapped(|ui| {
                        if matches.is_empty() {
                            ui.weak("No settings found");
                        }
                        for target in matches {
                            let response = ui.button(target.label());
                            #[cfg(all(feature = "desktop-ui-tests", target_os = "macos"))]
                            crate::desktop_test::observe(
                                &format!("settings.result.{}", target.label()),
                                &response,
                            );
                            if response.clicked() {
                                self.state.scroll_target = Some(target);
                                let duration = std::time::Duration::from_secs(3);
                                let id = super::settings_highlight_id(ui.ctx().viewport_id());
                                ui.ctx().data_mut(|data| {
                                    data.insert_temp(
                                        id,
                                        (target, std::time::Instant::now() + duration),
                                    )
                                });
                                ui.ctx().request_repaint_after(duration);
                            }
                        }
                    });
                });
        }
        // Keep the fixed search area distinct from category navigation and forms.
        ui.separator();
        if let Some(target) = self.state.scroll_target {
            super::settings_navigation::route_target(
                &mut self.state.category,
                &mut self.state.colors_open,
                target,
            );
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
        let mut settings_scroll_target = self.state.scroll_target.take();
        if wide {
            let rail_width = super::settings_navigation::rail_width(ui);
            ui.horizontal_top(|ui| {
                ui.allocate_ui_with_layout(
                    Vec2::new(rail_width, ui.available_height()),
                    Layout::top_down(Align::Min),
                    |ui| {
                        super::settings_navigation::scrolling_rail(ui, &mut self.state.category);
                    },
                );
                ui.separator();
                ui.vertical(|ui| {
                    ui.set_width(ui.available_width());
                    settings_scroll_target = self.show_category_scroll(
                        ui,
                        &mut edited,
                        settings_scroll_target,
                        deterministic_settings,
                    );
                });
            });
        } else {
            settings_scroll_target = self.show_category_scroll(
                ui,
                &mut edited,
                settings_scroll_target,
                deterministic_settings,
            );
        }

        self.state.scroll_target = settings_scroll_target;

        if !deterministic_settings && &edited != original {
            self.actions.push(SettingsAction::Update(Box::new(edited)));
        }
    }
    fn show_category_scroll(
        &mut self,
        ui: &mut egui::Ui,
        edited: &mut AppSettings,
        target: Option<SettingsTarget>,
        deterministic: bool,
    ) -> Option<SettingsTarget> {
        // Responsive rails add layout ancestors, but editor drafts and widget
        // history must retain the same identity when the window crosses them.
        let content_id = egui::Id::new((
            ui.ctx().viewport_id(),
            "settings-category-content",
            self.state.category,
        ));
        ui.scope_builder(egui::UiBuilder::new().id(content_id), |ui| {
            if self.state.category == SettingsSection::Snippets {
                // The library owns its full remaining-height scrolling area.
                return self.show_category(ui, edited, target, deterministic);
            }
            let scroll = egui::ScrollArea::vertical()
                .id_salt(("settings-scroll", self.state.category))
                .auto_shrink([false, false]);
            let reset = deterministic
                && !matches!(
                    self.snapshot_scene,
                    Some(
                        UiSnapshotScene::BracketSettings
                            | UiSnapshotScene::SettingsColors
                            | UiSnapshotScene::SettingsEditor
                            | UiSnapshotScene::SettingsStatus
                    )
                );
            let scroll = if reset {
                scroll.vertical_scroll_offset(0.0)
            } else {
                scroll
            };
            scroll
                .show(ui, |ui| {
                    self.show_category(ui, edited, target, deterministic)
                })
                .inner
        })
        .inner
    }
    fn show_category(
        &mut self,
        ui: &mut egui::Ui,
        edited: &mut AppSettings,
        mut settings_scroll_target: Option<SettingsTarget>,
        deterministic_settings: bool,
    ) -> Option<SettingsTarget> {
        settings_heading(ui, self.state.category);
        match self.state.category {
            SettingsSection::Appearance => {
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
                settings_target_anchor(ui, SettingsTarget::Appearance, &mut settings_scroll_target);
                settings_control_row(ui, "Interface", |ui| {
                    ui.add_enabled_ui(theme_picker_enabled, |ui| {
                        ui.horizontal_wrapped(|ui| {
                            for appearance in InterfaceTheme::ALL {
                                let response = ui.selectable_value(
                                    &mut edited.interface_theme,
                                    appearance,
                                    appearance.label(),
                                );
                                #[cfg(all(feature = "desktop-ui-tests", target_os = "macos"))]
                                if ui.is_rect_visible(response.rect) {
                                    crate::desktop_test::observe(
                                        &format!("settings.theme.{}", appearance.label()),
                                        &response,
                                    );
                                }
                                let _ = response;
                            }
                        });
                    });
                    if theme_overridden {
                        ui.weak("QA override");
                    }
                });
                crate::window_background::settings_ui(ui, &mut edited.window_background);
                let reveal_target = matches!(
                    settings_scroll_target,
                    Some(
                        SettingsTarget::SourceRevealAnimation
                            | SettingsTarget::PreviewRevealAnimation
                    )
                );
                egui::CollapsingHeader::new("Effects")
                    .id_salt("appearance-effects")
                    .open(reveal_target.then_some(true))
                    .show(ui, |ui| {
                        for (target, animation) in [
                            (
                                SettingsTarget::SourceRevealAnimation,
                                &mut edited.source_reveal_animation,
                            ),
                            (
                                SettingsTarget::PreviewRevealAnimation,
                                &mut edited.preview_reveal_animation,
                            ),
                        ] {
                            settings_target_anchor(ui, target, &mut settings_scroll_target);
                            reveal_animation_controls(ui, target.label(), animation);
                        }
                    });
                settings_target_anchor(
                    ui,
                    SettingsTarget::TypstSyntax,
                    &mut settings_scroll_target,
                );
                settings_control_row(ui, SettingsTarget::TypstSyntax.label(), |ui| {
                    if crate::app::icons::action_button(ui, "Overrides…").clicked() {
                        self.actions.push(SettingsAction::ShowOverrides {
                            dark: effective_theme == egui::Theme::Dark,
                        });
                    }
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
                    settings_control_row(ui, target.label(), |ui| {
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
                            let picker =
                                egui::ComboBox::from_id_salt((picker_id, self.snapshot_scene))
                                    .width((ui.available_width() - 34.0).clamp(72.0, 220.0))
                                    .height(METRICS.settings.theme_picker_max_height)
                                    .selected_text(selected_name)
                                    .show_ui(ui, |ui| {
                                        show_theme_choices(ui, choice, appearance);
                                    })
                                    .response;
                            if let Some(path) = &imported_path {
                                settings_hover_text(picker, path.clone());
                            }
                            if crate::app::icons::action_button(ui, "Import…").clicked() {
                                self.actions.push(SettingsAction::ChooseTool(
                                    ToolPickerTarget::SublimeTheme {
                                        dark_mode: appearance == egui::Theme::Dark,
                                    },
                                ));
                            }
                        });
                    });
                }

                let (displayed_invert, displayed_hue_shift) = self.theme_override.as_ref().map_or(
                    (edited.theme_invert, edited.theme_hue_shift_degrees),
                    |profile| (profile.invert, profile.hue_shift_degrees),
                );
                settings_target_anchor(
                    ui,
                    SettingsTarget::InvertColors,
                    &mut settings_scroll_target,
                );
                settings_target_anchor(ui, SettingsTarget::HueShift, &mut settings_scroll_target);
                settings_target_anchor(
                    ui,
                    SettingsTarget::ThemeColors,
                    &mut settings_scroll_target,
                );
                // Capture themes intentionally ignore persisted color adjustments.
                let colors = if theme_overridden {
                    Default::default()
                } else {
                    edited.theme_colors
                };
                let mut transform = crate::theme_transform::ThemeTransform::new(
                    displayed_invert,
                    f32::from(displayed_hue_shift),
                )
                .with_colors(colors);
                let reveal_colors = std::mem::take(&mut self.state.colors_open)
                    || matches!(self.snapshot_scene, Some(UiSnapshotScene::SettingsColors));
                let colors = egui::CollapsingHeader::new("Color adjustments")
                    .id_salt("settings-colors")
                    .open(reveal_colors.then_some(true))
                    .show(ui, |ui| {
                        ui.add_enabled_ui(!theme_overridden, |ui| {
                            show_theme_color_controls(ui, &mut transform);
                        });
                    });
                #[cfg(all(feature = "desktop-ui-tests", target_os = "macos"))]
                crate::desktop_test::observe("settings.colors.disclosure", &colors.header_response);
                let _ = colors;

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

                ui.separator();
                ui.strong("Page appearance");
                let page_width = ui.available_width();
                ui.horizontal_top(|ui| {
                    let controls_width = (page_width - 124.0).max(150.0);
                    ui.allocate_ui_with_layout(
                        Vec2::new(controls_width, 0.0),
                        Layout::top_down(Align::Min),
                        |ui| {
                            settings_target_anchor(
                                ui,
                                SettingsTarget::PageTheme,
                                &mut settings_scroll_target,
                            );
                            ui.horizontal_wrapped(|ui| {
                                for page in DocumentTheme::ALL {
                                    let response = ui.selectable_value(
                                        &mut edited.document_theme,
                                        page,
                                        page.label(),
                                    );
                                    #[cfg(all(feature = "desktop-ui-tests", target_os = "macos"))]
                                    if ui.is_rect_visible(response.rect) {
                                        crate::desktop_test::observe(
                                            &format!("settings.page.{page:?}"),
                                            &response,
                                        );
                                    }
                                    let _ = response;
                                }
                            });
                            let background =
                                ui.checkbox(&mut edited.comfy_background, "Comfy background");
                            let text = ui.checkbox(&mut edited.comfy_text, "Comfy text");
                            #[cfg(all(feature = "desktop-ui-tests", target_os = "macos"))]
                            {
                                if ui.is_rect_visible(background.rect) {
                                    crate::desktop_test::observe("settings.comfy", &background);
                                }
                                if ui.is_rect_visible(text.rect) {
                                    crate::desktop_test::observe("settings.comfy.text", &text);
                                }
                            }
                            let _ = (background, text);
                            settings_target_anchor(
                                ui,
                                SettingsTarget::DarkImages,
                                &mut settings_scroll_target,
                            );
                            image_appearance_preference(ui, edited);
                        },
                    );
                    show_page_appearance_sample(ui, edited);
                });
                ui.separator();
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
                settings_control_row(ui, SettingsTarget::InterfaceScale.label(), |ui| {
                    ui.add(
                        egui::Slider::new(&mut edited.ui_scale_percent, 75..=150)
                            .suffix("%")
                            .clamping(egui::SliderClamping::Always),
                    );
                });
                settings_control_row(ui, "Tabs and menus", |ui| {
                    ui.checkbox(
                        &mut edited.titlebar_menus,
                        SettingsTarget::TitleBarMenus.label(),
                    );
                    ui.checkbox(
                        &mut edited.fixed_tab_width,
                        SettingsTarget::FixedTabWidth.label(),
                    );
                });
                settings_target_anchor(
                    ui,
                    SettingsTarget::CompactLayoutControls,
                    &mut settings_scroll_target,
                );
                let compact_layout = ui.checkbox(
                    &mut edited.compact_layout_controls,
                    SettingsTarget::CompactLayoutControls.label(),
                );
                #[cfg(all(feature = "desktop-ui-tests", target_os = "macos"))]
                crate::desktop_test::observe("settings.compact_layout", &compact_layout);
                let _ = compact_layout;
                settings_target_anchor(
                    ui,
                    SettingsTarget::ToolbarStyle,
                    &mut settings_scroll_target,
                );
                settings_control_row(ui, SettingsTarget::ToolbarStyle.label(), |ui| {
                    egui::ComboBox::from_id_salt("toolbar-style")
                        .selected_text(edited.toolbar_style.label())
                        .show_ui(ui, |ui| {
                            for style in crate::settings::ToolbarStyle::ALL {
                                ui.selectable_value(
                                    &mut edited.toolbar_style,
                                    style,
                                    style.label(),
                                );
                            }
                        });
                });
                settings_target_anchor(ui, SettingsTarget::UiFont, &mut settings_scroll_target);
                settings_target_anchor(
                    ui,
                    SettingsTarget::UiFontWeight,
                    &mut settings_scroll_target,
                );
                settings_control_row(ui, SettingsTarget::UiFont.label(), |ui| {
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
                    if crate::app::icons::action_button(ui, "Choose…").clicked() {
                        self.actions
                            .push(SettingsAction::ChooseTool(ToolPickerTarget::UiFont));
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
            }
            SettingsSection::Editor => {
                settings_target_anchor(
                    ui,
                    SettingsTarget::EditorParticles,
                    &mut settings_scroll_target,
                );
                editor_particle_controls(ui, &mut edited.editor_particles);
                settings_target_anchor(
                    ui,
                    SettingsTarget::IndependentDividers,
                    &mut settings_scroll_target,
                );
                let dividers = ui.checkbox(&mut edited.independent_main_dividers, SettingsTarget::IndependentDividers.label())
                    .on_hover_text("Keep the code/preview divider in place when resizing Explorer. Disable to keep the code panel width fixed.");
                #[cfg(all(feature = "desktop-ui-tests", target_os = "macos"))]
                crate::desktop_test::observe("settings.dividers", &dividers);
                let _ = dividers;
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
                    let _wrap =
                        ui.checkbox(&mut edited.line_wrap, SettingsTarget::WrapLines.label());
                    #[cfg(all(feature = "desktop-ui-tests", target_os = "macos"))]
                    crate::desktop_test::observe("settings.editor.wrap", &_wrap);
                    ui.checkbox(
                        &mut edited.line_numbers,
                        SettingsTarget::LineNumbers.label(),
                    );
                    ui.checkbox(
                        &mut edited.sticky_context_rows,
                        SettingsTarget::StickyContextRows.label(),
                    );
                    ui.checkbox(&mut edited.auto_save, SettingsTarget::AutoSave.label());
                    settings_slider_enabled(
                        ui,
                        SettingsTarget::AutoSaveDelay.label(),
                        edited.auto_save,
                        egui::Slider::new(&mut edited.auto_save_delay_ms, 250..=5_000)
                            .suffix(" ms")
                            .logarithmic(true),
                    );
                });
                settings_target_anchor(
                    ui,
                    SettingsTarget::IndentationGuides,
                    &mut settings_scroll_target,
                );
                ui.horizontal_wrapped(|ui| {
                    let _guides = ui.checkbox(&mut edited.indentation_guides, "Indentation guides");
                    #[cfg(all(feature = "desktop-ui-tests", target_os = "macos"))]
                    crate::desktop_test::observe("settings.editor.guides", &_guides);
                    ui.label("Guide character");
                    let before = edited.indentation_guide_character.clone();
                    let response = ui.add(egui::TextEdit::singleline(&mut edited.indentation_guide_character).char_limit(1).desired_width(40.0));
                    if response.changed() && (edited.indentation_guide_character.chars().count() != 1 || edited.indentation_guide_character.chars().any(|c| c.is_control() || c.is_whitespace())) {
                        edited.indentation_guide_character = before;
                    }
                }).response.on_hover_text("Display-only lines at each indent level. Choose one visible character, e.g. │ or ┆. Saved whitespace is unchanged.");
                settings_target_anchor(
                    ui,
                    SettingsTarget::LocalUseHighlights,
                    &mut settings_scroll_target,
                );
                let _local_uses = ui.checkbox(&mut edited.highlight_local_uses, SettingsTarget::LocalUseHighlights.label())
                    .on_hover_text("Emphasize uses of names defined or changed in this document, including Typst bindings and TeX macros or environments. Declaration sites stay unchanged.");
                #[cfg(all(feature = "desktop-ui-tests", target_os = "macos"))]
                crate::desktop_test::observe("settings.editor.local-uses", &_local_uses);
                settings_target_anchor(
                    ui,
                    SettingsTarget::TextHighlights,
                    &mut settings_scroll_target,
                );
                egui::CollapsingHeader::new("Custom text highlights").open(self.state.query.to_lowercase().contains("highlight").then_some(true)).show(ui, |ui| {
                    ui.weak("Literal, case-sensitive matches in source. Later rows win on overlap. Unset colors follow the theme accent.");
                    super::settings_controls::show_text_highlight_editor(ui, &mut edited.highlight_rules, &self.font_configuration.editor_weight_support);
                });
                settings_target_anchor(
                    ui,
                    SettingsTarget::AutoPairDelimiters,
                    &mut settings_scroll_target,
                );
                ui.checkbox(&mut edited.auto_pair_delimiters, SettingsTarget::AutoPairDelimiters.label())
                    .on_hover_text("Typing an opening bracket, quote, dollar, asterisk, underscore or backtick surrounds selected text. Typst also pairs delimiters at an empty caret. Paste stays unchanged.");
                settings_target_anchor(
                    ui,
                    SettingsTarget::Indentation,
                    &mut settings_scroll_target,
                );
                ui.horizontal(|ui| {
                    ui.label(SettingsTarget::Indentation.label());
                    let mut spaces = edited.indent_spaces != 0;
                    ui.checkbox(&mut spaces, "Spaces");
                    if spaces {
                        if edited.indent_spaces == 0 {
                            edited.indent_spaces = 2;
                        }
                        ui.add(egui::DragValue::new(&mut edited.indent_spaces).range(1..=16));
                    } else {
                        edited.indent_spaces = 0;
                        ui.label("Tab character");
                    }
                });
                settings_target_anchor(
                    ui,
                    SettingsTarget::GitDiffStyle,
                    &mut settings_scroll_target,
                );
                ui.horizontal_wrapped(|ui| {
                    theme::apply_compact_control_spacing(ui);
                    ui.label(RichText::new(SettingsTarget::GitDiffStyle.label()).strong());
                    for style in GitDiffStyle::ALL {
                        ui.selectable_value(&mut edited.git_diff_style, style, style.label());
                    }
                });
                settings_target_anchor(
                    ui,
                    SettingsTarget::RainbowBrackets,
                    &mut settings_scroll_target,
                );
                show_bracket_controls(ui, &mut edited.rainbow_brackets);
                settings_target_anchor(ui, SettingsTarget::CodeFont, &mut settings_scroll_target);
                settings_target_anchor(
                    ui,
                    SettingsTarget::CodeFontWeight,
                    &mut settings_scroll_target,
                );
                settings_control_row(ui, SettingsTarget::CodeFont.label(), |ui| {
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
                    if crate::app::icons::action_button(ui, "Choose…").clicked() {
                        self.actions
                            .push(SettingsAction::ChooseTool(ToolPickerTarget::CodeFont));
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
                    SettingsTarget::ExplorerOrder,
                    &mut settings_scroll_target,
                );
                show_explorer_order_controls(ui, &mut edited.explorer_order);
                settings_target_anchor(ui, SettingsTarget::HoverDelay, &mut settings_scroll_target);
                settings_control_row(ui, SettingsTarget::HoverDelay.label(), |ui| {
                    ui.add(
                        egui::DragValue::new(&mut edited.hover_delay_ms)
                            .range(0..=2_000)
                            .speed(10)
                            .suffix(" ms"),
                    );
                });
            }
            SettingsSection::Writing => {
                settings_target_anchor(
                    ui,
                    SettingsTarget::EnglishGrammar,
                    &mut settings_scroll_target,
                );
                ui.checkbox(&mut edited.english_grammar, SettingsTarget::EnglishGrammar.label()).on_hover_text("Offline checks after a short typing pause. Documents up to 2 MB; suggestions appear in Problems and inline diagnostics.");
                ui.horizontal_wrapped(|ui| {
                    ui.label("Writing language");
                    for language in crate::settings::WritingLanguage::ALL {
                        ui.selectable_value(&mut edited.writing_language, language, language.label());
                    }
                }).response.on_hover_text("Auto reads top-level #set text(lang: \"en\", region: \"us\") in Typst. Use region: \"gb\" for UK English. lang: \"uk\" means Ukrainian. Other languages are not checked by Harper. Computed/imported settings use the UK fallback.");
                settings_target_anchor(
                    ui,
                    SettingsTarget::HarperCapitalizedNames,
                    &mut settings_scroll_target,
                );
                let names = ui.checkbox(
                    &mut edited.harper_ignore_capitalized_names,
                    SettingsTarget::HarperCapitalizedNames.label(),
                );
                #[cfg(all(feature = "desktop-ui-tests", target_os = "macos"))]
                crate::desktop_test::observe("settings.harper.ignore_names", &names);
                #[cfg(not(all(feature = "desktop-ui-tests", target_os = "macos")))]
                let _ = names;
                ui.label(
                    RichText::new("Leray, MacDonald — spelling only; grammar checks stay on.")
                        .weak(),
                );
                settings_target_anchor(
                    ui,
                    SettingsTarget::HarperDictionary,
                    &mut settings_scroll_target,
                );
                let label =
                    ui.label(RichText::new(SettingsTarget::HarperDictionary.label()).strong());
                ui.label(
                    RichText::new(
                        "One word per line. Built-in English and mathematics words stay included.",
                    )
                    .weak(),
                );
                let mut dictionary = self
                    .state
                    .harper_dictionary_draft
                    .take()
                    .filter(|(base, _)| base == &edited.harper_dictionary)
                    .map(|(_, text)| text)
                    .unwrap_or_else(|| edited.harper_dictionary.clone());
                let dictionary_response = egui::ScrollArea::vertical()
                    .id_salt("harper-personal-dictionary-scroll")
                    .max_height(140.0)
                    .show(ui, |ui| {
                        ui.add(
                            egui::TextEdit::multiline(&mut dictionary)
                                .id_salt("harper-personal-dictionary")
                                .font(egui::TextStyle::Monospace)
                                .desired_width(f32::INFINITY)
                                .desired_rows(6)
                                .char_limit(65_536)
                                .hint_text("One word per line"),
                        )
                        .labelled_by(label.id)
                    })
                    .inner;
                #[cfg(all(feature = "desktop-ui-tests", target_os = "macos"))]
                crate::desktop_test::observe("settings.harper.dictionary", &dictionary_response);
                #[cfg(not(all(feature = "desktop-ui-tests", target_os = "macos")))]
                let _ = dictionary_response;
                match crate::writing::validate_dictionary(&dictionary) {
                    Ok(()) => edited.harper_dictionary = dictionary,
                    Err(error) => {
                        ui.colored_label(ui.visuals().error_fg_color, error);
                        self.state.harper_dictionary_draft =
                            Some((edited.harper_dictionary.clone(), dictionary));
                    }
                }
                settings_target_anchor(
                    ui,
                    SettingsTarget::Handwriting,
                    &mut settings_scroll_target,
                );
                crate::handwriting::settings_ui(ui, &mut edited.handwriting);
                settings_target_anchor(
                    ui,
                    SettingsTarget::UnicodeWarnings,
                    &mut settings_scroll_target,
                );
                ui.checkbox(
                    &mut edited.unicode_warnings,
                    SettingsTarget::UnicodeWarnings.label(),
                );
                settings_target_anchor(
                    ui,
                    SettingsTarget::AsciiPunctuation,
                    &mut settings_scroll_target,
                );
                let ascii = ui.checkbox(
                    &mut edited.ascii_punctuation,
                    SettingsTarget::AsciiPunctuation.label(),
                );
                #[cfg(all(feature = "desktop-ui-tests", target_os = "macos"))]
                crate::desktop_test::observe("settings.editor.ascii", &ascii);
                #[cfg(not(all(feature = "desktop-ui-tests", target_os = "macos")))]
                let _ = ascii;
                settings_target_anchor(
                    ui,
                    SettingsTarget::AutoFormatMath,
                    &mut settings_scroll_target,
                );
                let _math_format = ui.checkbox(&mut edited.format_after_math_conversion, SettingsTarget::AutoFormatMath.label())
                    .on_hover_text("Format the document after switching inline/display math. Uses the selected formatter and ordinary auto-save settings.");
                #[cfg(all(feature = "desktop-ui-tests", target_os = "macos"))]
                crate::desktop_test::observe("settings.editor.math_format", &_math_format);
                settings_target_anchor(
                    ui,
                    SettingsTarget::MitexDollars,
                    &mut settings_scroll_target,
                );
                ui.checkbox(
                    &mut edited.mitex_auto_enable,
                    SettingsTarget::MitexDollars.label(),
                );
                ui.horizontal_wrapped(|ui| {
                    ui.label("MiTeX version");
                    ui.add(egui::TextEdit::singleline(&mut edited.mitex_version).desired_width(65.0))
                        .on_hover_text("Pinned package version used when enabling TeX mode. Existing imports are not rewritten.");
                });
                ui.label("$x$ → mi · $ x $ → mitex. Native Typst math prevents enabling.");
            }
            SettingsSection::Preview => {
                settings_target_anchor(
                    ui,
                    SettingsTarget::PreviewBackend,
                    &mut settings_scroll_target,
                );
                ui.horizontal_wrapped(|ui| {
                    for preference in PreviewPreference::ALL {
                        let _response = ui.selectable_value(
                            &mut edited.preview_preference,
                            preference,
                            preference.label(),
                        )
                        .on_hover_text(match preference {
                            PreviewPreference::Interactive => "Tinymist provides interactive Typst preview and source navigation. TeX and opened PDFs always use PDFium. Errors do not switch renderers.",
                            PreviewPreference::InteractivePartial => "Default Typst viewer: render the visible area and a nearby buffer, refilling as you scroll. The complete Tinymist renderer remains a separate option. TeX and opened PDFs always use PDFium.",
                            PreviewPreference::Pdfium => "PDFium displays compiled Typst and TeX output and opened PDFs, retaining the previous page while an update renders.",
                        });
                        #[cfg(all(feature = "desktop-ui-tests", target_os = "macos"))]
                        if ui.is_rect_visible(_response.rect) {
                            crate::desktop_test::observe(&format!("settings.backend.{preference:?}"), &_response);
                        }
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
                ui.label("TeX and opened PDFs always use PDFium.");
                if !deterministic_settings
                    && let Some(reason) = self.status.preview_failure.as_ref()
                {
                    ui.colored_label(ui.visuals().error_fg_color, reason);
                }
                settings_target_anchor(
                    ui,
                    SettingsTarget::PreviewZoomSnapshot,
                    &mut settings_scroll_target,
                );
                let _snapshot_response = ui.add_enabled(cfg!(target_os = "macos"), egui::Checkbox::new(
                    &mut edited.preview_zoom_snapshot, SettingsTarget::PreviewZoomSnapshot.label(),
                )).on_hover_text("Scale a cached, higher-resolution viewport image while Tinymist redraws, then reveal the sharp preview. Works with both Tinymist modes on macOS. Keeps one image in memory, capped at four million pixels.");
                #[cfg(all(feature = "desktop-ui-tests", target_os = "macos"))]
                if ui.is_rect_visible(_snapshot_response.rect) {
                    crate::desktop_test::observe(
                        "settings.preview.zoom_snapshot",
                        &_snapshot_response,
                    );
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
                    "Automatically scroll the Tinymist preview to your edit after compilation. Does not move the editor cursor. PDFium previews do not support source jumps.",
                );
                if !deterministic_settings
                    && self.status.requested_backend.is_interactive()
                    && !self.status.interactive_active
                    && crate::app::icons::action_button(ui, "Retry Tinymist").clicked()
                {
                    self.actions.push(SettingsAction::RetryTinymist);
                }
                settings_target_anchor(
                    ui,
                    SettingsTarget::PreviewJump,
                    &mut settings_scroll_target,
                );
                settings_control_row(ui, SettingsTarget::PreviewJump.label(), |ui| {
                    egui::ComboBox::from_id_salt("source-preview-trigger")
                        .width(
                            ui.available_width()
                                .min(METRICS.settings.source_preview_trigger_width),
                        )
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
                });
            }
            SettingsSection::Tools => {
                ui.label(
                    RichText::new(
                        "Packaged builds use pinned sidecars. A custom path overrides one tool without changing the other.",
                    )
                    .size(theme::TYPE.supporting)
                    .color(ui.visuals().weak_text_color()),
                );
                ui.strong("Typst tools");
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
                    self.settings,
                ) {
                    self.actions
                        .push(SettingsAction::ChooseTool(ToolPickerTarget::Typst));
                }
                if !deterministic_settings && let Some(reason) = &staged_typst.fallback_reason {
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
                    self.settings,
                ) {
                    self.actions
                        .push(SettingsAction::ChooseTool(ToolPickerTarget::Tinymist));
                }
                if !deterministic_settings && let Some(reason) = &staged_tinymist.fallback_reason {
                    fallback_notice(ui, "Binary fallback active", reason);
                }
                configuration_editor(
                    ui,
                    crate::toolchain::ToolKind::Tinymist,
                    &mut edited.tinymist_configuration,
                    self.settings,
                    None,
                );
                settings_target_anchor(
                    ui,
                    SettingsTarget::TexServices,
                    &mut settings_scroll_target,
                );
                ui.scope(|ui| {
                    ui.strong("TeX tools");
                    show_tex_preferences(ui, &mut edited.tex, self.tex_tools, self.settings);
                    for (kind, preference, resolution) in [
                        (
                            crate::toolchain::ToolKind::Tectonic,
                            &mut edited.tex.tectonic,
                            &self.tex_tools.tectonic,
                        ),
                        (
                            crate::toolchain::ToolKind::Texlab,
                            &mut edited.tex.texlab,
                            &self.tex_tools.texlab,
                        ),
                        (
                            crate::toolchain::ToolKind::Badness,
                            &mut edited.tex.badness,
                            &self.tex_tools.badness,
                        ),
                        (
                            crate::toolchain::ToolKind::TexFmt,
                            &mut edited.tex.tex_fmt,
                            &self.tex_tools.tex_fmt,
                        ),
                    ] {
                        if tool_preference_editor(
                            ui,
                            kind.label(),
                            preference,
                            resolution,
                            deterministic_settings,
                            self.settings,
                        ) {
                            self.actions
                                .push(SettingsAction::ChooseTool(ToolPickerTarget::Tex(kind)));
                        }
                        if !deterministic_settings && let Some(reason) = &resolution.fallback_reason
                        {
                            fallback_notice(ui, kind.label(), reason);
                        }
                    }
                    ui.strong("LaTeXdiff");
                    super::latexdiff::preferences(ui, &mut edited.latexdiff);
                    configuration_editor(
                        ui,
                        crate::toolchain::ToolKind::Texlab,
                        &mut edited.tex.texlab_configuration,
                        self.settings,
                        Some(&mut edited.tex.texlab_ignored_codes),
                    );
                    configuration_editor(
                        ui,
                        crate::toolchain::ToolKind::Badness,
                        &mut edited.tex.badness_configuration,
                        self.settings,
                        Some(&mut edited.tex.badness_ignored_codes),
                    );
                });
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
                ui.separator();
                ui.strong("Service status");
                let status = if deterministic_settings {
                    format!("{} (deterministic QA build)", crate::build_info::APP_NAME)
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
                        for name in ["Editing", "LSP", "Interactive", "PDF", "Rendering", "Links"] {
                            show_status_chip(ui, name, "Ready", "Ready", syntax_color);
                        }
                    } else {
                        let capabilities = &self.status.capabilities;
                        show_service_status_chip(ui, "Editing", &capabilities.editing);
                        show_service_status_chip(ui, "LSP", &capabilities.lsp);
                        show_service_status_chip(
                            ui,
                            "Interactive",
                            &capabilities.interactive_preview,
                        );
                        show_service_status_chip(ui, "PDF", &capabilities.pdf_generation);
                        show_service_status_chip(ui, "Rendering", &capabilities.pdf_rendering);
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
                    if crate::app::icons::action_button(ui, "Main").clicked() {
                        self.captures.queue("main", "main");
                    }
                    if crate::app::icons::action_button(ui, "Settings").clicked() {
                        self.captures.queue("settings", "settings");
                    }
                    if crate::app::icons::action_button(ui, "Both").clicked() {
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
            }
            SettingsSection::Shortcuts => {
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
                        RichText::new(
                            "All application, editor, build, preview, and window bindings",
                        )
                        .size(theme::TYPE.supporting)
                        .weak(),
                    );
                });
            }
            SettingsSection::Snippets => {
                settings_target_anchor(ui, SettingsTarget::Snippets, &mut settings_scroll_target);
                let snippets = ui.button("Edit snippets…");
                #[cfg(all(feature = "desktop-ui-tests", target_os = "macos"))]
                crate::desktop_test::observe("settings.snippets.open", &snippets);
                if snippets.clicked() {
                    self.actions.push(SettingsAction::ShowSnippets);
                }
                if let Some(selection) =
                    super::settings_snippets::library(ui, &mut self.state.snippet_library, edited)
                {
                    self.actions.push(SettingsAction::EditSnippet(selection));
                }
            }
            SettingsSection::Templates => {
                settings_target_anchor(ui, SettingsTarget::Templates, &mut settings_scroll_target);
                ui.scope(|ui| {
                    ui.weak("These templates appear in New from template. Changes are saved with Settings.");
                    let mut remove = None;
                    for (index, template) in edited.templates.iter_mut().enumerate() {
                        ui.push_id(("template", index), |ui| {
                            ui.horizontal(|ui| {
                                ui.label("Name"); ui.text_edit_singleline(&mut template.name);
                                if ui.small_button("Remove").clicked() { remove = Some(index); }
                            });
                            ui.horizontal(|ui| {
                                ui.selectable_value(&mut template.language, crate::document_templates::Language::Typst, "Typst");
                                ui.selectable_value(&mut template.language, crate::document_templates::Language::Tex, "TeX");
                            });
                            super::code_editor::editor(ui, "template-source", &mut template.source, template.language.extension(), self.settings, 8);
                            ui.separator();
                        });
                    }
                    if let Some(index) = remove { edited.templates.remove(index); }
                    if ui.button("Add template").clicked() { edited.templates.push(crate::document_templates::Template {name:"New template".into(), language:crate::document_templates::Language::Typst, source:String::new()}); }
                    if ui.button("Restore default templates").clicked() { edited.templates = crate::document_templates::defaults(); }
                });
            }
        }
        settings_scroll_target
    }
}

fn reveal_animation_controls(ui: &mut egui::Ui, label: &str, animation: &mut RevealAnimation) {
    ui.push_id(label, |ui| {
        ui.horizontal_wrapped(|ui| {
            let enabled = ui.checkbox(&mut animation.enabled, label);
            let style = egui::ComboBox::from_id_salt("style")
                .width(94.0)
                .selected_text(animation.style.label())
                .show_ui(ui, |ui| {
                    for value in RevealStyle::ALL {
                        ui.selectable_value(&mut animation.style, value, value.label());
                    }
                })
                .response
                .labelled_by(enabled.id);
            let duration_label = ui.label("Duration");
            let duration = ui
                .add(
                    egui::DragValue::new(&mut animation.duration_ms)
                        .range(250..=2000)
                        .speed(25)
                        .suffix(" ms"),
                )
                .labelled_by(duration_label.id);
            let size_label = ui.label("Size");
            let size = ui
                .add(
                    egui::DragValue::new(&mut animation.size_percent)
                        .range(50..=200)
                        .speed(5)
                        .suffix("%"),
                )
                .labelled_by(size_label.id);
            #[cfg(all(feature = "desktop-ui-tests", target_os = "macos"))]
            for (name, response) in [
                ("enabled", &enabled),
                ("style", &style),
                ("duration", &duration),
                ("size", &size),
            ] {
                let scope = if label == SettingsTarget::SourceRevealAnimation.label() {
                    "source"
                } else {
                    "preview"
                };
                crate::desktop_test::observe(&format!("settings.reveal.{scope}.{name}"), response);
            }
            let _ = (enabled, style, duration, size);
        });
    });
}

fn editor_particle_controls(ui: &mut egui::Ui, particles: &mut EditorParticleSettings) {
    ui.horizontal_wrapped(|ui| {
        let enabled = ui.checkbox(
            &mut particles.enabled,
            SettingsTarget::EditorParticles.label(),
        );
        let style_label = ui.label("Style");
        let style = egui::ComboBox::from_id_salt("typing-particle-style")
            .width(128.0)
            .selected_text(particles.style.label())
            .show_ui(ui, |ui| {
                for value in ParticleStyle::ALL {
                    let response = ui.selectable_value(&mut particles.style, value, value.label());
                    #[cfg(all(feature = "desktop-ui-tests", target_os = "macos"))]
                    crate::desktop_test::observe(
                        &format!("settings.editor.particles.style.{}", value.label()),
                        &response,
                    );
                    let _ = response;
                }
            })
            .response
            .labelled_by(style_label.id);
        let density_label = ui.label("Density");
        let density = egui::ComboBox::from_id_salt("typing-particle-density")
            .width(94.0)
            .selected_text(particles.density.label())
            .show_ui(ui, |ui| {
                for value in ParticleDensity::ALL {
                    ui.selectable_value(&mut particles.density, value, value.label());
                }
            })
            .response
            .labelled_by(density_label.id);
        #[cfg(all(feature = "desktop-ui-tests", target_os = "macos"))]
        {
            crate::desktop_test::observe("settings.editor.particles.enabled", &enabled);
            crate::desktop_test::observe("settings.editor.particles.style", &style);
            crate::desktop_test::observe("settings.editor.particles.density", &density);
        }
        let _ = (enabled, style, density);
    });
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

fn show_theme_color_reset(
    ui: &mut egui::Ui,
    transform: &mut crate::theme_transform::ThemeTransform,
) {
    if crate::app::icons::action_button_enabled(
        ui,
        *transform != Default::default(),
        "Reset colors",
    )
    .clicked()
    {
        *transform = Default::default();
    }
}

pub(super) fn show_theme_color_controls(
    ui: &mut egui::Ui,
    transform: &mut crate::theme_transform::ThemeTransform,
) {
    show_theme_color_reset(ui, transform);
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
                if crate::app::icons::action_button_enabled(
                    ui,
                    *order != ExplorerOrder::default(),
                    "Reset panel order",
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

fn show_tex_preferences(
    ui: &mut egui::Ui,
    settings: &mut crate::tex::settings::TexSettings,
    tools: &crate::tex::tools::TexTools,
    _app_settings: &AppSettings,
) {
    use crate::tex::settings::{BuildEngine, Formatter};
    ui.checkbox(&mut settings.build_enabled, "Build TeX documents");
    ui.horizontal_wrapped(|ui| {
        ui.label("Build engine:");
        for engine in BuildEngine::ALL {
            ui.selectable_value(&mut settings.build_engine, engine, engine.label());
        }
    });
    if settings.build_engine != BuildEngine::Tectonic {
        if let Some(path) = tools.distribution(settings.build_engine) {
            ui.weak(format!("Detected: {}", path.display()));
        } else {
            ui.weak("Not found. Install a TeX distribution such as MacTeX and restart the app.");
        }
    }
    ui.add_enabled_ui(settings.build_engine == BuildEngine::Tectonic, |ui| {
        ui.checkbox(&mut settings.only_cached, "Use cached TeX packages only")
            .on_hover_text(
                "Otherwise Tectonic downloads missing packages. Shell escape is disabled.",
            );
    });
    ui.checkbox(&mut settings.synctex, "SyncTeX source/preview navigation")
        .on_hover_text("Emit a SyncTeX map so source and PDF positions can follow each other.");
    ui.checkbox(&mut settings.texlab_enabled, "TexLab editor intelligence");
    ui.indent("texlab-features", |ui| {
        ui.checkbox(&mut settings.completion, "TeX completions");
        ui.checkbox(&mut settings.hover, "TeX hover documentation");
        ui.checkbox(&mut settings.diagnostics, "TexLab diagnostics");
    });
    ui.horizontal_wrapped(|ui| {
        ui.label("TeX formatter:");
        for choice in [Formatter::Badness, Formatter::TexFmt, Formatter::Disabled] {
            ui.selectable_value(&mut settings.formatter, choice, choice.label());
        }
    });
    ui.checkbox(&mut settings.lint, "Badness linting");
    ui.checkbox(
        &mut settings.embedded_diagnostics,
        "Check TeX math in auto-miTeX documents",
    );
    ui.checkbox(
        &mut settings.embedded_formatting,
        "Use TeX formatter for auto-miTeX math",
    )
    .on_hover_text("Only math payloads are formatted. Surrounding Typst remains unchanged.");
    ui.label(
        egui::RichText::new(
            "The main buffer is built from the editor; included files are read from disk.",
        )
        .weak(),
    );
}

fn image_appearance_preference(ui: &mut egui::Ui, settings: &mut AppSettings) {
    let _images = ui.checkbox(&mut settings.dark_images, SettingsTarget::DarkImages.label())
        .on_hover_text("Invert image colors when pages are dark. Light pages keep the original colors; image files are unchanged.");
    #[cfg(all(feature = "desktop-ui-tests", target_os = "macos"))]
    if ui.is_rect_visible(_images.rect) {
        crate::desktop_test::observe("settings.dark_images", &_images);
    }
}

fn ignored_codes_editor(ui: &mut egui::Ui, ignored_codes: &mut Vec<String>) {
    ui.horizontal(|ui| {
        ui.label("Ignored diagnostic codes:");
        let id = ui.id().with("ignored-codes-draft");
        let mut codes = ui
            .ctx()
            .data(|data| data.get_temp::<(Vec<String>, String)>(id))
            .filter(|(original, _)| original == &*ignored_codes)
            .map(|(_, text)| text)
            .unwrap_or_else(|| ignored_codes.join(", "));
        if ui
            .add(egui::TextEdit::singleline(&mut codes).hint_text("e.g. redundant-script-braces"))
            .changed()
        {
            *ignored_codes = codes
                .split(',')
                .map(str::trim)
                .filter(|code| !code.is_empty())
                .map(str::to_owned)
                .collect();
        }
        ui.ctx()
            .data_mut(|data| data.insert_temp(id, (ignored_codes.clone(), codes)));
    });
    ui.weak("Comma-separated codes, for this provider only. Empty means no exclusions.");
}

fn configuration_editor(
    ui: &mut egui::Ui,
    kind: crate::toolchain::ToolKind,
    value: &mut serde_json::Value,
    app_settings: &AppSettings,
    ignored_codes: Option<&mut Vec<String>>,
) {
    use crate::toolchain::ToolKind;
    let (example, docs) = match kind {
        ToolKind::Tinymist => (
            r#"{"formatterPrintWidth":100,"typstExtraArgs":["--input=mode=draft"]}"#,
            "https://github.com/Myriad-Dreamin/tinymist/blob/v0.15.2/crates/tinymist/src/config.rs",
        ),
        ToolKind::Texlab => (
            r#"{"texlab":{"diagnosticsDelay":500}}"#,
            "https://github.com/latex-lsp/texlab/wiki/Configuration#texlabdiagnosticsdelay",
        ),
        ToolKind::Badness => (
            r#"{"lineWidth":100,"indentWidth":4}"#,
            "https://github.com/jolars/badness/blob/v0.24.0/docs/src/guide/editor-setup.md",
        ),
        _ => unreachable!("only language servers have JSON configuration"),
    };
    let label = format!("{} configuration (JSON)", kind.label());
    let header = egui::CollapsingHeader::new(label).show(ui, |ui| {
        super::settings_controls::documentation_link(ui, "Configuration options", docs);
        ui.weak("Example override:");
        ui.monospace(example);
        if kind == ToolKind::Tinymist {
            ui.weak("Use flat keys. typstExtraArgs accepts --input=KEY=VALUE options; the app controls the entry and preview connection.");
        }
        if let Some(codes) = ignored_codes {
            ignored_codes_editor(ui, codes);
        }
        ui.weak(
            "These are the settings sent to the language server. Edit and apply to override them.",
        );
        let id = ui.id().with("configuration-draft");
        let mut text = ui
            .ctx()
            .data_mut(|data| data.get_temp::<(serde_json::Value, String)>(id))
            .filter(|(original, _)| original == value)
            .map(|(_, draft)| draft)
            .unwrap_or_else(|| serde_json::to_string_pretty(value).unwrap());
        super::code_editor::editor(ui, "configuration-code", &mut text, "json", app_settings, 6);
        let parsed = if text.len() > 2_000_000 {
            Err("Configuration JSON exceeds 2 MB".to_owned())
        } else {
            serde_json::from_str::<serde_json::Value>(&text).map_err(|e| e.to_string())
        }
            .and_then(|v| {
                if v.is_object() {
                    if kind == ToolKind::Tinymist {
                        crate::tinymist::validate_server_configuration(&v)?;
                    }
                    Ok(v)
                } else {
                    Err("Configuration must be a JSON object".into())
                }
            });
        if ui
            .add_enabled(parsed.is_ok(), egui::Button::new("Apply configuration"))
            .clicked()
        {
            *value = parsed.as_ref().unwrap().clone();
        }
        if let Err(error) = parsed {
            ui.colored_label(ui.visuals().error_fg_color, error);
        }
        if ui.button("Reset configuration").clicked() {
            *value = match kind {
                ToolKind::Tinymist => crate::tinymist::default_server_configuration(),
                ToolKind::Texlab => {
                    crate::tex::settings::TexSettings::default().texlab_configuration
                }
                ToolKind::Badness => {
                    crate::tex::settings::TexSettings::default().badness_configuration
                }
                _ => unreachable!(),
            };
            text = serde_json::to_string_pretty(value).unwrap();
        }
        ui.ctx()
            .data_mut(|data| data.insert_temp(id, (value.clone(), text)));
    });
    header.header_response.on_hover_ui(|ui| {
        ui.label("Override this language server’s settings; the defaults are shown in the editor.");
        ui.monospace(example);
        super::settings_controls::documentation_link(ui, "Configuration options", docs);
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
    fn reveal_and_typing_effect_controls_change_only_their_own_preferences() {
        let mut harness = Harness::builder()
            .with_size(Vec2::new(680.0, 220.0))
            .build_ui_state(
                |ui, settings: &mut AppSettings| {
                    reveal_animation_controls(
                        ui,
                        "Reveal in source",
                        &mut settings.source_reveal_animation,
                    );
                    editor_particle_controls(ui, &mut settings.editor_particles);
                },
                AppSettings::default(),
            );
        harness.run();
        harness
            .get_by_role_and_label(egui::accesskit::Role::CheckBox, "Reveal in source")
            .click();
        harness.run();
        assert!(!harness.state().source_reveal_animation.enabled);
        harness
            .get_by_role_and_label(egui::accesskit::Role::ComboBox, "Reveal in source")
            .click();
        harness.run();
        harness.get_by_label("Spotlight").click();
        harness.run();
        assert_eq!(
            harness.state().source_reveal_animation.style,
            RevealStyle::Spotlight
        );
        harness.get_by_label("Editor particles").click();
        harness.run();
        harness
            .get_by_role_and_label(egui::accesskit::Role::ComboBox, "Density")
            .click();
        harness.run();
        harness.get_by_label("High").click();
        harness.run();
        assert!(harness.state().editor_particles.enabled);
        assert_eq!(
            harness.state().editor_particles.density,
            ParticleDensity::High
        );
        assert_eq!(
            harness.state().preview_reveal_animation,
            RevealAnimation::default()
        );
        assert_eq!(harness.state().source_reveal_animation.duration_ms, 900);
        assert_eq!(harness.state().source_reveal_animation.size_percent, 100);
    }

    #[test]
    fn typing_particle_style_control_selects_every_style_without_enabling_effects() {
        let mut harness = Harness::builder()
            .with_size(Vec2::new(680.0, 220.0))
            .build_ui_state(
                |ui, settings: &mut AppSettings| {
                    editor_particle_controls(ui, &mut settings.editor_particles);
                },
                AppSettings::default(),
            );
        harness.run();
        for style in ParticleStyle::ALL.into_iter().rev() {
            harness
                .get_by_role_and_label(egui::accesskit::Role::ComboBox, "Style")
                .click();
            harness.run();
            harness.get_by_label(style.label()).click();
            harness.run();
            assert_eq!(harness.state().editor_particles.style, style);
            assert!(!harness.state().editor_particles.enabled);
            assert_eq!(
                harness.state().editor_particles.density,
                ParticleDensity::Medium
            );
            assert_eq!(
                harness.state().source_reveal_animation,
                RevealAnimation::default()
            );
            assert_eq!(
                harness.state().preview_reveal_animation,
                RevealAnimation::default()
            );
        }
    }

    #[test]
    fn documentation_link_queues_the_exact_url_without_opening_a_browser() {
        let mut harness = Harness::builder().build_ui(|ui| {
            super::super::settings_controls::documentation_link(
                ui,
                "Options",
                "https://badness.dev/reference/cli.html#options",
            );
        });
        harness.run();
        harness.get_by_label("Options").click();
        harness.run();
        let id = egui::Id::new(("settings-documentation", harness.ctx.viewport_id()));
        assert_eq!(
            harness
                .ctx
                .data_mut(|data| data.remove_temp::<String>(id))
                .as_deref(),
            Some("https://badness.dev/reference/cli.html#options")
        );
    }

    #[test]
    fn image_appearance_toggle_keeps_pdf_and_comfy_choices() {
        let mut harness = Harness::builder().build_ui_state(
            |ui, settings: &mut AppSettings| image_appearance_preference(ui, settings),
            AppSettings::default(),
        );
        harness.run();
        harness
            .get_by_label("Apply dark page appearance to images")
            .click();
        harness.run();
        assert!(!harness.state().dark_images);
        assert_eq!(
            harness.state().document_theme,
            DocumentTheme::FollowInterface
        );
        assert!(harness.state().comfy_background);
        assert!(!harness.state().comfy_text);
        harness
            .get_by_label("Apply dark page appearance to images")
            .click();
        harness.run();
        assert!(harness.state().dark_images);
    }

    #[test]
    fn tinymist_configuration_editor_applies_valid_json_and_resets_visible_defaults() {
        use egui_kittest::kittest::by;
        let mut harness = Harness::builder()
            .with_size(Vec2::new(1000.0, 900.0))
            .build_ui_state(
                |ui, settings: &mut AppSettings| {
                    let original = settings.clone();
                    configuration_editor(
                        ui,
                        crate::toolchain::ToolKind::Tinymist,
                        &mut settings.tinymist_configuration,
                        &original,
                        None,
                    );
                },
                AppSettings::default(),
            );
        theme::configure_test_fonts(&harness.ctx);
        harness.run();
        harness
            .get_by_label("Tinymist configuration (JSON)")
            .click();
        harness.run();
        harness
            .get(by().role(egui::accesskit::Role::MultilineTextInput))
            .focus();
        harness.run();
        harness.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::A);
        harness
            .get(by().role(egui::accesskit::Role::MultilineTextInput))
            .type_text(r#"{"formatterMode":"invalid"}"#);
        harness.run();
        let defaults = crate::tinymist::default_server_configuration();
        assert_eq!(harness.state().tinymist_configuration, defaults);
        assert!(
            harness
                .query_by_label("formatterMode: choose typstyle, typstfmt, disable")
                .is_some()
        );
        harness
            .get(by().role(egui::accesskit::Role::MultilineTextInput))
            .focus();
        harness.run();
        harness.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::A);
        harness
            .get(by().role(egui::accesskit::Role::MultilineTextInput))
            .type_text(r#"{"formatterPrintWidth":100}"#);
        harness.run();
        harness.get_by_label("Apply configuration").click();
        harness.run();
        assert_eq!(
            harness.state().tinymist_configuration["formatterPrintWidth"],
            100
        );
        harness.get_by_label("Reset configuration").click();
        harness.run();
        assert_eq!(harness.state().tinymist_configuration, defaults);
    }

    #[test]
    fn ignored_diagnostic_codes_keep_the_comma_while_typing() {
        use crate::tex::settings::TexSettings;
        use egui_kittest::kittest::by;
        let mut harness = Harness::builder()
            .with_size(Vec2::new(900.0, 1200.0))
            .build_ui_state(
                |ui, settings: &mut TexSettings| {
                    ignored_codes_editor(ui, &mut settings.badness_ignored_codes)
                },
                TexSettings::default(),
            );
        harness.run();
        harness
            .get(by().role(egui::accesskit::Role::TextInput))
            .focus();
        harness.run();
        harness
            .get(by().role(egui::accesskit::Role::TextInput))
            .type_text("first,");
        harness.run();
        harness
            .get(by().role(egui::accesskit::Role::TextInput))
            .type_text(" second");
        harness.run();
        assert_eq!(harness.state().badness_ignored_codes, ["first", "second"]);
    }

    #[test]
    fn tex_controls_keep_build_intelligence_format_and_lint_independent() {
        use crate::tex::settings::{Formatter, TexSettings};
        let tools = crate::tex::tools::TexTools::resolve(&TexSettings::default());
        let mut harness = Harness::builder()
            .with_size(Vec2::new(700.0, 600.0))
            .build_ui_state(
                |ui, settings: &mut TexSettings| {
                    show_tex_preferences(ui, settings, &tools, &AppSettings::default())
                },
                TexSettings::default(),
            );
        harness.run();
        harness.get_by_label("Build TeX documents").click();
        harness.run();
        assert!(!harness.state().build_enabled);
        assert!(harness.state().texlab_enabled && harness.state().lint);
        harness.get_by_label("TexLab editor intelligence").click();
        harness.run();
        assert!(!harness.state().texlab_enabled);
        assert!(harness.state().lint);
        harness.get_by_label("tex-fmt").click();
        harness.run();
        assert_eq!(harness.state().formatter, Formatter::TexFmt);
        assert!(harness.state().needs_badness());
        harness.get_by_label("Badness linting").click();
        harness.run();
        assert!(!harness.state().needs_badness());
        harness.get_by_label("TeX completions").click();
        harness.run();
        assert!(!harness.state().completion);
        assert!(harness.state().hover && harness.state().diagnostics);
    }

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
            bundled_program: None,
            command: Default::default(),
            kind: ToolKind::Typst,
            program: "typst".into(),
            origin: ToolOrigin::Bundled,
            fallback_reason: None,
        };
        let tinymist = ToolResolution {
            bundled_program: None,
            command: Default::default(),
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
                        tex_tools: &crate::tex::tools::TexTools::resolve(
                            &crate::tex::settings::TexSettings::default(),
                        ),
                        status: SettingsStatus {
                            backend_label: "PDFium",
                            preview_failure: None,
                            requested_backend: PreviewPreference::Interactive,
                            interactive_active: false,
                            capabilities: CapabilitySnapshot {
                                editing: ServiceState::Ready("ready".into()),
                                lsp: ServiceState::Failed("failure".into()),
                                interactive_preview: ServiceState::Failed("failure".into()),
                                pdf_generation: ServiceState::Ready("ready".into()),
                                pdf_rendering: ServiceState::Failed(
                                    "PDF page decoding failed".into(),
                                ),
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
                    AppSettings {
                        preview_preference: PreviewPreference::Interactive,
                        ..AppSettings::default()
                    },
                    Vec::new(),
                    None,
                ),
            );
        harness.run();
        fn category(
            harness: &mut Harness<
                '_,
                (
                    SettingsUiState,
                    AppSettings,
                    Vec<SettingsAction>,
                    Option<AppSettings>,
                ),
            >,
            section: SettingsSection,
        ) {
            use egui_kittest::kittest::by;
            harness
                .get(
                    by().label(section.title())
                        .role(egui::accesskit::Role::Button),
                )
                .click();
            harness.run();
            assert_eq!(harness.state().0.category, section);
        }
        category(&mut harness, SettingsSection::Tools);
        for capability in ["Editing", "LSP", "Interactive", "PDF", "Rendering", "Links"] {
            harness.get_by_label(capability);
        }
        assert!(
            harness.state().2.is_empty(),
            "idle Settings must not emit updates"
        );
        category(&mut harness, SettingsSection::Snippets);
        harness.get_by_label("Edit snippets…").click();
        harness.run();
        assert!(
            harness
                .state()
                .2
                .iter()
                .any(|action| matches!(action, SettingsAction::ShowSnippets))
        );
        assert!(
            harness.query_by_label("Snippet source").is_none(),
            "the manager belongs to its own window"
        );
        harness.state_mut().2.clear();
        category(&mut harness, SettingsSection::Preview);
        harness
            .get_by_label("Tinymist partial (experimental)")
            .click();
        harness.run();
        assert_eq!(
            harness.state().1.preview_preference,
            PreviewPreference::Interactive
        );
        assert!(harness.state().2.iter().any(|action| matches!(action,
            SettingsAction::Update(settings) if settings.preview_preference == PreviewPreference::InteractivePartial)));
        harness.state_mut().2.clear();
        if cfg!(target_os = "macos") {
            harness.get_by_label("Smooth Tinymist zoom").click();
            harness.run();
            assert!(harness.state().2.iter().any(|action| matches!(action,
                SettingsAction::Update(settings) if !settings.preview_zoom_snapshot)));
        } else {
            // Native snapshot support is disabled on other platforms.
            harness.get_by_label("Smooth Tinymist zoom");
            assert!(harness.state().2.is_empty());
        }
        harness.state_mut().2.clear();
        harness
            .get_by_label("Follow Typst edits in preview")
            .click();
        harness.run();
        assert!(harness.state().1.preview_follow_edits);
        assert!(harness.state().2.iter().any(|action| {
            matches!(action, SettingsAction::Update(settings) if !settings.preview_follow_edits)
        }));
        harness.state_mut().2.clear();
        category(&mut harness, SettingsSection::Editor);
        harness.get_by_label("Spaces").click();
        harness.run();
        assert!(harness.state().2.iter().any(|action| matches!(action, SettingsAction::Update(settings) if settings.indent_spaces == 0)));
        harness.state_mut().2.clear();
        category(&mut harness, SettingsSection::Writing);
        harness
            .get_by_label(SettingsTarget::AsciiPunctuation.label())
            .click();
        harness.run();
        assert!(harness.state().2.iter().any(|action| matches!(action, SettingsAction::Update(settings) if settings.ascii_punctuation)));
        harness.state_mut().2.clear();
        category(&mut harness, SettingsSection::Preview);
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
        category(&mut harness, SettingsSection::Appearance);
        assert!(
            harness.query_by_label("Invert colors").is_none(),
            "advanced colors start collapsed"
        );
        harness.state_mut().0.scroll_target = Some(SettingsTarget::InvertColors);
        harness.run();
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
        category(&mut harness, SettingsSection::Editor);
        harness.get_by_label("Side-by-side").click();
        harness.run();
        assert!(harness.state().2.iter().any(
            |a| matches!(a, SettingsAction::Update(settings) if settings.git_diff_style
                == crate::settings::GitDiffStyle::SideBySide)
        ));
        harness.state_mut().2.clear();
        category(&mut harness, SettingsSection::Appearance);
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

#[cfg(test)]
#[path = "settings_writing_tests.rs"]
mod writing_tests;
