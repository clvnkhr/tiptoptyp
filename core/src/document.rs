use std::{path::PathBuf, sync::Arc};

/// Allocated by the window shell before creating a document or its services.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct WindowSessionId(u64);
impl WindowSessionId {
    pub const fn new(value: u64) -> Self {
        Self(value)
    }
}

/// Stable identity for work that must not cross a document replacement or
/// overwrite a newer edit in the same document.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DocumentKey {
    pub owner: WindowSessionId,
    pub epoch: u64,
    pub revision: u64,
}

impl DocumentKey {
    pub const fn new(owner: WindowSessionId, epoch: u64, revision: u64) -> Self {
        Self {
            owner,
            epoch,
            revision,
        }
    }
    pub const fn after_edit(self) -> Self {
        Self {
            revision: self.revision.wrapping_add(1),
            ..self
        }
    }
}

/// Source and its cache identity cannot be assembled independently by consumers.
#[derive(Debug, Clone)]
pub struct DocumentSnapshot {
    key: DocumentKey,
    source: Arc<str>,
}

#[derive(Debug)]
pub struct SaveRequest {
    snapshot: DocumentSnapshot,
    path: PathBuf,
    kind: DocumentKind,
}
impl SaveRequest {
    pub fn source(&self) -> &str {
        self.snapshot.source()
    }
    pub fn path(&self) -> &std::path::Path {
        &self.path
    }
    /// Called by the effect adapter only after the destination is committed.
    pub fn committed(self, disk_fingerprint: u64) -> SaveReceipt {
        SaveReceipt {
            request: self,
            disk_fingerprint,
        }
    }
}
#[derive(Debug)]
pub struct SaveReceipt {
    request: SaveRequest,
    disk_fingerprint: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SaveStatus {
    /// The receipt became the document's latest persisted snapshot.
    Applied,
    /// A newer receipt for this document epoch was already recorded.
    Stale,
}

impl DocumentSnapshot {
    pub fn key(&self) -> DocumentKey {
        self.key
    }
    pub fn source(&self) -> &str {
        &self.source
    }
    #[cfg(any(test, feature = "test-support"))]
    pub fn fixture(key: DocumentKey, source: &str) -> Self {
        Self {
            key,
            source: Arc::from(source),
        }
    }
}

#[derive(Debug, Clone)]
pub struct EditorSnapshot<C> {
    pub source: Arc<str>,
    pub cursor: C,
}

/// Owns the editor buffer, on-disk identity, revision clock, and undo history.
///
/// Document replacement and save transitions live here so their epoch,
/// revision, dirty-state, and history updates cannot drift apart in
/// `EditorApp`.
///
/// ```compile_fail
/// use tiptoptyp_core::document::{DocumentSession, DocumentKind, WindowSessionId};
/// let mut document = DocumentSession::<usize>::new(WindowSessionId::new(1), "text", DocumentKind::Typst);
/// document.source.push_str("unversioned edit");
/// ```
pub struct DocumentSession<C> {
    owner: WindowSessionId,
    source: String,
    source_snapshot: Arc<str>,
    saved_source: String,
    path: Option<PathBuf>,
    epoch: u64,
    revision: u64,
    saved_revision: u64,
    disk_fingerprint: Option<u64>,
    kind: DocumentKind,
    pending_edit: bool,
    pub reset_editor_history: bool,
    undo: Vec<EditorSnapshot<C>>,
    redo: Vec<EditorSnapshot<C>>,
}

impl<C> DocumentSession<C> {
    pub fn new(owner: WindowSessionId, source: impl Into<String>, kind: DocumentKind) -> Self {
        let source = source.into();
        Self {
            owner,
            source_snapshot: Arc::from(source.as_str()),
            saved_source: source.clone(),
            source,
            path: None,
            epoch: 0,
            revision: 0,
            saved_revision: 0,
            disk_fingerprint: None,
            kind,
            pending_edit: false,
            reset_editor_history: true,
            undo: Vec::new(),
            redo: Vec::new(),
        }
    }

    pub const fn key(&self) -> DocumentKey {
        DocumentKey::new(self.owner, self.epoch, self.revision)
    }

    pub fn snapshot(&self) -> DocumentSnapshot {
        DocumentSnapshot {
            key: self.key(),
            source: self.source_snapshot.clone(),
        }
    }
    /// Reattach a parked tab without replacing its buffer or undo history.
    pub fn reactivate_after(&mut self, previous: DocumentKey) {
        assert_eq!(self.owner, previous.owner);
        self.epoch = self.epoch.max(previous.epoch).wrapping_add(1);
        self.revision = self.revision.max(previous.revision).wrapping_add(1);
        self.saved_revision = self.revision;
        self.pending_edit = false;
    }

    /// Coalesces edits until the runtime consumes their effects.
    pub fn take_edit(&mut self) -> Option<DocumentSnapshot> {
        std::mem::take(&mut self.pending_edit).then(|| self.snapshot())
    }

    pub fn is_dirty(&self) -> bool {
        self.kind.is_editable() && self.source != self.saved_source
    }

    pub fn name(&self) -> String {
        self.path
            .as_ref()
            .and_then(|path| path.file_name())
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| {
                match self.kind {
                    DocumentKind::Typst => "Untitled.typ",
                    DocumentKind::Tex => "Untitled.tex",
                    DocumentKind::Text => "Untitled.txt",
                    DocumentKind::Pdf => "Untitled.pdf",
                    DocumentKind::Image => "Untitled image",
                }
                .to_owned()
            })
    }

    pub fn source(&self) -> &String {
        &self.source
    }
    pub fn saved_source(&self) -> &String {
        &self.saved_source
    }
    pub fn path(&self) -> &Option<PathBuf> {
        &self.path
    }
    pub fn epoch(&self) -> u64 {
        self.epoch
    }
    pub fn revision(&self) -> u64 {
        self.revision
    }
    pub fn disk_fingerprint(&self) -> Option<u64> {
        self.disk_fingerprint
    }
    pub fn kind(&self) -> DocumentKind {
        self.kind
    }

    /// The only ordinary buffer mutation boundary. The closure cannot retain
    /// a mutable reference to the source. No-op interactions allocate no snapshot
    /// and do not invalidate caches or undo history.
    pub fn edit<R>(&mut self, cursor: C, edit: impl FnOnce(&mut String) -> R) -> R {
        let transaction = EditTransaction {
            document: self,
            cursor: Some(cursor),
        };
        edit(&mut transaction.document.source)
    }

    pub fn rename(&mut self, path: PathBuf, kind: DocumentKind) {
        if self.path.as_ref() != Some(&path) {
            self.path = Some(path);
            self.kind = kind;
            self.epoch = self.epoch.wrapping_add(1);
            self.revision = self.revision.wrapping_add(1);
        }
    }

    pub fn restore_saved_source(&mut self) {
        if self.source != self.saved_source {
            self.revision = self.revision.wrapping_add(1);
            self.pending_edit = true;
        }
        self.source.clone_from(&self.saved_source);
        self.source_snapshot = Arc::from(self.source.as_str());
    }

    pub fn replace_untitled(&mut self, source: impl Into<String>) {
        self.replace(source.into(), None, DocumentKind::Typst, None);
    }

    pub fn replace_untitled_kind(&mut self, kind: DocumentKind) {
        self.replace(String::new(), None, kind, None);
    }

    pub fn replace_loaded(
        &mut self,
        source: String,
        path: PathBuf,
        kind: DocumentKind,
        disk_fingerprint: Option<u64>,
    ) {
        self.replace(source, Some(path), kind, disk_fingerprint);
    }

    /// Switch an editor's source representation without pretending to load or
    /// save a file. Both current and persisted buffers must be supplied in the
    /// new representation. Path, kind and the actual disk fingerprint survive.
    /// Old cursor/history coordinates and in-flight receipts do not.
    pub fn replace_representation(&mut self, source: String, saved_source: String) {
        self.source_snapshot = Arc::from(source.as_str());
        self.source = source;
        self.saved_source = saved_source;
        self.epoch = self.epoch.wrapping_add(1);
        self.revision = self.revision.wrapping_add(1);
        self.saved_revision = self.revision;
        self.pending_edit = true;
        self.clear_history();
    }

    fn replace(
        &mut self,
        source: String,
        path: Option<PathBuf>,
        kind: DocumentKind,
        disk_fingerprint: Option<u64>,
    ) {
        self.saved_source = source.clone();
        self.source_snapshot = Arc::from(source.as_str());
        self.source = source;
        self.path = path;
        self.epoch = self.epoch.wrapping_add(1);
        self.revision = self.revision.wrapping_add(1);
        self.saved_revision = self.revision;
        self.disk_fingerprint = disk_fingerprint;
        self.kind = kind;
        self.pending_edit = false;
        self.clear_history();
    }

    pub fn prepare_save(&self, path: PathBuf, kind: DocumentKind) -> SaveRequest {
        SaveRequest {
            snapshot: self.snapshot(),
            path,
            kind,
        }
    }

    pub fn record_save(&mut self, receipt: SaveReceipt) -> Result<SaveStatus, &'static str> {
        let SaveRequest {
            snapshot,
            path,
            kind,
        } = receipt.request;
        if snapshot.key.owner != self.owner || snapshot.key.epoch != self.epoch {
            return Err("The file was saved, but its document has since been replaced");
        }
        if snapshot.key.revision < self.saved_revision {
            return Ok(SaveStatus::Stale);
        }
        let path_changed = self.path.as_ref() != Some(&path);
        self.path = Some(path);
        if path_changed {
            self.epoch = self.epoch.wrapping_add(1);
            self.revision = self.revision.wrapping_add(1);
            self.kind = kind;
        }
        self.saved_source = snapshot.source.to_string();
        self.saved_revision = snapshot.key.revision;
        self.disk_fingerprint = Some(receipt.disk_fingerprint);
        Ok(SaveStatus::Applied)
    }

    pub fn clear_history(&mut self) {
        self.reset_editor_history = true;
        self.undo.clear();
        self.redo.clear();
    }

    pub fn history_availability(&self) -> (bool, bool) {
        (
            self.kind.is_editable() && !self.undo.is_empty(),
            self.kind.is_editable() && !self.redo.is_empty(),
        )
    }

    pub fn history_step(&mut self, redo: bool, cursor: C) -> Option<EditorSnapshot<C>> {
        let current = EditorSnapshot {
            source: self.source_snapshot.clone(),
            cursor,
        };
        let next = if redo {
            let next = self.redo.pop();
            if next.is_some() {
                self.undo.push(current);
            }
            next
        } else {
            let next = self.undo.pop();
            if next.is_some() {
                self.redo.push(current);
            }
            next
        };
        if let Some(next) = &next
            && self.source != next.source.as_ref()
        {
            self.source = next.source.to_string();
            self.source_snapshot = next.source.clone();
            self.revision = self.revision.wrapping_add(1);
            self.pending_edit = true;
        }
        next
    }

    fn push_undo_snapshot(&mut self, snapshot: EditorSnapshot<C>) {
        if self
            .undo
            .last()
            .is_none_or(|latest| latest.source != snapshot.source)
        {
            self.undo.push(snapshot);
            const MAX_EDITOR_HISTORY: usize = 100;
            if self.undo.len() > MAX_EDITOR_HISTORY {
                self.undo.remove(0);
            }
        }
        self.redo.clear();
    }
}

/// Finalize even if an adapter unwinds after modifying the buffer.
struct EditTransaction<'a, C> {
    document: &'a mut DocumentSession<C>,
    cursor: Option<C>,
}
impl<C> Drop for EditTransaction<'_, C> {
    fn drop(&mut self) {
        let document = &mut self.document;
        if document.source != document.source_snapshot.as_ref() {
            document.push_undo_snapshot(EditorSnapshot {
                source: document.source_snapshot.clone(),
                cursor: self.cursor.take().expect("transaction owns its cursor"),
            });
            document.revision = document.revision.wrapping_add(1);
            document.pending_edit = true;
            document.source_snapshot = Arc::from(document.source.as_str());
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DocumentKind {
    Typst,
    Tex,
    Text,
    Image,
    Pdf,
}
impl DocumentKind {
    pub fn is_editable(self) -> bool {
        matches!(self, Self::Typst | Self::Tex | Self::Text)
    }
    pub fn is_typst(self) -> bool {
        self == Self::Typst
    }
    pub fn preview_only(self) -> bool {
        !self.is_editable()
    }
    /// Source language is independent of installed tools and editing projections.
    pub fn typesetting_language(self) -> Option<TypesettingLanguage> {
        match self {
            Self::Typst => Some(TypesettingLanguage::Typst),
            Self::Tex => Some(TypesettingLanguage::Tex),
            Self::Text | Self::Image | Self::Pdf => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TypesettingLanguage {
    Typst,
    Tex,
}

impl TypesettingLanguage {
    pub fn extension(self) -> &'static str {
        match self {
            Self::Typst => "typ",
            Self::Tex => "tex",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn representation_change_retains_disk_identity_but_invalidates_old_coordinates_and_receipts() {
        let mut document =
            DocumentSession::<usize>::new(WindowSessionId::new(1), "", DocumentKind::Typst);
        document.replace_loaded(
            "canonical saved".into(),
            "draft.typ".into(),
            DocumentKind::Typst,
            Some(41),
        );
        document.edit(0, |source| source.push_str(" edit"));
        let old = document.key();
        let receipt = document
            .prepare_save("draft.typ".into(), DocumentKind::Typst)
            .committed(42);
        document.replace_representation("view edited".into(), "view saved".into());
        assert_ne!(document.key().epoch, old.epoch);
        assert!(document.record_save(receipt).is_err());
        assert_eq!(
            document.path().as_deref(),
            Some(std::path::Path::new("draft.typ"))
        );
        assert_eq!(document.kind(), DocumentKind::Typst);
        assert_eq!(document.disk_fingerprint(), Some(41));
        assert_eq!(document.snapshot().source(), "view edited");
        assert_eq!(document.saved_source(), "view saved");
        assert!(document.is_dirty());
        assert_eq!(document.history_availability(), (false, false));
        assert!(document.reset_editor_history);
        assert_eq!(document.take_edit().unwrap().source(), "view edited");
        assert!(document.take_edit().is_none());
    }

    #[test]
    fn no_op_edits_reuse_snapshot_and_preserve_redo() {
        let mut document =
            DocumentSession::new(WindowSessionId::new(1), "文稿", DocumentKind::Typst);
        let initial = document.snapshot();
        document.edit(0usize, |_| {});
        assert_eq!(initial.key(), document.key());
        assert!(Arc::ptr_eq(&initial.source, &document.source_snapshot));
        document.edit(0, |source| source.push('🦀'));
        let revision = document.revision();
        document.history_step(false, 2).unwrap();
        document.edit(0, |_| {});
        assert_eq!(document.history_availability(), (false, true));
        document.history_step(true, 0).unwrap();
        assert_eq!(document.source(), "文稿🦀");
        assert_eq!(document.revision(), revision + 2);
    }

    #[test]
    fn edit_unwind_still_versions_the_buffer_and_records_undo() {
        let mut document =
            DocumentSession::new(WindowSessionId::new(1), "before", DocumentKind::Text);
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            document.edit(0usize, |source| {
                source.push_str(" after");
                panic!("injected adapter failure");
            });
        }));
        assert!(result.is_err());
        assert_eq!(document.revision(), 1);
        assert_eq!(document.snapshot().source(), "before after");
        document.history_step(false, 0).unwrap();
        assert_eq!(document.source(), "before");
    }

    #[test]
    fn save_receipt_records_written_bytes_without_clearing_new_edits() {
        let mut document =
            DocumentSession::new(WindowSessionId::new(1), "saved", DocumentKind::Text);
        let save = document.prepare_save("notes.txt".into(), DocumentKind::Text);
        document.edit(0usize, |source| source.push_str(" later"));
        document.record_save(save.committed(12)).unwrap();
        assert_eq!(document.saved_source(), "saved");
        assert_eq!(document.source(), "saved later");
        assert!(document.is_dirty());
        let stale = document.prepare_save("notes.txt".into(), DocumentKind::Text);
        document.replace_untitled("replacement");
        assert!(document.record_save(stale.committed(13)).is_err());
        assert!(document.path().is_none());
        assert_eq!(document.saved_source(), "replacement");
    }

    #[test]
    fn unicode_edit_sequences_match_history_and_snapshot_identity() {
        // Exhaust all four-step edit/no-op/undo/redo sequences without a GUI,
        // filesystem, random seed, or clock.
        for sequence in 0..256usize {
            let mut document =
                DocumentSession::new(WindowSessionId::new(1), String::new(), DocumentKind::Text);
            let mut expected = String::new();
            let mut undo = Vec::new();
            let mut redo = Vec::new();
            for step in 0..4 {
                let before = document.snapshot();
                match (sequence >> (step * 2)) & 3 {
                    0 => {
                        undo.push(expected.clone());
                        redo.clear();
                        expected.push_str("文🦀e\u{301}\r\n");
                        document.edit(0usize, |source| source.push_str("文🦀e\u{301}\r\n"));
                    }
                    1 => {
                        document.edit(0, |_| {});
                    }
                    2 => {
                        if let Some(next) = undo.pop() {
                            redo.push(expected);
                            expected = next;
                        }
                        document.history_step(false, 0);
                    }
                    _ => {
                        if let Some(next) = redo.pop() {
                            undo.push(expected);
                            expected = next;
                        }
                        document.history_step(true, 0);
                    }
                }
                assert_eq!(document.source(), &expected);
                assert_eq!(document.snapshot().source(), expected);
                assert_eq!(
                    document.revision(),
                    before.key().revision + u64::from(before.source() != expected)
                );
            }
        }
    }
    #[test]
    fn replacement_advances_identity_and_resets_history_as_one_transition() {
        let mut document =
            DocumentSession::<usize>::new(WindowSessionId::new(1), "old", DocumentKind::Typst);
        document.push_undo_snapshot(EditorSnapshot {
            source: Arc::from("before"),
            cursor: 0usize,
        });
        let previous = document.key();

        document.replace_loaded(
            "new".to_owned(),
            PathBuf::from("chapter.typ"),
            DocumentKind::Typst,
            Some(7),
        );

        assert_eq!(document.epoch, previous.epoch.wrapping_add(1));
        assert_eq!(document.revision, previous.revision.wrapping_add(1));
        assert_eq!(document.source, "new");
        assert_eq!(document.saved_source, "new");
        assert!(!document.is_dirty());
        assert_eq!(document.history_availability(), (false, false));
        assert!(document.reset_editor_history);
    }

    #[test]
    fn save_as_changes_identity_but_an_in_place_save_does_not() {
        let mut document =
            DocumentSession::<usize>::new(WindowSessionId::new(1), "draft", DocumentKind::Typst);
        document.edit(0usize, |source| source.push('!'));
        let edited = document.key();

        let save = document.prepare_save(PathBuf::from("draft.typ"), DocumentKind::Typst);
        document.record_save(save.committed(11)).unwrap();
        assert_eq!(document.epoch, edited.epoch.wrapping_add(1));
        assert_eq!(document.revision, edited.revision.wrapping_add(1));
        assert!(!document.is_dirty());

        let saved = document.key();
        document.edit(0usize, |source| source.push('?'));
        let save = document.prepare_save(PathBuf::from("draft.typ"), DocumentKind::Typst);
        document.record_save(save.committed(12)).unwrap();
        assert_eq!(document.epoch, saved.epoch);
        assert_eq!(document.revision, saved.revision.wrapping_add(1));
        assert!(!document.is_dirty());
    }

    #[test]
    fn older_in_place_save_completion_cannot_regress_persisted_metadata() {
        let mut document =
            DocumentSession::<usize>::new(WindowSessionId::new(1), "saved", DocumentKind::Text);
        document.replace_loaded(
            "saved".to_owned(),
            PathBuf::from("draft.txt"),
            DocumentKind::Text,
            Some(1),
        );
        let older = document.prepare_save(PathBuf::from("draft.txt"), DocumentKind::Text);
        document.edit(0usize, |source| source.push_str(" newer"));
        let newer = document.prepare_save(PathBuf::from("draft.txt"), DocumentKind::Text);

        assert_eq!(
            document.record_save(newer.committed(2)).unwrap(),
            SaveStatus::Applied
        );
        assert_eq!(
            document.record_save(older.committed(1)).unwrap(),
            SaveStatus::Stale
        );

        assert_eq!(document.saved_source(), "saved newer");
        assert_eq!(document.disk_fingerprint(), Some(2));
        assert!(!document.is_dirty());
    }

    #[test]
    fn restoring_saved_source_emits_one_change_notification() {
        let mut document =
            DocumentSession::<usize>::new(WindowSessionId::new(1), "saved", DocumentKind::Text);
        document.edit(0usize, |source| source.push_str(" edit"));
        assert!(document.take_edit().is_some());
        let revision = document.revision();

        document.restore_saved_source();

        assert_eq!(document.source(), "saved");
        assert_eq!(document.revision(), revision + 1);
        assert_eq!(document.take_edit().unwrap().source(), "saved");
        assert!(document.take_edit().is_none());
    }
}
