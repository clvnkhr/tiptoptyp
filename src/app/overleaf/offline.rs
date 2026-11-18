//! Saved local work is independent of an Overleaf acknowledgement.
use super::*;
use crate::overleaf::workspace_session::WorkspaceSession;

const RESTORE_PAGES_PER_FRAME: usize = 4;
struct RestorePages {
    paths: VecDeque<(String, PathBuf)>,
    active: Option<String>,
    preview: Option<String>,
    frame: Option<u64>,
}

#[cfg(test)]
mod state_tests {
    use super::*;

    #[test]
    fn a_previous_workspace_checkpoint_failure_cannot_replace_current_project_status() {
        for changed_root in [false, true] {
            let directory = tempfile::tempdir().unwrap();
            let original_root = directory.path().join("original");
            let current_root = if changed_root {
                directory.path().join("replacement")
            } else {
                original_root.clone()
            };
            fs::create_dir_all(&original_root).unwrap();
            fs::create_dir_all(&current_root).unwrap();
            let original_project =
                ProjectUrl::parse("https://www.overleaf.com/project/0123456789abcdef01234567")
                    .unwrap();
            let current_project = if changed_root {
                ProjectUrl::parse("https://www.overleaf.com/project/1123456789abcdef01234567")
                    .unwrap()
            } else {
                original_project.clone()
            };
            let original = WorkspaceSession::new(&original_project, [1; 16]);
            let current =
                WorkspaceSession::new(&current_project, [if changed_root { 1 } else { 2 }; 16]);
            let context = egui::Context::default();
            let mut app = EditorApp::dormant_for_tests(&context, current_root.clone());
            app.overleaf.offline.install(original.clone(), false);
            app.overleaf
                .offline
                .writer
                .start(
                    SessionWritten {
                        root: original_root,
                        session: original,
                    },
                    &context,
                    || Err("old writer failed".into()),
                )
                .unwrap();
            app.overleaf.connection = Some(Connection {
                model: Model::for_project(1, &current_project),
                project: current_project,
                root: current_root.clone(),
                status: "Current project".into(),
                transport: Transport::Offline,
                storage: None,
                hydration: Hydration::default(),
            });
            app.overleaf.offline.install(current, false);
            app.overleaf.error = Some("current project status".into());
            let deadline = Instant::now() + Duration::from_secs(5);
            while app.overleaf.offline.writer.running() {
                app.poll_overleaf_session(&context);
                assert!(Instant::now() < deadline);
                std::thread::yield_now();
            }
            assert_eq!(
                app.overleaf.error.as_deref(),
                Some("current project status")
            );
            assert_eq!(
                app.overleaf.offline.saved(),
                changed_root,
                "an uncertain write at the same root must invalidate the newer session's cached disk receipt"
            );
        }
    }
}

#[derive(Default)]
pub(super) struct State {
    pub(super) session: Option<WorkspaceSession>,
    pub(super) restored: bool,
    pub(super) departed: bool,
    restoration: PageRestoration,
    pub(super) saved_sources: BTreeMap<String, String>,
    pub(super) recovery_sources: BTreeMap<String, String>,
    pub(super) checkpoint_waiting: BTreeSet<String>,
    pub(super) checkpoint_failed: BTreeSet<String>,
    pub(super) disk_untrusted: BTreeSet<String>,
    saved: Option<WorkspaceSession>,
    writer: SessionWriter,
}
#[derive(Default)]
enum PageRestoration {
    #[default]
    NotRequested,
    WaitingForProject,
    Opening(RestorePages),
    Complete,
}
impl PageRestoration {
    fn active(&self) -> bool {
        matches!(self, Self::WaitingForProject | Self::Opening(_))
    }
    fn plan(&self) -> Option<&RestorePages> {
        if let Self::Opening(plan) = self {
            Some(plan)
        } else {
            None
        }
    }
    fn plan_mut(&mut self) -> Option<&mut RestorePages> {
        if let Self::Opening(plan) = self {
            Some(plan)
        } else {
            None
        }
    }
    fn finish(&mut self) -> RestorePages {
        let Self::Opening(plan) = std::mem::replace(self, Self::Complete) else {
            panic!("only an active restoration plan can finish")
        };
        plan
    }
}
#[derive(Default)]
enum SessionWriter {
    #[default]
    Idle,
    Writing {
        requested: SessionWritten,
        job: ExclusiveJob<SessionWritten>,
    },
    Failed(SessionWritten),
}
impl SessionWriter {
    fn running(&self) -> bool {
        matches!(self, Self::Writing { .. })
    }
    fn blocks(&self, root: &Path, session: &WorkspaceSession) -> bool {
        self.running()
            || matches!(self, Self::Failed(failed) if failed.root == root && &failed.session == session)
    }
    fn poll(&mut self) -> Option<Result<SessionWritten, SessionWriteFailure>> {
        let Self::Writing { job, .. } = self else {
            return None;
        };
        match job.poll() {
            LatestJobPoll::Ready(written) => {
                *self = Self::Idle;
                Some(Ok(written))
            }
            LatestJobPoll::Failed(error) => {
                let Self::Writing { requested, .. } = std::mem::take(self) else {
                    unreachable!()
                };
                let failure = SessionWriteFailure {
                    root: requested.root.clone(),
                    profile_id: requested.session.profile_id,
                    error,
                };
                *self = Self::Failed(requested);
                Some(Err(failure))
            }
            _ => None,
        }
    }
    fn start(
        &mut self,
        requested: SessionWritten,
        context: &egui::Context,
        operation: impl FnOnce() -> Result<SessionWritten, String> + Send + 'static,
    ) -> Result<(), String> {
        assert!(
            !self.running(),
            "one workspace checkpoint writer owns its receipt"
        );
        let mut job = ExclusiveJob::default();
        match job.start_and_repaint("Remember Overleaf workspace", context, operation) {
            Ok(()) => {
                *self = Self::Writing { requested, job };
                Ok(())
            }
            Err(error) => {
                *self = Self::Failed(requested);
                Err(error)
            }
        }
    }
}
struct SessionWritten {
    root: PathBuf,
    session: WorkspaceSession,
}

/// Failure delivery retains the same root/session identity as successful
/// checkpoints. A completed former workspace must not replace today's status.
struct SessionWriteFailure {
    root: PathBuf,
    profile_id: [u8; 16],
    error: String,
}
impl OperationSummary for SessionWritten {
    fn completion_summary(&self) -> String {
        "Overleaf workspace remembered".into()
    }
}
impl State {
    pub(super) fn install(&mut self, session: WorkspaceSession, restored: bool) {
        self.saved = Some(session.clone());
        self.session = Some(session);
        self.restored = restored;
        self.departed = false;
        self.restoration = if restored {
            PageRestoration::WaitingForProject
        } else {
            PageRestoration::NotRequested
        };
        self.saved_sources.clear();
        self.recovery_sources.clear();
        self.checkpoint_waiting.clear();
        self.checkpoint_failed.clear();
        self.disk_untrusted.clear();
        if matches!(self.writer, SessionWriter::Failed(_)) {
            self.writer = SessionWriter::Idle;
        }
    }
    pub(super) fn restoring_pages(&self) -> bool {
        self.restoration.active()
    }
    #[cfg(all(feature = "desktop-ui-tests", target_os = "macos"))]
    pub(super) fn restoration_phase(&self) -> &'static str {
        match self.restoration {
            PageRestoration::NotRequested => "not_requested",
            PageRestoration::WaitingForProject => "waiting_for_project",
            PageRestoration::Opening(_) => "opening",
            PageRestoration::Complete => "complete",
        }
    }
    pub(super) fn active(&self) -> bool {
        self.session.as_ref().is_some_and(|s| s.offline)
    }
    #[cfg(any(test, feature = "desktop-ui-tests"))]
    pub(super) fn saved(&self) -> bool {
        !self.writer.running() && self.session == self.saved
    }
}

impl super::State {
    pub(super) fn offline_transition_busy(&self) -> bool {
        self.save.pending().is_some()
            || self.save.active()
            || self.io.is_running()
            || self.storage_job.is_running()
            || !self.queued.is_empty()
            || self.tree_pending.is_some()
            || self.downloads.has_pending()
            || !self.rebases.is_empty()
            || !self.offline.checkpoint_waiting.is_empty()
    }
}

impl EditorApp {
    pub(in crate::app) fn restoring_overleaf_pages(&self) -> bool {
        self.overleaf.offline.restoring_pages()
            && !self.overleaf.offline.departed
            && self
                .overleaf
                .connection
                .as_ref()
                .is_some_and(|c| c.root == self.workspace_root)
    }
    pub(crate) fn prepare_overleaf_shutdown(&mut self, context: &egui::Context) -> bool {
        let tabs: Vec<_> = self.tabs.ids().collect();
        self.prepare_overleaf_workspace_departure(&tabs, context)
            || self.wait_for_overleaf_session(context)
    }
    pub(super) fn checkpoint_discarded_overleaf_source(&mut self, id: &str) -> bool {
        if self.overleaf.offline.checkpoint_failed.contains(id) {
            return false;
        }
        let Some(connection) = &mut self.overleaf.connection else {
            return true;
        };
        let Some(document) = connection.model.document(id) else {
            return true;
        };
        let target = self
            .overleaf
            .offline
            .saved_sources
            .get(id)
            .unwrap_or(&document.baseline)
            .clone();
        let needs_write = document.source() != target
            || self
                .overleaf
                .offline
                .recovery_sources
                .get(id)
                .is_some_and(|source| source != &target);
        // A failed cache comparison can mean another window owns these bytes.
        // Discarding this editor must never roll that file back over its owner.
        let preserve_disk = self.overleaf.offline.disk_untrusted.contains(id);
        let released = if self.overleaf.offline.saved_sources.contains_key(id) {
            connection.model.set_draft(id, target).is_ok()
        } else {
            connection.model.release_native_draft(id)
        };
        let work =
            (released && needs_write && !preserve_disk && self.overleaf.offline.session.is_some())
                .then(|| {
                    let entry = connection.model.tree()?.entry(id)?.clone();
                    let source = connection.model.document(id)?.baseline.clone();
                    Some(MirrorWork {
                        checkpoint: true,
                        entry,
                        data: MirrorData::Document(source),
                        save: None,
                    })
                })
                .flatten();
        if let Some(work) = work {
            self.overleaf.queue(work);
        }
        released
    }

    pub(in crate::app) fn prepare_overleaf_workspace_departure(
        &mut self,
        tabs: &[u64],
        context: &egui::Context,
    ) -> bool {
        let ids: BTreeSet<_> = tabs
            .iter()
            .filter_map(|&tab| self.overleaf_entity_for_tab(tab).map(str::to_owned))
            .collect();
        if ids
            .iter()
            .any(|id| self.overleaf.offline.checkpoint_failed.contains(id))
        {
            self.cancel_workspace_switch(context);
            self.document_workflow.revoke_close();
            self.tabs.approved.clear();
            return true;
        }
        for id in ids {
            self.checkpoint_discarded_overleaf_source(&id);
        }
        let busy = self.overleaf.io.is_running()
            || !self.overleaf.offline.checkpoint_waiting.is_empty()
            || self.overleaf.queued.iter().any(|work| work.checkpoint);
        if busy {
            context.request_repaint();
        }
        busy
    }
    pub(in crate::app) fn overleaf_work_offline(
        &mut self,
        context: &egui::Context,
    ) -> Result<(), String> {
        if self.overleaf.offline_transition_busy() || self.document_workflow.save_in_flight() {
            return Err("Wait for the current file operation before going offline.".into());
        }
        let connection = self
            .overleaf
            .connection
            .as_ref()
            .ok_or("Open an Overleaf project first.")?;
        if !connection.storage.as_ref().is_some_and(|s| s.persistent())
            || connection.model.tree().is_none()
        {
            return Err("Keep a local copy of this project before working offline.".into());
        }
        let next = self
            .overleaf
            .epoch
            .checked_add(1)
            .ok_or("Connection generation exhausted.")?;
        let _ = self.overleaf.send(BrowserCommand::Disconnect {
            epoch: self.overleaf.epoch,
        });
        self.stop_overleaf_cursor();
        self.overleaf.epoch = next;
        self.overleaf.browser = BrowserLifecycle::Closed;
        self.overleaf.events = None;
        self.overleaf.sender = None;
        self.overleaf.browser.cancel_creation();
        self.overleaf.cancel_adoption();
        self.overleaf.show_browser = false;
        self.overleaf.authentication.set_required(false);
        self.overleaf.open_after = None;
        self.overleaf.navigation = None;
        self.overleaf.queued.retain(|work| work.checkpoint);
        self.overleaf.tree_pending = None;
        self.overleaf.downloads.clear();
        self.overleaf.rebases.clear();
        self.overleaf.review = None;
        if self.overleaf.blocked.is_empty() {
            self.overleaf.error = None;
        }
        self.overleaf.browser_status = activity::BrowserStatus::Disconnected;
        let connection = self.overleaf.connection.as_mut().unwrap();
        connection.model.reconnect(next);
        connection.transport = Transport::Offline;
        connection.hydration = Hydration::default();
        connection.status = "Offline".into();
        if let Some(session) = &mut self.overleaf.offline.session {
            session.offline = true;
        }
        self.poll_overleaf_session(context);
        context.request_repaint();
        Ok(())
    }

    pub(super) fn submit_overleaf_local_save(&mut self, tab: u64, context: &egui::Context) -> bool {
        let result = (|| -> Result<bool, String> {
            if self.overleaf.io.is_running()
                || self.overleaf.save.pending().is_some()
                || self.overleaf.save.active()
                || self.document_workflow.save_in_flight()
            {
                return Ok(false);
            }
            let entry = self
                .overleaf_entry_for_tab(tab)
                .cloned()
                .ok_or("This file is not in the saved Overleaf project.")?;
            if entry.kind != EntryKind::Doc || self.overleaf.blocked.contains(&entry.id) {
                return Err("Resolve the Overleaf conflict or Save As a local copy before saving this file.".into());
            }
            let connection = self
                .overleaf
                .connection
                .as_ref()
                .ok_or("Open the saved project first.")?;
            if !connection.storage.as_ref().is_some_and(|s| s.persistent()) {
                return Err("Keep a local copy before working offline.".into());
            }
            let document = self
                .document_for_tab(tab)
                .ok_or("This file is no longer open.")?;
            let request = document
                .prepare_save(connection.root.join(&entry.path), document.kind())
                .map_err(|e| e.to_string())?;
            let key = document.key();
            self.overleaf
                .connection
                .as_mut()
                .unwrap()
                .model
                .set_draft(&entry.id, request.source().into())?;
            self.overleaf.error = None;
            let source = request.source().to_owned();
            let continuation = (self.tabs.active_id() == Some(tab))
                .then(|| self.document_workflow.continuation_token())
                .flatten();
            let operation = self
                .document_workflow
                .begin_save(key, crate::workflow::SaveDomain::Offline)
                .map_err(str::to_owned)?;
            self.overleaf.save = SavePhase::WritingMirror {
                entity: entry.id.clone(),
                operation,
            };
            self.set_tab_autosave(tab, None);
            self.overleaf.queue(MirrorWork {
                checkpoint: false,
                entry: entry.clone(),
                data: MirrorData::Document(source),
                save: Some(RemoteSave {
                    operation,
                    id: entry.id,
                    wire: 0,
                    tab,
                    key,
                    request,
                    continuation,
                    started: Instant::now(),
                }),
            });
            context.request_repaint();
            Ok(true)
        })();
        match result {
            Ok(admitted) => admitted,
            Err(message) => {
                self.document_workflow.cancel_continuation();
                self.overleaf.error = Some(message.clone());
                self.notice = Some(Notice {
                    message,
                    kind: NoticeKind::Error,
                });
                false
            }
        }
    }

    /// Snapshot only small IDs. No document cloning or filesystem work during an idle frame.
    pub(in crate::app) fn poll_overleaf_session(&mut self, context: &egui::Context) {
        match self.overleaf.offline.writer.poll() {
            Some(Ok(written)) => {
                if self
                    .overleaf
                    .connection
                    .as_ref()
                    .is_some_and(|c| c.root == written.root)
                {
                    self.overleaf.offline.saved = Some(written.session);
                }
            }
            Some(Err(failure))
                if self
                    .overleaf
                    .connection
                    .as_ref()
                    .is_some_and(|connection| connection.root == failure.root) =>
            {
                // A failed atomic checkpoint can leave its disk outcome
                // uncertain. A newer session at this root must rewrite
                // its own metadata rather than trusting an older receipt.
                self.overleaf.offline.saved = None;
                if self
                    .overleaf
                    .offline
                    .session
                    .as_ref()
                    .is_some_and(|session| session.profile_id == failure.profile_id)
                {
                    self.overleaf.error = Some(format!(
                        "Could not remember the Overleaf workspace: {}",
                        failure.error
                    ));
                }
                // A subsequent state change may retry; no idle failure loop.
            }
            Some(Err(_)) => {}
            None => {}
        }
        let Some(connection) = &self.overleaf.connection else {
            return;
        };
        if !connection.storage.as_ref().is_some_and(|s| s.persistent()) {
            return;
        }
        if self.workspace_root == connection.root
            && !self.overleaf.offline.restoring_pages()
            && !self.overleaf.offline.departed
        {
            let ids = self
                .tabs
                .ids()
                .filter_map(|tab| self.overleaf_entity_for_tab(tab))
                .take(256);
            let changed_ids = self
                .overleaf
                .offline
                .session
                .as_ref()
                .is_some_and(|session| !session.open_documents.iter().map(String::as_str).eq(ids));
            let ids = changed_ids.then(|| {
                self.tabs
                    .ids()
                    .filter_map(|tab| self.overleaf_entity_for_tab(tab).map(str::to_owned))
                    .take(256)
                    .collect()
            });
            let active = self
                .tabs
                .active_id()
                .and_then(|tab| self.overleaf_entity_for_tab(tab));
            let active = self
                .overleaf
                .offline
                .session
                .as_ref()
                .filter(|s| s.active_document.as_deref() != active)
                .map(|_| active.map(str::to_owned));
            let preview = self
                .tabs
                .preview_id()
                .and_then(|tab| self.overleaf_entity_for_tab(tab));
            let preview = self
                .overleaf
                .offline
                .session
                .as_ref()
                .filter(|s| s.preview_document.as_deref() != preview)
                .map(|_| preview.map(str::to_owned));
            if let Some(session) = &mut self.overleaf.offline.session {
                session.has_page_state = true;
                if let Some(ids) = ids {
                    session.open_documents = ids;
                }
                if let Some(active) = active {
                    session.active_document = active;
                }
                if let Some(preview) = preview {
                    session.preview_document = preview;
                }
            }
        }
        let Some(session) = self.overleaf.offline.session.as_ref() else {
            return;
        };
        if self
            .overleaf
            .offline
            .writer
            .blocks(&connection.root, session)
            || self.overleaf.offline.saved.as_ref() == Some(session)
        {
            return;
        }
        let session = session.clone();
        let root = connection.root.clone();
        let project = connection.project.clone();
        let lease = connection.storage.clone();
        let requested = SessionWritten {
            root: root.clone(),
            session: session.clone(),
        };
        if let Err(error) = self
            .overleaf
            .offline
            .writer
            .start(requested, context, move || {
                let _lease = lease;
                session.write(&root, &project)?;
                Ok(SessionWritten { root, session })
            })
        {
            self.overleaf.error = Some(error);
        }
    }

    pub(in crate::app) fn restore_overleaf_pages(&mut self, context: &egui::Context) {
        if !self.overleaf.offline.restoring_pages() {
            return;
        }
        let Some(connection) = &self.overleaf.connection else {
            return;
        };
        if self.workspace_root != connection.root {
            return;
        }
        let Some(tree) = connection.model.tree() else {
            return;
        };
        let root = connection.root.clone();
        if matches!(
            self.overleaf.offline.restoration,
            PageRestoration::WaitingForProject
        ) {
            let session = self.overleaf.offline.session.clone().unwrap();
            let requested = self
                .overleaf
                .reopen_relative
                .take()
                .and_then(|(_, _, path)| {
                    path.to_str()
                        .and_then(|path| tree.id_for_path(path))
                        .map(str::to_owned)
                });
            let mut ids: Vec<_> = if !session.has_page_state {
                tree.main.iter().cloned().collect()
            } else {
                session.open_documents.clone()
            };
            if let Some(id) = requested.as_ref()
                && !ids.contains(id)
            {
                ids.push(id.clone());
            }
            let paths: VecDeque<_> = ids
                .into_iter()
                .filter_map(|id| tree.entry(&id).map(|e| (id, root.join(&e.path))))
                .collect();
            self.overleaf.offline.restoration = PageRestoration::Opening(RestorePages {
                paths,
                active: requested.or(session.active_document),
                preview: session.preview_document,
                frame: None,
            });
        }
        let plan = self.overleaf.offline.restoration.plan_mut().unwrap();
        let frame = context.cumulative_frame_nr();
        if plan.frame == Some(frame) {
            return;
        }
        plan.frame = Some(frame);
        let paths: Vec<_> = plan
            .paths
            .drain(..plan.paths.len().min(RESTORE_PAGES_PER_FRAME))
            .collect();
        let mut awaiting_download = false;
        for (id, _) in paths {
            let Some(connection) = &self.overleaf.connection else {
                break;
            };
            let Some(entry) = connection.model.tree().and_then(|tree| tree.entry(&id)) else {
                continue;
            };
            let path = root.join(&entry.path);
            if !path.is_file() && connection.hydration.pending() && !self.overleaf.offline.active()
            {
                self.overleaf
                    .offline
                    .restoration
                    .plan_mut()
                    .unwrap()
                    .paths
                    .push_back((id, path));
                awaiting_download = true;
                continue;
            }
            if !path.is_file() || self.overleaf_path_owned_by_other_entity(&path, &id) {
                continue;
            }
            self.overleaf.admitted_path = Some(path.clone());
            let opened = self.open_workspace_document(path, context);
            self.overleaf.admitted_path = None;
            if opened && let Some(tab) = self.tabs.active_id() {
                self.overleaf.tab_entities.insert(tab, id);
                self.set_tab_workspace(tab, root.clone());
            }
        }
        if !self
            .overleaf
            .offline
            .restoration
            .plan()
            .unwrap()
            .paths
            .is_empty()
        {
            if !awaiting_download {
                context.request_repaint();
            }
            return;
        }
        let plan = self.overleaf.offline.restoration.finish();
        if let Some(tab) = self
            .overleaf
            .tab_entities
            .iter()
            .find_map(|(&tab, id)| (Some(id) == plan.preview.as_ref()).then_some(tab))
        {
            self.select_preview_tab(tab, context);
        }
        let active_id = plan.active.as_ref();
        if let Some(tab) = self
            .overleaf
            .tab_entities
            .iter()
            .find_map(|(&tab, id)| (Some(id) == active_id).then_some(tab))
        {
            self.activate_tab(tab, context);
        }
        self.overleaf.open_after = None;
        self.overleaf.navigation = None;
    }

    pub(super) fn open_cached_overleaf_path(
        &mut self,
        path: &Path,
        context: &egui::Context,
    ) -> bool {
        let Some(entry) = self.overleaf_entry(path).cloned() else {
            return false;
        };
        if entry.kind == EntryKind::Folder {
            return true;
        }
        if self.overleaf_path_owned_by_other_entity(path, &entry.id) {
            self.overleaf.error = Some("The original Overleaf document is no longer available at this path. Save a local copy or close its tab before opening the replacement.".into());
            return true;
        }
        if !path.is_file() {
            self.overleaf.error = Some(
                "This file is not available locally. Reconnect to Overleaf to refresh it.".into(),
            );
            return true;
        }
        self.overleaf.admitted_path = Some(path.to_owned());
        let opened = self.open_workspace_document(path.to_owned(), context);
        self.overleaf.admitted_path = None;
        if opened && let Some(tab) = self.tabs.active_id() {
            self.overleaf.tab_entities.insert(tab, entry.id);
            self.set_tab_workspace(tab, self.workspace_root.clone());
        }
        true
    }

    pub(in crate::app) fn wait_for_overleaf_session(&mut self, context: &egui::Context) -> bool {
        self.poll_overleaf_session(context);
        self.overleaf.offline.writer.running()
    }
}
