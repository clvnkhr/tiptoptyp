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
        json_setting(ui, "tex-prose-rules", &mut edited.tex_prose_rules, settings,
            crate::writing::prose_rules::TexProseRules::validate,
            "Apply prose rules", "Reset prose rules");
    });
}

pub(super) fn ltex(ui: &mut egui::Ui, edited: &mut AppSettings, settings: &AppSettings) -> bool {
    let mut restart = false;
    ui.separator();
    let enabled = ui.checkbox(&mut edited.ltex.enabled, "LTeX+ spelling and grammar");
    #[cfg(all(feature = "desktop-ui-tests", target_os = "macos"))]
    crate::desktop_test::observe("settings.ltex.enabled", &enabled);
    #[cfg(not(all(feature = "desktop-ui-tests", target_os = "macos")))]
    let _ = enabled;
    ui.weak("Optional local LanguageTool checker for TeX and Typst. Results appear in Problems; Harper remains independent.");
    egui::CollapsingHeader::new("LTeX+ setup and configuration").show(ui, |ui| {
        ui.horizontal_wrapped(|ui| {
            ui.hyperlink_to("Download LTeX+", "https://github.com/ltex-plus/ltex-ls-plus/releases");
            ui.hyperlink_to("Configuration reference", "https://ltex-plus.github.io/ltex-plus/settings.html");
        });
        ui.weak("Choose the launcher from an extracted distribution, or use ltex-ls-plus on PATH. Mac distributions include Java. Nothing is downloaded automatically.");
        let id = ui.id().with("ltex-setup-draft");
        let mut draft = ui.ctx().data_mut(|data| data.get_temp::<(crate::ltex::LtexSettings, crate::ltex::LtexSettings)>(id))
            .filter(|(base, _)| base == &edited.ltex)
            .map(|(_, draft)| draft).unwrap_or_else(|| edited.ltex.clone());
        super::settings_controls::settings_control_row(ui, "Executable", |ui| {
            ui.add(egui::TextEdit::singleline(&mut draft.executable).hint_text("ltex-ls-plus").desired_width(f32::INFINITY));
        });
        super::settings_controls::settings_control_row(ui, "Arguments", |ui| {
            ui.add(egui::TextEdit::singleline(&mut draft.command.arguments).font(egui::TextStyle::Monospace).desired_width(f32::INFINITY));
        });
        super::settings_controls::settings_control_row(ui, "Environment", |ui| {
            ui.add(egui::TextEdit::singleline(&mut draft.command.environment).font(egui::TextStyle::Monospace).desired_width(f32::INFINITY));
        });
        super::settings_controls::settings_control_row(ui, "Working directory", |ui| {
            ui.add(egui::TextEdit::singleline(&mut draft.command.directory).desired_width(f32::INFINITY));
        });
        let configuration_id = ui.id().with("ltex-json-draft");
        let mut json = ui.ctx().data_mut(|data| data.get_temp::<(serde_json::Value, String)>(configuration_id))
            .filter(|(base, _)| base == &draft.configuration)
            .map(|(_, draft)| draft).unwrap_or_else(|| serde_json::to_string_pretty(&draft.configuration).unwrap());
        super::code_editor::editor(ui, "ltex-configuration", &mut json, "json", settings, 8);
        let validation = if json.len() > 2_000_000 {
            Err("Configuration exceeds 2 MB".into())
        } else {
            serde_json::from_str(&json).map_err(|e| e.to_string()).and_then(|value| {
                draft.configuration = value;
                draft.validate()
            })
        };
        ui.horizontal(|ui| {
            if ui.add_enabled(validation.is_ok(), egui::Button::new("Apply LTeX settings")).clicked() {
                edited.ltex = draft.clone();
            }
            restart |= ui.add_enabled(edited.ltex.enabled, egui::Button::new("Restart LTeX+")).clicked();
            if ui.button("Reset LTeX settings").clicked() {
                edited.ltex = Default::default();
                draft = edited.ltex.clone();
                json = serde_json::to_string_pretty(&draft.configuration).unwrap();
            }
        });
        if let Err(error) = validation { ui.colored_label(ui.visuals().error_fg_color, error); }
        ui.ctx().data_mut(|data| {
            data.insert_temp(configuration_id, (draft.configuration.clone(), json));
            data.insert_temp(id, (edited.ltex.clone(), draft));
        });
    });
    restart
}

fn json_setting<T>(
    ui: &mut egui::Ui,
    salt: &str,
    value: &mut T,
    settings: &AppSettings,
    validate: fn(&T) -> Result<(), String>,
    apply: &str,
    reset: &str,
) where
    T: Serialize + DeserializeOwned + Default + Clone + PartialEq + Send + Sync + 'static,
{
    let id = ui.id().with(salt);
    let mut text = ui
        .ctx()
        .data_mut(|data| data.get_temp::<(T, String)>(id))
        .filter(|(base, _)| base == value)
        .map(|(_, draft)| draft)
        .unwrap_or_else(|| serde_json::to_string_pretty(value).unwrap());
    super::code_editor::editor(ui, salt, &mut text, "json", settings, 12);
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
        if ui
            .add_enabled(parsed.is_ok(), egui::Button::new(apply))
            .clicked()
        {
            *value = parsed.as_ref().unwrap().clone();
        }
        if ui.button(reset).clicked() {
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
