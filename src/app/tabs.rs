//! Tabs own stable identity and ordered document records, never an EditorApp or
//! background service. The first tab is the initial preview source.
use super::*;
mod motion;
mod trace;

pub(super) struct TabRecord {
    pub(super) document: DocumentSession,
    pub(super) folding: crate::folding::Folding,
    pub(super) editor: Option<egui::text_edit::TextEditState>,
    pub(super) autosave: Option<Instant>,
    pub(super) workspace: PathBuf,
    pub(super) pdf_output: super::pdf_output::PdfOutput,
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
    tab_motion: BTreeMap<u64, motion::Motion>,
    tab_drag_grab: f32,
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
                pdf_output: Default::default(),
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
            tab_motion: BTreeMap::new(),
            tab_drag_grab: 0.0,
            #[cfg(target_os = "macos")]
            native_drag_guard: None,
        }
    }
}
impl Tabs {
    pub(super) fn set_workspace(&mut self, root: &Path) {
        for record in self.records.values_mut() {
            record.workspace = root.to_owned();
        }
    }
    #[cfg(target_os = "macos")]
    pub(super) fn suppress_native_drag(
        &mut self,
        parent: Option<&crate::native_window::NativeWindowHandle>,
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
                pdf_output: Default::default(),
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
    pub(super) fn has_unsaved_changes(&self) -> bool {
        self.records
            .values()
            .any(|record| record.document.is_dirty())
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
                pdf_output: Default::default(),
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
                pdf_output: Default::default(),
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
    pub(super) fn pdf_record(&self, tab: Option<u64>) -> &TabRecord {
        tab.and_then(|id| self.records.get(&id))
            .unwrap_or(&self.empty_record)
    }
    pub(super) fn pdf_record_mut(&mut self, tab: Option<u64>) -> &mut TabRecord {
        match tab {
            Some(id) => self.records.get_mut(&id).expect("PDF source tab"),
            None => &mut self.empty_record,
        }
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

    pub(super) fn adopt_active_preview(&mut self) {
        self.preview = self.active;
        self.preview_explicit = false;
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
            Some((
                root,
                LanguageSupport::for_document(document.kind())
                    .build
                    .is_some(),
            ))
        });
        crate::language_support::preview_root(&self.workspace_root, preview)
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
        incoming.pdf_output.rekey(old_key, new_key);
        if let Some(pending) = &mut self.document_workflow.pending_export
            && pending.source.tab == Some(id)
            && pending.source.epoch == old_key.epoch
        {
            pending.source.epoch = new_key.epoch;
        }
        if let Some(dialog) = &mut self.document_workflow.pending_export_dialog {
            dialog.rekey_pdf_source(Some(id), old_key.epoch, new_key.epoch);
        }
        let incoming_editor = incoming.editor.take().unwrap_or_default();
        // Close approvals survive a pure view switch, never an intervening edit.
        for (_, key) in &mut self.tabs.approved {
            if *key == old_key {
                *key = new_key;
            }
        }
        self.tabs.active = Some(id);
        incoming_editor.store(context, source_editor_id(context));
        self.snippet_session = None;
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
                pdf_output: Default::default(),
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
        self.open_tab_path_with(canonical_or_absolute(&path), context, |app, path| {
            app.load_path(path)
        })
    }

    pub(super) fn open_git_file_diff_path(
        &mut self,
        path: PathBuf,
        context: &egui::Context,
    ) -> bool {
        let started = Instant::now();
        let Some((source, kind)) = crate::git::repository::diff::read_editable_source(&path) else {
            return false;
        };
        let disk_fingerprint = Some(fingerprint(source.as_bytes()));
        self.open_tab_path_with(path, context, move |app, path| {
            app.load_prepared_path(path, source, kind, disk_fingerprint, started)
        })
    }

    fn open_tab_path_with(
        &mut self,
        path: PathBuf,
        context: &egui::Context,
        load: impl FnOnce(&mut Self, PathBuf) -> bool,
    ) -> bool {
        if self.tabs.is_empty() {
            self.tabs.open_first(self.workspace_root.clone());
            if load(self, path) {
                return true;
            }
            self.empty_workspace(context);
            return false;
        }
        let existing = self.source_tab_for_path(&path);
        if let Some(id) = existing {
            self.activate_tab(id, context);
            return true;
        }
        let replace_welcome =
            self.tabs.len() == 1 && self.document().path().is_none() && !self.is_dirty();
        if replace_welcome || !self.lifecycle.allows_document_work() {
            return load(self, path);
        }
        let old = self.tabs.active_id().expect("non-empty tab set");
        self.append_tab(context);
        if load(self, path) {
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
        self.close_version_history(context);
        let previous_kind = self.document().kind();
        let preserve_preview = self.source_preview_available();
        self.activate_record(id, context);
        self.tabs.reveal_active = true;
        self.tabs.refresh_autosave();
        self.clear_preview_for_document(preserve_preview);
        // Preview ownership and editor ownership are independent. A pinned PDF
        // may survive this switch; editor-provider results belong to the old tab.
        self.preview.tinymist_diagnostics.clear();
        self.sync_tex(context);
        self.update_writing_checks(context);
        self.update_tex_diagnostics();
        self.git_editor.clear_document();
        if previous_kind == DocumentKind::Tex && self.document().kind().is_typst() {
            // A retained preview service may have no active editor diagnostics
            // after a TeX session. Reopen the current source through initialization
            // so even an unchanged buffer receives a fresh publication.
            self.restart_tinymist_preserving_preview();
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
        if self.source_preview_available()
            && (self.document_workflow.pending_export.is_some()
                || switch_needs_compile(
                    preserve_preview,
                    self.pdfium_preview_requested(),
                    self.preview.content.pdf().is_some(),
                    matches!(
                        self.preview.status,
                        PreviewStatus::Compiling | PreviewStatus::Waiting
                    ),
                ))
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
            self.web_search_query.clear();
            self.web_search_offset = 0;
            self.preview.content.clear();
            self.pdfium_preview = Default::default();
            let _ = self.compiler.pause(self.preview_document_revision());
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
            .is_none_or(|d| LanguageSupport::for_document(d.kind()).build.is_none())
        {
            return;
        }
        self.tabs.preview = Some(id);
        self.tabs.preview_explicit = true;
        self.cancel_pdf_output_if_source_changed();
        self.web_search_query.clear();
        self.web_search_offset = 0;
        let _ = self.compiler.pause(self.preview_document_revision());
        self.preview
            .clear_for_document(self.preview_document_revision(), false);
        self.update_tex_diagnostics();
        self.pdfium_preview = Default::default();
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
        let mut tab_visuals = Vec::new();
        let (primary_down, primary_released, pointer, decidedly_dragging, time) =
            ui.input(|input| {
                (
                    input.pointer.primary_down(),
                    input.pointer.button_released(egui::PointerButton::Primary),
                    input.pointer.latest_pos(),
                    input.pointer.is_decidedly_dragging(),
                    input.time,
                )
            });
        let source_before = self.tabs.tab_drag_source;
        let dragged =
            source_before.filter(|_| decidedly_dragging && (primary_down || primary_released));
        let mut moving = false;
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
                                let origin = ui.cursor().left();
                                for index in 0..self.tabs.len() {
                                    let id = self.tabs.id_at(index).unwrap();
                                    let document = self.document_for_tab(id).unwrap();
                                    let name = document.name();
                                    let compatible = LanguageSupport::for_document(document.kind()).build.is_some();
                                    let title_width = tab_title_width(&name, width, self.settings.fixed_tab_width, compatible);
                                    let active = Some(id) == self.tabs.active_id();
                                    ui.push_id(id, |ui| {
                                        let controls = if compatible { 2.0 } else { 1.0 };
                                        let (slot, _) = ui.allocate_exact_size(
                                            Vec2::new(title_width + controls * (TAB_BUTTON_WIDTH + TAB_ITEM_SPACING), height),
                                            Sense::hover(),
                                        );
                                        let target = slot.left() - origin;
                                        let grabbed_x = (dragged == Some(id)).then(|| pointer.map_or(target, |pointer| pointer.x - self.tabs.tab_drag_grab - origin));
                                        let motion = self.tabs.tab_motion.entry(id).or_insert_with(|| motion::Motion::new(target));
                                        let mut x = motion.update(target, grabbed_x, time);
                                        if primary_released { x = motion.update(target, None, time); }
                                        moving |= motion.moving();
                                        let visual_slot = slot.translate(Vec2::new(x - target, 0.0));
                                        let mut child = ui.new_child(egui::UiBuilder::new()
                                            .id_salt("tab-content").max_rect(visual_slot)
                                            .layout(Layout::left_to_right(Align::Center)));
                                        let document = self.document_for_tab(id).unwrap();
                                        let mut tab = tab_widget(&mut child, document, active, Some(id) == self.tabs.preview_id(), title_width);
                                        // Drawing and input follow the animation. Reordering and
                                        // scroll-to-active use only these unanimated insertion slots.
                                        tab.rect = slot;
                                        tab_rects.push((id, slot));
                                        tab_visuals.push((id, tab.visual));
                                        if active && reveal {
                                            ui.scroll_to_rect(tab.rect, Some(Align::Center));
                                        }
                                        let response = tab.title;
                                        #[cfg(all(feature = "desktop-ui-tests", target_os = "macos"))]
                                        crate::desktop_test::observe(&format!("tab.{id}"), &response);
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
                                            pressed_tab = Some((id, response.rect.left()));
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
        if let Some((id, left)) = pressed_tab {
            if self.tabs.tab_drag_source != Some(id)
                && let Some(pointer) = ui.input(|input| input.pointer.press_origin())
            {
                self.tabs.tab_drag_grab = pointer.x - left;
            }
            self.tabs.tab_drag_source = Some(id);
        }
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
        let painter = ui.painter().with_clip_rect(tab_viewport);
        let mut lifted_visuals = Vec::new();
        for (id, visual) in tab_visuals {
            if self
                .tabs
                .tab_motion
                .get(&id)
                .is_some_and(motion::Motion::lifted)
            {
                lifted_visuals.push((id, visual));
            } else {
                visual.paint(&painter);
            }
        }
        // A second gesture can start before the first landing finishes. Keep
        // every landing visible, with the currently grabbed tab above them.
        lifted_visuals.sort_by_key(|(id, _)| dragged == Some(*id));
        for (_, mut visual) in lifted_visuals {
            if visual.fill == Color32::TRANSPARENT {
                visual.fill = ui.visuals().panel_fill;
            }
            visual.paint(&painter);
        }
        self.tabs
            .tab_motion
            .retain(|id, _| self.tabs.records.contains_key(id));
        if moving {
            ui.ctx().request_repaint();
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
    visual: TabVisual,
}

const FIXED_TAB_WIDTH: f32 = 160.0;
const TAB_BUTTON_WIDTH: f32 = 18.0;
const TAB_ITEM_SPACING: f32 = 2.0;
const TAB_FILE_ICON_WIDTH: f32 = 22.0;

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
        + METRICS.toolbar.title_padding
        + TAB_FILE_ICON_WIDTH)
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

struct TabVisual {
    rect: Rect,
    title_rect: Rect,
    title: Arc<egui::Galley>,
    color: Color32,
    fill: Color32,
    corner: egui::CornerRadius,
    file_icon: char,
    preview: Option<(Rect, UiIcon, Color32)>,
    close: (Rect, Color32),
}

impl TabVisual {
    fn paint(&self, painter: &egui::Painter) {
        let rect = self.rect;
        if !painter.clip_rect().intersects(rect) {
            return;
        }
        painter.rect_filled(rect, self.corner, self.fill);
        let title = self.title_rect;
        let icon_rect = Rect::from_center_size(
            Pos2::new(title.left() + TAB_FILE_ICON_WIDTH * 0.5, title.center().y),
            Vec2::splat(15.0),
        );
        if !crate::file_icons::paint(painter, icon_rect, self.file_icon, self.color) {
            paint_ui_icon(painter, icon_rect, UiIcon::File, self.color);
        }
        let text_rect = Rect::from_min_max(
            Pos2::new(title.left() + TAB_FILE_ICON_WIDTH, title.top()),
            title.max,
        );
        let position = Pos2::new(
            text_rect.left(),
            title.center().y - self.title.size().y * 0.5,
        );
        painter
            .with_clip_rect(painter.clip_rect().intersect(text_rect))
            .galley(position, self.title.clone(), self.color);
        if let Some((button, icon, color)) = self.preview {
            paint_ui_icon(painter, tab_icon_rect(button, icon), icon, color);
        }
        paint_ui_icon(
            painter,
            tab_icon_rect(self.close.0, UiIcon::Close),
            UiIcon::Close,
            self.close.1,
        );
    }
}

fn tab_widget(
    ui: &mut egui::Ui,
    document: &DocumentSession,
    active: bool,
    chosen: bool,
    title_width: f32,
) -> TabResponse {
    let name = document.name();
    let title_text = format!("{name}{}", if document.is_dirty() { "*" } else { "" });
    let height = METRICS.toolbar.title_height;
    let frame = egui::Frame::new().show(ui, |ui| {
        ui.spacing_mut().item_spacing.x = TAB_ITEM_SPACING;
        ui.horizontal(|ui| {
            let title = ui.add_sized(
                [title_width, height],
                egui::Button::new("")
                    .frame(false)
                    .sense(Sense::click_and_drag()),
            );
            title.widget_info(|| {
                egui::WidgetInfo::labeled(egui::WidgetType::Button, true, &title_text)
            });
            let preview = LanguageSupport::for_document(document.kind())
                .build
                .is_some()
                .then(|| {
                    let response = tab_icon_button(ui, true, &format!("Preview {name}"));
                    native_hover_text(
                        response,
                        if chosen {
                            "Preview source"
                        } else {
                            "Use this tab for preview"
                        },
                    )
                });
            let close = tab_icon_button(ui, true, &format!("Close {name}"));
            let close = native_hover_text(close, format!("Close {name}"));
            (title, preview, close)
        })
        .inner
    });
    let (title, preview, close) = frame.inner;
    let color = ui.visuals().strong_text_color();
    let font = egui::TextStyle::Button.resolve(ui.style());
    let mut job = egui::text::LayoutJob::simple(title_text, font, color, f32::INFINITY);
    job.wrap.max_width = (title.rect.width() - TAB_FILE_ICON_WIDTH).max(0.0);
    job.wrap.max_rows = 1;
    job.wrap.break_anywhere = true;
    let galley = ui.fonts_mut(|fonts| fonts.layout_job(job));
    let visual = TabVisual {
        rect: frame.response.rect,
        title_rect: title.rect,
        title: galley,
        color,
        fill: if active {
            ui.visuals().selection.bg_fill
        } else {
            Color32::TRANSPARENT
        },
        corner: ui.visuals().widgets.inactive.corner_radius,
        file_icon: crate::file_icons::for_path(document.path().as_deref()),
        preview: preview.as_ref().map(|response| {
            (
                response.rect,
                if chosen {
                    UiIcon::Eye
                } else {
                    UiIcon::EyeClosed
                },
                ui.style().interact(response).fg_stroke.color,
            )
        }),
        close: (close.rect, ui.style().interact(&close).fg_stroke.color),
    };
    TabResponse {
        rect: frame.response.rect,
        title,
        preview,
        close,
        visual,
    }
}

fn tab_icon_rect(button: Rect, icon: UiIcon) -> Rect {
    Rect::from_center_size(
        button.center(),
        Vec2::splat(if icon == UiIcon::Close { 8.0 } else { 14.0 }),
    )
}

fn tab_icon_button(ui: &mut egui::Ui, enabled: bool, label: &str) -> egui::Response {
    let response = ui
        .add_enabled_ui(enabled, |ui| {
            ui.add_sized(
                [18.0, METRICS.toolbar.title_height],
                egui::Button::new("").frame(false),
            )
        })
        .inner;
    response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, enabled, label));
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
