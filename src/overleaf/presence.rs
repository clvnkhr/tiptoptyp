//! Ephemeral, source-anchored collaboration state. Nothing here is saved to disk.
use super::{EntryKind, MAX_SOURCE_BYTES, Tree};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
};

const MAX_PEERS: usize = 128;

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct PresenceSnapshot {
    pub(crate) own_id: Option<String>,
    pub(crate) self_user_id: Option<String>,
    pub(crate) peers: Vec<PresencePeer>,
    pub(crate) documents: Vec<PresenceDocument>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct PresencePeer {
    pub(crate) id: String,
    pub(crate) user_id: Option<String>,
    pub(crate) name: String,
    pub(crate) doc_id: Option<String>,
    pub(crate) row: Option<usize>,
    pub(crate) column: Option<usize>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct PresenceDocument {
    pub(crate) id: String,
    pub(crate) source_fingerprint: String,
    pub(crate) source_utf16: usize,
    pub(crate) cursors: Vec<PresenceCursor>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct PresenceCursor {
    pub(crate) id: String,
    pub(crate) anchor: usize,
    pub(crate) head: usize,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Collaborator {
    pub(crate) id: String,
    pub(crate) name: String,
    pub(crate) color: [u8; 3],
    pub(crate) doc_id: Option<String>,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct RemoteCursor {
    pub(crate) id: String,
    pub(crate) name: String,
    pub(crate) color: [u8; 3],
    pub(crate) anchor: usize,
    pub(crate) head: usize,
}
#[derive(Default)]
pub(crate) struct Presence {
    generation: u64,
    snapshot: PresenceSnapshot,
    collaborators: Arc<[Collaborator]>,
    documents: BTreeMap<String, PresenceDocument>,
}
impl Presence {
    pub(crate) fn generation(&self) -> u64 {
        self.generation
    }
    pub(crate) fn collaborators(&self) -> Arc<[Collaborator]> {
        self.collaborators.clone()
    }
    pub(crate) fn clear(&mut self) {
        self.replace(PresenceSnapshot::default(), None);
    }
    /// Invalid ephemeral data cannot take down the authoritative save stream.
    /// A full replacement also makes departed clients disappear immediately.
    pub(crate) fn replace(&mut self, snapshot: PresenceSnapshot, tree: Option<&Tree>) -> bool {
        if snapshot == self.snapshot || !valid(&snapshot, tree) {
            return false;
        }
        let own = snapshot.own_id.as_deref();
        let mut collaborators: Vec<_> = snapshot
            .peers
            .iter()
            .filter(|peer| Some(peer.id.as_str()) != own)
            .map(|peer| Collaborator {
                id: peer.id.clone(),
                name: peer.name.clone(),
                doc_id: peer.doc_id.clone(),
                color: site_color(peer.user_id.as_deref(), snapshot.self_user_id.as_deref()),
            })
            .collect();
        collaborators.sort_by(|a, b| a.name.cmp(&b.name).then_with(|| a.id.cmp(&b.id)));
        self.collaborators = collaborators.into();
        self.documents = snapshot
            .documents
            .iter()
            .cloned()
            .map(|doc| (doc.id.clone(), doc))
            .collect();
        self.snapshot = snapshot;
        self.generation = self.generation.wrapping_add(1);
        true
    }
    pub(crate) fn retain_tree(&mut self, tree: &Tree) {
        let mut snapshot = self.snapshot.clone();
        snapshot.documents.retain(|doc| {
            tree.entry(&doc.id)
                .is_some_and(|e| e.kind == EntryKind::Doc)
        });
        for peer in &mut snapshot.peers {
            if peer
                .doc_id
                .as_ref()
                .is_some_and(|id| tree.entry(id).is_none())
            {
                peer.doc_id = None;
                peer.row = None;
                peer.column = None;
            }
        }
        self.replace(snapshot, Some(tree));
    }
    pub(crate) fn cursors(
        &self,
        id: &str,
        index: &SourceIndex,
        source: &str,
    ) -> Arc<[RemoteCursor]> {
        let Some(document) = self.documents.get(id).filter(|document| {
            document.source_fingerprint == index.fingerprint
                && document.source_utf16 == index.utf16_len
        }) else {
            return Arc::default();
        };
        document
            .cursors
            .iter()
            .filter_map(|cursor| {
                let peer = self
                    .collaborators
                    .iter()
                    .find(|peer| peer.id == cursor.id && peer.doc_id.as_deref() == Some(id))?;
                Some(RemoteCursor {
                    id: peer.id.clone(),
                    name: peer.name.clone(),
                    color: peer.color,
                    anchor: index.scalar_at_utf16(source, cursor.anchor)?,
                    head: index.scalar_at_utf16(source, cursor.head)?,
                })
            })
            .collect::<Vec<_>>()
            .into()
    }
}
fn valid(snapshot: &PresenceSnapshot, tree: Option<&Tree>) -> bool {
    let identity = |s: &str| !s.is_empty() && s.len() <= 256 && !s.chars().any(char::is_control);
    if snapshot.peers.len() > MAX_PEERS
        || snapshot.documents.len() > MAX_PEERS
        || snapshot.own_id.as_deref().is_some_and(|s| !identity(s))
        || snapshot
            .self_user_id
            .as_deref()
            .is_some_and(|s| !identity(s))
    {
        return false;
    }
    let mut peers = BTreeSet::new();
    for peer in &snapshot.peers {
        if !identity(&peer.id)
            || peer.name.len() > 256
            || peer.name.chars().any(char::is_control)
            || peer.user_id.as_deref().is_some_and(|s| !identity(s))
            || !peers.insert(peer.id.as_str())
            || peer.doc_id.as_deref().is_some_and(|s| !identity(s))
            || peer.row.is_some_and(|n| n > MAX_SOURCE_BYTES)
            || peer.column.is_some_and(|n| n > MAX_SOURCE_BYTES)
        {
            return false;
        }
    }
    let mut documents = BTreeSet::new();
    let mut count = 0usize;
    for doc in &snapshot.documents {
        count = count.saturating_add(doc.cursors.len());
        if count > MAX_PEERS
            || !documents.insert(doc.id.as_str())
            || !identity(&doc.id)
            || doc.source_utf16 > MAX_SOURCE_BYTES
            || doc.source_fingerprint.len() != 16
            || !doc
                .source_fingerprint
                .bytes()
                .all(|b| b.is_ascii_hexdigit())
            || !tree.is_some_and(|tree| {
                tree.entry(&doc.id)
                    .is_some_and(|entry| entry.kind == EntryKind::Doc)
            })
        {
            return false;
        }
        let mut cursors = BTreeSet::new();
        for cursor in &doc.cursors {
            if !peers.contains(cursor.id.as_str())
                || !cursors.insert(cursor.id.as_str())
                || cursor.anchor > doc.source_utf16
                || cursor.head > doc.source_utf16
            {
                return false;
            }
        }
    }
    true
}
/// Overleaf shared/utils/colors.ts: HSL(hue,70%,50%), including anonymous and
/// same-account clients. MD5 is a stable color selector, not authentication.
fn site_color(user: Option<&str>, own_user: Option<&str>) -> [u8; 3] {
    let hue = match user {
        None => 100,
        Some(user) if Some(user) == own_user => 200,
        Some(user) => {
            let digest = md5::compute(user.as_bytes());
            let mut hue = u32::from_be_bytes(digest.0[..4].try_into().unwrap()) % 320;
            if hue > 180 && hue < 220 {
                hue += 140;
            }
            hue
        }
    };
    let h = hue as f32 / 60.0;
    let x = 0.7 * (1.0 - ((h % 2.0) - 1.0).abs());
    let rgb = match hue / 60 {
        0 => (0.7, x, 0.0),
        1 => (x, 0.7, 0.0),
        2 => (0.0, 0.7, x),
        3 => (0.0, x, 0.7),
        4 => (x, 0.0, 0.7),
        _ => (0.7, 0.0, x),
    };
    [rgb.0, rgb.1, rgb.2].map(|v| ((v + 0.15) * 255.0).round() as u8)
}
#[derive(Debug)]
struct Line {
    byte: usize,
    scalar: usize,
    utf16: usize,
    utf16_end: usize,
}
/// One index per native DocumentKey. Cursor updates scan only their line;
/// fingerprints and document-wide coordinates are never recomputed per paint.
#[derive(Debug)]
pub(crate) struct SourceIndex {
    pub(crate) fingerprint: String,
    pub(crate) utf16_len: usize,
    scalar_len: usize,
    lines: Vec<Line>,
}
impl SourceIndex {
    pub(crate) fn new(source: &str) -> Self {
        let hash = source.bytes().fold(0xcbf29ce484222325u64, |h, b| {
            (h ^ u64::from(b)).wrapping_mul(0x100000001b3)
        });
        let mut lines = vec![Line {
            byte: 0,
            scalar: 0,
            utf16: 0,
            utf16_end: 0,
        }];
        let mut utf16 = 0;
        let mut scalar_len = 0;
        let mut previous = None;
        for (scalar, (byte, ch)) in source.char_indices().enumerate() {
            utf16 += ch.len_utf16();
            scalar_len = scalar + 1;
            if ch == '\n' {
                lines.last_mut().unwrap().utf16_end =
                    utf16 - 1 - usize::from(previous == Some('\r'));
                lines.push(Line {
                    byte: byte + 1,
                    scalar: scalar + 1,
                    utf16,
                    utf16_end: utf16,
                });
            }
            previous = Some(ch);
        }
        lines.last_mut().unwrap().utf16_end = utf16 - usize::from(previous == Some('\r'));
        Self {
            fingerprint: format!("{hash:016x}"),
            utf16_len: utf16,
            scalar_len,
            lines,
        }
    }
    pub(crate) fn scalar_at_utf16(&self, source: &str, target: usize) -> Option<usize> {
        if target > self.utf16_len {
            return None;
        }
        let line = &self.lines[self.lines.partition_point(|line| line.utf16 <= target) - 1];
        let mut utf16 = line.utf16;
        for (scalar, ch) in source[line.byte..].chars().enumerate() {
            if utf16 == target {
                return Some(line.scalar + scalar);
            }
            utf16 += ch.len_utf16();
            if utf16 > target {
                return None;
            }
        }
        (utf16 == target).then_some(self.scalar_len)
    }
    pub(crate) fn utf16_at_position(&self, row: u32, column: u32) -> Option<usize> {
        let line = self.lines.get(row as usize)?;
        line.utf16
            .checked_add(column as usize)
            .filter(|n| *n <= line.utf16_end)
    }
}

#[cfg(test)]
mod tests;
