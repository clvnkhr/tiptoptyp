//! PDF disk output is separate from preview rendering and private build caches.
use super::pdf_output::PdfOutputStatus;
use super::*;

impl EditorApp {
    pub(super) fn pdf_output_source(&self) -> PdfSource {
        let tab = if self.source_preview_available() && self.tabs.uses_designated_preview() {
            self.tabs.preview_id()
        } else {
            self.tabs.active_id()
        };
        PdfSource {
            tab,
            epoch: self.tabs.pdf_record(tab).document.epoch(),
        }
    }

    pub(super) fn pdf_output_record(&self) -> &tabs::TabRecord {
        self.tabs.pdf_record(self.pdf_output_source().tab)
    }

    pub(super) fn pdf_output_record_mut(&mut self) -> &mut tabs::TabRecord {
        let tab = self.pdf_output_source().tab;
        self.tabs.pdf_record_mut(tab)
    }

    pub(super) fn pdf_output_key(&self) -> DocumentKey {
        self.pdf_output_record().document.key()
    }

    pub(super) fn cancel_pdf_output_if_source_changed(&mut self) {
        let source = self.pdf_output_source();
        if self
            .document_workflow
            .pending_export
            .as_ref()
            .is_some_and(|pending| pending.source != source)
            && let Some(pending) = self.document_workflow.pending_export.take()
        {
            self.notice = Some(Notice {
                message: format!(
                    "Queued {} canceled because its source changed",
                    pending.intent.dialog_title()
                ),
                kind: NoticeKind::Info,
            });
        }
    }

    pub(super) fn pdf_output_status(&self) -> PdfOutputStatus {
        let artifact = if self.source_preview_available() {
            &self.preview
        } else {
            &self.asset_preview
        };
        self.pdf_output_record().pdf_output.status(
            self.pdf_output_key(),
            artifact.content.artifact_key(),
            self.document_workflow
                .pending_export
                .as_ref()
                .is_some_and(|pending| pending.source == self.pdf_output_source()),
        )
    }

    pub(super) fn pdf_output_tooltip(&self, shortcuts: &ShortcutBindings) -> String {
        let key = self.pdf_output_key();
        let output = &self.pdf_output_record().pdf_output;
        let detail = match self.pdf_output_status() {
            PdfOutputStatus::Unverified => "No PDF saved by this window for this document yet. An existing file has not been checked.".to_owned(),
            PdfOutputStatus::Saved => format!("PDF saved: {}", output.path(key).expect("saved PDF path").display()),
            PdfOutputStatus::Stale => format!("PDF needs updating: {}", output.path(key).expect("stale PDF path").display()),
            PdfOutputStatus::Building => format!("Creating PDF: {}", self.document_workflow.pending_export.as_ref().expect("pending PDF").path.display()),
            PdfOutputStatus::Failed => format!("PDF creation failed: {}", output.error(key).unwrap_or("unknown error")),
        };
        let destination = default_compile_pdf_path(
            None,
            self.pdf_output_record().document.kind(),
            self.pdf_output_record().document.path().as_deref(),
        )
        .map_or_else(
            || "Choose where to save the PDF.".to_owned(),
            |path| format!("Save to {}.", path.display()),
        );
        format!(
            "Save PDF · {}\n{detail}\n{destination}\nLive preview updates do not save this file. Status reflects writes made by this window.",
            shortcuts
                .display(ShortcutAction::Compile)
                .unwrap_or_else(|| "unbound".into())
        )
    }

    pub(super) fn show_pdf_output_button(
        &mut self,
        ui: &mut egui::Ui,
        frame: Option<&eframe::Frame>,
        shortcuts: &ShortcutBindings,
    ) {
        let status = self.pdf_output_status();
        let palette = theme::palette(ui.ctx());
        let (icon, color) = match status {
            PdfOutputStatus::Unverified => (UiIcon::Save, ui.visuals().text_color()),
            PdfOutputStatus::Saved => (UiIcon::Check, palette.success),
            PdfOutputStatus::Stale => (UiIcon::Save, palette.warning),
            PdfOutputStatus::Building => (UiIcon::Compile, palette.warning),
            PdfOutputStatus::Failed => (UiIcon::Warning, palette.error),
        };
        let response = ui
            .scope(|ui| {
                let widgets = &mut ui.visuals_mut().widgets;
                for visuals in [
                    &mut widgets.inactive,
                    &mut widgets.hovered,
                    &mut widgets.active,
                    &mut widgets.noninteractive,
                ] {
                    visuals.fg_stroke.color = color;
                }
                icons::toolbar_button(
                    ui,
                    self.source_preview_available() && status != PdfOutputStatus::Building,
                    false,
                    icon,
                    "PDF",
                    crate::settings::ToolbarStyle::TextAndIcons,
                    status == PdfOutputStatus::Building,
                )
            })
            .inner;
        #[cfg(all(feature = "desktop-ui-tests", target_os = "macos"))]
        crate::desktop_test::observe("build.pdf", &response);
        if native_hover_text(response, self.pdf_output_tooltip(shortcuts)).clicked() {
            self.compile_pdf(frame);
        }
    }

    pub(super) fn refresh_preview(&mut self) {
        if self.interactive_preview_requested() {
            self.restart_tinymist_preserving_preview();
        } else {
            self.schedule_compile_now();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui_kittest::{Harness, kittest::Queryable as _};

    #[test]
    fn preview_artifact_is_not_a_saved_pdf_and_pdf_button_confirms_the_disk_write() {
        let directory = tempfile::tempdir().unwrap();
        let context = egui::Context::default();
        let mut app = EditorApp::dormant_for_tests(&context, directory.path().into());
        let source = directory.path().join("paper.typ");
        app.document_mut().replace_loaded_unprojected(
            "Hello".into(),
            source.clone(),
            DocumentKind::Typst,
            None,
        );
        let pdf: Arc<[u8]> = crate::pdf::test_pdf().into();
        app.preview.accept_artifact(
            ArtifactKey::unversioned(app.preview_document_revision()),
            pdf.clone(),
        );
        app.preview.status = PreviewStatus::Ready(Duration::ZERO);
        app.compile_deadline = None;
        assert_eq!(app.pdf_output_status(), PdfOutputStatus::Unverified);
        assert!(!source.with_extension("pdf").exists());
        let mut harness = Harness::builder()
            .with_size(Vec2::new(1400.0, 80.0))
            .build_ui_state(|ui, app: &mut EditorApp| app.show_toolbar(ui, None), app);
        harness.run_steps(3);
        let button_rect = harness.get_by_label("PDF").rect();
        harness.get_by_label("PDF").click();
        harness.run_steps(3);
        assert_eq!(
            fs::read(source.with_extension("pdf")).unwrap(),
            pdf.as_ref()
        );
        assert_eq!(harness.state().pdf_output_status(), PdfOutputStatus::Saved);
        assert!(
            harness
                .state()
                .pdf_output_tooltip(&harness.state().settings.effective_shortcuts())
                .contains("PDF saved:")
        );
        harness
            .state_mut()
            .document_mut()
            .edit(CCursorRange::default(), |source| source.push('!'));
        harness.run_steps(3);
        assert_eq!(harness.state().pdf_output_status(), PdfOutputStatus::Stale);
        assert_eq!(
            fs::read(source.with_extension("pdf")).unwrap(),
            pdf.as_ref()
        );
        assert_eq!(
            harness.get_by_label("PDF").rect(),
            button_rect,
            "state changes must not move the toolbar"
        );
    }

    #[test]
    fn failed_build_clears_queued_output_and_retry_can_succeed() {
        let directory = tempfile::tempdir().unwrap();
        let context = egui::Context::default();
        let mut app = EditorApp::dormant_for_tests(&context, directory.path().into());
        let destination = directory.path().join("paper.pdf");
        app.document_mut().replace_loaded_unprojected(
            "Hello".into(),
            directory.path().join("paper.typ"),
            DocumentKind::Typst,
            None,
        );
        app.finish_pdf_output(destination.clone(), PdfWriteIntent::Compile);
        assert_eq!(app.pdf_output_status(), PdfOutputStatus::Building);
        app.set_compile_error("bad source".into());
        assert_eq!(app.pdf_output_status(), PdfOutputStatus::Failed);
        assert!(app.document_workflow.pending_export.is_none());
        assert!(!destination.exists());
        app.preview.accept_artifact(
            ArtifactKey::unversioned(app.preview_document_revision()),
            crate::pdf::test_pdf().into(),
        );
        app.preview.status = PreviewStatus::Ready(Duration::ZERO);
        app.compile_deadline = None;
        app.finish_pdf_output(destination.clone(), PdfWriteIntent::Compile);
        assert_eq!(app.pdf_output_status(), PdfOutputStatus::Saved);
        assert!(destination.exists());
    }
}
