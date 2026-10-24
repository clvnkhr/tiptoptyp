//! Overleaf owns the saved version; local files are a compiler/LSP working mirror.
use super::*;
mod downloads;
pub(super) mod reopen;
mod storage_ui;
mod workspace;
use crate::overleaf::{
    BrowserCommand, BrowserEvent, Collaborator, Entry, EntryKind, Mirror, Model, ModelChange,
    ProjectUrl, RemoteCursor, SourceIndex, Tree,
};
use crate::worker::{ExclusiveJob, OperationSummary};
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use workspace::{Hydration, PreparedStorage};

pub(super) const SALT: &str = "tiptoptyp-overleaf";
const MAX_PENDING_BYTES: usize = 64 * 1024 * 1024;
const SAVE_TIMEOUT: Duration = Duration::from_secs(45);
const CONFLICT_ERROR: &str =
    "Overleaf and your local draft differ. Review them in the Overleaf window.";

#[derive(Default)]
pub(super) struct State {
    pub(super) visible: bool,
    query: String,
    error: Option<String>,
    epoch: u64,
    connection: Option<Connection>,
    pub(super) browser: Option<crate::overleaf_browser::Browser>,
    pub(super) edit_events: Vec<egui::Event>,
    tab_entities: BTreeMap<u64, String>,
    events: Option<crate::overleaf_inbox::Inbox>,
    sender: Option<crate::overleaf_inbox::Sender>,
    create_browser: bool,
    awaiting_project: bool,
    show_browser: bool,
    workspace: Option<WorkspaceTree>,
    tree_pending: Option<Tree>,
    queued: VecDeque<MirrorWork>,
    downloads: downloads::Deferred,
    io: ExclusiveJob<Mirrored>,
    pending: Option<RemoteSave>,
    save_active: bool,
    open_after: Option<String>,
    blocked: BTreeSet<String>,
    focus: bool,
    pub(super) context: Option<egui::Context>,
    reopen: reopen::State,
    return_workspace: Option<PathBuf>,
    storage_draft: Option<storage_ui::StorageDraft>,
    storage_job: ExclusiveJob<PreparedStorage>,
    folder_picker: Option<crate::workflow::PendingDialog<String>>,
    admitted_path: Option<PathBuf>,
    reopen_relative: Option<(String, u64, PathBuf)>,
    navigation: Option<NavigationOwner>,
    presence_cache: Option<ActivePresenceCache>,
    last_cursor: Option<(u64, DocumentKey, usize, usize)>,
    last_cursor_at: Option<Instant>,
    empty_cursors: Arc<[RemoteCursor]>,
    empty_collaborators: Arc<[Collaborator]>,
}
struct NavigationOwner {
    tab: Option<u64>,
    key: Option<DocumentKey>,
    workspace: PathBuf,
}
struct ActivePresenceCache {
    key: DocumentKey,
    id: String,
    snapshot: tiptoptyp::mitex_document::CanonicalSnapshot,
    index: SourceIndex,
    generation: u64,
    cursors: Arc<[RemoteCursor]>,
}
struct Connection {
    project: ProjectUrl,
    root: PathBuf,
    model: Model,
    status: String,
    online: bool,
    storage: Option<Arc<crate::overleaf::StorageLease>>,
    hydration: Hydration,
}
struct RemoteSave {
    id: String,
    wire: u64,
    tab: u64,
    key: DocumentKey,
    request: tiptoptyp::mitex_document::SaveRequest,
    continuation: Option<tiptoptyp_core::workflow::SaveContinuationToken>,
    started: Instant,
}
struct MirrorWork {
    entry: Entry,
    data: MirrorData,
    save: Option<RemoteSave>,
}
enum MirrorData {
    Document(String),
    File(String),
}
impl MirrorData {
    fn len(&self) -> usize {
        match self {
            Self::Document(s) | Self::File(s) => s.len(),
        }
    }
}
struct Mirrored {
    epoch: u64,
    hydration_session: u64,
    tree: Option<Tree>,
    error: Option<String>,
    files: Vec<(MirrorWork, Result<PathBuf, String>)>,
}
impl OperationSummary for Mirrored {
    fn completion_summary(&self) -> String {
        "Overleaf working copy updated".into()
    }
}
impl State {
    fn send(&self, command: BrowserCommand) -> Result<(), String> {
        let browser = self
            .browser
            .as_ref()
            .ok_or("Connect to Overleaf before saving.")?;
        browser.command(&serde_json::to_value(command).map_err(|e| e.to_string())?)
    }
    fn queue(&mut self, work: MirrorWork) {
        // Coalesce passive refreshes, but never supersede an acknowledged save.
        if work.save.is_none() {
            let session = self.connection.as_ref().map_or(0, |c| c.hydration.session);
            self.downloads.received(self.epoch, session, &work.entry);
            self.queued
                .retain(|old| old.save.is_some() || old.entry.id != work.entry.id);
        }
        let bytes: usize = self.queued.iter().map(|old| old.data.len()).sum();
        if work.save.is_none()
            && (self.queued.len() >= crate::overleaf::MAX_ENTRIES
                || bytes.saturating_add(work.data.len()) > MAX_PENDING_BYTES)
        {
            if let Some(connection) = &self.connection
                && connection
                    .model
                    .tree()
                    .and_then(|tree| tree.entry(&work.entry.id))
                    == Some(&work.entry)
            {
                if let Err(error) =
                    self.downloads
                        .defer(self.epoch, connection.hydration.session, work.entry)
                {
                    self.error = Some(error);
                }
            } else {
                self.error = Some(
                    "The Overleaf working-copy queue is full. Refresh after it finishes.".into(),
                );
            }
            return;
        }
        self.queued.push_back(work);
    }
}

pub(super) fn cache_base() -> Result<PathBuf, String> {
    #[cfg(target_os = "windows")]
    let base = std::env::var_os("LOCALAPPDATA").map(PathBuf::from);
    #[cfg(target_os = "macos")]
    let base = std::env::var_os("HOME")
        .map(|home| PathBuf::from(home).join("Library/Application Support"));
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    let base = std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".local/share")));
    base.filter(|base| base.is_absolute())
        .map(|base| base.join("dev.tiptoptyp.editor/overleaf"))
        .ok_or_else(|| "The application data directory is unavailable.".into())
}
pub(super) fn is_mirror_path(path: &Path) -> bool {
    cache_base().is_ok_and(|root| path.starts_with(root))
}

impl EditorApp {
    pub(super) fn hide_overleaf(&mut self, context: &egui::Context) {
        self.overleaf.visible = false;
        if let Some(browser) = &self.overleaf.browser {
            let _ = browser.set_visible(false);
        }
        ChildViewHost::hide(context, SALT);
        crate::window_host::return_focus(context);
    }
    pub(super) fn begin_overleaf(&mut self, context: &egui::Context) {
        self.overleaf.visible = true;
        self.overleaf.focus = true;
        if self.overleaf.query.is_empty() {
            self.overleaf.query = self
                .overleaf
                .connection
                .as_ref()
                .map(|connection| connection.project.url().to_owned())
                .or_else(|| self.settings.overleaf_projects.first().cloned())
                .unwrap_or_default();
        }
        self.close_app_popup();
        let child = scoped_child_viewport_id(context, SALT);
        context.send_viewport_cmd_to(child, egui::ViewportCommand::Visible(true));
        context.send_viewport_cmd_to(child, egui::ViewportCommand::Minimized(false));
        crate::window_host::focus(context, child, crate::window_host::FocusCause::UserAction);
        context.request_repaint_of(child);
        context.request_repaint();
    }
    fn connect_overleaf(&mut self, context: &egui::Context) -> Result<(), String> {
        if self.overleaf.storage_job.is_running() {
            return Err("The Overleaf workspace is being prepared.".into());
        }
        let project = ProjectUrl::parse(&self.overleaf.query)?;
        let preference = if let Some(draft) = self
            .overleaf
            .storage_draft
            .as_ref()
            .filter(|draft| draft.project_url == project.url())
        {
            draft.preference()?
        } else {
            self.pending_settings
                .as_ref()
                .unwrap_or(&self.settings)
                .overleaf_storage_preference(project.url())?
        };
        let options = crate::overleaf::StorageOptions {
            keep_local_copy: preference.keep_local_copy,
            directory: preference.directory.as_ref().map(PathBuf::from),
        };
        let same_storage = if let Some(old) = &self.overleaf.connection {
            let desired = options.root_hint(&project, &cache_base()?)?;
            let same_storage = old.project == project
                && old
                    .storage
                    .as_ref()
                    .is_some_and(|lease| lease.persistent() == preference.keep_local_copy)
                && (!preference.keep_local_copy || desired.as_ref() == Some(&old.root));
            let has_tabs = self.tabs.ids().any(|id| {
                self.document_for_tab(id)
                    .and_then(|d| d.path().as_ref())
                    .is_some_and(|path| path.starts_with(&old.root))
            });
            if has_tabs && (old.project != project || !same_storage) {
                return Err("Close this project's tabs before changing its local-copy folder or connecting to another project.".into());
            }
            same_storage
        } else {
            false
        };
        let mut settings = self
            .pending_settings
            .clone()
            .unwrap_or_else(|| self.settings.clone());
        settings.set_overleaf_storage_preference(project.url(), preference)?;
        self.queue_settings(settings, context);
        // A manual Connect never reuses another request's relative file.
        self.overleaf.reopen_relative = None;
        self.arm_overleaf_navigation();
        self.overleaf.epoch = self
            .overleaf
            .epoch
            .checked_add(1)
            .ok_or("Connection generation exhausted.")?;
        if same_storage {
            let old = self.overleaf.connection.as_mut().unwrap();
            old.model.reconnect(self.overleaf.epoch);
            old.online = false;
            old.hydration = Hydration::waiting();
            old.status = "Syncing with Overleaf…".into();
            self.prepare_overleaf_browser(context);
            return Ok(());
        }
        let epoch = self.overleaf.epoch;
        let base = cache_base()?;
        if let Some(old) = &mut self.overleaf.connection {
            old.online = false;
        }
        self.overleaf.storage_job.start_and_repaint(
            "Prepare Overleaf workspace",
            context,
            move || {
                let storage = crate::overleaf::StorageLease::prepare(&project, &options, &base)?;
                Ok(PreparedStorage {
                    epoch,
                    project,
                    storage,
                })
            },
        )?;
        self.overleaf.error = None;
        self.overleaf.visible = true;
        Ok(())
    }
    fn prepare_overleaf_browser(&mut self, context: &egui::Context) {
        let had_pending = self.overleaf.pending.take().is_some();
        if std::mem::take(&mut self.overleaf.save_active) || had_pending {
            self.document_workflow.save_in_flight = false;
            self.overleaf.save_active = false;
            self.document_workflow.cancel_continuation();
        }
        self.overleaf.queued.clear();
        self.overleaf.downloads.clear();
        self.overleaf.tree_pending = None;
        self.overleaf.open_after = None;
        self.overleaf.browser = None;
        let inbox = crate::overleaf_inbox::Inbox::new(context);
        self.overleaf.sender = Some(inbox.sender());
        self.overleaf.events = Some(inbox);
        self.overleaf.create_browser = true;
        self.overleaf.awaiting_project = true;
        self.overleaf.show_browser = true;
        self.overleaf.error = None;
        self.overleaf.visible = true;
        context.send_viewport_cmd_to(
            scoped_child_viewport_id(context, SALT),
            egui::ViewportCommand::InnerSize(Vec2::new(1040.0, 740.0)),
        );
        let drop = viewport_scoped_id(context, "file-drop-target");
        context.data_mut(|data| {
            data.remove::<FileDropTarget>(drop);
            data.remove::<RememberedFileDropTarget>(drop);
        });
    }
    pub(super) fn restore_overleaf_tab_workspaces(&mut self) {
        let Some(root) = self
            .overleaf
            .connection
            .as_ref()
            .map(|connection| connection.root.clone())
        else {
            return;
        };
        let tabs: Vec<_> = self
            .tabs
            .ids()
            .filter(|&tab| {
                self.document_for_tab(tab)
                    .and_then(|document| document.path().as_ref())
                    .is_some_and(|path| path.starts_with(&root))
            })
            .collect();
        for tab in tabs {
            self.set_tab_workspace(tab, root.clone());
        }
    }
    pub(super) fn overleaf_explorer(&self) -> Option<(&WorkspaceTree, String)> {
        let connection = self.overleaf.connection.as_ref()?;
        if self.workspace_root != connection.root {
            return None;
        }
        let tree = connection.model.tree()?;
        let workspace = self.overleaf.workspace.as_ref()?;
        let state = if self.overleaf.save_active || self.overleaf.pending.is_some() {
            "Saving"
        } else if !self.overleaf.blocked.is_empty() || self.overleaf.error.is_some() {
            "Needs attention"
        } else if connection.model.has_unsynced()
            || self.tabs.ids().any(|tab| {
                self.document_for_tab(tab).is_some_and(|document| {
                    document.is_dirty()
                        && document
                            .path()
                            .as_ref()
                            .is_some_and(|path| path.starts_with(&connection.root))
                })
            })
        {
            "Unsynced"
        } else {
            &connection.status
        };
        Some((workspace, format!("Overleaf · {} · {state}", tree.name)))
    }
    pub(super) fn overleaf_collaborators(&self) -> Arc<[Collaborator]> {
        self.overleaf
            .connection
            .as_ref()
            .filter(|connection| connection.online)
            .map_or_else(
                || self.overleaf.empty_collaborators.clone(),
                |connection| connection.model.presence().collaborators(),
            )
    }
    /// Shared, revision-owned overlay data. Idle paint only clones an Arc;
    /// canonical/auto-miTeX coordinates are rebuilt when that revision changes.
    pub(super) fn overleaf_presence_for_active(&mut self) -> Arc<[RemoteCursor]> {
        let Some(connection) = self
            .overleaf
            .connection
            .as_ref()
            .filter(|connection| connection.online)
        else {
            self.overleaf.presence_cache = None;
            return self.overleaf.empty_cursors.clone();
        };
        let Some(id) = self
            .document()
            .path()
            .as_ref()
            .and_then(|path| self.overleaf_entry(path))
            .map(|entry| entry.id.as_str())
        else {
            self.overleaf.presence_cache = None;
            return self.overleaf.empty_cursors.clone();
        };
        // Local edits have no trustworthy server cursor coordinates yet.
        // Keep badges, but never reindex source or paint stale remote carets.
        if self.document().is_dirty() {
            return self.overleaf.empty_cursors.clone();
        }
        let key = self.document().key();
        let generation = connection.model.presence().generation();
        if self
            .overleaf
            .presence_cache
            .as_ref()
            .is_none_or(|cache| cache.key != key || cache.id != id)
        {
            let Ok(snapshot) = self.document().canonical_snapshot() else {
                self.overleaf.presence_cache = None;
                return self.overleaf.empty_cursors.clone();
            };
            self.overleaf.presence_cache = Some(ActivePresenceCache {
                key,
                id: id.to_owned(),
                index: SourceIndex::new(snapshot.source()),
                snapshot,
                generation: generation.wrapping_sub(1),
                cursors: Arc::default(),
            });
        }
        let cache = self.overleaf.presence_cache.as_mut().unwrap();
        if cache.generation != generation {
            let presence = self.overleaf.connection.as_ref().unwrap().model.presence();
            let cursors = presence.cursors(&cache.id, &cache.index, cache.snapshot.source());
            cache.cursors = cursors
                .iter()
                .filter_map(|cursor| {
                    let anchor = cache
                        .snapshot
                        .editor_scalar_cursor(tiptoptyp_core::text::ScalarOffset::new(
                            cursor.anchor,
                        ))?
                        .get();
                    let head = cache
                        .snapshot
                        .editor_scalar_cursor(tiptoptyp_core::text::ScalarOffset::new(cursor.head))?
                        .get();
                    Some(RemoteCursor {
                        anchor,
                        head,
                        ..cursor.clone()
                    })
                })
                .collect::<Vec<_>>()
                .into();
            cache.generation = generation;
        }
        cache.cursors.clone()
    }
    /// The official site transports the selection head only. Preserve the full
    /// local selection in the bridge while publishing only source-safe carets.
    pub(super) fn publish_overleaf_cursor(
        &mut self,
        context: &egui::Context,
        anchor: usize,
        head: usize,
    ) {
        if self.document().is_dirty() || self.overleaf.browser.is_none() {
            return;
        }
        let _ = self.overleaf_presence_for_active();
        let Some(cache) = self.overleaf.presence_cache.as_ref() else {
            return;
        };
        let identity = (self.overleaf.epoch, cache.key, anchor, head);
        if self.overleaf.last_cursor == Some(identity) {
            return;
        }
        let interval = Duration::from_millis(100);
        if let Some(last) = self.overleaf.last_cursor_at {
            let elapsed = last.elapsed();
            if elapsed < interval {
                context.request_repaint_after(interval - elapsed);
                return;
            }
        }
        let position = |offset| {
            cache
                .snapshot
                .canonical_lsp_position(tiptoptyp_core::text::ScalarOffset::new(offset))
                .and_then(|position| {
                    cache
                        .index
                        .utf16_at_position(position.line.get(), position.character.get())
                })
        };
        let Some((anchor, head)) = position(anchor).zip(position(head)) else {
            return;
        };
        let command = BrowserCommand::Cursor {
            epoch: self.overleaf.epoch,
            id: cache.id.clone(),
            source_fingerprint: cache.index.fingerprint.clone(),
            anchor,
            head,
        };
        if self.overleaf.send(command).is_ok() {
            self.overleaf.last_cursor = Some(identity);
            self.overleaf.last_cursor_at = Some(Instant::now());
        }
    }
    pub(super) fn overleaf_entry(&self, path: &Path) -> Option<&Entry> {
        let connection = self.overleaf.connection.as_ref()?;
        let relative = path
            .strip_prefix(&connection.root)
            .ok()?
            .to_str()?
            .replace('\\', "/");
        let tree = connection.model.tree()?;
        tree.entry(tree.id_for_path(&relative)?)
    }
    fn overleaf_tab_matches(&self, tab: u64, entity: &str) -> bool {
        self.overleaf
            .tab_entities
            .get(&tab)
            .is_some_and(|id| id == entity)
            && self.overleaf.connection.as_ref().is_some_and(|connection| {
                self.document_for_tab(tab)
                    .and_then(|document| document.path().as_ref())
                    .is_some_and(|path| path.starts_with(&connection.root))
            })
    }
    fn prune_overleaf_bindings(&mut self) {
        let stale: Vec<_> = self
            .overleaf
            .tab_entities
            .iter()
            .filter(|(tab, id)| !self.overleaf_tab_matches(**tab, id))
            .map(|(&tab, id)| (tab, id.clone()))
            .collect();
        for (tab, id) in stale {
            let still_owned = self.overleaf.tab_entities.iter().any(|(&other, entity)| {
                other != tab && entity == &id && self.overleaf_tab_matches(other, &id)
            });
            let released = still_owned
                || self
                    .overleaf
                    .connection
                    .as_mut()
                    .is_none_or(|connection| connection.model.release_native_draft(&id));
            if released {
                self.overleaf.tab_entities.remove(&tab);
                if !still_owned {
                    self.overleaf.blocked.remove(&id);
                    if self.overleaf.blocked.is_empty()
                        && self.overleaf.error.as_deref() == Some(CONFLICT_ERROR)
                    {
                        self.overleaf.error = None;
                    }
                }
            }
        }
    }
    /// Cache ownership is checked before every filesystem save admission.
    pub(super) fn submit_overleaf_save(
        &mut self,
        tab: u64,
        path: &Path,
        context: &egui::Context,
    ) -> bool {
        let result = (|| -> Result<bool, String> {
            if self.document_workflow.save_in_flight || self.overleaf.pending.is_some() {
                return Ok(false);
            }
            let entry = self
                .overleaf
                .tab_entities
                .get(&tab)
                .and_then(|id| {
                    self.overleaf
                        .connection
                        .as_ref()
                        .and_then(|c| c.model.tree())
                        .and_then(|tree| tree.entry(id))
                })
                .or_else(|| self.overleaf_entry(path))
                .cloned()
                .ok_or("Reconnect this Overleaf project before saving its working copy.")?;
            if entry.kind != EntryKind::Doc {
                return Err("This Overleaf file is not an editable document.".into());
            }
            if self.overleaf.blocked.contains(&entry.id) {
                return Err(
                    "Review the Overleaf conflict before saving. Your local draft is retained."
                        .into(),
                );
            }
            let document = self
                .document_for_tab(tab)
                .ok_or("The document is no longer open.")?;
            let destination = self
                .overleaf
                .connection
                .as_ref()
                .ok_or("Connect to Overleaf first.")?
                .root
                .join(&entry.path);
            let request = document
                .prepare_save(destination, document.kind())
                .map_err(|e| e.to_string())?;
            let key = document.key();
            let connection = self
                .overleaf
                .connection
                .as_mut()
                .ok_or("Connect to Overleaf first.")?;
            if connection.hydration.pending {
                return Err(
                    "Wait for Overleaf to finish syncing before saving. Your draft is retained."
                        .into(),
                );
            }
            if !connection.online {
                return Err("Overleaf is offline. Your local edits remain unsynced; reconnect or Save As a local copy.".into());
            }
            connection
                .model
                .set_draft(&entry.id, request.source().into())?;
            if connection
                .model
                .document(&entry.id)
                .is_some_and(|doc| doc.conflict.is_some())
            {
                self.overleaf.blocked.insert(entry.id.clone());
                return Err("Overleaf changed while you were editing. Review the retained draft before saving.".into());
            }
            let Some(command) = connection.model.publish(&entry.id) else {
                // Another publication still owns the browser session.
                return Ok(false);
            };
            let BrowserCommand::Edit { request: wire, .. } = &command else {
                unreachable!()
            };
            let wire = *wire;
            if let Err(error) = self.overleaf.send(command) {
                // Dispatch failures can be ambiguous. Require a fresh server
                // read before another publish, while retaining the draft.
                let connection = self.overleaf.connection.as_mut().unwrap();
                connection.model.reconnect(self.overleaf.epoch);
                connection.online = false;
                connection.status = "Reconnect required".into();
                return Err(error);
            }
            self.overleaf.pending = Some(RemoteSave {
                id: entry.id,
                wire,
                tab,
                key,
                request,
                continuation: self.document_workflow.continuation_token(),
                started: Instant::now(),
            });
            self.document_workflow.save_in_flight = true;
            self.overleaf.save_active = true;
            self.set_tab_autosave(tab, None);
            context.request_repaint_after(SAVE_TIMEOUT);
            Ok(true)
        })();
        match result {
            Ok(value) => value,
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
    pub(super) fn open_overleaf_path(&mut self, path: &Path) -> bool {
        let Some(entry) = self.overleaf_entry(path).cloned() else {
            return false;
        };
        if entry.kind == EntryKind::Folder {
            return true;
        }
        let epoch = self.overleaf.epoch;
        if let Err(error) = self.overleaf.send(BrowserCommand::Open {
            id: entry.id.clone(),
            path: entry.path,
            epoch,
        }) {
            self.overleaf.error = Some(error);
        } else {
            self.arm_overleaf_navigation();
            self.overleaf.open_after = Some(entry.id);
        }
        true
    }
    pub(super) fn refresh_overleaf(&mut self) {
        if self.overleaf.pending.is_some()
            || self.overleaf.save_active
            || self.overleaf.storage_job.is_running()
        {
            self.overleaf.error =
                Some("Wait for the current Overleaf save to finish before refreshing.".into());
            return;
        }
        if let Err(error) = self.overleaf.send(BrowserCommand::Refresh {
            epoch: self.overleaf.epoch,
        }) {
            self.overleaf.error = Some(error);
            return;
        }
        if let Some(connection) = &mut self.overleaf.connection {
            connection.online = false;
            connection.hydration.restart();
            self.overleaf.downloads.clear();
            connection.status = "Syncing files…".into();
        }
        self.overleaf.queued.retain(|work| work.save.is_some());
    }
    fn update_overleaf_event(
        &mut self,
        event: BrowserEvent,
        context: &egui::Context,
    ) -> Result<(), String> {
        let Some(connection) = self.overleaf.connection.as_ref() else {
            return Ok(());
        };
        if event.epoch() != connection.model.epoch() {
            return Ok(());
        }
        if matches!(&event, BrowserEvent::Document{id,..} | BrowserEvent::Ack{id,..}
            if self.overleaf.presence_cache.as_ref().is_some_and(|cache|&cache.id==id))
        {
            // A prior cursor command can have arrived before the hosted editor
            // installed this file or confirmed this source. Retry once ready.
            self.overleaf.last_cursor = None;
        }
        self.prune_overleaf_bindings();
        let bindings: Vec<_> = self
            .tabs
            .ids()
            .filter_map(|tab| {
                self.document_for_tab(tab)
                    .and_then(|d| d.path().as_ref())
                    .and_then(|path| self.overleaf_entry(path))
                    .map(|entry| (tab, entry.id.clone()))
            })
            .collect();
        self.overleaf.tab_entities.extend(bindings);
        // Record every open native draft before applying a remote publication,
        // including parked tabs and edits that have not reached auto-save yet.
        let ids: Vec<_> = if matches!(
            event,
            BrowserEvent::Project { .. }
                | BrowserEvent::Document { .. }
                | BrowserEvent::Ack { .. }
                | BrowserEvent::Conflict { .. }
        ) {
            self.tabs.ids().collect()
        } else {
            Vec::new()
        };
        let mut drafts = Vec::new();
        for tab in ids {
            let Some(document) = self.document_for_tab(tab) else {
                continue;
            };
            if !document.is_dirty() {
                continue;
            }
            let Some(path) = document.path() else {
                continue;
            };
            let entity = self
                .overleaf
                .tab_entities
                .get(&tab)
                .filter(|id| self.overleaf_tab_matches(tab, id))
                .cloned()
                .or_else(|| self.overleaf_entry(path).map(|entry| entry.id.clone()));
            let Some(entity) = entity else {
                continue;
            };
            if let Ok(request) = document.prepare_save(path.clone(), document.kind()) {
                drafts.push((entity, request.source().to_owned()));
            }
        }
        let connection = self.overleaf.connection.as_mut().unwrap();
        for (id, source) in drafts {
            if connection.model.document(&id).is_some() {
                connection.model.set_draft(&id, source)?;
            }
        }
        let confirmed_open = match &event {
            BrowserEvent::Document { id, source, .. }
                if connection.hydration.pending
                    || self.overleaf.open_after.as_ref() == Some(id) =>
            {
                Some((id.clone(), source.clone()))
            }
            _ => None,
        };
        let change = connection.model.apply(event)?;
        match change {
            ModelChange::Ignored => {
                if let Some((id, source)) = confirmed_open
                    && let Some(entry) = self
                        .overleaf
                        .connection
                        .as_ref()
                        .and_then(|c| c.model.tree())
                        .and_then(|tree| tree.entry(&id))
                        .cloned()
                {
                    self.overleaf.queue(MirrorWork {
                        entry,
                        data: MirrorData::Document(source),
                        save: None,
                    });
                }
            }
            ModelChange::Presence => {}
            ModelChange::Project => {
                let navigation_current = self.overleaf_navigation_current();
                if !navigation_current {
                    self.overleaf.open_after = None;
                    self.overleaf.reopen_relative = None;
                    self.overleaf.awaiting_project = false;
                }
                let connection = self.overleaf.connection.as_mut().unwrap();
                let root = connection.root.clone();
                if self.overleaf.awaiting_project && navigation_current {
                    self.workspace_root = root.clone();
                    if let Some(owner) = &mut self.overleaf.navigation {
                        owner.workspace = root.clone();
                    }
                }
                let tree = connection.model.tree().unwrap().clone();
                let fresh_hydration = !connection.online || connection.hydration.pending;
                if !connection.online {
                    connection.hydration.restart();
                    self.overleaf.downloads.clear();
                    self.overleaf.queued.retain(|work| work.save.is_some());
                }
                connection.hydration.reset(&tree);
                connection.status = "Syncing files…".into();
                connection.online = true;
                self.overleaf.tree_pending = Some(tree.clone());
                let snapshot = Arc::new(remote_snapshot(&connection.root, &tree));
                if let Some(workspace) = &mut self.overleaf.workspace {
                    if workspace.root() == snapshot.root {
                        workspace.apply_shared(snapshot);
                    } else {
                        *workspace = WorkspaceTree::from_shared(snapshot);
                    }
                } else {
                    self.overleaf.workspace = Some(WorkspaceTree::from_shared(snapshot));
                }
                let reopened_relative =
                    self.overleaf
                        .reopen_relative
                        .take()
                        .filter(|(url, epoch, _)| {
                            url == connection.project.url() && *epoch == self.overleaf.epoch
                        });
                if navigation_current
                    && self.overleaf.awaiting_project
                    && self.overleaf.open_after.is_none()
                    && !self.tabs.ids().any(|id| {
                        self.document_for_tab(id)
                            .and_then(|d| d.path().as_ref())
                            .is_some_and(|path| path.starts_with(&root))
                    })
                {
                    self.overleaf.open_after = reopened_relative
                        .as_ref()
                        .and_then(|(_, _, path)| path.to_str())
                        .and_then(|path| tree.id_for_path(path))
                        .map(str::to_owned)
                        .or_else(|| tree.main.clone())
                        .or_else(|| {
                            tree.entries()
                                .iter()
                                .find(|entry| entry.kind == EntryKind::Doc)
                                .map(|entry| entry.id.clone())
                        });
                }
                // Relocating an open tab from an old baseline is safe only
                // during live tree changes. Reconnect must wait for fresh bytes.
                let relocated: Vec<_> = self
                    .overleaf
                    .tab_entities
                    .iter()
                    .filter(|_| !fresh_hydration)
                    .filter_map(|(&tab, id)| {
                        let connection = self.overleaf.connection.as_ref()?;
                        let entry = connection.model.tree()?.entry(id)?.clone();
                        let document = self.document_for_tab(tab)?;
                        if document.path().as_ref() == Some(&connection.root.join(&entry.path)) {
                            return None;
                        }
                        let source = connection.model.document(id)?.baseline.clone();
                        Some(MirrorWork {
                            entry,
                            data: MirrorData::Document(source),
                            save: None,
                        })
                    })
                    .collect();
                for work in relocated {
                    self.overleaf.queue(work);
                }
                let removed: Vec<_> = self
                    .overleaf
                    .tab_entities
                    .values()
                    .filter(|id| {
                        self.overleaf
                            .connection
                            .as_ref()
                            .and_then(|c| c.model.document(id))
                            .is_some_and(|d| d.conflict.is_some())
                    })
                    .cloned()
                    .collect();
                self.overleaf.blocked.extend(removed);
                let url = self
                    .overleaf
                    .connection
                    .as_ref()
                    .unwrap()
                    .project
                    .url()
                    .to_owned();
                let mut settings = self
                    .pending_settings
                    .clone()
                    .unwrap_or_else(|| self.settings.clone());
                if settings.overleaf_projects.first() != Some(&url)
                    && settings.remember_overleaf_project(&url).is_ok()
                {
                    self.queue_settings(settings, context);
                }
                if std::mem::take(&mut self.overleaf.awaiting_project) {
                    self.overleaf.show_browser = false;
                    self.hide_overleaf(context);
                }
            }
            ModelChange::Document {
                id,
                source,
                conflict,
            } => {
                if conflict {
                    self.overleaf.blocked.insert(id.clone());
                }
                let connection = self.overleaf.connection.as_ref().unwrap();
                if let Some(entry) = connection
                    .model
                    .tree()
                    .and_then(|tree| tree.entry(&id))
                    .cloned()
                {
                    self.overleaf.queue(MirrorWork {
                        entry,
                        data: MirrorData::Document(source),
                        save: None,
                    });
                }
            }
            ModelChange::Acknowledged {
                id,
                request,
                source,
                ..
            } => {
                let save = if self
                    .overleaf
                    .pending
                    .as_ref()
                    .is_some_and(|p| p.id == id && p.wire == request)
                {
                    self.overleaf.pending.take()
                } else {
                    None
                };
                let connection = self.overleaf.connection.as_ref().unwrap();
                if let Some(entry) = connection
                    .model
                    .tree()
                    .and_then(|tree| tree.entry(&id))
                    .cloned()
                {
                    self.overleaf.save_active |= save.is_some();
                    self.overleaf.queue(MirrorWork {
                        entry,
                        data: MirrorData::Document(source),
                        save,
                    });
                } else if save.is_some() {
                    self.document_workflow.save_in_flight = false;
                    self.overleaf.save_active = false;
                    self.document_workflow.cancel_continuation();
                    self.overleaf.blocked.insert(id);
                    self.overleaf.error = Some(
                        "The saved file was removed from Overleaf. Your local draft is retained."
                            .into(),
                    );
                }
            }
            ModelChange::Conflict { id } => {
                self.overleaf.blocked.insert(id.clone());
                if self
                    .overleaf
                    .pending
                    .as_ref()
                    .is_some_and(|pending| pending.id == id)
                {
                    self.overleaf.pending = None;
                    self.document_workflow.save_in_flight = false;
                    self.overleaf.save_active = false;
                    self.document_workflow.cancel_continuation();
                }
                self.overleaf.error = Some(CONFLICT_ERROR.into());
                self.overleaf.visible = true;
            }
            ModelChange::File {
                id,
                path: _,
                base64,
            } => {
                let connection = self.overleaf.connection.as_ref().unwrap();
                if let Some(entry) = connection
                    .model
                    .tree()
                    .and_then(|tree| tree.entry(&id))
                    .cloned()
                {
                    self.overleaf.queue(MirrorWork {
                        entry,
                        data: MirrorData::File(base64),
                        save: None,
                    });
                }
            }
            ModelChange::Status { state, message } => {
                let connection = self.overleaf.connection.as_mut().unwrap();
                let online = matches!(state.as_str(), "connected" | "saving");
                if connection.online != online && (!online || !connection.hydration.pending) {
                    connection.hydration.restart();
                    self.overleaf.downloads.clear();
                    self.overleaf.queued.retain(|work| work.save.is_some());
                    self.overleaf.tree_pending = None;
                }
                connection.online = online;
                connection.status = if connection.online && connection.hydration.pending {
                    "Syncing files…".into()
                } else {
                    message.unwrap_or(state)
                };
            }
            ModelChange::Error(message) => {
                self.overleaf.error = Some(message);
            }
        }
        Ok(())
    }
    pub(super) fn poll_overleaf(&mut self, context: &egui::Context) {
        self.poll_overleaf_storage(context);
        self.prune_overleaf_bindings();
        self.retire_unused_overleaf(context);
        let mut events = Vec::new();
        if let Some(receiver) = &mut self.overleaf.events {
            for _ in 0..8 {
                if let Some(event) = receiver.poll() {
                    events.push(event);
                } else {
                    break;
                }
            }
        }
        for event in events {
            match event.and_then(|event| self.update_overleaf_event(event, context)) {
                Ok(()) => {}
                Err(error) => {
                    self.overleaf.error = Some(error);
                    if let Some(connection) = &mut self.overleaf.connection {
                        connection.online = false;
                        connection.model.clear_presence();
                        connection.status = "Reconnect required".into();
                    }
                }
            }
        }
        if let Some(pending) = &self.overleaf.pending {
            if pending.started.elapsed() >= SAVE_TIMEOUT {
                self.overleaf.error=Some("Overleaf has not confirmed this save. Your draft is retained; reconnect before retrying.".into());
            } else {
                context.request_repaint_after(SAVE_TIMEOUT - pending.started.elapsed());
            }
        }
        match self.overleaf.io.poll() {
            LatestJobPoll::Ready(reply) => self.apply_overleaf_mirror(reply, context),
            LatestJobPoll::Failed(error) => self.overleaf.error = Some(error),
            _ => {}
        }
        self.retry_overleaf_downloads(context);
        self.finish_overleaf_hydration(context);
        if self.overleaf.io.is_running() {
            return;
        }
        let Some(connection) = &self.overleaf.connection else {
            return;
        };
        if self.overleaf.tree_pending.is_none() && self.overleaf.queued.is_empty() {
            return;
        }
        let root = connection.root.clone();
        let epoch = connection.model.epoch();
        let storage = connection.storage.clone();
        let hydration_session = connection.hydration.session;
        let tree = self.overleaf.tree_pending.take();
        let files = self
            .overleaf
            .queued
            .drain(..self.overleaf.queued.len().min(32))
            .collect::<Vec<_>>();
        if let Err(error) =
            self.overleaf
                .io
                .start_and_repaint("Overleaf working copy", context, move || {
                    let _storage = storage;
                    let mirror = Mirror::open(&root);
                    let error = mirror.as_ref().err().cloned().or_else(|| {
                        tree.as_ref().and_then(|tree| {
                            mirror
                                .as_ref()
                                .ok()
                                .and_then(|mirror| mirror.apply_tree(tree).err())
                        })
                    });
                    let files = files
                        .into_iter()
                        .map(|work| {
                            let result = if let Some(error) = &error {
                                Err(error.clone())
                            } else {
                                let mirror = mirror.as_ref().unwrap();
                                match &work.data {
                                    MirrorData::Document(source) => {
                                        mirror.write_document(&work.entry, source)
                                    }
                                    MirrorData::File(source) => {
                                        mirror.write_file(&work.entry, source)
                                    }
                                }
                            };
                            (work, result)
                        })
                        .collect();
                    Ok(Mirrored {
                        epoch,
                        hydration_session,
                        tree,
                        error,
                        files,
                    })
                })
        {
            self.overleaf.error = Some(error);
        }
    }
    fn apply_overleaf_mirror(&mut self, reply: Mirrored, context: &egui::Context) {
        if reply.epoch != self.overleaf.epoch {
            return;
        }
        let hydration_current = self
            .overleaf
            .connection
            .as_ref()
            .is_some_and(|connection| connection.hydration.session == reply.hydration_session);
        let tree_written = reply.error.is_none() && hydration_current;
        if let Some(error) = reply.error {
            self.overleaf.error = Some(error);
        }
        if tree_written
            && let Some(tree) = &reply.tree
            && let Some(connection) = &mut self.overleaf.connection
        {
            connection.hydration.accept_tree(tree);
        }
        let mut changed = false;
        for (work, result) in reply.files {
            // Old downloads must never be relabeled as fresh work after a
            // reconnect/rename. Save receipts still retire their exact request.
            if !hydration_current && work.save.is_none() {
                continue;
            }
            let current_entry = self
                .overleaf
                .connection
                .as_ref()
                .and_then(|connection| connection.model.tree())
                .and_then(|tree| tree.entry(&work.entry.id))
                .cloned();
            let Some(entry) = current_entry else {
                if work.save.is_some() {
                    self.document_workflow.save_in_flight = false;
                    self.overleaf.save_active = false;
                    self.document_workflow.cancel_continuation();
                    self.overleaf.blocked.insert(work.entry.id);
                    self.overleaf.error = Some(
                        "The saved file was removed from Overleaf. Your local draft is retained."
                            .into(),
                    );
                }
                continue;
            };
            if entry != work.entry {
                self.overleaf.queue(MirrorWork {
                    entry,
                    data: work.data,
                    save: work.save,
                });
                continue;
            }
            if work.save.is_none()
                && let MirrorData::Document(source) = &work.data
                && self
                    .overleaf
                    .connection
                    .as_ref()
                    .and_then(|c| c.model.document(&work.entry.id))
                    .is_some_and(|document| document.baseline != *source)
            {
                continue;
            }
            let path = match result {
                Ok(path) => path,
                Err(error) => {
                    self.overleaf.error = Some(format!(
                        "Saved Overleaf version could not be cached: {error}"
                    ));
                    if work.save.is_some() {
                        self.document_workflow.save_in_flight = false;
                        self.overleaf.save_active = false;
                        self.document_workflow.cancel_continuation();
                    }
                    continue;
                }
            };
            if hydration_current && let Some(connection) = &mut self.overleaf.connection {
                connection.hydration.accept_file(&entry);
            }
            if let Some(save) = work.save {
                self.document_workflow.save_in_flight = false;
                self.overleaf.save_active = false;
                let active = self.tabs.active_id() == Some(save.tab);
                if let Some(document) = self.tabs.document_mut(save.tab)
                    && document.key().owner == save.key.owner
                {
                    let bytes = match &work.data {
                        MirrorData::Document(source) => source.as_bytes(),
                        MirrorData::File(_) => continue,
                    };
                    let receipt = save.request.committed(fingerprint(bytes));
                    match self.document_workflow.complete_save(
                        save.continuation,
                        document,
                        receipt,
                        active,
                    ) {
                        Ok(Some(mut action)) => {
                            if document.path().as_ref() != Some(&path) {
                                let kind = document.kind();
                                let _ = document.rename(path.clone(), kind);
                            }
                            action.key = document.key();
                            action.allow_discard = false;
                            self.document_workflow.queue_action(action);
                        }
                        Ok(None) => {}
                        Err(error) => self.overleaf.error = Some(error.into()),
                    }
                    if document.path().as_ref() != Some(&path) {
                        let kind = document.kind();
                        let _ = document.rename(path.clone(), kind);
                    }
                    let dirty = document.is_dirty();
                    self.activity.save_error = None;
                    let delay = Duration::from_millis(self.settings.auto_save_delay_ms.max(100));
                    let deadline =
                        (dirty && self.settings.auto_save).then(|| Instant::now() + delay);
                    self.set_tab_autosave(save.tab, deadline);
                    if deadline.is_some() {
                        context.request_repaint_after(delay);
                    }
                }
            } else if let MirrorData::Document(source) = &work.data {
                let tabs: Vec<_> = self
                    .tabs
                    .ids()
                    .filter(|&id| {
                        self.document_for_tab(id)
                            .and_then(|d| d.path().as_ref())
                            .is_some_and(|p| same_path(p, &path))
                            || self.overleaf_tab_matches(id, &work.entry.id)
                    })
                    .collect();
                for tab in tabs {
                    let document = self.tabs.document_mut(tab).unwrap();
                    if document.path().as_ref() != Some(&path)
                        && !self.document_workflow.save_in_flight
                    {
                        let kind = document.kind();
                        let _ = document.rename(path.clone(), kind);
                    }
                    if document.is_dirty() {
                        continue;
                    }
                    let canonical = document.prepare_save(path.clone(), document.kind()).ok();
                    if canonical
                        .as_ref()
                        .is_some_and(|request| request.source() == source)
                    {
                        continue;
                    }
                    let kind = document.kind();
                    document.replace_loaded_unprojected(
                        source.clone(),
                        path.clone(),
                        kind,
                        Some(fingerprint(source.as_bytes())),
                    );
                    changed |= self.tabs.active_id() == Some(tab);
                }
            }
            if self.overleaf.open_after.as_deref() == Some(&work.entry.id) {
                self.overleaf.open_after = None;
                if !self.overleaf_navigation_current() {
                    self.overleaf.navigation = None;
                    continue;
                }
                self.overleaf.navigation = None;
                self.overleaf.admitted_path = Some(path.clone());
                let opened = self.open_tab_path(path.clone(), context);
                self.overleaf.admitted_path = None;
                if opened {
                    if let Some(connection) = &self.overleaf.connection {
                        self.workspace_root = connection.root.clone();
                    }
                    if let Some(tab) = self.tabs.active_id() {
                        self.overleaf
                            .tab_entities
                            .insert(tab, work.entry.id.clone());
                        self.set_tab_workspace(tab, self.workspace_root.clone());
                        self.select_preview_tab(tab, context);
                    }
                    changed = true;
                }
            }
            self.schedule_project_index();
            if self.designated_preview_path().is_some() {
                self.schedule_compile_now();
            }
        }
        self.sync_parked_tinymist();
        if changed {
            self.reset_document_services();
            self.schedule_compile_now();
        }
    }
    fn resolve_overleaf_server(&mut self, id: &str, context: &egui::Context) {
        let Some(connection) = &mut self.overleaf.connection else {
            return;
        };
        let Some(entry) = connection
            .model
            .tree()
            .and_then(|tree| tree.entry(id))
            .cloned()
        else {
            return;
        };
        if !connection.model.resolve_server(id) {
            return;
        }
        let Some(source) = connection
            .model
            .document(id)
            .map(|doc| doc.baseline.clone())
        else {
            return;
        };
        let path = connection.root.join(&entry.path);
        let tabs: Vec<_> = self
            .tabs
            .ids()
            .filter(|&tab| {
                self.document_for_tab(tab)
                    .and_then(|d| d.path().as_ref())
                    .is_some_and(|p| same_path(p, &path))
                    || self.overleaf_tab_matches(tab, id)
            })
            .collect();
        for tab in tabs {
            let document = self.tabs.document_mut(tab).unwrap();
            let kind = document.kind();
            document.replace_loaded_unprojected(
                source.clone(),
                path.clone(),
                kind,
                Some(fingerprint(source.as_bytes())),
            );
        }
        self.overleaf.blocked.remove(id);
        self.overleaf.error = None;
        self.overleaf.queue(MirrorWork {
            entry,
            data: MirrorData::Document(source),
            save: None,
        });
        self.reset_document_services();
        self.schedule_compile_now();
        context.request_repaint();
    }
    #[cfg(all(feature = "desktop-ui-tests", target_os = "macos"))]
    pub(super) fn overleaf_inspection(&self) -> serde_json::Value {
        serde_json::json!({"visible":self.overleaf.visible,"query_fingerprint":crate::desktop_test::fingerprint(&self.overleaf.query),"error":self.overleaf.error,"connection":self.overleaf.connection.is_some(),"sync_ready":self.overleaf.connection.as_ref().is_some_and(|c|c.online && !c.hydration.pending),"source_locked":self.overleaf_source_locked(),"storage_preparing":self.overleaf.storage_job.is_running(),"storage_keep_local_copy":self.overleaf.storage_draft.as_ref().is_none_or(|d|d.keep_local_copy),"storage_directory_fingerprint":crate::desktop_test::fingerprint(self.overleaf.storage_draft.as_ref().map_or("",|d|d.directory.as_str())),"storage_directory_visible":self.overleaf.visible && self.overleaf.storage_draft.as_ref().is_none_or(|d|d.keep_local_copy),"keep_local_copy":self.overleaf.connection.as_ref().and_then(|c|c.storage.as_ref()).map(|s|s.persistent()),"browser":self.overleaf.browser.is_some(),"create_browser":self.overleaf.create_browser,"pending":self.overleaf.pending.is_some(),"epoch":self.overleaf.epoch,"collaborator_count":self.overleaf_collaborators().len(),"cursor_count":self.overleaf.presence_cache.as_ref().filter(|cache|cache.key==self.document().key() && self.overleaf.connection.as_ref().is_some_and(|connection|connection.online && connection.model.presence().generation()==cache.generation)).map_or(0,|cache|cache.cursors.len())})
    }
    pub(super) fn show_overleaf(&mut self, context: &egui::Context) {
        if !self.overleaf.visible && self.overleaf.browser.is_none() {
            return;
        }
        let appearance = context.theme();
        let style = context.style_of(appearance);
        let shortcuts = self.settings.effective_shortcuts();
        let child = scoped_child_viewport_id(context, SALT);
        let admitted = self
            .pending_widget_paste
            .is_some_and(|pending| pending.admits(child, context.cumulative_frame_nr()));
        let mut paste_requested = false;
        let mut paste_admitted = false;
        let mut connect = false;
        let mut hide = false;
        let mut disconnect = false;
        let mut refresh = false;
        let mut resolve = None;
        let mut choose_directory = false;
        let key = ProjectUrl::parse(&self.overleaf.query)
            .map(|p| p.url().to_owned())
            .unwrap_or_else(|_| self.overleaf.query.trim().to_owned());
        if self
            .overleaf
            .storage_draft
            .as_ref()
            .is_none_or(|draft| draft.project_url != key)
        {
            let preferences = self.pending_settings.as_ref().unwrap_or(&self.settings);
            let preference = preferences
                .overleaf_storage_preference(&key)
                .unwrap_or_default();
            self.overleaf.storage_draft = Some(storage_ui::StorageDraft::new(&key, &preference));
        }
        let default_directory = cache_base().ok();
        let mut state = std::mem::take(&mut self.overleaf);
        ChildViewHost::show(
            context,
            &self.captures,
            ChildViewSpec::persistent(SALT, "Overleaf", [980.0, 700.0], [520.0, 300.0], "overleaf")
                .with_visible(state.visible),
            appearance,
            &style,
            |ui, input| {
                if !state.visible {
                    if let Some(browser) = &state.browser {
                        let _ = browser.set_visible(false);
                    }
                    return;
                }
                let edits = prepare_child_widget_edit_events(ui.ctx(), &shortcuts, admitted);
                paste_requested |= edits.paste_requested;
                paste_admitted |= edits.requested_paste_admitted;
                let _queued =
                    crate::child_view::begin_queued_edit_events(ui.ctx(), &mut state.edit_events);
                handle_window_management_shortcuts(ui.ctx(), ui.ctx().viewport_id(), &shortcuts);
                if shortcuts
                    .egui(ShortcutAction::CaptureUi)
                    .is_some_and(|shortcut| ui.input_mut(|input| input.consume_shortcut(&shortcut)))
                {
                    self.captures.queue_for_viewport(ui.ctx().viewport_id());
                }
                if input.close_requested
                    || context.input_mut_for(ui.ctx().viewport_id(), |input| {
                        shortcuts
                            .egui(ShortcutAction::CloseTab)
                            .is_some_and(|chord| input.consume_shortcut(&chord))
                    })
                {
                    hide = true;
                    ui.ctx()
                        .send_viewport_cmd(egui::ViewportCommand::CancelClose);
                }
                theme::apply_compact_tool_spacing(ui);
                let title = Rect::from_min_size(
                    ui.max_rect().min,
                    Vec2::new(ui.max_rect().width(), METRICS.chrome.toolbar_height),
                );
                if ui
                    .interact(title, ui.id().with("overleaf-window-drag"), Sense::drag())
                    .drag_started()
                {
                    ui.ctx().send_viewport_cmd(egui::ViewportCommand::StartDrag);
                }
                egui::Panel::top("overleaf-titlebar")
                    .exact_size(METRICS.chrome.toolbar_height)
                    .frame(theme::settings_title_frame(ui.style()))
                    .show(ui, |ui| {
                        ui.horizontal_centered(|ui| {
                            #[cfg(target_os = "macos")]
                            theme::reserve_window_controls(ui);
                            let url = ui.add(
                                egui::TextEdit::singleline(&mut state.query)
                                    .hint_text("https://www.overleaf.com/project/…")
                                    .desired_width((ui.available_width() - 210.0).max(120.0)),
                            );
                            observe("url", &url);
                            if state.focus {
                                url.request_focus();
                                // A new child has no native window at command dispatch.
                                // Finish the user-requested handoff once its host exists.
                                crate::window_host::focus(
                                    ui.ctx(),
                                    ui.ctx().viewport_id(),
                                    crate::window_host::FocusCause::UserAction,
                                );
                                state.focus = input.focused != Some(true);
                            }
                            let go = ui.add_enabled(
                                !state.storage_job.is_running(),
                                egui::Button::new(if state.storage_job.is_running() {
                                    "Preparing…"
                                } else if state.connection.is_some() {
                                    "Reconnect"
                                } else {
                                    "Connect"
                                }),
                            );
                            observe("connect", &go);
                            connect |= go.clicked()
                                || url.lost_focus()
                                    && ui.input(|input| input.key_pressed(egui::Key::Enter));
                            let close = ui.button("Hide");
                            observe("hide", &close);
                            hide |= close.clicked();
                        });
                    });
                if let Some(draft) = &mut state.storage_draft {
                    let action = storage_ui::show(
                        ui,
                        draft,
                        default_directory.as_deref(),
                        state.storage_job.is_running(),
                    );
                    choose_directory |= action.choose_folder;
                }
                ui.horizontal(|ui| {
                    if let Some(connection) = &state.connection {
                        ui.label(&connection.status);
                        let reload = ui.add_enabled(
                            state.pending.is_none()
                                && !state.save_active
                                && !state.storage_job.is_running(),
                            egui::Button::new("Refresh"),
                        );
                        observe("refresh", &reload);
                        refresh |= reload.clicked();
                        let stop = ui.button("Disconnect");
                        observe("disconnect", &stop);
                        disconnect |= stop.clicked();
                        let site = ui.button(if state.show_browser {
                            "Hide browser"
                        } else {
                            "Show Overleaf"
                        });
                        observe("show_browser", &site);
                        if site.clicked() {
                            state.show_browser = !state.show_browser;
                        }
                    } else {
                        ui.label("Sign in to Overleaf to open its files here.");
                    }
                });
                if let Some(error) = &state.error {
                    ui.colored_label(error_color(ui.ctx()), error);
                }
                if let Some(connection) = &state.connection {
                    for id in &state.blocked {
                        if let Some(document) = connection.model.document(id) {
                            ui.horizontal(|ui| {
                                ui.strong(
                                    connection
                                        .model
                                        .tree()
                                        .and_then(|tree| tree.entry(id))
                                        .map_or(id.as_str(), |entry| entry.path.as_str()),
                                );
                                let exists = connection
                                    .model
                                    .tree()
                                    .is_some_and(|tree| tree.entry(id).is_some());
                                let server = ui
                                    .add_enabled(exists, egui::Button::new("Use Overleaf version"));
                                observe("use-server", &server);
                                if server.clicked() {
                                    resolve = Some(id.clone());
                                }
                            });
                            ui.label(document.conflict.as_ref().map_or("Your local draft is retained. Save As a local copy before discarding it.",|conflict|conflict.message.as_str()));
                        }
                    }
                }
                ui.separator();
                let bounds = ui.available_rect_before_wrap();
                if state.create_browser
                    && let (Some(connection), Some(sender)) = (&state.connection, &state.sender)
                {
                    match crate::overleaf_browser::Browser::new(
                        ui.ctx(),
                        connection.project.url(),
                        state.epoch,
                        sender.clone(),
                    ) {
                        Ok(browser) => {
                            state.browser = Some(browser);
                            state.create_browser = false;
                        }
                        Err(error) => {
                            state.error = Some(error);
                            state.create_browser = false;
                        }
                    }
                }
                if let Some(browser) = &mut state.browser
                    && let Err(error) = browser
                        .set_bounds(bounds, ui.ctx().zoom_factor())
                        .and_then(|()| browser.set_visible(state.show_browser))
                {
                    let _ = browser.set_visible(false);
                    state.error = Some(error);
                }
                ui.allocate_rect(bounds, Sense::hover());
            },
        );
        self.overleaf = state;
        if paste_admitted {
            self.pending_widget_paste = None;
        }
        if paste_requested {
            self.request_widget_paste(context, child);
        }
        if choose_directory {
            self.choose_overleaf_directory(context);
        }
        if connect && let Err(error) = self.connect_overleaf(context) {
            self.overleaf.error = Some(error);
        }
        if refresh {
            self.refresh_overleaf();
        }
        if disconnect {
            self.overleaf.open_after = None;
            self.overleaf.awaiting_project = false;
            self.overleaf.queued.clear();
            self.overleaf.downloads.clear();
            self.overleaf.tree_pending = None;
            let _ = self.overleaf.send(BrowserCommand::Disconnect {
                epoch: self.overleaf.epoch,
            });
            self.overleaf.browser = None;
            self.overleaf.events = None;
            self.overleaf.sender = None;
            if let Some(connection) = &mut self.overleaf.connection {
                connection.online = false;
                connection.model.clear_presence();
                connection.status = "Disconnected".into();
                connection.hydration = Hydration::default();
            }
        }
        if let Some(id) = resolve {
            self.resolve_overleaf_server(&id, context);
        }
        if hide {
            self.overleaf.visible = false;
            if let Some(browser) = &self.overleaf.browser {
                let _ = browser.set_visible(false);
            }
            ChildViewHost::hide(context, SALT);
            crate::window_host::return_focus(context);
        }
    }
}
fn observe(_name: &str, _response: &egui::Response) {
    #[cfg(all(feature = "desktop-ui-tests", target_os = "macos"))]
    crate::desktop_test::observe(&format!("overleaf.{_name}"), _response);
}
fn remote_snapshot(root: &Path, tree: &Tree) -> crate::workspace::WorkspaceSnapshot {
    use crate::workspace::{WorkspaceNode, WorkspaceNodeKind, WorkspaceSnapshot};
    let mut kinds = BTreeMap::new();
    for entry in tree.entries() {
        let path = PathBuf::from(&entry.path);
        kinds.insert(path.clone(), entry.kind);
        let mut parent = path.parent();
        while let Some(path) = parent.filter(|path| !path.as_os_str().is_empty()) {
            kinds.entry(path.to_owned()).or_insert(EntryKind::Folder);
            parent = path.parent();
        }
    }
    let mut by_parent: BTreeMap<PathBuf, Vec<(PathBuf, EntryKind)>> = BTreeMap::new();
    for (path, kind) in kinds {
        by_parent
            .entry(path.parent().unwrap_or(Path::new("")).to_owned())
            .or_default()
            .push((path, kind));
    }
    fn children(
        root: &Path,
        parent: &Path,
        table: &mut BTreeMap<PathBuf, Vec<(PathBuf, EntryKind)>>,
    ) -> Vec<WorkspaceNode> {
        let mut nodes: Vec<_> = table
            .remove(parent)
            .unwrap_or_default()
            .into_iter()
            .map(|(path, kind)| WorkspaceNode {
                name: path.file_name().unwrap().into(),
                path: root.join(&path),
                relative_path: path.clone(),
                kind: if kind == EntryKind::Folder {
                    WorkspaceNodeKind::Directory
                } else {
                    WorkspaceNodeKind::File
                },
                children: children(root, &path, table),
            })
            .collect();
        nodes.sort_by(|a, b| a.kind.cmp(&b.kind).then(a.name.cmp(&b.name)));
        nodes
    }
    WorkspaceSnapshot {
        root: root.to_owned(),
        nodes: children(root, Path::new(""), &mut by_parent),
    }
}

#[cfg(test)]
mod tests;
