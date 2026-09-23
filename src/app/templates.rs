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
const COMFY_START: &str = "// tiptoptyp comfy defaults\n";
const COMFY_END: &str = "// end tiptoptyp comfy defaults\n";
fn comfy_prefix(background: Color32, foreground: Color32) -> String {
    format!(
        "{COMFY_START}#set page(fill: rgb(\"#{:02x}{:02x}{:02x}\"))\n#set text(fill: rgb(\"#{:02x}{:02x}{:02x}\"))\n{COMFY_END}",
        background.r(),
        background.g(),
        background.b(),
        foreground.r(),
        foreground.g(),
        foreground.b()
    )
}
pub(super) fn without_comfy(source: &str) -> Option<&str> {
    source
        .strip_prefix(COMFY_START)?
        .split_once(COMFY_END)
        .map(|(_, body)| body)
}
impl EditorApp {
    pub(super) fn preview_comfy(&self) -> bool {
        self.tab_preview_document()
            .is_some_and(|document| without_comfy(document.source()).is_some())
    }
    pub(super) fn refresh_comfy_theme(&mut self, context: &egui::Context) {
        let Some(body) = without_comfy(self.document().source()) else {
            return;
        };
        let style = context.style_of(context.theme());
        let prefix = comfy_prefix(style.visuals.panel_fill, style.visuals.text_color());
        if self.document().source().starts_with(&prefix) {
            return;
        }
        let updated = prefix + body;
        self.document_mut()
            .edit(CCursorRange::default(), |source| *source = updated);
        self.mark_edited();
    }

    pub(super) fn toggle_comfy(&mut self, context: &egui::Context) {
        if self.document().kind() != DocumentKind::Typst {
            return;
        }
        let source = self.document().source();
        let text = if let Some(body) = without_comfy(source) {
            body.to_owned()
        } else {
            format!(
                "{}{}",
                comfy_prefix(
                    context.style_of(context.theme()).visuals.panel_fill,
                    context.style_of(context.theme()).visuals.text_color()
                ),
                source
            )
        };
        self.document_mut()
            .edit(CCursorRange::default(), |source| *source = text);
        self.preview.source_colors = self.preview_comfy();
        self.mark_edited();
        self.restart_tinymist_preserving_preview();
    }
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
    fn comfy_toggle_keeps_body_and_tex_is_unchanged() {
        let context = egui::Context::default();
        let directory = tempfile::tempdir().unwrap();
        let mut app = EditorApp::dormant_for_tests(&context, directory.path().into());
        app.document_mut().replace_unprojected_untitled("= Body\n");
        app.toggle_comfy(&context);
        assert_eq!(without_comfy(app.document().source()), Some("= Body\n"));
        assert!(app.document().is_dirty());
        app.toggle_comfy(&context);
        assert_eq!(app.document().source(), "= Body\n");
        app.document_mut().replace_untitled_kind(DocumentKind::Tex);
        app.toggle_comfy(&context);
        assert!(app.document().source().is_empty());
    }
    #[test]
    fn comfy_defaults_are_reversible_and_use_theme_rgb() {
        let prefix = comfy_prefix(Color32::BLACK, Color32::WHITE);
        assert!(prefix.contains("rgb(\"#000000\")"));
        assert_eq!(
            without_comfy(&(prefix + "= My document\n")),
            Some("= My document\n")
        );
        assert_eq!(without_comfy("ordinary source"), None);
    }
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
