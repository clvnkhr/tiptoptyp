//! Tabs own stable identity and ordered document records, never an EditorApp or
//! background service. The first tab is the initial preview source.
use super::*;
mod trace;

pub(super) struct TabRecord {
    pub(super) document: DocumentSession,
    pub(super) folding: crate::folding::Folding,
    pub(super) editor: Option<egui::text_edit::TextEditState>,
    pub(super) autosave: Option<Instant>,
    pub(super) workspace: PathBuf,
}

#[cfg(test)]
#[path = "tabs_tests.rs"]
mod tests;

pub(super) struct Tabs {
    records: BTreeMap<u64, TabRecord>,
    order: Vec<u64>,
    active: Option<u64>,
    preview: Option<u64>,
    preview_explicit: bool,
    next_id: u64,
    empty_record: TabRecord,
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
}

impl Default for Tabs {
    fn default() -> Self {
        let owner = tiptoptyp_core::document::WindowSessionId::new(0);
        Self {
            records: BTreeMap::new(),
            order: Vec::new(),
            active: None,
            preview: None,
            preview_explicit: false,
            next_id: 0,
            empty_record: TabRecord {
                document: DocumentSession::new(owner, "", DocumentKind::Text),
                folding: Default::default(),
                editor: None,
                autosave: None,
                workspace: PathBuf::new(),
            },
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
    pub(super) fn new(
        preview_explicit: bool,
        document: DocumentSession,
        workspace: PathBuf,
    ) -> Self {
        let mut tabs = Self::default();
        let id = tabs.next_id;
        tabs.next_id += 1;
        tabs.records.insert(
            id,
            TabRecord {
                document,
                folding: Default::default(),
                editor: None,
                autosave: None,
                workspace,
            },
        );
        tabs.order.push(id);
        tabs.active = Some(id);
        tabs.preview = Some(id);
        tabs.preview_explicit = preview_explicit;
        tabs
    }
    pub(super) fn len(&self) -> usize {
        self.order.len()
    }
    pub(super) fn is_empty(&self) -> bool {
        self.order.is_empty()
    }
    pub(super) fn empty_after(&self) -> Self {
        let owner = self.active_record().map_or_else(
            || self.empty_record.document.key().owner,
            |record| record.document.key().owner,
        );
        Self {
            records: BTreeMap::new(),
            order: Vec::new(),
            active: None,
            preview: None,
            next_id: self.next_id,
            empty_record: TabRecord {
                document: DocumentSession::new(owner, "", DocumentKind::Text),
                folding: Default::default(),
                editor: None,
                autosave: None,
                workspace: PathBuf::new(),
            },
            preview_explicit: true,
            ..Self::default()
        }
    }
    fn open_first(&mut self, workspace: PathBuf) {
        assert!(self.is_empty());
        let id = self.next_id;
        self.next_id += 1;
        let owner = self.empty_record.document.key().owner;
        self.records.insert(
            id,
            TabRecord {
                document: DocumentSession::new(owner, "", DocumentKind::Typst),
                folding: Default::default(),
                editor: None,
                autosave: None,
                workspace,
            },
        );
        self.order.push(id);
        self.active = Some(id);
        self.preview = Some(id);
        self.preview_explicit = true;
    }
    fn active_record(&self) -> Option<&TabRecord> {
        self.active.and_then(|id| self.records.get(&id))
    }
    pub(super) fn current_record(&self) -> &TabRecord {
        self.active_record().unwrap_or(&self.empty_record)
    }
    pub(super) fn current_record_mut(&mut self) -> &mut TabRecord {
        let Some(id) = self.active else {
            return &mut self.empty_record;
        };
        self.records.get_mut(&id).expect("active tab record")
    }
    pub(super) fn document_mut(&mut self, id: u64) -> Option<&mut DocumentSession> {
        self.index_of(id)?;
        self.records.get_mut(&id).map(|tab| &mut tab.document)
    }
    pub(super) fn active_id(&self) -> Option<u64> {
        self.active
    }
    pub(super) fn preview_id(&self) -> Option<u64> {
        self.preview
    }
    pub(super) fn active_index(&self) -> Option<usize> {
        self.active.and_then(|id| self.index_of(id))
    }
    pub(super) fn ids(&self) -> impl ExactSizeIterator<Item = u64> + '_ {
        self.order.iter().copied()
    }

    #[cfg(test)]
    pub(super) fn set_active_for_test(&mut self, id: u64) {
        assert!(self.index_of(id).is_some(), "test tab must exist");
        self.active = Some(id);
    }

    pub(super) fn uses_designated_preview(&self) -> bool {
        self.len() > 1 || self.preview_explicit
    }
    pub(super) fn id_at(&self, index: usize) -> Option<u64> {
        self.order.get(index).copied()
    }
    pub(super) fn index_of(&self, id: u64) -> Option<usize> {
        self.order.iter().position(|&value| value == id)
    }

    pub(super) fn claims_window_drag(&self, pointer: Option<Pos2>, primary_down: bool) -> bool {
        pointer.is_some_and(|pointer| {
            self.tab_drag_rects
                .iter()
                .any(|rect| rect.contains(pointer))
        }) || (self.tab_drag_active && primary_down)
    }

    #[cfg(feature = "profiling")]
    pub(super) fn profile_tab_centers(&self) -> [Option<Pos2>; 3] {
        let mut centers = [None; 3];
        for (index, rect) in self.tab_drag_rects.iter().take(3).enumerate() {
            centers[index] = Some(rect.center());
        }
        centers
    }

    /// Move one tab to another position without changing which document is
    /// active or which document drives the preview.
    pub(super) fn reorder(&mut self, from: usize, to: usize) -> bool {
        if from >= self.len() || to >= self.len() || from == to {
            return false;
        }

        let id = self.order.remove(from);
        self.order.insert(to, id);
        true
    }
    fn refresh_autosave(&mut self) {
        self.next_autosave = self.records.values().filter_map(|tab| tab.autosave).min();
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
    pub(super) fn document(&self) -> &DocumentSession {
        &self.tabs.current_record().document
    }

    pub(super) fn document_mut(&mut self) -> &mut DocumentSession {
        &mut self.tabs.current_record_mut().document
    }

    pub(super) fn folding(&self) -> &crate::folding::Folding {
        &self.tabs.current_record().folding
    }

    pub(super) fn folding_mut(&mut self) -> &mut crate::folding::Folding {
        &mut self.tabs.current_record_mut().folding
    }

    pub(super) fn active_autosave_deadline(&self) -> Option<Instant> {
        self.tabs.current_record().autosave
    }

    pub(super) fn set_active_autosave_deadline(&mut self, deadline: Option<Instant>) {
        self.tabs.current_record_mut().autosave = deadline;
        self.tabs.refresh_autosave();
    }

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
        let methods_path = self.workspace_root.join("methods.typ");
        self.document_mut().replace_loaded_unprojected(
            "= Methods\n\nDraft notes for the next section.\n".into(),
            methods_path,
            DocumentKind::Typst,
            None,
        );
        self.document_mut().edit(CCursorRange::default(), |source| {
            source.push_str("\nAn unsaved revision.\n")
        });
        self.append_tab(context);
        let notes_path = self.workspace_root.join("notes.md");
        self.document_mut().replace_loaded_unprojected(
            "# Research notes\n".into(),
            notes_path,
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
        self.document_mut()
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
        let id = self.tabs.id_at(index)?;
        self.tabs.records.get(&id).map(|tab| &tab.document)
    }
    pub(super) fn tab_preview_document(&self) -> Option<&DocumentSession> {
        self.document_for_tab(self.tabs.preview_id()?)
    }
    pub(super) fn document_for_tab(&self, id: u64) -> Option<&DocumentSession> {
        self.document_at_index(self.tabs.index_of(id)?)
    }
    pub(super) fn set_tab_autosave(&mut self, id: u64, deadline: Option<Instant>) {
        if self.tabs.index_of(id).is_none() {
            return;
        }
        if let Some(tab) = self.tabs.records.get_mut(&id) {
            tab.autosave = deadline;
        }
        self.tabs.refresh_autosave();
    }
    pub(super) fn tab_preview_root(&self) -> &Path {
        let preview = self.tabs.preview_id().and_then(|id| {
            let document = self.document_for_tab(id)?;
            let root = self.tab_workspace(id)?;
            Some((root, document.kind().is_typst()))
        });
        crate::tinymist_sync::preview_root(&self.workspace_root, preview)
    }
    pub(super) fn tab_workspace(&self, id: u64) -> Option<&Path> {
        self.tabs
            .records
            .get(&id)
            .map(|tab| tab.workspace.as_path())
    }

    pub(super) fn set_tab_workspace(&mut self, id: u64, workspace: PathBuf) {
        if let Some(record) = self.tabs.records.get_mut(&id) {
            record.workspace = workspace;
        }
    }
    pub(super) fn untitled_tab_path(&self, id: u64) -> PathBuf {
        if let Some(backing) = self.tinymist_sync.tab_backings.get(&id) {
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
            if self.tinymist_sync.tab_backings.contains_key(&id) {
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
            self.tinymist_sync.tab_backings.insert(id, backing);
        }
        Ok(())
    }

    pub(super) fn sync_parked_tinymist(&mut self) {
        if self.tinymist_sync.generation.is_none() {
            return;
        }
        let mut inputs = Vec::new();
        for id in self.tabs.ids() {
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
            if let Ok(input) = crate::tinymist_sync::collect(document, &path) {
                inputs.push(input);
            }
        }
        for input in inputs {
            let batch = self.tinymist_sync.open_background(input);
            let _ = self.apply_tinymist_sync_batch(batch);
        }
    }

    pub(super) fn rename_parked_tab(&mut self, old: &Path, new: &Path) {
        let active = self.tabs.active;
        let mut renamed_preview = false;
        for (id, tab) in &mut self.tabs.records {
            if Some(*id) == active {
                continue;
            }
            if tab.document.path().as_deref() == Some(old) {
                renamed_preview |= self.tabs.preview == Some(*id);
                let kind = parked_rename_kind(&tab.document, new);
                // Renaming through Explorer must update the parked buffer,
                // not leave it autosaving back to the old filename.
                tab.document
                    .rename(new.into(), kind)
                    .expect("parked rename preflight");
            }
        }
        if let Ok(uri) = crate::tinymist::path_to_file_uri(old)
            && let Some(effect) = self.tinymist_sync.close_uri(&uri)
        {
            let _ = self.apply_tinymist_sync_batch(crate::tinymist_sync::Batch {
                source: String::new(),
                effects: vec![effect],
            });
        }
        if renamed_preview {
            self.restart_tinymist_for_preview_entry();
            self.schedule_compile_now();
        } else {
            self.sync_parked_tinymist();
        }
    }

    pub(super) fn preflight_parked_rename(&self, old: &Path, new: &Path) -> Result<(), String> {
        if self.path_open_in_another_tab(new) {
            return Err("The destination is open in another tab".into());
        }
        for tab in self
            .tabs
            .records
            .iter()
            .filter(|(id, tab)| {
                Some(**id) != self.tabs.active && tab.document.path().as_deref() == Some(old)
            })
            .map(|(_, tab)| tab)
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
        let active = self.tabs.active;
        for (id, tab) in &mut self.tabs.records {
            if Some(*id) == active {
                continue;
            }
            tab.autosave = (self.settings.auto_save
                && tab.document.path().is_some()
                && tab.document.is_dirty())
            .then_some(deadline);
        }
        self.tabs.refresh_autosave();
    }

    fn activate_record(&mut self, id: u64, context: &egui::Context) {
        let previous = self.document().key();
        let previous_id = self.tabs.active;
        if let Some(backing) = self.tinymist_sync.active_backing.take()
            && let Some(previous_id) = previous_id
        {
            self.tinymist_sync.tab_backings.insert(previous_id, backing);
        }
        let outgoing_editor =
            egui::text_edit::TextEditState::load(context, source_editor_id(context));
        let current_workspace = self.workspace_root.clone();
        if let Some(previous_id) = previous_id
            && let Some(previous_record) = self.tabs.records.get_mut(&previous_id)
        {
            previous_record.editor = outgoing_editor;
            previous_record.workspace = current_workspace;
        }

        let incoming = self.tabs.records.get_mut(&id).expect("target tab record");
        let old_key = incoming.document.key();
        incoming.document.reactivate_after(previous);
        incoming.folding.rekey(old_key, incoming.document.key());
        let new_key = incoming.document.key();
        let incoming_editor = incoming.editor.take().unwrap_or_default();
        let incoming_workspace = incoming.workspace.clone();
        // Close approvals survive a pure view switch, never an intervening edit.
        for (_, key) in &mut self.tabs.approved {
            if *key == old_key {
                *key = new_key;
            }
        }
        self.tabs.active = Some(id);
        self.workspace_root = incoming_workspace;
        incoming_editor.store(context, source_editor_id(context));
        self.reset_transient_editor_state();
        self.last_editor_caret = None;
        self.editor_completion = None;
        self.editor_hover = None;
        self.tooltip_request = None;
        self.close_app_popup();
        self.document_workflow.revoke_close();
    }

    fn append_tab(&mut self, context: &egui::Context) {
        let id = self.tabs.next_id;
        self.tabs.next_id += 1;
        self.tabs.records.insert(
            id,
            TabRecord {
                document: DocumentSession::new(
                    self.document().key().owner,
                    "",
                    DocumentKind::Typst,
                ),
                folding: Default::default(),
                editor: None,
                autosave: None,
                workspace: self.workspace_root.clone(),
            },
        );
        self.tabs.order.push(id);
        self.activate_record(id, context);
        self.tabs.reveal_active = true;
        self.tabs.refresh_autosave();
    }

    pub(super) fn new_tab(&mut self, context: &egui::Context) {
        if self.tabs.is_empty() {
            self.tabs.open_first(self.workspace_root.clone());
        } else if self.lifecycle.allows_document_work() {
            self.append_tab(context);
        }
        self.reset_untitled_document();
    }

    pub(super) fn open_tab_path(&mut self, path: PathBuf, context: &egui::Context) -> bool {
        let path = canonical_or_absolute(&path);
        if self.tabs.is_empty() {
            self.tabs.open_first(self.workspace_root.clone());
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
            self.tabs.len() == 1 && self.document().path().is_none() && !self.is_dirty();
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
        if self.table_editor.is_some() {
            return;
        }
        if self.tabs.index_of(id).is_none() {
            return;
        }
        if Some(id) == self.tabs.active_id() {
            return;
        }
        let preserve_preview = self.typst_preview_available();
        let workspace_changed = self.tabs.records[&id].workspace != self.workspace_root;
        self.activate_record(id, context);
        self.tabs.reveal_active = true;
        self.tabs.refresh_autosave();
        self.clear_preview_for_document(preserve_preview);
        self.git_editor.clear_document();
        if workspace_changed {
            self.reset_document_services();
        } else if let Some(path) = self.document().path().clone() {
            if self.tinymist_sync.generation.is_some() {
                self.reopen_tinymist_current_document(&path, self.document().kind());
            } else {
                self.restart_tinymist();
            }
        } else {
            self.restart_tinymist_preserving_preview();
        }
        if self.document().kind().preview_only()
            && let Some(path) = self.document().path().clone()
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
        assert_ne!(Some(id), self.tabs.active);
        if let Some(document) = self.document_for_tab(id) {
            let path = document
                .path()
                .clone()
                .unwrap_or_else(|| self.untitled_tab_path(id));
            if let Ok(uri) = crate::tinymist::path_to_file_uri(&path)
                && let Some(effect) = self.tinymist_sync.close_uri(&uri)
            {
                let _ = self.apply_tinymist_sync_batch(crate::tinymist_sync::Batch {
                    source: String::new(),
                    effects: vec![effect],
                });
            }
        }
        let removed = self.tabs.order.remove(index);
        debug_assert_eq!(removed, id);
        self.tinymist_sync.tab_backings.remove(&id);
        self.tabs.records.remove(&id);
        self.tabs.approved.retain(|(approved, _)| *approved != id);
        if self.tabs.preview == Some(id) {
            self.tabs.preview = self.tabs.order.first().copied();
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
            self.restart_tinymist_for_preview_entry();
            self.schedule_compile_now();
        }
    }

    pub(super) fn tabs_close_approved(&self) -> bool {
        self.tabs.records.iter().all(|(id, tab)| {
            !tab.document.is_dirty() || self.tabs.approved.contains(&(*id, tab.document.key()))
        })
    }

    pub(super) fn approve_tab_window_close(&mut self) {
        let id = self
            .tabs
            .active_id()
            .expect("approval requires an active tab");
        self.tabs.approved.push((id, self.document().key()));
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
                self.document().key(),
                self.is_dirty(),
                &self.document().name(),
                DeferredDocumentAction::CloseWindow,
                "closing this window",
            );
        } else {
            self.document_workflow
                .allow_close_for(self.document().key());
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
        self.tabs.preview = Some(id);
        self.tabs.preview_explicit = true;
        self.restart_tinymist_for_preview_entry();
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
                if let Some(from) = self.tabs.order.iter().position(|id| *id == source)
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
                self.tabs.native_drag_suppressed(), self.tabs.len(), &self.tabs.order[..self.tabs.order.len().min(16)]));
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
            if let Some(path) = self.document().path().clone() {
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
        let active = self.tabs.active;
        let candidate = self.tabs.records.iter_mut().find_map(|(id, tab)| {
            if Some(*id) == active {
                return None;
            }
            if tab.autosave.is_none_or(|deadline| deadline > now) {
                return None;
            }
            tab.autosave = None;
            (tab.document.is_dirty())
                .then(|| tab.document.path().clone())
                .flatten()
                .map(|path| (*id, path))
        });
        self.tabs.refresh_autosave();
        if let Some((id, path)) = candidate {
            self.submit_save(id, path, SaveIntent::Auto, false, context);
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
