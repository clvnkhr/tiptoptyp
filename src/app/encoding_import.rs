use super::*;

pub(super) struct EncodingImport {
    path: PathBuf,
    bytes: Vec<u8>,
    encoding: usize,
    preview: Result<String, String>,
}
impl EncodingImport {
    pub(super) fn new(path: PathBuf, bytes: Vec<u8>) -> Self {
        let preview = crate::text_encoding::decode(&bytes, crate::text_encoding::ENCODINGS[0].1);
        Self {
            path,
            bytes,
            encoding: 0,
            preview,
        }
    }
}
impl EditorApp {
    pub(super) fn show_encoding_import(&mut self, context: &egui::Context) {
        let Some(dialog) = &mut self.encoding_import else {
            return;
        };
        let mut close = false;
        let mut import = false;
        let captures = self.captures.clone();
        let appearance = context.theme();
        ChildViewHost::show(
            context,
            &captures,
            ChildViewSpec::persistent(
                "tiptoptyp-encoding-import",
                "Import legacy text",
                [760.0, 560.0],
                [480.0, 320.0],
                "encoding-import",
            ),
            appearance,
            &context.style_of(appearance),
            |ui, input| {
                close |= input.close_requested || input.escape_pressed;
                match show_import(ui, dialog, &self.settings) {
                    Some(ImportAction::Open) => import = true,
                    Some(ImportAction::Cancel) => close = true,
                    None => {}
                }
            },
        );
        if import {
            let dialog = self.encoding_import.take().expect("open import");
            if let Ok(source) = dialog.preview {
                let kind = if dialog
                    .path
                    .extension()
                    .is_some_and(|e| e.eq_ignore_ascii_case("tex"))
                {
                    DocumentKind::Tex
                } else if dialog
                    .path
                    .extension()
                    .is_some_and(|e| e.eq_ignore_ascii_case("typ"))
                {
                    DocumentKind::Typst
                } else {
                    DocumentKind::Text
                };
                self.new_tab(context);
                self.document_mut().replace_untitled_kind(kind);
                self.document_mut()
                    .edit(CCursorRange::default(), |text| *text = source);
                self.reset_document_services();
                self.mark_edited();
                self.notice = Some(Notice {
                    message: "Imported an unsaved UTF-8 copy. Save As to choose its destination."
                        .into(),
                    kind: NoticeKind::Success,
                });
            }
        } else if close {
            self.encoding_import = None;
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ImportAction {
    Open,
    Cancel,
}

fn show_import(
    ui: &mut egui::Ui,
    dialog: &mut EncodingImport,
    settings: &AppSettings,
) -> Option<ImportAction> {
    let mut action = None;
    egui::Panel::top("encoding-import-title")
        .exact_size(ui.spacing().interact_size.y.max(24.0) + 6.0)
        .frame(theme::settings_title_frame(ui.style()))
        .show(ui, |ui| {
            ui.horizontal_centered(|ui| {
                #[cfg(target_os = "macos")]
                theme::reserve_window_controls(ui);
                ui.strong("Import legacy text");
            });
        });
    egui::Panel::bottom("encoding-import-actions")
        .frame(theme::settings_content_frame(ui.style()))
        .show(ui, |ui| {
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if ui
                    .add_enabled(
                        dialog.preview.is_ok(),
                        egui::Button::new(RichText::new("Open converted copy").strong())
                            .fill(ui.visuals().selection.bg_fill),
                    )
                    .clicked()
                {
                    action = Some(ImportAction::Open);
                }
                if ui.button("Cancel").clicked() {
                    action = Some(ImportAction::Cancel);
                }
            });
        });
    egui::CentralPanel::default()
        .frame(theme::settings_content_frame(ui.style()))
        .show(ui, |ui| {
            let path = dialog.path.display().to_string();
            let glyph_width = ui.text_style_height(&egui::TextStyle::Body) * 0.6;
            let max_chars = (ui.available_width() / glyph_width).max(1.0) as usize;
            ui.add(
                egui::Label::new(tail_elide(&path, max_chars))
                    .truncate()
                    .show_tooltip_when_elided(false),
            );
            let before = dialog.encoding;
            egui::ComboBox::from_label("Source encoding")
                .selected_text(crate::text_encoding::ENCODINGS[dialog.encoding].0)
                .show_ui(ui, |ui| {
                    for (index, (name, _)) in crate::text_encoding::ENCODINGS.iter().enumerate() {
                        ui.selectable_value(&mut dialog.encoding, index, *name);
                    }
                });
            if before != dialog.encoding {
                dialog.preview = crate::text_encoding::decode(
                    &dialog.bytes,
                    crate::text_encoding::ENCODINGS[dialog.encoding].1,
                );
            }
            egui::ScrollArea::both()
                .id_salt("encoding-import-preview-scroll")
                .auto_shrink([false, false])
                .show(ui, |ui| match &dialog.preview {
                    Ok(text) => {
                        let mut source = text.as_str();
                        let extension = dialog
                            .path
                            .extension()
                            .and_then(|ext| ext.to_str())
                            .unwrap_or("txt");
                        super::code_editor::editor(
                            ui,
                            "encoding-import-preview",
                            &mut source,
                            extension,
                            settings,
                            12,
                        );
                    }
                    Err(error) => {
                        ui.colored_label(ui.visuals().error_fg_color, error);
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
    fn converted_copy_actions_stay_visible_and_invalid_preview_cannot_open() {
        let settings = AppSettings::default();
        let source = "Legacy source preview\n".repeat(200);
        let dialog =
            EncodingImport::new(PathBuf::from("/tmp/paper.tex"), source.as_bytes().to_vec());
        let mut fonts_ready = false;
        let mut harness = Harness::builder()
            .with_size(Vec2::new(480.0, 320.0))
            .build_ui_state(
                move |ui, state: &mut (EncodingImport, Option<ImportAction>)| {
                    if !fonts_ready {
                        theme::configure_test_fonts(ui.ctx());
                        fonts_ready = true;
                        return;
                    }
                    theme::apply_compact_tool_spacing(ui);
                    state.1 = show_import(ui, &mut state.0, &settings);
                },
                (dialog, None),
            );
        harness.run();
        let initial = harness.get_by_label("Open converted copy").rect();
        assert!(
            initial.top() > 260.0 && initial.bottom() <= 320.0,
            "{initial:?}"
        );
        harness.get_by_label("Open converted copy").click();
        harness.step();
        assert_eq!(harness.state().1, Some(ImportAction::Open));
        assert_eq!(harness.state().0.preview.as_deref(), Ok(source.as_str()));
        harness.state_mut().0.preview = Err("Invalid source bytes".repeat(200));
        harness.run();
        let open = harness.get_by_label("Open converted copy");
        assert!(open.accesskit_node().is_disabled());
        assert_eq!(open.rect(), initial);
        harness.get_by_label("Cancel").click();
        harness.step();
        assert_eq!(harness.state().1, Some(ImportAction::Cancel));
    }
    #[test]
    fn invalid_utf8_opens_import_without_mutating_original_or_current_document() {
        let context = egui::Context::default();
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("legacy.tex");
        let bytes = b"price \x96 value";
        fs::write(&path, bytes).unwrap();
        let mut app = EditorApp::dormant_for_tests(&context, directory.path().into());
        app.document_mut()
            .replace_unprojected_untitled("Keep my source");
        let key = app.document().key();
        assert!(!app.load_path(path.clone()));
        assert!(app.encoding_import.is_some());
        assert_eq!(app.document().key(), key);
        assert_eq!(app.document().source(), "Keep my source");
        assert_eq!(fs::read(path).unwrap(), bytes);
    }
}
