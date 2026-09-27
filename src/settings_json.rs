//! The editable JSON settings view uses the same schema and update path as the form.
use crate::settings::{AppSettings, ColorThemeChoice, ToolMode};

pub(crate) fn parse(text: &str) -> Result<AppSettings, String> {
    if text.len() > 2_000_000 {
        return Err("Settings JSON exceeds 2 MB".into());
    }
    let mut deserializer = serde_json::Deserializer::from_str(text);
    let settings: AppSettings =
        serde_path_to_error::deserialize(&mut deserializer).map_err(|error| {
            let path = error.path().to_string();
            let detail = error
                .inner()
                .to_string()
                .replace("unknown variant", "unsupported option")
                .replace("expected one of", "choose one of")
                .replace("expected `", "choose `");
            format!("{path}: {detail}")
        })?;
    let original: serde_json::Value = serde_json::from_str(text).map_err(|e| e.to_string())?;
    let roundtrip = serde_json::to_value(&settings).map_err(|e| e.to_string())?;
    reject_unknown(&original, &roundtrip, "settings")?;
    crate::snippets::validate(&settings.snippets)?;
    for (name, valid) in [
        (
            "ui_scale_percent (75–150)",
            (75..=150).contains(&settings.ui_scale_percent),
        ),
        (
            "auto_save_delay_ms (250–5000)",
            (250..=5000).contains(&settings.auto_save_delay_ms),
        ),
        ("hover_delay_ms (0–2000)", settings.hover_delay_ms <= 2000),
        (
            "indent_spaces (1–16)",
            (1..=16).contains(&settings.indent_spaces),
        ),
        (
            "ui_font_weight (1–1000)",
            (1..=1000).contains(&settings.ui_font_weight),
        ),
        (
            "code_font_weight (1–1000)",
            (1..=1000).contains(&settings.code_font_weight),
        ),
        (
            "theme_hue_shift_degrees (-180–180)",
            (-180..=180).contains(&settings.theme_hue_shift_degrees),
        ),
        (
            "theme_colors",
            settings.theme_colors == settings.theme_colors.normalized(),
        ),
        (
            "tex.texlab_configuration (JSON object)",
            settings.tex.texlab_configuration.is_object(),
        ),
        (
            "tex.badness_configuration (JSON object)",
            settings.tex.badness_configuration.is_object(),
        ),
    ] {
        if !valid {
            return Err(format!("Invalid {name}"));
        }
    }
    for choice in [&settings.light_theme, &settings.dark_theme] {
        if let ColorThemeChoice::Builtin(id) = choice
            && crate::builtin_themes::find(id).is_none()
        {
            return Err(format!("Unknown built-in theme: {id}"));
        }
    }
    let mut shortcuts = settings.shortcut_overrides.clone();
    let report = shortcuts.normalize();
    if report.unknown_actions != 0 || report.invalid_bindings != 0 {
        return Err("shortcut_overrides contains an unknown action or invalid key binding".into());
    }
    for (name, tool) in [
        ("typst", &settings.typst),
        ("tinymist", &settings.tinymist),
        ("tex.tectonic", &settings.tex.tectonic),
        ("tex.texlab", &settings.tex.texlab),
        ("tex.badness", &settings.tex.badness),
        ("tex.tex_fmt", &settings.tex.tex_fmt),
    ] {
        tool.command
            .validate()
            .map_err(|e| format!("{name}: {e}"))?;
        if tool.mode == ToolMode::Custom && tool.custom_path.trim().is_empty() {
            return Err(format!("{name}: custom path cannot be empty"));
        }
    }
    Ok(settings)
}

fn reject_unknown(
    input: &serde_json::Value,
    output: &serde_json::Value,
    path: &str,
) -> Result<(), String> {
    if let Some(object) = input.as_object() {
        for (key, value) in object {
            // Empty shortcut overrides are deliberately omitted when serializing.
            if path == "settings"
                && key == "shortcut_overrides"
                && value.as_object().is_some_and(|v| v.is_empty())
            {
                continue;
            }
            let Some(known) = output.get(key) else {
                return Err(format!("Unknown or invalid setting: {path}.{key}"));
            };
            reject_unknown(value, known, &format!("{path}.{key}"))?;
        }
    }
    if let (Some(input), Some(output)) = (input.as_array(), output.as_array()) {
        for (index, (value, known)) in input.iter().zip(output).enumerate() {
            reject_unknown(value, known, &format!("{path}[{index}]"))?;
        }
    }
    Ok(())
}

pub(crate) struct Draft {
    pub(crate) text: String,
    pub(crate) base: AppSettings,
    pub(crate) validation: Result<AppSettings, String>,
    pub(crate) saved: bool,
}
impl Draft {
    pub(crate) fn new(settings: &AppSettings) -> Self {
        let text = serde_json::to_string_pretty(settings).expect("settings serialize");
        let validation = parse(&text);
        Self {
            text,
            base: settings.clone(),
            validation,
            saved: false,
        }
    }
    pub(crate) fn validate(&mut self) {
        self.validation = parse(&self.text);
        self.saved = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn impossible_choices_identify_the_field_and_allowed_options() {
        for (path, field, valid) in [
            ("/writing_language", "writing_language", "british"),
            ("/interface_theme", "interface_theme", "System"),
            ("/tex/build_engine", "tex.build_engine", "tectonic"),
            ("/tex/formatter", "tex.formatter", "badness"),
            ("/typst/mode", "typst.mode", "Bundled"),
            ("/light_theme/source", "light_theme", "builtin"),
        ] {
            let mut json = serde_json::to_value(AppSettings::default()).unwrap();
            *json.pointer_mut(path).unwrap() = serde_json::json!("impossible");
            let error = parse(&json.to_string()).unwrap_err();
            assert!(error.contains(field), "{error}");
            assert!(
                error.contains("impossible") && error.contains(valid),
                "{error}"
            );
            assert!(error.contains("choose"), "{error}");
        }
    }

    #[test]
    fn roundtrip_defaults_and_reject_invalid_edits() {
        let original = AppSettings::default();
        assert_eq!(Draft::new(&original).validation.unwrap(), original);
        for (path, value) in [
            ("/ui_scale_percent", serde_json::json!(0)),
            ("/indent_spaces", serde_json::json!(99)),
            ("/tex/badness_configuration", serde_json::json!(false)),
            ("/typst/command/environment", serde_json::json!("{bad}")),
            ("/typst/command/arguments", serde_json::json!("'unfinished")),
        ] {
            let mut json = serde_json::to_value(&original).unwrap();
            *json.pointer_mut(path).unwrap() = value;
            assert!(parse(&json.to_string()).is_err(), "{path}");
        }
        let mut json = serde_json::to_value(&original).unwrap();
        json["tex"]["typo"] = serde_json::json!(true);
        assert!(parse(&json.to_string()).unwrap_err().contains("typo"));
        assert!(parse("{").is_err());
    }
}
