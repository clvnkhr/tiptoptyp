//! Document ownership for MiTeX dollar notation.
//!
//! The editor and its undo stack own the displayed source. Only a checked
//! `CanonicalSnapshot` or `SaveRequest` may leave that domain. Invalid edits
//! remain editable/undoable but cannot be saved or sent to a compiler. This
//! adapter leaves ordinary documents on the unprojected editor path.
use std::{
    cell::RefCell,
    path::PathBuf,
    sync::{Arc, OnceLock},
};

use tiptoptyp_core::document::{
    self, DocumentKey, DocumentKind, DocumentSnapshot, EditorSnapshot, SaveStatus, WindowSessionId,
};

use crate::mitex_projection::{Config, Projection, Translation};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    Translation(crate::mitex_projection::Error),
    NotTypst,
    AlreadyEnabled,
    StaleSnapshot,
    ServiceEdit(String),
    UnrepresentableEdit,
}

impl From<crate::mitex_projection::Error> for Error {
    fn from(error: crate::mitex_projection::Error) -> Self {
        Self::Translation(error)
    }
}
impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Translation(error) => error.fmt(f),
            Self::NotTypst => {
                f.write_str("MiTeX dollar notation requires a Typst document and save destination")
            }
            Self::AlreadyEnabled => {
                f.write_str("Disable MiTeX dollar notation before changing its package")
            }
            Self::StaleSnapshot => {
                f.write_str("The service response belongs to an older document snapshot")
            }
            Self::ServiceEdit(message) => f.write_str(message),
            Self::UnrepresentableEdit => f.write_str(
                "This edit changes hidden MiTeX syntax; disable dollar notation to apply it",
            ),
        }
    }
}
impl std::error::Error for Error {}

/// Immutable canonical bytes and their matching editor revision/source map.
/// Deliberately not a `DocumentSnapshot`: editor caches must not confuse the
/// two source domains, even when their revision keys are equal.
#[derive(Debug, Clone)]
pub struct CanonicalSnapshot {
    editor: DocumentSnapshot,
    translation: Option<Arc<Translation>>,
    coordinates: Arc<OnceLock<coordinates::Coordinates>>,
}

impl CanonicalSnapshot {
    pub fn key(&self) -> DocumentKey {
        self.editor.key()
    }
    pub fn source(&self) -> &str {
        self.translation
            .as_ref()
            .map_or_else(|| self.editor.source(), |value| value.output())
    }
    pub fn editor_source(&self) -> &str {
        self.editor.source()
    }
    pub fn editor_to_canonical(&self, byte: usize) -> Option<usize> {
        self.translation.as_ref().map_or_else(
            || self.editor.source().is_char_boundary(byte).then_some(byte),
            |value| value.input_to_output(byte),
        )
    }
    pub fn canonical_to_editor(&self, byte: usize) -> Option<usize> {
        self.translation.as_ref().map_or_else(
            || self.editor.source().is_char_boundary(byte).then_some(byte),
            |value| value.output_to_input(byte),
        )
    }
}

/// The bytes to write and the displayed snapshot to acknowledge are one
/// transaction. Call `committed` only after the canonical bytes are committed.
#[derive(Debug)]
pub struct SaveRequest {
    editor: document::SaveRequest,
    canonical: Option<CanonicalSnapshot>,
}
impl SaveRequest {
    pub fn path(&self) -> &std::path::Path {
        self.editor.path()
    }
    pub fn source(&self) -> &str {
        self.canonical
            .as_ref()
            .map_or_else(|| self.editor.source(), CanonicalSnapshot::source)
    }
    pub fn committed(self, disk_fingerprint: u64) -> SaveReceipt {
        SaveReceipt {
            editor: self.editor.committed(disk_fingerprint),
            canonical: self.canonical,
        }
    }
}

#[derive(Debug)]
pub struct SaveReceipt {
    editor: document::SaveReceipt,
    canonical: Option<CanonicalSnapshot>,
}

struct Active {
    config: Config,
    projection: Projection,
    saved_canonical: Arc<str>,
}
struct Cached {
    key: DocumentKey,
    result: Result<CanonicalSnapshot, Error>,
    dirty: bool,
}

/// Read-only access to the ordinary editor model is intentional: all document
/// replacements, representation changes and save receipts pass this boundary.
pub struct Document<C> {
    editor: document::DocumentSession<C>,
    active: Option<Active>,
    canonical: RefCell<Option<Cached>>,
    #[cfg(test)]
    encodes: std::cell::Cell<usize>,
}

// Read-only delegation keeps existing editor consumers in the displayed-source
// domain. There is deliberately no DerefMut: mutations must use this adapter.
impl<C> std::ops::Deref for Document<C> {
    type Target = document::DocumentSession<C>;
    fn deref(&self) -> &Self::Target {
        &self.editor
    }
}

impl<C> Document<C> {
    pub fn new(owner: WindowSessionId, source: impl Into<String>, kind: DocumentKind) -> Self {
        Self {
            editor: document::DocumentSession::new(owner, source, kind),
            active: None,
            canonical: RefCell::new(None),
            #[cfg(test)]
            encodes: std::cell::Cell::new(0),
        }
    }

    pub fn editor(&self) -> &document::DocumentSession<C> {
        &self.editor
    }
    pub fn config(&self) -> Option<&Config> {
        self.active.as_ref().map(|active| &active.config)
    }
    pub fn reactivate_after(&mut self, previous: DocumentKey) {
        self.editor.reactivate_after(previous);
        self.canonical.get_mut().take();
    }
    pub fn edit<R>(&mut self, cursor: C, edit: impl FnOnce(&mut String) -> R) -> R {
        self.editor.edit(cursor, edit)
    }
    pub fn history_step(&mut self, redo: bool, cursor: C) -> Option<EditorSnapshot<C>> {
        self.editor.history_step(redo, cursor)
    }
    pub fn take_edit(&mut self) -> Option<DocumentSnapshot> {
        self.editor.take_edit()
    }
    pub fn take_history_reset(&mut self) -> bool {
        std::mem::take(&mut self.editor.reset_editor_history)
    }
    pub fn set_history_reset(&mut self, reset: bool) {
        self.editor.reset_editor_history = reset;
    }
    pub fn clear_history(&mut self) {
        self.editor.clear_history();
    }
    pub fn can_rename(&self, kind: DocumentKind) -> Result<(), Error> {
        if self.active.is_some() && !kind.is_typst() {
            Err(Error::NotTypst)
        } else {
            Ok(())
        }
    }
    pub fn rename(&mut self, path: PathBuf, kind: DocumentKind) -> Result<(), Error> {
        self.can_rename(kind)?;
        self.editor.rename(path, kind);
        Ok(())
    }
    /// A confirmed New/discard or QA fixture reset starts an ordinary document.
    /// Unlike disabling a mode this replaces, rather than translates, the buffer.
    pub fn replace_unprojected_untitled(&mut self, source: impl Into<String>) {
        self.editor.replace_untitled(source);
        self.active = None;
        self.canonical.get_mut().take();
    }
    pub fn restore_saved_source(&mut self) -> Result<(), Error> {
        if let Some(active) = &mut self.active {
            // Reverting must restore literal spelling too, even if both
            // canonical buffers happen to project to identical dollar text.
            let projection = match Projection::open(&active.saved_canonical, active.config.clone())
            {
                Ok(projection) => projection,
                Err(_) => {
                    // The user may have removed native math before enabling.
                    // Revert restores that disk representation and leaves TeX mode.
                    let source = active.saved_canonical.to_string();
                    self.editor.replace_representation(source.clone(), source);
                    self.active = None;
                    self.canonical.get_mut().take();
                    return Ok(());
                }
            };
            let view = projection.view().output().to_owned();
            self.editor.replace_representation(view.clone(), view);
            active.projection = projection;
            self.canonical.get_mut().take();
        } else {
            self.editor.restore_saved_source();
        }
        Ok(())
    }

    /// Preflight both representations before changing any state. Mode switches
    /// retain unsaved work but clear history whose coordinates no longer apply.
    /// The returned canonical-to-editor map can reposition the current cursor.
    pub fn enable(&mut self, config: Config) -> Result<Option<Translation>, Error> {
        if let Some(active) = &self.active {
            return if active.config == config {
                Ok(None)
            } else {
                Err(Error::AlreadyEnabled)
            };
        }
        if !self.editor.kind().is_typst() {
            return Err(Error::NotTypst);
        }
        let projection = Projection::open(self.editor.source(), config.clone())?;
        let saved_view = Projection::open(self.editor.saved_source(), config.clone())
            .map(|saved| saved.view().output().to_owned())
            .unwrap_or_else(|_| self.editor.saved_source().clone());
        let map = projection.view().clone();
        let saved_canonical = Arc::from(self.editor.saved_source().as_str());
        self.editor
            .replace_representation(map.output().into(), saved_view);
        self.active = Some(Active {
            config,
            projection,
            saved_canonical,
        });
        self.canonical.get_mut().take();
        Ok(Some(map))
    }

    /// An unfinished/invalid projected buffer cannot silently become native
    /// Typst math. Failure leaves source, mode, history and disk identity intact.
    pub fn disable(&mut self) -> Result<Option<Translation>, Error> {
        let Some(active) = &self.active else {
            return Ok(None);
        };
        let snapshot = self.canonical_snapshot()?;
        let map = snapshot.translation.as_ref().expect("active projection");
        self.editor
            .replace_representation(snapshot.source().into(), active.saved_canonical.to_string());
        self.active = None;
        self.canonical.get_mut().take();
        Ok(Some(map.as_ref().clone()))
    }

    /// Successful and failed translations are cached once per full document
    /// key. No-op edits/idle consumers reuse the same allocation; even a panic
    /// in an edit closure cannot leave an old map under a new revision.
    pub fn canonical_snapshot(&self) -> Result<CanonicalSnapshot, Error> {
        let key = self.editor.key();
        let mut cached = self.canonical.borrow_mut();
        if cached.as_ref().is_none_or(|cached| cached.key != key) {
            let translation = self
                .active
                .as_ref()
                .map(|active| {
                    #[cfg(test)]
                    self.encodes.set(self.encodes.get() + 1);
                    active.projection.encode(self.editor.source()).map(Arc::new)
                })
                .transpose();
            let result = translation
                .map(|translation| CanonicalSnapshot {
                    editor: self.editor.snapshot(),
                    translation,
                    coordinates: Arc::default(),
                })
                .map_err(Error::from);
            let dirty = match &result {
                Ok(snapshot) => {
                    snapshot.source()
                        != self.active.as_ref().map_or_else(
                            || self.editor.saved_source().as_str(),
                            |active| active.saved_canonical.as_ref(),
                        )
                }
                Err(_) => true,
            };
            *cached = Some(Cached { key, result, dirty });
        }
        cached.as_ref().expect("initialized above").result.clone()
    }

    pub fn is_dirty(&self) -> bool {
        if self.active.is_none() {
            return self.editor.is_dirty();
        }
        let _ = self.canonical_snapshot();
        self.canonical
            .borrow()
            .as_ref()
            .expect("initialized above")
            .dirty
    }

    pub fn prepare_save(&self, path: PathBuf, kind: DocumentKind) -> Result<SaveRequest, Error> {
        if self.active.is_some() && !kind.is_typst() {
            return Err(Error::NotTypst);
        }
        Ok(SaveRequest {
            editor: self.editor.prepare_save(path, kind),
            canonical: self
                .active
                .as_ref()
                .map(|_| self.canonical_snapshot())
                .transpose()?,
        })
    }
    pub fn record_save(&mut self, receipt: SaveReceipt) -> Result<SaveStatus, &'static str> {
        let status = self.editor.record_save(receipt.editor)?;
        if status == SaveStatus::Applied
            && let Some(active) = &mut self.active
        {
            active.saved_canonical = Arc::from(
                receipt
                    .canonical
                    .as_ref()
                    .expect("projected save receipt")
                    .source(),
            );
        }
        if status == SaveStatus::Applied {
            // An in-place receipt can change the persisted baseline without
            // changing the current editor key. Keep the map, refresh dirtiness.
            if let Some(cached) = self.canonical.get_mut() {
                cached.dirty = match &cached.result {
                    Ok(snapshot) => {
                        snapshot.source()
                            != receipt.canonical.as_ref().map_or_else(
                                || self.editor.saved_source().as_str(),
                                CanonicalSnapshot::source,
                            )
                    }
                    Err(_) => true,
                };
            }
        }
        Ok(status)
    }

    /// Reloads canonical bytes, retaining the mode for Typst files. A refusal
    /// is atomic, so the caller can show an error without losing unsaved edits.
    /// Opening a different document kind exits the mode explicitly.
    pub fn replace_loaded(
        &mut self,
        source: String,
        path: PathBuf,
        kind: DocumentKind,
        disk_fingerprint: Option<u64>,
    ) -> Result<(), Error> {
        let active = self.prepare_replacement(&source, kind)?;
        let view = active
            .as_ref()
            .map_or(source.as_str(), |active| active.projection.view().output());
        self.editor
            .replace_loaded(view.into(), path, kind, disk_fingerprint);
        self.active = active;
        self.canonical.get_mut().take();
        Ok(())
    }
    /// Open canonical source without inheriting another document's view mode.
    /// The application can then apply its automatic-activation preference.
    pub fn replace_loaded_unprojected(
        &mut self,
        source: String,
        path: PathBuf,
        kind: DocumentKind,
        disk_fingerprint: Option<u64>,
    ) {
        self.editor
            .replace_loaded(source, path, kind, disk_fingerprint);
        self.active = None;
        self.canonical.get_mut().take();
    }
    pub fn replace_untitled(&mut self, source: impl Into<String>) -> Result<(), Error> {
        let source = source.into();
        let active = self.prepare_replacement(&source, DocumentKind::Typst)?;
        let view = active
            .as_ref()
            .map_or(source.as_str(), |active| active.projection.view().output());
        self.editor.replace_untitled(view);
        self.active = active;
        self.canonical.get_mut().take();
        Ok(())
    }
    fn prepare_replacement(
        &self,
        source: &str,
        kind: DocumentKind,
    ) -> Result<Option<Active>, Error> {
        self.active
            .as_ref()
            .filter(|_| kind.is_typst())
            .map(|active| {
                Ok(Active {
                    config: active.config.clone(),
                    projection: Projection::open(source, active.config.clone())?,
                    saved_canonical: Arc::from(source),
                })
            })
            .transpose()
    }
}

mod coordinates;
mod service_edits;

#[cfg(test)]
mod tests;
