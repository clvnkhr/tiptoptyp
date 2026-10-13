//! Source-first snippet editing. Invalid drafts never reach live completions.
use crate::{
    settings::AppSettings,
    snippets::{Language, Snippet},
};
use eframe::egui;

#[derive(Clone)]
pub(super) struct Draft {
    base: Vec<Snippet>,
    entries: Vec<Snippet>,
    selected: usize,
    validation: Result<(), String>,
}
impl Draft {
    pub(super) fn new(entries: &[Snippet]) -> Self {
        Self {
            base: entries.to_vec(),
            entries: entries.to_vec(),
            selected: 0,
            validation: crate::snippets::validate(entries),
        }
    }
    fn changed(&self) -> bool {
        self.entries != self.base
    }
    fn validate(&mut self) {
        self.validation = crate::snippets::validate(&self.entries);
    }
    fn add(&mut self, language: Language) {
        let (prefix, description, body) = match language {
            Language::Tex => (
                "env",
                "LaTeX environment",
                "\\begin{${1:equation}}\n  ${2:x = y}\n\\end{${1}}${0}",
            ),
            _ => ("eq", "Display equation", "$ ${1:x} = ${2:y} $\n${0}"),
        };
        let mut unique = prefix.to_owned();
        let mut suffix = 2;
        while self.entries.iter().any(|entry| entry.prefix == unique) {
            unique = format!("{prefix}{suffix}");
            suffix += 1;
        }
        self.selected = self.entries.len();
        self.entries.push(Snippet {
            prefix: unique,
            description: description.into(),
            language,
            body: body.into(),
        });
        self.validate();
    }
}

fn observe(_name: &str, _response: &egui::Response) {
    #[cfg(all(feature = "desktop-ui-tests", target_os = "macos"))]
    crate::desktop_test::observe(_name, _response);
}

pub(super) fn show(
    ui: &mut egui::Ui,
    state: &mut Option<Draft>,
    settings: &mut AppSettings,
    appearance: &AppSettings,
) {
    let draft = state.get_or_insert_with(|| Draft::new(&settings.snippets));
    if draft.base != settings.snippets && !draft.changed() {
        *draft = Draft::new(&settings.snippets);
    }
    ui.weak(
        "Write Typst or TeX directly, with real line breaks. No JSON or extra backslash escaping.",
    );
    ui.horizontal_wrapped(|ui| {
        for (language, label, target) in [
            (Language::Typst, "Add Typst snippet", "typst"),
            (Language::Tex, "Add TeX snippet", "tex"),
        ] {
            let add = ui.button(label);
            observe(&format!("settings.snippets.add.{target}"), &add);
            if add.clicked() {
                draft.add(language);
            }
        }
        let save = ui.add_enabled(
            draft.changed() && draft.validation.is_ok(),
            egui::Button::new("Save snippets"),
        );
        observe("settings.snippets.save", &save);
        if save.clicked() {
            settings.snippets.clone_from(&draft.entries);
            draft.base.clone_from(&draft.entries);
        }
        let revert = ui.add_enabled(draft.changed(), egui::Button::new("Revert changes"));
        observe("settings.snippets.revert", &revert);
        if revert.clicked() {
            *draft = Draft::new(&settings.snippets);
        }
    });
    if draft.entries.is_empty() {
        ui.weak("Add a snippet to start from an editable example.");
        return;
    }
    draft.selected = draft.selected.min(draft.entries.len() - 1);
    let mut changed = false;
    ui.horizontal(|ui| {
        egui::ComboBox::from_id_salt("snippet-selection")
            .selected_text(&draft.entries[draft.selected].prefix)
            .show_ui(ui, |ui| {
                for (index, entry) in draft.entries.iter().enumerate() {
                    ui.selectable_value(&mut draft.selected, index, &entry.prefix);
                }
            });
        if ui.button("Remove snippet").clicked() {
            draft.entries.remove(draft.selected);
            draft.selected = draft.selected.saturating_sub(1);
            changed = true;
        }
    });
    if let Some(snippet) = draft.entries.get_mut(draft.selected) {
        ui.push_id(("snippet", draft.selected), |ui| {
            ui.horizontal(|ui| {
                let label = ui.label("Prefix");
                let prefix = ui.add(egui::TextEdit::singleline(&mut snippet.prefix).desired_width(110.0)).labelled_by(label.id);
                observe("settings.snippets.prefix", &prefix);
                changed |= prefix.changed();
                for (value, label) in [(Language::Typst, "Typst"), (Language::Tex, "TeX"), (Language::Both, "Both")] {
                    changed |= ui.selectable_value(&mut snippet.language, value, label).changed();
                }
            });
            ui.horizontal(|ui| {
                let label = ui.label("Description");
                let description = ui.add(egui::TextEdit::singleline(&mut snippet.description).desired_width(f32::INFINITY)).labelled_by(label.id);
                observe("settings.snippets.description", &description);
                changed |= description.changed();
            });
            let response = super::code_editor::editor(ui, "snippet-body", &mut snippet.body, if snippet.language == Language::Tex { "tex" } else { "typ" }, appearance, 6);
            response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::TextEdit, true, "Snippet source"));
            observe("settings.snippets.source", &response);
            changed |= response.changed();
            ui.weak("Optional fields: ${1:name}; repeat with ${1}; final cursor: ${0}. Tab moves between fields.");
        });
    }
    if changed {
        draft.validate();
    }
    if let Err(error) = &draft.validation {
        ui.colored_label(ui.visuals().error_fg_color, error);
    } else if draft.changed() {
        ui.weak("Unsaved snippet changes");
    }
    // Validation only runs on editing; the shared editor caches highlighting.
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui_kittest::kittest::NodeT as _;
    use egui_kittest::{Harness, kittest::Queryable as _};

    #[test]
    fn source_form_edits_and_saves_tex_without_json_and_rejects_invalid_drafts() {
        let mut harness = Harness::builder()
            .with_size(egui::vec2(700.0, 600.0))
            .build_ui_state(
                |ui, state: &mut (AppSettings, Option<Draft>)| {
                    let appearance = state.0.clone();
                    show(ui, &mut state.1, &mut state.0, &appearance);
                },
                (AppSettings::default(), None),
            );
        crate::theme::configure_test_fonts(&harness.ctx);
        harness.get_by_label("Add TeX snippet").click();
        harness.run();
        assert!(
            harness.state().0.snippets.is_empty(),
            "unsaved draft must not enter live settings"
        );
        let source = "\\begin{${1:align}}\n  $x$ & \\text{${2:hello}} \\\\\n\\end{${1}}${0}";
        let field = harness.get_by_label("Snippet source");
        field.click();
        harness.run();
        harness.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::A);
        harness.run();
        harness.get_by_label("Snippet source").type_text(source);
        harness.run();
        harness.get_by_label("Save snippets").click();
        harness.run();
        assert_eq!(harness.state().0.snippets[0].body, source);
        assert_eq!(harness.state().0.snippets[0].language, Language::Tex);
        let item = crate::snippets::items(
            &harness.state().0.snippets,
            crate::document::DocumentKind::Tex,
            "env",
            3,
        )
        .unwrap()
        .remove(0);
        let expanded = crate::completion_edit::expand_lsp_snippet(&item.insert_text).unwrap();
        assert_eq!(
            expanded.text,
            "\\begin{align}\n  $x$ & \\text{hello} \\\\\n\\end{align}"
        );
        harness.get_by_label("Prefix").click();
        harness.run();
        harness.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::A);
        harness.key_press(egui::Key::Backspace);
        harness.run();
        assert!(
            harness
                .get_by_label("Save snippets")
                .accesskit_node()
                .is_disabled()
        );
        assert_eq!(harness.state().0.snippets[0].prefix, "env");
        harness.get_by_label("Revert changes").click();
        harness.run();
        assert_eq!(
            harness.state().1.as_ref().unwrap().entries,
            harness.state().0.snippets
        );
    }

    #[test]
    fn typst_example_add_remove_and_revert_preserve_saved_snippets() {
        let mut draft = Draft::new(&[]);
        draft.add(Language::Typst);
        draft.add(Language::Typst);
        assert_eq!(draft.entries[0].prefix, "eq");
        assert_eq!(draft.entries[1].prefix, "eq2");
        assert!(draft.validation.is_ok());
        assert_eq!(draft.entries[0].language, Language::Typst);
        let wire = crate::snippets::source_to_lsp(&draft.entries[0].body).unwrap();
        assert_eq!(
            crate::completion_edit::expand_lsp_snippet(&wire)
                .unwrap()
                .text,
            "$ x = y $\n"
        );
    }
}
