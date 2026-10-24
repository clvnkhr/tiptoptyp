use super::*;
impl EditorApp {
    pub(super) fn begin_template_picker(&mut self) {
        let language = if self.tabs.is_empty() {
            None
        } else {
            match self.document().kind() {
                DocumentKind::Typst => Some(crate::document_templates::Language::Typst),
                DocumentKind::Tex => Some(crate::document_templates::Language::Tex),
                _ => None,
            }
        };
        self.template_picker = Some(crate::document_templates::initial_selection(
            &self.settings.templates,
            language,
        ));
    }

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
                match show_picker(ui, selected, &self.settings) {
                    Some(PickerAction::Create) => create = true,
                    Some(PickerAction::Cancel) => close = true,
                    None => {}
                }
            },
        );
        if create {
            let template = self.settings.templates[self.template_picker.take().unwrap()].clone();
            let mut source = template.source.clone();
            if template.language == crate::document_templates::Language::Tex {
                let required = crate::tex::imports::Requirements::fragment(&source);
                if let Err(error) = crate::tex::imports::ensure(&mut source, &mut [], &required) {
                    self.show_file_error(error);
                    return;
                }
            }
            self.new_tab(context);
            self.document_mut()
                .replace_untitled_kind(match template.language {
                    crate::document_templates::Language::Typst => DocumentKind::Typst,
                    crate::document_templates::Language::Tex => DocumentKind::Tex,
                });
            let source_text = source;
            self.document_mut().edit(CCursorRange::default(), |source| {
                *source = source_text.clone()
            });
            self.reset_document_services();
            self.mark_edited();
        } else if close {
            self.template_picker = None;
        }
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PickerAction {
    Create,
    Cancel,
}

fn show_picker(
    ui: &mut egui::Ui,
    selected: &mut usize,
    settings: &AppSettings,
) -> Option<PickerAction> {
    let mut action = None;
    egui::Panel::top("template-title")
        .exact_size(ui.spacing().interact_size.y.max(24.0) + 6.0)
        .frame(theme::settings_title_frame(ui.style()))
        .show(ui, |ui| {
            ui.horizontal_centered(|ui| {
                #[cfg(target_os = "macos")]
                theme::reserve_window_controls(ui);
                ui.strong("New from template");
            });
        });
    egui::Panel::bottom("template-actions")
        .frame(theme::settings_content_frame(ui.style()))
        .show(ui, |ui| {
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let create = ui.add_enabled(
                    !settings.templates.is_empty(),
                    egui::Button::new(RichText::new("Create document").strong())
                        .fill(ui.visuals().selection.bg_fill),
                );
                #[cfg(all(feature = "desktop-ui-tests", target_os = "macos"))]
                crate::desktop_test::observe("template.create", &create);
                if create.clicked() {
                    action = Some(PickerAction::Create);
                }
                let cancel = ui.button("Cancel");
                #[cfg(all(feature = "desktop-ui-tests", target_os = "macos"))]
                crate::desktop_test::observe("template.cancel", &cancel);
                if cancel.clicked() {
                    action = Some(PickerAction::Cancel);
                }
            });
        });
    egui::CentralPanel::default()
        .frame(theme::settings_content_frame(ui.style()))
        .show(ui, |ui| {
            let picker = egui::ComboBox::from_label("Template")
                .selected_text(
                    settings
                        .templates
                        .get(*selected)
                        .map_or("No templates", |template| template.name.as_str()),
                )
                .show_ui(ui, |ui| {
                    for (index, template) in settings.templates.iter().enumerate() {
                        ui.selectable_value(selected, index, &template.name);
                    }
                });
            #[cfg(all(feature = "desktop-ui-tests", target_os = "macos"))]
            if let Some(template) = settings.templates.get(*selected) {
                crate::desktop_test::observe(
                    &format!("template.selected.{}", template.language.extension()),
                    &picker.response,
                );
            }
            #[cfg(not(all(feature = "desktop-ui-tests", target_os = "macos")))]
            let _ = picker;
            egui::ScrollArea::both()
                .id_salt("template-preview-scroll")
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    if let Some(template) = settings.templates.get(*selected) {
                        let mut source = template.source.as_str();
                        super::code_editor::editor(
                            ui,
                            "template-preview",
                            &mut source,
                            template.language.extension(),
                            settings,
                            12,
                        );
                    } else {
                        ui.label("Add a template in Settings → Document templates.");
                    }
                });
        });
    action
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui_kittest::{
        Harness,
        kittest::{NodeT, Queryable},
    };

    #[test]
    fn opening_templates_follows_the_active_source_kind_and_keeps_every_language_available() {
        let context = egui::Context::default();
        let mut app = EditorApp::dormant_for_tests(&context, std::env::temp_dir());
        let original = app.settings.templates.clone();
        for kind in [DocumentKind::Tex, DocumentKind::Typst, DocumentKind::Text] {
            app.document_mut().replace_untitled_kind(kind);
            app.begin_template_picker();
            let selected = app.template_picker.unwrap();
            let expected = match kind {
                DocumentKind::Tex => crate::document_templates::Language::Tex,
                _ => crate::document_templates::Language::Typst,
            };
            assert_eq!(
                app.settings.templates[selected].language, expected,
                "{kind:?}"
            );
            assert_eq!(app.settings.templates, original);
        }
        app.settings.templates.clear();
        app.begin_template_picker();
        assert_eq!(app.template_picker, Some(0));
    }

    #[test]
    fn template_actions_remain_labeled_and_visible_below_long_preview() {
        for (width, height) in [(480.0, 320.0), (760.0, 560.0)] {
            let mut settings = AppSettings::default();
            settings.templates[0].source = "// A long template\n".repeat(200);
            let original = settings.templates[0].source.clone();
            let mut fonts_ready = false;
            let mut harness = Harness::builder()
                .with_size(Vec2::new(width, height))
                .build_ui_state(
                    move |ui, state: &mut (usize, AppSettings, Option<PickerAction>)| {
                        if !fonts_ready {
                            theme::configure_test_fonts(ui.ctx());
                            fonts_ready = true;
                            return;
                        }
                        theme::apply_compact_tool_spacing(ui);
                        state.2 = show_picker(ui, &mut state.0, &state.1);
                    },
                    (0, settings, None),
                );
            harness.run();
            for label in ["Create document", "Cancel"] {
                let rect = harness.get_by_label(label).rect();
                assert!(
                    rect.left() >= 0.0 && rect.right() <= width,
                    "{label}: {rect:?}"
                );
                assert!(
                    rect.bottom() <= height && rect.top() > height - 60.0,
                    "{label}: {rect:?}"
                );
            }
            assert_eq!(harness.state().1.templates[0].source, original);
            harness.get_by_label("Create document").click();
            harness.step();
            assert_eq!(harness.state().2, Some(PickerAction::Create));
        }
    }

    #[test]
    fn empty_templates_disable_create_and_keep_cancel_available() {
        let mut settings = AppSettings::default();
        settings.templates.clear();
        let mut harness = Harness::builder()
            .with_size(Vec2::new(480.0, 320.0))
            .build_ui_state(
                |ui, state: &mut (usize, AppSettings, Option<PickerAction>)| {
                    state.2 = show_picker(ui, &mut state.0, &state.1);
                },
                (0, settings, None),
            );
        harness.run();
        assert!(
            harness
                .get_by_label("Create document")
                .accesskit_node()
                .is_disabled()
        );
        harness.get_by_label("Cancel").click();
        harness.step();
        assert_eq!(harness.state().2, Some(PickerAction::Cancel));
    }
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
