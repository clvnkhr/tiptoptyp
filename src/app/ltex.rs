//! Optional local LTeX+ lifecycle. The subprocess and its checks never run on the UI thread.
use super::*;
use crate::ltex::{LtexEvent, LtexService, LtexSettings, LtexSnapshot};

#[derive(Default)]
pub(super) struct LtexState {
    settings: Option<LtexSettings>,
    resource_generation: u64,
    root: Option<PathBuf>,
    service: Option<LtexService>,
    observed: Option<DocumentKey>,
    observed_uri: Option<String>,
    observed_path: Option<PathBuf>,
    observed_language: Option<crate::settings::WritingLanguage>,
    completed: Option<DocumentKey>,
    diagnostics_key: Option<DocumentKey>,
    ready: bool,
    failed: bool,
    active: bool,
    error: Option<String>,
    pub(super) diagnostics: Vec<Diagnostic>,
}
impl LtexState {
    pub(super) fn configure(&mut self, settings: &LtexSettings) {
        if self.settings.as_ref() == Some(settings) {
            return;
        }
        self.settings = Some(settings.clone());
        self.service = None;
        self.observed = None;
        self.observed_language = None;
        self.completed = None;
        self.ready = false;
        self.failed = false;
        self.error = None;
        if !settings.enabled {
            self.active = false;
        }
    }
    pub(super) fn restart(&mut self) {
        self.service = None;
        self.observed = None;
        self.observed_language = None;
        self.completed = None;
        self.ready = false;
        self.failed = false;
        self.error = None;
    }
    fn refresh_resources(&mut self, generation: u64, mode: crate::settings::ToolMode) {
        if self.resource_generation != generation {
            self.resource_generation = generation;
            if mode == crate::settings::ToolMode::Bundled {
                self.restart();
            }
        }
    }
    #[cfg(any(test, feature = "desktop-ui-tests"))]
    pub(super) fn current(&self, key: DocumentKey) -> bool {
        !self.failed && self.completed == Some(key) && self.observed == Some(key)
    }
    fn needs_snapshot(
        &self,
        key: DocumentKey,
        path: Option<&std::path::Path>,
        language: Option<crate::settings::WritingLanguage>,
    ) -> bool {
        self.observed != Some(key)
            || self.observed_path.as_deref() != path
            || self.observed_language != language
    }

    fn accepts(&self, key: DocumentKey, uri: &str) -> bool {
        !self.failed && self.observed == Some(key) && self.observed_uri.as_deref() == Some(uri)
    }
    fn fail(&mut self, error: String) {
        self.failed = true;
        self.ready = false;
        self.error = Some(error);
    }
    pub(super) fn activity(&self) -> crate::activity::Activity {
        use crate::activity::Activity;
        if !self.settings.as_ref().is_some_and(|s| s.enabled) {
            Activity::Inactive("Disabled")
        } else if !self.active {
            Activity::Inactive("No TeX or Typst document")
        } else if let Some(error) = &self.error {
            Activity::Failed(error.clone())
        } else if !self.ready || self.completed != self.observed {
            Activity::Running
        } else {
            Activity::Idle
        }
    }
}
impl EditorApp {
    pub(crate) fn begin_writing_shutdown(&self) {
        if let Some(service) = &self.ltex.service {
            service.begin_stop();
        }
    }
    pub(crate) fn shutdown_writing_checks(&mut self) {
        if let Some(service) = self.ltex.service.take() {
            service.shutdown();
        }
    }
    pub(super) fn update_ltex_checks(&mut self, context: &egui::Context) {
        self.ltex.refresh_resources(
            crate::resource_repair::ltex_generation(),
            self.settings.ltex.mode,
        );
        if self.ltex.settings.is_none() {
            self.ltex.configure(&self.settings.ltex);
        }
        let supported = !self.tabs.is_empty()
            && matches!(
                self.document().kind(),
                DocumentKind::Tex | DocumentKind::Typst
            );
        self.ltex.active = self.settings.ltex.enabled && supported;
        if !self.ltex.active {
            if self.ltex.observed.take().is_some()
                && let Some(service) = &self.ltex.service
            {
                service.close();
            }
            self.ltex.completed = None;
            self.ltex.observed_uri = None;
            self.ltex.observed_path = None;
            self.ltex.observed_language = None;
            self.ltex.diagnostics_key = None;
            if !self.ltex.diagnostics.is_empty() {
                self.ltex.diagnostics.clear();
                self.update_tex_diagnostics();
            }
            return;
        }
        let root = self.project_root();
        if self.ltex.root.as_ref() != Some(&root) {
            self.ltex.service = None;
            self.ltex.root = Some(root.clone());
            self.ltex.observed = None;
            self.ltex.completed = None;
            self.ltex.ready = false;
            self.ltex.failed = false;
            self.ltex.error = None;
        }
        if self.ltex.service.is_none() {
            self.ltex.service = Some(LtexService::spawn(
                self.settings.ltex.clone(),
                root,
                context.clone(),
            ));
        }
        let key = self.document().key();
        let language = self
            .settings
            .ltex
            .follow_document_language
            .then_some(self.settings.writing_language);
        if self
            .ltex
            .needs_snapshot(key, self.document().path().as_deref(), language)
        {
            let keep = self
                .ltex
                .diagnostics_key
                .is_some_and(|old| crate::diagnostics::same_document(old, key));
            if !keep && !self.ltex.diagnostics.is_empty() {
                self.ltex.diagnostics.clear();
                self.update_tex_diagnostics();
            }
            self.ltex.observed = Some(key);
            self.ltex.observed_path = self.document().path().clone();
            self.ltex.observed_language = language;
            self.ltex.completed = None;
            self.ltex.observed_uri = None;
            if self.ltex.failed {
                return;
            }
            if self.document().source().len() > 2_000_000 {
                self.ltex.error =
                    Some("LTeX+: document exceeds the 2 MB writing-check limit".into());
                self.ltex.service.as_ref().unwrap().close();
                return;
            }
            let path = self
                .document()
                .path()
                .clone()
                .unwrap_or_else(|| self.tinymist_document_path());
            let Ok(uri) = url::Url::from_file_path(path) else {
                self.ltex.error =
                    Some("LTeX+: document path cannot be represented as a file URI".into());
                return;
            };
            self.ltex.observed_uri = Some(uri.to_string());
            // A failed subprocess remains failed until explicit Retry or a
            // settings/workspace change. Edits cannot revive its dead worker.
            if !self.ltex.failed {
                self.ltex.error = None;
                self.ltex.service.as_ref().unwrap().update(LtexSnapshot {
                    key,
                    uri: uri.into(),
                    language_id: if self.document().kind() == DocumentKind::Tex {
                        "latex"
                    } else {
                        "typst"
                    }
                    .into(),
                    writing_language: self.settings.writing_language,
                    source: Arc::from(self.document().source().as_str()),
                });
            }
        }
        // Bounded even if a malfunctioning server floods its output.
        for _ in 0..64 {
            let Some(event) = self.ltex.service.as_ref().and_then(|s| s.try_recv()) else {
                break;
            };
            match event {
                LtexEvent::Ready if !self.ltex.failed => self.ltex.ready = true,
                LtexEvent::Error(error) => self.ltex.fail(error),
                LtexEvent::Diagnostics {
                    key: completed,
                    uri,
                    diagnostics,
                } => {
                    self.accept_ltex_diagnostics(completed, &uri, diagnostics);
                }
                _ => {}
            }
        }
    }

    /// Only this admission boundary binds edit actions to an active source.
    /// Untitled tabs use their owned backing URI, never `Main` (the preview
    /// document can be a different tab).
    fn accept_ltex_diagnostics(
        &mut self,
        key: DocumentKey,
        uri: &str,
        mut diagnostics: Vec<Diagnostic>,
    ) -> bool {
        if self.tabs.is_empty() || self.document().key() != key || !self.ltex.accepts(key, uri) {
            return false;
        }
        let Some(path) = url::Url::parse(uri)
            .ok()
            .and_then(|uri| uri.to_file_path().ok())
        else {
            return false;
        };
        crate::diagnostics::bind_document(&mut diagnostics, key);
        for diagnostic in &mut diagnostics {
            diagnostic.source = DiagnosticSource::File(path.clone());
        }
        self.ltex.completed = Some(key);
        self.ltex.diagnostics_key = Some(key);
        self.ltex.error = None;
        self.ltex.diagnostics = diagnostics;
        self.update_tex_diagnostics();
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(revision: u64) -> DocumentKey {
        DocumentKey::new(
            tiptoptyp_core::document::WindowSessionId::new(1),
            2,
            revision,
        )
    }

    #[test]
    fn following_language_changes_recheck_unchanged_source_without_restarting_service() {
        use crate::settings::WritingLanguage;
        let state = LtexState {
            observed: Some(key(3)),
            observed_path: Some(PathBuf::from("/tmp/paper.typ")),
            observed_language: Some(WritingLanguage::Auto),
            ..Default::default()
        };
        let path = Some(std::path::Path::new("/tmp/paper.typ"));
        assert!(!state.needs_snapshot(key(3), path, Some(WritingLanguage::Auto)));
        assert!(state.needs_snapshot(key(3), path, Some(WritingLanguage::American)));
        assert!(state.needs_snapshot(key(4), path, Some(WritingLanguage::Auto)));
        let explicit = LtexState {
            observed_language: None,
            ..state
        };
        assert!(!explicit.needs_snapshot(key(3), path, None));
    }

    #[test]
    fn result_admission_requires_matching_document_revision_and_exact_uri() {
        let mut state = LtexState {
            observed: Some(key(3)),
            observed_uri: Some("file:///tmp/first.tex".into()),
            ..Default::default()
        };
        assert!(state.accepts(key(3), "file:///tmp/first.tex"));
        assert!(!state.accepts(key(2), "file:///tmp/first.tex"));
        assert!(!state.accepts(key(3), "file:///tmp/renamed.tex"));
        let other = DocumentKey::new(tiptoptyp_core::document::WindowSessionId::new(2), 2, 3);
        assert!(!state.accepts(other, "file:///tmp/first.tex"));
        state.observed_uri = Some("file:///tmp/renamed.tex".into());
        assert!(!state.accepts(key(3), "file:///tmp/first.tex"));
        assert!(state.accepts(key(3), "file:///tmp/renamed.tex"));
        state.observed_uri = None;
        assert!(!state.accepts(key(3), "file:///tmp/renamed.tex"));
    }

    #[test]
    fn bundled_resource_refresh_restarts_only_the_bundled_checker_once() {
        let mut state = LtexState {
            observed: Some(key(3)),
            completed: Some(key(3)),
            ..Default::default()
        };
        state.refresh_resources(1, crate::settings::ToolMode::Custom);
        assert_eq!(state.observed, Some(key(3)));
        state.refresh_resources(2, crate::settings::ToolMode::Bundled);
        assert_eq!(state.observed, None);
        state.observed = Some(key(4));
        state.refresh_resources(2, crate::settings::ToolMode::Bundled);
        assert_eq!(state.observed, Some(key(4)));
    }

    #[test]
    fn fatal_error_cannot_be_cleared_by_queued_results_or_subsequent_edits() {
        let mut state = LtexState::default();
        let settings = LtexSettings {
            enabled: true,
            ..Default::default()
        };
        state.configure(&settings);
        state.active = true;
        state.observed = Some(key(3));
        state.observed_uri = Some("file:///tmp/paper.tex".into());
        state.completed = Some(key(3));
        state.fail("server exited".into());
        assert!(!state.current(key(3)));
        assert!(!state.accepts(key(3), "file:///tmp/paper.tex"));
        state.observed = Some(key(4));
        assert!(!state.accepts(key(4), "file:///tmp/paper.tex"));
        assert_eq!(
            state.activity(),
            crate::activity::Activity::Failed("server exited".into())
        );
        state.restart();
        assert!(!state.failed);
        assert!(state.error.is_none());
        assert!(!state.current(key(4)));
    }

    #[test]
    fn unpreviewed_untitled_results_and_corrections_stay_in_the_original_tab() {
        let context = egui::Context::default();
        let root = tempfile::tempdir().unwrap();
        let mut app = EditorApp::dormant_for_tests(&context, root.path().into());
        let first = app.tabs.active_id();
        app.lifecycle = DocumentLifecycle::Active;
        app.new_tab(&context);
        assert_ne!(app.tabs.active_id(), first);
        assert_eq!(app.tabs.preview_id(), first);
        app.document_mut()
            .replace_unprojected_untitled("teh result.");
        let key = app.document().key();
        let path = app.tinymist_document_path();
        let uri = url::Url::from_file_path(&path).unwrap().to_string();
        app.ltex.observed = Some(key);
        app.ltex.observed_uri = Some(uri.clone());
        let diagnostic = Diagnostic {
            correction: Some(crate::diagnostics::DiagnosticCorrection::new(
                0..3,
                "teh".into(),
                Some("SPELLING".into()),
                None,
                vec!["the".into()],
            )),
            provider: Some("LTeX+".into()),
            severity: DiagnosticSeverity::Warning,
            source: DiagnosticSource::Main,
            location: Some(crate::diagnostics::DiagnosticLocation { line: 1, column: 1 }),
            message: "Spelling".into(),
            details: Vec::new(),
        };
        assert!(app.accept_ltex_diagnostics(key, &uri, vec![diagnostic]));
        let published = app.ltex.diagnostics[0].clone();
        assert_eq!(published.source, DiagnosticSource::File(path));
        assert!(app.diagnostic_targets_current_document(&published));
        app.apply_diagnostic_correction(&published, 0, &context)
            .unwrap();
        assert_eq!(app.document().source(), "the result.");
        assert!(
            matches!(app.pending_editor_selection, Some(EditorSelection::Focus(ref range)) if *range == (0..3))
        );
        assert_eq!(app.tabs.preview_id(), first);
        assert!(
            !app.accept_ltex_diagnostics(key, &uri, Vec::new()),
            "stale empty publication cannot clear diagnostics"
        );
        assert_eq!(app.ltex.diagnostics, vec![published]);
    }

    #[test]
    fn settings_and_retry_do_not_discard_stale_results() {
        let mut state = LtexState::default();
        let mut settings = LtexSettings {
            enabled: true,
            ..Default::default()
        };
        state.configure(&settings);
        let key = DocumentKey::new(tiptoptyp_core::document::WindowSessionId::new(1), 1, 1);
        state.observed = Some(key);
        state.completed = Some(key);
        state.diagnostics_key = Some(key);
        state.diagnostics.push(Diagnostic {
            correction: None,
            provider: Some("LTeX+".into()),
            severity: DiagnosticSeverity::Help,
            source: DiagnosticSource::Main,
            location: None,
            message: "Stale grammar result".into(),
            details: vec![],
        });
        settings.configuration["language"] = "en-US".into();
        state.configure(&settings);
        assert_eq!(state.diagnostics.len(), 1);
        assert_eq!(state.diagnostics_key, Some(key));
        assert!(!state.current(key));
        state.restart();
        assert_eq!(state.diagnostics.len(), 1);
        assert!(!state.current(key));
    }

    #[test]
    fn disabled_checker_never_launches_and_retires_only_its_results() {
        let context = egui::Context::default();
        let root = tempfile::tempdir().unwrap();
        let mut app = EditorApp::dormant_for_tests(&context, root.path().into());
        app.ltex.diagnostics.push(Diagnostic {
            correction: None,
            provider: Some("LTeX+".into()),
            severity: DiagnosticSeverity::Help,
            source: DiagnosticSource::Main,
            location: None,
            message: "old".into(),
            details: vec![],
        });
        app.update_ltex_checks(&context);
        assert!(app.ltex.service.is_none());
        assert!(app.ltex.diagnostics.is_empty());
        assert_eq!(
            app.ltex.activity(),
            crate::activity::Activity::Inactive("Disabled")
        );
    }
}
