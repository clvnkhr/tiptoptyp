//! Working-copy ownership and the fresh-server-to-disk editing barrier.
use super::*;

#[derive(Default)]
pub(super) struct Hydration {
    pub(super) pending: bool,
    pub(super) session: u64,
    tree: Option<Tree>,
    tree_written: bool,
    files: BTreeMap<String, Entry>,
}
impl Hydration {
    pub(super) fn waiting() -> Self {
        Self {
            pending: true,
            ..Default::default()
        }
    }
    pub(super) fn restart(&mut self) {
        let session = self.session.wrapping_add(1);
        *self = Self {
            pending: true,
            session,
            ..Default::default()
        };
    }
    pub(super) fn reset(&mut self, tree: &Tree) {
        self.pending = true;
        self.files.retain(|id, entry| tree.entry(id) == Some(entry));
        self.tree = Some(tree.clone());
        self.tree_written = false;
    }
    pub(super) fn accept_tree(&mut self, tree: &Tree) {
        if self.tree.as_ref() == Some(tree) {
            self.tree_written = true;
        }
    }
    pub(super) fn accept_file(&mut self, entry: &Entry) {
        if self.tree.as_ref().and_then(|tree| tree.entry(&entry.id)) == Some(entry) {
            self.files.insert(entry.id.clone(), entry.clone());
        }
    }
    fn complete(&self) -> bool {
        self.tree_written
            && self.tree.as_ref().is_some_and(|tree| {
                tree.entries()
                    .iter()
                    .filter(|entry| entry.kind != EntryKind::Folder)
                    .all(|entry| self.files.get(&entry.id) == Some(entry))
            })
    }
}

pub(super) struct PreparedStorage {
    pub(super) epoch: u64,
    pub(super) project: ProjectUrl,
    pub(super) storage: Arc<crate::overleaf::StorageLease>,
}
impl OperationSummary for PreparedStorage {
    fn completion_summary(&self) -> String {
        "Overleaf workspace prepared".into()
    }
}

impl EditorApp {
    pub(super) fn arm_overleaf_navigation(&mut self) {
        self.overleaf.navigation = Some(NavigationOwner {
            tab: self.tabs.active_id(),
            key: (!self.tabs.is_empty()).then(|| self.document().key()),
            workspace: self.workspace_root.clone(),
        });
    }
    pub(super) fn overleaf_navigation_current(&self) -> bool {
        self.overleaf.navigation.as_ref().is_none_or(|owner| {
            owner.tab == self.tabs.active_id()
                && owner.key == (!self.tabs.is_empty()).then(|| self.document().key())
                && owner.workspace == self.workspace_root
        })
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
            return !connection.online
                || connection.hydration.pending
                || self
                    .overleaf_entry(path)
                    .is_some_and(|entry| self.overleaf.blocked.contains(&entry.id));
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
        let preference = self
            .pending_settings
            .as_ref()
            .unwrap_or(&self.settings)
            .overleaf_storage_preference(project.url())?;
        self.overleaf.query = project.url().into();
        self.overleaf.storage_draft =
            Some(storage_ui::StorageDraft::new(project.url(), &preference));
        self.connect_overleaf(context)?;
        self.overleaf.reopen_relative =
            relative.map(|path| (project.url().into(), self.overleaf.epoch, path));
        Ok(())
    }
    pub(in crate::app) fn poll_overleaf_storage(&mut self, context: &egui::Context) {
        self.poll_overleaf_reopen(context);
        if let crate::workflow::DialogPoll::Ready {
            request,
            selection: Some(folder),
        } = crate::workflow::poll_dialog(&mut self.overleaf.folder_picker, context)
            && let Some(draft) = &mut self.overleaf.storage_draft
            && draft.project_url == request
        {
            draft.directory = folder.path().to_string_lossy().into_owned();
        }
        match self.overleaf.storage_job.poll() {
            LatestJobPoll::Ready(prepared) if prepared.epoch == self.overleaf.epoch => {
                let root = prepared.storage.root().to_owned();
                self.remember_overleaf_return_workspace();
                self.overleaf.connection = Some(Connection {
                    model: Model::for_project(prepared.epoch, &prepared.project),
                    project: prepared.project,
                    root,
                    storage: Some(prepared.storage),
                    hydration: Hydration::waiting(),
                    status: "Connecting to Overleaf…".into(),
                    online: false,
                });
                self.overleaf.workspace = None;
                self.overleaf.tab_entities.clear();
                self.overleaf.blocked.clear();
                self.prepare_overleaf_browser(context);
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
            && connection.online
            && connection.hydration.pending
            && quiet
            && connection.hydration.complete()
        {
            connection.hydration.pending = false;
            connection.status = "Ready".into();
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
                && (connection.online || self.overleaf.browser.is_some()))
            || (self.overleaf.visible && self.overleaf.browser.is_some())
            || self.overleaf.storage_job.is_running()
            || self.overleaf.awaiting_project
            || connection.hydration.pending
            || self.overleaf.save_active
            || self.overleaf.pending.is_some()
            || self.overleaf.io.is_running()
            || !self.overleaf.queued.is_empty()
            || self.overleaf.downloads.has_pending()
            || connection.model.has_unsynced()
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
        self.overleaf.browser = None;
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
            online: false,
            hydration: Hydration::waiting(),
        });
        for epoch in [1, 2] {
            if epoch == 2 {
                app.overleaf.epoch = epoch;
                let c = app.overleaf.connection.as_mut().unwrap();
                c.model.reconnect(epoch);
                c.online = false;
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
            online: true,
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
        app.overleaf.connection.as_mut().unwrap().online = false;
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
