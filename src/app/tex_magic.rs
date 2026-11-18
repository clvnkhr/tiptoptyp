//! Document-local magic state. Parsing is revision-bound; file reads remain on
//! the compiler worker, and resolved build data never changes saved settings.
use super::*;
use crate::{compiler::TexBuildInfo, tex::magic::Spellcheck};

#[derive(Default)]
pub(super) struct State {
    local: Option<(DocumentKey, bool)>,
    pub(super) build: Option<(DocumentKey, TexBuildInfo)>,
    dependencies: Option<Dependencies>,
}

struct Dependency {
    tab: u64,
    path: PathBuf,
    source: tiptoptyp_core::document::DocumentSnapshot,
}

/// Source snapshots retain their Arc identity across tab activation, which
/// deliberately rebases document epochs and revisions without changing text.
struct Dependencies {
    root: PathBuf,
    sources: Vec<Dependency>,
}

impl EditorApp {
    fn tex_dependency_documents(&self) -> impl Iterator<Item = (u64, &DocumentSession, &Path)> {
        let root = self.tab_preview_root();
        self.tabs.ids().filter_map(move |tab| {
            let document = self.document_for_tab(tab)?;
            let path = document.path().as_deref()?;
            (document.kind() == DocumentKind::Tex && path.starts_with(root))
                .then_some((tab, document, path))
        })
    }

    pub(super) fn capture_tex_dependencies(&mut self, overlays: bool) {
        self.tex_magic.dependencies = overlays.then(|| Dependencies {
            root: self.tab_preview_root().to_owned(),
            sources: self
                .tex_dependency_documents()
                .map(|(tab, document, path)| Dependency {
                    tab,
                    path: path.to_owned(),
                    source: document.snapshot(),
                })
                .collect(),
        });
    }

    pub(super) fn tex_dependencies_current(&self) -> bool {
        self.tex_magic
            .dependencies
            .as_ref()
            .is_none_or(|dependencies| {
                dependencies.root == self.tab_preview_root()
                    && self.tex_dependency_documents().count() == dependencies.sources.len()
                    && dependencies.sources.iter().all(|dependency| {
                        self.document_for_tab(dependency.tab)
                            .is_some_and(|document| {
                                let current = document.snapshot();
                                document.kind() == DocumentKind::Tex
                                    && document.path().as_deref() == Some(dependency.path.as_path())
                                    && std::ptr::eq(
                                        current.source().as_ptr(),
                                        dependency.source.source().as_ptr(),
                                    )
                                    && current.source().len() == dependency.source.source().len()
                            })
                    })
            })
    }

    pub(super) fn invalidate_tex_build_inputs(&mut self) {
        let _ = self.compiler.pause(self.preview_document_revision());
        self.tex_magic.dependencies = None;
        self.tex_magic.build = None;
        self.preview.content.invalidate();
        self.pdf_output_record_mut().pdf_output.invalidate();
    }

    /// Check only at result or PDF admission boundaries. No dependency walk is
    /// added to the toolbar's per-frame resolved-engine lookup.
    pub(super) fn reject_stale_tex_dependencies(&mut self) -> bool {
        if self.preview_document_kind() != DocumentKind::Tex || self.tex_dependencies_current() {
            return false;
        }
        self.invalidate_tex_build_inputs();
        self.schedule_compile_now();
        true
    }

    pub(super) fn tex_edit_affects_preview(&self) -> bool {
        self.preview_document_kind() == DocumentKind::Tex
            && self.document().kind() == DocumentKind::Tex
            && (self.current_is_preview_document()
                || self
                    .document()
                    .path()
                    .as_deref()
                    .is_some_and(|path| path.starts_with(self.tab_preview_root())))
    }

    pub(super) fn refresh_tex_magic(&mut self) {
        if self.tabs.is_empty() || self.document().kind() != DocumentKind::Tex {
            self.tex_magic.local = None;
            return;
        }
        let key = self.document().key();
        if self.tex_magic.local.is_some_and(|cached| cached.0 == key) {
            return;
        }
        let disabled = crate::tex::magic::parse(self.document().source())
            .is_ok_and(|directives| directives.spellcheck == Some(Spellcheck::Disabled));
        self.tex_magic.local = Some((key, disabled));
    }

    pub(super) fn tex_spellcheck_disabled(&self) -> bool {
        self.document().kind() == DocumentKind::Tex
            && self.tex_magic.local == Some((self.document().key(), true))
    }

    pub(super) fn tex_magic_overlays(&self) -> Vec<(PathBuf, String)> {
        (0..self.tabs.len())
            .filter_map(|index| self.tabs.id_at(index))
            .filter_map(|id| self.document_for_tab(id))
            .filter(|document| document.kind() == DocumentKind::Tex && document.is_dirty())
            .filter_map(|document| Some((document.path().clone()?, document.source().clone())))
            .collect()
    }

    fn tex_preview_key(&self) -> DocumentKey {
        self.tab_preview_document()
            .map_or_else(|| self.document().key(), |document| document.key())
    }

    pub(super) fn current_tex_build(&self) -> Option<&TexBuildInfo> {
        self.tex_magic
            .build
            .as_ref()
            .filter(|(key, _)| {
                self.preview_document_kind() == DocumentKind::Tex && *key == self.tex_preview_key()
            })
            .map(|(_, info)| info)
    }

    pub(super) fn accept_tex_build_info(&mut self, info: TexBuildInfo) {
        let key = self.tex_preview_key();
        let source = self.pdf_output_source();
        // Redirect only the automatic adjacent Compile PDF destination. An
        // explicit Export destination or an unsaved document's chosen path wins.
        if let Some(pending) = &mut self.document_workflow.pending_export
            && pending.source == source
            && pending.intent == PdfWriteIntent::Compile
            && pending.automatic_destination
        {
            pending.path = info.entry.with_extension("pdf");
        }
        self.tex_magic.build = Some((key, info));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tex::settings::BuildEngine;

    fn root_and_pinned_chapter(context: &egui::Context, root: &Path) -> EditorApp {
        let root = root.canonicalize().unwrap();
        let mut app = EditorApp::dormant_for_tests(context, root.clone());
        app.snapshot_scene = None;
        app.settings.tex.texlab_enabled = false;
        app.settings.tex.lint = false;
        app.settings.ltex.enabled = false;
        app.document_mut().replace_loaded_unprojected(
            "% !TeX root = main.tex\nChapter".into(),
            root.join("chapter.tex"),
            DocumentKind::Tex,
            None,
        );
        app.lifecycle = DocumentLifecycle::Active;
        app.new_tab(context);
        app.document_mut().replace_loaded_unprojected(
            "Root document".into(),
            root.join("main.tex"),
            DocumentKind::Tex,
            None,
        );
        assert!(!app.current_is_preview_document());
        app.preview.accept_artifact(
            ArtifactKey::unversioned(app.preview_document_revision()),
            crate::pdf::test_pdf().into(),
        );
        app.preview.status = PreviewStatus::Ready(Duration::ZERO);
        app.compile_deadline = None;
        app.accept_tex_build_info(TexBuildInfo {
            entry: root.join("main.tex"),
            engine: BuildEngine::XeLatex,
        });
        app
    }

    #[test]
    fn unsaved_tex_root_edit_invalidates_the_pinned_chapter_artifact_and_rebuilds() {
        let context = egui::Context::default();
        let directory = tempfile::tempdir().unwrap();
        let mut app = root_and_pinned_chapter(&context, directory.path());
        let preview = app.preview_document_revision();
        app.document_mut()
            .edit(CCursorRange::default(), |source| source.push_str(" edited"));
        app.mark_edited();
        assert_eq!(app.preview_document_revision(), preview);
        assert!(
            app.compile_deadline.is_some(),
            "the root overlay needs a fresh build"
        );
        assert!(
            app.preview.content.artifact_key().is_none(),
            "old root bytes cannot be exported"
        );
        assert!(
            app.current_tex_build().is_none(),
            "old resolved root options are stale"
        );
    }

    #[test]
    fn dependency_snapshots_survive_activation_but_reject_programmatic_source_changes() {
        let context = egui::Context::default();
        let directory = tempfile::tempdir().unwrap();
        let mut app = root_and_pinned_chapter(&context, directory.path());
        let active = app.tabs.active_id().unwrap();
        let chapter = app.tabs.id_at(0).unwrap();
        app.capture_tex_dependencies(true);
        assert!(app.tex_dependencies_current());
        app.activate_tab(chapter, &context);
        assert!(
            app.tex_dependencies_current(),
            "activation must preserve source identity"
        );
        app.activate_tab(active, &context);
        assert!(app.tex_dependencies_current());
        app.document_mut().edit(CCursorRange::default(), |source| {
            source.push_str(" changed")
        });
        assert!(!app.tex_dependencies_current());
        assert!(app.reject_stale_tex_dependencies());
        assert!(app.preview.content.artifact_key().is_none());
        assert!(app.compile_deadline.is_some());
    }

    #[test]
    fn local_spellcheck_cache_changes_only_with_document_identity_and_never_settings() {
        let context = egui::Context::default();
        let directory = tempfile::tempdir().unwrap();
        let mut app = EditorApp::dormant_for_tests(&context, directory.path().into());
        app.document_mut().replace_loaded_unprojected(
            "% !TeX spellcheck = none\nText".into(),
            directory.path().join("main.tex"),
            DocumentKind::Tex,
            None,
        );
        let settings = serde_json::to_value(&app.settings).unwrap();
        app.refresh_tex_magic();
        assert!(app.tex_spellcheck_disabled());
        let cached = app.tex_magic.local;
        for _ in 0..100 {
            app.refresh_tex_magic();
        }
        assert_eq!(app.tex_magic.local, cached);
        app.document_mut().edit(CCursorRange::default(), |source| {
            *source = "% !TeX spellcheck = en-GB\nText".into()
        });
        assert!(
            !app.tex_spellcheck_disabled(),
            "stale cached directives cannot affect a new revision"
        );
        app.refresh_tex_magic();
        assert!(!app.tex_spellcheck_disabled());
        assert_eq!(serde_json::to_value(&app.settings).unwrap(), settings);
    }

    #[test]
    fn resolved_build_is_bound_to_preview_revision_and_updates_only_automatic_pdf_output() {
        let context = egui::Context::default();
        let directory = tempfile::tempdir().unwrap();
        let mut app = EditorApp::dormant_for_tests(&context, directory.path().into());
        let chapter = directory.path().join("chapter.tex");
        let main = directory.path().join("main.tex");
        app.document_mut().replace_loaded_unprojected(
            "% !TeX root = main.tex".into(),
            chapter.clone(),
            DocumentKind::Tex,
            None,
        );
        app.document_workflow.pending_export = Some(PendingExport {
            automatic_destination: true,
            path: chapter.with_extension("pdf"),
            source: app.pdf_output_source(),
            intent: PdfWriteIntent::Compile,
            after_artifact_generation: None,
        });
        app.accept_tex_build_info(TexBuildInfo {
            entry: main.clone(),
            engine: BuildEngine::XeLatex,
        });
        assert_eq!(
            app.document_workflow.pending_export.as_ref().unwrap().path,
            main.with_extension("pdf")
        );
        assert_eq!(
            app.current_tex_build().unwrap().engine,
            BuildEngine::XeLatex
        );
        let export = directory.path().join("chosen.pdf");
        app.document_workflow.pending_export.as_mut().unwrap().path = export.clone();
        app.document_workflow
            .pending_export
            .as_mut()
            .unwrap()
            .intent = PdfWriteIntent::Export;
        app.accept_tex_build_info(TexBuildInfo {
            entry: main,
            engine: BuildEngine::LuaLatex,
        });
        assert_eq!(
            app.document_workflow.pending_export.as_ref().unwrap().path,
            export
        );
        app.document_mut()
            .edit(CCursorRange::default(), |source| source.push('\n'));
        assert!(app.current_tex_build().is_none());
    }
}
