//! Created diffs use the same selected compiler and preview as ordinary TeX files.
use super::*;

impl EditorApp {
    pub(super) fn open_created_latexdiff(&mut self, path: PathBuf, context: &egui::Context) {
        self.close_latexdiff(context);
        if !self.open_workspace_document(path.clone(), context) {
            self.notice = Some(Notice {
                message: format!(
                    "Created {}, but could not open it for PDF compilation",
                    path.display()
                ),
                kind: NoticeKind::Error,
            });
            return;
        }
        let tab = self.tabs.active_id().expect("opened LaTeX diff");
        let source = self.document().path().clone().expect("saved LaTeX diff");
        if !source.starts_with(&self.workspace_root)
            && let Some(directory) = source.parent()
        {
            self.set_tab_workspace(
                tab,
                canonical_or_absolute(&discover_project_root(directory)),
            );
        }
        self.select_preview_tab(tab, context);
        let pdf = source.with_extension("pdf");
        self.finish_pdf_output(pdf.clone(), PdfWriteIntent::CompileNew);
        let queued = self
            .document_workflow
            .pending_export
            .as_ref()
            .is_some_and(|pending| {
                pending.intent == PdfWriteIntent::CompileNew
                    && pending.path == pdf
                    && pending.source.tab == Some(tab)
            });
        if queued {
            self.notice = Some(Notice {
                message: format!("Created {} · Compiling PDF…", source.display()),
                kind: NoticeKind::Info,
            });
        } else if let Some(notice) = &mut self.notice {
            notice.message = format!("Created {}. {}", source.display(), notice.message);
            if self.pdf_output_status() != pdf_output::PdfOutputStatus::Saved {
                let key = self.pdf_output_key();
                let message = self.notice.as_ref().unwrap().message.clone();
                self.pdf_output_record_mut().pdf_output.failed(key, message);
                self.notice.as_mut().unwrap().kind = NoticeKind::Error;
            }
        }
        context.request_repaint();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        compiler::EngineConfig, language_support::BuildEngineKind, tex::settings::BuildEngine,
    };

    const DIFF: &str = "\\documentclass{article}\n\\begin{document}\nChanges.\n\\end{document}\n";

    fn fixture() -> (tempfile::TempDir, EditorApp, egui::Context, PathBuf) {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().canonicalize().unwrap();
        let context = egui::Context::default();
        let mut app = EditorApp::dormant_for_tests(&context, root.clone());
        let old = root.join("original.typ");
        fs::write(&old, "Original source\n").unwrap();
        app.document_mut().replace_loaded_unprojected(
            "Original source\n".into(),
            old,
            DocumentKind::Typst,
            None,
        );
        app.document_mut().edit(CCursorRange::default(), |source| {
            source.push_str("Unsaved original draft\n");
        });
        let created = root.join("changes.tex");
        fs::write(&created, DIFF).unwrap();
        (directory, app, context, created)
    }

    fn accept_pdf(app: &mut EditorApp) {
        app.preview.accept_artifact(
            ArtifactKey::unversioned(app.preview_document_revision()),
            crate::pdf::test_pdf().into(),
        );
        app.preview.status = PreviewStatus::Ready(Duration::ZERO);
        app.complete_pending_export();
    }

    #[test]
    fn created_diff_preserves_the_original_and_queues_one_selected_build_and_preview() {
        let (_directory, mut app, context, created) = fixture();
        let original = app.tabs.active_id().unwrap();
        let source = app.document().source().clone();
        let root = app.workspace_root.clone();
        app.settings.tex.build_engine = BuildEngine::XeLatex;
        app.settings.tex.system_command.arguments = "-interaction=nonstopmode".into();
        app.settings.preview_preference = PreviewPreference::Pdfium;
        app.preview.set_requested_backend(PreviewPreference::Pdfium);
        app.begin_latexdiff(&context);

        app.open_created_latexdiff(created.clone(), &context);

        assert!(app.latexdiff.is_none());
        assert_eq!(app.workspace_root, root);
        assert_eq!(app.tabs.len(), 2);
        assert_eq!(app.document_for_tab(original).unwrap().source(), &source);
        assert!(app.document_for_tab(original).unwrap().is_dirty());
        assert_eq!(app.document().path().as_ref(), Some(&created));
        assert_eq!(app.tabs.preview_id(), app.tabs.active_id());
        assert_eq!(app.preview.requested_backend, PreviewPreference::Pdfium);
        let EngineConfig::Tex(options) = app.configured_build_engine(BuildEngineKind::Tex) else {
            panic!("diff must use the configured TeX compiler");
        };
        assert_eq!(options.engine, BuildEngine::XeLatex);
        assert_eq!(options.command.arguments, "-interaction=nonstopmode");
        let pending = app.document_workflow.pending_export.as_ref().unwrap();
        let owner = pending.source;
        assert_eq!(pending.path, created.with_extension("pdf"));
        assert_eq!(pending.intent, PdfWriteIntent::CompileNew);
        assert_eq!(owner.tab, app.tabs.active_id());
        assert!(
            app.notice
                .as_ref()
                .unwrap()
                .message
                .contains("Compiling PDF")
        );

        // A repeated delivery may not create another source owner or enqueue
        // an independent compiler. The existing source-bound request stays put.
        app.open_created_latexdiff(created, &context);
        assert_eq!(app.tabs.len(), 2);
        assert_eq!(
            app.document_workflow
                .pending_export
                .as_ref()
                .unwrap()
                .source,
            owner
        );
    }

    #[test]
    fn an_external_diff_uses_its_own_build_root_without_switching_the_window_workspace() {
        let (_directory, mut app, context, _) = fixture();
        let external = tempfile::tempdir().unwrap();
        let root = external.path().canonicalize().unwrap();
        let created = root.join("outside.tex");
        fs::write(&created, DIFF).unwrap();
        let original_root = app.workspace_root.clone();

        app.open_created_latexdiff(created, &context);

        assert_eq!(app.workspace_root, original_root);
        assert_eq!(app.tab_preview_root(), root);
        assert_eq!(app.tabs.len(), 2);
        assert!(app.document_workflow.pending_export.is_some());
    }

    #[test]
    fn automatic_pdf_publication_saves_new_output_and_rejects_a_racing_existing_file() {
        for collision in [false, true] {
            let (_directory, mut app, context, created) = fixture();
            app.open_created_latexdiff(created.clone(), &context);
            let destination = created.with_extension("pdf");
            if collision {
                fs::write(&destination, "Another writer's existing PDF").unwrap();
            }

            accept_pdf(&mut app);

            assert_eq!(fs::read_to_string(&created).unwrap(), DIFF);
            assert!(app.document_workflow.pending_export.is_none());
            if collision {
                assert_eq!(
                    fs::read(&destination).unwrap(),
                    b"Another writer's existing PDF"
                );
                assert_eq!(app.pdf_output_status(), pdf_output::PdfOutputStatus::Failed);
                assert!(app.document_workflow.modal().is_some());
            } else {
                assert_eq!(fs::read(&destination).unwrap(), crate::pdf::test_pdf());
                assert_eq!(app.pdf_output_status(), pdf_output::PdfOutputStatus::Saved);
                assert!(app.notice.as_ref().unwrap().message.contains("Compiled"));
            }
        }
    }

    #[test]
    fn compile_failure_and_disabled_builds_keep_the_created_source_and_original_draft() {
        for disabled in [false, true] {
            let (_directory, mut app, context, created) = fixture();
            let original = app.tabs.active_id().unwrap();
            app.settings.tex.build_enabled = !disabled;

            app.open_created_latexdiff(created.clone(), &context);
            if !disabled {
                app.set_compile_error("The selected compiler is unavailable".into());
            }

            assert_eq!(fs::read_to_string(&created).unwrap(), DIFF);
            assert!(!created.with_extension("pdf").exists());
            assert_eq!(app.document().path().as_ref(), Some(&created));
            assert!(app.document_for_tab(original).unwrap().is_dirty());
            assert!(app.document_workflow.pending_export.is_none());
            assert_eq!(app.pdf_output_status(), pdf_output::PdfOutputStatus::Failed);
            assert!(matches!(
                app.notice.as_ref().unwrap().kind,
                NoticeKind::Error
            ));
        }
    }
}
