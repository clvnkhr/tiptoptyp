use super::*;
#[derive(Clone)]
pub(super) struct VersionedMap {
    pub path: PathBuf,
    pub key: ArtifactKey,
    pub map: Arc<crate::synctex::Artifact>,
    pub pdf: Arc<[u8]>,
}
struct Reply {
    path: PathBuf,
    key: ArtifactKey,
    result: Result<crate::synctex::Destination, String>,
    automatic: bool,
}
struct PendingQuery {
    query: crate::synctex::Query,
    automatic: bool,
    artifact: Option<VersionedMap>,
}
#[derive(Default)]
pub(super) struct State {
    pub artifact: Option<VersionedMap>,
    pending: Option<PendingQuery>,
    job: LatestJob<Reply>,
    error: Option<String>,
}
impl State {
    pub(super) fn activity(&self) -> crate::activity::Activity {
        use crate::activity::Activity;
        if self.job.is_running() {
            Activity::Running
        } else if self.pending.is_some() {
            Activity::Pending("Navigation queued")
        } else if let Some(error) = &self.error {
            Activity::Failed(error.clone())
        } else {
            self.job.activity()
        }
    }
}
impl EditorApp {
    pub(super) fn request_synctex(&mut self, query: crate::synctex::Query, automatic: bool) {
        self.synctex.pending = Some(PendingQuery {
            query,
            automatic,
            artifact: self.synctex.artifact.clone(),
        });
        self.synctex.error = None;
    }
    pub(super) fn poll_synctex(&mut self, context: &egui::Context) {
        // A setting/layout change also cancels an automatic lookup already
        // queued or running. Manual forward/inverse search remains independent.
        let automatic_allowed = self.settings.tex_preview_follow_edits
            && self.preview_visible()
            && !self.compilation_paused;
        if !automatic_allowed {
            self.pdfium_preview.cancel_automatic_follow();
        }
        if let LatestJobPoll::Ready(reply) = self.synctex.job.poll()
            && self.synctex.pending.is_none()
            && (!reply.automatic || automatic_allowed)
            && self
                .synctex
                .artifact
                .as_ref()
                .is_some_and(|current| current.path == reply.path && current.key == reply.key)
            && self.preview_document_path() == reply.path
        {
            match reply.result {
                Ok(crate::synctex::Destination::Page { page, x, y }) => {
                    let pdf = self
                        .synctex
                        .artifact
                        .as_ref()
                        .map(|artifact| artifact.pdf.clone());
                    self.pdfium_preview
                        .reveal_position_for_pdf(page, x, y, !reply.automatic, pdf);
                    if self.view_mode == ViewMode::Code && self.preview_controls.popout.is_none() {
                        self.view_mode = ViewMode::Split;
                    }
                }
                Ok(crate::synctex::Destination::Source { path, line, column }) => self
                    .navigate_file_location(
                        path,
                        None,
                        Some((line, column)),
                        "following a PDF location",
                    ),
                Err(error) => {
                    self.synctex.error = Some(error.clone());
                    if !reply.automatic {
                        self.show_file_error(error);
                    }
                }
            }
        }
        if self.synctex.job.is_running() {
            return;
        }
        let Some(PendingQuery {
            query,
            automatic,
            artifact,
        }) = self.synctex.pending.take()
        else {
            return;
        };
        if automatic && !automatic_allowed {
            return;
        }
        let Some(artifact) = artifact else {
            if !automatic {
                self.show_file_error("Compile this TeX document to produce its SyncTeX map".into());
            }
            return;
        };
        // A click refers to the PDF/map pair visible at that moment. Never
        // resolve those coordinates against a replacement compile artifact.
        if artifact.path != self.preview_document_path()
            || !self
                .synctex
                .artifact
                .as_ref()
                .is_some_and(|current| current.path == artifact.path && current.key == artifact.key)
        {
            return;
        }
        if let Err(error) = self
            .synctex
            .job
            .start_and_repaint("synctex", context, move || {
                Ok(Reply {
                    path: artifact.path,
                    key: artifact.key,
                    result: crate::synctex::query(&artifact.map, &artifact.pdf, query),
                    automatic,
                })
            })
        {
            self.synctex.error = Some(error.clone());
            if !automatic {
                self.show_file_error(error);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn automatic_sync_without_a_map_never_opens_a_dialog_or_starts_a_worker() {
        let context = egui::Context::default();
        let directory = tempfile::tempdir().unwrap();
        let mut app = EditorApp::dormant_for_tests(&context, directory.path().into());
        app.request_synctex(
            crate::synctex::Query::Page {
                page: 0,
                x: 0.0,
                y: 0.0,
            },
            true,
        );
        app.poll_synctex(&context);
        assert!(app.document_workflow.modal().is_none());
        assert!(!app.synctex.job.is_running());
        assert!(app.synctex.pending.is_none());
    }
    #[test]
    fn turning_off_follow_discards_queued_automatic_queries_without_work() {
        let context = egui::Context::default();
        let directory = tempfile::tempdir().unwrap();
        let mut app = EditorApp::dormant_for_tests(&context, directory.path().into());
        app.view_mode = ViewMode::Split;
        app.settings.tex_preview_follow_edits = true;
        app.synctex.artifact = Some(VersionedMap {
            path: app.preview_document_path(),
            key: ArtifactKey::unversioned(1),
            map: Arc::new(crate::synctex::Artifact {
                data: Arc::from([]),
                compressed: false,
                root: directory.path().into(),
                mirror: directory.path().join("mirror"),
            }),
            pdf: Arc::from(&b"PDF"[..]),
        });
        app.request_synctex(
            crate::synctex::Query::Page {
                page: 0,
                x: 0.0,
                y: 0.0,
            },
            true,
        );
        app.settings.tex_preview_follow_edits = false;
        app.poll_synctex(&context);
        assert!(app.synctex.pending.is_none());
        assert!(!app.synctex.job.is_running());
        assert!(app.synctex.error.is_none());
        assert!(app.document_workflow.modal().is_none());
    }

    #[test]
    fn retired_automatic_lookup_cannot_reopen_preview_but_manual_search_can() {
        let context = egui::Context::default();
        let directory = tempfile::tempdir().unwrap();
        for automatic in [true, false] {
            let mut app = EditorApp::dormant_for_tests(&context, directory.path().into());
            app.view_mode = ViewMode::Code;
            app.settings.tex_preview_follow_edits = false;
            let artifact = VersionedMap {
                path: app.preview_document_path(),
                key: ArtifactKey::unversioned(1),
                map: Arc::new(crate::synctex::Artifact {
                    data: Arc::from([]),
                    compressed: false,
                    root: directory.path().into(),
                    mirror: directory.path().join("mirror"),
                }),
                pdf: Arc::from(&b"PDF"[..]),
            };
            app.synctex.artifact = Some(artifact.clone());
            app.synctex
                .job
                .start("test lookup", move || {
                    Ok(Reply {
                        path: artifact.path,
                        key: artifact.key,
                        automatic,
                        result: Ok(crate::synctex::Destination::Page {
                            page: 0,
                            x: 80.0,
                            y: 400.0,
                        }),
                    })
                })
                .unwrap();
            app.synctex.job.wait_until_ready_for_test().unwrap();
            app.poll_synctex(&context);
            assert!(!app.synctex.job.is_running());
            assert_eq!(
                app.view_mode,
                if automatic {
                    ViewMode::Code
                } else {
                    ViewMode::Split
                }
            );
            assert!(app.document_workflow.modal().is_none());
        }
    }

    #[test]
    fn queued_pdf_coordinates_cannot_use_a_retired_preview_or_compile_map() {
        for switch_preview in [false, true] {
            let context = egui::Context::default();
            let directory = tempfile::tempdir().unwrap();
            let mut app = EditorApp::dormant_for_tests(&context, directory.path().into());
            let path = directory.path().join("main.tex");
            app.document_mut().replace_loaded_unprojected(
                "body".into(),
                path.clone(),
                DocumentKind::Tex,
                None,
            );
            let map = VersionedMap {
                path,
                key: ArtifactKey::unversioned(1),
                map: Arc::new(crate::synctex::Artifact {
                    data: Arc::from([]),
                    compressed: false,
                    root: directory.path().into(),
                    mirror: directory.path().join("mirror"),
                }),
                pdf: Arc::from(&b"old PDF"[..]),
            };
            app.synctex.artifact = Some(map.clone());
            app.request_synctex(
                crate::synctex::Query::Page {
                    page: 2,
                    x: 15.0,
                    y: 25.0,
                },
                false,
            );
            assert_eq!(
                app.synctex
                    .pending
                    .as_ref()
                    .unwrap()
                    .artifact
                    .as_ref()
                    .unwrap()
                    .key,
                map.key
            );
            if switch_preview {
                app.document_mut().replace_loaded_unprojected(
                    "other body".into(),
                    directory.path().join("other.tex"),
                    DocumentKind::Tex,
                    None,
                );
            } else {
                app.synctex.artifact = Some(VersionedMap {
                    key: ArtifactKey::unversioned(2),
                    pdf: Arc::from(&b"new PDF"[..]),
                    ..map
                });
            }
            app.poll_synctex(&context);
            assert!(app.synctex.pending.is_none());
            assert!(!app.synctex.job.is_running());
            assert!(app.document_workflow.modal().is_none());
        }
    }

    #[test]
    fn source_context_action_uses_its_unicode_click_and_rejects_an_edited_document() {
        let context = egui::Context::default();
        let directory = tempfile::tempdir().unwrap();
        let mut app = EditorApp::dormant_for_tests(&context, directory.path().into());
        let path = directory.path().join("main.tex");
        let source = "α first\nsecond line";
        app.document_mut().replace_loaded_unprojected(
            source.into(),
            path.clone(),
            DocumentKind::Tex,
            None,
        );
        let target = preview_jump::SourceLocation {
            key: app.document().key(),
            char_index: 10,
        };
        app.pending_app_popup_action = Some(AppPopupAction::Editor(
            EditorMenuAction::RevealInPreview(target),
        ));
        app.execute_pending_app_popup_action(&context, None);
        let pending = app.synctex.pending.take().unwrap();
        assert!(
            matches!(pending.query, crate::synctex::Query::Source { path: ref actual, line: 2, column: 3 } if actual == &path)
        );
        assert_eq!(app.document().source(), source);
        app.document_mut()
            .replace_unprojected_untitled("replacement");
        app.pending_app_popup_action = Some(AppPopupAction::Editor(
            EditorMenuAction::RevealInPreview(target),
        ));
        app.execute_pending_app_popup_action(&context, None);
        assert!(app.synctex.pending.is_none());
        assert_eq!(app.document().source(), "replacement");
    }
}
