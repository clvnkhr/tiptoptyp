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
    settings.window_background.validate()?;
    for (name, animation) in [
        ("source_reveal_animation", settings.source_reveal_animation),
        (
            "preview_reveal_animation",
            settings.preview_reveal_animation,
        ),
    ] {
        animation
            .validate()
            .map_err(|error| format!("{name}.{error}"))?;
    }
    crate::liquid_runtime::validate_settings(
        &settings.liquid_cache_directory,
        settings.liquid_threads,
    )?;
    settings.liquid_input.validate()?;
    settings
        .tex_prose_rules
        .validate()
        .map_err(|e| format!("tex_prose_rules: {e}"))?;
    settings.ltex.validate().map_err(|e| format!("ltex: {e}"))?;
    crate::writing::validate_dictionary(&settings.harper_dictionary)
        .map_err(|error| format!("harper_dictionary: {error}"))?;
    crate::snippets::validate(&settings.snippet_presets)
        .map_err(|error| format!("snippet_presets: {error}"))?;
    crate::snippets::validate(&settings.snippets).map_err(|error| format!("snippets: {error}"))?;
    crate::tinymist::validate_server_configuration(&settings.tinymist_configuration)
        .map_err(|error| format!("tinymist_configuration: {error}"))?;
    if settings.indentation_guide_character.chars().count() != 1
        || settings
            .indentation_guide_character
            .chars()
            .any(|c| c.is_control() || c.is_whitespace())
    {
        return Err(
            "indentation_guide_character: choose one visible character, e.g. │ or ┆".into(),
        );
    }
    if settings.highlight_rules.len() > 100
        || settings.highlight_rules.iter().any(|r| {
            r.text.chars().count() > 256
                || r.text.contains(['\n', '\r'])
                || r.style
                    .weight
                    .is_some_and(|weight| !(1..=1000).contains(&weight))
        })
    {
        return Err("highlight_rules: use at most 100 rules, each matching a single line of up to 256 characters; font weights must be 1–1000".into());
    }
    settings.tex.validate_build_settings()?;
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
            "indent_spaces (0–16; 0 means tabs)",
            settings.indent_spaces <= 16,
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
    for (name, kind, tool) in [
        ("typst", crate::toolchain::ToolKind::Typst, &settings.typst),
        (
            "tinymist",
            crate::toolchain::ToolKind::Tinymist,
            &settings.tinymist,
        ),
        (
            "tex.tectonic",
            crate::toolchain::ToolKind::Tectonic,
            &settings.tex.tectonic,
        ),
        (
            "tex.texlab",
            crate::toolchain::ToolKind::Texlab,
            &settings.tex.texlab,
        ),
        (
            "tex.badness",
            crate::toolchain::ToolKind::Badness,
            &settings.tex.badness,
        ),
        (
            "tex.tex_fmt",
            crate::toolchain::ToolKind::TexFmt,
            &settings.tex.tex_fmt,
        ),
    ] {
        tool.command
            .validate_for_tool(kind)
            .map_err(|e| format!("{name}: {e}"))?;
        if tool.mode == ToolMode::Custom && tool.custom_path.trim().is_empty() {
            return Err(format!("{name}: custom path cannot be empty"));
        }
    }
    settings
        .latexdiff
        .validate()
        .map_err(|e| format!("latexdiff: {e}"))?;
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
    #[test]
    fn removed_python_setting_is_not_silently_accepted() {
        let mut value = serde_json::to_value(AppSettings::default()).unwrap();
        assert!(value.get("liquid_python").is_none());
        value["liquid_python"] = serde_json::json!("python3.12");
        let error = super::parse(&value.to_string()).unwrap_err();
        assert!(
            error.starts_with("Unknown or invalid setting: settings.liquid_python"),
            "{error}"
        );
    }

    #[test]
    fn liquid_configuration_rejects_invalid_paths_and_thread_counts() {
        let mut value = serde_json::to_value(AppSettings::default()).unwrap();
        assert!(super::parse(&value.to_string()).is_ok());
        for (key, invalid, expected) in [
            (
                "liquid_cache_directory",
                serde_json::json!("relative/model"),
                "absolute folder",
            ),
            ("liquid_threads", serde_json::json!(0), "between 1 and 8"),
            ("liquid_threads", serde_json::json!(9), "between 1 and 8"),
        ] {
            let before = value[key].clone();
            value[key] = invalid;
            let error = super::parse(&value.to_string()).unwrap_err();
            assert!(error.contains(key) && error.contains(expected), "{error}");
            value[key] = before;
        }
    }

    #[test]
    fn liquid_input_choices_reject_unknown_values_and_invalid_placeholders() {
        let mut value = serde_json::to_value(AppSettings::default()).unwrap();
        for protected in ["placeholder", "split"] {
            for formatted in ["include", "skip"] {
                value["liquid_input"]["protected_content"] = serde_json::json!(protected);
                value["liquid_input"]["formatted_text"] = serde_json::json!(formatted);
                value["liquid_input"]["placeholder"] = serde_json::json!("MATH");
                let parsed = super::parse(&value.to_string()).unwrap();
                assert_eq!(parsed.liquid_input.placeholder, "MATH");
            }
        }
        for (key, invalid) in [
            ("protected_content", serde_json::json!("raw")),
            ("formatted_text", serde_json::json!("sometimes")),
            ("placeholder", serde_json::json!("")),
            ("placeholder", serde_json::json!("$math$")),
            ("placeholder", serde_json::json!("two words")),
            ("placeholder", serde_json::json!("a".repeat(25))),
        ] {
            let mut invalid_settings = value.clone();
            invalid_settings["liquid_input"][key] = invalid;
            let error = super::parse(&invalid_settings.to_string()).unwrap_err();
            assert!(error.contains(key), "{error}");
        }
    }

    #[test]
    fn motion_json_accepts_valid_options_and_rejects_impossible_or_out_of_range_values() {
        use crate::settings::{EditorAction, ParticleDensity, ParticleStyle, RevealStyle};
        let mut value = serde_json::to_value(AppSettings::default()).unwrap();
        for style in RevealStyle::ALL {
            for key in ["source_reveal_animation", "preview_reveal_animation"] {
                value[key]["style"] = serde_json::to_value(style).unwrap();
            }
            assert!(super::parse(&value.to_string()).is_ok());
        }
        for density in ParticleDensity::ALL {
            value["editor_effects"]["density"] = serde_json::to_value(density).unwrap();
            assert!(super::parse(&value.to_string()).is_ok());
        }
        for action in EditorAction::ALL {
            for style in ParticleStyle::ALL.into_iter().map(Some).chain([None]) {
                value["editor_effects"]["actions"][action.key()]["particles"] =
                    serde_json::to_value(style).unwrap();
                assert!(super::parse(&value.to_string()).is_ok());
            }
            let mut invalid_style = value.clone();
            invalid_style["editor_effects"]["actions"][action.key()]["particles"] =
                serde_json::json!("Impossible");
            let error = super::parse(&invalid_style.to_string()).unwrap_err();
            assert!(
                error.contains(&format!(
                    "editor_effects.actions.{}.particles",
                    action.key()
                )) && error.contains("choose")
                    && ParticleStyle::ALL
                        .into_iter()
                        .all(|style| error.contains(style.label())),
                "{error}"
            );
            for field in ["shake", "flash", "glow"] {
                let mut invalid_toggle = value.clone();
                invalid_toggle["editor_effects"]["actions"][action.key()][field] =
                    serde_json::json!("yes");
                let error = super::parse(&invalid_toggle.to_string()).unwrap_err();
                assert!(
                    error.contains(&format!("editor_effects.actions.{}.{field}", action.key()))
                        && error.contains("boolean"),
                    "{error}"
                );
            }
        }
        for key in ["source_reveal_animation", "preview_reveal_animation"] {
            for (field, invalid, expected) in [
                ("duration_ms", 249, "250 and 2000"),
                ("duration_ms", 2001, "250 and 2000"),
                ("size_percent", 49, "50 and 200"),
                ("size_percent", 201, "50 and 200"),
            ] {
                let mut invalid_value = value.clone();
                invalid_value[key]["enabled"] = serde_json::json!(false);
                invalid_value[key][field] = serde_json::json!(invalid);
                let error = super::parse(&invalid_value.to_string()).unwrap_err();
                assert!(
                    error.contains(&format!("{key}.{field}")) && error.contains(expected),
                    "{error}"
                );
            }
            let mut invalid_value = value.clone();
            invalid_value[key]["style"] = serde_json::json!("Impossible");
            let error = super::parse(&invalid_value.to_string()).unwrap_err();
            assert!(
                error.contains(&format!("{key}.style"))
                    && error.contains("choose")
                    && error.contains("Spotlight"),
                "{error}"
            );
        }
        value["editor_effects"]["density"] = serde_json::json!("Impossible");
        let error = super::parse(&value.to_string()).unwrap_err();
        assert!(
            error.contains("editor_effects.density")
                && error.contains("choose")
                && error.contains("Medium"),
            "{error}"
        );
    }

    #[test]
    fn personal_dictionary_json_validates_words_and_name_toggle_types() {
        let mut settings = AppSettings {
            harper_dictionary: "MacDonald\nÉvariste\nwell-posedness\n".into(),
            harper_ignore_capitalized_names: true,
            ..Default::default()
        };
        let json = serde_json::to_string(&settings).unwrap();
        assert_eq!(super::parse(&json).unwrap(), settings);
        settings.harper_dictionary = "valid\ntwo words".into();
        let error = super::parse(&serde_json::to_string(&settings).unwrap()).unwrap_err();
        assert!(
            error.contains("harper_dictionary") && error.contains("2"),
            "{error}"
        );
        let mut value = serde_json::to_value(AppSettings::default()).unwrap();
        value["harper_ignore_capitalized_names"] = serde_json::json!("yes");
        assert!(
            super::parse(&value.to_string())
                .unwrap_err()
                .contains("harper_ignore_capitalized_names")
        );
    }

    use super::*;
    #[test]
    fn tex_preview_follow_edits_json_accepts_booleans_and_rejects_other_types() {
        let mut value = serde_json::to_value(AppSettings::default()).unwrap();
        for enabled in [false, true] {
            value["tex_preview_follow_edits"] = serde_json::json!(enabled);
            let parsed = parse(&value.to_string()).unwrap();
            assert_eq!(parsed.tex_preview_follow_edits, enabled);
            assert!(parsed.preview_follow_edits);
            assert_eq!(
                parse(&serde_json::to_string(&parsed).unwrap()).unwrap(),
                parsed
            );
        }
        for invalid in [
            serde_json::json!("yes"),
            serde_json::json!(1),
            serde_json::Value::Null,
        ] {
            value["tex_preview_follow_edits"] = invalid;
            let error = parse(&value.to_string()).unwrap_err();
            assert!(
                error.contains("tex_preview_follow_edits") && error.contains("bool"),
                "{error}"
            );
        }
    }

    #[test]
    fn compact_layout_controls_json_accepts_boolean_modes_and_rejects_invalid_choices() {
        let mut value = serde_json::to_value(AppSettings::default()).unwrap();
        for compact in [false, true] {
            value["compact_layout_controls"] = serde_json::json!(compact);
            let parsed = parse(&value.to_string()).unwrap();
            assert_eq!(parsed.compact_layout_controls, compact);
            assert_eq!(
                parse(&serde_json::to_string(&parsed).unwrap()).unwrap(),
                parsed
            );
        }
        for invalid in [
            serde_json::json!("old"),
            serde_json::json!(1),
            serde_json::Value::Null,
        ] {
            value["compact_layout_controls"] = invalid;
            let error = parse(&value.to_string()).unwrap_err();
            assert!(
                error.contains("compact_layout_controls") && error.contains("bool"),
                "{error}"
            );
        }
    }

    #[test]
    fn snippet_suggestions_default_for_missing_field_but_saved_empty_stays_empty() {
        let defaults = AppSettings::default();
        let mut value = serde_json::to_value(&defaults).unwrap();
        value["snippets"] = serde_json::json!([{
            "prefix": "mine", "description": "My template", "language": "typst",
            "body": "$ {{1:x}} ${{0}}", "scope": "text", "line_start": false
        }]);
        value.as_object_mut().unwrap().remove("snippet_presets");
        let parsed = parse(&value.to_string()).unwrap();
        assert_eq!(parsed.snippet_presets, defaults.snippet_presets);
        assert_eq!(parsed.snippets[0].prefix, "mine");
        assert!(parsed.snippets[0].enabled);
        assert_eq!(parsed.snippets[0].placeholder_open, "{{");
        assert_eq!(parsed.snippets[0].placeholder_close, "}}");
        value["snippet_presets"] = serde_json::json!([]);
        let parsed = parse(&value.to_string()).unwrap();
        assert!(parsed.snippet_presets.is_empty());
        assert_eq!(
            parse(&serde_json::to_string(&parsed).unwrap()).unwrap(),
            parsed
        );
    }

    #[test]
    fn snippet_scope_choices_and_drafts_are_validated_for_both_lists() {
        for name in ["snippets", "snippet_presets"] {
            let mut value = serde_json::to_value(AppSettings::default()).unwrap();
            value[name] = serde_json::json!([{
                "prefix": "mine", "description": "My template", "language": "typst",
                "body": "{{1:x}}{{0}}", "scope": "impossible", "line_start": false
            }]);
            let error = parse(&value.to_string()).unwrap_err();
            assert!(
                error.contains(name)
                    && error.contains("scope")
                    && error.contains("choose")
                    && error.contains("math"),
                "{error}"
            );
            value[name][0]["scope"] = serde_json::json!("math");
            value[name][0]["body"] = serde_json::json!("{{1:unfinished");
            let error = parse(&value.to_string()).unwrap_err();
            assert!(error.contains(name) && error.contains("Close"), "{error}");
            value[name][0]["body"] = serde_json::json!("{{1:x}}{{0}}");
            assert!(parse(&value.to_string()).is_ok());
        }
    }

    #[test]
    fn disabled_snippets_and_custom_placeholder_pairs_roundtrip_and_still_validate() {
        for list in ["snippets", "snippet_presets"] {
            let mut value = serde_json::to_value(AppSettings::default()).unwrap();
            value[list] = serde_json::json!([{
                "prefix": "mine", "description": "A temporarily disabled template",
                "language": "typst", "scope": "math", "line_start": false,
                "enabled": false, "placeholder_open": "⟪", "placeholder_close": "⟫",
                "body": "frac(⟪1:x⟫, ⟪2:y⟫)⟪0⟫"
            }]);
            let parsed = parse(&value.to_string()).unwrap();
            let entries = if list == "snippets" {
                &parsed.snippets
            } else {
                &parsed.snippet_presets
            };
            assert!(!entries[0].enabled);
            assert_eq!(entries[0].placeholder_open, "⟪");
            assert_eq!(entries[0].placeholder_close, "⟫");
            assert_eq!(
                parse(&serde_json::to_string(&parsed).unwrap()).unwrap(),
                parsed
            );
            value[list][0]["placeholder_open"] = serde_json::json!("bad marker");
            let error = parse(&value.to_string()).unwrap_err();
            assert!(error.contains(list) && error.contains("marker"), "{error}");
            value[list][0]["placeholder_open"] = serde_json::json!("⟪");
            value[list][0]["body"] = serde_json::json!("⟪1:unfinished");
            assert!(
                parse(&value.to_string()).is_err(),
                "disabled definitions must remain valid"
            );
            value[list][0]["body"] = serde_json::json!("⟪1:x⟫");
            value[list][0]["field_command"] = serde_json::json!("snip");
            let error = parse(&value.to_string()).unwrap_err();
            assert!(error.contains("field_command"), "{error}");
        }
    }

    #[test]
    fn tinymist_configuration_uses_shared_validation_and_roundtrips() {
        let mut value = serde_json::to_value(AppSettings::default()).unwrap();
        value["tinymist_configuration"]["formatterMode"] = serde_json::json!("invalid");
        let error = parse(&value.to_string()).unwrap_err();
        assert!(error.contains("tinymist_configuration") && error.contains("typstyle"));
        value["tinymist_configuration"] = serde_json::json!({"formatterPrintWidth":100});
        assert_eq!(
            parse(&value.to_string()).unwrap().tinymist_configuration["formatterPrintWidth"],
            100
        );
        value
            .as_object_mut()
            .unwrap()
            .remove("tinymist_configuration");
        value["hover_delay_ms"] = serde_json::json!(123);
        let parsed = parse(&value.to_string()).unwrap();
        assert_eq!(parsed.hover_delay_ms, 123);
        assert_eq!(
            parsed.tinymist_configuration,
            crate::tinymist::default_server_configuration()
        );
    }

    #[test]
    fn impossible_choices_identify_the_field_and_allowed_options() {
        for (path, field, valid) in [
            ("/handwriting/engine", "handwriting.engine", "detexify"),
            ("/writing_language", "writing_language", "british"),
            ("/interface_theme", "interface_theme", "System"),
            ("/tex/build_engine", "tex.build_engine", "tectonic"),
            ("/tex/build_driver", "tex.build_driver", "latexmk"),
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
    fn partial_preview_roundtrips_and_invalid_mode_reports_valid_choices() {
        let mut value = serde_json::to_value(AppSettings::default()).unwrap();
        value["preview_preference"] = serde_json::json!("InteractivePartial");
        assert_eq!(
            parse(&value.to_string()).unwrap().preview_preference,
            crate::settings::PreviewPreference::InteractivePartial
        );
        value["preview_preference"] = serde_json::json!("impossible");
        let error = parse(&value.to_string()).unwrap_err();
        assert!(
            error.contains("preview_preference")
                && error.contains("choose")
                && error.contains("InteractivePartial"),
            "{error}"
        );
    }

    #[test]
    fn local_use_highlights_round_trip_and_old_declaration_setting_is_rejected() {
        let settings = AppSettings {
            highlight_local_uses: true,
            ..Default::default()
        };
        let mut value = serde_json::to_value(&settings).unwrap();
        assert_eq!(value["highlight_local_uses"], true);
        assert_eq!(parse(&value.to_string()).unwrap(), settings);
        value
            .as_object_mut()
            .unwrap()
            .remove("highlight_local_uses");
        value["highlight_definitions"] = serde_json::json!(true);
        assert!(
            parse(&value.to_string())
                .unwrap_err()
                .contains("highlight_definitions")
        );
    }

    #[test]
    fn retired_argument_template_is_not_a_literal_argv_or_a_compatibility_alias() {
        let defaults = AppSettings::default();
        let mut value = serde_json::to_value(&defaults).unwrap();
        value["typst"]["command"]
            .as_object_mut()
            .unwrap()
            .remove("argv");
        value["typst"]["command"]["arguments"] = serde_json::json!("{args}");
        // Persisted older settings discard the removed field; optional argv
        // receives this tool's defaults. Explicit JSON edits reject obsolete keys.
        let loaded: AppSettings = serde_json::from_value(value.clone()).unwrap();
        assert_eq!(loaded, defaults);
        assert!(parse(&value.to_string()).unwrap_err().contains("arguments"));
        assert!(
            !serde_json::to_value(loaded).unwrap()["typst"]["command"]
                .as_object()
                .unwrap()
                .contains_key("arguments")
        );
    }

    #[test]
    fn tool_arguments_show_defaults_and_missing_nested_values_keep_them() {
        let defaults = AppSettings::default();
        let original = serde_json::to_value(&defaults).unwrap();
        for (path, expected) in [
            ("/typst", "watch --diagnostic-format short"),
            ("/tinymist", "lsp"),
            (
                "/tex/tectonic",
                "-X --chatter minimal --color never compile --untrusted --keep-logs --keep-intermediates",
            ),
            ("/tex/texlab", ""),
            ("/tex/badness", "lsp"),
            ("/tex/tex_fmt", "--stdin --quiet"),
        ] {
            assert_eq!(
                original.pointer(&format!("{path}/command/argv")).unwrap(),
                expected
            );
            for remove_command in [false, true] {
                let mut value = original.clone();
                let tool = value.pointer_mut(path).unwrap().as_object_mut().unwrap();
                if remove_command {
                    tool.remove("command");
                } else {
                    tool["command"].as_object_mut().unwrap().remove("argv");
                }
                assert_eq!(parse(&value.to_string()).unwrap(), defaults, "{path}");
            }
            let mut value = original.clone();
            *value.pointer_mut(&format!("{path}/command/argv")).unwrap() = serde_json::json!("");
            if expected.is_empty() {
                assert!(parse(&value.to_string()).is_ok());
            } else {
                let error = parse(&value.to_string()).unwrap_err();
                assert!(error.contains("must retain"), "{error}");
            }
        }
        let mut value = original;
        value["typst"]["command"]["environment"] = serde_json::json!(r#"{"MODE":"draft"}"#);
        value["typst"]["command"]
            .as_object_mut()
            .unwrap()
            .remove("argv");
        let parsed = parse(&value.to_string()).unwrap();
        assert_eq!(
            parsed.typst.command.arguments,
            "watch --diagnostic-format short"
        );
        assert_eq!(parsed.typst.command.environment, r#"{"MODE":"draft"}"#);
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
            ("/typst/command/argv", serde_json::json!("'unfinished")),
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

#[cfg(test)]
mod editor_option_tests {
    use super::*;
    #[test]
    fn editor_decoration_options_roundtrip_and_reject_invalid_values() {
        let mut settings = AppSettings {
            indent_spaces: 0,
            ..Default::default()
        };
        assert_eq!(
            parse(&serde_json::to_string(&settings).unwrap()).unwrap(),
            settings
        );
        for character in ["", "ab", "\n", " "] {
            settings.indentation_guide_character = character.into();
            assert!(
                parse(&serde_json::to_string(&settings).unwrap())
                    .unwrap_err()
                    .contains("indentation_guide_character")
            );
        }
        settings.indentation_guide_character = "┆".into();
        settings.highlight_rules[0].text = "one\ntwo".into();
        assert!(
            parse(&serde_json::to_string(&settings).unwrap())
                .unwrap_err()
                .contains("highlight_rules")
        );
    }
}

#[cfg(test)]
mod writing_service_settings_tests {
    use super::*;

    #[test]
    fn writing_defaults_roundtrip_with_full_prose_registry() {
        let settings = AppSettings::default();
        assert!(!settings.ltex.enabled);
        assert!(
            settings
                .tex_prose_rules
                .commands
                .iter()
                .any(|c| c.name == "footnote")
        );
        let text = serde_json::to_string(&settings).unwrap();
        assert_eq!(parse(&text).unwrap(), settings);
    }

    #[test]
    fn invalid_ltex_choices_and_prose_roles_have_setting_paths() {
        for (field, value, expected) in [
            (
                "/ltex/configuration/checkFrequency",
                serde_json::json!("edit"),
                "checkFrequency",
            ),
            (
                "/ltex/configuration/dictionary",
                serde_json::json!(["word"]),
                "dictionary",
            ),
            (
                "/ltex/configuration/completionEnabled",
                serde_json::json!("yes"),
                "completionEnabled",
            ),
            (
                "/tex_prose_rules/commands/0/arguments/0",
                serde_json::json!("probably-prose"),
                "tex_prose_rules.commands[0].arguments[0]",
            ),
        ] {
            let mut settings = serde_json::to_value(AppSettings::default()).unwrap();
            let (parent, name) = field.rsplit_once('/').unwrap();
            if let Some(object) = settings.pointer_mut(parent).unwrap().as_object_mut() {
                object.insert(name.into(), value);
            } else {
                *settings.pointer_mut(field).unwrap() = value;
            }
            let error = parse(&settings.to_string()).unwrap_err();
            assert!(error.contains(expected), "{field}: {error}");
        }
    }
}
