//! Overleaf identities, authoritative browser messages and working-copy policy.
//!
//! The browser owns Overleaf's collaborative protocol. This module never
//! equates a local mirror write with acknowledgement from Overleaf.
mod mirror;
mod model;
mod presence;
mod storage;

pub(crate) use mirror::Mirror;
pub(crate) use model::{Model, ModelChange};
pub(crate) use presence::{Collaborator, PresenceSnapshot, RemoteCursor, SourceIndex};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Component, Path, PathBuf},
};
pub(crate) use storage::{StorageLease, StorageOptions, StoredProject, finish_temporary_cleanup};
use unicode_normalization::UnicodeNormalization as _;

pub(crate) const MAX_SOURCE_BYTES: usize = 4 * 1024 * 1024;
pub(crate) const MAX_FILE_BYTES: usize = 8 * 1024 * 1024;
pub(crate) const MAX_ENTRIES: usize = 10_000;
// The browser IPC envelope is capped at 16 MiB. A retained listing must be
// readable under the same bound on the next worker/session.
const MAX_TREE_MANIFEST_BYTES: usize = 16 * 1024 * 1024;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ProjectUrl {
    url: String,
    origin: String,
    id: String,
}
impl ProjectUrl {
    pub(crate) fn parse(input: &str) -> Result<Self, String> {
        let parsed = url::Url::parse(input.trim())
            .map_err(|_| "Enter an Overleaf project URL.".to_owned())?;
        if !parsed.username().is_empty()
            || parsed.password().is_some()
            || parsed.query().is_some()
            || parsed.fragment().is_some()
        {
            return Err(
                "Use a project URL without credentials, query parameters or a fragment.".into(),
            );
        }
        let host = parsed.host_str().ok_or("The project URL needs a host.")?;
        let local = matches!(host, "localhost" | "127.0.0.1" | "[::1]" | "::1");
        if parsed.scheme() != "https" && !(parsed.scheme() == "http" && local) {
            return Err(
                "Overleaf connections require HTTPS (HTTP is allowed only on localhost).".into(),
            );
        }
        let path = parsed.path().strip_suffix('/').unwrap_or(parsed.path());
        let id = path
            .strip_prefix("/project/")
            .filter(|id| id.len() == 24 && id.bytes().all(|byte| byte.is_ascii_hexdigit()))
            .ok_or("Use a project URL ending in /project/ followed by its 24-character ID.")?;
        let id = id.to_ascii_lowercase();
        let origin = parsed.origin().ascii_serialization();
        Ok(Self {
            url: format!("{origin}/project/{id}"),
            origin,
            id,
        })
    }
    pub(crate) fn url(&self) -> &str {
        &self.url
    }
    pub(crate) fn origin(&self) -> &str {
        &self.origin
    }
    pub(crate) fn id(&self) -> &str {
        &self.id
    }
    /// Stable across sessions; different self-hosted origins cannot alias.
    pub(crate) fn mirror_root(&self, base: &Path) -> PathBuf {
        use sha2::{Digest, Sha256};
        let digest = Sha256::digest(self.origin.as_bytes());
        let namespace: String = digest[..8]
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect();
        base.join(format!("{namespace}-{}", self.id))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum EntryKind {
    Doc,
    File,
    Folder,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Entry {
    pub(crate) id: String,
    pub(crate) path: String,
    pub(crate) kind: EntryKind,
}
impl Entry {
    pub(crate) fn relative_path(&self) -> Result<PathBuf, String> {
        validate_path(&self.path)
    }
}

/// Remote listing, never a scan of generated local build artifacts.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Tree {
    pub(crate) name: String,
    pub(crate) main: Option<String>,
    pub(crate) entries: Vec<Entry>,
    by_id: BTreeMap<String, usize>,
    by_path: BTreeMap<String, usize>,
}
impl Tree {
    pub(crate) fn new(
        name: String,
        main: Option<String>,
        mut entries: Vec<Entry>,
    ) -> Result<Self, String> {
        if entries.len() > MAX_ENTRIES {
            return Err("The Overleaf project contains too many files.".into());
        }
        if name.len() > 4096 {
            return Err("The project name is too long.".into());
        }
        let mut ids = BTreeSet::new();
        let mut paths = BTreeMap::new();
        for entry in &entries {
            entry.relative_path()?;
            if entry.id.is_empty()
                || entry.id.len() > 256
                || entry.id.chars().any(char::is_control)
                || !ids.insert(entry.id.clone())
            {
                return Err("The project listing contains an invalid or repeated file ID.".into());
            }
            // Native macOS filesystems commonly fold case. Reject a listing
            // that cannot be represented faithfully rather than clobber it.
            if paths
                .insert(
                    entry.path.nfc().collect::<String>().to_lowercase(),
                    entry.kind,
                )
                .is_some()
            {
                return Err(format!(
                    "Project paths collide on this filesystem: {}",
                    entry.path
                ));
            }
        }
        for entry in &entries {
            let mut parent = Path::new(&entry.path).parent();
            while let Some(path) = parent.filter(|path| !path.as_os_str().is_empty()) {
                if paths
                    .get(
                        &path
                            .to_string_lossy()
                            .nfc()
                            .collect::<String>()
                            .to_lowercase(),
                    )
                    .is_some_and(|kind| *kind != EntryKind::Folder)
                {
                    return Err(format!(
                        "A file is also used as a folder: {}",
                        path.display()
                    ));
                }
                parent = path.parent();
            }
        }
        if main.as_ref().is_some_and(|id| {
            !entries
                .iter()
                .any(|entry| &entry.id == id && entry.kind == EntryKind::Doc)
        }) {
            return Err("The main Overleaf document is absent from the project listing.".into());
        }
        check_manifest_size(&entries)?;
        entries.sort_by(|a, b| a.path.cmp(&b.path));
        let by_id = entries
            .iter()
            .enumerate()
            .map(|(i, entry)| (entry.id.clone(), i))
            .collect();
        let by_path = entries
            .iter()
            .enumerate()
            .map(|(i, entry)| (entry.path.clone(), i))
            .collect();
        Ok(Self {
            name,
            main,
            entries,
            by_id,
            by_path,
        })
    }
    pub(crate) fn entries(&self) -> &[Entry] {
        &self.entries
    }
    pub(crate) fn entry(&self, id: &str) -> Option<&Entry> {
        self.by_id.get(id).map(|&index| &self.entries[index])
    }
    pub(crate) fn id_for_path(&self, path: &str) -> Option<&str> {
        self.by_path
            .get(path)
            .map(|&index| self.entries[index].id.as_str())
    }
}

/// Count the actual escaped JSON representation without allocating another
/// copy of a large listing or touching the filesystem.
fn check_manifest_size(entries: &[Entry]) -> Result<(), String> {
    struct SizeLimit(usize);
    impl std::io::Write for SizeLimit {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            let next = self
                .0
                .checked_add(bytes.len())
                .filter(|next| *next <= MAX_TREE_MANIFEST_BYTES)
                .ok_or_else(|| std::io::Error::other("Overleaf listing exceeds 16 MiB"))?;
            self.0 = next;
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    serde_json::to_writer(SizeLimit(0), entries)
        .map_err(|_| "The Overleaf project listing exceeds the 16 MiB size limit.".into())
}

pub(crate) fn validate_path(path: &str) -> Result<PathBuf, String> {
    if path.is_empty()
        || path.len() > 4096
        || path.contains('\\')
        || path.chars().any(char::is_control)
        || path.starts_with('/')
        || path.ends_with('/')
        || path.split('/').any(|part| {
            part.is_empty()
                || part == "."
                || part == ".."
                || part.eq_ignore_ascii_case(".tiptoptyp")
                || part.eq_ignore_ascii_case(".git")
        })
        || path.as_bytes().get(1) == Some(&b':')
    {
        return Err(format!("Unsafe Overleaf project path: {path:?}"));
    }
    let value = PathBuf::from(path);
    if !value
        .components()
        .all(|part| matches!(part, Component::Normal(_)))
    {
        return Err("Project files must have relative paths.".into());
    }
    Ok(value)
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "command", rename_all = "lowercase")]
pub(crate) enum BrowserCommand {
    Download {
        id: String,
        path: String,
        epoch: u64,
    },
    Open {
        id: String,
        path: String,
        epoch: u64,
    },
    Edit {
        id: String,
        base: String,
        source: String,
        request: u64,
        epoch: u64,
    },
    Cursor {
        epoch: u64,
        id: String,
        source_fingerprint: String,
        anchor: usize,
        head: usize,
    },
    Refresh {
        epoch: u64,
    },
    Disconnect {
        epoch: u64,
    },
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "lowercase")]
pub(crate) enum BrowserEvent {
    Project {
        epoch: u64,
        project_id: String,
        name: String,
        entries: Vec<Entry>,
        #[serde(default)]
        root_doc: Option<String>,
    },
    Document {
        epoch: u64,
        id: String,
        path: String,
        source: String,
    },
    Ack {
        epoch: u64,
        id: String,
        request: u64,
        source: String,
    },
    File {
        epoch: u64,
        id: String,
        path: String,
        base64: String,
    },
    Conflict {
        epoch: u64,
        id: String,
        request: u64,
        source: String,
        message: String,
    },
    Error {
        epoch: u64,
        message: String,
    },
    Presence {
        epoch: u64,
        #[serde(flatten)]
        snapshot: PresenceSnapshot,
    },
    Status {
        epoch: u64,
        state: String,
        #[serde(default)]
        message: Option<String>,
    },
}
impl BrowserEvent {
    pub(crate) fn epoch(&self) -> u64 {
        match self {
            Self::Project { epoch, .. }
            | Self::Document { epoch, .. }
            | Self::Ack { epoch, .. }
            | Self::File { epoch, .. }
            | Self::Conflict { epoch, .. }
            | Self::Error { epoch, .. }
            | Self::Status { epoch, .. }
            | Self::Presence { epoch, .. } => *epoch,
        }
    }
}

#[cfg(test)]
mod tests;
