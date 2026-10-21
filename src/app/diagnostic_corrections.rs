//! Reusable correction actions for offline and language-server diagnostics.
use super::*;
use crate::diagnostics::DiagnosticCorrection;

pub(super) fn show_corrections(
    ui: &mut egui::Ui,
    correction: &DiagnosticCorrection,
    current: Option<DocumentKey>,
) -> Option<usize> {
    ui.set_max_width(440.0);
    let mut selected = None;
    let enabled = current.is_some_and(|key| correction.is_current(key));
    for (index, replacement) in correction.replacements.iter().enumerate() {
        let label = if replacement.is_empty() {
            "Remove".to_owned()
        } else {
            format!("Replace with “{replacement}”")
        };
        if ui
            .add_enabled(enabled, egui::Button::new(label).wrap())
            .clicked()
        {
            selected = Some(index);
        }
    }
    if !correction.replacements.is_empty() {
        if !enabled {
            ui.weak("Waiting for refreshed suggestions");
        }
        ui.separator();
    }
    selected
}

impl EditorApp {
    pub(super) fn apply_diagnostic_correction(
        &mut self,
        diagnostic: &Diagnostic,
        index: usize,
        context: &egui::Context,
    ) -> Result<(), String> {
        if self.tabs.is_empty()
            || !self.document().kind().is_editable()
            || !self.diagnostic_targets_current_document(diagnostic)
        {
            return Err("Open the source document and request fresh suggestions.".into());
        }
        let correction = diagnostic
            .correction
            .as_ref()
            .ok_or("This diagnostic has no suggested correction.")?;
        let replacement = correction
            .replacements
            .get(index)
            .ok_or("This correction is no longer available.")?;
        let bytes =
            correction.checked_byte_range(self.document().source(), self.document().key())?;
        let cursor = CCursorRange::two(
            CCursor::new(correction.range.start),
            CCursor::new(correction.range.end),
        );
        self.document_mut().edit(cursor, |source| {
            source.replace_range(bytes, replacement);
        });
        let end = correction.range.start + replacement.chars().count();
        self.pending_editor_selection = Some(EditorSelection::Focus(correction.range.start..end));
        self.find_bar.search.clear();
        self.mark_edited();
        self.focus_source_editor(context);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui_kittest::{
        Harness,
        kittest::{NodeT as _, Queryable as _},
    };

    fn diagnostic(app: &EditorApp, range: Range<usize>, replacement: &str) -> Diagnostic {
        let original = app
            .document()
            .source()
            .chars()
            .skip(range.start)
            .take(range.len())
            .collect();
        Diagnostic {
            provider: Some("Harper".into()),
            severity: DiagnosticSeverity::Help,
            source: DiagnosticSource::Main,
            location: Some(crate::diagnostics::DiagnosticLocation {
                line: 1,
                column: range.start + 1,
            }),
            message: "Spelling".into(),
            details: Vec::new(),
            correction: Some(DiagnosticCorrection {
                range,
                original,
                rule: Some("SpellCheck".into()),
                category: Some("Spelling".into()),
                replacements: vec![replacement.into()],
                document: Some(app.document().key()),
            }),
        }
    }

    #[test]
    fn corrections_replace_unicode_scalar_span_in_one_undo_transaction() {
        let context = egui::Context::default();
        let root = tempfile::tempdir().unwrap();
        let mut app = EditorApp::dormant_for_tests(&context, root.path().into());
        app.document_mut()
            .replace_unprojected_untitled("é 😀 teh result.");
        let correction = diagnostic(&app, 4..7, "the");
        let before = app.document().key();
        app.apply_diagnostic_correction(&correction, 0, &context)
            .unwrap();
        assert_eq!(app.document().source(), "é 😀 the result.");
        assert_ne!(app.document().key(), before);
        assert!(
            app.apply_diagnostic_correction(&correction, 0, &context)
                .is_err()
        );
        assert_eq!(app.document().source(), "é 😀 the result.");
        let restored = app
            .document_mut()
            .history_step(false, CCursorRange::default())
            .unwrap();
        assert_eq!(restored.cursor.primary.index.0, 7);
        assert_eq!(restored.cursor.secondary.index.0, 4);
        assert!(
            app.document_mut()
                .history_step(false, CCursorRange::default())
                .is_none()
        );
        assert_eq!(app.document().source(), "é 😀 teh result.");
    }

    #[test]
    fn stale_or_other_document_suggestions_never_edit_even_matching_text() {
        let context = egui::Context::default();
        let root = tempfile::tempdir().unwrap();
        let mut app = EditorApp::dormant_for_tests(&context, root.path().into());
        app.document_mut()
            .replace_unprojected_untitled("teh result.");
        let correction = diagnostic(&app, 0..3, "the");
        app.document_mut()
            .edit(CCursorRange::default(), |source| source.push(' '));
        let before = app.document().snapshot();
        assert!(
            app.apply_diagnostic_correction(&correction, 0, &context)
                .is_err()
        );
        assert_eq!(app.document().source(), before.source());
        assert_eq!(app.document().key(), before.key());
        app.document_mut()
            .replace_unprojected_untitled("teh result.");
        assert!(
            app.apply_diagnostic_correction(&correction, 0, &context)
                .is_err()
        );
    }

    #[test]
    fn correction_controls_select_replacements_and_disable_stale_actions() {
        let key = DocumentKey::new(tiptoptyp_core::document::WindowSessionId::new(5), 1, 1);
        let mut correction = DiagnosticCorrection::new(
            0..3,
            "teh".into(),
            Some("SpellCheck".into()),
            None,
            vec!["the".into(), String::new()],
        );
        correction.document = Some(key);
        let mut harness = Harness::builder().build_ui_state(
            |ui, state: &mut (DiagnosticCorrection, Option<DocumentKey>, Option<usize>)| {
                if let Some(index) = show_corrections(ui, &state.0, state.1) {
                    state.2 = Some(index);
                }
            },
            (correction, Some(key), None),
        );
        harness.get_by_label("Replace with “the”").click();
        harness.run();
        assert_eq!(harness.state().2, Some(0));
        harness.state_mut().1 = Some(key.after_edit());
        harness.state_mut().2 = None;
        harness.run();
        assert!(
            harness
                .query_by_label("Waiting for refreshed suggestions")
                .is_some()
        );
        assert!(
            harness
                .get_by_label("Replace with “the”")
                .accesskit_node()
                .is_disabled()
        );
        assert!(
            harness
                .get_by_label("Remove")
                .accesskit_node()
                .is_disabled()
        );
    }
}
