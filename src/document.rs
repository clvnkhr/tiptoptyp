use std::{
    path::{Path, PathBuf},
    sync::Arc,
};

use eframe::egui::text::CCursorRange;

/// Stable identity for work that must not cross a document replacement or
/// overwrite a newer edit in the same document.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct DocumentKey {
    pub(crate) epoch: u64,
    pub(crate) revision: u64,
}

#[derive(Debug, Clone)]
pub(crate) struct EditorSnapshot {
    pub(crate) source: Arc<str>,
    pub(crate) cursor: CCursorRange,
}

/// Owns the editor buffer, on-disk identity, revision clock, and undo history.
///
/// Document replacement and save transitions live here so their epoch,
/// revision, dirty-state, and history updates cannot drift apart in
/// `EditorApp`.
pub(crate) struct DocumentSession {
    pub(crate) source: String,
    pub(crate) saved_source: String,
    pub(crate) path: Option<PathBuf>,
    pub(crate) epoch: u64,
    pub(crate) revision: u64,
    pub(crate) disk_fingerprint: Option<u64>,
    pub(crate) kind: DocumentKind,
    pub(crate) reset_editor_history: bool,
    undo: Vec<EditorSnapshot>,
    redo: Vec<EditorSnapshot>,
}

impl DocumentSession {
    pub(crate) fn new(source: impl Into<String>, kind: DocumentKind) -> Self {
        let source = source.into();
        Self {
            saved_source: source.clone(),
            source,
            path: None,
            epoch: 0,
            revision: 0,
            disk_fingerprint: None,
            kind,
            reset_editor_history: true,
            undo: Vec::new(),
            redo: Vec::new(),
        }
    }

    pub(crate) const fn key(&self) -> DocumentKey {
        DocumentKey {
            epoch: self.epoch,
            revision: self.revision,
        }
    }

    pub(crate) fn is_dirty(&self) -> bool {
        self.kind.is_editable() && self.source != self.saved_source
    }

    pub(crate) fn name(&self) -> String {
        self.path
            .as_ref()
            .and_then(|path| path.file_name())
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| "Untitled.typ".to_owned())
    }

    pub(crate) fn mark_edited(&mut self) {
        self.revision = self.revision.wrapping_add(1);
    }

    pub(crate) fn restore_saved_source(&mut self) {
        if self.source != self.saved_source {
            self.revision = self.revision.wrapping_add(1);
        }
        self.source.clone_from(&self.saved_source);
    }

    pub(crate) fn replace_untitled(&mut self, source: impl Into<String>) {
        self.replace(source.into(), None, DocumentKind::Typst, None);
    }

    pub(crate) fn replace_loaded(
        &mut self,
        source: String,
        path: PathBuf,
        kind: DocumentKind,
        disk_fingerprint: Option<u64>,
    ) {
        self.replace(source, Some(path), kind, disk_fingerprint);
    }

    fn replace(
        &mut self,
        source: String,
        path: Option<PathBuf>,
        kind: DocumentKind,
        disk_fingerprint: Option<u64>,
    ) {
        self.saved_source = source.clone();
        self.source = source;
        self.path = path;
        self.epoch = self.epoch.wrapping_add(1);
        self.revision = self.revision.wrapping_add(1);
        self.disk_fingerprint = disk_fingerprint;
        self.kind = kind;
        self.clear_history();
    }

    pub(crate) fn complete_save(
        &mut self,
        path: PathBuf,
        kind: DocumentKind,
        disk_fingerprint: u64,
        path_changed: bool,
    ) {
        self.path = Some(path);
        self.disk_fingerprint = Some(disk_fingerprint);
        if path_changed {
            self.epoch = self.epoch.wrapping_add(1);
            self.revision = self.revision.wrapping_add(1);
            self.kind = kind;
        }
        self.saved_source.clone_from(&self.source);
    }

    pub(crate) fn clear_history(&mut self) {
        self.reset_editor_history = true;
        self.undo.clear();
        self.redo.clear();
    }

    pub(crate) fn history_availability(&self) -> (bool, bool) {
        (
            self.kind.is_editable() && !self.undo.is_empty(),
            self.kind.is_editable() && !self.redo.is_empty(),
        )
    }

    pub(crate) fn history_step(
        &mut self,
        redo: bool,
        current: EditorSnapshot,
    ) -> Option<EditorSnapshot> {
        if redo {
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
        }
    }

    pub(crate) fn push_undo_snapshot(&mut self, snapshot: EditorSnapshot) {
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

/// How the currently selected file should be presented.
///
/// `ViewMode` remains the user's layout preference. Binary documents use
/// `preview_only` as a temporary presentation override, so opening a PDF or
/// image never silently changes that preference.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DocumentKind {
    Typst,
    Text,
    Image,
    Pdf,
}

impl DocumentKind {
    pub fn detect(path: &Path, bytes: &[u8]) -> Result<Self, String> {
        // Content signatures take precedence over a misleading extension. In
        // particular, renaming an already-open PDF/image must never turn its
        // empty editor buffer into an editable text document.
        if bytes.starts_with(b"%PDF-") {
            return Ok(Self::Pdf);
        }
        if image::guess_format(bytes).is_ok() {
            return Ok(Self::Image);
        }

        match extension_kind(path) {
            Some(Self::Typst) => return utf8_document(bytes, Self::Typst, path),
            Some(Self::Text) => return utf8_document(bytes, Self::Text, path),
            Some(kind) => return Ok(kind),
            None if looks_like_text(bytes) => return Ok(Self::Text),
            None => {}
        }

        Err(format!(
            "{} is not a supported text, image, or PDF file",
            path.display()
        ))
    }

    pub fn is_editable(self) -> bool {
        matches!(self, Self::Typst | Self::Text)
    }

    pub fn is_typst(self) -> bool {
        self == Self::Typst
    }

    pub fn preview_only(self) -> bool {
        matches!(self, Self::Image | Self::Pdf)
    }

    pub fn supports_path(path: &Path) -> bool {
        extension_kind(path).is_some()
    }
}

fn utf8_document(bytes: &[u8], kind: DocumentKind, path: &Path) -> Result<DocumentKind, String> {
    std::str::from_utf8(bytes)
        .map(|_| kind)
        .map_err(|error| format!("Could not open {} as UTF-8 text: {error}", path.display()))
}

fn looks_like_text(bytes: &[u8]) -> bool {
    !bytes.contains(&0) && std::str::from_utf8(bytes).is_ok()
}

fn extension_kind(path: &Path) -> Option<DocumentKind> {
    let extension = path.extension()?.to_str()?;
    if extension.eq_ignore_ascii_case("typ") {
        Some(DocumentKind::Typst)
    } else if extension.eq_ignore_ascii_case("pdf") {
        Some(DocumentKind::Pdf)
    } else if IMAGE_EXTENSIONS
        .iter()
        .any(|known| extension.eq_ignore_ascii_case(known))
    {
        Some(DocumentKind::Image)
    } else if TEXT_EXTENSIONS
        .iter()
        .any(|known| extension.eq_ignore_ascii_case(known))
    {
        Some(DocumentKind::Text)
    } else {
        None
    }
}

const IMAGE_EXTENSIONS: &[&str] = &[
    "png", "jpg", "jpeg", "gif", "webp", "bmp", "ico", "tif", "tiff",
];

const TEXT_EXTENSIONS: &[&str] = &[
    "txt", "md", "markdown", "rs", "toml", "json", "jsonc", "yaml", "yml", "xml", "html", "htm",
    "css", "scss", "js", "jsx", "ts", "tsx", "py", "rb", "go", "java", "c", "h", "cc", "cpp",
    "hpp", "sh", "bash", "zsh", "fish", "sql", "csv", "tsv", "ini", "cfg", "conf", "log", "tex",
    "bib",
];

#[cfg(test)]
mod tests {
    use eframe::egui::text::CCursor;

    use super::*;

    #[test]
    fn recognizes_supported_document_kinds_case_insensitively() {
        assert_eq!(
            DocumentKind::detect(Path::new("main.TYP"), b"= Hello").unwrap(),
            DocumentKind::Typst
        );
        assert_eq!(
            DocumentKind::detect(Path::new("notes.MD"), b"# Hello").unwrap(),
            DocumentKind::Text
        );
        assert_eq!(
            DocumentKind::detect(Path::new("paper.PDF"), b"%PDF-1.7").unwrap(),
            DocumentKind::Pdf
        );
    }

    #[test]
    fn accepts_extensionless_utf8_but_rejects_binary_data() {
        assert_eq!(
            DocumentKind::detect(Path::new("README"), b"plain text").unwrap(),
            DocumentKind::Text
        );
        assert!(DocumentKind::detect(Path::new("blob"), b"\0\x01\x02").is_err());
    }

    #[test]
    fn pdf_magic_wins_without_an_extension() {
        assert_eq!(
            DocumentKind::detect(Path::new("download"), b"%PDF-2.0\n").unwrap(),
            DocumentKind::Pdf
        );
    }

    #[test]
    fn binary_magic_wins_after_renaming_to_a_text_extension() {
        assert_eq!(
            DocumentKind::detect(Path::new("renamed.typ"), b"%PDF-2.0\n").unwrap(),
            DocumentKind::Pdf
        );
        let png = b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR";
        assert_eq!(
            DocumentKind::detect(Path::new("renamed.txt"), png).unwrap(),
            DocumentKind::Image
        );
    }

    #[test]
    fn advertised_paths_and_detection_share_one_case_insensitive_extension_policy() {
        for (path, expected) in [
            ("main.TyP", DocumentKind::Typst),
            ("notes.JsOnC", DocumentKind::Text),
            ("photo.WeBp", DocumentKind::Image),
            ("paper.PdF", DocumentKind::Pdf),
        ] {
            let path = Path::new(path);
            assert!(DocumentKind::supports_path(path));
            assert_eq!(
                DocumentKind::detect(path, b"plain utf-8 text").unwrap(),
                expected
            );
        }
        assert!(!DocumentKind::supports_path(Path::new("archive.zip")));
    }

    #[test]
    fn declared_text_files_still_require_valid_utf8() {
        let error = DocumentKind::detect(Path::new("broken.toml"), b"\xff\xfe").unwrap_err();
        assert!(error.contains("UTF-8"));
        assert!(error.contains("broken.toml"));
    }

    #[test]
    fn replacement_advances_identity_and_resets_history_as_one_transition() {
        let mut document = DocumentSession::new("old", DocumentKind::Typst);
        document.push_undo_snapshot(EditorSnapshot {
            source: Arc::from("before"),
            cursor: CCursorRange::one(CCursor::new(0)),
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
        let mut document = DocumentSession::new("draft", DocumentKind::Typst);
        document.source.push('!');
        document.mark_edited();
        let edited = document.key();

        document.complete_save(PathBuf::from("draft.typ"), DocumentKind::Typst, 11, true);
        assert_eq!(document.epoch, edited.epoch.wrapping_add(1));
        assert_eq!(document.revision, edited.revision.wrapping_add(1));
        assert!(!document.is_dirty());

        let saved = document.key();
        document.source.push('?');
        document.mark_edited();
        document.complete_save(PathBuf::from("draft.typ"), DocumentKind::Typst, 12, false);
        assert_eq!(document.epoch, saved.epoch);
        assert_eq!(document.revision, saved.revision.wrapping_add(1));
        assert!(!document.is_dirty());
    }
}
