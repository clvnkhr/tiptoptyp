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
                egui::CentralPanel::default().show(ui, |ui| {
                    ui.horizontal(|ui| {
                        #[cfg(target_os = "macos")]
                        theme::reserve_window_controls(ui);
                        ui.heading("Import legacy text as UTF-8");
                    });
                    ui.label(dialog.path.display().to_string());
                    ui.label("Choose the encoding and inspect the preview. Mixed encodings may look valid but decode incorrectly. The original file will remain unchanged.");
                    let before = dialog.encoding;
                    egui::ComboBox::from_label("Source encoding")
                        .selected_text(crate::text_encoding::ENCODINGS[dialog.encoding].0)
                        .show_ui(ui, |ui| {
                            for (index, (name, _)) in crate::text_encoding::ENCODINGS.iter().enumerate() {
                                ui.selectable_value(&mut dialog.encoding, index, *name);
                            }
                        });
                    if before != dialog.encoding {
                        dialog.preview = crate::text_encoding::decode(&dialog.bytes, crate::text_encoding::ENCODINGS[dialog.encoding].1);
                    }
                    ui.horizontal(|ui| {
                        import = crate::app::icons::action_button_enabled(ui, dialog.preview.is_ok(), "Open converted copy").clicked();
                        close |= crate::app::icons::action_button(ui, "Cancel").clicked();
                    });
                    ui.separator();
                    match &dialog.preview {
                        Ok(text) => {
                            egui::ScrollArea::both().show(ui, |ui| {
                                let mut text = text.as_str();
                                ui.add(egui::TextEdit::multiline(&mut text).font(egui::TextStyle::Monospace).desired_width(f32::INFINITY));
                            });
                        }
                        Err(error) => { ui.colored_label(ui.visuals().error_fg_color, error); }
                    }
                });
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

#[cfg(test)]
mod tests {
    use super::*;
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
