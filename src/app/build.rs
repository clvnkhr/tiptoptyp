//! Canonical-source to build-service adapter; compiler formats stay in engine modules.
use super::*;
use crate::{
    compiler::{
        CompileEvent, CompileInput, CompileRequest, EngineConfig, TexOptions, TypstOptions,
    },
    language_support::BuildEngineKind,
};

impl EditorApp {
    pub(super) fn request_compile(&mut self) {
        self.compile_deadline = None;
        if !self.preview_processing_enabled() || !self.may_run_compilation() {
            return;
        }
        let preview_path = self.preview_document_path();
        let source = match self.preview_document_source() {
            Ok(source) => source,
            Err(error) => {
                self.set_compile_error(error);
                return;
            }
        };
        let source_dir = self.preview_source_directory();
        let display_name = preview_path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| "Untitled.typ".to_owned());
        let Some(engine) = self.preview_language_support().build else {
            return;
        };
        let Some(language) = self.preview_document_kind().typesetting_language() else {
            return;
        };
        if engine == BuildEngineKind::Tex && !self.settings.tex.build_enabled {
            self.set_compile_error("TeX builds are disabled in Settings".into());
            return;
        }
        let engine = match engine {
            BuildEngineKind::Typst => EngineConfig::Typst(TypstOptions {
                command: self.typst_tool.command.clone().into(),
                executable: self.typst_tool.program.clone(),
                font_paths: self.font_catalog.workspace_directories().to_vec(),
            }),
            BuildEngineKind::Tex => EngineConfig::Tex(TexOptions {
                command: if self.settings.tex.build_engine
                    == crate::tex::settings::BuildEngine::Tectonic
                {
                    self.tex_tools.tectonic.command.clone()
                } else {
                    Default::default()
                }
                .into(),
                engine: self.settings.tex.build_engine,
                executable: if self.settings.tex.build_engine
                    == crate::tex::settings::BuildEngine::Tectonic
                {
                    self.tex_tools.tectonic.program.clone()
                } else {
                    self.tex_tools
                        .distribution(self.settings.tex.build_engine)
                        .unwrap_or_else(|| Path::new(self.settings.tex.build_engine.executable()))
                        .to_owned()
                },
                only_cached: self.settings.tex.only_cached,
            }),
        };
        let request = CompileRequest {
            revision: self.preview_document_revision(),
            input: CompileInput {
                language,
                source,
                project_root: self.tab_preview_root().to_owned(),
                source_dir,
                display_name,
            },
            engine,
        };

        match self.compiler.request(request) {
            Ok(()) => {
                self.activity.build = crate::activity::Activity::Running;
                self.preview.status = PreviewStatus::Compiling;
            }
            Err(error) => self.set_compile_error(error),
        }
    }

    pub(super) fn preview_source_directory(&self) -> PathBuf {
        if self.tab_preview_document().is_some_and(|document| {
            LanguageSupport::for_document(document.kind())
                .build
                .is_some()
                && document.path().is_none()
        }) {
            return self.tab_preview_root().into();
        }
        if self.current_is_preview_document() && self.document().path().is_none() {
            // The preview path is only a virtual identity for an untitled
            // document. Relative imports and private mirrors need a real root.
            self.current_directory()
                .unwrap_or_else(|| self.project_root())
        } else {
            self.preview_document_path()
                .parent()
                .map(Path::to_path_buf)
                .unwrap_or_else(|| self.project_root())
        }
    }

    pub(super) fn tick_compile(&mut self, context: &egui::Context) {
        if !self.preview_processing_enabled() || !self.may_run_compilation() {
            self.compile_deadline = None;
            return;
        }
        let Some(deadline) = self.compile_deadline else {
            return;
        };
        let now = Instant::now();
        if deadline <= now {
            self.request_compile();
        } else {
            context.request_repaint_after(deadline - now);
        }
    }

    pub(super) fn receive_compile_results(&mut self) {
        let _span = crate::performance::span("compile.receive");
        while let Some(result) = self.compiler.try_recv() {
            if !self.compiler.is_latest(&result)
                || !self.preview_processing_enabled()
                || !self.may_run_compilation()
                || result.revision != self.preview_document_revision()
            {
                continue;
            }

            match result.event {
                CompileEvent::Started => {
                    // Keep the previous artifact and pages until their
                    // independently versioned replacements arrive.
                    self.preview.status = PreviewStatus::Compiling;
                }
                CompileEvent::Failed(report) => self.set_compile_failure(report),
                CompileEvent::Artifact(artifact) => {
                    self.synctex.artifact =
                        artifact.synctex.map(|map| super::synctex::VersionedMap {
                            path: self.preview_document_path(),
                            key: artifact.key,
                            map,
                            pdf: artifact.pdf.clone(),
                        });
                    self.preview.accept_artifact(artifact.key, artifact.pdf);
                    self.set_diagnostics(artifact.diagnostics);
                    self.activity.build = crate::activity::Activity::Idle;
                    self.preview.status = PreviewStatus::Ready(result.elapsed);
                    self.complete_pending_export();
                    if self.settings.preview_follow_edits
                        && self.preview_visible()
                        && self.tex_tools.synctex.is_some()
                        && self.current_is_preview_document()
                        && self.document().kind() == DocumentKind::Tex
                        && let Some(caret) = &self.last_editor_caret
                        && caret.key == self.document().key()
                    {
                        self.jump_source_to_preview_with_mode(caret.char_index, true);
                    }
                }
            }
        }
    }

    pub(super) fn set_compile_error(&mut self, error: String) {
        self.set_compile_failure(DiagnosticReport::error(error));
    }

    pub(super) fn set_compile_failure(&mut self, report: DiagnosticReport) {
        self.activity.build = crate::activity::Activity::Failed(report.raw.clone());
        self.preview.content.invalidate();
        self.preview.status = PreviewStatus::Error;
        self.set_diagnostics(report);
    }

    pub(super) fn set_diagnostics(&mut self, report: DiagnosticReport) {
        let DiagnosticReport {
            raw,
            mut diagnostics,
        } = report;
        if self.document().config().is_some()
            && let Ok(snapshot) = self.document().canonical_snapshot()
        {
            for diagnostic in &mut diagnostics {
                if self.diagnostic_targets_current_document(diagnostic) {
                    diagnostic.location = diagnostic.location.and_then(|location| {
                        let cursor = char_index_at_line_column(
                            snapshot.source(),
                            location.line,
                            location.column,
                        );
                        let cursor = snapshot.editor_scalar_cursor(ScalarOffset::new(cursor))?;
                        let (line, column) =
                            line_column_at_char(snapshot.editor_source(), cursor.get());
                        Some(crate::diagnostics::DiagnosticLocation { line, column })
                    });
                }
            }
        }
        self.preview.set_diagnostics(raw, diagnostics);
    }
}
