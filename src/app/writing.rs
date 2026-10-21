use super::*;
type WritingIdentity = (
    DocumentKey,
    bool,
    bool,
    crate::settings::WritingLanguage,
    u64,
);
#[derive(Default)]
pub(super) struct WritingState {
    observed: Option<WritingIdentity>,
    completed: Option<WritingIdentity>,
    skipped: Option<&'static str>,
    deadline: Option<Instant>,
    job: LatestJob<(WritingIdentity, Vec<Diagnostic>)>,
    checker: Arc<std::sync::Mutex<crate::writing::Checker>>,
    dictionary: Arc<str>,
    tex_prose_rules: Arc<crate::writing::prose_rules::TexProseRules>,
    ignore_capitalized_names: bool,
    configuration_revision: u64,
    pub(super) diagnostics: Vec<Diagnostic>,
    pub(super) markers: Vec<usize>,
}
impl WritingState {
    /// Called when Settings are applied, so dictionary contents are not scanned
    /// or copied by the per-frame scheduler. Jobs carry this revision to reject
    /// old results while the previous diagnostics remain visible.
    pub(super) fn configure(
        &mut self,
        dictionary: &str,
        ignore_capitalized_names: bool,
        rules: &crate::writing::prose_rules::TexProseRules,
    ) {
        if self.dictionary.as_ref() != dictionary
            || self.ignore_capitalized_names != ignore_capitalized_names
            || self.tex_prose_rules.as_ref() != rules
        {
            if self.dictionary.as_ref() != dictionary {
                self.dictionary = Arc::from(dictionary);
            }
            self.ignore_capitalized_names = ignore_capitalized_names;
            self.tex_prose_rules = Arc::new(rules.clone());
            self.configuration_revision = self.configuration_revision.wrapping_add(1);
        }
    }
    #[cfg(any(test, feature = "desktop-ui-tests"))]
    pub(super) fn current(
        &self,
        key: DocumentKey,
        grammar: bool,
        unicode: bool,
        language: crate::settings::WritingLanguage,
    ) -> bool {
        self.completed == Some((key, grammar, unicode, language, self.configuration_revision))
    }

    pub(super) fn activity(&self) -> crate::activity::Activity {
        if let Some(reason) = self.skipped {
            return crate::activity::Activity::Inactive(reason);
        }
        let state = self.job.activity();
        if self.deadline.is_some() && matches!(state, crate::activity::Activity::Idle) {
            crate::activity::Activity::Pending("Stale; waiting for edits to settle")
        } else {
            state
        }
    }
    pub(super) fn markers(&self, key: DocumentKey) -> &[usize] {
        if self.observed.is_some_and(|observed| observed.0 == key) {
            &self.markers
        } else {
            &[]
        }
    }
}
impl EditorApp {
    pub(super) fn update_writing_checks(&mut self, context: &egui::Context) {
        if self.tabs.is_empty() {
            // Superseding only drops result acceptance; it cannot stop a worker
            // that still owns the checker. Keep that receiver until completion
            // so reopening cannot queue more workers behind the same mutex.
            if !matches!(self.writing.job.poll(), LatestJobPoll::Pending) {
                self.writing.job.supersede();
            }
            self.writing.deadline = None;
            self.writing.observed = None;
            self.writing.completed = None;
            self.writing.skipped = Some("No document is open");
            if !self.writing.diagnostics.is_empty() || !self.writing.markers.is_empty() {
                self.writing.diagnostics.clear();
                self.writing.markers.clear();
                self.update_tex_diagnostics();
            }
            return;
        }
        if self.writing.observed.is_none()
            || self.writing.ignore_capitalized_names
                != self.settings.harper_ignore_capitalized_names
        {
            self.writing.configure(
                &self.settings.harper_dictionary,
                self.settings.harper_ignore_capitalized_names,
                &self.settings.tex_prose_rules,
            );
        }
        let key = self.document().key();
        let identity = (
            key,
            self.settings.english_grammar,
            self.settings.unicode_warnings,
            self.settings.writing_language,
            self.writing.configuration_revision,
        );
        if self.writing.observed != Some(identity) {
            let retain = self
                .writing
                .observed
                .is_some_and(|previous| crate::diagnostics::same_document(previous.0, key));
            self.writing.observed = Some(identity);
            self.writing.skipped = None;
            self.writing.deadline = Some(Instant::now() + Duration::from_millis(200));
            let previous_count = self.writing.diagnostics.len();
            self.writing.diagnostics.retain(|diagnostic| {
                retain
                    && match diagnostic.provider.as_deref() {
                        Some("Harper") => identity.1,
                        Some("Unicode") => identity.2,
                        _ => true,
                    }
            });
            if !retain || !identity.2 {
                self.writing.markers.clear();
            }
            if self.writing.diagnostics.len() != previous_count {
                self.update_tex_diagnostics();
            }
        }
        match self.writing.job.poll() {
            LatestJobPoll::Ready((completed, mut diagnostics))
                if Some(completed) == self.writing.observed =>
            {
                crate::diagnostics::bind_document(&mut diagnostics, completed.0);
                let mut starts = vec![0];
                for (index, ch) in self.document().source().chars().enumerate() {
                    if ch == '\n' {
                        starts.push(index + 1);
                    }
                }
                self.writing.markers = diagnostics
                    .iter()
                    .filter(|d| d.provider.as_deref() == Some("Unicode"))
                    .filter_map(|d| {
                        let location = d.location?;
                        Some(
                            starts.get(location.line.checked_sub(1)?)?
                                + location.column.saturating_sub(1),
                        )
                    })
                    .collect();
                self.writing.completed = Some(completed);
                self.writing.diagnostics = diagnostics;
                self.update_tex_diagnostics();
            }
            LatestJobPoll::Failed(error) => self.show_file_error(error),
            _ => {}
        }
        let Some(deadline) = self.writing.deadline else {
            return;
        };
        if !identity.1 && !identity.2 {
            self.writing.deadline = None;
            return;
        }
        let now = Instant::now();
        if now < deadline {
            context.request_repaint_after(deadline - now);
            return;
        }
        // Finish the previous job before launching another. The newest revision
        // replaces pending work; typing never creates concurrent grammar jobs.
        if self.writing.job.is_running() {
            return;
        }
        self.writing.deadline = None;
        let snapshot = self.document().snapshot();
        let kind = self.document().kind();
        let path = self
            .document()
            .path()
            .clone()
            .unwrap_or_else(|| self.tinymist_document_path());
        if snapshot.source().len() > 2_000_000 {
            self.writing.skipped = Some("Document exceeds writing-check size limit");
            return;
        }
        let checker = Arc::clone(&self.writing.checker);
        let dictionary = Arc::clone(&self.writing.dictionary);
        let rules = Arc::clone(&self.writing.tex_prose_rules);
        let ignore_capitalized_names = self.writing.ignore_capitalized_names;
        if let Err(error) =
            self.writing
                .job
                .start_and_repaint("writing-checks", context, move || {
                    let mut checker_guard = checker.lock().unwrap_or_else(|poisoned| {
                        let mut guard = poisoned.into_inner();
                        *guard = crate::writing::Checker::default();
                        checker.clear_poison();
                        guard
                    });
                    Ok((
                        identity,
                        checker_guard.check_with_options(
                            snapshot.source(),
                            kind,
                            &path,
                            identity.1,
                            identity.2,
                            identity.3,
                            crate::writing::Options {
                                dictionary: &dictionary,
                                ignore_capitalized_names,
                                tex_prose_rules: Some(&rules),
                            },
                        ),
                    ))
                })
        {
            self.show_file_error(error);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn dictionary_and_name_settings_supersede_old_jobs_without_clearing_stale_lints() {
        // Test the asynchronous acceptance boundary with controlled replies;
        // actual dictionary additions/removals are covered by Checker tests.
        let root = tempfile::tempdir().unwrap();
        let context = egui::Context::default();
        let mut app = EditorApp::dormant_for_tests(&context, root.path().into());
        app.settings.english_grammar = true;
        app.settings.unicode_warnings = false;
        app.settings.harper_dictionary.clear();
        app.settings.harper_ignore_capitalized_names = false;
        app.document_mut()
            .replace_unprojected_untitled("Zulverton uses zorbaflume.");
        app.update_writing_checks(&context);
        let old = app.writing.observed.unwrap();
        let stale = vec![Diagnostic {
            correction: None,
            provider: Some("Harper".into()),
            severity: DiagnosticSeverity::Help,
            source: DiagnosticSource::Main,
            location: Some(crate::diagnostics::DiagnosticLocation { line: 1, column: 1 }),
            message: "old spelling result".into(),
            details: Vec::new(),
        }];
        app.writing.diagnostics = stale.clone();
        app.writing.completed = Some(old);
        let mut settings = app.settings_snapshot();
        settings.harper_dictionary = "zorbaflume".into();
        settings.harper_ignore_capitalized_names = true;
        app.apply_shared_settings(settings, &context);
        assert!(!app.writing.current(old.0, old.1, old.2, old.3));
        app.update_writing_checks(&context);
        let changed = app.writing.observed.unwrap();
        assert_ne!(changed, old);
        assert_eq!(app.writing.dictionary.as_ref(), "zorbaflume");
        assert!(app.writing.ignore_capitalized_names);
        assert_eq!(app.writing.diagnostics, stale);
        app.writing.deadline = None;
        app.writing
            .job
            .start_and_repaint("old-dictionary-test", &context, move || {
                Ok((old, Vec::new()))
            })
            .unwrap();
        let timeout = Instant::now() + Duration::from_secs(30);
        while app.writing.job.is_running() && Instant::now() < timeout {
            app.update_writing_checks(&context);
            std::thread::yield_now();
        }
        assert!(!app.writing.job.is_running());
        assert_eq!(app.writing.completed, Some(old));
        assert_eq!(
            app.writing.diagnostics, stale,
            "obsolete settings cannot clear stale diagnostics"
        );
        app.writing
            .job
            .start_and_repaint("new-dictionary-test", &context, move || {
                Ok((changed, Vec::new()))
            })
            .unwrap();
        let timeout = Instant::now() + Duration::from_secs(30);
        while app.writing.job.is_running() && Instant::now() < timeout {
            app.update_writing_checks(&context);
            std::thread::yield_now();
        }
        assert_eq!(app.writing.completed, Some(changed));
        assert!(app.writing.diagnostics.is_empty());
        let mut settings = app.settings_snapshot();
        settings.harper_dictionary.clear();
        settings.harper_ignore_capitalized_names = false;
        app.apply_shared_settings(settings, &context);
        app.update_writing_checks(&context);
        assert_ne!(app.writing.observed, Some(changed));
        assert!(
            !app.writing
                .current(changed.0, changed.1, changed.2, changed.3)
        );
        assert!(app.writing.dictionary.is_empty());
        assert!(!app.writing.ignore_capitalized_names);
        assert!(
            app.writing.deadline.is_some(),
            "removal schedules an authoritative replacement"
        );
    }

    #[test]
    fn edits_keep_writing_diagnostics_until_replacement_but_document_changes_retire_them() {
        let root = tempfile::tempdir().unwrap();
        let context = egui::Context::default();
        let mut app = EditorApp::dormant_for_tests(&context, root.path().into());
        app.settings.unicode_warnings = true;
        app.document_mut()
            .replace_unprojected_untitled("old\u{200b}");
        app.update_writing_checks(&context);
        app.writing.diagnostics = crate::writing::check(
            "old\u{200b}",
            DocumentKind::Typst,
            Path::new("fixture.typ"),
            false,
            true,
            crate::settings::WritingLanguage::Auto,
        );
        let original = app.writing.diagnostics.clone();
        assert!(!original.is_empty());
        app.document_mut()
            .edit(CCursorRange::default(), |source| source.push('!'));
        app.update_writing_checks(&context);
        assert_eq!(
            app.writing.diagnostics, original,
            "edits must not remove stale diagnostics"
        );
        // A successful empty response is authoritative.
        let identity = app.writing.observed.unwrap();
        app.writing
            .job
            .start_and_repaint("writing-test", &context, move || Ok((identity, Vec::new())))
            .unwrap();
        for _ in 0..1000 {
            app.update_writing_checks(&context);
            if !app.writing.job.is_running() {
                break;
            }
            std::thread::sleep(Duration::from_millis(1));
        }
        assert!(app.writing.diagnostics.is_empty());
        assert_eq!(app.writing.completed, app.writing.observed);
        assert!(
            app.writing
                .current(identity.0, identity.1, identity.2, identity.3)
        );
        app.writing.diagnostics = original;
        app.document_mut()
            .replace_unprojected_untitled("new document");
        app.update_writing_checks(&context);
        assert!(
            app.writing.diagnostics.is_empty(),
            "never show another document's lints"
        );
    }

    #[test]
    fn closing_and_reopening_waits_for_the_previous_writing_worker() {
        let root = tempfile::tempdir().unwrap();
        let context = egui::Context::default();
        let mut app = EditorApp::dormant_for_tests(&context, root.path().into());
        app.settings.english_grammar = true;
        app.settings.unicode_warnings = false;
        app.settings.mitex_auto_enable = false;
        app.document_mut()
            .replace_unprojected_untitled("This is the old result.");
        app.update_writing_checks(&context);
        let previous = app.writing.observed.unwrap();
        app.writing.deadline = None;
        let obsolete = Diagnostic {
            correction: None,
            provider: Some("Harper".into()),
            severity: DiagnosticSeverity::Help,
            source: DiagnosticSource::Main,
            location: None,
            message: "obsolete writing result".into(),
            details: Vec::new(),
        };
        app.writing.diagnostics = vec![obsolete.clone()];
        let checker = Arc::clone(&app.writing.checker);
        let (started, started_rx) = std::sync::mpsc::channel();
        let (release, release_rx) = std::sync::mpsc::channel();
        app.writing
            .job
            .start_and_repaint("blocked-writing-test", &context, move || {
                let _guard = checker.lock().unwrap();
                started.send(()).map_err(|error| error.to_string())?;
                release_rx.recv().map_err(|error| error.to_string())?;
                Ok((previous, vec![obsolete]))
            })
            .unwrap();
        started_rx.recv_timeout(Duration::from_secs(2)).unwrap();

        for _ in 0..3 {
            app.finish_close_tab(&context);
            app.update_writing_checks(&context);
            assert!(app.tabs.is_empty());
            assert!(
                app.writing.job.is_running(),
                "closing tabs cannot forget a worker that still owns the checker"
            );
            assert!(app.writing.diagnostics.is_empty());
            assert!(app.writing.completed.is_none());
            assert!(matches!(
                app.writing.activity(),
                crate::activity::Activity::Inactive(_)
            ));

            app.new_tab(&context);
            app.document_mut()
                .replace_unprojected_untitled("This is the the fresh result.");
            app.update_writing_checks(&context);
            assert_ne!(app.writing.observed, Some(previous));
            app.writing.deadline = Some(Instant::now());
            app.update_writing_checks(&context);
            assert!(app.writing.job.is_running());
            assert!(
                app.writing.deadline.is_some(),
                "the reopened revision must wait rather than queue another worker"
            );
            assert!(app.writing.completed.is_none());
            assert!(app.writing.diagnostics.is_empty());
        }

        release.send(()).unwrap();
        app.writing.job.wait_until_ready_for_test().unwrap();
        app.update_writing_checks(&context);
        assert!(app.writing.completed.is_none());
        assert!(
            app.writing.diagnostics.is_empty(),
            "the retired worker's result must never reach the reopened document"
        );
        assert!(
            app.writing.job.is_running(),
            "the latest revision starts only after the old worker completes"
        );
        app.writing.job.wait_until_ready_for_test().unwrap();
        app.update_writing_checks(&context);
        assert_eq!(app.writing.completed, app.writing.observed);
        assert!(
            app.writing
                .diagnostics
                .iter()
                .any(|diagnostic| diagnostic.message.contains("repeat")),
            "the latest revision must complete a real fresh grammar check"
        );
    }

    #[test]
    fn writing_worker_recovers_after_a_checker_panics() {
        let root = tempfile::tempdir().unwrap();
        let context = egui::Context::default();
        let mut app = EditorApp::dormant_for_tests(&context, root.path().into());
        app.settings.english_grammar = true;
        app.settings.unicode_warnings = false;
        let source = "This is is a result.";
        app.document_mut().replace_unprojected_untitled(source);
        let checker = Arc::clone(&app.writing.checker);
        let panicked = std::thread::spawn(move || {
            let _guard = checker.lock().unwrap();
            panic!("simulate an interrupted writing check");
        });
        assert!(panicked.join().is_err());
        assert!(app.writing.checker.is_poisoned());

        app.update_writing_checks(&context);
        app.writing.deadline = Some(Instant::now());
        app.update_writing_checks(&context);
        app.writing.job.wait_until_ready_for_test().unwrap();
        app.update_writing_checks(&context);
        assert_eq!(app.writing.completed, app.writing.observed);
        assert!(!app.writing.checker.is_poisoned());
        assert!(
            app.writing.diagnostics.iter().any(|diagnostic| {
                diagnostic.provider.as_deref() == Some("Harper")
                    && diagnostic.message.contains("repeat")
            }),
            "the recovered worker must publish real grammar diagnostics"
        );
        assert_eq!(app.document().source(), source);
    }

    #[test]
    fn current_writing_requires_a_completed_revision_and_the_same_settings() {
        let root = tempfile::tempdir().unwrap();
        let context = egui::Context::default();
        let mut app = EditorApp::dormant_for_tests(&context, root.path().into());
        let old = app.document().key();
        let language = crate::settings::WritingLanguage::British;
        app.writing.observed = Some((old, true, false, language, 0));
        assert!(!app.writing.current(old, true, false, language));
        app.writing.completed = app.writing.observed;
        assert!(app.writing.current(old, true, false, language));
        assert!(!app.writing.current(old, false, false, language));
        assert!(!app.writing.current(old, true, true, language));
        assert!(
            !app.writing
                .current(old, true, false, crate::settings::WritingLanguage::American)
        );
        app.document_mut()
            .edit(CCursorRange::default(), |source| source.push('!'));
        let new = app.document().key();
        app.writing.observed = Some((new, true, false, language, 0));
        assert!(
            !app.writing.current(new, true, false, language),
            "scheduling a new request does not make stale diagnostics current"
        );
    }

    #[test]
    fn closing_the_last_tab_retires_writing_checks_without_requesting_a_document_path() {
        let root = tempfile::tempdir().unwrap();
        let context = egui::Context::default();
        let mut app = EditorApp::dormant_window_for_tests(
            &context,
            root.path().into(),
            egui::ViewportId::ROOT,
        );
        app.lifecycle = DocumentLifecycle::Active;
        app.settings.english_grammar = true;
        app.settings.unicode_warnings = true;
        app.finish_close_tab(&context);
        app.writing.observed = Some((
            app.document().key(),
            true,
            true,
            crate::settings::WritingLanguage::Auto,
            0,
        ));
        app.writing.deadline = Some(Instant::now() - Duration::from_secs(1));
        app.update_writing_checks(&context);
        assert!(app.tabs.is_empty());
        assert!(!app.close_accepted());
        assert!(app.writing.deadline.is_none());
        assert!(app.writing.completed.is_none());
        assert!(!app.writing.job.is_running());
        assert!(matches!(
            app.writing.activity(),
            crate::activity::Activity::Inactive(_)
        ));
    }
}
