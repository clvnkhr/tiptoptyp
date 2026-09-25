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
#[derive(Default)]
pub(super) struct State {
    pub artifact: Option<VersionedMap>,
    pending: Option<(crate::synctex::Query, bool)>,
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
        self.synctex.pending = Some((query, automatic));
        self.synctex.error = None;
    }
    pub(super) fn poll_synctex(&mut self, context: &egui::Context) {
        if let LatestJobPoll::Ready(reply) = self.synctex.job.poll()
            && self
                .synctex
                .artifact
                .as_ref()
                .is_some_and(|current| current.path == reply.path && current.key == reply.key)
            && self.preview_document_path() == reply.path
        {
            match reply.result {
                Ok(crate::synctex::Destination::Page { page, x: _, y }) => {
                    self.pdfium_preview.go_to_position(page, y);
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
        let Some((query, automatic)) = self.synctex.pending.take() else {
            return;
        };
        let Some(artifact) = self
            .synctex
            .artifact
            .clone()
            .filter(|a| a.path == self.preview_document_path())
        else {
            if !automatic {
                self.show_file_error("Compile this TeX document to produce its SyncTeX map".into());
            }
            return;
        };
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
}
