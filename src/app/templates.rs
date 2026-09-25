use super::*;
struct Template {
    name: &'static str,
    kind: DocumentKind,
    source: &'static str,
}
const TEMPLATES: &[Template] = &[
    Template {
        name: "Typst — article",
        kind: DocumentKind::Typst,
        source: include_str!("../../manual-tests/templates/article.typ"),
    },
    Template {
        name: "Typst — letter",
        kind: DocumentKind::Typst,
        source: include_str!("../../manual-tests/templates/letter.typ"),
    },
    Template {
        name: "Typst — notes",
        kind: DocumentKind::Typst,
        source: include_str!("../../manual-tests/templates/notes.typ"),
    },
    Template {
        name: "LaTeX — article",
        kind: DocumentKind::Tex,
        source: include_str!("../../manual-tests/templates/article.tex"),
    },
    Template {
        name: "LaTeX — letter",
        kind: DocumentKind::Tex,
        source: include_str!("../../manual-tests/templates/letter.tex"),
    },
];
impl EditorApp {
    pub(super) fn show_template_picker(&mut self, context: &egui::Context) {
        let Some(selected) = &mut self.template_picker else {
            return;
        };
        let mut close = false;
        let mut create = false;
        let appearance = context.theme();
        ChildViewHost::show(
            context,
            &self.captures.clone(),
            ChildViewSpec::persistent(
                "tiptoptyp-templates",
                "New from template",
                [760.0, 560.0],
                [480.0, 320.0],
                "templates",
            ),
            appearance,
            &context.style_of(appearance),
            |ui, input| {
                close |= input.close_requested || input.escape_pressed;
                egui::CentralPanel::default().show(ui, |ui| {
                    ui.horizontal(|ui| {
                        #[cfg(target_os = "macos")]
                        theme::reserve_window_controls(ui);
                        ui.heading("New from template");
                    });
                    egui::ComboBox::from_label("Template")
                        .selected_text(TEMPLATES[*selected].name)
                        .show_ui(ui, |ui| {
                            for (index, template) in TEMPLATES.iter().enumerate() {
                                ui.selectable_value(selected, index, template.name);
                            }
                        });
                    ui.horizontal(|ui| {
                        create = crate::app::icons::action_button(ui, "Create document").clicked();
                        close |= crate::app::icons::action_button(ui, "Cancel").clicked();
                    });
                    egui::ScrollArea::both().show(ui, |ui| {
                        ui.monospace(TEMPLATES[*selected].source);
                    });
                });
            },
        );
        if create {
            let template = &TEMPLATES[self.template_picker.take().unwrap()];
            self.new_tab(context);
            self.document_mut().replace_untitled_kind(template.kind);
            self.document_mut().edit(CCursorRange::default(), |source| {
                *source = template.source.into()
            });
            self.reset_document_services();
            self.mark_edited();
        } else if close {
            self.template_picker = None;
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn typst_templates_parse_without_syntax_errors() {
        for template in TEMPLATES.iter().filter(|t| t.kind == DocumentKind::Typst) {
            assert!(
                typst_syntax::Source::detached(template.source)
                    .root()
                    .errors_and_warnings()
                    .0
                    .is_empty(),
                "{}",
                template.name
            );
        }
    }
}
