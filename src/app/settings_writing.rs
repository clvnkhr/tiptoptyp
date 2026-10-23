//! Writing-tool drafts are validated before becoming live settings.
use super::*;
use serde::{Serialize, de::DeserializeOwned};

pub(super) fn prose_rules(ui: &mut egui::Ui, edited: &mut AppSettings, settings: &AppSettings) {
    egui::CollapsingHeader::new("TeX prose rules").show(ui, |ui| {
        ui.horizontal_wrapped(|ui| {
            for role in crate::writing::prose_rules::TexArgumentRole::ALL {
                ui.weak(role.label());
            }
        });
        ui.weak("Commands and environments are matched by name. The full default registry is editable below.");
        json_setting(ui, &mut edited.tex_prose_rules, settings,
            crate::writing::prose_rules::TexProseRules::validate,
            JsonSettingUi { salt: "tex-prose-rules", label: "TeX prose rules", apply: "Apply prose rules", reset: "Reset prose rules", target: None });
    });
}

pub(super) fn harper_rules(ui: &mut egui::Ui, edited: &mut AppSettings, settings: &AppSettings) {
    ui.strong("Harper rule overrides");
    ui.weak("Unlisted rules keep Harper's curated defaults.");
    json_setting(
        ui,
        &mut edited.harper_rule_overrides,
        settings,
        crate::writing::RuleOverrides::validate,
        JsonSettingUi {
            salt: "harper-rule-overrides",
            label: "Harper rule overrides",
            apply: "Apply Harper rules",
            reset: "Reset Harper rules",
            target: Some("settings.harper.rules"),
        },
    );
}

pub(super) fn ltex(
    ui: &mut egui::Ui,
    edited: &mut AppSettings,
    settings: &AppSettings,
    actions: &mut Vec<super::settings_panel::SettingsAction>,
    repair_busy: bool,
    repair_active: bool,
    repair_status: Option<&str>,
) {
    ui.separator();
    let enabled = ui
        .horizontal_wrapped(|ui| {
            let response = ui.checkbox(&mut edited.ltex.enabled, "LTeX+ spelling and grammar");
            super::tool_reference::controls(
                ui,
                crate::tool_reference::ReferenceId::Ltex,
                edited.ltex.mode == crate::settings::ToolMode::Bundled,
                settings,
            );
            response
        })
        .inner;
    #[cfg(all(feature = "desktop-ui-tests", target_os = "macos"))]
    crate::desktop_test::observe("settings.ltex.enabled", &enabled);
    #[cfg(not(all(feature = "desktop-ui-tests", target_os = "macos")))]
    let _ = enabled;
    ui.weak("Bundled with Java. Prepared automatically when enabled.");
    let setup = egui::CollapsingHeader::new("LTeX+ setup and configuration").show(ui, |ui| {
        ui.horizontal_wrapped(|ui| {
            let redownload = ui.add_enabled(!repair_busy, egui::Button::new("Redownload LTeX+"));
            #[cfg(all(feature = "desktop-ui-tests", target_os = "macos"))]
            crate::desktop_test::observe("settings.ltex.redownload", &redownload);
            if redownload.clicked() {
                actions.push(super::settings_panel::SettingsAction::RedownloadLtex);
            }
            if repair_busy && repair_active {
                let cancel = ui.small_button("Cancel");
                cancel.widget_info(|| {
                    egui::WidgetInfo::labeled(
                        egui::WidgetType::Button,
                        true,
                        "Cancel LTeX+ download",
                    )
                });
                #[cfg(all(feature = "desktop-ui-tests", target_os = "macos"))]
                crate::desktop_test::observe("settings.ltex.cancel_redownload", &cancel);
                if cancel.clicked() {
                    actions.push(super::settings_panel::SettingsAction::CancelResourceRepair);
                }
            }
            super::settings_controls::documentation_link(
                ui,
                "Configuration reference",
                "https://ltex-plus.github.io/ltex-plus/settings.html",
            );
            if repair_active && let Some(status) = repair_status {
                ui.weak(status);
            }
        });
        let id = ui.id().with("ltex-setup-draft");
        let mut draft = ui
            .ctx()
            .data_mut(|data| {
                data.get_temp::<(crate::ltex::LtexSettings, crate::ltex::LtexSettings)>(id)
            })
            .map(|(base, draft)| super::tool_drafts::rebase_ltex(&base, &edited.ltex, draft))
            .unwrap_or_else(|| edited.ltex.clone());
        let follow = ui.checkbox(
            &mut draft.follow_document_language,
            "Follow document language",
        );
        #[cfg(all(feature = "desktop-ui-tests", target_os = "macos"))]
        crate::desktop_test::observe("settings.ltex.follow_language", &follow);
        #[cfg(not(all(feature = "desktop-ui-tests", target_os = "macos")))]
        let _ = follow;
        ui.horizontal_wrapped(|ui| {
            for mode in crate::settings::ToolMode::ALL {
                let selected = ui.selectable_value(&mut draft.mode, mode, mode.label());
                #[cfg(all(feature = "desktop-ui-tests", target_os = "macos"))]
                crate::desktop_test::observe(
                    if mode == crate::settings::ToolMode::Bundled {
                        "settings.ltex.mode.bundled"
                    } else {
                        "settings.ltex.mode.custom"
                    },
                    &selected,
                );
                #[cfg(not(all(feature = "desktop-ui-tests", target_os = "macos")))]
                let _ = selected;
            }
        });
        if draft.mode == crate::settings::ToolMode::Custom {
            super::settings_controls::settings_control_row(ui, "Executable", |ui| {
                let executable = ui.add(
                    egui::TextEdit::singleline(&mut draft.executable)
                        .id_salt("ltex-executable")
                        .hint_text("ltex-ls-plus")
                        .desired_width(f32::INFINITY),
                );
                #[cfg(all(feature = "desktop-ui-tests", target_os = "macos"))]
                crate::desktop_test::observe("settings.ltex.executable", &executable);
                executable.widget_info(|| {
                    egui::WidgetInfo::labeled(egui::WidgetType::TextEdit, true, "LTeX+ executable")
                });
            });
        }
        super::settings_controls::settings_control_row(ui, "Arguments", |ui| {
            ui.add(
                egui::TextEdit::singleline(&mut draft.command.arguments)
                    .font(egui::TextStyle::Monospace)
                    .desired_width(f32::INFINITY),
            );
        });
        super::settings_controls::settings_control_row(ui, "Environment", |ui| {
            ui.add(
                egui::TextEdit::singleline(&mut draft.command.environment)
                    .font(egui::TextStyle::Monospace)
                    .desired_width(f32::INFINITY),
            );
        });
        super::settings_controls::settings_control_row(ui, "Working directory", |ui| {
            ui.add(
                egui::TextEdit::singleline(&mut draft.command.directory)
                    .desired_width(f32::INFINITY),
            );
        });
        let configuration_id = ui.id().with("ltex-json-draft");
        let mut json = ui
            .ctx()
            .data_mut(|data| data.get_temp::<(serde_json::Value, String)>(configuration_id))
            .map(|(base, text)| super::tool_drafts::rebase_json(&base, &draft.configuration, text))
            .unwrap_or_else(|| serde_json::to_string_pretty(&draft.configuration).unwrap());
        egui::ScrollArea::vertical()
            .id_salt("ltex-configuration-scroll")
            .max_height(ui.text_style_height(&egui::TextStyle::Monospace) * 12.0)
            .show(ui, |ui| {
                super::code_editor::editor(
                    ui,
                    "ltex-configuration",
                    &mut json,
                    "json",
                    settings,
                    8,
                );
            });
        let validation = if json.len() > 2_000_000 {
            Err("Configuration exceeds 2 MB".into())
        } else {
            serde_json::from_str(&json)
                .map_err(|e| e.to_string())
                .and_then(|value| {
                    draft.configuration = value;
                    draft.validate()
                })
        };
        ui.horizontal(|ui| {
            let apply =
                ui.add_enabled(validation.is_ok(), egui::Button::new("Apply LTeX settings"));
            #[cfg(all(feature = "desktop-ui-tests", target_os = "macos"))]
            crate::desktop_test::observe("settings.ltex.apply", &apply);
            if apply.clicked() {
                edited.ltex = draft.clone();
            }
            if ui
                .add_enabled(
                    edited.ltex.enabled && !repair_busy,
                    egui::Button::new("Restart LTeX+"),
                )
                .clicked()
            {
                actions.push(super::settings_panel::SettingsAction::RestartLtex);
            }
            if ui.button("Reset LTeX settings").clicked() {
                edited.ltex = Default::default();
                draft = edited.ltex.clone();
                json = serde_json::to_string_pretty(&draft.configuration).unwrap();
            }
        });
        if let Err(error) = validation {
            ui.colored_label(ui.visuals().error_fg_color, error);
        }
        ui.ctx().data_mut(|data| {
            data.insert_temp(configuration_id, (draft.configuration.clone(), json));
            data.insert_temp(id, (edited.ltex.clone(), draft));
        });
    });
    #[cfg(all(feature = "desktop-ui-tests", target_os = "macos"))]
    crate::desktop_test::observe("settings.ltex.setup", &setup.header_response);
    #[cfg(not(all(feature = "desktop-ui-tests", target_os = "macos")))]
    let _ = setup;
}

struct JsonSettingUi<'a> {
    salt: &'a str,
    label: &'a str,
    apply: &'a str,
    reset: &'a str,
    target: Option<&'a str>,
}

fn json_setting<T>(
    ui: &mut egui::Ui,
    value: &mut T,
    settings: &AppSettings,
    validate: fn(&T) -> Result<(), String>,
    presentation: JsonSettingUi<'_>,
) where
    T: Serialize + DeserializeOwned + Default + Clone + PartialEq + Send + Sync + 'static,
{
    let JsonSettingUi {
        salt,
        label,
        apply,
        reset,
        target,
    } = presentation;
    let id = ui.id().with(salt);
    let mut text = ui
        .ctx()
        .data_mut(|data| data.get_temp::<(T, String)>(id))
        .map(|(base, text)| super::tool_drafts::rebase_json(&base, value, text))
        .unwrap_or_else(|| serde_json::to_string_pretty(value).unwrap());
    let field = super::code_editor::editor(ui, salt, &mut text, "json", settings, 12);
    ui.ctx()
        .accesskit_node_builder(field.id, |node| node.set_label(label));
    #[cfg(all(feature = "desktop-ui-tests", target_os = "macos"))]
    if let Some(target) = target {
        crate::desktop_test::observe(target, &field);
    }
    #[cfg(not(all(feature = "desktop-ui-tests", target_os = "macos")))]
    let _ = target;
    let parsed = if text.len() > 2_000_000 {
        Err("Configuration exceeds 2 MB".into())
    } else {
        serde_json::from_str::<T>(&text)
            .map_err(|e| e.to_string())
            .and_then(|value| {
                validate(&value)?;
                Ok(value)
            })
    };
    ui.horizontal(|ui| {
        let apply = ui.add_enabled(parsed.is_ok(), egui::Button::new(apply));
        #[cfg(all(feature = "desktop-ui-tests", target_os = "macos"))]
        if let Some(target) = target {
            crate::desktop_test::observe(&format!("{target}.apply"), &apply);
        }
        if apply.clicked() {
            *value = parsed.as_ref().unwrap().clone();
        }
        let reset = ui.button(reset);
        #[cfg(all(feature = "desktop-ui-tests", target_os = "macos"))]
        if let Some(target) = target {
            crate::desktop_test::observe(&format!("{target}.reset"), &reset);
        }
        if reset.clicked() {
            *value = T::default();
            text = serde_json::to_string_pretty(value).unwrap();
        }
    });
    if let Err(error) = parsed {
        ui.colored_label(ui.visuals().error_fg_color, error);
    }
    ui.ctx()
        .data_mut(|data| data.insert_temp(id, (value.clone(), text)));
}
