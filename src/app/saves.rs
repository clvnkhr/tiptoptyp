//! Owner-local admission and receipt routing; disk effects stay in save_io.
use super::*;
use tiptoptyp::save_transaction::WriteDurability;

pub(super) struct PendingSave {
    pub tab: u64,
    pub key: DocumentKey,
    continuation: Option<tiptoptyp_core::workflow::SaveContinuationToken>,
    path: PathBuf,
    previous_sync_path: PathBuf,
    path_changed: bool,
    format_after: bool,
    started: Instant,
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
        // Auto-save and the second write after formatting never request formatting.
        let Some(tab) = self.tabs.active_id() else {
            return false;
        };
        self.submit_save(tab, path, intent, false, context)
    }
    pub(super) fn submit_save(
        &mut self,
        tab: u64,
        path: PathBuf,
        intent: SaveIntent,
        format_after: bool,
        context: &egui::Context,
    ) -> bool {
        let started = Instant::now();
        if self.snapshot_scene.is_some() || self.save_job.is_running() {
            return false;
        }
        let path = canonical_or_absolute(&path);
        if self.tabs.ids().any(|id| {
            id != tab
                && self
                    .document_for_tab(id)
                    .is_some_and(|document| document.path().as_ref() == Some(&path))
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
            .is_none_or(|current| !same_path(current, &path));
        let previous_path = document.path().clone();
        let expected = match intent {
            SaveIntent::ExplicitConfirmed { observed } => {
                observed.map_or(ExpectedDiskState::Missing, ExpectedDiskState::Fingerprint)
            }
            SaveIntent::Auto if document.disk_fingerprint().is_none() || path_changed => {
                self.notice = Some(Notice {
                    message: "Auto-save paused: no known disk baseline".into(),
                    kind: NoticeKind::Error,
                });
                return false;
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
        if let Err(error) = self
            .save_job
            .start_and_repaint("Save document", context, move || {
                Ok(crate::save_io::execute(input))
            })
        {
            if intent != SaveIntent::Auto {
                self.document_workflow.cancel_continuation();
            }
            self.notice = Some(Notice {
                message: error,
                kind: NoticeKind::Error,
            });
            return false;
        }
        self.pending_save = Some(PendingSave {
            tab,
            key,
            continuation,
            path,
            previous_sync_path,
            path_changed,
            format_after: format_after && continuation.is_none(),
            started,
        });
        self.document_workflow.save_in_flight = true;
        self.set_tab_autosave(tab, None);
        self.notice = Some(Notice {
            message: "Saving…".into(),
            kind: NoticeKind::Info,
        });
        true
    }

    pub(super) fn poll_save(&mut self, context: &egui::Context) {
        self.poll_save_inner(context);
        self.document_workflow.finish_dispatch();
    }

    fn poll_save_inner(&mut self, context: &egui::Context) {
        let result = match self.save_job.poll() {
            LatestJobPoll::Idle | LatestJobPoll::Pending => return,
            LatestJobPoll::Ready(result) => Ok(result),
            LatestJobPoll::Failed(error) => Err(error),
        };
        self.document_workflow.save_in_flight = false;
        let Some(pending) = self.pending_save.take() else {
            return;
        };
        let result = match result {
            Ok(result) => result,
            Err(error) => {
                if pending.continuation == self.document_workflow.continuation_token() {
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
        let token_matches =
            result.completion.continuation == self.document_workflow.continuation_token();
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
        let committed = match result.completion.result {
            Ok(committed) => committed,
            Err(error) => {
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
                        self.document_workflow.modal_had_focus = false;
                        self.document_workflow.modal_suspended = false;
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
            active && synchronized,
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
            let workspace = if pending.path.starts_with(&previous_workspace) {
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
        if active && unchanged && pending.format_after && self.document().kind().is_typst() {
            if pending.path_changed {
                self.format_when_tinymist_ready =
                    save_as_format_handoff(true, self.document().kind(), self.document().key());
            } else {
                self.request_format_after_manual_save();
            }
        }
        context.request_repaint();
    }

    #[cfg(test)]
    pub(super) fn finish_save_for_test(&mut self, context: &egui::Context) {
        let deadline = Instant::now() + Duration::from_secs(10);
        while self.save_job.is_running() {
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
    fn failed_save_worker_does_not_cancel_a_newer_close_request() {
        let root = tempfile::tempdir().unwrap();
        let context = egui::Context::default();
        let mut app = EditorApp::dormant_for_tests(&context, root.path().into());
        app.pending_save = Some(PendingSave {
            continuation: None,
            tab: app.tabs.active_id().unwrap(),
            key: app.document().key(),
            path: root.path().join("failed.typ"),
            previous_sync_path: app.tinymist_document_path(),
            path_changed: true,
            format_after: false,
            started: Instant::now(),
        });
        app.save_job
            .start_and_repaint("failed save worker", &context, || {
                Err("worker failed".into())
            })
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
        app.pending_save = Some(PendingSave {
            tab,
            key,
            continuation: app.document_workflow.continuation_token(),
            path: path.clone(),
            previous_sync_path: app.tinymist_document_path(),
            path_changed: true,
            format_after: true,
            started: Instant::now(),
        });
        app.save_job
            .start_and_repaint("uncertain save", &context, || Ok(result))
            .unwrap();
        app.finish_save_for_test(&context);
        assert_eq!(fs::read_to_string(&path).unwrap(), "saved bytes");
        assert_eq!(app.document().path().as_ref(), Some(&path));
        assert!(!app.document().is_dirty());
        assert!(!app.document_workflow.has_continuation());
        assert!(app.document_workflow.take_action().is_none());
        assert!(app.format_when_tinymist_ready.is_none());
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
        app.document_workflow.finish_dispatch();
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
        assert!(app.manual_format_revision.is_none());
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
        app.document_workflow.finish_dispatch();
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
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("note.typ");
        let context = egui::Context::default();
        let mut app = EditorApp::dormant_for_tests(&context, root.path().into());
        app.snapshot_scene = None;
        app.document_mut().replace_unprojected_untitled("hello");
        let (release, holder) = hold_destination(path.clone());
        assert!(app.save_to(path.clone(), &context));
        assert!(app.format_when_tinymist_ready.is_none());
        assert!(app.document().path().is_none());
        release.send(()).unwrap();
        holder.join().unwrap();
        app.finish_save_for_test(&context);
        assert_eq!(app.format_when_tinymist_ready, Some(app.document().key()));
        app.format_when_tinymist_ready = None;
        app.document_mut()
            .edit(CCursorRange::default(), |source| source.push('!'));
        assert!(app.save_to_with_intent(path, SaveIntent::Auto, &context));
        app.finish_save_for_test(&context);
        assert!(app.format_when_tinymist_ready.is_none());
        assert!(app.manual_format_revision.is_none());
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
