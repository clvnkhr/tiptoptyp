//! Literal Problems-diagnostic presets. They never change process logging,
//! executable arguments, provider activation, languages, dictionaries or builds.
use crate::{settings::AppSettings, tool_reference::ReferenceId};
use serde_json::{Value, json};
use std::sync::LazyLock;

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ToolProfile {
    #[default]
    Normal,
    Verbose,
}
impl ToolProfile {
    pub(crate) const ALL: [Self; 2] = [Self::Normal, Self::Verbose];

    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Normal => "Normal",
            Self::Verbose => "Verbose",
        }
    }
}

// Tectonic and system TeX share the same underfull-box display preference;
// apply it once when changing all diagnostic providers together.
pub(crate) const DIAGNOSTIC_PROVIDERS: [ReferenceId; 6] = [
    ReferenceId::Tinymist,
    ReferenceId::TexEngine,
    ReferenceId::Texlab,
    ReferenceId::Badness,
    ReferenceId::Ltex,
    ReferenceId::Harper,
];

static NORMAL_TEX: LazyLock<crate::tex::settings::TexSettings> =
    LazyLock::new(crate::tex::settings::TexSettings::default);
static NORMAL_LTEX: LazyLock<crate::ltex::LtexSettings> =
    LazyLock::new(crate::ltex::LtexSettings::default);
static NORMAL_HARPER: LazyLock<crate::writing::RuleOverrides> =
    LazyLock::new(crate::writing::RuleOverrides::default);
static VERBOSE_HARPER: LazyLock<crate::writing::RuleOverrides> = LazyLock::new(|| {
    serde_json::from_value(json!({})).expect("Harper's curated rules must be valid")
});
static EMPTY_ARRAY: LazyLock<Value> = LazyLock::new(|| json!([]));
static EMPTY_OBJECT: LazyLock<Value> = LazyLock::new(|| json!({}));

/// Only controls that actually add Problems diagnostics qualify. A formatter,
/// document generator or a process's debug logging is not a verbose checker.
pub(crate) fn supports_verbose(id: ReferenceId) -> bool {
    matches!(
        id,
        ReferenceId::Tinymist
            | ReferenceId::Tectonic
            | ReferenceId::TexEngine
            | ReferenceId::Texlab
            | ReferenceId::Badness
            | ReferenceId::Ltex
            | ReferenceId::Harper
    )
}

/// Matching reads only the diagnostic fields, allocating nothing per call.
/// Custom arguments, logging, dictionaries and paths never affect this selection.
pub(crate) fn matches(id: ReferenceId, profile: ToolProfile, settings: &AppSettings) -> bool {
    if profile == ToolProfile::Verbose && !supports_verbose(id) {
        return false;
    }
    let verbose = profile == ToolProfile::Verbose;
    match id {
        ReferenceId::Tinymist => {
            settings
                .tinymist_configuration
                .pointer("/lint/enabled")
                .and_then(Value::as_bool)
                == Some(verbose)
                && settings
                    .tinymist_configuration
                    .pointer("/lint/when")
                    .and_then(Value::as_str)
                    == Some("onType")
        }
        ReferenceId::Tectonic | ReferenceId::TexEngine => {
            settings.tex.show_underfull_box_warnings == verbose
        }
        ReferenceId::Texlab => {
            settings.tex.texlab_ignored_codes.is_empty()
                && settings
                    .tex
                    .texlab_configuration
                    .pointer("/texlab/diagnostics/allowedPatterns")
                    == Some(&*EMPTY_ARRAY)
                && settings
                    .tex
                    .texlab_configuration
                    .pointer("/texlab/diagnostics/ignoredPatterns")
                    == Some(texlab_ignored_patterns(profile))
        }
        ReferenceId::Badness => {
            if verbose {
                settings.tex.badness_ignored_codes.is_empty()
            } else {
                settings.tex.badness_ignored_codes == NORMAL_TEX.badness_ignored_codes
            }
        }
        ReferenceId::Ltex => {
            settings.ltex.configuration.get("disabledRules") == Some(ltex_disabled_rules(profile))
                && settings.ltex.configuration.get("enabledRules") == Some(&*EMPTY_OBJECT)
                && settings
                    .ltex
                    .configuration
                    .pointer("/additionalRules/enablePickyRules")
                    .and_then(Value::as_bool)
                    == Some(verbose)
        }
        ReferenceId::Harper => settings.harper_rule_overrides == *harper_rules(profile),
        _ => true,
    }
}

/// No stored preset key: editing literal diagnostic fields immediately reports Custom.
pub(crate) fn selected(id: ReferenceId, settings: &AppSettings) -> Option<ToolProfile> {
    ToolProfile::ALL
        .into_iter()
        .find(|profile| matches(id, *profile, settings))
}

/// A mixed or customized provider configuration has no selected global preset.
pub(crate) fn selected_all(settings: &AppSettings) -> Option<ToolProfile> {
    ToolProfile::ALL.into_iter().find(|profile| {
        DIAGNOSTIC_PROVIDERS
            .into_iter()
            .all(|id| matches(id, *profile, settings))
    })
}

/// Apply atomically. An invalid JSON parent must not partially change filters.
pub(crate) fn apply(
    id: ReferenceId,
    profile: ToolProfile,
    settings: &mut AppSettings,
) -> Result<bool, String> {
    if profile == ToolProfile::Verbose && !supports_verbose(id) {
        return Err(format!("{id:?} has no additional Problems diagnostics"));
    }
    if matches(id, profile, settings) {
        return Ok(false);
    }
    let mut next = settings.clone();
    apply_diagnostics(id, profile, &mut next)?;
    let changed = next != *settings;
    *settings = next;
    Ok(changed)
}

/// One settings copy and one commit even if multiple providers change. Any
/// malformed configuration leaves all providers and every user choice intact.
pub(crate) fn apply_all(profile: ToolProfile, settings: &mut AppSettings) -> Result<bool, String> {
    if selected_all(settings) == Some(profile) {
        return Ok(false);
    }
    let mut next = settings.clone();
    for id in DIAGNOSTIC_PROVIDERS {
        if !matches(id, profile, &next) {
            apply_diagnostics(id, profile, &mut next)?;
        }
    }
    let changed = next != *settings;
    *settings = next;
    Ok(changed)
}

fn apply_diagnostics(
    id: ReferenceId,
    profile: ToolProfile,
    settings: &mut AppSettings,
) -> Result<(), String> {
    let verbose = profile == ToolProfile::Verbose;
    match id {
        ReferenceId::Tinymist => {
            set_path(
                &mut settings.tinymist_configuration,
                &["lint", "enabled"],
                json!(verbose),
            )?;
            // This client sends unsaved edits, not didSave notifications.
            set_path(
                &mut settings.tinymist_configuration,
                &["lint", "when"],
                json!("onType"),
            )?;
        }
        ReferenceId::Tectonic | ReferenceId::TexEngine => {
            settings.tex.show_underfull_box_warnings = verbose;
        }
        ReferenceId::Texlab => {
            settings.tex.texlab_ignored_codes.clear();
            set_path(
                &mut settings.tex.texlab_configuration,
                &["texlab", "diagnostics", "allowedPatterns"],
                json!([]),
            )?;
            set_path(
                &mut settings.tex.texlab_configuration,
                &["texlab", "diagnostics", "ignoredPatterns"],
                texlab_ignored_patterns(profile).clone(),
            )?;
        }
        ReferenceId::Badness => {
            settings.tex.badness_ignored_codes = if verbose {
                Vec::new()
            } else {
                NORMAL_TEX.badness_ignored_codes.clone()
            };
        }
        ReferenceId::Ltex => {
            set_path(
                &mut settings.ltex.configuration,
                &["disabledRules"],
                ltex_disabled_rules(profile).clone(),
            )?;
            set_path(
                &mut settings.ltex.configuration,
                &["enabledRules"],
                json!({}),
            )?;
            set_path(
                &mut settings.ltex.configuration,
                &["additionalRules", "enablePickyRules"],
                json!(verbose),
            )?;
        }
        ReferenceId::Harper => settings.harper_rule_overrides = harper_rules(profile).clone(),
        _ => {}
    }
    Ok(())
}

fn texlab_ignored_patterns(profile: ToolProfile) -> &'static Value {
    match profile {
        ToolProfile::Normal => {
            &NORMAL_TEX.texlab_configuration["texlab"]["diagnostics"]["ignoredPatterns"]
        }
        ToolProfile::Verbose => &EMPTY_ARRAY,
    }
}
fn ltex_disabled_rules(profile: ToolProfile) -> &'static Value {
    match profile {
        ToolProfile::Normal => &NORMAL_LTEX.configuration["disabledRules"],
        ToolProfile::Verbose => &EMPTY_OBJECT,
    }
}
fn harper_rules(profile: ToolProfile) -> &'static crate::writing::RuleOverrides {
    match profile {
        ToolProfile::Normal => &NORMAL_HARPER,
        ToolProfile::Verbose => &VERBOSE_HARPER,
    }
}
fn set_path(root: &mut Value, path: &[&str], value: Value) -> Result<(), String> {
    let (last, parents) = path.split_last().expect("preset path is nonempty");
    let mut current = root;
    for key in parents {
        current = current
            .as_object_mut()
            .ok_or("Configuration must contain objects at preset settings paths")?
            .entry(*key)
            .or_insert_with(|| json!({}));
    }
    current
        .as_object_mut()
        .ok_or("Configuration must contain objects at preset settings paths")?
        .insert((*last).into(), value);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::{ToolMode, WritingLanguage};

    #[test]
    fn every_factory_default_is_normal_without_rewriting_any_settings() {
        assert_eq!(ToolProfile::default(), ToolProfile::Normal);
        let mut settings = AppSettings::default();
        let before = settings.clone();
        for id in ReferenceId::ALL {
            assert!(matches(id, ToolProfile::Normal, &settings), "{id:?}");
            assert_eq!(selected(id, &settings), Some(ToolProfile::Normal));
            assert!(
                !apply(id, ToolProfile::Normal, &mut settings).unwrap(),
                "{id:?}"
            );
        }
        assert_eq!(settings, before);
    }

    #[test]
    fn every_supported_preset_validates_and_roundtrips_to_exact_factory_defaults() {
        for id in ReferenceId::ALL {
            for profile in ToolProfile::ALL {
                let mut settings = AppSettings::default();
                let before = settings.clone();
                let result = apply(id, profile, &mut settings);
                if profile == ToolProfile::Verbose && !supports_verbose(id) {
                    assert!(result.is_err(), "{id:?}");
                    assert_eq!(settings, before);
                    assert!(!matches(id, profile, &settings));
                    continue;
                }
                result.unwrap_or_else(|error| panic!("{id:?}/{profile:?}: {error}"));
                assert!(matches(id, profile, &settings), "{id:?}/{profile:?}");
                assert_eq!(selected(id, &settings), Some(profile));
                assert!(
                    !apply(id, profile, &mut settings).unwrap(),
                    "idempotent {id:?}"
                );
                crate::settings_json::parse(&serde_json::to_string(&settings).unwrap())
                    .unwrap_or_else(|error| panic!("{id:?}/{profile:?}: {error}"));
                apply(id, ToolProfile::Normal, &mut settings).unwrap();
                assert_eq!(
                    settings, before,
                    "{id:?}: Normal must restore exact factory values"
                );
            }
        }
    }

    #[test]
    fn presets_preserve_paths_environment_enablement_dictionary_and_language() {
        let mut settings = AppSettings::default();
        for preference in [
            &mut settings.typst,
            &mut settings.tinymist,
            &mut settings.tex.tectonic,
            &mut settings.tex.texlab,
            &mut settings.tex.badness,
            &mut settings.tex.tex_fmt,
        ] {
            preference.mode = ToolMode::Custom;
            preference.custom_path = "/custom/tool".into();
            preference.command.environment = r#"{"CUSTOM":"keep"}"#.into();
            preference.command.directory = "/custom/workspace".into();
        }
        settings.tex.system_command.environment = r#"{"TEXINPUTS":"keep"}"#.into();
        settings.tex.system_command.directory = "/custom/project".into();
        settings.tex.latexmk.executable = "/custom/latexmk".into();
        settings.tex.bibtex.executable = "/custom/bibtex".into();
        settings.tex.biber.executable = "/custom/biber".into();
        settings.tex.build_engine = crate::tex::settings::BuildEngine::XeLatex;
        settings.tex.build_driver = crate::tex::settings::BuildDriver::Direct;
        settings.tex.build_enabled = false;
        settings.tex.bibliography_enabled = false;
        settings.tex.texlab_enabled = false;
        settings.tex.lint = false;
        settings.tex.formatter = crate::tex::settings::Formatter::Disabled;
        settings.tex.synctex = false;
        settings.tex.only_cached = true;
        settings.english_grammar = true;
        settings.harper_ignore_capitalized_names = true;
        settings.harper_dictionary = "MyCustomWord".into();
        settings.writing_language = WritingLanguage::American;
        settings.ltex.enabled = true;
        settings.ltex.follow_document_language = false;
        settings.ltex.mode = ToolMode::Custom;
        settings.ltex.executable = "/custom/ltex".into();
        settings.ltex.command.environment = r#"{"JAVA_OPTS":"-Xmx1g"}"#.into();
        settings.ltex.command.directory = "/custom/ltex-workspace".into();
        settings.ltex.configuration["language"] = "de-DE".into();
        settings.ltex.configuration["dictionary"] = json!({"de-DE":["Customwort"]});
        settings.ltex.configuration["hiddenFalsePositives"] = json!({"de-DE":["keep"]});
        settings.ltex.configuration["latex"] = json!({"commands":{"\\custom{}":"ignore"}});
        settings.latexdiff.executable = "/custom/latexdiff".into();
        settings.latexdiff.command.environment = r#"{"CUSTOM":"keep"}"#.into();
        settings.latexdiff.command.directory = "/custom/diff-workspace".into();
        settings.latexdiff.flatten = false;
        settings.latexdiff.markup = crate::tex::diff::Markup::Color;
        settings.tinymist_configuration["formatterPrintWidth"] = 88.into();
        settings.tex.badness_configuration["lineWidth"] = 96.into();
        settings.tex.texlab_configuration["texlab"]["diagnosticsDelay"] = 750.into();
        settings.tex.texlab_configuration["texlab"]["forwardSearch"] = json!({"executable":"keep"});
        settings.liquid_threads = 2;
        settings.liquid_cache_directory = "/custom/liquid".into();
        let before = settings.clone();
        for id in ReferenceId::ALL {
            assert!(
                matches(id, ToolProfile::Normal, &settings),
                "{id:?}: unrelated choices are not custom presets"
            );
            if supports_verbose(id) {
                apply(id, ToolProfile::Verbose, &mut settings).unwrap();
                apply(id, ToolProfile::Normal, &mut settings).unwrap();
                assert_eq!(settings, before, "{id:?} changed unrelated settings");
            }
        }
    }

    #[test]
    fn verbose_adds_diagnostics_without_changing_logging_or_build_scheduling() {
        let mut settings = AppSettings::default();
        let before = settings.clone();
        apply_all(ToolProfile::Verbose, &mut settings).unwrap();
        assert_eq!(selected_all(&settings), Some(ToolProfile::Verbose));
        assert_eq!(settings.tinymist_configuration["lint"]["enabled"], true);
        assert_eq!(settings.tinymist_configuration["lint"]["when"], "onType");
        assert!(settings.tex.show_underfull_box_warnings);
        assert!(settings.tex.badness_ignored_codes.is_empty());
        assert_eq!(
            settings.tex.texlab_configuration["texlab"]["diagnostics"]["ignoredPatterns"],
            json!([])
        );
        assert_eq!(
            settings.tex.texlab_configuration["texlab"]["build"]["onSave"],
            false
        );
        assert_eq!(
            settings.tex.texlab_configuration["texlab"]["chktex"]["onEdit"],
            false
        );
        assert_eq!(
            settings.tex.texlab_configuration["texlab"]["latexFormatter"],
            "none"
        );
        assert_eq!(settings.tinymist_configuration["exportPdf"], "never");
        assert_eq!(settings.ltex.configuration["checkFrequency"], "manual");
        assert_eq!(settings.ltex.configuration["completionEnabled"], false);
        assert_eq!(
            settings.ltex.configuration["additionalRules"]["enablePickyRules"],
            true
        );
        assert_eq!(
            settings.ltex.configuration["ltex-ls"],
            before.ltex.configuration["ltex-ls"]
        );
        assert_eq!(settings.ltex.configuration["disabledRules"], json!({}));
        assert_eq!(
            serde_json::to_value(&settings.harper_rule_overrides).unwrap(),
            json!({})
        );
        assert_eq!(settings.tinymist.command, before.tinymist.command);
        assert_eq!(settings.tex.tectonic.command, before.tex.tectonic.command);
        assert_eq!(settings.tex.texlab.command, before.tex.texlab.command);
        assert_eq!(settings.tex.tex_fmt.command, before.tex.tex_fmt.command);
        assert_eq!(settings.tex.latexmk, before.tex.latexmk);
        assert_eq!(settings.tex.bibtex, before.tex.bibtex);
        assert_eq!(settings.tex.biber, before.tex.biber);
        assert_eq!(settings.latexdiff, before.latexdiff);
        assert!(!settings.ltex.enabled);
        assert_eq!(settings.english_grammar, before.english_grammar);
        assert!(!apply_all(ToolProfile::Verbose, &mut settings).unwrap());
        apply_all(ToolProfile::Normal, &mut settings).unwrap();
        assert_eq!(selected_all(&settings), Some(ToolProfile::Normal));
        assert_eq!(settings, before);
    }

    #[test]
    fn process_arguments_and_logging_never_change_diagnostic_selection_or_get_overwritten() {
        let mut settings = AppSettings::default();
        for command in [
            &mut settings.typst.command,
            &mut settings.tinymist.command,
            &mut settings.tex.tectonic.command,
            &mut settings.tex.texlab.command,
            &mut settings.tex.badness.command,
            &mut settings.tex.tex_fmt.command,
            &mut settings.tex.system_command,
            &mut settings.ltex.command,
            &mut settings.latexdiff.command,
        ] {
            command
                .arguments
                .push_str(" --custom-argument 'keep exactly'");
        }
        settings.tex.latexmk.arguments = "-quiet -bibtex-cond -custom".into();
        settings.tex.bibtex.arguments = "-min-crossrefs=5".into();
        settings.tex.biber.arguments = "--debug".into();
        settings.ltex.configuration["ltex-ls"]["logLevel"] = "severe".into();
        let before = settings.clone();
        for id in ReferenceId::ALL {
            assert_eq!(selected(id, &settings), Some(ToolProfile::Normal));
            if supports_verbose(id) {
                apply(id, ToolProfile::Verbose, &mut settings).unwrap();
                assert_eq!(settings.tinymist.command, before.tinymist.command);
                assert_eq!(settings.tex.tectonic.command, before.tex.tectonic.command);
                assert_eq!(settings.tex.texlab.command, before.tex.texlab.command);
                assert_eq!(settings.tex.badness.command, before.tex.badness.command);
                assert_eq!(settings.tex.tex_fmt.command, before.tex.tex_fmt.command);
                assert_eq!(settings.tex.system_command, before.tex.system_command);
                assert_eq!(settings.ltex.command, before.ltex.command);
                assert_eq!(
                    settings.ltex.configuration["ltex-ls"],
                    before.ltex.configuration["ltex-ls"]
                );
                apply(id, ToolProfile::Normal, &mut settings).unwrap();
                assert_eq!(settings, before, "{id:?} overwrote process choices");
            }
        }
        apply_all(ToolProfile::Verbose, &mut settings).unwrap();
        apply_all(ToolProfile::Normal, &mut settings).unwrap();
        assert_eq!(settings, before);
    }

    #[test]
    fn global_profile_is_mixed_for_custom_rules_and_atomic_on_invalid_provider_json() {
        let mut settings = AppSettings::default();
        assert_eq!(selected_all(&settings), Some(ToolProfile::Normal));
        settings
            .tex
            .badness_ignored_codes
            .push("custom-rule".into());
        assert_eq!(selected_all(&settings), None);
        for id in [
            ReferenceId::Tinymist,
            ReferenceId::Texlab,
            ReferenceId::Ltex,
        ] {
            let mut settings = AppSettings::default();
            match id {
                ReferenceId::Tinymist => settings.tinymist_configuration["lint"] = json!(false),
                ReferenceId::Texlab => {
                    settings.tex.texlab_configuration["texlab"]["diagnostics"] = json!(false)
                }
                ReferenceId::Ltex => settings.ltex.configuration["additionalRules"] = json!(false),
                _ => unreachable!(),
            }
            let before = settings.clone();
            assert!(
                apply_all(ToolProfile::Verbose, &mut settings).is_err(),
                "{id:?}"
            );
            assert_eq!(settings, before, "{id:?}: no partial global change");
        }
    }

    #[test]
    fn custom_diagnostic_choices_clear_the_selected_preset_until_explicitly_reset() {
        let mut settings = AppSettings::default();
        settings.tex.texlab_configuration["texlab"]["diagnostics"]["allowedPatterns"] =
            json!(["error"]);
        settings.tex.badness_ignored_codes = vec!["deprecated-command".into()];
        settings.ltex.configuration["disabledRules"] = json!({"en-US":["CUSTOM_RULE"]});
        for id in [ReferenceId::Texlab, ReferenceId::Badness, ReferenceId::Ltex] {
            assert!(
                ToolProfile::ALL
                    .into_iter()
                    .all(|profile| !matches(id, profile, &settings))
            );
            apply(id, ToolProfile::Normal, &mut settings).unwrap();
            assert!(matches(id, ToolProfile::Normal, &settings));
        }
    }

    #[test]
    fn invalid_json_parent_does_not_partially_apply_filters() {
        for id in [ReferenceId::Texlab, ReferenceId::Ltex] {
            let mut settings = AppSettings::default();
            match id {
                ReferenceId::Texlab => {
                    settings.tex.texlab_configuration["texlab"]["diagnostics"] = json!(false)
                }
                ReferenceId::Ltex => settings.ltex.configuration["additionalRules"] = json!(false),
                _ => unreachable!(),
            }
            let before = settings.clone();
            assert!(apply(id, ToolProfile::Verbose, &mut settings).is_err());
            assert_eq!(settings, before, "{id:?}");
        }
    }
}
