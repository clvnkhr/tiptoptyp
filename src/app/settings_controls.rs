//! Reusable Settings controls operating on borrowed values, not the application.
use super::{
    approximate_char_capacity, error_color, info_color, neutral_color, success_color, tail_elide,
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
    toolchain::{ToolOrigin, ToolResolution},
};
use eframe::egui::{self, Align, Color32, Layout, RichText, Vec2};
use std::path::Path;

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
        .width(190.0)
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
    if ui.small_button("Reset").clicked() {
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
    for group in ["Markup", "Math", "Code", "Diagnostics"] {
        ui.label(RichText::new(group).strong());
        egui::Grid::new(("typst-override-grid", group))
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

                for role in TypstSyntaxRole::ALL
                    .into_iter()
                    .filter(|role| role.group() == group)
                {
                    let inherited = ResolvedTypstStyles::resolve_style(
                        role,
                        palette,
                        Some(syntect_theme),
                        None,
                    );
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
                    optional_bool_override(
                        ui,
                        (role, "underline"),
                        "U",
                        &mut style_override.underline,
                    );
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

                    if ui
                        .add_sized(
                            [
                                METRICS.settings.override_reset_width,
                                METRICS.settings.override_row_height,
                            ],
                            egui::Button::new("Reset"),
                        )
                        .clicked()
                    {
                        style_override = TypstStyleOverride::default();
                    }
                    overrides.set(role, style_override);
                    ui.end_row();
                }
            });
        ui.add_space(theme::SPACE.small);
    }
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
) -> bool {
    let mut browse = false;
    ui.push_id(label, |ui| {
        ui.horizontal_wrapped(|ui| {
            ui.label(RichText::new(label).strong());
            for mode in ToolMode::ALL {
                ui.selectable_value(&mut preference.mode, mode, mode.label());
            }
            ui.separator();
            let (origin_label, color, full_path) = if deterministic_snapshot {
                (
                    "Bundled",
                    success_color(ui.ctx()),
                    format!(
                        "<packaged>/{} {}",
                        resolution.kind.binary_name(),
                        resolution.kind.bundled_version()
                    ),
                )
            } else {
                let color = match resolution.origin {
                    ToolOrigin::Bundled | ToolOrigin::Custom => success_color(ui.ctx()),
                    ToolOrigin::Environment | ToolOrigin::Path => warning_color(ui.ctx()),
                    ToolOrigin::Missing => error_color(ui.ctx()),
                };
                (
                    resolution.origin.label(),
                    color,
                    resolution.program.display().to_string(),
                )
            };
            ui.label(
                RichText::new(origin_label)
                    .size(theme::TYPE.supporting)
                    .strong()
                    .color(color),
            );
            let path_width = ui
                .available_width()
                .max(METRICS.settings.tool_path_min_width);
            let path_chars = approximate_char_capacity(
                path_width,
                METRICS.settings.tool_path_estimated_font_size,
            );
            let visible_path = tail_elide(&full_path, path_chars);
            let elided = visible_path != full_path;
            let path = ui.add_sized(
                [path_width, METRICS.settings.tool_path_row_height],
                egui::Label::new(
                    RichText::new(visible_path)
                        .size(theme::TYPE.supporting)
                        .monospace(),
                )
                .truncate(),
            );
            if elided
                || path
                    .intrinsic_size()
                    .is_some_and(|size| size.x > path.rect.width())
            {
                settings_hover_text(path, full_path);
            }
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
                browse |= ui.button("Browse…").clicked();
            });
        }
    });
    browse
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

pub(super) fn show_tool_status_chip(ui: &mut egui::Ui, name: &str, resolution: &ToolResolution) {
    let color = match resolution.origin {
        ToolOrigin::Bundled | ToolOrigin::Custom => success_color(ui.ctx()),
        ToolOrigin::Environment | ToolOrigin::Path => warning_color(ui.ctx()),
        ToolOrigin::Missing => error_color(ui.ctx()),
    };
    show_status_chip(
        ui,
        name,
        resolution.origin.label(),
        &resolution.detail(),
        color,
    );
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
