//! Semantic coverage for provider-specific grammar and spelling preferences.
use super::*;
use crate::{
    preview::ServiceState,
    toolchain::{ToolKind, ToolOrigin},
};
use egui_kittest::{
    Harness,
    kittest::{NodeT as _, Queryable as _, by},
};

struct WritingForm {
    ui: SettingsUiState,
    settings: AppSettings,
    updates: usize,
    actions: Vec<SettingsAction>,
    repair_component: Option<crate::resource_repair::Component>,
    repair_busy: bool,
    repair_status: Option<String>,
}

fn harness() -> Harness<'static, WritingForm> {
    let fonts = FontCatalog::default();
    let captures = CaptureController::disabled_for_tests();
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
    let tool = |kind, program: &str| ToolResolution {
        bundled_program: None,
        command: Default::default(),
        kind,
        program: program.into(),
        origin: ToolOrigin::Bundled,
        fallback_reason: None,
    };
    let typst = tool(ToolKind::Typst, "typst");
    let tinymist = tool(ToolKind::Tinymist, "tinymist");
    let tex = crate::tex::tools::TexTools::resolve(&crate::tex::settings::TexSettings::default());
    let mut harness = Harness::builder()
        .with_size(Vec2::new(900.0, 1500.0))
        .build_ui_state(
            move |ui, state: &mut WritingForm| {
                let ready = || ServiceState::Ready("ready".into());
                let mut actions = Vec::new();
                SettingsPanel {
                    state: &mut state.ui,
                    settings: &state.settings,
                    pending_settings: None,
                    theme_override: None,
                    snapshot_scene: None,
                    font_catalog: &fonts,
                    font_catalog_scanning: false,
                    font_configuration: &font_configuration,
                    typst_tool: &typst,
                    tinymist_tool: &tinymist,
                    tex_tools: &tex,
                    status: SettingsStatus {
                        backend_label: "PDFium",
                        preview_failure: None,
                        requested_backend: PreviewPreference::Pdfium,
                        interactive_active: false,
                        resource_repair_component: state.repair_component,
                        resource_repair_busy: state.repair_busy,
                        resource_repair_status: state.repair_status.clone(),
                        capabilities: CapabilitySnapshot {
                            editing: ready(),
                            lsp: ready(),
                            interactive_preview: ready(),
                            pdf_generation: ready(),
                            pdf_rendering: ready(),
                            link_extraction: ready(),
                        },
                    },
                    project_root: Path::new("."),
                    captures: &captures,
                    actions: &mut actions,
                }
                .show(ui);
                for action in actions {
                    match action {
                        SettingsAction::Update(settings) => {
                            state.settings = *settings;
                            state.updates += 1;
                        }
                        action => state.actions.push(action),
                    }
                }
            },
            WritingForm {
                ui: SettingsUiState {
                    category: SettingsSection::Grammar,
                    ..Default::default()
                },
                settings: AppSettings::default(),
                updates: 0,
                actions: Vec::new(),
                repair_component: None,
                repair_busy: false,
                repair_status: None,
            },
        );
    theme::configure_test_fonts(&harness.ctx);
    harness.run();
    harness
}

fn open_harper(harness: &mut Harness<'_, WritingForm>) {
    harness.get_by_label("Harper configuration").click();
    harness.run();
}

#[test]
fn dictionary_can_be_typed_edited_and_revisited_without_trimming_input() {
    let mut harness = harness();
    open_harper(&mut harness);
    let field = by()
        .label("Harper dictionary")
        .role(egui::accesskit::Role::MultilineTextInput);
    harness.get(field.clone()).focus();
    harness.run();
    let words = "qzorthal\nÉvariste\n";
    harness.get(field.clone()).type_text(words);
    harness.run();
    assert_eq!(harness.state().settings.harper_dictionary, words);
    assert!(!harness.state().settings.english_grammar);
    assert!(!harness.state().settings.harper_ignore_capitalized_names);

    harness
        .get(by().label("Editor").role(egui::accesskit::Role::Button))
        .click();
    harness.run();
    harness
        .get(
            by().label("Grammar & spelling")
                .role(egui::accesskit::Role::Button),
        )
        .click();
    harness.run();
    assert_eq!(harness.state().settings.harper_dictionary, words);
    harness.get(field.clone()).focus();
    harness.run();
    harness.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::A);
    harness.get(field).type_text("Romito\n");
    harness.run();
    assert_eq!(harness.state().settings.harper_dictionary, "Romito\n");
    let updates = harness.state().updates;
    harness.run_steps(4);
    assert_eq!(harness.state().updates, updates, "idle form must stay idle");
}

#[test]
fn name_filter_toggle_does_not_change_dictionary_language_or_enable_grammar() {
    let mut harness = harness();
    open_harper(&mut harness);
    harness.state_mut().settings.harper_dictionary = "qzorthal\n".into();
    harness.state_mut().settings.writing_language = crate::settings::WritingLanguage::American;
    harness.run();
    harness
        .get_by_label("Harper: ignore capitalized names")
        .click();
    harness.run();
    assert!(harness.state().settings.harper_ignore_capitalized_names);
    assert!(!harness.state().settings.english_grammar);
    assert_eq!(harness.state().settings.harper_dictionary, "qzorthal\n");
    assert_eq!(
        harness.state().settings.writing_language,
        crate::settings::WritingLanguage::American
    );
    harness
        .get_by_label("Harper: ignore capitalized names")
        .click();
    harness.run();
    assert!(!harness.state().settings.harper_ignore_capitalized_names);
    assert_eq!(harness.state().settings.harper_dictionary, "qzorthal\n");
}

#[test]
fn invalid_dictionary_draft_stays_visible_without_replacing_the_valid_dictionary() {
    let mut harness = harness();
    open_harper(&mut harness);
    harness.state_mut().settings.harper_dictionary = "qzorthal\n".into();
    harness.run();
    let field = by()
        .label("Harper dictionary")
        .role(egui::accesskit::Role::MultilineTextInput);
    harness.get(field.clone()).focus();
    harness.run();
    harness.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::A);
    harness.get(field.clone()).type_text("not one word\n");
    harness.run();
    assert_eq!(harness.state().settings.harper_dictionary, "qzorthal\n");
    assert_eq!(harness.state().updates, 0);
    harness.get_by_label(
        "Harper dictionary line 1 must contain one word, without spaces or control characters.",
    );
    assert_eq!(
        harness.state().ui.harper_dictionary_draft,
        Some(("qzorthal\n".into(), "not one word\n".into()))
    );

    // A separate preference may be saved while this invalid text stays parked.
    harness
        .get_by_label("Harper: ignore capitalized names")
        .click();
    harness.run();
    assert!(harness.state().settings.harper_ignore_capitalized_names);
    assert_eq!(harness.state().settings.harper_dictionary, "qzorthal\n");
    harness
        .get(by().label("Editor").role(egui::accesskit::Role::Button))
        .click();
    harness.run();
    harness
        .get(
            by().label("Grammar & spelling")
                .role(egui::accesskit::Role::Button),
        )
        .click();
    harness.run();
    harness.get_by_label(
        "Harper dictionary line 1 must contain one word, without spaces or control characters.",
    );
    harness.get(field.clone()).focus();
    harness.run();
    harness.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::A);
    harness.get(field).type_text("Romito\n");
    harness.run();
    assert_eq!(harness.state().settings.harper_dictionary, "Romito\n");
    assert!(harness.state().ui.harper_dictionary_draft.is_none());
    assert!(harness.state().settings.harper_ignore_capitalized_names);
}

#[test]
fn harper_rule_json_validates_before_apply_and_reset_restores_quiet_defaults() {
    let mut harness = harness();
    open_harper(&mut harness);
    let field = by()
        .label("Harper rule overrides")
        .role(egui::accesskit::Role::MultilineTextInput);
    let original = harness.state().settings.harper_rule_overrides.clone();
    let mut changed = serde_json::to_value(&original).unwrap();
    changed["NoFrenchSpaces"] = serde_json::json!(true);
    harness.get(field.clone()).focus();
    harness.run();
    harness.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::A);
    harness.get(field.clone()).type_text(&changed.to_string());
    harness.run();
    assert_eq!(
        harness.state().settings.harper_rule_overrides,
        original,
        "typing a valid draft must not publish it before Apply"
    );
    harness.get_by_label("Apply Harper rules").click();
    harness.run();
    assert_eq!(
        serde_json::to_value(&harness.state().settings.harper_rule_overrides).unwrap(),
        changed
    );
    assert!(!harness.state().settings.english_grammar);
    assert!(!harness.state().settings.harper_ignore_capitalized_names);
    let applied = harness.state().settings.harper_rule_overrides.clone();
    harness.get(field.clone()).focus();
    harness.run();
    harness.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::A);
    harness.get(field).type_text(r#"{"SpellChek":false}"#);
    harness.run();
    assert!(
        harness
            .get_by_label("Apply Harper rules")
            .accesskit_node()
            .is_disabled()
    );
    assert_eq!(harness.state().settings.harper_rule_overrides, applied);
    harness.get_by_label("Reset Harper rules").click();
    harness.run();
    assert_eq!(harness.state().settings.harper_rule_overrides, original);
    let updates = harness.state().updates;
    harness.run_steps(4);
    assert_eq!(
        harness.state().updates,
        updates,
        "idle rule editor cannot publish repeated updates"
    );
}

#[test]
fn ltex_language_following_requires_apply_and_does_not_change_harper() {
    let mut harness = harness();
    let harper = harness.state().settings.harper_rule_overrides.clone();
    harness
        .get_by_label("LTeX+ setup and configuration")
        .click();
    harness.run();
    assert!(harness.state().settings.ltex.follow_document_language);
    harness.get_by_label("Follow document language").click();
    harness.run();
    assert!(
        harness.state().settings.ltex.follow_document_language,
        "draft language preference waits for Apply"
    );
    harness.get_by_label("Apply LTeX settings").click();
    harness.run();
    assert!(!harness.state().settings.ltex.follow_document_language);
    assert_eq!(harness.state().settings.harper_rule_overrides, harper);
    assert!(!harness.state().settings.english_grammar);
    assert!(!harness.state().settings.ltex.enabled);
}

#[test]
fn ltex_enable_is_independent_of_harper_dictionary_and_language() {
    let mut harness = harness();
    assert!(!harness.state().settings.ltex.enabled);
    let dictionary = harness.state().settings.harper_dictionary.clone();
    let language = harness.state().settings.writing_language;
    harness.get_by_label("LTeX+ spelling and grammar").click();
    harness.run();
    assert!(harness.state().settings.ltex.enabled);
    assert!(!harness.state().settings.english_grammar);
    assert_eq!(harness.state().settings.harper_dictionary, dictionary);
    assert_eq!(harness.state().settings.writing_language, language);
    harness.get_by_label("LTeX+ spelling and grammar").click();
    harness.run();
    assert!(!harness.state().settings.ltex.enabled);
    let updates = harness.state().updates;
    harness.run_steps(4);
    assert_eq!(
        harness.state().updates,
        updates,
        "idle controls must not publish settings"
    );
}

#[test]
fn resource_repair_buttons_emit_explicit_actions_without_changing_preferences() {
    let mut harness = harness();
    let settings = harness.state().settings.clone();
    harness.run_steps(4);
    assert!(harness.state().actions.is_empty());
    assert_eq!(harness.state().updates, 0);

    harness
        .get_by_label("LTeX+ setup and configuration")
        .click();
    harness.run();
    assert!(harness.query_by_label("Download LTeX+").is_none());
    harness.get_by_label("Redownload LTeX+").click();
    harness.run();
    assert!(matches!(
        harness.state().actions.as_slice(),
        [SettingsAction::RedownloadLtex]
    ));
    assert_eq!(harness.state().settings, settings);
    assert_eq!(harness.state().updates, 0);

    harness.state_mut().actions.clear();
    harness.get_by_label("Download Liquid").click();
    harness.run();
    assert!(matches!(
        harness.state().actions.as_slice(),
        [SettingsAction::DownloadLiquid]
    ));
    assert_eq!(harness.state().settings, settings);
    assert_eq!(harness.state().updates, 0);
    harness.state_mut().actions.clear();
    harness.run_steps(4);
    assert!(harness.state().actions.is_empty());
}

#[test]
fn resource_repairs_disable_repeat_actions_and_present_borrowed_status() {
    let mut harness = harness();
    harness.state_mut().settings.ltex.enabled = true;
    harness
        .get_by_label("LTeX+ setup and configuration")
        .click();
    harness.run();
    harness.state_mut().repair_component = Some(crate::resource_repair::Component::Liquid);
    harness.state_mut().repair_busy = true;
    harness.state_mut().repair_status = Some("Downloading Liquid…".into());
    harness.run();
    for label in ["Redownload LTeX+", "Restart LTeX+", "Download Liquid"] {
        assert!(harness.get_by_label(label).accesskit_node().is_disabled());
    }
    assert!(harness.query_all(by().label("Downloading Liquid…")).count() > 0);
    assert!(harness.state().actions.is_empty());
    assert_eq!(harness.state().updates, 0);
    harness.get_by_label("Cancel Liquid download").click();
    harness.run();
    assert!(matches!(
        harness.state().actions.as_slice(),
        [SettingsAction::CancelResourceRepair]
    ));
    harness.state_mut().actions.clear();

    harness.state_mut().repair_busy = false;
    harness.state_mut().repair_status = Some("Liquid downloaded".into());
    harness.run();
    for label in ["Redownload LTeX+", "Restart LTeX+", "Download Liquid"] {
        assert!(!harness.get_by_label(label).accesskit_node().is_disabled());
    }
    assert!(harness.query_all(by().label("Liquid downloaded")).count() > 0);
    assert!(harness.state().actions.is_empty());
    assert_eq!(harness.state().updates, 0);
}

#[test]
fn ltex_custom_executable_is_hidden_for_bundled_mode_and_preserved_when_switching() {
    let mut harness = harness();
    harness
        .get_by_label("LTeX+ setup and configuration")
        .click();
    harness.run();
    assert!(harness.query_by_label("LTeX+ executable").is_none());
    harness.get_by_label("Custom path").click();
    harness.run();
    harness.get_by_label("LTeX+ executable").focus();
    harness.run();
    harness
        .get_by_label("LTeX+ executable")
        .type_text("/tmp/custom-ltex");
    harness.run();
    harness.get_by_label("Apply LTeX settings").click();
    harness.run();
    assert_eq!(
        harness.state().settings.ltex.mode,
        crate::settings::ToolMode::Custom
    );
    assert_eq!(harness.state().settings.ltex.executable, "/tmp/custom-ltex");

    harness.get_by_label("Bundled").click();
    harness.run();
    assert!(harness.query_by_label("LTeX+ executable").is_none());
    harness.get_by_label("Apply LTeX settings").click();
    harness.run();
    assert_eq!(
        harness.state().settings.ltex.mode,
        crate::settings::ToolMode::Bundled
    );
    assert_eq!(harness.state().settings.ltex.executable, "/tmp/custom-ltex");
    harness.get_by_label("Custom path").click();
    harness.run();
    assert_eq!(
        harness.get_by_label("LTeX+ executable").value().as_deref(),
        Some("/tmp/custom-ltex")
    );
    assert_eq!(harness.state().settings.ltex.executable, "/tmp/custom-ltex");
    assert!(!harness.state().settings.ltex.enabled);
}

#[test]
fn resource_repair_status_and_cancel_belong_only_to_the_matching_section() {
    use crate::resource_repair::Component;
    let mut harness = harness();
    harness
        .get_by_label("LTeX+ setup and configuration")
        .click();
    harness.run();
    for (component, status, cancel, other_cancel) in [
        (
            Component::Ltex,
            "LTeX+ downloaded",
            "Cancel LTeX+ download",
            "Cancel Liquid download",
        ),
        (
            Component::Liquid,
            "Liquid downloaded",
            "Cancel Liquid download",
            "Cancel LTeX+ download",
        ),
    ] {
        harness.state_mut().repair_component = Some(component);
        harness.state_mut().repair_busy = true;
        harness.state_mut().repair_status = Some(status.into());
        harness.run();
        assert_eq!(harness.query_all(by().label(status)).count(), 1);
        harness.get_by_label(cancel);
        assert!(harness.query_by_label(other_cancel).is_none());
        let status_y = harness.get_by_label(status).rect().center().y;
        let liquid_y = harness
            .get_by_label(SettingsTarget::LiquidGrammar.label())
            .rect()
            .top();
        if component == Component::Ltex {
            assert!(
                status_y < liquid_y,
                "LTeX status must stay inside LTeX setup"
            );
        } else {
            assert!(
                status_y >= liquid_y,
                "Liquid status must stay inside its section"
            );
        }
        for label in ["Redownload LTeX+", "Download Liquid"] {
            assert!(harness.get_by_label(label).accesskit_node().is_disabled());
        }
        harness.state_mut().repair_busy = false;
        harness.run();
        assert_eq!(harness.query_all(by().label(status)).count(), 1);
        assert!(harness.query_by_label(cancel).is_none());
        assert!(harness.query_by_label(other_cancel).is_none());
        assert!(harness.state().actions.is_empty());
        assert_eq!(harness.state().updates, 0);
    }
}

#[test]
fn grammar_providers_are_visible_together_and_enable_independently() {
    let mut harness = harness();
    let initial = harness.state().settings.clone();
    let harper = by()
        .label("Harper spelling and grammar")
        .role(egui::accesskit::Role::CheckBox);
    let ltex = by()
        .label("LTeX+ spelling and grammar")
        .role(egui::accesskit::Role::CheckBox);
    harness.get(harper.clone());
    harness.get(ltex.clone());
    assert!(harness.query_by_label("Harper language").is_none());
    assert!(harness.query_by_label("Harper dictionary").is_none());
    assert!(harness.query_by_label("LTeX+ executable").is_none());
    assert!(harness.query_by_label("Redownload LTeX+").is_none());

    harness.get(harper.clone()).click();
    harness.run();
    let mut expected = initial.clone();
    expected.english_grammar = true;
    assert_eq!(harness.state().settings, expected);
    harness.get(ltex.clone()).click();
    harness.run();
    expected.ltex.enabled = true;
    assert_eq!(harness.state().settings, expected);
    harness.get(harper).click();
    harness.run();
    expected.english_grammar = false;
    assert_eq!(harness.state().settings, expected);
    harness.get(ltex).click();
    harness.run();
    assert_eq!(harness.state().settings, initial);
    assert_eq!(harness.state().updates, 4);
    assert!(harness.state().actions.is_empty());
}

#[test]
fn grammar_and_writing_categories_preserve_settings_and_separate_controls() {
    let mut harness = harness();
    harness.state_mut().settings.english_grammar = true;
    harness.state_mut().settings.ltex.enabled = true;
    harness.state_mut().settings.harper_dictionary = "Évariste\nRomito\n".into();
    harness.state_mut().settings.writing_language = crate::settings::WritingLanguage::American;
    harness.run();
    let settings = harness.state().settings.clone();
    assert!(
        harness
            .query_by_label(SettingsTarget::AsciiPunctuation.label())
            .is_none()
    );
    assert!(
        harness
            .query_by_label(SettingsTarget::UnicodeWarnings.label())
            .is_none()
    );
    for _ in 0..2 {
        harness
            .get(by().label("Writing").role(egui::accesskit::Role::Button))
            .click();
        harness.run();
        assert_eq!(harness.state().ui.category, SettingsSection::Writing);
        harness.get_by_label(SettingsTarget::AsciiPunctuation.label());
        harness.get_by_label(SettingsTarget::UnicodeWarnings.label());
        for label in [
            "Harper spelling and grammar",
            "Harper configuration",
            "LTeX+ spelling and grammar",
            "LTeX+ setup and configuration",
            SettingsTarget::LiquidGrammar.label(),
            SettingsTarget::TexProseRules.label(),
        ] {
            assert!(
                harness.query_by_label(label).is_none(),
                "{label} must belong to Grammar & spelling"
            );
        }
        harness
            .get(
                by().label("Grammar & spelling")
                    .role(egui::accesskit::Role::Button),
            )
            .click();
        harness.run();
        assert_eq!(harness.state().ui.category, SettingsSection::Grammar);
        harness.get_by_label("Harper spelling and grammar");
        harness.get_by_label("LTeX+ spelling and grammar");
        harness.get_by_label(SettingsTarget::LiquidGrammar.label());
        harness.get_by_label(SettingsTarget::TexProseRules.label());
        assert_eq!(harness.state().settings, settings);
        assert_eq!(harness.state().updates, 0);
        assert!(harness.state().actions.is_empty());
    }
}

#[test]
fn dictionary_search_opens_harper_configuration_and_consumes_the_jump() {
    let mut harness = harness();
    let settings = harness.state().settings.clone();
    harness.state_mut().ui.category = SettingsSection::Writing;
    harness.state_mut().ui.scroll_target = Some(SettingsTarget::HarperDictionary);
    harness.run();
    assert_eq!(harness.state().ui.category, SettingsSection::Grammar);
    assert!(harness.state().ui.scroll_target.is_none());
    let dictionary = by()
        .label("Harper dictionary")
        .role(egui::accesskit::Role::MultilineTextInput);
    harness.get(dictionary.clone());
    harness.get_by_label("Harper language");
    harness.get_by_label("Harper: ignore capitalized names");
    harness.run_steps(4);
    harness.get(dictionary);
    assert!(harness.state().ui.scroll_target.is_none());
    assert_eq!(harness.state().settings, settings);
    assert_eq!(harness.state().updates, 0);
    assert!(harness.state().actions.is_empty());

    harness
        .get(by().label("Writing").role(egui::accesskit::Role::Button))
        .click();
    harness.run();
    assert_eq!(harness.state().ui.category, SettingsSection::Writing);
    harness.run_steps(4);
    assert_eq!(harness.state().ui.category, SettingsSection::Writing);
}

#[test]
fn grammar_search_targets_route_to_the_shared_category_without_saving_preferences() {
    let mut harness = harness();
    let settings = harness.state().settings.clone();
    for target in [
        SettingsTarget::EnglishGrammar,
        SettingsTarget::HarperDictionary,
        SettingsTarget::HarperRules,
        SettingsTarget::HarperCapitalizedNames,
        SettingsTarget::Ltex,
        SettingsTarget::LiquidGrammar,
        SettingsTarget::TexProseRules,
    ] {
        harness.state_mut().ui.category = SettingsSection::Writing;
        harness.state_mut().ui.scroll_target = Some(target);
        harness.run();
        assert_eq!(
            harness.state().ui.category,
            SettingsSection::Grammar,
            "{target:?}"
        );
        assert!(harness.state().ui.scroll_target.is_none(), "{target:?}");
        assert!(harness.query_all(by().label(target.label())).count() > 0);
        assert_eq!(harness.state().settings, settings);
        assert_eq!(harness.state().updates, 0);
        assert!(harness.state().actions.is_empty());
    }
}

#[test]
fn ltex_configuration_reference_uses_the_background_browser_action() {
    let mut harness = harness();
    let original = harness.state().settings.clone();
    harness
        .get_by_label("LTeX+ setup and configuration")
        .click();
    harness.run();
    harness.get_by_label("Configuration reference").click();
    harness.run_steps(4);
    assert!(matches!(harness.state().actions.as_slice(),
        [SettingsAction::OpenDocumentation(url)]
            if url == "https://ltex-plus.github.io/ltex-plus/settings.html"));
    assert_eq!(harness.state().settings, original);
    assert_eq!(harness.state().updates, 0);
    harness.run_steps(4);
    assert_eq!(
        harness.state().actions.len(),
        1,
        "idle frames cannot reopen documentation"
    );
}
