//! Semantic coverage for editable Harper preferences in the real Settings form.
use super::*;
use crate::{
    preview::ServiceState,
    toolchain::{ToolKind, ToolOrigin},
};
use egui_kittest::{
    Harness,
    kittest::{Queryable as _, by},
};

struct WritingForm {
    ui: SettingsUiState,
    settings: AppSettings,
    updates: usize,
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
                    if let SettingsAction::Update(settings) = action {
                        state.settings = *settings;
                        state.updates += 1;
                    }
                }
            },
            WritingForm {
                ui: SettingsUiState {
                    category: SettingsSection::Writing,
                    ..Default::default()
                },
                settings: AppSettings::default(),
                updates: 0,
            },
        );
    theme::configure_test_fonts(&harness.ctx);
    harness.run();
    harness
}

#[test]
fn dictionary_can_be_typed_edited_and_revisited_without_trimming_input() {
    let mut harness = harness();
    let field = by()
        .label("Personal dictionary")
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
        .get(by().label("Writing").role(egui::accesskit::Role::Button))
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
    harness.state_mut().settings.harper_dictionary = "qzorthal\n".into();
    harness.state_mut().settings.writing_language = crate::settings::WritingLanguage::American;
    harness.run();
    harness.get_by_label("Ignore capitalized names").click();
    harness.run();
    assert!(harness.state().settings.harper_ignore_capitalized_names);
    assert!(!harness.state().settings.english_grammar);
    assert_eq!(harness.state().settings.harper_dictionary, "qzorthal\n");
    assert_eq!(
        harness.state().settings.writing_language,
        crate::settings::WritingLanguage::American
    );
    harness.get_by_label("Ignore capitalized names").click();
    harness.run();
    assert!(!harness.state().settings.harper_ignore_capitalized_names);
    assert_eq!(harness.state().settings.harper_dictionary, "qzorthal\n");
}

#[test]
fn invalid_dictionary_draft_stays_visible_without_replacing_the_valid_dictionary() {
    let mut harness = harness();
    harness.state_mut().settings.harper_dictionary = "qzorthal\n".into();
    harness.run();
    let field = by()
        .label("Personal dictionary")
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
    harness.get_by_label("Ignore capitalized names").click();
    harness.run();
    assert!(harness.state().settings.harper_ignore_capitalized_names);
    assert_eq!(harness.state().settings.harper_dictionary, "qzorthal\n");
    harness
        .get(by().label("Editor").role(egui::accesskit::Role::Button))
        .click();
    harness.run();
    harness
        .get(by().label("Writing").role(egui::accesskit::Role::Button))
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
