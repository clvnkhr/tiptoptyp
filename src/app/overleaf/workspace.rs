//! Working-copy ownership and the fresh-server-to-disk editing barrier.
use super::*;

/// Fresh network content is editable only after both metadata and every file
/// have been persisted. Transport connectivity alone never opens this barrier.
#[derive(Default)]
pub(super) struct Hydration {
    pub(super) session: u64,
    phase: HydrationPhase,
}
#[derive(Default)]
enum HydrationPhase {
    #[default]
    Unrestricted,
    AwaitingTree,
    Copying {
        tree: Tree,
        metadata: MetadataCopy,
        files: BTreeMap<String, Entry>,
    },
    Ready {
        tree: Tree,
        files: BTreeMap<String, Entry>,
    },
}
#[derive(PartialEq, Eq)]
enum MetadataCopy {
    Pending,
    Written,
}
impl Hydration {
    pub(super) fn pending(&self) -> bool {
        matches!(
            self.phase,
            HydrationPhase::AwaitingTree | HydrationPhase::Copying { .. }
        )
    }
    pub(super) fn waiting() -> Self {
        Self {
            session: 0,
            phase: HydrationPhase::AwaitingTree,
        }
    }
    pub(super) fn restart(&mut self) {
        self.session = self.session.wrapping_add(1);
        self.phase = HydrationPhase::AwaitingTree;
    }
    pub(super) fn reset(&mut self, tree: &Tree) {
        let mut files = match std::mem::take(&mut self.phase) {
            HydrationPhase::Copying { files, .. } | HydrationPhase::Ready { files, .. } => files,
            _ => BTreeMap::new(),
        };
        files.retain(|id, entry| tree.entry(id) == Some(entry));
        self.phase = HydrationPhase::Copying {
            tree: tree.clone(),
            metadata: MetadataCopy::Pending,
            files,
        };
    }
    pub(super) fn accept_tree(&mut self, written: &Tree) {
        if let HydrationPhase::Copying { tree, metadata, .. } = &mut self.phase
            && tree == written
        {
            *metadata = MetadataCopy::Written;
        }
    }
    pub(super) fn accept_file(&mut self, entry: &Entry) {
        match &mut self.phase {
            HydrationPhase::Copying { tree, files, .. } | HydrationPhase::Ready { tree, files }
                if tree.entry(&entry.id) == Some(entry) =>
            {
                files.insert(entry.id.clone(), entry.clone());
            }
            _ => {}
        }
    }
    fn complete(&self) -> bool {
        match &self.phase {
            HydrationPhase::Ready { .. } => true,
            HydrationPhase::Copying {
                tree,
                metadata: MetadataCopy::Written,
                files,
            } => tree
                .entries()
                .iter()
                .filter(|entry| entry.kind != EntryKind::Folder)
                .all(|entry| files.get(&entry.id) == Some(entry)),
            _ => false,
        }
    }
    pub(super) fn finish(&mut self) {
        assert!(
            self.complete(),
            "fresh Overleaf metadata and files must be persisted before editing"
        );
        self.phase = match std::mem::take(&mut self.phase) {
            HydrationPhase::Copying { tree, files, .. } => HydrationPhase::Ready { tree, files },
            other => other,
        };
    }
    #[cfg(all(feature = "desktop-ui-tests", target_os = "macos"))]
    pub(super) fn phase(&self) -> &'static str {
        match self.phase {
            HydrationPhase::Unrestricted => "unrestricted",
            HydrationPhase::AwaitingTree => "awaiting_tree",
            HydrationPhase::Copying { .. } => "copying",
            HydrationPhase::Ready { .. } => "ready",
        }
    }
}

pub(super) struct PreparedStorage {
    pub(super) epoch: u64,
    pub(super) project: ProjectUrl,
    pub(super) storage: Arc<crate::overleaf::StorageLease>,
    pub(super) pending_storage: Option<crate::settings::OverleafStoragePreference>,
    pub(super) session: crate::overleaf::workspace_session::WorkspaceSession,
    pub(super) model: Option<Model>,
}
impl OperationSummary for PreparedStorage {
    fn completion_summary(&self) -> String {
        "Overleaf workspace prepared".into()
    }
}

impl EditorApp {
    pub(super) fn open_stored_overleaf_project(
        &mut self,
        project: &ProjectUrl,
        root: &Path,
        relative: Option<PathBuf>,
        context: &egui::Context,
    ) -> Result<(), String> {
        if !self.overleaf.offline.disk_untrusted.is_empty()
            && self
                .overleaf
                .connection
                .as_ref()
                .is_some_and(|c| c.root == root)
            && self.tabs.ids().any(|tab| {
                self.document_for_tab(tab)
                    .and_then(|d| d.path().as_ref())
                    .is_some_and(|p| p.starts_with(root))
            })
        {
            return Err("Close this project's pages before reopening its local copy. Save As keeps a separate copy of unsaved edits.".into());
        }
        let base = root
            .parent()
            .ok_or("The saved project folder is invalid.")?;
        if project.mirror_root(base) != root {
            return self.open_overleaf_project_url(project.url(), relative, context);
        }
        let preference = crate::settings::OverleafStoragePreference {
            keep_local_copy: true,
            directory: Some(base.to_string_lossy().into_owned()),
        };
        self.overleaf.query = project.url().into();
        self.overleaf.pending_storage = None;
        self.overleaf.storage_draft =
            Some(storage_ui::StorageDraft::new(project.url(), &preference));
        self.overleaf.reopening_workspace = true;
        let result = self.apply_overleaf_storage(context);
        self.overleaf.reopening_workspace = false;
        result?;
        self.overleaf.reopen_relative =
            relative.map(|path| (project.url().into(), self.overleaf.epoch, path));
        Ok(())
    }
    pub(in crate::app) fn overleaf_persistent_path(&self, path: &Path) -> bool {
        self.overleaf.connection.as_ref().is_some_and(|c| {
            path.starts_with(&c.root) && c.storage.as_ref().is_some_and(|s| s.persistent())
        })
    }
    pub(in crate::app) fn stage_overleaf_workspace(
        &mut self,
        folder: &Path,
        context: &egui::Context,
    ) {
        let project_query = ProjectUrl::parse(&self.overleaf.query)
            .ok()
            .map(|p| p.url().to_owned());
        // A newly selected workspace owns a distinct browser profile from the
        // first login page, before a project has been chosen.
        self.overleaf.browser = BrowserLifecycle::Closed;
        self.overleaf.browser.cancel_creation();
        self.overleaf.events = None;
        self.overleaf.sender = None;
        if let Some(connection) = &mut self.overleaf.connection {
            // The chooser's new login view cannot publish through the old
            // project's previously authenticated connection.
            connection.transport = Transport::Offline;
            connection.model.reconnect(self.overleaf.epoch);
            connection.status = "Reconnect required".into();
        }
        self.overleaf.browser_profile = Some(crate::overleaf::workspace_session::new_profile_id());
        self.begin_overleaf(context);
        self.overleaf.ensure_browsing(context);
        if let Some(query) = project_query {
            self.overleaf.query = query;
        }
        self.overleaf.error = None;
        self.overleaf.pending_storage = Some(crate::settings::OverleafStoragePreference {
            keep_local_copy: true,
            directory: Some(folder.to_string_lossy().into_owned()),
        });
        self.overleaf.storage_draft = None;
        let project = ProjectUrl::parse(&self.overleaf.query).ok();
        self.overleaf.address_draft = project.is_some();
        if let Err(error) = self.overleaf.ensure_storage_draft(
            project.as_ref(),
            self.pending_settings.as_ref().unwrap_or(&self.settings),
        ) {
            self.overleaf.error = Some(error);
        }
    }
    pub(super) fn arm_overleaf_navigation(&mut self) {
        self.overleaf.navigation = Some(NavigationOwner {
            tab: self.tabs.active_id(),
            key: (!self.tabs.is_empty()).then(|| self.document().key()),
            workspace: self.workspace_root.clone(),
        });
    }
    pub(super) fn overleaf_navigation_current(&self) -> bool {
        self.workspace_switch.adopting_overleaf(self.overleaf.epoch)
            || self.overleaf.navigation.as_ref().is_none_or(|owner| {
                owner.tab == self.tabs.active_id()
                    && owner.key == (!self.tabs.is_empty()).then(|| self.document().key())
                    && owner.workspace == self.workspace_root
            })
    }
    pub(in crate::app) fn resume_overleaf_workspace_open(&mut self, _context: &egui::Context) {
        self.remember_workspace(&self.workspace_root.clone());
        if self.overleaf.offline.restoring_pages() {
            self.restore_overleaf_pages(_context);
            return;
        }
        self.arm_overleaf_navigation();
        let path = self.overleaf.open_after.as_ref().and_then(|id| {
            let connection = self.overleaf.connection.as_ref()?;
            let entry = connection.model.tree()?.entry(id)?;
            Some(connection.root.join(&entry.path))
        });
        if let Some(path) = path {
            // Approval may outlast the first mirror download. Ask for fresh
            // bytes again, with the newly committed workspace as owner.
            self.open_overleaf_path(&path);
        }
    }
    pub(in crate::app) fn resume_workspace_overleaf_connection(
        &mut self,
        url: &str,
        preference: crate::settings::OverleafStoragePreference,
        context: &egui::Context,
    ) {
        self.overleaf.query = url.into();
        self.overleaf.storage_draft = Some(storage_ui::StorageDraft::new(url, &preference));
        let relative = self
            .overleaf
            .reopen_relative
            .take()
            .filter(|(project, _, _)| project == url);
        match self.apply_overleaf_storage(context) {
            Ok(()) => {
                self.overleaf.reopen_relative =
                    relative.map(|(project, _, path)| (project, self.overleaf.epoch, path))
            }
            Err(error) => self.overleaf.error = Some(error),
        }
    }
    pub(in crate::app) fn overleaf_workspace_matches(&self, root: &Path, epoch: u64) -> bool {
        self.overleaf.epoch == epoch
            && self
                .overleaf
                .connection
                .as_ref()
                .is_some_and(|connection| connection.root == root)
    }
    pub(in crate::app) fn cancel_overleaf_workspace_open(&mut self, epoch: u64) {
        if self.overleaf.epoch == epoch {
            self.overleaf.open_after = None;
            self.overleaf.navigation = None;
            self.overleaf.reopen_relative = None;
            self.overleaf.cancel_adoption();
        }
    }
    pub(in crate::app) fn depart_overleaf_workspace(&mut self, root: &Path, connecting: bool) {
        if self
            .overleaf
            .connection
            .as_ref()
            .is_some_and(|connection| connection.root == root)
        {
            self.overleaf.offline.departed = true;
            self.stop_overleaf_cursor();
            self.overleaf.open_after = None;
            self.overleaf.navigation = None;
            if !connecting {
                self.overleaf.reopen_relative = None;
            }
            self.overleaf.cancel_adoption();
        }
    }
    pub(in crate::app) fn overleaf_source_locked(&self) -> bool {
        if self.overleaf_reopen_locked() {
            return true;
        }
        let Some(path) = self.document().path() else {
            return false;
        };
        if let Some(connection) = &self.overleaf.connection
            && path.starts_with(&connection.root)
        {
            if self.overleaf.offline.active() {
                return self.overleaf_file_operation_pending()
                    || self
                        .tabs
                        .active_id()
                        .and_then(|tab| self.overleaf_entry_for_tab(tab))
                        .is_none_or(|entry| {
                            entry.kind != EntryKind::Doc
                                || connection.model.document(&entry.id).is_none()
                                || self.overleaf.blocked.contains(&entry.id)
                        });
            }
            return self.overleaf.authentication.required()
                || !connection.online()
                || connection.hydration.pending()
                || self.overleaf_file_operation_pending()
                || self
                    .tabs
                    .active_id()
                    .and_then(|tab| self.overleaf_entry_for_tab(tab))
                    .is_none_or(|entry| {
                        self.overleaf.blocked.contains(&entry.id)
                            || self.overleaf.rebases.contains_key(&entry.id)
                    });
        }
        self.is_overleaf_path(path)
    }
    pub(in crate::app) fn open_overleaf_project_url(
        &mut self,
        url: &str,
        relative: Option<PathBuf>,
        context: &egui::Context,
    ) -> Result<(), String> {
        let project = ProjectUrl::parse(url)?;
        self.overleaf.query = project.url().into();
        self.overleaf.storage_draft = None;
        self.overleaf.ensure_storage_draft(
            Some(&project),
            self.pending_settings.as_ref().unwrap_or(&self.settings),
        )?;
        self.connect_overleaf(context)?;
        self.overleaf.reopen_relative =
            relative.map(|path| (project.url().into(), self.overleaf.epoch, path));
        Ok(())
    }
    pub(in crate::app) fn poll_overleaf_storage(&mut self, context: &egui::Context) {
        self.poll_overleaf_reopen(context);
        #[cfg(target_os = "macos")]
        match self.overleaf.passwords_job.poll() {
            LatestJobPoll::Ready(_) => {}
            LatestJobPoll::Failed(error) => self.overleaf.error = Some(error),
            _ => {}
        }
        if let crate::workflow::DialogPoll::Ready {
            request,
            selection: Some(folder),
        } = crate::workflow::poll_dialog(&mut self.overleaf.folder_picker, context)
            && let Some(draft) = &mut self.overleaf.storage_draft
            && draft.project_url == request
        {
            draft.directory = folder.path().to_string_lossy().into_owned();
            if self.overleaf.pending_storage.is_some() {
                self.overleaf.pending_storage = Some(draft.raw_preference());
            }
        }
        match self.overleaf.storage_job.poll() {
            LatestJobPoll::Ready(prepared) if prepared.epoch == self.overleaf.epoch => {
                let root = prepared.storage.root().to_owned();
                let restored = prepared.model.is_some();
                self.overleaf.browser_profile = Some(prepared.session.profile_id);
                self.overleaf.offline.install(prepared.session, restored);
                self.overleaf
                    .accept_pending_storage(&prepared.pending_storage);
                self.remember_overleaf_return_workspace();
                self.overleaf.connection = Some(Connection {
                    model: prepared
                        .model
                        .unwrap_or_else(|| Model::for_project(prepared.epoch, &prepared.project)),
                    project: prepared.project,
                    root: root.clone(),
                    storage: Some(prepared.storage),
                    hydration: Hydration::waiting(),
                    status: "Connecting to Overleaf…".into(),
                    transport: Transport::Offline,
                });
                self.overleaf.workspace = None;
                self.overleaf.tab_entities.clear();
                self.overleaf.blocked.clear();
                self.overleaf.rebases.clear();
                self.overleaf.blocked.extend(
                    self.overleaf
                        .connection
                        .as_ref()
                        .unwrap()
                        .model
                        .conflict_ids()
                        .map(str::to_owned),
                );
                if let Some(tree) = self
                    .overleaf
                    .connection
                    .as_ref()
                    .and_then(|c| c.model.tree())
                {
                    for entry in tree.entries() {
                        if let Some(document) = self
                            .overleaf
                            .connection
                            .as_ref()
                            .and_then(|c| c.model.document(&entry.id))
                        {
                            self.overleaf
                                .offline
                                .recovery_sources
                                .insert(entry.id.clone(), document.source().to_owned());
                            if document.is_dirty() || document.conflict().is_some() {
                                self.overleaf
                                    .offline
                                    .saved_sources
                                    .insert(entry.id.clone(), document.source().to_owned());
                            }
                            if document.conflict().is_some() {
                                self.overleaf.blocked.insert(entry.id.clone());
                            }
                        }
                    }
                }
                if restored {
                    let connection = self.overleaf.connection.as_ref().unwrap();
                    if let Some(tree) = connection.model.tree() {
                        self.overleaf.workspace = Some(WorkspaceTree::from_shared(Arc::new(
                            remote_snapshot(&connection.root, tree),
                        )));
                    }
                    if root != self.workspace_root {
                        self.request_workspace_switch(
                            crate::app::workspace_switch::Destination::Overleaf {
                                root: root.clone(),
                                epoch: self.overleaf.epoch,
                            },
                            context,
                        );
                    } else {
                        self.restore_overleaf_pages(context);
                    }
                }
                if self.overleaf.offline.active() && restored {
                    let connection = self.overleaf.connection.as_mut().unwrap();
                    connection.hydration = Hydration::default();
                    connection.status = "Offline".into();
                    self.overleaf.browser.cancel_creation();
                    self.overleaf.browser = BrowserLifecycle::Closed;
                    self.overleaf.events = None;
                    self.overleaf.sender = None;
                    self.overleaf.cancel_adoption();
                    self.hide_overleaf(context);
                } else {
                    self.prepare_overleaf_browser(context);
                }
            }
            LatestJobPoll::Failed(error) => {
                self.overleaf.error = Some(error);
            }
            _ => {}
        }
    }
    fn remember_overleaf_return_workspace(&mut self) {
        // The latest local workspace is the useful destination after closing a
        // temporary project. Never replace it with another managed root that
        // may itself be retired as the connection changes.
        if !self.is_overleaf_path(&self.workspace_root) {
            self.overleaf.return_workspace = Some(self.workspace_root.clone());
        }
    }
    pub(in crate::app) fn finish_overleaf_hydration(&mut self, context: &egui::Context) {
        let quiet = !self.overleaf.io.is_running()
            && self.overleaf.tree_pending.is_none()
            && self.overleaf.queued.is_empty()
            && !self.overleaf.downloads.has_pending();
        if let Some(connection) = &mut self.overleaf.connection
            && connection.online()
            && !self.overleaf.authentication.required()
            && connection.hydration.pending()
            && quiet
            && connection.hydration.complete()
        {
            connection.hydration.finish();
            connection.status = "Ready".into();
            self.overleaf.authentication.allow_return();
            if let Some(browser) = self.overleaf.browser.as_ref() {
                browser.project_ready();
            }
            context.request_repaint();
        }
    }
    pub(in crate::app) fn retire_unused_overleaf(&mut self, context: &egui::Context) {
        let Some(connection) = &self.overleaf.connection else {
            return;
        };
        if connection
            .storage
            .as_ref()
            .is_none_or(|storage| storage.persistent())
            || (self.workspace_root == connection.root
                && (connection.online() || self.overleaf.browser.is_some()))
            || (self.overleaf.visible & self.overleaf.browser.as_ref().is_some())
            || self.overleaf.storage_job.is_running()
            || self.overleaf.awaiting_project()
            || connection.hydration.pending()
            || self.overleaf.save.active()
            || self.overleaf.save.pending().is_some()
            || self.overleaf.io.is_running()
            || !self.overleaf.queued.is_empty()
            || self.overleaf.downloads.has_pending()
            || connection.model.has_unsynced()
            || self
                .tabs
                .ids()
                .any(|tab| self.tab_workspace(tab) == Some(connection.root.as_path()))
            || self.tabs.ids().any(|tab| {
                self.document_for_tab(tab)
                    .and_then(|doc| doc.path().as_ref())
                    .is_some_and(|path| path.starts_with(&connection.root))
            })
        {
            return;
        }
        let root = connection.root.clone();
        let _ = self.overleaf.send(BrowserCommand::Disconnect {
            epoch: self.overleaf.epoch,
        });
        self.overleaf.epoch = self.overleaf.epoch.saturating_add(1);
        self.overleaf.browser = BrowserLifecycle::Closed;
        self.overleaf.events = None;
        self.overleaf.sender = None;
        self.overleaf.workspace = None;
        self.overleaf.connection = None;
        self.overleaf.presence_cache = None;
        self.overleaf.tab_entities.clear();
        if self.workspace_root == root {
            self.workspace_root = self
                .overleaf
                .return_workspace
                .take()
                .filter(|path| path.is_dir())
                .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));
            self.workspace = None;
            self.refresh_workspace();
        }
        context.request_repaint();
    }
    pub(in crate::app) fn choose_overleaf_directory(&mut self, context: &egui::Context) {
        if self.overleaf.folder_picker.is_some() {
            return;
        }
        let Some(draft) = &self.overleaf.storage_draft else {
            return;
        };
        let mut dialog = AsyncFileDialog::new().set_title("Overleaf local copies");
        #[cfg(any(target_os = "macos", target_os = "windows"))]
        if let Some(parent) =
            crate::native_window::for_viewport(context, scoped_child_viewport_id(context, SALT))
        {
            dialog = dialog.set_parent(&parent);
        }
        if !draft.directory.is_empty() {
            dialog = dialog.set_directory(&draft.directory);
        }
        self.overleaf.folder_picker = Some(crate::workflow::PendingDialog::new(
            draft.project_url.clone(),
            dialog.pick_folder(),
        ));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn hydration_requires_the_current_tree_receipt_and_every_current_file() {
        let source = tree("main.tex");
        let mut state = Hydration::waiting();
        state.reset(&source);
        for entry in source.entries() {
            state.accept_file(entry);
        }
        assert!(state.pending());
        assert!(
            !state.complete(),
            "file receipts alone cannot release the metadata barrier"
        );
        state.accept_tree(&source);
        assert!(state.complete());
        state.finish();
        assert!(!state.pending());
        state.finish();
        assert!(
            state.complete(),
            "a repeated finish cannot erase the ready snapshot"
        );
        let changed = tree("renamed.tex");
        state.reset(&changed);
        state.accept_tree(&source);
        assert!(
            !state.complete(),
            "old metadata cannot admit a changed tree"
        );
        state.accept_tree(&changed);
        assert!(!state.complete(), "a renamed file needs its own receipt");
        state.accept_file(changed.entry("a").unwrap());
        assert!(state.complete(), "unchanged file receipts remain reusable");
        state.restart();
        assert!(state.pending());
        assert!(!state.complete());
    }
    fn tree(path: &str) -> Tree {
        Tree::new(
            "Paper".into(),
            Some("a".into()),
            vec![
                Entry {
                    id: "a".into(),
                    path: path.into(),
                    kind: EntryKind::Doc,
                },
                Entry {
                    id: "b".into(),
                    path: "image.png".into(),
                    kind: EntryKind::File,
                },
            ],
        )
        .unwrap()
    }
    #[test]
    fn editing_requires_tree_and_every_current_file_written() {
        let initial = tree("main.tex");
        let mut state = Hydration::waiting();
        state.reset(&initial);
        state.accept_file(&initial.entries()[0]);
        state.accept_file(&initial.entries()[1]);
        assert!(!state.complete());
        state.accept_tree(&initial);
        assert!(state.complete());
        state.reset(&tree("renamed.tex"));
        assert!(!state.complete());
    }
    #[test]
    fn assets_and_stale_renamed_receipts_cannot_open_the_gate() {
        let old = tree("main.tex");
        let current = tree("new.tex");
        let mut state = Hydration::waiting();
        state.reset(&current);
        state.accept_tree(&old);
        state.accept_file(old.entry("a").unwrap());
        assert!(!state.complete());
        state.accept_tree(&current);
        state.accept_file(current.entry("a").unwrap());
        assert!(!state.complete());
        state.accept_file(current.entry("b").unwrap());
        assert!(state.complete());
    }
}

#[cfg(test)]
mod controller_tests {
    use super::*;
    const ID: &str = "0123456789abcdef01234567";
    fn project_event(epoch: u64) -> BrowserEvent {
        BrowserEvent::Project {
            epoch,
            project_id: ID.into(),
            name: "Paper".into(),
            root_doc: Some("a".into()),
            entries: vec![
                Entry {
                    id: "a".into(),
                    path: "main.tex".into(),
                    kind: EntryKind::Doc,
                },
                Entry {
                    id: "b".into(),
                    path: "figure.png".into(),
                    kind: EntryKind::File,
                },
            ],
        }
    }
    fn write_received(app: &mut EditorApp, context: &egui::Context) {
        let root = app.overleaf.connection.as_ref().unwrap().root.clone();
        let mirror = Mirror::open(&root).unwrap();
        let tree = app.overleaf.tree_pending.take();
        if let Some(tree) = &tree {
            mirror.apply_tree(tree).unwrap();
        }
        let files = app
            .overleaf
            .queued
            .drain(..)
            .map(|work| {
                let result = match &work.data {
                    MirrorData::Document(source) => mirror.write_document(&work.entry, source),
                    MirrorData::File(bytes) => mirror.write_file(&work.entry, bytes),
                };
                (work, result)
            })
            .collect();
        app.apply_overleaf_mirror(
            Mirrored {
                origin: MirrorOrigin {
                    root: root.to_path_buf(),
                    profile: app.overleaf.browser_profile,
                },
                heads: BTreeMap::new(),
                documents: BTreeMap::new(),
                epoch: app.overleaf.epoch,
                hydration_session: app.overleaf.connection.as_ref().unwrap().hydration.session,
                tree,
                error: None,
                files,
            },
            context,
        );
        app.finish_overleaf_hydration(context);
    }
    #[test]
    fn opening_and_reconnecting_wait_for_fresh_documents_assets_and_disk_receipts() {
        let context = egui::Context::default();
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().canonicalize().unwrap();
        let project = ProjectUrl::parse(&format!("https://www.overleaf.com/project/{ID}")).unwrap();
        let mut app = EditorApp::dormant_for_tests(&context, root.clone());
        app.document_mut().replace_loaded_unprojected(
            "stale disk copy".into(),
            root.join("main.tex"),
            DocumentKind::Tex,
            None,
        );
        app.overleaf.epoch = 1;
        app.overleaf.connection = Some(Connection {
            model: Model::for_project(1, &project),
            project,
            root: root.clone(),
            storage: None,
            status: "Connecting".into(),
            transport: Transport::Offline,
            hydration: Hydration::waiting(),
        });
        for epoch in [1, 2] {
            if epoch == 2 {
                app.overleaf.epoch = epoch;
                let c = app.overleaf.connection.as_mut().unwrap();
                c.model.reconnect(epoch);
                c.transport = Transport::Offline;
                c.hydration = Hydration::waiting();
            }
            assert!(app.overleaf_source_locked());
            app.update_overleaf_event(project_event(epoch), &context)
                .unwrap();
            app.update_overleaf_event(
                BrowserEvent::Status {
                    epoch,
                    state: "connected".into(),
                    message: None,
                },
                &context,
            )
            .unwrap();
            app.finish_overleaf_hydration(&context);
            assert!(app.overleaf_source_locked(), "connected is not hydrated");
            app.update_overleaf_event(
                BrowserEvent::Document {
                    epoch,
                    id: "a".into(),
                    path: "main.tex".into(),
                    source: "server version".into(),
                },
                &context,
            )
            .unwrap();
            assert!(
                !app.overleaf.queued.is_empty(),
                "unchanged reconnect source still needs a fresh receipt"
            );
            write_received(&mut app, &context);
            assert!(
                app.overleaf_source_locked(),
                "a missing asset keeps editing blocked"
            );
            app.update_overleaf_event(
                BrowserEvent::File {
                    epoch,
                    id: "b".into(),
                    path: "figure.png".into(),
                    base64: "eA==".into(),
                },
                &context,
            )
            .unwrap();
            assert!(app.overleaf_source_locked(), "downloaded is not written");
            write_received(&mut app, &context);
            assert!(!app.overleaf_source_locked());
            assert_eq!(app.document().source(), "server version");
            assert_eq!(
                std::fs::read_to_string(root.join("main.tex")).unwrap(),
                "server version"
            );
        }
    }
}

#[cfg(test)]
mod lifetime_tests {
    use super::*;
    use crate::overleaf::{StorageLease, StorageOptions};
    use std::sync::Weak;

    const URL: &str = "https://www.overleaf.com/project/0123456789abcdef01234567";

    fn temporary_workspace(
        context: &egui::Context,
        local: &Path,
        entries: Vec<Entry>,
    ) -> (EditorApp, Weak<StorageLease>) {
        let mut app = EditorApp::dormant_for_tests(context, local.to_owned());
        let project = ProjectUrl::parse(URL).unwrap();
        let storage = StorageLease::prepare(
            &project,
            &StorageOptions {
                keep_local_copy: false,
                directory: None,
            },
            local,
        )
        .unwrap();
        let weak = Arc::downgrade(&storage);
        let root = storage.root().to_owned();
        let mut model = Model::for_project(1, &project);
        model
            .apply(BrowserEvent::Project {
                epoch: 1,
                project_id: project.id().into(),
                name: "Project".into(),
                entries,
                root_doc: None,
            })
            .unwrap();
        app.overleaf.epoch = 1;
        app.overleaf.return_workspace = Some(local.to_owned());
        app.overleaf.connection = Some(Connection {
            model,
            project,
            root: root.clone(),
            storage: Some(storage),
            status: "Ready".into(),
            transport: Transport::Online,
            hydration: Hydration::default(),
        });
        app.workspace_root = root;
        (app, weak)
    }

    #[test]
    fn active_empty_and_asset_only_projects_do_not_need_a_document_tab_to_stay_open() {
        let directory = tempfile::tempdir().unwrap();
        let local = directory.path().canonicalize().unwrap();
        let context = egui::Context::default();
        for entries in [
            Vec::new(),
            vec![Entry {
                id: "figure".into(),
                path: "figure.png".into(),
                kind: EntryKind::File,
            }],
        ] {
            let (mut app, weak) = temporary_workspace(&context, &local, entries);
            let root = app.workspace_root.clone();
            app.retire_unused_overleaf(&context);
            assert!(
                app.overleaf.connection.is_some(),
                "the Explorer is still in use"
            );
            assert_eq!(app.workspace_root, root);
            assert!(weak.upgrade().is_some());
        }
    }

    #[test]
    fn explicit_disconnect_releases_a_tabless_project_and_restores_the_local_workspace() {
        let directory = tempfile::tempdir().unwrap();
        let local = directory.path().canonicalize().unwrap();
        let context = egui::Context::default();
        let (mut app, weak) = temporary_workspace(&context, &local, Vec::new());
        app.overleaf.connection.as_mut().unwrap().transport = Transport::Offline;
        // The Disconnect action has also dropped the browser. A network loss
        // with its browser still alive retains the active project instead.
        assert!(app.overleaf.browser.is_none());
        app.retire_unused_overleaf(&context);
        assert!(app.overleaf.connection.is_none());
        assert_eq!(app.workspace_root, local);
        assert!(app.overleaf.return_workspace.is_none());
        assert!(
            weak.upgrade().is_none(),
            "the last lease was released for cleanup"
        );
    }

    #[test]
    fn switching_workspace_releases_a_tabless_project_without_replacing_the_new_workspace() {
        let directory = tempfile::tempdir().unwrap();
        let local = directory.path().canonicalize().unwrap();
        let next = local.join("next");
        std::fs::create_dir(&next).unwrap();
        let context = egui::Context::default();
        let (mut app, weak) = temporary_workspace(&context, &local, Vec::new());
        app.workspace_root = next.clone();
        app.retire_unused_overleaf(&context);
        assert!(app.overleaf.connection.is_none());
        assert_eq!(app.workspace_root, next);
        assert!(weak.upgrade().is_none());
    }

    #[test]
    fn return_workspace_tracks_the_latest_local_workspace_and_never_a_managed_copy() {
        let directory = tempfile::tempdir().unwrap();
        let local = directory.path().canonicalize().unwrap();
        let next = local.join("next");
        std::fs::create_dir(&next).unwrap();
        let context = egui::Context::default();
        let (mut app, _) = temporary_workspace(&context, &local, Vec::new());
        let managed = app.workspace_root.clone();
        app.remember_overleaf_return_workspace();
        assert_eq!(app.overleaf.return_workspace.as_ref(), Some(&local));
        app.workspace_root = next.clone();
        app.remember_overleaf_return_workspace();
        assert_eq!(app.overleaf.return_workspace.as_ref(), Some(&next));
        app.workspace_root = managed;
        app.remember_overleaf_return_workspace();
        assert_eq!(app.overleaf.return_workspace.as_ref(), Some(&next));
    }
}
