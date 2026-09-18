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

pub(crate) fn preview_root<'a>(
    active_root: &'a std::path::Path,
    preview: Option<(&'a std::path::Path, bool)>,
) -> &'a std::path::Path {
    preview
        .filter(|(_, is_typst)| *is_typst)
        .map_or(active_root, |(root, _)| root)
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
}

#[derive(Default)]
pub(crate) struct Coordinator {
    pub(crate) generation: Option<Generation>,
    pub(crate) current_uri: Option<String>,
    pub(crate) preview_uri: Option<String>,
    pub(crate) current_open: bool,
    pub(crate) active_backing: Option<UnsavedTextDocument>,
    pub(crate) tab_backings: BTreeMap<u64, UnsavedTextDocument>,
    pub(crate) open_uris: BTreeSet<String>,
}

impl Coordinator {
    pub(crate) fn begin(
        &mut self,
        generation: Generation,
        current_uri: String,
        preview: VersionedInput,
    ) -> Batch {
        let version = preview.version();
        self.generation = Some(generation);
        self.current_uri = Some(current_uri.clone());
        self.preview_uri = None;
        self.current_open = current_uri == preview.uri;
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
        if let Some(generation) = self.generation
            && self.current_open
            && self.current_uri.as_deref() == Some(input.uri.as_str())
        {
            effects.push(Effect::Change {
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

    pub(crate) fn switch(
        &mut self,
        input: Option<VersionedInput>,
        close_unpinned_current: bool,
        service_ready: bool,
    ) -> Batch {
        let Some(generation) = self.generation else {
            return Batch::empty();
        };
        let mut effects = Vec::with_capacity(2);
        if self.current_open
            && close_unpinned_current
            && let Some(uri) = self.current_uri.take()
            && self.preview_uri.as_deref() != Some(uri.as_str())
        {
            self.open_uris.remove(&uri);
            effects.push(Effect::Close { generation, uri });
        }
        self.current_open = false;
        let Some(input) = input else {
            self.current_uri = None;
            return Batch {
                source: String::new(),
                effects,
            };
        };
        let version = input.version();
        self.current_uri = Some(input.uri.clone());
        let already_open = self.preview_uri.as_deref() == Some(input.uri.as_str())
            || self.open_uris.contains(&input.uri);
        if already_open {
            self.current_open = true;
            effects.push(Effect::Change {
                generation,
                uri: input.uri,
                version,
            });
        } else if service_ready {
            self.open_uris.insert(input.uri.clone());
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

    pub(crate) fn preview_entry_changed(&mut self, input: VersionedInput) -> Batch {
        let Some(generation) = self.generation else {
            return Batch::empty();
        };
        let version = input.version();
        let mut effects = Vec::with_capacity(2);
        if let Some(old) = self.preview_uri.replace(input.uri.clone())
            && old != input.uri
            && self.current_uri.as_deref() != Some(old.as_str())
        {
            self.open_uris.remove(&old);
            effects.push(Effect::Close {
                generation,
                uri: old,
            });
        }
        if !self.open_uris.contains(&input.uri) {
            self.open_uris.insert(input.uri.clone());
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
        let Some(generation) = self.generation else {
            return Batch::empty();
        };
        if self.open_uris.contains(&input.uri) {
            return Batch::empty();
        }
        let version = input.version();
        self.open_uris.insert(input.uri.clone());
        Batch {
            source: input.source,
            effects: vec![Effect::Open {
                generation,
                uri: input.uri,
                version,
            }],
        }
    }

    pub(crate) fn close_uri(&mut self, uri: &str) -> Option<Effect> {
        let generation = self.generation?;
        if !self.open_uris.remove(uri) {
            return None;
        }
        if self.current_uri.as_deref() == Some(uri) {
            self.current_open = false;
        }
        Some(Effect::Close {
            generation,
            uri: uri.to_owned(),
        })
    }

    pub(crate) fn confirm_open(&mut self, uri: &str) {
        if self.current_uri.as_deref() == Some(uri) {
            self.current_open = true;
        }
    }

    pub(crate) fn stop_effects(&self) -> Vec<Effect> {
        let Some(generation) = self.generation else {
            return Vec::new();
        };
        let mut uris = BTreeSet::new();
        if self.current_open
            && let Some(uri) = &self.current_uri
        {
            uris.insert(uri.clone());
        }
        if let Some(uri) = &self.preview_uri {
            uris.insert(uri.clone());
        }
        uris.into_iter()
            .map(|uri| Effect::Close { generation, uri })
            .collect()
    }

    /// Clear only after close effects have been sent, so private files remain
    /// alive while Tinymist processes `didClose`.
    pub(crate) fn finish_stop(&mut self) {
        self.generation = None;
        self.current_uri = None;
        self.preview_uri = None;
        self.current_open = false;
        self.open_uris.clear();
        self.active_backing = None;
    }

    pub(crate) fn accepts_reply(&self, reply: ReplyIdentity<'_>, current: DocumentKey) -> bool {
        self.generation == Some(reply.generation)
            && self.current_uri.as_deref() == Some(reply.uri)
            && reply.version == revision_as_i32(current.revision)
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

    #[test]
    fn preview_root_uses_only_a_compatible_designated_document() {
        let active = std::path::Path::new("/active");
        let preview = std::path::Path::new("/preview");
        assert_eq!(preview_root(active, None), active);
        assert_eq!(preview_root(active, Some((preview, false))), active);
        assert_eq!(preview_root(active, Some((preview, true))), preview);
    }
}
