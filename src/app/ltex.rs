//! Optional local LTeX+ lifecycle. The subprocess and its checks never run on the UI thread.
use super::*;
use crate::ltex::{LtexEvent, LtexService, LtexSettings, LtexSnapshot};

#[derive(Default)]
pub(super) struct LtexState {
    settings: Option<LtexSettings>,
    resource_generation: u64,
    root: Option<PathBuf>,
    service: ServicePhase,
    check: CheckPhase,
    diagnostics_key: Option<DocumentKey>,
    pub(super) diagnostics: Vec<Diagnostic>,
}
#[derive(Default)]
enum ServicePhase {
    #[default]
    Stopped,
    Starting(LtexService),
    Ready(LtexService),
    Failed {
        process: Option<LtexService>,
        message: String,
    },
}
impl ServicePhase {
    fn process(&self) -> Option<&LtexService> {
        match self {
            Self::Starting(process) | Self::Ready(process) => Some(process),
            Self::Failed { process, .. } => process.as_ref(),
            Self::Stopped => None,
        }
    }
    fn take_process(&mut self) -> Option<LtexService> {
        match std::mem::take(self) {
            Self::Starting(process) | Self::Ready(process) => Some(process),
            Self::Failed { process, .. } => process,
            Self::Stopped => None,
        }
    }
    fn failed(&self) -> bool {
        matches!(self, Self::Failed { .. })
    }
}
struct Identity {
    key: DocumentKey,
    path: Option<PathBuf>,
    language: Option<crate::settings::WritingLanguage>,
}
struct Submitted {
    identity: Identity,
    uri: String,
}
#[derive(Default)]
enum CheckPhase {
    #[default]
    Inactive,
    AwaitingSnapshot,
    Unsubmitted(Identity),
    Checking(Submitted),
    Complete(Submitted),
    Rejected {
        identity: Identity,
        message: String,
    },
}
impl CheckPhase {
    fn identity(&self) -> Option<&Identity> {
        match self {
            Self::Unsubmitted(identity) | Self::Rejected { identity, .. } => Some(identity),
            Self::Checking(submitted) | Self::Complete(submitted) => Some(&submitted.identity),
            _ => None,
        }
    }
    fn submitted(&self) -> Option<&Submitted> {
        match self {
            Self::Checking(submitted) | Self::Complete(submitted) => Some(submitted),
            _ => None,
        }
    }
    fn complete(&mut self) {
        *self = match std::mem::take(self) {
            Self::Checking(submitted) | Self::Complete(submitted) => Self::Complete(submitted),
            other => other,
        };
    }
}
impl LtexState {
    pub(super) fn configure(&mut self, settings: &LtexSettings) {
        if self.settings.as_ref() == Some(settings) {
            return;
        }
        let active = settings.enabled && self.active();
        self.settings = Some(settings.clone());
        self.service = ServicePhase::Stopped;
        self.check = if active {
            CheckPhase::AwaitingSnapshot
        } else {
            CheckPhase::Inactive
        };
    }
    pub(super) fn restart(&mut self) {
        let active = self.active();
        self.service = ServicePhase::Stopped;
        self.check = if active {
            CheckPhase::AwaitingSnapshot
        } else {
            CheckPhase::Inactive
        };
    }
    pub(super) fn active(&self) -> bool {
        !matches!(self.check, CheckPhase::Inactive)
    }
    pub(super) fn ready(&self) -> bool {
        matches!(self.service, ServicePhase::Ready(_))
    }
    pub(super) fn failed(&self) -> bool {
        self.service.failed()
    }
    pub(super) fn error(&self) -> Option<&str> {
        match (&self.service, &self.check) {
            (ServicePhase::Failed { message, .. }, _)
            | (_, CheckPhase::Rejected { message, .. }) => Some(message),
            _ => None,
        }
    }
    pub(super) fn observed(&self) -> Option<DocumentKey> {
        self.check.identity().map(|identity| identity.key)
    }
    pub(super) fn completed(&self) -> Option<DocumentKey> {
        if let CheckPhase::Complete(submitted) = &self.check {
            Some(submitted.identity.key)
        } else {
            None
        }
    }
    fn observe(
        &mut self,
        key: DocumentKey,
        path: Option<PathBuf>,
        language: Option<crate::settings::WritingLanguage>,
    ) {
        self.check = CheckPhase::Unsubmitted(Identity {
            key,
            path,
            language,
        });
    }
    fn submit(&mut self, uri: String) {
        let CheckPhase::Unsubmitted(identity) = std::mem::take(&mut self.check) else {
            panic!("LTeX snapshots must be selected before submission")
        };
        self.check = CheckPhase::Checking(Submitted { identity, uri });
    }
    fn reject_snapshot(&mut self, message: String) {
        let CheckPhase::Unsubmitted(identity) = std::mem::take(&mut self.check) else {
            panic!("only an unsubmitted LTeX snapshot can be rejected")
        };
        self.check = CheckPhase::Rejected { identity, message };
    }
    fn mark_ready(&mut self) {
        self.service = match std::mem::take(&mut self.service) {
            ServicePhase::Starting(process) => ServicePhase::Ready(process),
            other => other,
        };
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
        !self.failed() && self.completed() == Some(key)
    }
    fn needs_snapshot(
        &self,
        key: DocumentKey,
        path: Option<&Path>,
        language: Option<crate::settings::WritingLanguage>,
    ) -> bool {
        self.check.identity().is_none_or(|identity| {
            identity.key != key || identity.path.as_deref() != path || identity.language != language
        })
    }
    fn accepts(&self, key: DocumentKey, uri: &str) -> bool {
        !self.failed()
            && self
                .check
                .submitted()
                .is_some_and(|submitted| submitted.identity.key == key && submitted.uri == uri)
    }
    fn fail(&mut self, message: String) {
        let process = self.service.take_process();
        self.service = ServicePhase::Failed { process, message };
    }
    pub(super) fn activity(&self) -> crate::activity::Activity {
        use crate::activity::Activity;
        if !self.settings.as_ref().is_some_and(|s| s.enabled) {
            Activity::Inactive("Disabled")
        } else if !self.active() {
            Activity::Inactive("No TeX or Typst document")
        } else if let Some(error) = self.error() {
            Activity::Failed(error.into())
        } else if !self.ready() || self.completed().is_none() {
            Activity::Running
        } else {
            Activity::Idle
        }
    }
}
impl EditorApp {
    pub(crate) fn begin_writing_shutdown(&self) {
        if let Some(service) = self.ltex.service.process() {
            service.begin_stop();
        }
    }
    pub(crate) fn shutdown_writing_checks(&mut self) {
        if let Some(service) = self.ltex.service.take_process() {
            service.shutdown();
        }
    }
    pub(super) fn update_ltex_checks(&mut self, context: &egui::Context) {
        self.refresh_tex_magic();
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
        let active = self.settings.ltex.enabled && supported && !self.tex_spellcheck_disabled();
        if !active {
            if self.ltex.observed().is_some()
                && let Some(service) = self.ltex.service.process()
            {
                service.close();
            }
            self.ltex.check = CheckPhase::Inactive;
            self.ltex.diagnostics_key = None;
            if !self.ltex.diagnostics.is_empty() {
                self.ltex.diagnostics.clear();
                self.update_tex_diagnostics();
            }
            return;
        }
        let root = self.project_root();
        if self.ltex.root.as_ref() != Some(&root) {
            self.ltex.restart();
            self.ltex.root = Some(root.clone());
        }
        if matches!(self.ltex.check, CheckPhase::Inactive) {
            self.ltex.check = CheckPhase::AwaitingSnapshot;
        }
        if matches!(self.ltex.service, ServicePhase::Stopped) {
            self.ltex.service = ServicePhase::Starting(LtexService::spawn(
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
            self.ltex
                .observe(key, self.document().path().clone(), language);
            if self.ltex.failed() {
                return;
            }
            if self.document().source().len() > 2_000_000 {
                self.ltex
                    .reject_snapshot("LTeX+: document exceeds the 2 MB writing-check limit".into());
                self.ltex.service.process().unwrap().close();
                return;
            }
            let path = self
                .document()
                .path()
                .clone()
                .unwrap_or_else(|| self.tinymist_document_path());
            let Ok(uri) = url::Url::from_file_path(path) else {
                self.ltex.reject_snapshot(
                    "LTeX+: document path cannot be represented as a file URI".into(),
                );
                return;
            };
            self.ltex.submit(uri.to_string());
            // A failed subprocess remains failed until explicit Retry or a
            // settings/workspace change. Edits cannot revive its dead worker.
            if !self.ltex.failed() {
                self.ltex.service.process().unwrap().update(LtexSnapshot {
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
            let Some(event) = self.ltex.service.process().and_then(|s| s.try_recv()) else {
                break;
            };
            match event {
                LtexEvent::Ready => self.ltex.mark_ready(),
                LtexEvent::Error(error) => self.ltex.fail(error),
                LtexEvent::Diagnostics {
                    key: completed,
                    uri,
                    diagnostics,
                } => {
                    self.accept_ltex_diagnostics(completed, &uri, diagnostics);
                }
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
        self.ltex.check.complete();
        self.ltex.diagnostics_key = Some(key);
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
        let mut state = LtexState::default();
        let path = Some(PathBuf::from("/tmp/paper.typ"));
        state.observe(key(3), path.clone(), Some(WritingLanguage::Auto));
        assert!(!state.needs_snapshot(key(3), path.as_deref(), Some(WritingLanguage::Auto)));
        assert!(state.needs_snapshot(key(3), path.as_deref(), Some(WritingLanguage::American)));
        assert!(state.needs_snapshot(key(4), path.as_deref(), Some(WritingLanguage::Auto)));
        state.observe(key(3), path.clone(), None);
        assert!(!state.needs_snapshot(key(3), path.as_deref(), None));
    }

    #[test]
    fn result_admission_requires_matching_document_revision_and_exact_uri() {
        let mut state = LtexState::default();
        state.observe(key(3), None, None);
        state.submit("file:///tmp/first.tex".into());
        assert!(state.accepts(key(3), "file:///tmp/first.tex"));
        assert!(!state.accepts(key(2), "file:///tmp/first.tex"));
        assert!(!state.accepts(key(3), "file:///tmp/renamed.tex"));
        let other = DocumentKey::new(tiptoptyp_core::document::WindowSessionId::new(2), 2, 3);
        assert!(!state.accepts(other, "file:///tmp/first.tex"));
        state.observe(key(3), None, None);
        state.submit("file:///tmp/renamed.tex".into());
        assert!(!state.accepts(key(3), "file:///tmp/first.tex"));
        assert!(state.accepts(key(3), "file:///tmp/renamed.tex"));
        state.observe(key(3), None, None);
        assert!(!state.accepts(key(3), "file:///tmp/renamed.tex"));
    }

    #[test]
    fn bundled_resource_refresh_restarts_only_the_bundled_checker_once() {
        let mut state = LtexState::default();
        state.observe(key(3), None, None);
        state.submit("file:///tmp/paper.tex".into());
        state.check.complete();
        state.refresh_resources(1, crate::settings::ToolMode::Custom);
        assert_eq!(state.observed(), Some(key(3)));
        state.refresh_resources(2, crate::settings::ToolMode::Bundled);
        assert_eq!(state.observed(), None);
        state.observe(key(4), None, None);
        state.refresh_resources(2, crate::settings::ToolMode::Bundled);
        assert_eq!(state.observed(), Some(key(4)));
    }

    #[test]
    fn fatal_error_cannot_be_cleared_by_queued_results_or_subsequent_edits() {
        let mut state = LtexState::default();
        let settings = LtexSettings {
            enabled: true,
            ..Default::default()
        };
        state.configure(&settings);
        state.observe(key(3), None, None);
        state.submit("file:///tmp/paper.tex".into());
        state.check.complete();
        state.fail("server exited".into());
        state.mark_ready();
        assert!(
            state.failed(),
            "a queued readiness event cannot revive a failed process"
        );
        assert!(!state.current(key(3)));
        assert!(!state.accepts(key(3), "file:///tmp/paper.tex"));
        state.observe(key(4), None, None);
        assert!(!state.accepts(key(4), "file:///tmp/paper.tex"));
        assert_eq!(
            state.activity(),
            crate::activity::Activity::Failed("server exited".into())
        );
        state.restart();
        assert!(!state.failed());
        assert!(state.error().is_none());
        assert!(!state.current(key(4)));
    }

    #[test]
    fn a_rejected_snapshot_can_be_replaced_without_reviving_a_failed_service() {
        let mut state = LtexState::default();
        state.observe(key(3), None, None);
        state.reject_snapshot("snapshot too large".into());
        assert_eq!(state.error(), Some("snapshot too large"));
        assert!(!state.accepts(key(3), "file:///tmp/paper.tex"));
        state.observe(key(4), None, None);
        state.submit("file:///tmp/paper.tex".into());
        assert!(state.error().is_none());
        assert!(state.accepts(key(4), "file:///tmp/paper.tex"));
        state.fail("process stopped".into());
        state.observe(key(5), None, None);
        assert!(!state.accepts(key(5), "file:///tmp/paper.tex"));
        assert_eq!(state.error(), Some("process stopped"));
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
        app.ltex.observe(key, None, None);
        app.ltex.submit(uri.clone());
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
        state.observe(key, None, None);
        state.submit("file:///tmp/paper.tex".into());
        state.check.complete();
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
        app.settings.ltex.enabled = false;
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
        assert!(app.ltex.service.process().is_none());
        assert!(app.ltex.diagnostics.is_empty());
        assert_eq!(
            app.ltex.activity(),
            crate::activity::Activity::Inactive("Disabled")
        );
    }
}
