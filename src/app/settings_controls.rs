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

                    if crate::app::icons::action_button(ui, "Reset").clicked() {
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
                cached.filter(|(original,_)| original == &preference.command).map(|(_,draft)| draft).unwrap_or_else(|| preference.command.clone())
            });
            command_help(ui.label("Arguments (quoted values supported; no shell expansion)"), resolution.kind, "Arguments");
            ui.add(egui::TextEdit::multiline(&mut draft.arguments).desired_rows(2).desired_width(f32::INFINITY));
            ui.label(egui::RichText::new("{args} keeps generated arguments. {arg:N} inserts one generated argument (zero-based). Remove {args} to replace the command arguments entirely.").small());
            command_help(ui.label("Environment (JSON object of strings)"), resolution.kind, "Environment");
            super::settings_code::editor(ui, "command-environment", &mut draft.environment, "json", settings, 2);
            command_help(ui.label("Working directory (blank uses the document/project directory)"), resolution.kind, "Working directory");
            ui.add(egui::TextEdit::singleline(&mut draft.directory).desired_width(f32::INFINITY));
            let validation_id = id.with("validation");
            let validation = ui.ctx().data_mut(|data| {
                let cached = data.get_temp::<(crate::tool_command::CommandCustomization, Result<(), String>)>(validation_id);
                let result = cached.filter(|(previous, _)| previous == &draft).map(|(_, result)| result).unwrap_or_else(|| draft.validate());
                data.insert_temp(validation_id, (draft.clone(), result.clone()));
                result
            });
            if let Err(error) = &validation { ui.colored_label(ui.visuals().error_fg_color, error); }
            ui.horizontal(|ui| {
                if crate::app::icons::action_button_enabled(ui, validation.is_ok() && draft != preference.command, "Apply command").clicked() { preference.command = draft.clone(); }
                if crate::app::icons::action_button(ui, "Reset command").clicked() { draft = Default::default(); preference.command = draft.clone(); }
            });
            ui.ctx().data_mut(|data| data.insert_temp(id,(preference.command.clone(),draft)));
        });
    });
    browse
}

fn command_docs(kind: crate::toolchain::ToolKind) -> &'static str {
    use crate::toolchain::ToolKind::*;
    match kind {
        Typst => "https://github.com/typst/typst/blob/main/crates/typst-cli/src/args.rs",
        Tinymist => "https://myriad-dreamin.github.io/tinymist/feature/cli.html#servers",
        Tectonic => "https://tectonic-typesetting.github.io/book/latest/ref/v1cli.html",
        Texlab => "https://github.com/latex-lsp/texlab/blob/master/crates/texlab/src/main.rs",
        Badness => "https://badness.dev/reference/cli.html#options",
        TexFmt => "https://github.com/WGUNDERWOOD/tex-fmt#command-line-options",
    }
}
pub(super) fn documentation_link(ui: &mut egui::Ui, title: &str, url: &str) {
    if ui.link(title).on_hover_text(url).clicked() {
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
                    Typst => ("{args} --font-path /Users/me/fonts", "Search an additional font directory."),
                    Tinymist => ("{args} --font-path /Users/me/fonts", "Give the language server extra fonts."),
                    Tectonic => ("{args} --keep-logs", "Keep the TeX compilation log for troubleshooting."),
                    Texlab => ("{args} -vv --log-file /tmp/texlab.log", "Write verbose server diagnostics to a log file."),
                    Badness => ("{args} --no-config", "Use built-in defaults instead of a discovered badness.toml."),
                    TexFmt => ("{args} --wraplen 100", "Wrap formatted lines at 100 columns."),
                };
                ui.label(reason); ui.monospace(example);
                ui.label("{args} preserves generated arguments. Quoting groups values; no shell expansion. Flags must suit the subcommand.");
                command_docs(kind)
            }
            "Environment" => {
                let (example, reason, docs) = match kind {
                    Typst | Tinymist => (r#"{"TYPST_FONT_PATHS":"/Users/me/fonts"}"#, "Search an extra font directory.", "https://github.com/typst/typst/blob/main/crates/typst-cli/src/args.rs"),
                    Tectonic => (r#"{"TECTONIC_CACHE_DIR":"/Users/me/tectonic-cache"}"#, "Store downloaded TeX resources in this directory.", "https://tectonic-typesetting.github.io/book/latest/getting-started/first-document.html#cache"),
                    Badness => (r#"{"BADNESS_CONFIG":"/Users/me/paper/badness.toml"}"#, "Choose a project configuration file.", "https://badness.dev/reference/configuration.html"),
                    Texlab | TexFmt => (r#"{"RUST_BACKTRACE":"1"}"#, "Include a stack trace if the Rust tool crashes. Normal operation is unchanged.", "https://doc.rust-lang.org/std/backtrace/index.html#environment-variables"),
                };
                ui.label(reason); ui.monospace(example);
                ui.label("JSON object of strings; overrides inherited values. Use absolute paths, not $HOME or ~."); docs
            }
            _ => {
                ui.monospace("/Users/me/paper");
                ui.label("An existing project directory, without quotes. Relative paths and project-file discovery start here. Blank keeps the document/project default.");
                match kind {
                    Badness => "https://badness.dev/reference/configuration.html",
                    TexFmt => "https://github.com/WGUNDERWOOD/tex-fmt#configuration",
                    Tinymist | Typst => "https://myriad-dreamin.github.io/tinymist/feature/compiler-settings.html#packages-roots-and-certificates",
                    Texlab => "https://github.com/latex-lsp/texlab/wiki/Configuration#texlabrootdirectory",
                    Tectonic => "https://tectonic-typesetting.github.io/book/latest/ref/tectonic-toml.html",
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
                        ui.label(label);
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
    ui.horizontal(|ui| {
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
