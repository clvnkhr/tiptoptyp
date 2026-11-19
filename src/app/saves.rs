//! Owner-local admission and receipt routing; disk effects stay in save_io.
use super::*;
use tiptoptyp::save_transaction::WriteDurability;

pub(super) struct PendingSave {
    operation: crate::workflow::SaveOperationId,
    pub tab: u64,
    pub key: DocumentKey,
    continuation: Option<tiptoptyp_core::workflow::SaveContinuationToken>,
    path: PathBuf,
    previous_sync_path: PathBuf,
    path_changed: bool,
    format_after: bool,
    started: Instant,
}

/// The local write and its routing context share one lifetime. There is no
/// worker without an owner and no pending receipt without an admitted worker.
#[derive(Default)]
pub(super) enum State {
    #[default]
    Idle,
    Writing(Box<Active>),
}

pub(super) struct Active {
    pending: PendingSave,
    job: crate::worker::ExclusiveJob<crate::save_io::SaveResult>,
}

impl State {
    fn start(
        &mut self,
        pending: PendingSave,
        context: &egui::Context,
        work: impl FnOnce() -> Result<crate::save_io::SaveResult, String> + Send + 'static,
    ) -> Result<(), String> {
        if self.is_running() {
            return Err("A document save is already running".into());
        }
        let mut job = crate::worker::ExclusiveJob::default();
        job.start_and_repaint("Save document", context, work)?;
        *self = Self::Writing(Box::new(Active { pending, job }));
        Ok(())
    }

    pub(super) fn is_running(&self) -> bool {
        matches!(self, Self::Writing(_))
    }

    pub(super) fn pending(&self) -> Option<&PendingSave> {
        match self {
            Self::Idle => None,
            Self::Writing(active) => Some(&active.pending),
        }
    }

    pub(super) fn activity(&self) -> crate::activity::Activity {
        match self {
            Self::Idle => crate::activity::Activity::Idle,
            Self::Writing(active) => active.job.activity(),
        }
    }

    fn poll(&mut self) -> Option<(PendingSave, Result<crate::save_io::SaveResult, String>)> {
        let Self::Writing(active) = self else {
            return None;
        };
        let result = match active.job.poll() {
            LatestJobPoll::Pending => return None,
            LatestJobPoll::Ready(result) => Ok(result),
            LatestJobPoll::Failed(error) => Err(error),
            LatestJobPoll::Idle => unreachable!("a writing phase owns an admitted save"),
        };
        let Self::Writing(active) = std::mem::take(self) else {
            unreachable!()
        };
        Some((active.pending, result))
    }
}

impl EditorApp {
    /// True means admitted, never that bytes are already durable.
    pub(super) fn save_to(&mut self, path: PathBuf, context: &egui::Context) -> bool {
        let Some(tab) = self.tabs.active_id() else {
            return false;
        };
        self.submit_save(tab, path, SaveIntent::Explicit, true, context)
    }
    pub(super) fn save_to_with_intent(
        &mut self,
        path: PathBuf,
        intent: SaveIntent,
        context: &egui::Context,
    ) -> bool {
        // Save intent owns which opt-in correction preference applies, including
        // confirmed manual overwrites and non-formatting automatic saves.
        let Some(tab) = self.tabs.active_id() else {
            return false;
        };
        self.submit_save(tab, path, intent, false, context)
    }
    /// Formatting completes a save that already passed on-save correction.
    /// Do not rescan its result or request another formatter pass.
    pub(super) fn save_to_after_format(&mut self, path: PathBuf, context: &egui::Context) -> bool {
        let Some(tab) = self.tabs.active_id() else {
            return false;
        };
        self.submit_save_prepared(tab, path, SaveIntent::Explicit, false, context)
    }
    pub(super) fn submit_save(
        &mut self,
        tab: u64,
        path: PathBuf,
        intent: SaveIntent,
        format_after: bool,
        context: &egui::Context,
    ) -> bool {
        let autofix_enabled = self.autofix_enabled_for_save(intent);
        let preserve_draft = !self.is_overleaf_path(&path) && self.overleaf_tab_source_locked(tab);
        let correct_before_save = autofix_enabled
            // Saving a protected draft (including Save As a local Overleaf
            // recovery copy) must remain possible without editing that source.
            && !preserve_draft
            && self
                .document_for_tab(tab)
                .is_some_and(|document| document.kind() == DocumentKind::Tex)
            && crate::tex::autofix::Rule::ALL
                .iter()
                .any(|rule| rule.enabled(&self.settings.autofix));
        if correct_before_save {
            if !self.autofix_save_admitted(tab, &path, intent) {
                return false;
            }
            return self.prepare_autofix_save(tab, path, intent, format_after, context);
        }
        self.submit_save_prepared(tab, path, intent, format_after && !preserve_draft, context)
    }

    pub(super) fn autofix_enabled_for_save(&self, intent: SaveIntent) -> bool {
        match intent {
            SaveIntent::Explicit | SaveIntent::ExplicitConfirmed { .. } => {
                self.settings.autofix_on_manual_save
            }
            SaveIntent::Auto => self.settings.autofix_on_autosave,
        }
    }

    /// Check mutable destination and transport guards before any source edits.
    /// A deferred scan must recheck this policy when its result is admitted.
    pub(super) fn autofix_save_admitted(
        &mut self,
        tab: u64,
        path: &Path,
        intent: SaveIntent,
    ) -> bool {
        if self.document_workflow.save_in_flight()
            || self.local_save.is_running()
            || self.snapshot_scene.is_some()
        {
            return false;
        }
        if self.is_overleaf_path(path) {
            return self.overleaf_autofix_save_admitted(tab);
        }
        self.local_save_destination_admitted(tab, &canonical_or_absolute(path), intent)
    }

    fn local_save_destination_admitted(
        &mut self,
        tab: u64,
        path: &Path,
        intent: SaveIntent,
    ) -> bool {
        if self.tabs.ids().any(|id| {
            id != tab
                && self.document_for_tab(id).is_some_and(|document| {
                    document
                        .path()
                        .as_deref()
                        .is_some_and(|current| same_path(current, path))
                })
        }) {
            self.show_file_error(
                "This file is already open in another tab; switch to it before saving".into(),
            );
            return false;
        }
        let Some(document) = self.document_for_tab(tab) else {
            return false;
        };
        let path_changed = document
            .path()
            .as_ref()
            .is_none_or(|current| !same_path(current, path));
        if intent == SaveIntent::Auto && (document.disk_fingerprint().is_none() || path_changed) {
            self.notice = Some(Notice {
                message: "Auto-save paused: no known disk baseline".into(),
                kind: NoticeKind::Error,
            });
            return false;
        }
        true
    }

    pub(super) fn submit_save_prepared(
        &mut self,
        tab: u64,
        path: PathBuf,
        intent: SaveIntent,
        format_after: bool,
        context: &egui::Context,
    ) -> bool {
        if self.document_workflow.save_in_flight() {
            return false;
        }
        if self.is_overleaf_path(&path) {
            return self.submit_overleaf_save(tab, &path, context);
        }
        let started = Instant::now();
        if self.snapshot_scene.is_some() || self.local_save.is_running() {
            return false;
        }
        self.activity.save_error = None;
        let path = canonical_or_absolute(&path);
        if !self.local_save_destination_admitted(tab, &path, intent) {
            return false;
        }
        let Some(document) = self.document_for_tab(tab) else {
            return false;
        };
        let path_changed = document
            .path()
            .as_ref()
            .is_none_or(|current| !same_path(current, &path));
        let previous_path = document.path().clone();
        let expected = match intent {
            SaveIntent::ExplicitConfirmed { observed } => {
                observed.map_or(ExpectedDiskState::Missing, ExpectedDiskState::Fingerprint)
            }
            _ if path_changed => ExpectedDiskState::Unchecked,
            _ => document
                .disk_fingerprint()
                .map_or(ExpectedDiskState::Unchecked, ExpectedDiskState::Fingerprint),
        };
        let kind = if path_changed {
            crate::document::detect_document(&path, document.source().as_bytes())
                .ok()
                .filter(|kind| kind.is_editable())
                .unwrap_or(DocumentKind::Text)
        } else {
            document.kind()
        };
        let key = document.key();
        let previous_sync_path = previous_path.unwrap_or_else(|| self.untitled_tab_path(tab));
        let request = match document.prepare_save(path.clone(), kind) {
            Ok(request) => request,
            Err(error) => {
                if intent != SaveIntent::Auto {
                    self.document_workflow.cancel_continuation();
                }
                self.notice = Some(Notice {
                    message: error.to_string(),
                    kind: NoticeKind::Error,
                });
                return false;
            }
        };
        let continuation = (self.tabs.active_id() == Some(tab))
            .then(|| self.document_workflow.continuation_token())
            .flatten();
        let input = SaveInput::new(request, expected, intent, continuation);
        let operation = match self
            .document_workflow
            .begin_save(key, crate::workflow::SaveDomain::Local)
        {
            Ok(operation) => operation,
            Err(error) => {
                self.notice = Some(Notice {
                    message: error.into(),
                    kind: NoticeKind::Error,
                });
                return false;
            }
        };
        let pending = PendingSave {
            operation,
            tab,
            key,
            continuation,
            path,
            previous_sync_path,
            path_changed,
            format_after: format_after && continuation.is_none(),
            started,
        };
        if let Err(error) = self
            .local_save
            .start(pending, context, move || Ok(crate::save_io::execute(input)))
        {
            self.document_workflow.end_save(operation);
            if intent != SaveIntent::Auto {
                self.document_workflow.cancel_continuation();
            }
            self.notice = Some(Notice {
                message: error,
                kind: NoticeKind::Error,
            });
            return false;
        }
        self.set_tab_autosave(tab, None);
        self.notice = Some(Notice {
            message: "Saving…".into(),
            kind: NoticeKind::Info,
        });
        true
    }

    pub(super) fn poll_save(&mut self, context: &egui::Context) {
        self.poll_save_inner(context);
        self.finish_document_dispatch();
    }

    fn poll_save_inner(&mut self, context: &egui::Context) {
        let Some((pending, result)) = self.local_save.poll() else {
            return;
        };
        let owns_operation = self.document_workflow.end_save(pending.operation);
        let result = match result {
            Ok(result) => result,
            Err(error) => {
                if owns_operation
                    && pending.continuation == self.document_workflow.continuation_token()
                {
                    self.show_file_error(error);
                } else {
                    self.notice = Some(Notice {
                        message: error,
                        kind: NoticeKind::Error,
                    });
                }
                return;
            }
        };
        let active = self.tabs.active_id() == Some(pending.tab);
        let token_matches = owns_operation
            && result.completion.continuation == self.document_workflow.continuation_token();
        let current = self.document_for_tab(pending.tab).is_some_and(|doc| {
            doc.key().owner == pending.key.owner && doc.epoch() == pending.key.epoch
        });
        if !current {
            if token_matches {
                self.document_workflow.cancel_continuation();
            }
            self.notice = Some(Notice {
                message: crate::worker::OperationSummary::completion_summary(&result),
                kind: NoticeKind::Info,
            });
            return;
        }
        let unchanged = self
            .document_for_tab(pending.tab)
            .is_some_and(|doc| doc.key() == pending.key);
        let intent = result.completion.intent;
        self.activity.save_error = None;
        let backup_warning = result.backup_warning;
        let committed = match result.completion.result {
            Ok(committed) => committed,
            Err(error) => {
                self.activity.save_error = Some(error.to_string());
                if let Some(observed) = result.conflict {
                    if active && unchanged && intent != SaveIntent::Auto {
                        self.document_workflow.set_modal(AppModal::Overwrite {
                            message: format!(
                                "{} changed on disk. Overwrite those changes?",
                                pending.path.display()
                            ),
                            path: pending.path,
                            key: self.document().key(),
                            expected_disk_fingerprint: self.document().disk_fingerprint(),
                            observed_disk_fingerprint: match observed {
                                ExpectedDiskState::Fingerprint(value) => Some(value),
                                _ => None,
                            },
                        });
                        self.document_workflow.modal_focus =
                            crate::child_view::ModalFocus::default();
                        return;
                    }
                } else if intent == SaveIntent::Auto {
                    self.set_tab_autosave(pending.tab, Some(Instant::now() + AUTOSAVE_RETRY_DELAY));
                }
                if token_matches {
                    self.document_workflow.cancel_continuation();
                }
                self.notice = Some(Notice {
                    message: format!("Could not save {}: {error}", pending.path.display()),
                    kind: NoticeKind::Error,
                });
                return;
            }
        };
        let synchronized = matches!(committed.durability, WriteDurability::Synchronized);
        if !active && token_matches {
            self.document_workflow.cancel_continuation();
        }
        // Both active and parked receipts update their stable tab, never an
        // active slot. Only the active owner may release a window continuation.
        let document = self
            .tabs
            .document_mut(pending.tab)
            .expect("validated save owner");
        let continuation = self.document_workflow.complete_save(
            result.completion.continuation,
            document,
            committed.receipt,
            active && synchronized && owns_operation,
        );
        let continuation = match continuation {
            Ok(action) => action,
            Err(error) => {
                self.notice = Some(Notice {
                    message: error.into(),
                    kind: NoticeKind::Error,
                });
                return;
            }
        };
        let dirty = document.is_dirty();
        let deadline = (dirty && self.settings.auto_save).then(|| {
            Instant::now() + Duration::from_millis(self.settings.auto_save_delay_ms.max(100))
        });
        self.set_tab_autosave(pending.tab, deadline);
        if active {
            self.external_file_change_notice = None;
            self.external_file_stamp = external_file_stamp(&pending.path).ok();
        }
        if pending.path_changed {
            let previous_workspace = self
                .tab_workspace(pending.tab)
                .map(Path::to_path_buf)
                .unwrap_or_else(|| self.workspace_root.clone());
            let workspace = if self
                .workspace_switch
                .keeps_save_workspace(pending.tab, &self.workspace_root)
            {
                // This Save As answers a departure prompt. The pending
                // workspace transaction still owns this page until every
                // draft is approved, including when another prompt cancels.
                self.workspace_root.clone()
            } else if pending.path.starts_with(&previous_workspace) {
                previous_workspace
            } else {
                pending
                    .path
                    .parent()
                    .map(discover_project_root)
                    .map(|root| canonical_or_absolute(&root))
                    .unwrap_or(previous_workspace)
            };
            self.set_tab_workspace(pending.tab, workspace.clone());
            if active {
                self.workspace_root = workspace;
                self.preview.content.invalidate();
                self.reset_document_services();
            } else {
                if let Ok(uri) = crate::tinymist::path_to_file_uri(&pending.previous_sync_path)
                    && let Some(effect) = self.tinymist_sync.close_uri(&uri)
                {
                    let _ = self.apply_tinymist_sync_batch(crate::tinymist_sync::Batch {
                        source: String::new(),
                        effects: vec![effect],
                    });
                }
                self.tinymist_sync.tab_backings.remove(&pending.tab);
                if self.tabs.preview_id() == Some(pending.tab) {
                    self.restart_tinymist_for_preview_entry();
                    self.schedule_compile_now();
                } else {
                    self.sync_parked_tinymist();
                }
            }
        }
        if active || pending.path_changed {
            self.remember_open_document(&pending.path);
        }
        self.refresh_workspace();
        self.git.request_refresh();
        self.git_editor.request_refresh();
        if self.preview_processing_enabled() {
            self.schedule_compile_now();
        }
        self.schedule_project_index();
        if let WriteDurability::Uncertain(error) = committed.durability {
            self.activity.save_error = Some(format!("Save durability uncertain: {error}"));
            if token_matches {
                self.document_workflow.cancel_continuation();
            }
            self.notice = Some(Notice {
                message: format!(
                    "Saved {}, but durability is uncertain: {error}. The document remains open.",
                    pending.path.display()
                ),
                kind: NoticeKind::Error,
            });
            return;
        }
        if let Some(mut action) = continuation {
            action.key = self.document().key();
            action.allow_discard = false;
            self.document_workflow.queue_action(action);
        }
        self.notice = Some(Notice {
            message: completed_action_label(
                if intent == SaveIntent::Auto {
                    "Auto-saved"
                } else {
                    "Saved"
                },
                &pending
                    .path
                    .file_name()
                    .unwrap_or_default()
                    .to_string_lossy(),
                pending.started.elapsed(),
            ),
            kind: NoticeKind::Success,
        });
        if let Some(warning) = backup_warning {
            self.activity.save_error = Some(format!("Local backup unavailable: {warning}"));
            self.notice = Some(Notice {
                message: format!(
                    "Saved {}, but local backup unavailable: {warning}",
                    pending.path.display()
                ),
                kind: NoticeKind::Error,
            });
        }
        if active
            && unchanged
            && pending.format_after
            && self.document().kind().typesetting_language().is_some()
        {
            if pending.path_changed {
                if let Some(key) =
                    save_as_format_handoff(true, self.document().kind(), self.document().key())
                {
                    self.formatting = formatting::State::WaitingForSavedDocument(key);
                }
            } else {
                self.request_format_after_manual_save();
            }
        }
        context.request_repaint();
    }

    #[cfg(test)]
    pub(crate) fn finish_save_for_test(&mut self, context: &egui::Context) {
        let deadline = Instant::now() + Duration::from_secs(10);
        while self.local_save.is_running() {
            self.poll_save(context);
            assert!(Instant::now() < deadline, "save never completed");
            std::thread::yield_now();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;

    #[test]
    fn ordinary_saves_bypass_autofix_when_disabled_unselected_or_not_tex() {
        for (on_save, enabled, kind, filename) in [
            (false, true, DocumentKind::Tex, "note.tex"),
            (true, false, DocumentKind::Tex, "note.tex"),
            (true, true, DocumentKind::Text, "note.txt"),
        ] {
            let root = tempfile::tempdir().unwrap();
            let path = root.path().join(filename);
            fs::write(&path, "old").unwrap();
            let context = egui::Context::default();
            let mut app = EditorApp::dormant_for_tests(&context, root.path().into());
            app.snapshot_scene = None;
            app.settings.autofix_on_manual_save = on_save;
            app.settings.autofix.enabled.clear();
            crate::tex::autofix::Rule::ReferenceTies
                .set_enabled(&mut app.settings.autofix, enabled);
            app.document_mut().replace_loaded_unprojected(
                "old".into(),
                path.clone(),
                kind,
                Some(fingerprint(b"old")),
            );
            let source = "Figure \\ref{fig:a}.";
            app.document_mut()
                .edit(CCursorRange::default(), |text| *text = source.into());
            assert!(app.save_to_with_intent(path.clone(), SaveIntent::Explicit, &context));
            assert!(app.local_save.is_running());
            assert!(!app.autofix.busy());
            app.finish_save_for_test(&context);
            assert_eq!(app.document().source(), source);
            assert_eq!(fs::read_to_string(path).unwrap(), source);
        }
    }

    #[test]
    fn formatter_resave_does_not_start_a_second_autofix_or_formatter_pass() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("note.tex");
        fs::write(&path, "old").unwrap();
        let context = egui::Context::default();
        let mut app = EditorApp::dormant_for_tests(&context, root.path().into());
        app.snapshot_scene = None;
        app.settings.autofix_on_manual_save = true;
        app.settings.autofix.enabled = [crate::tex::autofix::Rule::ReferenceTies].into();
        app.document_mut().replace_loaded_unprojected(
            "old".into(),
            path.clone(),
            DocumentKind::Tex,
            Some(fingerprint(b"old")),
        );
        let source = "Figure \\ref{fig:a}.";
        app.document_mut()
            .edit(CCursorRange::default(), |text| *text = source.into());
        assert!(app.save_to_after_format(path.clone(), &context));
        assert!(app.local_save.is_running());
        assert!(!app.autofix.busy());
        app.finish_save_for_test(&context);
        assert_eq!(fs::read_to_string(path).unwrap(), source);
        assert!(app.formatting.saved_document().is_none());
        assert!(app.formatting.request_key().is_none());
    }

    #[test]
    fn snapshot_save_never_admits_an_on_save_autofix() {
        let root = tempfile::tempdir().unwrap();
        let context = egui::Context::default();
        let mut app = EditorApp::dormant_for_tests(&context, root.path().into());
        app.settings.autofix_on_manual_save = true;
        app.document_mut().replace_untitled_kind(DocumentKind::Tex);
        app.document_mut().edit(CCursorRange::default(), |text| {
            *text = "Figure \\ref{fig:a}.".into();
        });
        let key = app.document().key();
        let path = root.path().join("snapshot.tex");
        assert!(!app.save_to(path.clone(), &context));
        assert!(!app.autofix.busy());
        assert!(!app.local_save.is_running());
        assert_eq!(app.document().key(), key);
        assert!(!path.exists());
    }

    #[test]
    fn auto_save_without_a_matching_baseline_never_edits_or_starts_autofix() {
        for changed_destination in [false, true] {
            let root = tempfile::tempdir().unwrap();
            let original = root.path().join("original.tex");
            fs::write(&original, "old").unwrap();
            let context = egui::Context::default();
            let mut app = EditorApp::dormant_for_tests(&context, root.path().into());
            app.snapshot_scene = None;
            app.settings.autofix_on_autosave = true;
            app.document_mut().replace_loaded_unprojected(
                "old".into(),
                original.clone(),
                DocumentKind::Tex,
                changed_destination.then(|| fingerprint(b"old")),
            );
            app.document_mut().edit(CCursorRange::default(), |text| {
                *text = "Figure \\ref{fig:a}.".into();
            });
            let key = app.document().key();
            let destination = if changed_destination {
                root.path().join("other.tex")
            } else {
                original.clone()
            };
            assert!(!app.save_to_with_intent(destination, SaveIntent::Auto, &context));
            assert!(!app.autofix.busy());
            assert!(!app.local_save.is_running());
            assert_eq!(app.document().key(), key);
            assert_eq!(fs::read_to_string(original).unwrap(), "old");
            assert!(!root.path().join("other.tex").exists());
            assert_eq!(
                app.notice.as_ref().unwrap().message,
                "Auto-save paused: no known disk baseline"
            );
        }
    }

    #[test]
    fn duplicate_open_destination_is_rejected_before_correcting_the_source() {
        let root = tempfile::tempdir().unwrap();
        let first_path = root.path().join("first.tex");
        let second_path = root.path().join("second.tex");
        fs::write(&first_path, "old").unwrap();
        fs::write(&second_path, "other").unwrap();
        let context = egui::Context::default();
        let mut app = EditorApp::dormant_for_tests(&context, root.path().into());
        app.snapshot_scene = None;
        app.settings.autofix_on_manual_save = true;
        app.document_mut().replace_loaded_unprojected(
            "old".into(),
            first_path.clone(),
            DocumentKind::Tex,
            Some(fingerprint(b"old")),
        );
        app.document_mut().edit(CCursorRange::default(), |text| {
            *text = "Figure \\ref{fig:a}.".into();
        });
        let first = app.tabs.active_id().unwrap();
        app.lifecycle = DocumentLifecycle::Active;
        app.new_tab(&context);
        app.lifecycle = DocumentLifecycle::Dormant;
        app.document_mut().replace_loaded_unprojected(
            "other".into(),
            second_path.clone(),
            DocumentKind::Tex,
            Some(fingerprint(b"other")),
        );
        let key = app.document_for_tab(first).unwrap().key();
        assert!(!app.submit_save(
            first,
            second_path.clone(),
            SaveIntent::Explicit,
            false,
            &context
        ));
        assert!(!app.autofix.busy());
        assert!(!app.local_save.is_running());
        assert_eq!(app.document_for_tab(first).unwrap().key(), key);
        assert_eq!(fs::read_to_string(first_path).unwrap(), "old");
        assert_eq!(fs::read_to_string(second_path).unwrap(), "other");
    }

    #[test]
    fn an_old_local_completion_cannot_release_a_newer_shared_save() {
        let root = tempfile::tempdir().unwrap();
        let context = egui::Context::default();
        let mut app = EditorApp::dormant_for_tests(&context, root.path().into());
        let key = app.document().key();
        let operation = app
            .document_workflow
            .begin_save(key, crate::workflow::SaveDomain::Local)
            .unwrap();
        app.local_save
            .start(
                PendingSave {
                    operation,
                    tab: app.tabs.active_id().unwrap(),
                    key,
                    continuation: None,
                    path: root.path().join("old.typ"),
                    previous_sync_path: app.tinymist_document_path(),
                    path_changed: false,
                    format_after: false,
                    started: Instant::now(),
                },
                &context,
                || Err("old worker failed".into()),
            )
            .unwrap();
        assert!(app.document_workflow.end_save(operation));
        let current = app
            .document_workflow
            .begin_save(key, crate::workflow::SaveDomain::Overleaf)
            .unwrap();
        app.finish_save_for_test(&context);
        assert!(app.local_save.pending().is_none());
        assert_eq!(app.document_workflow.current_save().unwrap().id, current);
        assert!(app.document_workflow.end_save(current));
        app.poll_save(&context);
        assert!(!app.document_workflow.save_in_flight());
    }

    #[test]
    fn failed_save_worker_does_not_cancel_a_newer_close_request() {
        let root = tempfile::tempdir().unwrap();
        let context = egui::Context::default();
        let mut app = EditorApp::dormant_for_tests(&context, root.path().into());
        let operation_key = app.document().key();
        let operation = app
            .document_workflow
            .begin_save(operation_key, crate::workflow::SaveDomain::Local)
            .unwrap();
        let pending = PendingSave {
            operation,
            continuation: None,
            tab: app.tabs.active_id().unwrap(),
            key: app.document().key(),
            path: root.path().join("failed.typ"),
            previous_sync_path: app.tinymist_document_path(),
            path_changed: true,
            format_after: false,
            started: Instant::now(),
        };
        app.local_save
            .start(pending, &context, || Err("worker failed".into()))
            .unwrap();
        app.document_workflow
            .continue_after_save(PendingDocumentAction {
                action: DeferredDocumentAction::CloseWindow,
                key: app.document().key(),
                allow_discard: false,
                description: "new close".into(),
            });
        let token = app.document_workflow.continuation_token();
        app.finish_save_for_test(&context);
        assert_eq!(app.document_workflow.continuation_token(), token);
        assert!(app.document_workflow.take_action().is_none());
        assert!(
            app.notice
                .as_ref()
                .unwrap()
                .message
                .contains("worker failed")
        );
    }

    #[test]
    fn uncertain_receipt_records_saved_bytes_but_never_releases_close_or_format() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("uncertain.typ");
        let context = egui::Context::default();
        let mut app = EditorApp::dormant_for_tests(&context, root.path().into());
        app.document_mut()
            .replace_unprojected_untitled("saved bytes");
        let tab = app.tabs.active_id().unwrap();
        let key = app.document().key();
        app.document_workflow
            .continue_after_save(PendingDocumentAction {
                action: DeferredDocumentAction::CloseWindow,
                key,
                allow_discard: false,
                description: "closing".into(),
            });
        let input = SaveInput::new(
            app.document()
                .prepare_save(path.clone(), DocumentKind::Typst)
                .unwrap(),
            ExpectedDiskState::Missing,
            SaveIntent::Explicit,
            app.document_workflow.continuation_token(),
        );
        // Real committed bytes/receipt, with only directory-sync confirmation
        // replaced by a deterministic uncertain outcome.
        let mut result = crate::save_io::execute(input);
        result.completion.result.as_mut().unwrap().durability =
            WriteDurability::Uncertain("injected directory sync failure".into());
        let operation_key = app.document().key();
        let operation = app
            .document_workflow
            .begin_save(operation_key, crate::workflow::SaveDomain::Local)
            .unwrap();
        let pending = PendingSave {
            operation,
            tab,
            key,
            continuation: app.document_workflow.continuation_token(),
            path: path.clone(),
            previous_sync_path: app.tinymist_document_path(),
            path_changed: true,
            format_after: true,
            started: Instant::now(),
        };
        app.local_save
            .start(pending, &context, || Ok(result))
            .unwrap();
        app.finish_save_for_test(&context);
        assert!(
            app.activity
                .save_error
                .as_deref()
                .is_some_and(|e| e.contains("directory sync failure"))
        );
        assert_eq!(fs::read_to_string(&path).unwrap(), "saved bytes");
        assert_eq!(app.document().path().as_ref(), Some(&path));
        assert!(!app.document().is_dirty());
        assert!(!app.document_workflow.has_continuation());
        assert!(app.document_workflow.take_action().is_none());
        assert!(app.formatting.saved_document().is_none());
        assert!(
            app.notice
                .as_ref()
                .unwrap()
                .message
                .contains("durability is uncertain")
        );
    }

    fn hold_destination(path: PathBuf) -> (mpsc::Sender<()>, std::thread::JoinHandle<()>) {
        let (entered, ready) = mpsc::channel();
        let (release, wait) = mpsc::channel();
        let thread = std::thread::spawn(move || {
            crate::resource_lock::with_resource(&path, || {
                entered.send(()).unwrap();
                let _ = wait.recv();
            })
        });
        ready.recv_timeout(Duration::from_secs(5)).unwrap();
        (release, thread)
    }

    #[test]
    fn rename_cannot_race_an_admitted_source_save() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().canonicalize().unwrap().join("note.txt");
        fs::write(&path, "old").unwrap();
        let context = egui::Context::default();
        let mut app = EditorApp::dormant_for_tests(&context, root.path().into());
        app.snapshot_scene = None;
        app.document_mut().replace_loaded_unprojected(
            "submitted".into(),
            path.clone(),
            DocumentKind::Text,
            Some(fingerprint(b"old")),
        );
        let (release, holder) = hold_destination(path.clone());
        assert!(app.save_to(path.clone(), &context));

        // Explorer can dispatch Rename after the save was admitted. It must
        // not retarget the document while its immutable write owns this path.
        app.begin_rename(path.clone());
        assert!(app.rename_dialog.is_none());
        app.commit_rename(path.clone(), "renamed.txt".into(), &context);
        assert_eq!(app.document().path().as_ref(), Some(&path));
        assert_eq!(fs::read_to_string(&path).unwrap(), "old");
        assert!(!path.with_file_name("renamed.txt").exists());
        app.delete_workspace_file(path.clone(), &context);
        assert_eq!(app.document().path().as_ref(), Some(&path));
        assert_eq!(app.document().source(), "submitted");
        assert_eq!(fs::read_to_string(&path).unwrap(), "old");

        release.send(()).unwrap();
        holder.join().unwrap();
        app.finish_save_for_test(&context);
        assert_eq!(fs::read_to_string(&path).unwrap(), "submitted");
        app.begin_rename(path.clone());
        assert_eq!(app.rename_dialog.take().unwrap().path, path);
        app.commit_rename(path.clone(), "renamed.txt".into(), &context);
        assert_eq!(
            fs::read_to_string(path.with_file_name("renamed.txt")).unwrap(),
            "submitted"
        );
        assert!(!path.exists());
    }

    #[test]
    fn delayed_save_keeps_new_edits_dirty_and_does_not_release_close() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("note.txt");
        fs::write(&path, "old").unwrap();
        let context = egui::Context::default();
        let mut app = EditorApp::dormant_for_tests(&context, root.path().into());
        app.snapshot_scene = None;
        app.document_mut().replace_loaded_unprojected(
            "old".into(),
            path.clone(),
            DocumentKind::Text,
            Some(fingerprint(b"old")),
        );
        app.document_mut().edit(CCursorRange::default(), |source| {
            *source = "submitted".into()
        });
        app.document_workflow
            .continue_after_save(PendingDocumentAction {
                action: DeferredDocumentAction::CloseWindow,
                key: app.document().key(),
                allow_discard: false,
                description: "closing".into(),
            });
        let (release, holder) = hold_destination(path.clone());
        assert!(app.save_to(path.clone(), &context));
        assert!(
            !app.save_to(path.clone(), &context),
            "only one immutable request per owner"
        );
        app.finish_document_dispatch();
        assert!(app.document_workflow.has_continuation());
        app.document_mut()
            .edit(CCursorRange::default(), |source| source.push_str(" newer"));
        app.poll_save(&context);
        assert_eq!(fs::read_to_string(&path).unwrap(), "old");
        release.send(()).unwrap();
        holder.join().unwrap();
        app.finish_save_for_test(&context);
        assert_eq!(fs::read_to_string(path).unwrap(), "submitted");
        assert_eq!(app.document().source(), "submitted newer");
        assert!(app.document().is_dirty());
        assert!(!app.document_workflow.has_continuation());
        assert!(app.document_workflow.take_action().is_none());
        assert!(!app.formatting.saves_after_reply());
    }

    #[test]
    fn durable_save_releases_one_close_and_conflicts_require_a_fresh_confirmation() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("note.txt");
        fs::write(&path, "old").unwrap();
        let context = egui::Context::default();
        let mut app = EditorApp::dormant_for_tests(&context, root.path().into());
        app.snapshot_scene = None;
        app.document_mut().replace_loaded_unprojected(
            "old".into(),
            path.clone(),
            DocumentKind::Text,
            Some(fingerprint(b"old")),
        );
        app.document_mut().edit(CCursorRange::default(), |source| {
            *source = "submitted".into()
        });
        app.document_workflow
            .continue_after_save(PendingDocumentAction {
                action: DeferredDocumentAction::CloseTab,
                key: app.document().key(),
                allow_discard: false,
                description: "closing".into(),
            });
        fs::write(&path, "external").unwrap();
        assert!(app.save_to(path.clone(), &context));
        app.finish_save_for_test(&context);
        assert!(matches!(
            app.document_workflow.modal(),
            Some(AppModal::Overwrite { .. })
        ));
        assert!(app.document_workflow.has_continuation());
        assert_eq!(fs::read_to_string(&path).unwrap(), "external");
        app.document_workflow.clear_modal();
        // A confirmation does not authorize overwriting a still later change.
        fs::write(&path, "external again").unwrap();
        let confirmed = SaveIntent::ExplicitConfirmed {
            observed: Some(fingerprint(b"external")),
        };
        assert!(app.save_to_with_intent(path.clone(), confirmed, &context));
        app.finish_save_for_test(&context);
        assert!(matches!(
            app.document_workflow.modal(),
            Some(AppModal::Overwrite { .. })
        ));
        assert_eq!(fs::read_to_string(&path).unwrap(), "external again");
        app.document_workflow.clear_modal();
        assert!(app.save_to_with_intent(
            path.clone(),
            SaveIntent::ExplicitConfirmed {
                observed: Some(fingerprint(b"external again"))
            },
            &context
        ));
        app.finish_document_dispatch();
        app.finish_save_for_test(&context);
        assert_eq!(fs::read_to_string(path).unwrap(), "submitted");
        assert!(matches!(
            app.document_workflow.take_action().unwrap().action,
            DeferredDocumentAction::CloseTab
        ));
        assert!(app.document_workflow.take_action().is_none());
    }

    #[test]
    fn auto_save_does_not_format_and_manual_save_as_waits_for_durable_completion() {
        for (extension, kind) in [("typ", DocumentKind::Typst), ("tex", DocumentKind::Tex)] {
            let root = tempfile::tempdir().unwrap();
            let path = root.path().join(format!("note.{extension}"));
            let context = egui::Context::default();
            let mut app = EditorApp::dormant_for_tests(&context, root.path().into());
            app.snapshot_scene = None;
            app.document_mut().replace_untitled_kind(kind);
            app.document_mut()
                .edit(CCursorRange::default(), |source| source.push_str("hello"));
            let (release, holder) = hold_destination(path.clone());
            assert!(app.save_to(path.clone(), &context));
            assert!(app.formatting.saved_document().is_none());
            assert!(app.document().path().is_none());
            release.send(()).unwrap();
            holder.join().unwrap();
            app.finish_save_for_test(&context);
            assert_eq!(
                app.formatting.saved_document(),
                Some(app.document().key()),
                "{kind:?}"
            );
            app.formatting.cancel();
            app.document_mut()
                .edit(CCursorRange::default(), |source| source.push('!'));
            assert!(app.save_to_with_intent(path, SaveIntent::Auto, &context));
            app.finish_save_for_test(&context);
            assert!(app.formatting.saved_document().is_none());
            assert!(!app.formatting.saves_after_reply());
        }
    }

    #[test]
    fn blank_untitled_document_saves_as_tex_or_an_arbitrary_text_extension() {
        for (name, kind) in [
            ("paper.tex", DocumentKind::Tex),
            ("notes.foo", DocumentKind::Text),
        ] {
            let root = tempfile::tempdir().unwrap();
            let context = egui::Context::default();
            let mut app = EditorApp::dormant_for_tests(&context, root.path().into());
            app.snapshot_scene = None;
            app.reset_untitled_document();
            assert_eq!(app.document().source(), "");
            let path = root.path().join(name);
            assert!(app.save_to(path.clone(), &context));
            app.finish_save_for_test(&context);
            assert_eq!(app.document().kind(), kind);
            assert_eq!(app.document().path().as_ref(), Some(&path));
            assert_eq!(fs::read_to_string(path).unwrap(), "");
        }
    }

    #[test]
    fn replaced_document_does_not_accept_a_late_receipt_or_lose_its_new_close_intent() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("note.txt");
        fs::write(&path, "old").unwrap();
        let context = egui::Context::default();
        let mut app = EditorApp::dormant_for_tests(&context, root.path().into());
        app.snapshot_scene = None;
        app.document_mut().replace_loaded_unprojected(
            "submitted".into(),
            path.clone(),
            DocumentKind::Text,
            Some(fingerprint(b"old")),
        );
        let (release, holder) = hold_destination(path.clone());
        assert!(app.save_to(path.clone(), &context));
        app.document_mut()
            .replace_unprojected_untitled("replacement");
        let key = app.document().key();
        app.document_workflow
            .continue_after_save(PendingDocumentAction {
                action: DeferredDocumentAction::CloseWindow,
                key,
                allow_discard: false,
                description: "new close".into(),
            });
        release.send(()).unwrap();
        holder.join().unwrap();
        app.finish_save_for_test(&context);
        assert_eq!(fs::read_to_string(path).unwrap(), "submitted");
        assert_eq!(app.document().source(), "replacement");
        assert_eq!(app.document().key(), key);
        assert!(app.document().path().is_none());
        assert!(app.document_workflow.has_continuation());
        assert!(app.document_workflow.take_action().is_none());
    }
}
