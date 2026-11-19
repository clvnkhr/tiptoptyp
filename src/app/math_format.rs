//! Formatting requested by a math conversion must remain separate from saving.
use super::*;

impl EditorApp {
    fn math_conversion_uses_tex_formatter(&self) -> bool {
        self.document().kind() == DocumentKind::Tex
            || (self.document().config().is_some() && self.settings.tex.embedded_formatting)
    }

    pub(super) fn queue_math_conversion_format(&mut self, context: &egui::Context) {
        if !self.settings.format_after_math_conversion
            || (self.math_conversion_uses_tex_formatter()
                && self.settings.tex.formatter == crate::tex::settings::Formatter::Disabled)
        {
            return;
        }
        self.formatting = formatting::State::WaitingForMathDocument(self.document().key());
        // One follow-up pass synchronizes TeX even with preview and autosave
        // paused. Service events drive later readiness; no polling loop.
        context.request_repaint();
    }

    pub(super) fn tick_math_conversion_format(&mut self) {
        let Some(key) = self.formatting.math_document() else {
            return;
        };
        if key != self.document().key()
            || !self.settings.format_after_math_conversion
            || self.table_editor.is_some()
        {
            self.formatting.cancel();
            return;
        }
        let ready = if self.math_conversion_uses_tex_formatter() {
            use crate::tex::settings::Formatter;
            if self.settings.tex.formatter == Formatter::Disabled {
                self.formatting.cancel();
                return;
            }
            if self.settings.tex.formatter == Formatter::Badness
                && let Some(error) = self.tex_service.error(crate::tex::Provider::Badness)
            {
                self.activity.format_error = Some(error.to_owned());
                self.formatting.cancel();
                return;
            }
            self.tex_service
                .identity()
                .is_some_and(|snapshot| snapshot.key == key)
                && match self.settings.tex.formatter {
                    Formatter::Badness => self.tex_service.ready(crate::tex::Provider::Badness),
                    // An unavailable binary is reported by the ordinary
                    // formatter request path, once synchronization completes.
                    Formatter::TexFmt => true,
                    Formatter::Disabled => false,
                }
        } else {
            if matches!(
                self.preview.tinymist_state,
                ServiceState::Failed(_) | ServiceState::Unsupported(_)
            ) {
                self.formatting.cancel();
                return;
            }
            self.preview.connection.is_ready()
                && self.tinymist_sync.current_open()
                && self.tinymist_sync.generation().is_some()
                && self.tinymist_sync.current_uri().is_some()
        };
        if ready {
            self.formatting.cancel();
            self.request_format_document();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn convert(app: &mut EditorApp, context: &egui::Context, source: &str, choice: &str) {
        let target = crate::math_conversion::Target::at(source, 2, app.document().kind())
            .unwrap()
            .bind_document(app.document().key());
        app.pending_app_popup_action = Some(AppPopupAction::Editor(EditorMenuAction::ConvertMath(
            target,
            choice.into(),
        )));
        app.execute_pending_app_popup_action(context, None);
    }

    #[test]
    fn converting_math_queues_formatting_for_the_edited_version_without_manual_save() {
        for (kind, source, choice) in [
            (DocumentKind::Typst, "$x$", "Display math"),
            (DocumentKind::Tex, "\\[x\\]", "Inline math \\(...\\)"),
        ] {
            let directory = tempfile::tempdir().unwrap();
            let context = egui::Context::default();
            let mut app = EditorApp::dormant_for_tests(&context, directory.path().into());
            app.document_mut().replace_loaded_unprojected(
                source.into(),
                directory.path().join(if kind == DocumentKind::Tex {
                    "math.tex"
                } else {
                    "math.typ"
                }),
                kind,
                None,
            );
            let before = app.document().key();
            convert(&mut app, &context, source, choice);
            assert_ne!(app.document().key(), before);
            assert_eq!(app.formatting.math_document(), Some(app.document().key()));
            assert!(!app.formatting.saves_after_reply());
        }
    }

    #[test]
    fn conversion_menu_cannot_mutate_another_document_with_the_same_formula() {
        let directory = tempfile::tempdir().unwrap();
        let context = egui::Context::default();
        let mut app = EditorApp::dormant_for_tests(&context, directory.path().into());
        app.document_mut().replace_loaded_unprojected(
            "$x$".into(),
            directory.path().join("a.typ"),
            DocumentKind::Typst,
            None,
        );
        let target = crate::math_conversion::Target::at("$x$", 2, DocumentKind::Typst)
            .unwrap()
            .bind_document(app.document().key());
        app.document_mut().replace_loaded_unprojected(
            "$x$".into(),
            directory.path().join("b.typ"),
            DocumentKind::Typst,
            None,
        );
        app.pending_app_popup_action = Some(AppPopupAction::Editor(EditorMenuAction::ConvertMath(
            target,
            "Display math".into(),
        )));
        app.execute_pending_app_popup_action(&context, None);
        assert_eq!(app.document().source(), "$x$");
        assert!(app.formatting.math_document().is_none());
    }

    #[test]
    fn tex_punctuation_conversion_is_one_undo_and_rejects_a_stale_menu() {
        let directory = tempfile::tempdir().unwrap();
        let context = egui::Context::default();
        let mut app = EditorApp::dormant_for_tests(&context, directory.path().into());
        app.snapshot_scene = None;
        app.settings.format_after_math_conversion = false;
        let source = "Before \\(α\\), after.";
        app.document_mut().replace_loaded_unprojected(
            source.into(),
            directory.path().join("math.tex"),
            DocumentKind::Tex,
            None,
        );
        context
            .run_ui(egui::RawInput::default(), |ui| app.show_editor(ui))
            .drop_without_applying_deltas();
        let target = crate::math_conversion::Target::at(source, 10, DocumentKind::Tex)
            .unwrap()
            .bind_document(app.document().key());
        app.pending_app_popup_action = Some(AppPopupAction::Editor(EditorMenuAction::ConvertMath(
            target.clone(),
            r"Display math \[...\]".into(),
        )));
        app.execute_pending_app_popup_action(&context, None);
        assert_eq!(app.document().source(), "Before \\[α,\\] after.");
        app.undo_editor(&context, false);
        assert_eq!(app.document().source(), source);

        // The punctuation shares the formula's captured range and owner.
        app.document_mut().edit(CCursorRange::default(), |source| {
            *source = "Before \\(α\\); after.".into();
        });
        app.pending_app_popup_action = Some(AppPopupAction::Editor(EditorMenuAction::ConvertMath(
            target,
            r"Display math \[...\]".into(),
        )));
        app.execute_pending_app_popup_action(&context, None);
        assert_eq!(app.document().source(), "Before \\(α\\); after.");
        assert!(
            app.notice
                .as_ref()
                .unwrap()
                .message
                .contains("formula changed")
        );
    }

    #[test]
    fn disabled_automatic_formatting_and_disabled_tex_formatter_still_convert() {
        for (kind, automatic, formatter) in [
            (
                DocumentKind::Typst,
                false,
                crate::tex::settings::Formatter::Badness,
            ),
            (
                DocumentKind::Tex,
                true,
                crate::tex::settings::Formatter::Disabled,
            ),
        ] {
            let directory = tempfile::tempdir().unwrap();
            let context = egui::Context::default();
            let mut app = EditorApp::dormant_for_tests(&context, directory.path().into());
            let source = if kind == DocumentKind::Tex {
                "\\[x\\]"
            } else {
                "$x$"
            };
            app.document_mut().replace_loaded_unprojected(
                source.into(),
                directory.path().join("math"),
                kind,
                None,
            );
            app.settings.format_after_math_conversion = automatic;
            app.settings.tex.formatter = formatter;
            convert(
                &mut app,
                &context,
                source,
                if kind == DocumentKind::Tex {
                    "Inline math \\(...\\)"
                } else {
                    "Display math"
                },
            );
            assert_ne!(app.document().source(), source);
            assert!(app.formatting.math_document().is_none());
            assert!(app.formatting.request_key().is_none());
        }
    }

    #[test]
    fn queued_conversion_format_waits_once_and_retires_after_an_edit_or_document_switch() {
        let directory = tempfile::tempdir().unwrap();
        let context = egui::Context::default();
        let mut app = EditorApp::dormant_for_tests(&context, directory.path().into());
        app.document_mut().replace_loaded_unprojected(
            "$x$".into(),
            directory.path().join("a.typ"),
            DocumentKind::Typst,
            None,
        );
        convert(&mut app, &context, "$x$", "Display math");
        let key = app.document().key();
        for _ in 0..8 {
            app.tick_math_conversion_format();
            assert_eq!(app.formatting.math_document(), Some(key));
            assert!(app.formatting.request_key().is_none());
        }
        app.document_mut()
            .edit(CCursorRange::default(), |source| source.push_str(" edit"));
        app.mark_edited();
        assert!(app.formatting.math_document().is_none());
        app.formatting = formatting::State::WaitingForMathDocument(app.document().key());
        app.document_mut().replace_loaded_unprojected(
            "$x$".into(),
            directory.path().join("b.typ"),
            DocumentKind::Typst,
            None,
        );
        app.tick_math_conversion_format();
        assert!(app.formatting.math_document().is_none());
    }

    #[test]
    fn conversion_formatter_reply_edits_without_saving_and_rejects_stale_replies() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("a.typ");
        std::fs::write(&path, "$x$").unwrap();
        let context = egui::Context::default();
        let mut app = EditorApp::dormant_for_tests(&context, directory.path().into());
        app.settings.auto_save = false;
        app.document_mut().replace_loaded_unprojected(
            "$x$".into(),
            path.clone(),
            DocumentKind::Typst,
            None,
        );
        convert(&mut app, &context, "$x$", "Display math");
        let key = app.document().key();
        let uri = url::Url::from_file_path(&path).unwrap().to_string();
        app.tinymist_sync
            .fixture_session(Generation(7), Some(uri.clone()), None);
        let edits = vec![LspTextEdit {
            range: LspRange {
                start: lsp_position_at_scalar(app.document().source(), ScalarOffset::new(0)),
                end: lsp_position_at_scalar(
                    app.document().source(),
                    ScalarOffset::new(app.document().source().chars().count()),
                ),
            },
            new_text: "$\n  x\n$".into(),
        }];
        // Delivery from the existing formatter uses the converted version,
        // with no explicit-save intent carried across this user action.
        app.formatting.cancel();
        app.formatting.request(key, formatting::Intent::Edit);
        app.receive_formatted_document(
            &context,
            Generation(7),
            &uri,
            revision_as_i32(key.revision),
            app.formatting.request_token().unwrap_or(0),
            edits.clone(),
        );
        assert_eq!(app.document().source(), "$\n  x\n$");
        assert!(app.local_save.pending().is_none());
        assert!(!app.local_save.is_running());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "$x$");
        app.formatting.request(key, formatting::Intent::Edit);
        app.receive_formatted_document(
            &context,
            Generation(7),
            &uri,
            revision_as_i32(key.revision),
            app.formatting.request_token().unwrap_or(0),
            edits,
        );
        assert_eq!(app.document().source(), "$\n  x\n$");
        assert!(app.local_save.pending().is_none());
    }

    #[test]
    fn math_formatting_is_a_visible_validated_json_setting() {
        let mut json = serde_json::to_value(AppSettings::default()).unwrap();
        assert_eq!(json["format_after_math_conversion"], true);
        json.as_object_mut()
            .unwrap()
            .remove("format_after_math_conversion");
        assert!(
            crate::settings_json::parse(&json.to_string())
                .unwrap()
                .format_after_math_conversion
        );
        json["format_after_math_conversion"] = serde_json::json!(false);
        let settings = crate::settings_json::parse(&json.to_string()).unwrap();
        assert_eq!(
            serde_json::to_value(settings).unwrap()["format_after_math_conversion"],
            false
        );
        json["format_after_math_conversion"] = serde_json::json!("sometimes");
        assert!(
            crate::settings_json::parse(&json.to_string())
                .unwrap_err()
                .contains("format_after_math_conversion")
        );
    }
}
