//! Window-owned document synchronization policy for Tinymist.
//!
//! This module decides versioned open/change/close and private-backing effects.
//! The app adapter remains responsible for filesystem and sidecar IO.
use std::collections::{BTreeMap, BTreeSet};

use tiptoptyp_core::document::DocumentKey;

use crate::{
    document::DocumentSession,
    tinymist::{Generation, UnsavedTextDocument, path_to_file_uri},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum BackingTarget {
    Active,
    Tab(u64),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Effect {
    UpdateBacking(BackingTarget),
    Open {
        generation: Generation,
        uri: String,
        version: i32,
    },
    Change {
        generation: Generation,
        uri: String,
        version: i32,
    },
    Close {
        generation: Generation,
        uri: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct VersionedInput {
    pub(crate) key: DocumentKey,
    pub(crate) uri: String,
    pub(crate) version: i32,
    pub(crate) source: String,
}

impl VersionedInput {
    pub(crate) fn version(&self) -> i32 {
        self.version
    }
}

/// Collects canonical bytes and identity without performing service or
/// filesystem IO. Ordinary documents reuse the document's cached snapshot and
/// do no coordinate translation; projected documents use their cached map.
pub(crate) fn collect(
    document: &DocumentSession,
    path: &std::path::Path,
) -> Result<VersionedInput, String> {
    if !document.kind().is_typst() {
        return Err("Tinymist requires a Typst document".to_owned());
    }
    let snapshot = document
        .canonical_snapshot()
        .map_err(|error| error.to_string())?;
    Ok(VersionedInput {
        key: snapshot.key(),
        uri: path_to_file_uri(path).map_err(|error| error.to_string())?,
        version: revision_as_i32(snapshot.key().revision),
        source: snapshot.source().to_owned(),
    })
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Batch {
    pub(crate) source: String,
    pub(crate) effects: Vec<Effect>,
}

impl Batch {
    fn empty() -> Self {
        Self {
            source: String::new(),
            effects: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ReplyIdentity<'a> {
    pub(crate) generation: Generation,
    pub(crate) uri: &'a str,
    pub(crate) version: i32,
    /// Identity captured when the request was sent. URI and LSP version are
    /// insufficient when a file is closed and reopened at the same revision.
    pub(crate) key: DocumentKey,
}

/// Selection, an issued open, and a synchronized current source are distinct:
/// replies and edits must never infer them from independent URI/open flags.
#[derive(Debug, Default)]
enum CurrentDocument {
    #[default]
    Absent,
    Selected(String),
    Opening(String),
    Open(String),
}
impl CurrentDocument {
    fn uri(&self) -> Option<&str> {
        match self {
            Self::Absent => None,
            Self::Selected(uri) | Self::Opening(uri) | Self::Open(uri) => Some(uri),
        }
    }
    fn is_open(&self) -> bool {
        matches!(self, Self::Open(_))
    }
}
struct ActiveSession {
    generation: Generation,
    current: CurrentDocument,
    preview_uri: Option<String>,
    open_uris: BTreeSet<String>,
}
#[derive(Default)]
enum SyncSession {
    #[default]
    Stopped,
    Active(ActiveSession),
}
impl SyncSession {
    fn active(&self) -> Option<&ActiveSession> {
        match self {
            Self::Stopped => None,
            Self::Active(session) => Some(session),
        }
    }
    fn active_mut(&mut self) -> Option<&mut ActiveSession> {
        match self {
            Self::Stopped => None,
            Self::Active(session) => Some(session),
        }
    }
}
#[derive(Default)]
pub(crate) struct Coordinator {
    session: SyncSession,
    pub(crate) active_backing: Option<UnsavedTextDocument>,
    pub(crate) tab_backings: BTreeMap<u64, UnsavedTextDocument>,
}

impl Coordinator {
    pub(crate) fn generation(&self) -> Option<Generation> {
        self.session.active().map(|session| session.generation)
    }
    pub(crate) fn current_uri(&self) -> Option<&str> {
        self.session
            .active()
            .and_then(|session| session.current.uri())
    }
    pub(crate) fn preview_uri(&self) -> Option<&str> {
        self.session
            .active()
            .and_then(|session| session.preview_uri.as_deref())
    }
    pub(crate) fn current_open(&self) -> bool {
        self.session
            .active()
            .is_some_and(|session| session.current.is_open())
    }
    #[cfg(test)]
    pub(crate) fn is_uri_open(&self, uri: &str) -> bool {
        self.session
            .active()
            .is_some_and(|session| session.open_uris.contains(uri))
    }
    /// Test setup uses one valid session owner rather than independently setting
    /// generation, URI and open flags into combinations no user can create.
    #[cfg(test)]
    pub(crate) fn fixture_session(
        &mut self,
        generation: Generation,
        current: Option<String>,
        preview: Option<String>,
    ) {
        let open_uris = preview.iter().cloned().collect();
        self.session = SyncSession::Active(ActiveSession {
            generation,
            current: current.map_or(CurrentDocument::Absent, CurrentDocument::Selected),
            preview_uri: preview,
            open_uris,
        });
    }
    #[cfg(test)]
    pub(crate) fn fixture_select_current(&mut self, uri: String) {
        self.session
            .active_mut()
            .expect("fixture session is active")
            .current = CurrentDocument::Selected(uri);
    }
    #[cfg(test)]
    pub(crate) fn fixture_open_uri(&mut self, uri: String) {
        self.session
            .active_mut()
            .expect("fixture session is active")
            .open_uris
            .insert(uri);
    }
    #[cfg(test)]
    pub(crate) fn fixture_stop_session(&mut self) {
        self.session = SyncSession::Stopped;
    }
    pub(crate) fn begin(
        &mut self,
        generation: Generation,
        current_uri: String,
        preview: VersionedInput,
    ) -> Batch {
        let version = preview.version();
        let current = if current_uri == preview.uri {
            CurrentDocument::Open(current_uri)
        } else {
            CurrentDocument::Selected(current_uri)
        };
        self.session = SyncSession::Active(ActiveSession {
            generation,
            current,
            preview_uri: None,
            open_uris: BTreeSet::new(),
        });
        let batch = self.preview_entry_changed(preview);
        debug_assert!(matches!(batch.effects.as_slice(), [Effect::Open { .. }]));
        debug_assert_eq!(
            batch.effects.first().and_then(|effect| match effect {
                Effect::Open { version, .. } => Some(*version),
                _ => None,
            }),
            Some(version)
        );
        batch
    }
    pub(crate) fn edit(&self, active_tab: Option<u64>, input: VersionedInput) -> Batch {
        let version = input.version();
        let mut effects = Vec::with_capacity(3);
        if active_tab.is_some_and(|id| self.tab_backings.contains_key(&id)) {
            effects.push(Effect::UpdateBacking(BackingTarget::Tab(
                active_tab.expect("checked above"),
            )));
        }
        if self.active_backing.is_some() {
            effects.push(Effect::UpdateBacking(BackingTarget::Active));
        }
        if let Some(session) = self.session.active()
            && session.current.is_open()
            && session.current.uri() == Some(input.uri.as_str())
        {
            effects.push(Effect::Change {
                generation: session.generation,
                uri: input.uri,
                version,
            });
        }
        Batch {
            source: input.source,
            effects,
        }
    }
    pub(crate) fn switch(
        &mut self,
        input: Option<VersionedInput>,
        close_unpinned_current: bool,
        service_ready: bool,
    ) -> Batch {
        let Some(session) = self.session.active_mut() else {
            return Batch::empty();
        };
        let generation = session.generation;
        let mut effects = Vec::with_capacity(2);
        if session.current.is_open()
            && close_unpinned_current
            && let Some(uri) = session.current.uri()
            && session.preview_uri.as_deref() != Some(uri)
        {
            let uri = uri.to_owned();
            session.open_uris.remove(&uri);
            effects.push(Effect::Close { generation, uri });
        }
        let Some(input) = input else {
            session.current = CurrentDocument::Absent;
            return Batch {
                source: String::new(),
                effects,
            };
        };
        let version = input.version();
        let already_open = session.preview_uri.as_deref() == Some(input.uri.as_str())
            || session.open_uris.contains(&input.uri);
        if already_open {
            session.current = CurrentDocument::Open(input.uri.clone());
            effects.push(Effect::Change {
                generation,
                uri: input.uri,
                version,
            });
        } else if service_ready {
            session.current = CurrentDocument::Opening(input.uri.clone());
            session.open_uris.insert(input.uri.clone());
            effects.push(Effect::Open {
                generation,
                uri: input.uri,
                version,
            });
        } else {
            session.current = CurrentDocument::Selected(input.uri);
        }
        Batch {
            source: input.source,
            effects,
        }
    }
    pub(crate) fn preview_entry_changed(&mut self, input: VersionedInput) -> Batch {
        let Some(session) = self.session.active_mut() else {
            return Batch::empty();
        };
        let generation = session.generation;
        let version = input.version();
        let mut effects = Vec::with_capacity(2);
        if let Some(old) = session.preview_uri.replace(input.uri.clone())
            && old != input.uri
            && session.current.uri() != Some(old.as_str())
        {
            session.open_uris.remove(&old);
            effects.push(Effect::Close {
                generation,
                uri: old,
            });
        }
        if !session.open_uris.contains(&input.uri) {
            session.open_uris.insert(input.uri.clone());
            effects.push(Effect::Open {
                generation,
                uri: input.uri,
                version,
            });
        }
        Batch {
            source: input.source,
            effects,
        }
    }
    pub(crate) fn open_background(&mut self, input: VersionedInput) -> Batch {
        let Some(session) = self.session.active_mut() else {
            return Batch::empty();
        };
        if session.open_uris.contains(&input.uri) {
            return Batch::empty();
        }
        let version = input.version();
        session.open_uris.insert(input.uri.clone());
        Batch {
            source: input.source,
            effects: vec![Effect::Open {
                generation: session.generation,
                uri: input.uri,
                version,
            }],
        }
    }
    /// A changed parked document updates an existing URI rather than silently
    /// stopping at the already-open guard used during initial hydration.
    pub(crate) fn change_background(&mut self, input: VersionedInput) -> Batch {
        if let Some(session) = self.session.active()
            && session.open_uris.contains(&input.uri)
        {
            let version = input.version();
            Batch {
                source: input.source,
                effects: vec![Effect::Change {
                    generation: session.generation,
                    uri: input.uri,
                    version,
                }],
            }
        } else {
            self.open_background(input)
        }
    }
    pub(crate) fn close_uri(&mut self, uri: &str) -> Option<Effect> {
        let session = self.session.active_mut()?;
        if !session.open_uris.remove(uri) {
            return None;
        }
        if session.current.uri() == Some(uri) {
            session.current = CurrentDocument::Selected(uri.to_owned());
        }
        if session.preview_uri.as_deref() == Some(uri) {
            session.preview_uri = None;
        }
        Some(Effect::Close {
            generation: session.generation,
            uri: uri.to_owned(),
        })
    }
    pub(crate) fn confirm_open(&mut self, uri: &str) {
        if let Some(session) = self.session.active_mut()
            && session.current.uri() == Some(uri)
        {
            session.open_uris.insert(uri.to_owned());
            session.current = CurrentDocument::Open(uri.to_owned());
        }
    }
    pub(crate) fn stop_effects(&self) -> Vec<Effect> {
        let Some(session) = self.session.active() else {
            return Vec::new();
        };
        let mut uris = BTreeSet::new();
        if session.current.is_open()
            && let Some(uri) = session.current.uri()
        {
            uris.insert(uri.to_owned());
        }
        if let Some(uri) = &session.preview_uri {
            uris.insert(uri.clone());
        }
        uris.into_iter()
            .map(|uri| Effect::Close {
                generation: session.generation,
                uri,
            })
            .collect()
    }
    /// Send close effects before releasing the active private backing file.
    pub(crate) fn finish_stop(&mut self) {
        self.session = SyncSession::Stopped;
        self.active_backing = None;
    }
    pub(crate) fn accepts_reply(&self, reply: ReplyIdentity<'_>, current: DocumentKey) -> bool {
        self.generation() == Some(reply.generation)
            && self.current_uri() == Some(reply.uri)
            && reply.version == revision_as_i32(current.revision)
            && reply.key == current
    }
    pub(crate) fn backing(&self, target: BackingTarget) -> Option<&UnsavedTextDocument> {
        match target {
            BackingTarget::Active => self.active_backing.as_ref(),
            BackingTarget::Tab(id) => self.tab_backings.get(&id),
        }
    }
}

pub(crate) fn revision_as_i32(revision: u64) -> i32 {
    i32::try_from(revision).unwrap_or(i32::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;
    use eframe::egui::text::CCursorRange;
    use tiptoptyp_core::document::{DocumentKind, WindowSessionId};

    #[test]
    fn session_generation_owns_every_open_uri_and_a_new_begin_starts_fresh() {
        let mut sync = Coordinator::default();
        assert!(sync.generation().is_none());
        assert!(sync.current_uri().is_none());
        assert!(!sync.current_open());
        sync.begin(
            Generation(1),
            "file:///main.typ".into(),
            input(1, 1, 1, "file:///main.typ", "main"),
        );
        sync.open_background(input(1, 2, 1, "file:///old.typ", "old"));
        sync.begin(
            Generation(2),
            "file:///main.typ".into(),
            input(1, 1, 1, "file:///main.typ", "main"),
        );
        assert!(!sync.is_uri_open("file:///old.typ"));
        let reopened = sync.open_background(input(1, 2, 1, "file:///old.typ", "old"));
        assert!(matches!(
            reopened.effects.as_slice(),
            [Effect::Open {
                generation: Generation(2),
                ..
            }]
        ));
        sync.finish_stop();
        assert!(sync.generation().is_none());
        assert!(sync.current_uri().is_none());
        assert!(sync.preview_uri().is_none());
        assert!(!sync.is_uri_open("file:///old.typ"));
    }

    #[test]
    fn selected_and_opening_sources_wait_for_the_exact_open_receipt() {
        let mut sync = Coordinator::default();
        sync.begin(
            Generation(1),
            "file:///main.typ".into(),
            input(1, 1, 1, "file:///main.typ", "main"),
        );
        let selected = sync.switch(
            Some(input(1, 2, 1, "file:///child.typ", "child")),
            false,
            false,
        );
        assert!(selected.effects.is_empty());
        assert!(!sync.current_open());
        let opening = sync.switch(
            Some(input(1, 2, 1, "file:///child.typ", "child")),
            false,
            true,
        );
        assert!(matches!(opening.effects.as_slice(), [Effect::Open { .. }]));
        assert!(!sync.current_open());
        assert!(
            sync.edit(None, input(1, 2, 2, "file:///child.typ", "edited"))
                .effects
                .is_empty()
        );
        sync.confirm_open("file:///other.typ");
        assert!(!sync.current_open());
        sync.confirm_open("file:///child.typ");
        assert!(sync.current_open());
        assert!(matches!(
            sync.edit(None, input(1, 2, 2, "file:///child.typ", "edited"))
                .effects
                .as_slice(),
            [Effect::Change { .. }]
        ));
        sync.close_uri("file:///child.typ").unwrap();
        assert!(!sync.current_open());
        assert_eq!(sync.current_uri(), Some("file:///child.typ"));
    }

    #[test]
    fn changed_parked_sources_update_open_documents_without_reopening_or_restarting() {
        let mut sync = Coordinator::default();
        sync.begin(
            Generation(77),
            "file:///main.typ".into(),
            input(1, 1, 1, "file:///main.typ", "main"),
        );
        let opened = sync.change_background(input(1, 1, 1, "file:///chapter.typ", "old"));
        assert!(matches!(opened.effects.as_slice(), [Effect::Open { .. }]));
        let changed = sync.change_background(input(1, 1, 2, "file:///chapter.typ", "new"));
        assert_eq!(changed.source, "new");
        assert!(
            matches!(changed.effects.as_slice(), [Effect::Change { generation: Generation(77), version: 2, uri }] if uri == "file:///chapter.typ")
        );
        assert_eq!(sync.current_uri(), Some("file:///main.typ"));
        assert_eq!(sync.preview_uri(), Some("file:///main.typ"));
        assert_eq!(sync.generation(), Some(Generation(77)));
    }

    #[test]
    fn native_tex_cannot_cross_the_tinymist_source_boundary() {
        let document =
            DocumentSession::new(WindowSessionId::new(1), "$\\alpha$", DocumentKind::Tex);
        assert_eq!(
            collect(&document, std::path::Path::new("/paper.tex")).unwrap_err(),
            "Tinymist requires a Typst document"
        );
    }

    fn input(owner: u64, epoch: u64, revision: u64, uri: &str, source: &str) -> VersionedInput {
        VersionedInput {
            key: DocumentKey {
                owner: WindowSessionId::new(owner),
                epoch,
                revision,
            },
            uri: uri.into(),
            version: revision_as_i32(revision),
            source: source.into(),
        }
    }

    #[test]
    fn command_log_covers_open_edit_switch_preview_change_and_close() {
        let generation = Generation(7);
        let mut sync = Coordinator::default();
        let started = sync.begin(
            generation,
            "file:///main.typ".into(),
            input(1, 2, 3, "file:///main.typ", "= Main"),
        );
        assert!(matches!(started.effects.as_slice(), [Effect::Open { .. }]));
        let edited = sync.edit(Some(0), input(1, 2, 4, "file:///main.typ", "= Edited"));
        assert!(matches!(
            edited.effects.as_slice(),
            [Effect::Change { version: 4, .. }]
        ));

        let switched = sync.switch(
            Some(input(1, 3, 1, "file:///chapter.typ", "= Chapter")),
            true,
            true,
        );
        assert!(matches!(switched.effects.as_slice(), [Effect::Open { .. }]));
        sync.confirm_open("file:///chapter.typ");
        let preview =
            sync.preview_entry_changed(input(1, 2, 5, "file:///preview.typ", "= Preview"));
        assert!(matches!(
            preview.effects.as_slice(),
            [Effect::Close { .. }, Effect::Open { .. }]
        ));
        assert_eq!(sync.stop_effects().len(), 2);
    }

    #[test]
    fn explicitly_closed_preview_uri_is_not_closed_again_on_shutdown() {
        let generation = Generation(8);
        let mut sync = Coordinator::default();
        sync.begin(
            generation,
            "file:///main.typ".into(),
            input(1, 1, 1, "file:///main.typ", "= Main"),
        );
        assert!(sync.close_uri("file:///main.typ").is_some());
        assert!(sync.preview_uri().is_none());
        assert!(sync.stop_effects().is_empty());
    }

    #[test]
    fn projection_mode_change_is_a_canonical_versioned_change() {
        let generation = Generation(9);
        let mut sync = Coordinator::default();
        sync.begin(
            generation,
            "file:///main.typ".into(),
            input(1, 1, 1, "file:///main.typ", "$ alpha $"),
        );
        let changed = sync.edit(
            Some(0),
            input(1, 2, 2, "file:///main.typ", "#mitex(`alpha`)"),
        );
        assert_eq!(changed.source, "#mitex(`alpha`)");
        assert!(matches!(
            changed.effects.as_slice(),
            [Effect::Change { version: 2, .. }]
        ));
    }

    #[test]
    fn backing_update_is_a_typed_effect_and_not_policy_layer_io() {
        let root = tempfile::tempdir().unwrap();
        let backing =
            UnsavedTextDocument::create(root.path(), root.path(), "Untitled.typ", "= Initial")
                .unwrap();
        let generation = Generation(10);
        let mut sync = Coordinator::default();
        sync.begin(
            generation,
            "file:///main.typ".into(),
            input(1, 1, 1, "file:///main.typ", "= Initial"),
        );
        sync.tab_backings.insert(4, backing);
        let batch = sync.edit(Some(4), input(1, 1, 2, "file:///main.typ", "= Edited"));
        assert!(matches!(
            batch.effects.as_slice(),
            [
                Effect::UpdateBacking(BackingTarget::Tab(4)),
                Effect::Change { version: 2, .. }
            ]
        ));
    }

    #[test]
    fn late_generation_uri_and_version_replies_are_rejected() {
        let generation = Generation(4);
        let mut sync = Coordinator::default();
        let current = input(2, 3, 8, "file:///main.typ", "text");
        sync.begin(generation, current.uri.clone(), current.clone());
        let accepted = ReplyIdentity {
            generation,
            uri: &current.uri,
            version: 8,
            key: current.key,
        };
        assert!(sync.accepts_reply(accepted, current.key));
        assert!(!sync.accepts_reply(
            ReplyIdentity {
                generation: Generation(3),
                ..accepted
            },
            current.key
        ));
        assert!(!sync.accepts_reply(
            ReplyIdentity {
                uri: "file:///old.typ",
                ..accepted
            },
            current.key
        ));
        assert!(!sync.accepts_reply(
            ReplyIdentity {
                version: 7,
                ..accepted
            },
            current.key
        ));
        assert!(!sync.accepts_reply(
            ReplyIdentity {
                key: DocumentKey {
                    epoch: current.key.epoch + 1,
                    ..current.key
                },
                ..accepted
            },
            current.key
        ));
    }

    #[test]
    fn collection_keeps_canonical_and_display_coordinates_separate() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("main.typ");
        let canonical = "#import \"@preview/mitex:0.2.7\": mi\n#mi(`\\alpha`)\n";
        let mut document =
            DocumentSession::new(WindowSessionId::new(3), canonical, DocumentKind::Typst);
        let ordinary = collect(&document, &path).unwrap();
        assert_eq!(ordinary.source, canonical);
        assert_eq!(ordinary.key, document.key());

        document
            .enable(tiptoptyp::mitex_projection::Config::default())
            .unwrap();
        assert_ne!(document.source(), canonical);
        let projected = collect(&document, &path).unwrap();
        assert_eq!(projected.source, canonical);
        assert_eq!(projected.key, document.key());

        document.edit(CCursorRange::default(), |source| source.push_str("text"));
        let edited = collect(&document, &path).unwrap();
        assert_ne!(edited.key, projected.key);
    }
}
