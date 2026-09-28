use super::*;
impl EditorApp {
    pub(super) fn show_template_picker(&mut self, context: &egui::Context) {
        let Some(selected) = &mut self.template_picker else {
            return;
        };
        let templates = &self.settings.templates;
        *selected = (*selected).min(templates.len().saturating_sub(1));
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
                        .selected_text(
                            templates
                                .get(*selected)
                                .map_or("No templates", |template| template.name.as_str()),
                        )
                        .show_ui(ui, |ui| {
                            for (index, template) in templates.iter().enumerate() {
                                ui.selectable_value(selected, index, &template.name);
                            }
                        });
                    ui.horizontal(|ui| {
                        create = crate::app::icons::action_button_enabled(
                            ui,
                            !templates.is_empty(),
                            "Create document",
                        )
                        .clicked();
                        close |= crate::app::icons::action_button(ui, "Cancel").clicked();
                    });
                    egui::ScrollArea::both().show(ui, |ui| {
                        if let Some(template) = templates.get(*selected) {
                            let mut source = template.source.as_str();
                            super::settings_code::editor(
                                ui,
                                "template-preview",
                                &mut source,
                                template.language.extension(),
                                &self.settings,
                                20,
                            );
                        } else {
                            ui.label("Add a template in Settings → Document templates.");
                        }
                    });
                });
            },
        );
        if create {
            let template = self.settings.templates[self.template_picker.take().unwrap()].clone();
            self.new_tab(context);
            self.document_mut()
                .replace_untitled_kind(match template.language {
                    crate::document_templates::Language::Typst => DocumentKind::Typst,
                    crate::document_templates::Language::Tex => DocumentKind::Tex,
                });
            self.document_mut().edit(CCursorRange::default(), |source| {
                *source = template.source.clone()
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
    #[test]
    fn typst_templates_parse_without_syntax_errors() {
        for template in crate::document_templates::defaults()
            .iter()
            .filter(|t| t.language == crate::document_templates::Language::Typst)
        {
            assert!(
                typst_syntax::Source::detached(template.source.clone())
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
