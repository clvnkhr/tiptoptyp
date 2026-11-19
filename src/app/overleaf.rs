//! Overleaf owns the saved version; local files are a compiler/LSP working mirror.
use super::*;
mod activity;
mod browsing;
mod downloads;
pub(super) mod files;
mod follow;
#[cfg(test)]
mod follow_tests;
mod offline;
#[cfg(all(test, unix))]
mod offline_tests;
mod reconcile;
#[cfg(test)]
mod reconnect_tests;
pub(super) mod reopen;
mod review;
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
const STORAGE_CHANGE_OPEN_TABS: &str = "Close this project's tabs before applying local-copy changes. Reconnect keeps the current folder.";
const CONFLICT_ERROR: &str =
    "Overleaf and your local draft differ. Review them in the Overleaf window.";

#[derive(Default)]
pub(super) struct State {
    pub(super) visible: bool,
    files: files::State,
    query: String,
    browser_url: String,
    url_editing: bool,
    // An unsubmitted target survives URL blur while its storage options are
    // edited. Native keyboard focus alone cannot express that user intent.
    address_draft: bool,
    authentication: Authentication,
    error: Option<String>,
    browser_status: activity::BrowserStatus,
    epoch: u64,
    connection: Option<Connection>,
    pub(super) browser: BrowserLifecycle,
    pub(super) edit_events: Vec<egui::Event>,
    #[cfg(target_os = "macos")]
    passwords_job: ExclusiveJob<String>,
    tab_entities: BTreeMap<u64, String>,
    events: Option<crate::overleaf_inbox::Inbox>,
    sender: Option<crate::overleaf_inbox::Sender>,
    adoption: ProjectAdoption,
    show_browser: bool,
    workspace: Option<WorkspaceTree>,
    tree_pending: Option<Tree>,
    queued: VecDeque<MirrorWork>,
    downloads: downloads::Deferred,
    io: ExclusiveJob<Mirrored>,
    save: SavePhase,
    open_after: Option<String>,
    blocked: BTreeSet<String>,
    rebases: BTreeMap<String, reconcile::NativeRebase>,
    pub(super) review: Option<review::Dialog>,
    focus: bool,
    pub(super) context: Option<egui::Context>,
    reopen: reopen::State,
    return_workspace: Option<PathBuf>,
    storage_draft: Option<storage_ui::StorageDraft>,
    // A newly created local workspace can be selected before any project URL.
    pending_storage: Option<crate::settings::OverleafStoragePreference>,
    storage_job: ExclusiveJob<PreparedStorage>,
    folder_picker: Option<crate::workflow::PendingDialog<Option<String>>>,
    admitted_path: Option<PathBuf>,
    reopen_relative: Option<(String, u64, PathBuf)>,
    navigation: Option<NavigationOwner>,
    presence_cache: Option<ActivePresenceCache>,
    cursor_navigation: follow::CursorNavigation,
    last_cursor: Option<(u64, DocumentKey, usize, usize)>,
    last_cursor_at: Option<Instant>,
    empty_cursors: Arc<[RemoteCursor]>,
    empty_collaborators: Arc<[Collaborator]>,
    browser_profile: Option<[u8; 16]>,
    offline: offline::State,
    reopening_workspace: bool,
}
#[derive(Default)]
pub(super) enum BrowserLifecycle {
    #[default]
    Closed,
    Creating,
    Ready(crate::overleaf_browser::Browser),
    Failed,
}
impl BrowserLifecycle {
    pub(super) fn as_ref(&self) -> Option<&crate::overleaf_browser::Browser> {
        match self {
            Self::Ready(browser) => Some(browser),
            _ => None,
        }
    }
    pub(super) fn as_mut(&mut self) -> Option<&mut crate::overleaf_browser::Browser> {
        match self {
            Self::Ready(browser) => Some(browser),
            _ => None,
        }
    }
    pub(super) fn is_some(&self) -> bool {
        matches!(self, Self::Ready(_))
    }
    pub(super) fn is_none(&self) -> bool {
        !self.is_some()
    }
    fn creating(&self) -> bool {
        matches!(self, Self::Creating)
    }
    fn begin(&mut self) {
        *self = Self::Creating;
    }
    fn cancel_creation(&mut self) {
        if self.creating() {
            *self = Self::Closed;
        }
    }
    #[cfg(all(feature = "desktop-ui-tests", target_os = "macos"))]
    fn phase(&self) -> &'static str {
        match self {
            Self::Closed => "closed",
            Self::Creating => "creating",
            Self::Ready(_) => "ready",
            Self::Failed => "failed",
        }
    }
}
/// Authentication observations and the one automatic-return admission persist
/// across native browser replacement; they do not claim network readiness.
#[derive(Default)]
enum Authentication {
    #[default]
    Unchallenged,
    Required,
    ReturnAttempted,
    RequiredAfterReturn,
}
impl Authentication {
    fn required(&self) -> bool {
        matches!(self, Self::Required | Self::RequiredAfterReturn)
    }
    fn return_attempted(&self) -> bool {
        matches!(self, Self::ReturnAttempted | Self::RequiredAfterReturn)
    }
    fn set_required(&mut self, required: bool) {
        *self = match (self.return_attempted(), required) {
            (false, false) => Self::Unchallenged,
            (false, true) => Self::Required,
            (true, false) => Self::ReturnAttempted,
            (true, true) => Self::RequiredAfterReturn,
        };
    }
    fn allow_return(&mut self) {
        *self = if self.required() {
            Self::Required
        } else {
            Self::Unchallenged
        };
    }
    fn mark_return_attempted(&mut self) {
        *self = if self.required() {
            Self::RequiredAfterReturn
        } else {
            Self::ReturnAttempted
        };
    }
    #[cfg(all(feature = "desktop-ui-tests", target_os = "macos"))]
    fn phase(&self) -> &'static str {
        match self {
            Self::Unchallenged => "unchallenged",
            Self::Required => "required",
            Self::ReturnAttempted => "return-attempted",
            Self::RequiredAfterReturn => "required-after-return",
        }
    }
}
#[derive(Default)]
enum ProjectAdoption {
    #[default]
    Idle,
    Awaiting {
        epoch: u64,
    },
}
impl State {
    fn mirror_profile(&self) -> Option<[u8; 16]> {
        self.offline
            .session
            .as_ref()
            .map(|session| session.profile_id)
            .or(self.browser_profile)
    }

    fn awaiting_project(&self) -> bool {
        matches!(self.adoption, ProjectAdoption::Awaiting { epoch } if epoch == self.epoch)
    }
    fn await_project(&mut self) {
        self.adoption = ProjectAdoption::Awaiting { epoch: self.epoch };
    }
    fn cancel_adoption(&mut self) {
        self.adoption = ProjectAdoption::Idle;
    }
    fn take_adoption(&mut self) -> bool {
        let waiting = self.awaiting_project();
        self.cancel_adoption();
        waiting
    }
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
#[derive(Clone, Copy, PartialEq, Eq)]
enum Transport {
    Offline,
    Online,
}
impl Transport {
    fn from_online(online: bool) -> Self {
        if online { Self::Online } else { Self::Offline }
    }
}
struct Connection {
    project: ProjectUrl,
    root: PathBuf,
    model: Model,
    status: String,
    transport: Transport,
    storage: Option<Arc<crate::overleaf::StorageLease>>,
    hydration: Hydration,
}
impl Connection {
    fn online(&self) -> bool {
        self.transport == Transport::Online
    }
}

struct RemoteSave {
    operation: crate::workflow::SaveOperationId,
    id: String,
    wire: u64,
    tab: u64,
    key: DocumentKey,
    request: tiptoptyp::mitex_document::SaveRequest,
    continuation: Option<tiptoptyp_core::workflow::SaveContinuationToken>,
    started: Instant,
}
/// One save owns publication through server acknowledgement and durable mirror
/// completion. The same ticket also guards the shared document workflow gate.
#[derive(Default)]
enum SavePhase {
    #[default]
    Idle,
    AwaitingAcknowledgement(RemoteSave),
    WritingMirror {
        entity: String,
        operation: crate::workflow::SaveOperationId,
    },
}
impl SavePhase {
    fn active(&self) -> bool {
        !matches!(self, Self::Idle)
    }
    fn pending(&self) -> Option<&RemoteSave> {
        if let Self::AwaitingAcknowledgement(save) = self {
            Some(save)
        } else {
            None
        }
    }
    #[cfg(test)]
    fn pending_mut(&mut self) -> Option<&mut RemoteSave> {
        if let Self::AwaitingAcknowledgement(save) = self {
            Some(save)
        } else {
            None
        }
    }
    fn entity(&self) -> Option<&str> {
        match self {
            Self::AwaitingAcknowledgement(save) => Some(&save.id),
            Self::WritingMirror { entity, .. } => Some(entity),
            Self::Idle => None,
        }
    }
    fn operation(&self) -> Option<crate::workflow::SaveOperationId> {
        match self {
            Self::AwaitingAcknowledgement(save) => Some(save.operation),
            Self::WritingMirror { operation, .. } => Some(*operation),
            Self::Idle => None,
        }
    }
    fn acknowledge(&mut self, entity: &str, wire: u64) -> Option<RemoteSave> {
        if !self
            .pending()
            .is_some_and(|save| save.id == entity && save.wire == wire)
        {
            return None;
        }
        let Self::AwaitingAcknowledgement(save) = std::mem::take(self) else {
            unreachable!()
        };
        *self = Self::WritingMirror {
            entity: save.id.clone(),
            operation: save.operation,
        };
        Some(save)
    }
    fn finish(&mut self, operation: crate::workflow::SaveOperationId) -> bool {
        if self.operation() != Some(operation) {
            return false;
        }
        *self = Self::Idle;
        true
    }
    #[cfg(all(feature = "desktop-ui-tests", target_os = "macos"))]
    fn phase(&self) -> &'static str {
        match self {
            Self::Idle => "idle",
            Self::AwaitingAcknowledgement(_) => "awaiting_acknowledgement",
            Self::WritingMirror { .. } => "writing_mirror",
        }
    }
}
#[cfg(test)]
fn begin_test_save(app: &mut EditorApp) -> crate::workflow::SaveOperationId {
    let key = app.document().key();
    app.document_workflow
        .current_save()
        .map(|operation| operation.id)
        .unwrap_or_else(|| {
            app.document_workflow
                .begin_save(key, crate::workflow::SaveDomain::Overleaf)
                .unwrap()
        })
}
#[cfg(test)]
fn hold_test_save(app: &mut EditorApp, entity: &str) {
    let operation = begin_test_save(app);
    app.overleaf.save = SavePhase::WritingMirror {
        entity: entity.into(),
        operation,
    };
}
#[cfg(test)]
fn clear_test_save(app: &mut EditorApp) {
    if let Some(operation) = app.overleaf.save.operation() {
        app.finish_overleaf_save(operation);
    }
}
struct MirrorWork {
    checkpoint: bool,
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
    origin: MirrorOrigin,
    hydration_session: u64,
    tree: Option<Tree>,
    error: Option<String>,
    files: Vec<(MirrorWork, Result<PathBuf, String>)>,
    documents: BTreeMap<String, crate::overleaf::Document>,
    heads: BTreeMap<String, CheckpointAdvance>,
}
struct MirrorOrigin {
    root: PathBuf,
    profile: Option<[u8; 16]>,
}
struct CheckpointAdvance {
    previous: Option<[u8; 32]>,
    written: [u8; 32],
}
impl CheckpointAdvance {
    fn record(
        heads: &mut BTreeMap<String, Self>,
        id: &str,
        previous: Option<[u8; 32]>,
        written: [u8; 32],
    ) {
        heads
            .entry(id.to_owned())
            .and_modify(|head| head.written = written)
            .or_insert(Self { previous, written });
    }
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
    fn queue(&mut self, mut work: MirrorWork) {
        // Coalesce passive refreshes, but never supersede an acknowledged save.
        if work.save.is_none() {
            work.checkpoint |= self
                .queued
                .iter()
                .any(|old| old.entry.id == work.entry.id && old.checkpoint);
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
            if work.checkpoint {
                self.offline.checkpoint_waiting.insert(work.entry.id);
                return;
            }
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
    pub(in crate::app) fn overleaf_local_copy_pending(&self) -> bool {
        self.overleaf.connection.as_ref().is_some_and(|connection| {
            connection.root == self.workspace_root
                && (connection.hydration.pending()
                    || self.overleaf.offline_transition_busy()
                    || self.overleaf_file_operation_pending())
        })
    }

    pub(super) fn hide_overleaf(&mut self, context: &egui::Context) {
        self.overleaf.visible = false;
        if let Some(browser) = self.overleaf.browser.as_ref() {
            let _ = browser.set_visible(false);
        }
        ChildViewHost::hide(context, SALT);
        crate::window_host::return_focus(context);
    }
    pub(super) fn begin_overleaf(&mut self, context: &egui::Context) {
        self.overleaf.visible = true;
        self.overleaf.focus = true;
        if self.overleaf.query.is_empty() {
            self.overleaf.query = crate::overleaf_browser::LOGIN_URL.into();
        }
        if !self.overleaf.offline.active() {
            self.overleaf.ensure_browsing(context);
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
        if self
            .overleaf
            .connection
            .as_ref()
            .is_some_and(|connection| connection.project == project && connection.storage.is_some())
            && self.overleaf.pending_storage.is_none()
        {
            // Reconnect owns the existing lease, not unfinished storage fields.
            // In particular, it must remain usable with dirty project tabs.
            self.overleaf.authentication.allow_return();
            return self.resume_overleaf(context);
        }
        self.apply_overleaf_storage(context)?;
        self.overleaf.address_draft = false;
        Ok(())
    }
    fn resume_overleaf(&mut self, context: &egui::Context) -> Result<(), String> {
        if !self.overleaf.offline.disk_untrusted.is_empty() {
            return Err("Close this project's pages and reopen the workspace to refresh its local copy. Save As keeps a separate copy of unsaved edits.".into());
        }
        if self.overleaf.io.is_running()
            || self.overleaf.queued.iter().any(|work| work.checkpoint)
            || !self.overleaf.offline.checkpoint_waiting.is_empty()
        {
            return Err("Wait for local changes to finish saving before reconnecting.".into());
        }
        let epoch = self
            .overleaf
            .epoch
            .checked_add(1)
            .ok_or("Connection generation exhausted.")?;
        self.overleaf.storage_job = ExclusiveJob::default();
        self.overleaf.reopen_relative = None;
        self.arm_overleaf_navigation();
        self.overleaf.epoch = epoch;
        if let Some(session) = &self.overleaf.offline.session {
            self.overleaf.browser_profile = Some(session.profile_id);
        }
        if let Some(session) = &mut self.overleaf.offline.session {
            session.offline = false;
        }
        let connection = self.overleaf.connection.as_mut().unwrap();
        connection.model.reconnect(epoch);
        connection.transport = Transport::Offline;
        connection.hydration = Hydration::waiting();
        connection.status = "Syncing with Overleaf…".into();
        self.prepare_overleaf_browser(context);
        Ok(())
    }
    fn apply_overleaf_storage(&mut self, context: &egui::Context) -> Result<(), String> {
        if self.overleaf.io.is_running()
            || self.overleaf.queued.iter().any(|work| work.checkpoint)
            || !self.overleaf.offline.checkpoint_waiting.is_empty()
        {
            return Err("Wait for local changes to finish saving before changing projects.".into());
        }
        if self.overleaf.storage_job.is_running() {
            return Err("The Overleaf workspace is being prepared.".into());
        }
        let epoch = self
            .overleaf
            .epoch
            .checked_add(1)
            .ok_or("Connection generation exhausted.")?;
        let base = cache_base()?;
        let project = ProjectUrl::parse(&self.overleaf.query)?;
        self.overleaf.ensure_storage_draft(
            Some(&project),
            self.pending_settings.as_ref().unwrap_or(&self.settings),
        )?;
        let preference = self.overleaf.storage_draft.as_ref().unwrap().preference()?;
        let pending_storage = self.overleaf.pending_storage.clone().filter(|pending| {
            storage_ui::StorageDraft::unbound(pending)
                .preference()
                .as_ref()
                .ok()
                == Some(&preference)
        });
        let options = crate::overleaf::StorageOptions {
            keep_local_copy: preference.keep_local_copy,
            directory: preference.directory.as_ref().map(PathBuf::from),
        };
        let profile_id = self
            .overleaf
            .browser_profile
            .filter(|profile| {
                self.overleaf
                    .offline
                    .session
                    .as_ref()
                    .is_none_or(|session| session.profile_id != *profile)
            })
            .unwrap_or_else(crate::overleaf::workspace_session::new_profile_id);
        let same_storage = if let Some(old) = &self.overleaf.connection {
            let desired = options.root_hint(&project, &base)?;
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
            let owned_tabs = self.tabs_owned_by_workspace(&old.root);
            let has_separate_managed_tabs = self.tabs.ids().any(|id| {
                self.tab_workspace(id)
                    .is_some_and(|owner| !owner.as_os_str().is_empty() && owner != old.root)
                    && self
                        .document_for_tab(id)
                        .and_then(|document| document.path().as_ref())
                        .is_some_and(|path| path.starts_with(&old.root))
            });
            if old.root == self.workspace_root
                && (old.project != project || !same_storage)
                && !owned_tabs.is_empty()
                && !has_separate_managed_tabs
            {
                self.request_workspace_switch(
                    crate::app::workspace_switch::Destination::ConnectOverleaf {
                        url: project.url().into(),
                        preference,
                    },
                    context,
                );
                return Ok(());
            }
            let saved_offline_work = old.storage.as_ref().is_some_and(|s| s.persistent())
                && old.model.unsynced_documents().all(|(id, document)| {
                    self.overleaf
                        .offline
                        .saved_sources
                        .get(id)
                        .is_some_and(|saved| saved == document.source())
                })
                && !self.overleaf.save.active()
                && self.overleaf.save.pending().is_none()
                && !self.overleaf.io.is_running()
                && self.overleaf.queued.is_empty()
                && self.overleaf.tree_pending.is_none();
            if (has_tabs || (old.model.has_unsynced() && !saved_offline_work))
                && (old.project != project || !same_storage)
            {
                return Err(if !has_tabs {
                    "Save or resolve this project's unsynced changes before changing its local-copy folder or connecting to another project. Reconnect keeps the current folder.".into()
                } else if old.project == project {
                    STORAGE_CHANGE_OPEN_TABS.into()
                } else {
                    "Close this project's tabs before connecting to another project.".into()
                });
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
        self.overleaf.authentication.allow_return();
        if same_storage && !self.overleaf.reopening_workspace {
            self.resume_overleaf(context)?;
            self.overleaf.accept_pending_storage(&pending_storage);
            return Ok(());
        }
        // A manual Connect never reuses another request's relative file.
        self.overleaf.reopen_relative = None;
        self.arm_overleaf_navigation();
        self.overleaf.epoch = epoch;
        if let Some(old) = &mut self.overleaf.connection {
            old.transport = Transport::Offline;
        }
        self.overleaf.browser = BrowserLifecycle::Closed;
        self.overleaf.events = None;
        self.overleaf.sender = None;
        self.overleaf.browser.cancel_creation();
        self.overleaf.downloads.clear();
        self.overleaf.queued.retain(|work| work.checkpoint);
        self.overleaf.tree_pending = None;
        self.overleaf.storage_job.start_and_repaint(
            "Prepare Overleaf workspace",
            context,
            move || {
                let storage = crate::overleaf::StorageLease::prepare(&project, &options, &base)?;
                let session = crate::overleaf::workspace_session::WorkspaceSession::load(
                    storage.root(),
                    &project,
                )?
                .unwrap_or_else(|| {
                    crate::overleaf::workspace_session::WorkspaceSession::new(&project, profile_id)
                });
                session.write(storage.root(), &project)?;
                if options.keep_local_copy
                    && let Some(directory) = &options.directory
                {
                    crate::overleaf::workspace_session::bind_workspace(
                        directory,
                        storage.root(),
                        &project,
                    )?;
                }
                let model = crate::overleaf::OfflineStore::load(storage.root(), &project, epoch)?;
                Ok(PreparedStorage {
                    epoch,
                    project,
                    storage,
                    pending_storage,
                    session,
                    model,
                })
            },
        )?;
        self.overleaf.error = None;
        self.overleaf.visible = true;
        Ok(())
    }
    fn finish_overleaf_save(&mut self, operation: crate::workflow::SaveOperationId) -> bool {
        self.overleaf.save.finish(operation);
        self.document_workflow.end_save(operation)
    }
    fn prepare_overleaf_browser(&mut self, context: &egui::Context) {
        self.overleaf.authentication.set_required(false);
        if let Some(operation) = self.overleaf.save.operation()
            && self.finish_overleaf_save(operation)
        {
            self.document_workflow.cancel_continuation();
        }
        self.overleaf.review = None;
        self.overleaf.queued.retain(|work| work.checkpoint);
        self.overleaf.downloads.clear();
        self.overleaf.tree_pending = None;
        self.overleaf.open_after = None;
        self.stop_overleaf_cursor();
        self.overleaf.browser = BrowserLifecycle::Closed;
        self.overleaf.browser_url = self.overleaf.connection.as_ref().map_or_else(
            || crate::overleaf_browser::LOGIN_URL.into(),
            |connection| connection.project.url().to_owned(),
        );
        self.overleaf.url_editing = false;
        self.overleaf.address_draft = false;
        let inbox = crate::overleaf_inbox::Inbox::new(context);
        self.overleaf.sender = Some(inbox.sender());
        self.overleaf.events = Some(inbox);
        self.overleaf.browser_status = activity::BrowserStatus::Connecting;
        self.overleaf.browser.begin();
        self.overleaf.await_project();
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
    #[cfg(test)]
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
    pub(super) fn overleaf_explorer(
        &self,
    ) -> Option<(&WorkspaceTree, &str, explorer_view::WorkspaceStatus<'_>)> {
        use explorer_view::{WorkspaceStatus, WorkspaceStatusTone};
        let connection = self.overleaf.connection.as_ref()?;
        if self.workspace_root != connection.root {
            return None;
        }
        let tree = connection.model.tree()?;
        let workspace = self.overleaf.workspace.as_ref()?;
        let unsynced = || {
            connection.model.has_unsynced()
                || self.tabs.ids().any(|tab| {
                    self.document_for_tab(tab).is_some_and(|document| {
                        document.is_dirty()
                            && document
                                .path()
                                .as_ref()
                                .is_some_and(|path| path.starts_with(&connection.root))
                    })
                })
        };
        let reported_error =
            self.overleaf
                .error
                .as_deref()
                .or(match &self.overleaf.browser_status {
                    activity::BrowserStatus::Failed(error) => Some(error.as_str()),
                    _ => None,
                });
        let status = if let Some(error) = reported_error {
            WorkspaceStatus {
                text: error,
                tone: WorkspaceStatusTone::Error,
            }
        } else if !self.overleaf.blocked.is_empty() {
            WorkspaceStatus {
                text: "Needs attention",
                tone: WorkspaceStatusTone::Error,
            }
        } else if self
            .overleaf
            .save
            .pending()
            .is_some_and(|pending| pending.started.elapsed() >= SAVE_TIMEOUT)
        {
            WorkspaceStatus {
                text: "Save not confirmed · reconnect required",
                tone: WorkspaceStatusTone::Error,
            }
        } else if self.overleaf.authentication.required()
            || matches!(
                self.overleaf.browser_status,
                activity::BrowserStatus::SignedOut
            )
        {
            WorkspaceStatus {
                text: "Sign in required",
                tone: WorkspaceStatusTone::Warning,
            }
        } else if !connection.online() {
            if self.overleaf.browser.creating()
                || matches!(
                    self.overleaf.browser_status,
                    activity::BrowserStatus::Connecting
                )
            {
                WorkspaceStatus {
                    text: "Connecting to Overleaf…",
                    tone: WorkspaceStatusTone::Busy,
                }
            } else if unsynced() {
                WorkspaceStatus {
                    text: "Offline · unsynced changes",
                    tone: WorkspaceStatusTone::Warning,
                }
            } else {
                WorkspaceStatus {
                    text: "Offline",
                    tone: WorkspaceStatusTone::Offline,
                }
            }
        } else if self.overleaf.save.active() || self.overleaf.save.pending().is_some() {
            WorkspaceStatus {
                text: "Saving",
                tone: WorkspaceStatusTone::Busy,
            }
        } else if connection.hydration.pending()
            || !self.overleaf.rebases.is_empty()
            || self.overleaf.io.is_running()
            || self.overleaf.tree_pending.is_some()
            || !self.overleaf.queued.is_empty()
            || self.overleaf.downloads.has_pending()
            || self.overleaf_file_operation_pending()
        {
            WorkspaceStatus {
                text: "Syncing files…",
                tone: WorkspaceStatusTone::Busy,
            }
        } else if unsynced() {
            WorkspaceStatus {
                text: "Unsynced",
                tone: WorkspaceStatusTone::Warning,
            }
        } else {
            WorkspaceStatus {
                text: &connection.status,
                tone: WorkspaceStatusTone::Ready,
            }
        };
        Some((workspace, &tree.name, status))
    }
    pub(super) fn overleaf_collaborators(&self) -> Arc<[Collaborator]> {
        self.overleaf
            .connection
            .as_ref()
            .filter(|connection| connection.online())
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
            .filter(|connection| connection.online())
        else {
            self.overleaf.presence_cache = None;
            return self.overleaf.empty_cursors.clone();
        };
        let Some(id) = self
            .tabs
            .active_id()
            .and_then(|tab| self.overleaf_entry_for_tab(tab))
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
        if self.document().is_dirty()
            || self.overleaf.browser.is_none()
            || self.overleaf_following_peer().is_some()
        {
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
    /// A bound tab keeps its server identity across renames and removals.
    /// Reusing its former path for another entity must never adopt that entity.
    fn overleaf_entity_for_tab(&self, tab: u64) -> Option<&str> {
        let path = self.document_for_tab(tab)?.path().as_ref()?;
        let connection = self.overleaf.connection.as_ref()?;
        if !path.starts_with(&connection.root) {
            return None;
        }
        self.overleaf
            .tab_entities
            .get(&tab)
            .map(String::as_str)
            .or_else(|| self.overleaf_entry(path).map(|entry| entry.id.as_str()))
    }
    fn overleaf_entry_for_tab(&self, tab: u64) -> Option<&Entry> {
        let id = self.overleaf_entity_for_tab(tab)?;
        self.overleaf.connection.as_ref()?.model.tree()?.entry(id)
    }
    fn overleaf_tab_matches(&self, tab: u64, entity: &str) -> bool {
        self.overleaf_entity_for_tab(tab) == Some(entity)
    }
    fn overleaf_path_owned_by_other_entity(&self, path: &Path, entity: &str) -> bool {
        self.tabs.ids().any(|tab| {
            self.document_for_tab(tab)
                .and_then(|document| document.path().as_deref())
                == Some(path)
                && self
                    .overleaf_entity_for_tab(tab)
                    .is_some_and(|id| id != entity)
        })
    }
    pub(super) fn prune_overleaf_bindings(&mut self) {
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
            let released = still_owned || self.checkpoint_discarded_overleaf_source(&id);
            if released {
                self.overleaf.tab_entities.remove(&tab);
                if !still_owned {
                    if !self.overleaf.offline.disk_untrusted.contains(&id)
                        && self
                            .overleaf
                            .connection
                            .as_ref()
                            .and_then(|c| c.model.document(&id))
                            .is_none_or(|d| d.conflict().is_none())
                    {
                        self.overleaf.blocked.remove(&id);
                    }
                    self.overleaf.rebases.remove(&id);
                    if self.overleaf.blocked.is_empty()
                        && self.overleaf.error.as_deref() == Some(CONFLICT_ERROR)
                    {
                        self.overleaf.error = None;
                    }
                }
            }
        }
    }
    /// A correction may edit only a source whose eventual save is currently
    /// admissible. Inspect the exact tab, including parked document owners.
    pub(super) fn overleaf_autofix_save_admitted(&mut self, tab: u64) -> bool {
        let result = (|| -> Result<bool, String> {
            let offline = self.overleaf.offline.active();
            if self.document_workflow.save_in_flight()
                || self.overleaf.save.active()
                || self.overleaf_file_operation_pending()
                || (offline && self.overleaf.io.is_running())
            {
                return Ok(false);
            }
            let entry = self.overleaf_entry_for_tab(tab).ok_or(if offline {
                "This file is not in the saved Overleaf project."
            } else {
                "The original Overleaf document is unavailable. Reconnect or Save As a local copy to retain your draft."
            })?;
            if entry.kind != EntryKind::Doc {
                return Err("This Overleaf file is not an editable document.".into());
            }
            if self.overleaf.rebases.contains_key(&entry.id) {
                return Ok(false);
            }
            if self.overleaf.blocked.contains(&entry.id) {
                return Err(
                    "Review the Overleaf conflict before saving. Your local draft is retained."
                        .into(),
                );
            }
            let connection = self
                .overleaf
                .connection
                .as_ref()
                .ok_or("Connect to Overleaf first.")?;
            let document = connection.model.document(&entry.id).ok_or(
                "Wait for Overleaf to finish syncing before saving. Your draft is retained.",
            )?;
            if document.conflict().is_some() {
                let id = entry.id.clone();
                self.overleaf.blocked.insert(id);
                return Err("Overleaf changed while you were editing. Review the retained draft before saving.".into());
            }
            if offline {
                if !connection
                    .storage
                    .as_ref()
                    .is_some_and(|storage| storage.persistent())
                {
                    return Err("Keep a local copy before working offline.".into());
                }
            } else {
                if self.overleaf.authentication.required() {
                    return Err("Sign in to Overleaf before saving.".into());
                }
                if connection.hydration.pending() {
                    return Err("Wait for Overleaf to finish syncing before saving. Your draft is retained.".into());
                }
                if !connection.online() {
                    return Err("Overleaf is offline. Your local edits remain unsynced; reconnect or Save As a local copy.".into());
                }
            }
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

    /// Cache ownership is checked before every filesystem save admission.
    pub(super) fn submit_overleaf_save(
        &mut self,
        tab: u64,
        _path: &Path,
        context: &egui::Context,
    ) -> bool {
        if self.overleaf.offline.active() {
            return self.submit_overleaf_local_save(tab, context);
        }
        let result = (|| -> Result<bool, String> {
            if self.document_workflow.save_in_flight() || self.overleaf.save.pending().is_some() {
                return Ok(false);
            }
            let entry = self
                .overleaf_entry_for_tab(tab)
                .cloned()
                .ok_or("The original Overleaf document is unavailable. Reconnect or Save As a local copy to retain your draft.")?;
            if entry.kind != EntryKind::Doc {
                return Err("This Overleaf file is not an editable document.".into());
            }
            // The editor still holds the pre-merge text until its mirror receipt.
            // Do not import or publish it over the merged model draft.
            if self.overleaf.rebases.contains_key(&entry.id) {
                return Ok(false);
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
            if connection.hydration.pending() {
                return Err(
                    "Wait for Overleaf to finish syncing before saving. Your draft is retained."
                        .into(),
                );
            }
            if !connection.online() {
                return Err("Overleaf is offline. Your local edits remain unsynced; reconnect or Save As a local copy.".into());
            }
            connection
                .model
                .set_draft(&entry.id, request.source().into())?;
            if connection
                .model
                .document(&entry.id)
                .is_some_and(|doc| doc.conflict().is_some())
            {
                self.overleaf.blocked.insert(entry.id.clone());
                return Err("Overleaf changed while you were editing. Review the retained draft before saving.".into());
            }
            let operation = self
                .document_workflow
                .begin_save(key, crate::workflow::SaveDomain::Overleaf)
                .map_err(str::to_owned)?;
            let Some(command) = connection.model.publish(&entry.id) else {
                self.document_workflow.end_save(operation);
                return Ok(false);
            };
            let BrowserCommand::Edit { request: wire, .. } = &command else {
                unreachable!()
            };
            let wire = *wire;
            if let Err(error) = self.overleaf.send(command) {
                self.document_workflow.end_save(operation);
                // Dispatch failures can be ambiguous. Require a fresh server
                // read before another publish, while retaining the draft.
                let connection = self.overleaf.connection.as_mut().unwrap();
                connection.model.reconnect(self.overleaf.epoch);
                connection.transport = Transport::Offline;
                connection.status = "Reconnect required".into();
                return Err(error);
            }
            self.overleaf.save = SavePhase::AwaitingAcknowledgement(RemoteSave {
                operation,
                id: entry.id,
                wire,
                tab,
                key,
                request,
                continuation: self.document_workflow.continuation_token(),
                started: Instant::now(),
            });
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
        if self.overleaf.offline.active()
            && let Some(context) = self.overleaf.context.clone()
        {
            return self.open_cached_overleaf_path(path, &context);
        }
        let Some(entry) = self.overleaf_entry(path).cloned() else {
            return false;
        };
        if entry.kind == EntryKind::Folder {
            return true;
        }
        if self.overleaf_path_owned_by_other_entity(path, &entry.id) {
            self.overleaf.open_after = None;
            self.overleaf.navigation = None;
            self.overleaf.error = Some("The original Overleaf document is no longer available at this path. Save a local copy or close its tab before opening the replacement.".into());
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
        if self.overleaf.save.pending().is_some()
            || self.overleaf.save.active()
            || self.overleaf.storage_job.is_running()
            || self.overleaf.io.is_running()
            || self.overleaf.queued.iter().any(|work| work.checkpoint)
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
            connection.transport = Transport::Offline;
            connection.hydration.restart();
            self.overleaf.downloads.clear();
            connection.status = "Syncing files…".into();
        }
        self.overleaf
            .queued
            .retain(|work| work.save.is_some() || work.checkpoint);
        self.overleaf.browser_status = activity::BrowserStatus::Connecting;
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
        if self.receive_overleaf_file_event(&event, context)? {
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
            .filter(|tab| !self.overleaf.tab_entities.contains_key(tab))
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
            let Some(path) = document.path() else {
                continue;
            };
            let entity = self.overleaf_entity_for_tab(tab).map(str::to_owned);
            let Some(entity) = entity else {
                continue;
            };
            if self.overleaf.rebases.contains_key(&entity) {
                continue;
            }
            if !document.is_dirty()
                && !self
                    .overleaf
                    .connection
                    .as_ref()
                    .and_then(|connection| connection.model.document(&entity))
                    .is_some_and(|document| {
                        document.draft().is_some() || document.conflict().is_some()
                    })
            {
                continue;
            }
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
                if connection.hydration.pending()
                    || self.overleaf.open_after.as_ref() == Some(id) =>
            {
                Some((id.clone(), source.clone()))
            }
            _ => None,
        };
        let saved_rebase = match &event {
            BrowserEvent::Document { id, source, .. } => self
                .overleaf
                .offline
                .saved_sources
                .get(id)
                .and_then(|local| {
                    connection
                        .model
                        .document(id)
                        .filter(|document| document.conflict().is_none_or(|c| c.base_known))
                        .map(|document| {
                            (
                                id.clone(),
                                Model::reconcile_saved_source(&document.baseline, local, source)
                                    .unwrap_or_else(|| local.clone()),
                            )
                        })
                }),
            _ => None,
        };
        let change = connection.model.apply(event)?;
        if let Some((id, saved)) = saved_rebase {
            self.overleaf.offline.saved_sources.insert(id, saved);
        }
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
                    self.stage_overleaf_rebase(&id, &source)?;
                    self.overleaf.queue(MirrorWork {
                        checkpoint: false,
                        entry,
                        data: MirrorData::Document(source),
                        save: None,
                    });
                }
            }
            ModelChange::Presence => {}
            ModelChange::Project => {
                self.overleaf.browser_status = activity::BrowserStatus::Connected;
                let navigation_current = self.overleaf_navigation_current();
                if !navigation_current {
                    self.overleaf.open_after = None;
                    self.overleaf.reopen_relative = None;
                    self.overleaf.cancel_adoption();
                }
                let awaiting_project = self.overleaf.awaiting_project();
                let connection = self.overleaf.connection.as_mut().unwrap();
                let root = connection.root.clone();
                let adopt_workspace =
                    awaiting_project && navigation_current && root != self.workspace_root;
                let tree = connection.model.tree().unwrap().clone();
                let fresh_hydration = !connection.online() || connection.hydration.pending();
                if !connection.online() {
                    connection.hydration.restart();
                    self.overleaf.downloads.clear();
                    self.overleaf
                        .queued
                        .retain(|work| work.save.is_some() || work.checkpoint);
                }
                connection.hydration.reset(&tree);
                connection.status = "Syncing files…".into();
                connection.transport = Transport::Online;
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
                    && self.overleaf.awaiting_project()
                    && self.overleaf.open_after.is_none()
                    && (reopened_relative.is_some()
                        || !self.overleaf.offline.restored
                        || self
                            .overleaf
                            .offline
                            .session
                            .as_ref()
                            .is_none_or(|s| !s.has_page_state))
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
                            checkpoint: false,
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
                            .is_some_and(|d| d.conflict().is_some())
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
                if self.overleaf.take_adoption() {
                    self.overleaf.show_browser = false;
                    self.hide_overleaf(context);
                }
                if adopt_workspace {
                    self.request_workspace_switch(
                        crate::app::workspace_switch::Destination::Overleaf {
                            root,
                            epoch: self.overleaf.epoch,
                        },
                        context,
                    );
                }
            }
            ModelChange::Document {
                id,
                source,
                conflict,
            } => {
                if conflict {
                    self.overleaf.rebases.remove(&id);
                    self.overleaf.blocked.insert(id.clone());
                    self.overleaf.error = Some(CONFLICT_ERROR.into());
                    self.overleaf.visible = true;
                } else {
                    self.overleaf.blocked.remove(&id);
                    if self.overleaf.blocked.is_empty()
                        && self.overleaf.error.as_deref() == Some(CONFLICT_ERROR)
                    {
                        self.overleaf.error = None;
                    }
                    self.stage_overleaf_rebase(&id, &source)?;
                }
                let connection = self.overleaf.connection.as_ref().unwrap();
                if let Some(entry) = connection
                    .model
                    .tree()
                    .and_then(|tree| tree.entry(&id))
                    .cloned()
                {
                    self.overleaf.queue(MirrorWork {
                        checkpoint: false,
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
                let save = self.overleaf.save.acknowledge(&id, request);
                let connection = self.overleaf.connection.as_ref().unwrap();
                if let Some(entry) = connection
                    .model
                    .tree()
                    .and_then(|tree| tree.entry(&id))
                    .cloned()
                {
                    self.overleaf.queue(MirrorWork {
                        checkpoint: false,
                        entry,
                        data: MirrorData::Document(source),
                        save,
                    });
                } else if let Some(save) = save {
                    if self.finish_overleaf_save(save.operation) {
                        self.document_workflow.cancel_continuation();
                    }
                    self.overleaf.blocked.insert(id);
                    self.overleaf.error = Some(
                        "The saved file was removed from Overleaf. Your local draft is retained."
                            .into(),
                    );
                }
            }
            ModelChange::Conflict { id } => {
                self.overleaf.rebases.remove(&id);
                self.overleaf.blocked.insert(id.clone());
                if self
                    .overleaf
                    .save
                    .pending()
                    .is_some_and(|pending| pending.id == id)
                {
                    let operation = self.overleaf.save.operation().unwrap();
                    if self.finish_overleaf_save(operation) {
                        self.document_workflow.cancel_continuation();
                    }
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
                        checkpoint: false,
                        entry,
                        data: MirrorData::File(base64),
                        save: None,
                    });
                }
            }
            ModelChange::Status { state, message } => {
                self.overleaf.browser_status =
                    activity::BrowserStatus::from_event(&state, message.as_deref());
                let connection = self.overleaf.connection.as_mut().unwrap();
                let online = matches!(state.as_str(), "connected" | "saving");
                if connection.online() != online && (!online || !connection.hydration.pending()) {
                    connection.hydration.restart();
                    self.overleaf.downloads.clear();
                    self.overleaf
                        .queued
                        .retain(|work| work.save.is_some() || work.checkpoint);
                    self.overleaf.tree_pending = None;
                }
                connection.transport = Transport::from_online(online);
                connection.status = if connection.online() && connection.hydration.pending() {
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
    /// A lost/invalid authoritative message ends this decoder session. Later
    /// messages already drained in the same frame cannot revive its old state.
    fn apply_overleaf_events(
        &mut self,
        events: impl IntoIterator<Item = Result<BrowserEvent, String>>,
        context: &egui::Context,
    ) {
        for event in events {
            if let Err(error) = event.and_then(|event| self.update_overleaf_event(event, context)) {
                self.overleaf.error = Some(error.clone());
                self.overleaf.browser_status = activity::BrowserStatus::Failed(error);
                self.overleaf.events = None;
                self.overleaf.sender = None;
                self.overleaf.downloads.clear();
                self.overleaf.tree_pending = None;
                self.overleaf
                    .queued
                    .retain(|work| work.save.is_some() || work.checkpoint);
                self.stop_overleaf_cursor();
                self.overleaf.open_after = None;
                self.overleaf.navigation = None;
                if let Some(connection) = &mut self.overleaf.connection {
                    connection.transport = Transport::Offline;
                    let _ = connection.model.apply(BrowserEvent::Status {
                        epoch: connection.model.epoch(),
                        state: "error".into(),
                        message: None,
                    });
                    connection.hydration.restart();
                    connection.status = "Reconnect required".into();
                }
                break;
            }
        }
    }
    pub(super) fn poll_overleaf(&mut self, context: &egui::Context) {
        self.poll_overleaf_browsing(context);
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
        self.apply_overleaf_events(events, context);
        self.restore_overleaf_pages(context);
        if let Some(pending) = &self.overleaf.save.pending() {
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
        self.poll_overleaf_session(context);
        if self.overleaf.io.is_running() {
            return;
        }
        let waiting: Vec<_> = self
            .overleaf
            .offline
            .checkpoint_waiting
            .iter()
            .take(8)
            .cloned()
            .collect();
        for id in waiting {
            self.overleaf.offline.checkpoint_waiting.remove(&id);
            self.checkpoint_discarded_overleaf_source(&id);
        }
        let Some(connection) = &self.overleaf.connection else {
            return;
        };
        if self.overleaf.tree_pending.is_none() && self.overleaf.queued.is_empty() {
            return;
        }
        let root = connection.root.clone();
        let origin = MirrorOrigin {
            root: root.clone(),
            profile: self.overleaf.mirror_profile(),
        };
        let epoch = connection.model.epoch();
        let storage = connection.storage.clone();
        let hydration_session = connection.hydration.session;
        let tree = self.overleaf.tree_pending.take();
        let project = connection.project.clone();
        let files = self
            .overleaf
            .queued
            .drain(..self.overleaf.queued.len().min(32))
            .collect::<Vec<_>>();
        let mut documents: BTreeMap<_, _> = files
            .iter()
            .filter_map(|work| {
                connection
                    .model
                    .document(&work.entry.id)
                    .map(|document| (work.entry.id.clone(), document.clone()))
            })
            .collect();
        if let Err(error) =
            self.overleaf
                .io
                .start_and_repaint("Overleaf working copy", context, move || {
                    let mut heads: BTreeMap<String, CheckpointAdvance> = BTreeMap::new();
                    let _storage = storage;
                    let mirror = Mirror::open(&root);
                    let error = mirror.as_ref().err().cloned().or_else(|| {
                        tree.as_ref().and_then(|tree| {
                            mirror
                                .as_ref()
                                .ok()
                                .and_then(|mirror| mirror.sync_tree(&project, tree).err())
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
                                    MirrorData::Document(_) => {
                                        if let Some(document) = documents.get_mut(&work.entry.id) {
                                            let previous = document.checkpoint;
                                            mirror.prepare_unbased_document(&work.entry, document).and_then(|_| mirror.sync_document(&work.entry, document)).map(|receipt| {
                                                CheckpointAdvance::record(&mut heads, &work.entry.id, previous, receipt.checkpoint);
                                                document.checkpoint = Some(receipt.checkpoint);
                                                receipt.path
                                            })
                                        } else {
                                            Err("The Overleaf document changed before its local copy could be updated.".into())
                                        }
                                    }
                                    MirrorData::File(source) => {
                                        mirror.sync_file(&work.entry, source)
                                    }
                                }
                            };
                            (work, result)
                        })
                        .collect();
                    Ok(Mirrored {
                        epoch,
                        origin,
                        hydration_session,
                        tree,
                        error,
                        files,
                        documents,
                        heads,
                    })
                })
        {
            self.overleaf.error = Some(error);
        }
    }
    fn apply_overleaf_mirror(&mut self, reply: Mirrored, context: &egui::Context) {
        if reply.epoch != self.overleaf.epoch
            || reply.origin.profile != self.overleaf.mirror_profile()
            || self.overleaf.connection.as_ref().is_none_or(|connection| {
                connection.root != reply.origin.root || connection.model.epoch() != reply.epoch
            })
        {
            return;
        }
        // Disk commits survive superseding source/tree events. Retain their
        // exact CAS progression even when the old contents cannot be installed
        // or counted toward the current hydration barrier.
        if let Some(connection) = &mut self.overleaf.connection {
            for (id, head) in &reply.heads {
                connection
                    .model
                    .advance_checkpoint(id, head.previous, head.written);
            }
        }
        // A staged login cannot install the old browser's state. Explicit
        // saves still belong to the retained document/workspace, including an
        // offline save submitted after the new login browser was staged.
        let browser_current = reply.origin.profile == self.overleaf.browser_profile;
        let hydration_current =
            browser_current
                && self.overleaf.connection.as_ref().is_some_and(|connection| {
                    connection.hydration.session == reply.hydration_session
                });
        let tree_written = reply.error.is_none() && hydration_current;
        if browser_current && let Some(error) = reply.error {
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
            if !browser_current {
                let owned_save = work.save.as_ref().is_some_and(|save| {
                    save.id == work.entry.id
                        && self.overleaf.save.entity() == Some(save.id.as_str())
                        && self.overleaf.save.operation() == Some(save.operation)
                        && self
                            .document_workflow
                            .current_save()
                            .is_some_and(|operation| {
                                operation.id == save.operation && operation.key == save.key
                            })
                });
                if !owned_save {
                    continue;
                }
            }
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
                if let Some(save) = &work.save {
                    if self.finish_overleaf_save(save.operation) {
                        self.document_workflow.cancel_continuation();
                    }
                    self.overleaf.blocked.insert(work.entry.id);
                    self.overleaf.error = Some(
                        "The saved file was removed from Overleaf. Your local draft is retained."
                            .into(),
                    );
                }
                continue;
            };
            if work.save.is_none()
                && let MirrorData::Document(source) = &work.data
                && self
                    .overleaf
                    .connection
                    .as_ref()
                    .and_then(|c| c.model.document(&work.entry.id))
                    .is_some_and(|document| document.baseline != *source)
            {
                if work.checkpoint {
                    if let Some(current) = self
                        .overleaf
                        .queued
                        .iter_mut()
                        .find(|current| current.entry.id == work.entry.id)
                    {
                        current.checkpoint = true;
                    } else {
                        self.overleaf
                            .offline
                            .checkpoint_waiting
                            .insert(work.entry.id.clone());
                    }
                }
                continue;
            }
            if entry != work.entry {
                self.overleaf.queue(MirrorWork {
                    checkpoint: work.checkpoint,
                    entry,
                    data: work.data,
                    save: work.save,
                });
                continue;
            }
            let path = match result {
                Ok(path) => path,
                Err(error) => {
                    if work.checkpoint {
                        self.overleaf
                            .offline
                            .checkpoint_failed
                            .insert(work.entry.id.clone());
                    }
                    if matches!(work.data, MirrorData::Document(_)) {
                        self.overleaf.blocked.insert(work.entry.id.clone());
                        self.overleaf
                            .offline
                            .disk_untrusted
                            .insert(work.entry.id.clone());
                    }
                    self.overleaf.error = Some(format!(
                        "Saved Overleaf version could not be cached: {error}"
                    ));
                    if let Some(save) = &work.save
                        && self.finish_overleaf_save(save.operation)
                    {
                        self.document_workflow.cancel_continuation();
                    }
                    continue;
                }
            };
            if let Some(document) = reply.documents.get(&work.entry.id)
                && let Some(connection) = &mut self.overleaf.connection
            {
                if document.conflict().is_some_and(|c| !c.base_known)
                    && connection
                        .model
                        .document(&work.entry.id)
                        .is_some_and(|d| d.conflict().is_none())
                {
                    match connection.model.import_local_conflict(
                        &work.entry.id,
                        &document.baseline,
                        document,
                    ) {
                        Ok(true) => {
                            self.overleaf
                                .offline
                                .saved_sources
                                .insert(work.entry.id.clone(), document.source().to_owned());
                            self.overleaf.blocked.insert(work.entry.id.clone());
                            self.overleaf.error = Some(CONFLICT_ERROR.into());
                            self.overleaf.visible = true;
                        }
                        Ok(false) | Err(_) => {
                            self.overleaf.blocked.insert(work.entry.id.clone());
                            self.overleaf
                                .offline
                                .disk_untrusted
                                .insert(work.entry.id.clone());
                            self.overleaf.error = Some("The local copy changed while it was opening. Save a local copy of your edits, then reopen the workspace.".into());
                        }
                    }
                }
                self.overleaf
                    .offline
                    .checkpoint_failed
                    .remove(&work.entry.id);
                if connection
                    .model
                    .document(&work.entry.id)
                    .is_some_and(|current| {
                        current.source() == document.source()
                            && current.baseline == document.baseline
                            && current.conflict() == document.conflict()
                    })
                {
                    self.overleaf.offline.disk_untrusted.remove(&work.entry.id);
                }
                if document.conflict().is_none()
                    && connection
                        .model
                        .document(&work.entry.id)
                        .is_some_and(|d| d.conflict().is_none())
                {
                    self.overleaf.blocked.remove(&work.entry.id);
                }
                self.overleaf
                    .offline
                    .recovery_sources
                    .insert(work.entry.id.clone(), document.source().to_owned());
            }
            if let Some(save) = &work.save
                && let Some(document) = reply.documents.get(&work.entry.id)
            {
                let saved = if save.wire == 0 {
                    save.request.source()
                } else {
                    document.baseline.as_str()
                };
                if saved != document.baseline {
                    self.overleaf
                        .offline
                        .saved_sources
                        .insert(work.entry.id.clone(), saved.to_owned());
                } else {
                    self.overleaf.offline.saved_sources.remove(&work.entry.id);
                }
            }
            if hydration_current && let Some(connection) = &mut self.overleaf.connection {
                connection.hydration.accept_file(&entry);
            }
            let owners: Vec<_> = self
                .tabs
                .ids()
                .filter(|&tab| self.overleaf_tab_matches(tab, &work.entry.id))
                .filter_map(|tab| {
                    self.document_for_tab(tab)
                        .map(|document| (tab, document.key()))
                })
                .collect();
            let mut content_changed = work.save.is_some()
                || matches!(&work.data, MirrorData::File(_))
                || owners.is_empty();
            if let Some(save) = work.save {
                if !self.finish_overleaf_save(save.operation) {
                    continue;
                }
                let active = self.tabs.active_id() == Some(save.tab);
                if let Some(document) = self.tabs.document_mut(save.tab)
                    && document.key().owner == save.key.owner
                {
                    let previous_path = document.path().clone();
                    let bytes = match &work.data {
                        MirrorData::Document(source) => source.as_bytes(),
                        MirrorData::File(_) => continue,
                    };
                    let receipt = save.request.committed(fingerprint(bytes));
                    let mut continuation = None;
                    match self.document_workflow.complete_save(
                        save.continuation,
                        document,
                        receipt,
                        active,
                    ) {
                        Ok(Some(action)) => continuation = Some(action),
                        Ok(None) => {}
                        Err(error) => self.overleaf.error = Some(error.into()),
                    }
                    let dirty = document.is_dirty();
                    let receipt_moved_path = document.path().as_deref() == Some(path.as_path());
                    if previous_path.as_deref() != Some(path.as_path())
                        && let Err(error) =
                            self.rename_overleaf_native_tab(save.tab, &path, context)
                    {
                        self.overleaf.error = Some(error);
                    }
                    if receipt_moved_path && previous_path.as_deref() != Some(path.as_path()) {
                        self.notify_overleaf_document_changed(save.tab, previous_path.as_deref());
                    }
                    if let Some(mut action) = continuation {
                        action.key = self.document_for_tab(save.tab).unwrap().key();
                        action.allow_discard = false;
                        self.document_workflow.queue_action(action);
                    }
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
                match self.apply_overleaf_rebase(&work.entry.id, &path, source, context) {
                    Ok(active_changed) => changed |= active_changed,
                    Err(error) => self.overleaf.error = Some(error),
                }
                let tabs: Vec<_> = self
                    .tabs
                    .ids()
                    .filter(|&id| self.overleaf_tab_matches(id, &work.entry.id))
                    .collect();
                for tab in tabs {
                    if self.overleaf.rebases.contains_key(&work.entry.id) {
                        continue;
                    }
                    let retained_local = self
                        .overleaf
                        .connection
                        .as_ref()
                        .and_then(|c| c.model.document(&work.entry.id))
                        .is_some_and(|d| d.draft().is_some() || d.conflict().is_some());
                    let document = self.tabs.document_mut(tab).unwrap();
                    let rename = document.path().as_ref() != Some(&path)
                        && !self.document_workflow.save_in_flight();
                    if document.is_dirty() {
                        if rename {
                            match self.rename_overleaf_native_tab(tab, &path, context) {
                                Ok(active_changed) => changed |= active_changed,
                                Err(error) => self.overleaf.error = Some(error),
                            }
                        }
                        continue;
                    }
                    if retained_local {
                        if rename {
                            match self.rename_overleaf_native_tab(tab, &path, context) {
                                Ok(active_changed) => changed |= active_changed,
                                Err(error) => self.overleaf.error = Some(error),
                            }
                        }
                        continue;
                    }
                    let canonical = document.prepare_save(path.clone(), document.kind()).ok();
                    if canonical
                        .as_ref()
                        .is_some_and(|request| request.source() == source)
                    {
                        if rename {
                            match self.rename_overleaf_native_tab(tab, &path, context) {
                                Ok(active_changed) => changed |= active_changed,
                                Err(error) => self.overleaf.error = Some(error),
                            }
                        }
                        continue;
                    }
                    match self.apply_overleaf_clean_update(tab, &path, source, context) {
                        Ok(active_changed) => changed |= active_changed,
                        Err(error) => self.overleaf.error = Some(error),
                    }
                }
            }
            content_changed |= owners.iter().any(|(tab, key)| {
                self.document_for_tab(*tab)
                    .is_some_and(|document| document.key() != *key)
            });
            // A fresh mirror can complete before the next follow-input pass.
            // Retire owned cursor opens before a table/source transaction can
            // be mistaken for a successful tab activation.
            if browser_current
                && self
                    .overleaf
                    .cursor_navigation
                    .target
                    .as_ref()
                    .is_some_and(|target| target.opening() == Some(&work.entry.id))
            {
                if self.document_transaction_busy() {
                    self.stop_overleaf_cursor();
                } else if self.document_workflow.save_in_flight() {
                    self.defer_overleaf_cursor_open(&work.entry.id);
                }
            }
            if browser_current && self.overleaf.open_after.as_deref() == Some(&work.entry.id) {
                if self.workspace_switch.adopting_overleaf(self.overleaf.epoch) {
                    continue;
                }
                if let Some(root) = self
                    .overleaf
                    .connection
                    .as_ref()
                    .map(|connection| connection.root.clone())
                    && root != self.workspace_root
                    && self.overleaf_navigation_current()
                {
                    self.request_workspace_switch(
                        crate::app::workspace_switch::Destination::Overleaf {
                            root,
                            epoch: self.overleaf.epoch,
                        },
                        context,
                    );
                    continue;
                }
                self.overleaf.open_after = None;
                if !self.overleaf_navigation_current() {
                    self.overleaf.navigation = None;
                    continue;
                }
                self.overleaf.navigation = None;
                if self.overleaf_path_owned_by_other_entity(&path, &work.entry.id) {
                    self.stop_overleaf_cursor();
                    self.overleaf.error = Some("The original Overleaf document is no longer available at this path. Save a local copy or close its tab before opening the replacement.".into());
                    continue;
                }
                let cursor_navigation = self
                    .overleaf
                    .cursor_navigation
                    .target
                    .as_ref()
                    .is_some_and(|target| target.opening() == Some(&work.entry.id));
                self.overleaf.admitted_path = Some(path.clone());
                // Normal document-open policy adopts the first source preview
                // and preserves any existing explicit pin.
                let opened = self.open_workspace_document(path.clone(), context);
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
                    }
                    if cursor_navigation {
                        self.finish_overleaf_cursor_open(&work.entry.id);
                    }
                    changed = true;
                }
            }
            if content_changed {
                self.schedule_project_index();
                if self.designated_preview_path().is_some() {
                    self.schedule_compile_now();
                }
            }
        }
        if changed {
            self.schedule_compile_now();
        }
    }
    #[cfg(all(feature = "desktop-ui-tests", target_os = "macos"))]
    pub(super) fn overleaf_inspection(&self) -> serde_json::Value {
        serde_json::json!({"visible":self.overleaf.visible,"save_phase":self.overleaf.save.phase(),"browser_phase":self.overleaf.browser.phase(),"authentication_phase":self.overleaf.authentication.phase(),"hydration_phase":self.overleaf.connection.as_ref().map(|c|c.hydration.phase()),"restoration_phase":self.overleaf.offline.restoration_phase(),"blocked_count":self.overleaf.blocked.len(),"review":self.overleaf.review.as_ref().map_or_else(||serde_json::json!({"open":false,"loading":false,"left":null,"right":null,"error":null}),review::Dialog::inspection),"query_fingerprint":crate::desktop_test::fingerprint(&self.overleaf.query),"browser_url_fingerprint":self.overleaf.browser.as_ref().map(|browser|crate::desktop_test::fingerprint(&browser.current_url())),"browser_start_url_fingerprint":self.overleaf.browser.as_ref().map(|_|crate::desktop_test::fingerprint(&self.overleaf.browser_url)),"browser_keyboard":self.overleaf.browser.as_ref().map(crate::overleaf_browser::Browser::keyboard_inspection),"storage_controls_visible":self.overleaf.storage_controls_visible(),"needs_signin":self.overleaf.authentication.required(),"show_browser":self.overleaf.show_browser,"files":self.overleaf_files_inspection(),"error":self.overleaf.error,"offline_mode":self.overleaf.offline.active(),"profile_fingerprint":self.overleaf.browser_profile.map(|id|crate::desktop_test::fingerprint(&hex::encode(id))),"restored_session":self.overleaf.offline.restored,"session_saved":self.overleaf.offline.saved() && !self.overleaf.save.active() && !self.overleaf.io.is_running() && self.overleaf.queued.is_empty() && self.overleaf.tree_pending.is_none(),"local_save_pending":self.overleaf.offline.active() && self.overleaf.save.active(),"connection":self.overleaf.connection.is_some(),"online":self.overleaf.connection.as_ref().is_some_and(|connection|connection.online()),"sync_ready":self.overleaf.connection.as_ref().is_some_and(|c|c.online() && !c.hydration.pending()),"source_locked":self.overleaf_source_locked(),"storage_preparing":self.overleaf.storage_job.is_running(),"storage_keep_local_copy":self.overleaf.storage_draft.as_ref().is_none_or(|d|d.keep_local_copy),"storage_directory_fingerprint":crate::desktop_test::fingerprint(self.overleaf.storage_draft.as_ref().map_or("",|d|d.directory.as_str())),"storage_directory_visible":self.overleaf.visible && self.overleaf.storage_controls_visible() && self.overleaf.storage_draft.as_ref().is_none_or(|d|d.keep_local_copy),"keep_local_copy":self.overleaf.connection.as_ref().and_then(|c|c.storage.as_ref()).map(|s|s.persistent()),"browser":self.overleaf.browser.is_some(),"create_browser":self.overleaf.browser.creating(),"pending":self.overleaf.save.pending().is_some(),"epoch":self.overleaf.epoch,"following_peer":self.overleaf_following_peer(),"seeking_peer":self.overleaf.cursor_navigation.target.as_ref().filter(|t|!t.following()).map(|t|t.peer.as_str()),"following_head":self.overleaf.cursor_navigation.target.as_ref().filter(|t|t.following()).and_then(|t|t.last().map(|(_,head)|head)),"collaborator_count":self.overleaf_collaborators().len(),"cursor_count":self.overleaf.presence_cache.as_ref().filter(|cache|cache.key==self.document().key() && self.overleaf.connection.as_ref().is_some_and(|connection|connection.online() && connection.model.presence().generation()==cache.generation)).map_or(0,|cache|cache.cursors.len())})
    }
    pub(super) fn show_overleaf(&mut self, context: &egui::Context) {
        if !self.overleaf.visible & self.overleaf.browser.as_ref().is_none() {
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
        let mut local_workspaces = false;
        let mut disconnect = false;
        let mut work_offline = false;
        let mut refresh = false;
        let mut review_id = None;
        #[cfg(target_os = "macos")]
        let mut open_passwords = false;
        let mut choose_directory = false;
        let mut apply_storage = false;
        let mut reset_storage = false;
        let project = ProjectUrl::parse(&self.overleaf.query).ok();
        let preferences = self.pending_settings.as_ref().unwrap_or(&self.settings);
        if let Err(error) = self
            .overleaf
            .ensure_storage_draft(project.as_ref(), preferences)
        {
            self.overleaf.error = Some(error);
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
                    if let Some(browser) = state.browser.as_ref() {
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
                let mut address_focus_requested = false;
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
                            let switch_width = ui
                                .painter()
                                .layout_no_wrap(
                                    "Local folders".into(),
                                    egui::TextStyle::Button.resolve(ui.style()),
                                    ui.visuals().text_color(),
                                )
                                .size()
                                .x
                                + ui.spacing().button_padding.x * 2.0
                                + ui.spacing().item_spacing.x;
                            let back = ui.add_enabled(
                                state.browser.is_some() && !state.storage_job.is_running(),
                                egui::Button::new("‹"),
                            );
                            observe("browser_back", &back);
                            if back.clicked()
                                && let Some(browser) = state.browser.as_ref()
                            {
                                match browser.back() {
                                    Ok(()) => state.address_draft = false,
                                    Err(error) => state.error = Some(error),
                                }
                            }
                            let forward = ui.add_enabled(
                                state.browser.is_some() && !state.storage_job.is_running(),
                                egui::Button::new("›"),
                            );
                            observe("browser_forward", &forward);
                            if forward.clicked()
                                && let Some(browser) = state.browser.as_ref()
                            {
                                match browser.forward() {
                                    Ok(()) => state.address_draft = false,
                                    Err(error) => state.error = Some(error),
                                }
                            }
                            let url = ui.add(
                                egui::TextEdit::singleline(&mut state.query)
                                    .hint_text("https://www.overleaf.com/login")
                                    .desired_width(
                                        (ui.available_width() - 210.0 - switch_width).max(80.0),
                                    ),
                            );
                            observe("url", &url);
                            state.address_draft |= url.changed();
                            address_focus_requested =
                                url.clicked() || url.gained_focus() || state.focus;
                            state.url_editing = url.has_focus();
                            if state.focus {
                                state.url_editing = true;
                                url.request_focus();
                                // A new child has no native window at command dispatch.
                                // Finish the user-requested handoff once its host exists.
                                crate::window_host::focus(
                                    ui.ctx(),
                                    ui.ctx().viewport_id(),
                                    crate::window_host::FocusCause::UserAction,
                                );
                                state.acknowledge_address_focus(input.focused);
                            }
                            let go = ui.add_enabled(
                                !state.storage_job.is_running(),
                                egui::Button::new(if state.storage_job.is_running() {
                                    "Preparing…"
                                } else if let Ok(project) = ProjectUrl::parse(&state.query) {
                                    if state
                                        .connection
                                        .as_ref()
                                        .is_some_and(|connection| connection.project == project)
                                    {
                                        "Reconnect"
                                    } else {
                                        "Connect"
                                    }
                                } else {
                                    "Go"
                                }),
                            );
                            observe("connect", &go);
                            connect |= !state.storage_job.is_running()
                                && (go.clicked()
                                    || url.lost_focus()
                                        && ui.input(|input| input.key_pressed(egui::Key::Enter)));
                            let local = native_views::workspace_chooser_button(
                                ui,
                                native_views::WorkspaceChooser::Local,
                            );
                            observe("local_workspaces", &local);
                            local_workspaces |= local.clicked();
                            let close = ui.button("Hide");
                            observe("hide", &close);
                            hide |= close.clicked();
                        });
                    });
                if local_workspaces {
                    return;
                }
                let pending_storage = state.storage_changes_pending(default_directory.as_deref());
                if state.storage_controls_visible()
                    && let Some(draft) = &mut state.storage_draft
                    && draft.project_url.as_deref()
                        == ProjectUrl::parse(&state.query)
                            .ok()
                            .as_ref()
                            .map(|project| project.url())
                {
                    let action = storage_ui::show(
                        ui,
                        draft,
                        default_directory.as_deref(),
                        state.storage_job.is_running(),
                        pending_storage,
                        state.pending_storage.is_some(),
                    );
                    if action.changed && state.pending_storage.is_some() {
                        state.pending_storage = Some(draft.raw_preference());
                    }
                    choose_directory |= action.choose_folder;
                    apply_storage |= action.apply;
                    reset_storage |= action.reset;
                }
                ui.horizontal(|ui| {
                    #[cfg(target_os = "macos")]
                    {
                        let passwords = ui.add_enabled(
                            !state.passwords_job.is_running(),
                            egui::Button::new("Passwords…"),
                        );
                        observe("passwords", &passwords);
                        open_passwords |= passwords.clicked();
                    }
                    if let Some(connection) = &state.connection {
                        ui.label(&connection.status);
                        let reload = ui.add_enabled(
                            state.save.pending().is_none()
                                && !state.save.active()
                                && !state.storage_job.is_running()
                                && !state.offline.active(),
                            egui::Button::new("Refresh"),
                        );
                        observe("refresh", &reload);
                        refresh |= reload.clicked();
                        let offline = ui.add_enabled(
                            !state.offline.active()
                                && connection.storage.as_ref().is_some_and(|s| s.persistent())
                                && connection.model.tree().is_some()
                                && !state.offline_transition_busy(),
                            egui::Button::new("Work offline"),
                        );
                        observe("work_offline", &offline);
                        work_offline |= offline.clicked();
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
                                let ready = connection.online()
                                    && !connection.hydration.pending()
                                    && document.conflict().is_some()
                                    && connection
                                        .model
                                        .tree()
                                        .is_some_and(|tree| tree.entry(id).is_some());
                                let review =
                                    ui.add_enabled(ready, egui::Button::new("Review changes"));
                                observe("review", &review);
                                if review.clicked() {
                                    review_id = Some(id.clone());
                                }
                            });
                            ui.label(document.conflict().map_or("Your local draft is retained. Save As a local copy before discarding it.",|conflict|conflict.message.as_str()));
                        }
                    }
                }
                ui.separator();
                let bounds = ui.available_rect_before_wrap();
                let mut browser_created = false;
                if state.browser.creating()
                    && let Some(sender) = &state.sender
                {
                    match crate::overleaf_browser::Browser::new(
                        ui.ctx(),
                        &state.browser_url,
                        state.epoch,
                        sender.clone(),
                        state.browser_profile,
                    ) {
                        Ok(browser) => {
                            state.browser = BrowserLifecycle::Ready(browser);
                            state.browser.cancel_creation();
                            browser_created = true;
                        }
                        Err(error) => {
                            state.error = Some(error);
                            state.browser = BrowserLifecycle::Failed;
                        }
                    }
                }
                if let Some(browser) = state.browser.as_mut()
                    && let Err(error) = browser
                        .set_bounds(bounds, ui.ctx().zoom_factor())
                        .and_then(|()| browser.set_visible(state.show_browser))
                {
                    let _ = browser.set_visible(false);
                    state.error = Some(error);
                }
                if browsing::address_handoff(
                    address_focus_requested,
                    browser_created,
                    state.url_editing,
                ) && let Some(browser) = state.browser.as_ref()
                {
                    // Constructing or showing a native child can replace the
                    // responder after the URL requested egui focus. Complete
                    // this explicit handoff after every native view mutation.
                    browser.focus_parent();
                    crate::window_host::focus(
                        ui.ctx(),
                        ui.ctx().viewport_id(),
                        crate::window_host::FocusCause::UserAction,
                    );
                }
                ui.allocate_rect(bounds, Sense::hover());
            },
        );
        self.overleaf = state;
        if paste_admitted {
            self.pending_widget_paste = None;
        }
        if local_workspaces {
            self.switch_workspace_chooser(context, native_views::WorkspaceChooser::Local);
            return;
        }
        if paste_requested {
            self.request_widget_paste(context, child);
        }
        #[cfg(target_os = "macos")]
        if open_passwords {
            self.overleaf.error = None;
            if let Err(error) =
                self.overleaf
                    .passwords_job
                    .start_and_repaint("open-passwords", context, || {
                        crate::overleaf_browser::open_passwords()?;
                        Ok("Opened Passwords".into())
                    })
            {
                self.overleaf.error = Some(error);
            }
        }
        if choose_directory {
            self.choose_overleaf_directory(context);
        }
        if reset_storage {
            self.overleaf
                .reset_storage_draft(default_directory.as_deref());
            if self
                .overleaf
                .offline
                .session
                .as_ref()
                .is_some_and(|session| Some(session.profile_id) != self.overleaf.browser_profile)
            {
                if self.overleaf.offline.active() {
                    self.overleaf.browser_profile =
                        self.overleaf.offline.session.as_ref().map(|s| s.profile_id);
                    self.overleaf.browser = BrowserLifecycle::Closed;
                    self.overleaf.events = None;
                    self.overleaf.sender = None;
                    self.overleaf.browser.cancel_creation();
                    self.overleaf.show_browser = false;
                    if let Some(connection) = &mut self.overleaf.connection {
                        connection.status = "Offline".into();
                    }
                } else if let Err(error) = self.resume_overleaf(context) {
                    self.overleaf.error = Some(error);
                }
            }
        }
        if apply_storage && let Err(error) = self.apply_overleaf_storage(context) {
            self.overleaf.error = Some(error);
        }
        if connect && let Err(error) = self.navigate_overleaf(context) {
            self.overleaf.error = Some(error);
        }
        if refresh {
            self.refresh_overleaf();
        }
        if work_offline && let Err(error) = self.overleaf_work_offline(context) {
            self.overleaf.error = Some(error);
        }
        if disconnect {
            self.overleaf.authentication.set_required(false);
            self.overleaf.browser_status = activity::BrowserStatus::Disconnected;
            self.overleaf.open_after = None;
            self.overleaf.cancel_adoption();
            self.overleaf.queued.retain(|work| work.checkpoint);
            self.overleaf.downloads.clear();
            self.overleaf.tree_pending = None;
            let _ = self.overleaf.send(BrowserCommand::Disconnect {
                epoch: self.overleaf.epoch,
            });
            self.overleaf.browser = BrowserLifecycle::Closed;
            self.overleaf.events = None;
            self.overleaf.sender = None;
            if let Some(connection) = &mut self.overleaf.connection {
                connection.transport = Transport::Offline;
                connection.model.clear_presence();
                connection.status = "Disconnected".into();
                connection.hydration = Hydration::default();
            }
        }
        if let Some(id) = review_id {
            self.open_overleaf_review(context, &id);
        }
        if hide {
            self.overleaf.visible = false;
            if let Some(browser) = self.overleaf.browser.as_ref() {
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
