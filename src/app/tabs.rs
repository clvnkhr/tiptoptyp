//! Tabs park document state, never an EditorApp or background service. Exactly
//! one slot is active; the first slot is the initial preview source.
use super::*;
mod trace;

pub(super) struct ParkedTab {
    pub(super) document: DocumentSession,
    folding: crate::folding::Folding,
    editor: Option<egui::text_edit::TextEditState>,
    autosave: Option<Instant>,
    workspace: PathBuf,
}

#[cfg(test)]
#[path = "tabs_tests.rs"]
mod tests;

pub(super) struct Tabs {
    parked: Vec<Option<ParkedTab>>,
    active: usize,
    preview: usize,
    preview_explicit: bool,
    ids: Vec<u64>,
    next_id: u64,
    pub(super) approved: Vec<(u64, DocumentKey)>,
    advance_close: bool,
    pub(super) process_close_key: Option<DocumentKey>,
    next_autosave: Option<Instant>,
    reveal_active: bool,
    tab_drag_rects: Vec<Rect>,
    tab_drag_active: bool,
    tab_drag_source: Option<u64>,
    drag_trace: trace::Trace,
    #[cfg(target_os = "macos")]
    native_drag_guard: Option<crate::native_window::TitlebarDragGuard>,
    pub(super) open_uris: std::collections::BTreeSet<String>,
    unsaved: BTreeMap<u64, UnsavedTextDocument>,
}

impl Default for Tabs {
    fn default() -> Self {
        Self {
            parked: vec![None],
            active: 0,
            preview: 0,
            preview_explicit: false,
            ids: vec![0],
            next_id: 1,
            approved: Vec::new(),
            advance_close: false,
            process_close_key: None,
            next_autosave: None,
            reveal_active: false,
            tab_drag_rects: Vec::new(),
            tab_drag_active: false,
            tab_drag_source: None,
            drag_trace: trace::Trace::default(),
            #[cfg(target_os = "macos")]
            native_drag_guard: None,
            open_uris: Default::default(),
            unsaved: Default::default(),
        }
    }
}
impl Tabs {
    #[cfg(target_os = "macos")]
    pub(super) fn suppress_native_drag(
        &mut self,
        parent: Option<&crate::native_window::ActiveWindowHandle>,
        suppress: bool,
    ) {
        if !suppress || parent.is_none() {
            self.native_drag_guard = None;
        } else if let Some(parent) = parent
            && self
                .native_drag_guard
                .as_ref()
                .is_none_or(|guard| !guard.matches_parent(parent))
        {
            // Restore the old window before acquiring a guard for its replacement.
            self.native_drag_guard = None;
            self.native_drag_guard = parent.suppress_titlebar_drag();
        }
    }

    fn native_drag_suppressed(&self) -> bool {
        #[cfg(target_os = "macos")]
        {
            self.native_drag_guard.is_some()
        }
        #[cfg(not(target_os = "macos"))]
        {
            false
        }
    }
    pub(super) fn trace_native_drag(&mut self, context: &egui::Context) {
        self.drag_trace.native_drag(context);
    }
    pub(super) fn new(preview_explicit: bool) -> Self {
        Self {
            preview_explicit,
            ..Self::default()
        }
    }
    pub(super) fn len(&self) -> usize {
        self.parked.len()
    }
    pub(super) fn is_empty(&self) -> bool {
        self.parked.is_empty()
    }
    pub(super) fn empty_after(&self) -> Self {
        Self {
            parked: Vec::new(),
            ids: Vec::new(),
            next_id: self.next_id,
            preview_explicit: true,
            ..Self::default()
        }
    }
    fn open_first(&mut self) {
        assert!(self.is_empty());
        self.parked.push(None);
        self.ids.push(self.next_id);
        self.next_id += 1;
        self.active = 0;
        self.preview = 0;
        self.preview_explicit = true;
    }
    pub(super) fn active_id(&self) -> Option<u64> {
        self.ids.get(self.active).copied()
    }
    pub(super) fn preview_id(&self) -> Option<u64> {
        self.ids.get(self.preview).copied()
    }
    pub(super) fn active_index(&self) -> Option<usize> {
        self.active_id().map(|_| self.active)
    }
    pub(super) fn ids(&self) -> impl ExactSizeIterator<Item = u64> + '_ {
        self.ids.iter().copied()
    }
    pub(super) fn uses_designated_preview(&self) -> bool {
        self.len() > 1 || self.preview_explicit
    }
    pub(super) fn id_at(&self, index: usize) -> Option<u64> {
        self.ids.get(index).copied()
    }
    pub(super) fn index_of(&self, id: u64) -> Option<usize> {
        self.ids.iter().position(|&value| value == id)
    }

    pub(super) fn claims_window_drag(&self, pointer: Option<Pos2>, primary_down: bool) -> bool {
        pointer.is_some_and(|pointer| {
            self.tab_drag_rects
                .iter()
                .any(|rect| rect.contains(pointer))
        }) || (self.tab_drag_active && primary_down)
    }

    /// Move one tab to another position without changing which document is
    /// active or which document drives the preview.
    pub(super) fn reorder(&mut self, from: usize, to: usize) -> bool {
        if from >= self.len() || to >= self.len() || from == to {
            return false;
        }

        let parked = self.parked.remove(from);
        self.parked.insert(to, parked);
        let id = self.ids.remove(from);
        self.ids.insert(to, id);
        self.active = moved_index(self.active, from, to);
        self.preview = moved_index(self.preview, from, to);
        true
    }
    fn refresh_autosave(&mut self) {
        self.next_autosave = self
            .parked
            .iter()
            .flatten()
            .filter_map(|tab| tab.autosave)
            .min();
    }
}

pub(super) fn opens_tab(action: &DeferredDocumentAction) -> bool {
    matches!(
        action,
        DeferredDocumentAction::New
            | DeferredDocumentAction::OpenFileDialog
            | DeferredDocumentAction::LoadPath(_)
            | DeferredDocumentAction::FollowFileLink { .. }
            | DeferredDocumentAction::FollowTinymistLocation { .. }
    )
}

pub(super) fn switch_needs_compile(
    preserve: bool,
    raster: bool,
    has_pages: bool,
    pending: bool,
) -> bool {
    !preserve || (raster && (!has_pages || pending))
}

pub(super) fn consume_window_close(
    input: &mut egui::InputState,
    shortcuts: &ShortcutBindings,
    child_focused: bool,
) -> bool {
    // Cmd+W in Settings or a popup closes that viewport, not its owner's tab.
    consume_shortcut_action(input, shortcuts, |action| {
        action == ShortcutAction::CloseWindow
            || (child_focused && action == ShortcutAction::CloseTab)
    })
    .is_some()
}

impl EditorApp {
    pub(super) fn collect_tab_sources(
        &self,
        overrides: &mut BTreeMap<PathBuf, String>,
    ) -> Result<(), ()> {
        for id in self.tabs.ids() {
            let document = self.document_for_tab(id).unwrap();
            if !document.kind().is_typst() {
                continue;
            }
            let path = document.path().clone().unwrap_or_else(|| {
                if Some(id) == self.tabs.preview_id() {
                    self.preview_document_path()
                } else {
                    self.untitled_tab_path(id)
                }
            });
            let source = if document.config().is_none() {
                document.source().clone()
            } else {
                document
                    .canonical_snapshot()
                    .map_err(|_| ())?
                    .source()
                    .to_owned()
            };
            // Saved paths were canonicalized when opened; never stat every
            // parked file just to assemble an indexing request.
            overrides.insert(path, source);
        }
        Ok(())
    }
    pub(super) fn prepare_tabs_fixture(&mut self, context: &egui::Context) {
        self.append_tab(context);
        self.document.replace_loaded_unprojected(
            "= Methods\n\nDraft notes for the next section.\n".into(),
            self.workspace_root.join("methods.typ"),
            DocumentKind::Typst,
            None,
        );
        self.document.edit(CCursorRange::default(), |source| {
            source.push_str("\nAn unsaved revision.\n")
        });
        self.append_tab(context);
        self.document.replace_loaded_unprojected(
            "# Research notes\n".into(),
            self.workspace_root.join("notes.md"),
            DocumentKind::Text,
            None,
        );
        let methods = self.tabs.id_at(1).unwrap();
        self.activate_tab(methods, context);
    }

    pub(super) fn prepare_asset_tab_fixture(
        &mut self,
        context: &egui::Context,
        path: PathBuf,
        kind: DocumentKind,
    ) {
        self.append_tab(context);
        self.document
            .replace_loaded_unprojected(String::new(), path.clone(), kind, None);
        self.clear_preview_for_document(true);
        self.request_asset(path);
    }
    pub(super) fn path_open_in_another_tab(&self, path: &Path) -> bool {
        self.tabs.ids().any(|id| {
            self.document_for_tab(id)
                .is_some_and(|document| document.path().as_deref() == Some(path))
        })
    }
    fn document_at_index(&self, index: usize) -> Option<&DocumentSession> {
        if index >= self.tabs.len() {
            return None;
        }
        if index == self.tabs.active {
            Some(&self.document)
        } else {
            self.tabs
                .parked
                .get(index)?
                .as_ref()
                .map(|tab| &tab.document)
        }
    }
    pub(super) fn tab_preview_document(&self) -> Option<&DocumentSession> {
        self.document_for_tab(self.tabs.preview_id()?)
    }
    pub(super) fn document_for_tab(&self, id: u64) -> Option<&DocumentSession> {
        self.document_at_index(self.tabs.index_of(id)?)
    }
    pub(super) fn document_for_tab_mut(&mut self, id: u64) -> Option<&mut DocumentSession> {
        let index = self.tabs.index_of(id)?;
        if index == self.tabs.active {
            Some(&mut self.document)
        } else {
            self.tabs.parked[index]
                .as_mut()
                .map(|tab| &mut tab.document)
        }
    }
    pub(super) fn set_tab_autosave(&mut self, id: u64, deadline: Option<Instant>) {
        let Some(index) = self.tabs.index_of(id) else {
            return;
        };
        if index == self.tabs.active {
            self.autosave_deadline = deadline;
        } else if let Some(tab) = &mut self.tabs.parked[index] {
            tab.autosave = deadline;
        }
        self.tabs.refresh_autosave();
    }
    pub(super) fn tab_preview_root(&self) -> &Path {
        let Some(id) = self.tabs.preview_id() else {
            return &self.workspace_root;
        };
        if self
            .document_for_tab(id)
            .is_none_or(|document| !document.kind().is_typst())
        {
            return &self.workspace_root;
        }
        self.tab_workspace(id).unwrap_or(&self.workspace_root)
    }
    fn tab_workspace(&self, id: u64) -> Option<&Path> {
        let index = self.tabs.index_of(id)?;
        if Some(id) == self.tabs.active_id() {
            Some(&self.workspace_root)
        } else {
            self.tabs.parked[index]
                .as_ref()
                .map(|tab| tab.workspace.as_path())
        }
    }
    pub(super) fn untitled_tab_path(&self, id: u64) -> PathBuf {
        if let Some(backing) = self.tabs.unsaved.get(&id) {
            return backing.path().into();
        }
        self.tab_workspace(id)
            .unwrap_or(&self.workspace_root)
            .join(".tiptoptyp/documents")
            .join(format!("untitled-{id}.typ"))
    }

    pub(super) fn prepare_tab_backings(&mut self) -> Result<(), String> {
        if self.tabs.len() == 1 && !self.tabs.preview_explicit {
            return Ok(());
        }
        for index in 0..self.tabs.len() {
            let id = self.tabs.id_at(index).unwrap();
            let document = self.document_for_tab(id).unwrap();
            if !document.kind().is_typst() || document.path().is_some() {
                continue;
            }
            if self.tabs.unsaved.contains_key(&id) {
                continue;
            }
            let source = document.canonical_snapshot().map_err(|e| e.to_string())?;
            let root = self.tab_workspace(id).unwrap_or(&self.workspace_root);
            let backing = UnsavedTextDocument::create(
                root,
                root,
                format!("Untitled-{}.typ", id + 1),
                source.source(),
            )
            .map_err(|e| e.to_string())?;
            self.tabs.unsaved.insert(id, backing);
        }
        Ok(())
    }

    pub(super) fn sync_parked_tinymist(&mut self) {
        let Some(generation) = self.tinymist_generation else {
            return;
        };
        for index in 0..self.tabs.len() {
            let id = self.tabs.id_at(index).unwrap();
            if Some(id) == self.tabs.active_id() {
                continue;
            }
            let document = self.document_for_tab(id).unwrap();
            if !document.kind().is_typst() {
                continue;
            }
            let path = document
                .path()
                .clone()
                .unwrap_or_else(|| self.untitled_tab_path(id));
            let Ok(source) = document.canonical_snapshot() else {
                continue;
            };
            let Ok(document) = TextDocument::from_path(
                &path,
                revision_as_i32(self.document.revision()),
                source.source(),
            ) else {
                continue;
            };
            if self.tabs.open_uris.contains(&document.uri) {
                continue;
            }
            let uri = document.uri.clone();
            if self.tinymist.did_open(generation, document).is_ok() {
                self.tabs.open_uris.insert(uri);
            }
        }
    }

    pub(super) fn update_active_tab_backing(&self, source: &str) -> Result<(), String> {
        if let Some(backing) = self
            .tabs
            .active_id()
            .and_then(|id| self.tabs.unsaved.get(&id))
        {
            backing
                .update_backing_source(source)
                .map_err(|e| e.to_string())?;
        }
        Ok(())
    }

    pub(super) fn rename_parked_tab(&mut self, old: &Path, new: &Path) {
        for tab in self.tabs.parked.iter_mut().flatten() {
            if tab.document.path().as_deref() == Some(old) {
                let kind = parked_rename_kind(&tab.document, new);
                // Renaming through Explorer must update the parked buffer,
                // not leave it autosaving back to the old filename.
                tab.document
                    .rename(new.into(), kind)
                    .expect("parked rename preflight");
            }
        }
        if let Ok(uri) = crate::tinymist::path_to_file_uri(old) {
            self.tabs.open_uris.remove(&uri);
            if let Some(generation) = self.tinymist_generation {
                let _ = self.tinymist.did_close(generation, uri);
            }
        }
        self.sync_parked_tinymist();
    }

    pub(super) fn preflight_parked_rename(&self, old: &Path, new: &Path) -> Result<(), String> {
        if self.path_open_in_another_tab(new) {
            return Err("The destination is open in another tab".into());
        }
        for tab in self
            .tabs
            .parked
            .iter()
            .flatten()
            .filter(|tab| tab.document.path().as_deref() == Some(old))
        {
            tab.document
                .can_rename(parked_rename_kind(&tab.document, new))
                .map_err(|e| e.to_string())?;
        }
        Ok(())
    }

    pub(super) fn reschedule_parked_autosave(&mut self) {
        let deadline =
            Instant::now() + Duration::from_millis(self.settings.auto_save_delay_ms.max(100));
        for tab in self.tabs.parked.iter_mut().flatten() {
            tab.autosave = (self.settings.auto_save
                && tab.document.path().is_some()
                && tab.document.is_dirty())
            .then_some(deadline);
        }
        self.tabs.refresh_autosave();
    }

    fn park_with(&mut self, mut incoming: ParkedTab, context: &egui::Context) -> ParkedTab {
        if let Some(backing) = self.tinymist_unsaved_document.take() {
            let id = self
                .tabs
                .active_id()
                .expect("parking requires an active tab");
            self.tabs.unsaved.insert(id, backing);
        }
        let old_key = incoming.document.key();
        incoming.document.reactivate_after(self.document.key());
        incoming.folding.rekey(old_key, incoming.document.key());
        // Close approvals survive a pure view switch, never an intervening edit.
        for (_, key) in &mut self.tabs.approved {
            if *key == old_key {
                *key = incoming.document.key();
            }
        }
        let outgoing = ParkedTab {
            document: std::mem::replace(&mut self.document, incoming.document),
            folding: std::mem::replace(&mut self.folding, incoming.folding),
            editor: egui::text_edit::TextEditState::load(context, source_editor_id(context)),
            autosave: std::mem::replace(&mut self.autosave_deadline, incoming.autosave),
            workspace: std::mem::replace(&mut self.workspace_root, incoming.workspace),
        };
        incoming
            .editor
            .unwrap_or_default()
            .store(context, source_editor_id(context));
        self.pending_editor_selection = None;
        self.editor_attention = None;
        self.last_editor_caret = None;
        self.editor_completion = None;
        self.editor_hover = None;
        self.tooltip_request = None;
        self.close_app_popup();
        self.search.clear();
        self.document_workflow.revoke_close();
        outgoing
    }

    fn append_tab(&mut self, context: &egui::Context) {
        let incoming = ParkedTab {
            document: DocumentSession::new(self.document.key().owner, "", DocumentKind::Typst),
            folding: Default::default(),
            editor: None,
            autosave: None,
            workspace: self.workspace_root.clone(),
        };
        let old = self.park_with(incoming, context);
        self.tabs.parked[self.tabs.active] = Some(old);
        self.tabs.active = self.tabs.len();
        self.tabs.parked.push(None);
        self.tabs.ids.push(self.tabs.next_id);
        self.tabs.next_id += 1;
        self.tabs.reveal_active = true;
        self.tabs.refresh_autosave();
    }

    pub(super) fn new_tab(&mut self, context: &egui::Context) {
        if self.tabs.is_empty() {
            self.tabs.open_first();
        } else if self.lifecycle.allows_document_work() {
            self.append_tab(context);
        }
        self.reset_untitled_document();
    }

    pub(super) fn open_tab_path(&mut self, path: PathBuf, context: &egui::Context) -> bool {
        let path = canonical_or_absolute(&path);
        if self.tabs.is_empty() {
            self.tabs.open_first();
            if self.load_path(path) {
                return true;
            }
            self.empty_workspace(context);
            return false;
        }
        let existing = self.tabs.ids().find(|&id| {
            self.document_for_tab(id)
                .is_some_and(|document| document.path().as_ref() == Some(&path))
        });
        if let Some(id) = existing {
            self.activate_tab(id, context);
            return true;
        }
        let replace_welcome =
            self.tabs.len() == 1 && self.document.path().is_none() && !self.is_dirty();
        if replace_welcome || !self.lifecycle.allows_document_work() {
            return self.load_path(path);
        }
        let old = self.tabs.active_id().expect("non-empty tab set");
        self.append_tab(context);
        if self.load_path(path) {
            return true;
        }
        let failed = self.tabs.active_id().expect("new tab is active");
        self.activate_tab(old, context);
        self.remove_parked_tab(failed);
        false
    }

    pub(super) fn activate_tab(&mut self, id: u64, context: &egui::Context) {
        let Some(index) = self.tabs.index_of(id) else {
            return;
        };
        if Some(id) == self.tabs.active_id() {
            return;
        }
        let incoming = self.tabs.parked[index]
            .take()
            .expect("inactive tab owns its document");
        let preserve_preview = self.typst_preview_available();
        let workspace_changed = incoming.workspace != self.workspace_root;
        let outgoing = self.park_with(incoming, context);
        self.tabs.parked[self.tabs.active] = Some(outgoing);
        self.tabs.active = index;
        self.tabs.reveal_active = true;
        self.tabs.refresh_autosave();
        self.clear_preview_for_document(preserve_preview);
        self.git_editor.clear_document();
        if workspace_changed {
            self.reset_document_services();
        } else if let Some(path) = self.document.path().clone() {
            if self.tinymist_generation.is_some() {
                self.reopen_tinymist_current_document(&path, self.document.kind());
            } else {
                self.restart_tinymist();
            }
        } else {
            self.restart_tinymist_preserving_preview();
        }
        if self.document.kind().preview_only()
            && let Some(path) = self.document.path().clone()
        {
            self.request_asset(path);
        }
        self.git_editor.request_refresh();
        if self.typst_preview_available()
            && switch_needs_compile(
                preserve_preview,
                self.raster_preview_required(),
                !self.preview.content.pages().is_empty(),
                matches!(
                    self.preview.status,
                    PreviewStatus::Compiling | PreviewStatus::Waiting
                ),
            )
        {
            self.schedule_compile_now();
        }
        self.schedule_project_index();
        context.request_repaint();
    }

    fn remove_parked_tab(&mut self, id: u64) {
        let index = self.tabs.index_of(id).expect("removed tab must exist");
        assert_ne!(index, self.tabs.active);
        if let Some(document) = self.document_for_tab(id) {
            let path = document
                .path()
                .clone()
                .unwrap_or_else(|| self.untitled_tab_path(id));
            if let Ok(uri) = crate::tinymist::path_to_file_uri(&path) {
                self.tabs.open_uris.remove(&uri);
                if let Some(generation) = self.tinymist_generation {
                    let _ = self.tinymist.did_close(generation, uri);
                }
            }
        }
        let removed = self.tabs.ids.remove(index);
        debug_assert_eq!(removed, id);
        self.tabs.unsaved.remove(&id);
        self.tabs.parked.remove(index);
        self.tabs.approved.retain(|(approved, _)| *approved != id);
        if self.tabs.active > index {
            self.tabs.active -= 1;
        }
        if self.tabs.preview > index {
            self.tabs.preview -= 1;
        } else if self.tabs.preview == index {
            self.tabs.preview = 0;
        }
        self.tabs.refresh_autosave();
    }

    pub(super) fn request_close_tab(&mut self, id: u64, context: &egui::Context) {
        if self.document_flow_busy()
            || self.process_close_pending
            || self.tabs.index_of(id).is_none()
        {
            return;
        }
        self.activate_tab(id, context);
        self.request_document_replacement(DeferredDocumentAction::CloseTab, "closing this tab");
    }

    pub(super) fn finish_close_tab(&mut self, context: &egui::Context) {
        if self.tabs.len() <= 1 {
            self.empty_workspace(context);
            return;
        }
        let closing = self.tabs.active_id().expect("non-empty tab set");
        let closing_index = self.tabs.active_index().unwrap();
        let changed_preview = self.tabs.preview_id() == Some(closing);
        let next_index = if closing_index > 0 {
            closing_index - 1
        } else {
            1
        };
        let next = self.tabs.id_at(next_index).unwrap();
        self.activate_tab(next, context);
        self.remove_parked_tab(closing);
        if changed_preview {
            self.tabs.preview_explicit = true;
            self.restart_tinymist_preserving_preview();
            self.schedule_compile_now();
        }
    }

    pub(super) fn tabs_close_approved(&self) -> bool {
        self.tabs.parked.iter().enumerate().all(|(i, tab)| {
            tab.as_ref().is_none_or(|tab| {
                !tab.document.is_dirty()
                    || self
                        .tabs
                        .approved
                        .contains(&(self.tabs.ids[i], tab.document.key()))
            })
        })
    }

    pub(super) fn approve_tab_window_close(&mut self) {
        let id = self
            .tabs
            .active_id()
            .expect("approval requires an active tab");
        self.tabs.approved.push((id, self.document.key()));
        self.tabs.advance_close = true;
    }

    pub(super) fn advance_tab_window_close(&mut self, context: &egui::Context) {
        if !std::mem::take(&mut self.tabs.advance_close) {
            return;
        }
        let unapproved = self.tabs.ids().find(|&id| {
            self.document_for_tab(id).is_some_and(|document| {
                document.is_dirty() && !self.tabs.approved.contains(&(id, document.key()))
            })
        });
        if let Some(id) = unapproved {
            self.activate_tab(id, context);
            self.document_workflow.queue_replacement(
                self.document.key(),
                self.is_dirty(),
                &self.document.name(),
                DeferredDocumentAction::CloseWindow,
                "closing this window",
            );
        } else {
            self.document_workflow.allow_close_for(self.document.key());
            if !self.process_close_pending {
                context.send_viewport_cmd(egui::ViewportCommand::Close);
            }
        }
    }

    pub(super) fn select_preview_tab(&mut self, id: u64, context: &egui::Context) {
        if self.tabs.preview_id() == Some(id) && self.tabs.preview_explicit {
            return;
        }
        if self
            .document_for_tab(id)
            .is_none_or(|d| !d.kind().is_typst())
        {
            return;
        }
        self.tabs.preview = self.tabs.index_of(id).unwrap();
        self.tabs.preview_explicit = true;
        self.restart_tinymist_preserving_preview();
        self.schedule_compile_now();
        self.schedule_project_index();
        context.request_repaint();
    }

    pub(super) fn show_tabs(&mut self, ui: &mut egui::Ui, frame: Option<&eframe::Frame>) {
        let trace_sample = self.tabs.drag_trace.sample(ui.ctx());
        let mut trace_widgets = String::new();
        let mut select = None;
        let mut close = None;
        let mut preview = None;
        let mut rename = None;
        let mut pressed_tab = None;
        let mut tab_rects = Vec::new();
        let height = METRICS.toolbar.title_height;
        let width = ui.available_width().max(0.0);
        if width < 1.0 {
            if trace_sample {
                self.tabs
                    .drag_trace
                    .record(ui.ctx(), format!("outcome=no-space width={width}"));
            }
            self.tabs.tab_drag_rects.clear();
            self.tabs.tab_drag_active = false;
            self.tabs.tab_drag_source = None;
            return;
        }
        let reveal = std::mem::take(&mut self.tabs.reveal_active);
        let tab_viewport = ui
            .allocate_ui_with_layout(
                Vec2::new(width, height),
                Layout::left_to_right(Align::Center),
                |ui| {
                    egui::ScrollArea::horizontal()
                        .id_salt("document-tabs")
                        .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysHidden)
                        .auto_shrink([false, true])
                        .max_height(height)
                        .show(ui, |ui| {
                            ui.horizontal(|ui| {
                                for index in 0..self.tabs.len() {
                                    let id = self.tabs.id_at(index).unwrap();
                                    let document = self.document_for_tab(id).unwrap();
                                    let name = document.name();
                                    let active = Some(id) == self.tabs.active_id();
                                    ui.push_id(id, |ui| {
                                        let compatible = document.kind().is_typst();
                                        let title_width = tab_title_width(
                                            &name,
                                            width,
                                            self.settings.fixed_tab_width,
                                            compatible,
                                        );
                                        let tab = tab_widget(
                                            ui,
                                            document,
                                            active,
                                            Some(id) == self.tabs.preview_id(),
                                            title_width,
                                        );
                                        tab_rects.push((id, tab.rect));
                                        if active && reveal {
                                            ui.scroll_to_rect(tab.rect, Some(Align::Center));
                                        }
                                        let response = tab.title;
                                        if trace_sample && index < 16 {
                                            use std::fmt::Write as _;
                                            let _ = write!(trace_widgets,
                                                " tab={} widget={:?} rect={:?} clip={:?} contains={} hovered={} owns={} dragged={} stopped={} clicked={};",
                                                id, response.id, response.rect, ui.clip_rect(),
                                                response.contains_pointer(), response.hovered(), response.is_pointer_button_down_on(),
                                                response.dragged(), response.drag_stopped(), response.clicked());
                                        }
                                        // Register ownership on the title itself, so neither
                                        // the scroll area nor the native title bar takes it.
                                        if response.is_pointer_button_down_on()
                                            && ui.input(|input| input.pointer.primary_down())
                                        {
                                            pressed_tab = Some(id);
                                        }
                                        if response.hovered() {
                                            native_hover_text(
                                                response.clone(),
                                                document.path().as_ref().map_or_else(
                                                    || "Unsaved document".into(),
                                                    |path| path.display().to_string(),
                                                ),
                                            );
                                        }
                                        if response.clicked() {
                                            select = Some(id);
                                        }
                                        if response.double_clicked() {
                                            rename = Some(id);
                                        }
                                        if response.clicked_by(egui::PointerButton::Middle) {
                                            close = Some(id);
                                        }
                                        if let Some(tab_preview) = tab.preview
                                            && tab_preview.clicked()
                                        {
                                            preview = Some(id);
                                        }
                                        if tab.close.clicked() {
                                            close = Some(id);
                                        }
                                        ui.separator();
                                    });
                                }
                            });
                        })
                },
            )
            .inner
            .inner_rect
            .intersect(ui.clip_rect());
        if let Some(id) = pressed_tab {
            self.tabs.tab_drag_source = Some(id);
        }
        let (primary_down, primary_released, pointer, decidedly_dragging) =
            ui.ctx().input(|input| {
                (
                    input.pointer.primary_down(),
                    input.pointer.button_released(egui::PointerButton::Primary),
                    input.pointer.latest_pos(),
                    input.pointer.is_decidedly_dragging(),
                )
            });
        // A release and the final movement can arrive together. Apply the
        // destination before clearing the gesture, including on that frame.
        let source_before = self.tabs.tab_drag_source;
        let mut outcome = if source_before.is_some() {
            "below-threshold"
        } else {
            "no-source"
        };
        if primary_down || primary_released {
            self.tabs.tab_drag_active = self.tabs.tab_drag_source.is_some();
            if let (Some(source), Some(pointer)) = (self.tabs.tab_drag_source, pointer)
                && decidedly_dragging
            {
                ui.ctx().set_cursor_icon(egui::CursorIcon::Grabbing);
                outcome = "no-drop-target";
                if let Some(from) = self.tabs.ids.iter().position(|id| *id == source)
                    && let Some(mut insertion) = tab_drop_index(&tab_rects, pointer)
                {
                    if insertion > from {
                        insertion -= 1;
                    }
                    if self.tabs.reorder(from, insertion) {
                        outcome = "reordered";
                        ui.ctx().request_repaint();
                    } else {
                        outcome = "same-slot";
                    }
                }
            }
        }
        if trace_sample {
            self.tabs.drag_trace.record(ui.ctx(), format!(
                "source={source_before:?} pressed_tab={pressed_tab:?} outcome={outcome} strip={tab_viewport:?} native_drag_suppressed={} tab_count={} order={:?} widgets(first16)={trace_widgets}",
                self.tabs.native_drag_suppressed(), self.tabs.len(), &self.tabs.ids[..self.tabs.ids.len().min(16)]));
        }
        if !primary_down {
            self.tabs.tab_drag_source = None;
            self.tabs.tab_drag_active = false;
        }
        self.tabs.tab_drag_rects.clear();
        self.tabs.tab_drag_rects.extend(
            tab_rects
                .iter()
                .map(|(_, rect)| rect.intersect(tab_viewport)),
        );
        if self.document_flow_busy() || self.process_close_pending {
            return;
        }
        if let Some(id) = close {
            self.request_close_tab(id, ui.ctx());
        } else if let Some(id) = preview {
            self.select_preview_tab(id, ui.ctx());
        } else if let Some(id) = rename {
            self.activate_tab(id, ui.ctx());
            if let Some(path) = self.document.path().clone() {
                self.begin_rename(path);
            } else {
                self.save_as(frame);
            }
        } else if let Some(id) = select {
            self.activate_tab(id, ui.ctx());
        }
    }

    pub(super) fn tick_parked_autosave(&mut self, context: &egui::Context) {
        let Some(deadline) = self.tabs.next_autosave else {
            return;
        };
        if !self.settings.auto_save || self.snapshot_scene.is_some() {
            return;
        }
        let now = Instant::now();
        if now < deadline {
            context.request_repaint_after(deadline - now);
            return;
        }
        if self.document_flow_busy() || self.process_close_pending {
            return;
        }
        let candidate = self
            .tabs
            .parked
            .iter_mut()
            .enumerate()
            .find_map(|(index, slot)| {
                let tab = slot.as_mut()?;
                if tab.autosave.is_none_or(|deadline| deadline > now) {
                    return None;
                }
                tab.autosave = None;
                (tab.document.is_dirty())
                    .then(|| tab.document.path().clone())
                    .flatten()
                    .map(|path| (index, path))
            });
        self.tabs.refresh_autosave();
        if let Some((index, path)) = candidate {
            self.submit_save(
                self.tabs.id_at(index).unwrap(),
                path,
                SaveIntent::Auto,
                false,
                context,
            );
        }
    }
}

struct TabResponse {
    rect: Rect,
    title: egui::Response,
    preview: Option<egui::Response>,
    close: egui::Response,
}

const FIXED_TAB_WIDTH: f32 = 160.0;
const TAB_BUTTON_WIDTH: f32 = 18.0;
const TAB_ITEM_SPACING: f32 = 2.0;

fn tab_title_width(name: &str, available_width: f32, fixed_width: bool, compatible: bool) -> f32 {
    let controls_width = TAB_BUTTON_WIDTH
        + TAB_ITEM_SPACING
        + if compatible {
            TAB_BUTTON_WIDTH + TAB_ITEM_SPACING
        } else {
            0.0
        };
    if fixed_width {
        return (FIXED_TAB_WIDTH - controls_width).max(24.0);
    }

    ((name.chars().count() + 1) as f32 * METRICS.toolbar.title_character_width
        + METRICS.toolbar.title_padding)
        .clamp(54.0, 200.0)
        .min((available_width - controls_width - TAB_ITEM_SPACING).max(24.0))
}

fn moved_index(index: usize, from: usize, to: usize) -> usize {
    if index == from {
        to
    } else if from < to && (from..=to).contains(&index) {
        index - 1
    } else if to < from && (to..=from).contains(&index) {
        index + 1
    } else {
        index
    }
}

fn tab_drop_index(tab_rects: &[(u64, Rect)], pointer: Pos2) -> Option<usize> {
    let first = tab_rects.first()?.1;
    let last = tab_rects.last()?.1;
    if pointer.y < first.top() || pointer.y > last.bottom() {
        return None;
    }
    tab_rects
        .iter()
        .position(|(_, rect)| pointer.x < rect.center().x)
        .or(Some(tab_rects.len()))
}

fn tab_widget(
    ui: &mut egui::Ui,
    document: &DocumentSession,
    active: bool,
    chosen: bool,
    title_width: f32,
) -> TabResponse {
    let name = document.name();
    let height = METRICS.toolbar.title_height;
    let frame = egui::Frame::new()
        .fill(if active {
            ui.visuals().selection.bg_fill
        } else {
            Color32::TRANSPARENT
        })
        .corner_radius(ui.visuals().widgets.inactive.corner_radius)
        .show(ui, |ui| {
            ui.spacing_mut().item_spacing.x = 2.0;
            ui.horizontal(|ui| {
                let title = ui.add_sized(
                    [title_width, height],
                    egui::Button::new(
                        RichText::new(format!(
                            "{name}{}",
                            if document.is_dirty() { "*" } else { "" }
                        ))
                        .strong(),
                    )
                    .frame(false)
                    .sense(Sense::click_and_drag())
                    .truncate(),
                );
                let preview = document.kind().is_typst().then(|| {
                    let preview = tab_icon_button(
                        ui,
                        true,
                        if chosen {
                            UiIcon::Eye
                        } else {
                            UiIcon::EyeClosed
                        },
                        &format!("Preview {name}"),
                    );
                    native_hover_text(
                        preview,
                        if chosen {
                            "Preview source"
                        } else {
                            "Use this tab for preview"
                        },
                    )
                });
                let close = tab_icon_button(ui, true, UiIcon::Close, &format!("Close {name}"));
                let close = native_hover_text(close, format!("Close {name}"));
                (title, preview, close)
            })
            .inner
        });
    let (title, preview, close) = frame.inner;
    TabResponse {
        rect: frame.response.rect,
        title,
        preview,
        close,
    }
}

fn tab_icon_rect(button: Rect, icon: UiIcon) -> Rect {
    Rect::from_center_size(
        button.center(),
        Vec2::splat(if icon == UiIcon::Close { 8.0 } else { 14.0 }),
    )
}

fn tab_icon_button(ui: &mut egui::Ui, enabled: bool, icon: UiIcon, label: &str) -> egui::Response {
    let response = ui
        .add_enabled_ui(enabled, |ui| {
            ui.add_sized(
                [18.0, METRICS.toolbar.title_height],
                egui::Button::new("").frame(false),
            )
        })
        .inner;
    response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, enabled, label));
    let color = if enabled {
        ui.style().interact(&response).fg_stroke.color
    } else {
        ui.visuals().weak_text_color()
    };
    paint_ui_icon(
        ui.painter(),
        tab_icon_rect(response.rect, icon),
        icon,
        color,
    );
    response
}

fn parked_rename_kind(document: &DocumentSession, path: &Path) -> DocumentKind {
    if document.kind().preview_only() {
        document.kind()
    } else {
        crate::document::detect_document(path, document.source().as_bytes())
            .unwrap_or(DocumentKind::Text)
    }
}
