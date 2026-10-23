//! Reusable Settings controls operating on borrowed values, not the application.
use super::{
    error_color, info_color, neutral_color, success_color,
    tooltips::{settings_hover_text, typst_overrides_hover_text},
    warning_color,
};
use crate::{
    builtin_themes,
    font_catalog::FontCatalog,
    preview::ServiceState,
    settings::{ColorThemeChoice, ToolMode, ToolPreference},
    sublime_theme::Rgba,
    syntax_theme::{ResolvedTypstStyles, TypstStyleOverride, TypstStyleOverrides, TypstSyntaxRole},
    theme::{self, METRICS},
    toolchain::ToolResolution,
};
use eframe::egui::{self, Align, Color32, Layout, RichText, Vec2};
use std::path::Path;

/// Consistent label and value columns that become a single stack in narrow windows.
pub(super) fn settings_control_row<R>(
    ui: &mut egui::Ui,
    label: &str,
    add_controls: impl FnOnce(&mut egui::Ui) -> R,
) -> egui::InnerResponse<R> {
    let width = ui.available_width();
    let gap = ui.spacing().item_spacing.x;
    let columns = settings_row_columns(width, gap);
    if let Some((label_width, controls_width)) = columns {
        let label_text = egui::WidgetText::from(label).into_galley(
            ui,
            Some(egui::TextWrapMode::Wrap),
            label_width,
            egui::TextStyle::Body,
        );
        let row_height = label_text
            .size()
            .y
            .max(ui.spacing().interact_size.y)
            .max(
                ui.text_style_height(&egui::TextStyle::Button)
                    + ui.spacing().button_padding.y * 2.0,
            )
            .max(METRICS.icon.button_size.y);
        ui.horizontal_top(|ui| {
            ui.allocate_ui_with_layout(
                Vec2::new(label_width, row_height),
                Layout::left_to_right(Align::Center),
                |ui| {
                    // Allocation uses the child's actual minimum size, not its
                    // requested width. Reserve the column even for short labels.
                    ui.set_min_size(Vec2::new(label_width, row_height));
                    ui.add(egui::Label::new(label_text));
                },
            );
            ui.allocate_ui_with_layout(
                Vec2::new(controls_width, row_height),
                Layout::left_to_right(Align::Center).with_main_wrap(true),
                |ui| {
                    ui.set_min_width(controls_width);
                    add_controls(ui)
                },
            )
            .inner
        })
    } else {
        ui.vertical(|ui| {
            ui.label(label);
            add_controls(ui)
        })
    }
}

fn settings_row_columns(width: f32, gap: f32) -> Option<(f32, f32)> {
    (width >= 360.0).then(|| {
        let label = 126.0;
        (label, (width - label - gap).max(0.0))
    })
}

/// A small page sample uses the same colors as the real preview, without
/// starting a compiler, native child, image worker, or repaint animation.
pub(super) fn show_page_appearance_sample(
    ui: &mut egui::Ui,
    settings: &crate::settings::AppSettings,
) {
    // At large interface scales the controls take priority over an unreadably
    // small sample. Its fixed text layout needs the full thumbnail width.
    if ui.available_width() < 112.0 {
        return;
    }
    let width = 112.0;
    let size = Vec2::new(width, width * 1.28);
    let (rect, response) = ui.allocate_exact_size(size, egui::Sense::hover());
    response.widget_info(|| {
        egui::WidgetInfo::labeled(
            egui::WidgetType::Image,
            ui.is_enabled(),
            "Page appearance sample",
        )
    });
    #[cfg(all(feature = "desktop-ui-tests", target_os = "macos"))]
    crate::desktop_test::observe("settings.page_sample", &response);
    if !ui.is_rect_visible(rect) {
        return;
    }
    let interface = if ui.visuals().dark_mode {
        egui::Theme::Dark
    } else {
        egui::Theme::Light
    };
    let dark = settings.document_theme.resolve(interface) == egui::Theme::Dark;
    let (background, foreground) =
        super::preview_palette::resolve_palette(settings, ui.visuals(), dark);
    let paper = rect.shrink(4.0);
    let painter = ui.painter().with_clip_rect(rect.intersect(ui.clip_rect()));
    painter.rect_filled(rect, 3.0, ui.visuals().extreme_bg_color);
    painter.rect_filled(paper, 0.0, background);
    painter.rect_stroke(
        paper,
        0.0,
        ui.visuals().widgets.noninteractive.bg_stroke,
        egui::StrokeKind::Inside,
    );
    let text = |offset: f32, value: &str, font: f32| {
        painter.text(
            egui::pos2(paper.center().x, paper.top() + offset),
            egui::Align2::CENTER_TOP,
            value,
            egui::FontId::proportional(font),
            foreground,
        );
    };
    text(10.0, "Energy estimate", 11.0);
    text(31.0, "Let A generate a\nsemigroup on H.", 10.0);
    text(58.0, "∂ₜu + Au = f", 12.0);
    paint_page_sample_image(
        &painter,
        page_sample_image_rect(paper),
        dark && settings.dark_images,
    );
    painter.text(
        egui::pos2(paper.center().x, paper.bottom() - 6.0),
        egui::Align2::CENTER_BOTTOM,
        "1",
        egui::FontId::proportional(9.0),
        foreground,
    );
}

fn page_sample_image_rect(paper: egui::Rect) -> egui::Rect {
    egui::Rect::from_min_max(
        egui::pos2(paper.left() + 10.0, paper.top() + 82.0),
        egui::pos2(paper.right() - 10.0, paper.top() + 117.0),
    )
}

// Four swatches stand in for image pixels. Cache their exact opened-image
// transform once; the sample never decodes or uploads a texture while painting.
fn page_sample_image_colors(dark: bool) -> [Color32; 4] {
    const SOURCE: [[u8; 4]; 4] = [
        [232, 242, 248, 255],
        [239, 174, 68, 255],
        [103, 151, 168, 255],
        [45, 96, 120, 255],
    ];
    static DARK: std::sync::OnceLock<[[u8; 4]; 4]> = std::sync::OnceLock::new();
    let pixels = if dark {
        *DARK.get_or_init(|| {
            let rgba = crate::preview::dark_preview_rgba(SOURCE.as_flattened());
            std::array::from_fn(|index| rgba[index * 4..index * 4 + 4].try_into().unwrap())
        })
    } else {
        SOURCE
    };
    pixels.map(|[r, g, b, a]| Color32::from_rgba_unmultiplied(r, g, b, a))
}

fn paint_page_sample_image(painter: &egui::Painter, image: egui::Rect, dark: bool) {
    let [sky, sun, distant, nearby] = page_sample_image_colors(dark);
    let at = |x: f32, y: f32| image.min + image.size() * egui::vec2(x, y);
    painter.rect_filled(image, 0.0, sky);
    painter.circle_filled(at(0.76, 0.28), 5.0, sun);
    painter.add(egui::Shape::convex_polygon(
        vec![at(0.0, 0.85), at(0.34, 0.22), at(0.81, 1.0), at(0.0, 1.0)],
        distant,
        egui::Stroke::NONE,
    ));
    painter.add(egui::Shape::convex_polygon(
        vec![at(0.31, 1.0), at(0.73, 0.45), at(1.0, 0.8), at(1.0, 1.0)],
        nearby,
        egui::Stroke::NONE,
    ));
}

pub(super) fn color_theme_choice_label(choice: &ColorThemeChoice) -> String {
    match choice {
        ColorThemeChoice::Builtin(id) => builtin_themes::find(id)
            .map_or_else(|| format!("Unknown · {id}"), |theme| theme.name.to_owned()),
        ColorThemeChoice::Sublime(path) => Path::new(path)
            .file_stem()
            .and_then(|name| name.to_str())
            .filter(|name| !name.is_empty())
            .map_or_else(
                || "Imported Sublime theme".to_owned(),
                |name| name.to_owned(),
            ),
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum FontPickerSelection {
    Default,
    Editor,
    Family {
        name: String,
        path: String,
        face_index: u32,
    },
}

pub(super) fn show_font_family_picker(
    ui: &mut egui::Ui,
    id: &'static str,
    catalog: &FontCatalog,
    selected_path: Option<&str>,
    selected_family: Option<&str>,
    default_label: &'static str,
    offer_editor_font: bool,
) -> Option<FontPickerSelection> {
    let selected_text = selected_family
        .map(str::to_owned)
        .or_else(|| {
            selected_path.and_then(|path| {
                Path::new(path)
                    .file_name()
                    .and_then(|name| name.to_str())
                    .map(str::to_owned)
            })
        })
        .unwrap_or_else(|| default_label.to_owned());
    let mut selection = None;
    egui::ComboBox::from_id_salt(id)
        .width(ui.available_width().clamp(0.0, 190.0))
        .height(350.0)
        .selected_text(selected_text)
        .show_ui(ui, |ui| {
            let query_id = ui.id().with("font-search");
            let mut query = ui
                .ctx()
                .data(|data| data.get_temp::<String>(query_id).unwrap_or_default());
            ui.add(egui::TextEdit::singleline(&mut query).hint_text("Search fonts"));
            ui.ctx()
                .data_mut(|data| data.insert_temp(query_id, query.clone()));
            if offer_editor_font {
                if ui
                    .selectable_label(
                        selected_path.is_none() && default_label == "System UI",
                        "System UI",
                    )
                    .clicked()
                {
                    selection = Some(FontPickerSelection::Default);
                }
                if ui
                    .selectable_label(
                        selected_path.is_none() && default_label == "Editor font",
                        "Editor font",
                    )
                    .clicked()
                {
                    selection = Some(FontPickerSelection::Editor);
                }
            } else if ui
                .selectable_label(selected_path.is_none(), default_label)
                .clicked()
            {
                selection = Some(FontPickerSelection::Default);
            }
            let mut previous_origin = None;
            for family in catalog
                .families()
                .iter()
                .filter(|family| crate::completion::fuzzy_score(&family.name, &query).is_some())
            {
                if previous_origin != Some(family.origin) {
                    ui.separator();
                    ui.label(RichText::new(family.origin.label()).strong());
                    previous_origin = Some(family.origin);
                }
                let is_selected = selected_family
                    .is_some_and(|selected| selected.eq_ignore_ascii_case(&family.name))
                    && selected_path.is_some_and(|path| family.contains_path(Path::new(path)));
                let response = ui
                    .selectable_label(is_selected, &family.name)
                    .on_hover_ui(|ui| {
                        ui.label(&family.name);
                        crate::font_preview::show(ui, id, family);
                    });
                if response.clicked()
                    && let Some(face) = family.primary_face()
                {
                    selection = Some(FontPickerSelection::Family {
                        name: family.name.clone(),
                        path: face.path.display().to_string(),
                        face_index: face.index,
                    });
                }
            }
        });
    selection
}

pub(super) fn update_staged_font_weight(
    committed: &mut u16,
    staged: &mut Option<u16>,
    displayed: u16,
    changed: bool,
    pointer_down: bool,
    drag_stopped: bool,
) {
    if drag_stopped {
        *committed = if changed {
            displayed
        } else {
            staged.take().unwrap_or(displayed)
        };
        *staged = None;
    } else if changed && pointer_down {
        *staged = Some(displayed);
    } else if changed {
        *committed = displayed;
        *staged = None;
    }
}

pub(super) fn show_font_weight_control(
    ui: &mut egui::Ui,
    id_salt: impl std::hash::Hash + std::fmt::Debug,
    weight: &mut u16,
    staged_weight: &mut Option<u16>,
    support: Option<&theme::FontWeightSupport>,
) {
    let Some(support) = support else {
        ui.label(RichText::new("Static weight").weak());
        return;
    };
    ui.label(RichText::new("Weight").strong());
    match support {
        theme::FontWeightSupport::Continuous { min, max, .. } => {
            *weight = (*weight).clamp(*min, *max);
            let mut displayed = staged_weight.unwrap_or(*weight).clamp(*min, *max);
            let response = ui.add_sized(
                [
                    METRICS.settings.ui_font_weight_width,
                    ui.spacing().interact_size.y,
                ],
                egui::Slider::new(&mut displayed, *min..=*max)
                    .clamping(egui::SliderClamping::Always),
            );
            update_staged_font_weight(
                weight,
                staged_weight,
                displayed,
                response.changed(),
                response.is_pointer_button_down_on() || response.dragged(),
                response.drag_stopped(),
            );
        }
        theme::FontWeightSupport::Discrete { values, .. } => {
            *staged_weight = None;
            *weight = support.clamp(*weight);
            ui.push_id(id_salt, |ui| {
                egui::ComboBox::from_id_salt("font-weight")
                    .selected_text(weight.to_string())
                    .show_ui(ui, |ui| {
                        for value in values {
                            ui.selectable_value(weight, *value, value.to_string());
                        }
                    });
            });
        }
    }
    if crate::app::icons::action_button(ui, "Reset").clicked() {
        *weight = support.default_weight();
        *staged_weight = None;
    }
}

pub(super) fn show_typst_override_editor(
    ui: &mut egui::Ui,
    overrides: &mut TypstStyleOverrides,
    palette: theme::SyntaxPalette,
    syntect_theme: &syntect::highlighting::Theme,
    weight_support: &theme::FontWeightSupport,
) {
    egui::Grid::new("typst-override-grid")
        .num_columns(9)
        .striped(true)
        .spacing(egui::vec2(theme::SPACE.control, theme::SPACE.tight))
        .show(ui, |ui| {
            ui.add_sized(
                [
                    METRICS.settings.override_role_width,
                    METRICS.settings.override_row_height,
                ],
                egui::Label::new(RichText::new("Syntax").size(theme::TYPE.supporting).weak()),
            );
            for (label, width) in [
                ("Foreground", METRICS.settings.override_color_width),
                ("Background", METRICS.settings.override_color_width),
                ("Weight", METRICS.settings.override_weight_width),
                ("Italic", METRICS.settings.override_decoration_width),
                ("Underline", METRICS.settings.override_decoration_width),
                ("Strike", METRICS.settings.override_decoration_width),
            ] {
                ui.add_sized(
                    [width, METRICS.settings.override_row_height],
                    egui::Label::new(RichText::new(label).size(theme::TYPE.supporting).weak())
                        .truncate(),
                );
            }
            ui.add_sized(
                [
                    METRICS.settings.override_sample_width,
                    METRICS.settings.override_row_height,
                ],
                egui::Label::new(
                    RichText::new("Live sample")
                        .size(theme::TYPE.supporting)
                        .weak(),
                ),
            );
            ui.add_sized(
                [
                    METRICS.settings.override_reset_width,
                    METRICS.settings.override_row_height,
                ],
                egui::Label::new(""),
            );
            ui.end_row();

            let mut previous_group = "";
            for role in ["Markup", "Math", "Code", "Diagnostics"]
                .into_iter()
                .flat_map(|group| {
                    TypstSyntaxRole::ALL
                        .into_iter()
                        .filter(move |role| role.group() == group)
                })
            {
                if role.group() != previous_group {
                    ui.label(RichText::new(role.group()).strong());
                    ui.end_row();
                }
                previous_group = role.group();
                let inherited =
                    ResolvedTypstStyles::resolve_style(role, palette, Some(syntect_theme), None);
                let mut style_override = overrides.get(role).cloned().unwrap_or_default();
                ui.add_sized(
                    [
                        METRICS.settings.override_role_width,
                        METRICS.settings.override_row_height,
                    ],
                    egui::Label::new(role.label()).truncate(),
                );
                optional_color_override(
                    ui,
                    (role, "foreground"),
                    &mut style_override.foreground,
                    inherited.foreground,
                );
                optional_color_override(
                    ui,
                    (role, "background"),
                    &mut style_override.background,
                    inherited.background,
                );
                optional_weight_override(
                    ui,
                    (role, "weight"),
                    &mut style_override.weight,
                    inherited.weight,
                    weight_support,
                );
                optional_bool_override(ui, (role, "italic"), "I", &mut style_override.italic);
                optional_bool_override(ui, (role, "underline"), "U", &mut style_override.underline);
                optional_bool_override(
                    ui,
                    (role, "strike"),
                    "S",
                    &mut style_override.strikethrough,
                );

                let resolved = ResolvedTypstStyles::resolve_style(
                    role,
                    palette,
                    Some(syntect_theme),
                    Some(&style_override),
                );
                let mut sample = egui::text::LayoutJob::default();
                sample.append(role.sample(), 0.0, resolved.text_format());
                ui.add_sized(
                    [
                        METRICS.settings.override_sample_width,
                        METRICS.settings.override_row_height,
                    ],
                    egui::Label::new(sample).truncate(),
                );

                if crate::app::icons::action_button(ui, "Reset").clicked() {
                    style_override = TypstStyleOverride::default();
                }
                overrides.set(role, style_override);
                ui.end_row();
            }
        });
}

fn optional_color_override(
    ui: &mut egui::Ui,
    id: impl std::hash::Hash + std::fmt::Debug,
    value: &mut Option<Rgba>,
    inherited: Color32,
) {
    ui.push_id(id, |ui| {
        ui.allocate_ui_with_layout(
            egui::vec2(
                METRICS.settings.override_color_width,
                METRICS.settings.override_row_height,
            ),
            Layout::left_to_right(Align::Center),
            |ui| {
                theme::apply_compact_control_spacing(ui);
                let mut color = value.map_or(inherited, color_from_rgba);
                let response = ui.color_edit_button_srgba(&mut color);
                if response.changed() {
                    *value = Some(rgba_from_color(color));
                }
            },
        );
    });
}

fn optional_bool_override(
    ui: &mut egui::Ui,
    id: impl std::hash::Hash + std::fmt::Debug,
    label: &str,
    value: &mut Option<bool>,
) {
    let state = typst_override_state(*value);
    if ui
        .push_id(id, |ui| {
            typst_overrides_hover_text(
                ui.add_sized(
                    [
                        METRICS.settings.override_decoration_width,
                        METRICS.settings.override_row_height,
                    ],
                    egui::Button::new(format!("{label} {state}")),
                ),
                typst_override_toggle_tooltip(label, *value),
            )
        })
        .inner
        .clicked()
    {
        *value = next_typst_override_state(*value);
    }
}

fn optional_weight_override(
    ui: &mut egui::Ui,
    id: impl std::hash::Hash + std::fmt::Debug,
    value: &mut Option<u16>,
    inherited: u16,
    support: &theme::FontWeightSupport,
) {
    ui.push_id(id, |ui| {
        ui.allocate_ui_with_layout(
            egui::vec2(
                METRICS.settings.override_weight_width,
                METRICS.settings.override_row_height,
            ),
            Layout::left_to_right(Align::Center),
            |ui| {
                egui::ComboBox::from_id_salt("value")
                    .width(METRICS.settings.override_weight_width)
                    .selected_text(
                        value.map_or_else(|| format!("inherit · {inherited}"), |it| it.to_string()),
                    )
                    .show_ui(ui, |ui| {
                        ui.selectable_value(value, None, format!("Inherit · {inherited}"));
                        match support {
                            theme::FontWeightSupport::Continuous { min, max, .. } => {
                                for weight in theme::EDITOR_FONT_WEIGHTS
                                    .into_iter()
                                    .filter(|weight| weight >= min && weight <= max)
                                {
                                    ui.selectable_value(value, Some(weight), weight.to_string());
                                }
                            }
                            theme::FontWeightSupport::Discrete { values, .. } => {
                                for weight in values {
                                    ui.selectable_value(value, Some(*weight), weight.to_string());
                                }
                            }
                        }
                    });
            },
        );
    });
}

pub(super) fn typst_override_state(value: Option<bool>) -> &'static str {
    match value {
        None => "inherit",
        Some(true) => "on",
        Some(false) => "off",
    }
}

pub(super) fn next_typst_override_state(value: Option<bool>) -> Option<bool> {
    match value {
        None => Some(true),
        Some(true) => Some(false),
        Some(false) => None,
    }
}

pub(super) fn typst_override_toggle_tooltip(label: &str, value: Option<bool>) -> String {
    format!("{label}: {}; click to cycle", typst_override_state(value))
}

pub(super) fn rgba_from_color(color: Color32) -> Rgba {
    let [red, green, blue, alpha] = color.to_srgba_unmultiplied();
    Rgba::from_rgba(red, green, blue, alpha)
}

pub(super) fn color_from_rgba(color: Rgba) -> Color32 {
    Color32::from_rgba_unmultiplied(color.r, color.g, color.b, color.a)
}

pub(super) fn settings_value_row(ui: &mut egui::Ui, name: &str, value: &str) {
    ui.horizontal_wrapped(|ui| {
        settings_inline_value(ui, name, value);
    });
}

pub(super) fn settings_inline_value(ui: &mut egui::Ui, name: &str, value: &str) {
    ui.label(RichText::new(format!("{name}:")).weak());
    ui.label(value);
}

pub(super) fn tool_preference_editor(
    ui: &mut egui::Ui,
    label: &str,
    preference: &mut ToolPreference,
    resolution: &ToolResolution,
    deterministic_snapshot: bool,
    settings: &crate::settings::AppSettings,
) -> bool {
    let mut browse = false;
    ui.push_id(label, |ui| {
        ui.horizontal_wrapped(|ui| {
            ui.label(RichText::new(label).strong());
            for mode in ToolMode::ALL {
                let response = ui.selectable_value(&mut preference.mode, mode, mode.label());
                let hint = if deterministic_snapshot {
                    format!(
                        "{} {}\n<packaged>/{}",
                        resolution.kind.label(),
                        resolution.kind.bundled_version(),
                        resolution.kind.binary_name()
                    )
                } else if mode == ToolMode::Bundled {
                    resolution.bundled_program.as_ref().map_or_else(
                        || format!("Bundled {} is unavailable\n{}", resolution.kind.label(), resolution.detail()),
                        |path| format!("Bundled {} {}\n{}", resolution.kind.label(), resolution.kind.bundled_version(), path.display()),
                    )
                } else { resolution.detail() };
                settings_hover_text(response, hint);
            }
            super::tool_reference::controls(ui, crate::tool_reference::ReferenceId::from_kind(resolution.kind),
                preference.mode == ToolMode::Bundled && resolution.origin == crate::toolchain::ToolOrigin::Bundled, settings);
        });
        if preference.mode == ToolMode::Custom {
            ui.horizontal_wrapped(|ui| {
                let path_width =
                    (ui.available_width() - METRICS.settings.tool_custom_label_reserve).clamp(
                        METRICS.settings.tool_custom_min_width,
                        METRICS.settings.tool_custom_max_width,
                    );
                ui.add(
                    egui::TextEdit::singleline(&mut preference.custom_path)
                        .hint_text("/absolute/path/to/executable")
                        .desired_width(path_width),
                );
                browse |= crate::app::icons::action_button(ui, "Browse…").clicked();
            });
        }
        egui::CollapsingHeader::new("Command customization").show(ui, |ui| {
            documentation_link(ui, "Command-line options", command_docs(resolution.kind));
            let id = ui.id().with("command-draft");
            let mut draft = ui.ctx().data_mut(|data| {
                let cached = data.get_temp::<(crate::tool_command::CommandCustomization, crate::tool_command::CommandCustomization)>(id);
                cached.map(|(base, draft)| super::tool_drafts::rebase_command(&base, &preference.command, draft)).unwrap_or_else(|| preference.command.clone())
            });
            command_help(ui.label("Arguments (quoted values supported; no shell expansion)"), resolution.kind, "Arguments");
            ui.add(egui::TextEdit::multiline(&mut draft.arguments).desired_rows(2).desired_width(f32::INFINITY));
            ui.label(egui::RichText::new("These are the actual default options. The app adds input/output paths, workspace fonts and the flags controlled by SyncTeX/offline settings.").small());
            command_help(ui.label("Environment (JSON object of strings)"), resolution.kind, "Environment");
            super::code_editor::editor(ui, "command-environment", &mut draft.environment, "json", settings, 2);
            command_help(ui.label("Working directory (blank uses the document/project directory)"), resolution.kind, "Working directory");
            ui.add(egui::TextEdit::singleline(&mut draft.directory).desired_width(f32::INFINITY));
            let validation_id = id.with("validation");
            let validation = ui.ctx().data_mut(|data| {
                let cached = data.get_temp::<(crate::tool_command::CommandCustomization, Result<(), String>)>(validation_id);
                let result = cached.filter(|(previous, _)| previous == &draft).map(|(_, result)| result).unwrap_or_else(|| draft.validate_for_tool(resolution.kind));
                data.insert_temp(validation_id, (draft.clone(), result.clone()));
                result
            });
            if let Err(error) = &validation { ui.colored_label(ui.visuals().error_fg_color, error); }
            ui.horizontal(|ui| {
                if crate::app::icons::action_button_enabled(ui, validation.is_ok() && draft != preference.command, "Apply command").clicked() { preference.command = draft.clone(); }
                if crate::app::icons::action_button(ui, "Reset command").clicked() { draft = crate::tool_command::CommandCustomization::for_tool(resolution.kind); preference.command = draft.clone(); }
            });
            ui.ctx().data_mut(|data| data.insert_temp(id,(preference.command.clone(),draft)));
        });
    });
    browse
}

fn command_docs(kind: crate::toolchain::ToolKind) -> &'static str {
    use crate::toolchain::ToolKind::*;
    match kind {
        Typst => "https://github.com/typst/typst/blob/v0.15.1/crates/typst-cli/src/args.rs",
        Tinymist => "https://myriad-dreamin.github.io/tinymist/feature/cli.html#servers",
        Tectonic => "https://tectonic-typesetting.github.io/book/latest/v2cli/compile.html",
        Texlab => "https://github.com/latex-lsp/texlab/blob/v5.26.0/crates/texlab/src/main.rs",
        Badness => "https://github.com/jolars/badness/blob/v0.24.0/docs/src/reference/cli.md",
        TexFmt => "https://github.com/WGUNDERWOOD/tex-fmt#command-line-options",
    }
}
// Native eframe links are disabled. Queue the owner action so the existing
// browser worker opens these links without blocking this Settings viewport.
pub(super) fn documentation_link(ui: &mut egui::Ui, title: &str, url: &str) {
    let link = ui.link(title);
    #[cfg(all(feature = "desktop-ui-tests", target_os = "macos"))]
    if ui.clip_rect().contains_rect(link.rect) {
        crate::desktop_test::observe(&format!("settings.documentation.{title}"), &link);
    }
    if link.clicked() {
        let id = egui::Id::new(("settings-documentation", ui.ctx().viewport_id()));
        ui.ctx()
            .data_mut(|data| data.insert_temp(id, url.to_owned()));
        ui.ctx().request_repaint();
    }
}
fn command_help(response: egui::Response, kind: crate::toolchain::ToolKind, field: &str) {
    use crate::toolchain::ToolKind::*;
    response.on_hover_ui(|ui| {
        let docs = match field {
            "Arguments" => {
                let (example, reason) = match kind {
                    Typst => ("watch --diagnostic-format short --font-path /Users/me/fonts", "Search an additional font directory."),
                    Tinymist => ("lsp --font-path /Users/me/fonts", "Give the language server extra fonts."),
                    Tectonic => ("-X compile --untrusted --keep-logs --keep-intermediates --print", "Include detailed TeX engine output for troubleshooting. Logs are already retained."),
                    Texlab => ("-vv --log-file /tmp/texlab.log", "Write verbose server diagnostics to a log file."),
                    Badness => ("lsp", "Keep the required lsp subcommand. Configure formatting/linting in JSON or badness.toml; optional CLI flags do not affect this version’s lsp mode."),
                    TexFmt => ("--stdin --quiet --wraplen 100", "Wrap formatted lines at 100 columns."),
                };
                ui.label(reason); ui.monospace(example);
                ui.label("Edit these literal options directly. Input/output paths are added by the app. Quoting groups values; no shell expansion. Flags must suit the subcommand.");
                command_docs(kind)
            }
            "Environment" => {
                let (example, reason, docs) = match kind {
                    Typst | Tinymist => (r#"{"TYPST_FONT_PATHS":"/Users/me/fonts"}"#, "Search an extra font directory.", "https://github.com/typst/typst/blob/v0.15.1/crates/typst-cli/src/args.rs"),
                    Tectonic => (r#"{"TECTONIC_CACHE_DIR":"/Users/me/tectonic-cache"}"#, "Store downloaded TeX resources in this directory.", "https://tectonic-typesetting.github.io/book/latest/getting-started/first-document.html#cache"),
                    Badness => (r#"{"BADNESS_CONFIG":"/Users/me/paper/badness.toml"}"#, "Use this configuration if no project badness.toml is found. Project configuration wins.", "https://github.com/jolars/badness/blob/v0.24.0/docs/src/reference/configuration.md"),
                    Texlab | TexFmt => (r#"{"RUST_BACKTRACE":"1"}"#, "Include a stack trace if the Rust tool crashes. Normal operation is unchanged.", "https://doc.rust-lang.org/std/backtrace/index.html#environment-variables"),
                };
                ui.label(reason); ui.monospace(example);
                ui.label("JSON object of strings; overrides inherited values. Use absolute paths, not $HOME or ~."); docs
            }
            _ => {
                ui.monospace("/Users/me/paper");
                ui.label("An existing working directory, without quotes. Blank keeps the app’s document/project default; it is usually best left blank. The app still supplies its workspace root and private output paths.");
                match kind {
                    Badness => "https://badness.dev/reference/configuration.html",
                    TexFmt => "https://github.com/WGUNDERWOOD/tex-fmt#configuration",
                    Tinymist | Typst => "https://myriad-dreamin.github.io/tinymist/feature/compiler-settings.html#packages-roots-and-certificates",
                    Texlab => "https://github.com/latex-lsp/texlab/blob/v5.26.0/crates/texlab/src/server/options.rs",
                    Tectonic => "https://tectonic-typesetting.github.io/book/latest/v2cli/compile.html",
                }
            }
        };
        documentation_link(ui, &format!("{} — {} documentation", kind.label(), field), docs);
    });
}

pub(super) fn fallback_notice(ui: &mut egui::Ui, label: &str, reason: &str) {
    let color = warning_color(ui.ctx());
    ui.group(|ui| {
        ui.label(
            RichText::new(label)
                .size(theme::TYPE.supporting)
                .strong()
                .color(color),
        );
        ui.label(reason);
    });
}

pub(super) fn show_service_status_chip(ui: &mut egui::Ui, name: &str, state: &ServiceState) {
    let color = match state {
        ServiceState::Ready(_) => success_color(ui.ctx()),
        ServiceState::Starting(_) => info_color(ui.ctx()),
        ServiceState::Degraded(_) => warning_color(ui.ctx()),
        ServiceState::Failed(_) => error_color(ui.ctx()),
        ServiceState::Disabled(_) | ServiceState::Unsupported(_) => neutral_color(ui.ctx()),
    };
    show_status_chip(ui, name, state.label(), state.detail(), color);
}

pub(super) fn show_status_chip(
    ui: &mut egui::Ui,
    name: &str,
    status: &str,
    detail: &str,
    color: Color32,
) {
    let show_detail = settings_status_has_detail(name, status, detail);
    let name = RichText::new(name).strong();
    let status = RichText::new(status)
        .size(theme::TYPE.supporting)
        .strong()
        .color(color);
    let text_size = |text: RichText| {
        egui::WidgetText::from(text)
            .into_galley(
                ui,
                Some(egui::TextWrapMode::Extend),
                f32::INFINITY,
                egui::TextStyle::Body,
            )
            .size()
    };
    let name_size = text_size(name.clone());
    let status_size = text_size(status.clone());
    let frame = theme::status_chip_frame(ui.style());
    let size = Vec2::new(
        name_size.x + ui.spacing().item_spacing.x + status_size.x,
        name_size.y.max(status_size.y),
    ) + frame.total_margin().sum();
    let response = ui
        .allocate_ui_with_layout(size, Layout::left_to_right(Align::Center), |ui| {
            frame
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.add(egui::Label::new(name).extend());
                        ui.add(egui::Label::new(status).extend());
                    });
                })
                .response
        })
        .inner;
    if show_detail {
        settings_hover_text(response, detail);
    }
}

pub(super) fn settings_status_has_detail(name: &str, status: &str, detail: &str) -> bool {
    let detail = detail.trim().trim_end_matches('.');
    !detail.is_empty()
        && !detail.eq_ignore_ascii_case(status)
        && !detail.eq_ignore_ascii_case(name)
        && !detail.eq_ignore_ascii_case(&format!("{name} {status}"))
        && !detail.eq_ignore_ascii_case(&format!("{name}: {status}"))
}

pub(super) fn show_text_highlight_editor(
    ui: &mut egui::Ui,
    rules: &mut Vec<crate::editor_decoration::HighlightRule>,
    support: &theme::FontWeightSupport,
) {
    let accent = theme::palette(ui.ctx()).accent;
    let mut remove = None;
    egui::ScrollArea::horizontal()
        .id_salt("text-highlights-scroll")
        .show(ui, |ui| {
            egui::Grid::new("text-highlights-grid")
                .num_columns(10)
                .striped(true)
                .show(ui, |ui| {
                    for label in [
                        "On",
                        "Text to match",
                        "Foreground",
                        "Background",
                        "Weight",
                        "Italic",
                        "Underline",
                        "Strike",
                        "Sample",
                        "",
                    ] {
                        ui.label(RichText::new(label).weak());
                    }
                    ui.end_row();
                    for (index, rule) in rules.iter_mut().enumerate() {
                        ui.checkbox(&mut rule.enabled, "");
                        let _match_field = ui.add(
                            egui::TextEdit::singleline(&mut rule.text)
                                .id_salt(("highlight-text", index))
                                .desired_width(140.0)
                                .char_limit(256),
                        );
                        #[cfg(all(feature = "desktop-ui-tests", target_os = "macos"))]
                        if ui.clip_rect().contains_rect(_match_field.rect) {
                            crate::desktop_test::observe(
                                &format!("settings.highlight.{index}"),
                                &_match_field,
                            );
                        }
                        optional_color_override(
                            ui,
                            (index, "foreground"),
                            &mut rule.style.foreground,
                            accent,
                        );
                        optional_color_override(
                            ui,
                            (index, "background"),
                            &mut rule.style.background,
                            Color32::TRANSPARENT,
                        );
                        optional_weight_override(
                            ui,
                            (index, "weight"),
                            &mut rule.style.weight,
                            theme::FONT_WEIGHT_NORMAL,
                            support,
                        );
                        optional_bool_override(ui, (index, "italic"), "I", &mut rule.style.italic);
                        optional_bool_override(
                            ui,
                            (index, "underline"),
                            "U",
                            &mut rule.style.underline,
                        );
                        optional_bool_override(
                            ui,
                            (index, "strike"),
                            "S",
                            &mut rule.style.strikethrough,
                        );
                        let mut job = egui::text::LayoutJob::simple(
                            rule.text.clone(),
                            theme::editor_font(),
                            accent,
                            f32::INFINITY,
                        );
                        let mut decorations = crate::editor_decoration::Decorations::default();
                        decorations.prepare(&rule.text, false, false, std::slice::from_ref(rule));
                        decorations.apply(&mut job, accent);
                        ui.add_sized(
                            [140.0, METRICS.settings.override_row_height],
                            egui::Label::new(job).truncate(),
                        );
                        if ui.small_button("Remove").clicked() {
                            remove = Some(index);
                        }
                        ui.end_row();
                    }
                });
        });
    if let Some(index) = remove {
        rules.remove(index);
    }
    ui.horizontal_wrapped(|ui| {
        if ui
            .add_enabled(rules.len() < 100, egui::Button::new("Add highlight"))
            .clicked()
        {
            rules.push(crate::editor_decoration::HighlightRule {
                text: "#mycomment".into(),
                enabled: true,
                style: TypstStyleOverride {
                    weight: Some(700),
                    ..Default::default()
                },
            });
        }
        if ui.button("Restore default highlights").clicked() {
            *rules = crate::editor_decoration::default_rules();
        }
    });
}

#[cfg(test)]
mod layout_tests {
    use super::*;

    #[test]
    fn compact_typst_override_grid_uses_one_header_and_one_heading_per_group() {
        use egui_kittest::{
            Harness,
            kittest::{Queryable as _, by},
        };
        let mut fonts_ready = false;
        let mut harness = Harness::builder()
            .with_size(Vec2::new(1100.0, 1400.0))
            .build_ui_state(
                move |ui, overrides: &mut TypstStyleOverrides| {
                    if !fonts_ready {
                        theme::configure_test_fonts(ui.ctx());
                        fonts_ready = true;
                        return;
                    }
                    theme::apply_compact_tool_spacing(ui);
                    show_typst_override_editor(
                        ui,
                        overrides,
                        theme::default_syntax_palette(true),
                        &syntect::highlighting::Theme::default(),
                        &theme::FontWeightSupport::Discrete {
                            values: vec![400, 700],
                            default: 400,
                        },
                    );
                },
                TypstStyleOverrides::default(),
            );
        theme::configure_test_fonts(&harness.ctx);
        harness.run();
        for label in [
            "Syntax",
            "Foreground",
            "Background",
            "Weight",
            "Live sample",
            "Markup",
            "Math",
            "Code",
            "Diagnostics",
        ] {
            assert_eq!(harness.query_all(by().label(label)).count(), 1, "{label}");
        }
        let first = harness.get_by_label(TypstSyntaxRole::Plain.label()).rect();
        for role in TypstSyntaxRole::ALL {
            let bounds = harness.get_by_label(role.label()).rect();
            assert!(
                (bounds.left() - first.left()).abs() < 0.5,
                "{}: {bounds:?}",
                role.label()
            );
        }
    }

    #[test]
    fn settings_row_labels_and_controls_have_stable_columns_and_vertical_centers() {
        use egui_kittest::{Harness, kittest::Queryable as _};
        for width in [400.0, 500.0, 730.0] {
            let mut harness = Harness::builder()
                .with_size(Vec2::new(width, 300.0))
                .build_ui(|ui| {
                    for (index, label) in ["UI", "Light theme", "A long setting label that wraps"]
                        .into_iter()
                        .enumerate()
                    {
                        settings_control_row(ui, label, |ui| {
                            let _ = ui.button(format!("Value {index}"));
                        });
                    }
                });
            harness.run();
            let controls = (0..3)
                .map(|index| harness.get_by_label(&format!("Value {index}")).rect())
                .collect::<Vec<_>>();
            for (label, control) in ["UI", "Light theme", "A long setting label that wraps"]
                .into_iter()
                .zip(&controls)
            {
                let label_rect = harness.get_by_label(label).rect();
                assert!(
                    (control.left() - controls[0].left()).abs() < 0.5,
                    "{width}px row {label} moved the controls column: {controls:?}"
                );
                assert!(
                    (label_rect.center().y - control.center().y).abs() < 0.5,
                    "{width}px row {label} has misaligned centers: {label_rect:?} / {control:?}"
                );
            }
        }
    }

    #[test]
    fn aligned_settings_rows_keep_controls_inside_narrow_and_wide_windows() {
        for width in [250.0, 360.0, 500.0, 730.0] {
            let context = egui::Context::default();
            let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, Vec2::new(width, 300.0));
            let mut controls = None;
            let output = context.run_ui(
                egui::RawInput {
                    screen_rect: Some(screen),
                    ..Default::default()
                },
                |ui| {
                    settings_control_row(ui, "A longer setting label", |ui| {
                        controls = Some(
                            ui.add_sized(
                                [
                                    ui.available_width().min(190.0),
                                    ui.spacing().interact_size.y,
                                ],
                                egui::Button::new("Choose value"),
                            )
                            .rect,
                        );
                    });
                },
            );
            output.drop_without_applying_deltas();
            assert!(screen.contains_rect(controls.unwrap()), "width {width}");
        }
        assert!(settings_row_columns(359.0, 8.0).is_none());
        let (label, value) = settings_row_columns(500.0, 8.0).unwrap();
        assert_eq!(label + value + 8.0, 500.0);
    }

    #[test]
    fn sample_image_uses_the_real_image_transform_without_changing_page_or_text() {
        let plain = page_sample_image_colors(false);
        let expected =
            crate::preview::dark_preview_rgba(plain.map(|color| color.to_array()).as_flattened());
        assert_eq!(
            page_sample_image_colors(true)
                .map(|color| color.to_array())
                .as_flattened(),
            expected.as_slice(),
        );
        assert_ne!(page_sample_image_colors(true), plain);

        use crate::settings::{AppSettings, DocumentTheme};
        use egui_kittest::{Harness, kittest::Queryable as _};
        for page in [DocumentTheme::Light, DocumentTheme::Dark] {
            let mut harness = Harness::builder()
                .with_size(Vec2::new(180.0, 200.0))
                .build_ui_state(
                    |ui, settings: &mut AppSettings| show_page_appearance_sample(ui, settings),
                    AppSettings {
                        document_theme: page,
                        dark_images: false,
                        ..AppSettings::default()
                    },
                );
            harness.run();
            let sample = harness.get_by_label("Page appearance sample").rect();
            let before = harness.output().shapes.clone();
            harness.state_mut().dark_images = true;
            harness.run();
            let after = &harness.output().shapes;
            assert_eq!(before.len(), after.len());
            let changed = before
                .iter()
                .zip(after)
                .filter(|(a, b)| a != b)
                .collect::<Vec<_>>();
            if page == DocumentTheme::Light {
                assert!(changed.is_empty(), "images are unchanged on light pages");
            } else {
                assert_eq!(
                    changed.len(),
                    4,
                    "only the four figure shapes should change"
                );
                let image = page_sample_image_rect(sample.shrink(4.0));
                for (before, after) in changed {
                    assert!(image.contains_rect(before.shape.visual_bounding_rect()));
                    assert!(image.contains_rect(after.shape.visual_bounding_rect()));
                }
            }
        }
    }

    #[test]
    fn page_sample_is_painted_without_overflow_or_a_native_child() {
        for width in [80.0, 112.0, 180.0] {
            let context = egui::Context::default();
            let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, Vec2::new(width, 200.0));
            let output = context.run_ui(
                egui::RawInput {
                    screen_rect: Some(screen),
                    ..Default::default()
                },
                |ui| {
                    show_page_appearance_sample(ui, &crate::settings::AppSettings::default());
                    assert!(screen.contains_rect(ui.min_rect()));
                },
            );
            // The sample is normal egui paint: no native child or image upload.
            assert_eq!(output.viewport_output.len(), 1);
            output.drop_without_applying_deltas();
        }
    }
}

#[cfg(test)]
mod command_editor_tests {
    use super::*;
    use crate::{settings::AppSettings, tool_command::CommandCustomization, toolchain::ToolKind};
    use egui_kittest::{
        Harness,
        kittest::{NodeT as _, Queryable as _},
    };

    #[test]
    fn command_preset_preserves_unfinished_environment_in_the_real_editor() {
        use egui_kittest::kittest::by;
        let settings = AppSettings::default();
        let resolution = crate::toolchain::resolve_tool(ToolKind::Tinymist, &settings.tinymist);
        let mut harness = Harness::builder()
            .with_size(egui::vec2(850.0, 900.0))
            .build_ui_state(
                move |ui, settings: &mut AppSettings| {
                    let mut edited = settings.clone();
                    tool_preference_editor(
                        ui,
                        "Tinymist",
                        &mut edited.tinymist,
                        &resolution,
                        true,
                        settings,
                    );
                    super::super::tool_reference::apply_requested(ui.ctx(), &mut edited);
                    *settings = edited;
                },
                settings,
            );
        theme::configure_test_fonts(&harness.ctx);
        harness.run();
        harness.get_by_label("Command customization").click();
        harness.run();
        harness
            .get(
                by().role(egui::accesskit::Role::MultilineTextInput)
                    .value("{}"),
            )
            .focus();
        harness.run();
        harness.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::A);
        harness.run();
        let unfinished = "{\"RUST_LOG\":\"debug";
        harness
            .get(
                by().role(egui::accesskit::Role::MultilineTextInput)
                    .value("{}"),
            )
            .type_text(unfinished);
        harness.run();
        assert_eq!(harness.state().tinymist.command.environment, "{}");
        assert!(
            harness
                .get_by_label("Apply command")
                .accesskit_node()
                .is_disabled()
        );
        harness.get_by_label("Tinymist Verbose preset").click();
        harness.run();
        assert_eq!(
            harness.state().tinymist.command.arguments,
            ToolKind::Tinymist.default_arguments()
        );
        assert_eq!(
            harness.state().tinymist_configuration["lint"]["enabled"],
            true
        );
        assert_eq!(
            harness.state().tinymist_configuration["lint"]["when"],
            "onType"
        );
        assert_eq!(harness.state().tinymist.command.environment, "{}");
        harness.get(
            by().role(egui::accesskit::Role::MultilineTextInput)
                .value(unfinished),
        );
        harness.get(
            by().role(egui::accesskit::Role::MultilineTextInput)
                .value(ToolKind::Tinymist.default_arguments()),
        );
        assert!(
            harness
                .get_by_label("Apply command")
                .accesskit_node()
                .is_disabled()
        );
    }

    #[test]
    fn command_editor_shows_literal_defaults_and_reset_is_specific_to_each_tool() {
        use egui_kittest::kittest::by;
        for kind in [
            ToolKind::Typst,
            ToolKind::Tinymist,
            ToolKind::Tectonic,
            ToolKind::Texlab,
            ToolKind::Badness,
            ToolKind::TexFmt,
        ] {
            let preference = ToolPreference::for_tool(kind);
            let resolution = crate::toolchain::resolve_tool(kind, &preference);
            let mut harness = Harness::builder()
                .with_size(egui::vec2(850.0, 650.0))
                .build_ui_state(
                    move |ui, preference: &mut ToolPreference| {
                        tool_preference_editor(
                            ui,
                            kind.label(),
                            preference,
                            &resolution,
                            true,
                            &AppSettings::default(),
                        );
                    },
                    preference,
                );
            theme::configure_test_fonts(&harness.ctx);
            harness.run();
            harness.get_by_label("Command customization").click();
            harness.run();
            assert!(
                harness
                    .query(
                        by().role(egui::accesskit::Role::MultilineTextInput)
                            .value(kind.default_arguments())
                    )
                    .is_some()
            );
            let draft = CommandCustomization {
                arguments: "'broken".into(),
                ..Default::default()
            };
            // Edit through the real input: an invalid draft cannot affect the live command.
            let field = harness.get(
                by().role(egui::accesskit::Role::MultilineTextInput)
                    .value(kind.default_arguments()),
            );
            field.focus();
            harness.run();
            harness.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::A);
            harness
                .get(
                    by().role(egui::accesskit::Role::MultilineTextInput)
                        .value(kind.default_arguments()),
                )
                .type_text(&draft.arguments);
            harness.run();
            assert_eq!(harness.state().command.arguments, kind.default_arguments());
            assert!(
                harness
                    .query_by_label("Arguments contain an unmatched quote")
                    .is_some()
            );
            let optional = if kind == crate::toolchain::ToolKind::Tectonic {
                "--hide 'two words'"
            } else {
                "--optional 'two words'"
            };
            let edited = format!("{} {optional}", kind.default_arguments());
            harness
                .get(
                    by().role(egui::accesskit::Role::MultilineTextInput)
                        .value("'broken"),
                )
                .focus();
            harness.run();
            harness.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::A);
            harness
                .get(
                    by().role(egui::accesskit::Role::MultilineTextInput)
                        .value("'broken"),
                )
                .type_text(&edited);
            harness.run();
            assert_eq!(harness.state().command.arguments, kind.default_arguments());
            harness.get_by_label("Apply command").click();
            harness.run();
            assert_eq!(harness.state().command.arguments, edited);
            harness.get_by_label("Reset command").click();
            harness.run();
            assert_eq!(
                harness.state().command,
                CommandCustomization::for_tool(kind)
            );
            assert!(
                harness
                    .query(
                        by().role(egui::accesskit::Role::MultilineTextInput)
                            .value(kind.default_arguments())
                    )
                    .is_some()
            );
        }
    }
}
