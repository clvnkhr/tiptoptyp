//! Bounded, worker-only checkpoints for saved offline work. Browser cookies and
//! publish receipts never enter this store. Each changed document writes one
//! journal, rather than rewriting all document contents after every edit.
use super::{Conflict, Document, Entry, EntryKind, Mirror, Model, ProjectUrl, Tree, mirror};
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, fs, io, path::Path};

const VERSION: u8 = 1;
const DOCUMENT_VERSION: u8 = 2;
const TREE_PATH: &str = ".tiptoptyp/overleaf-sync/tree.json";
const MAX_JOURNAL_BYTES: usize = 64 * 1024 * 1024;
const MAX_RESTORE_BYTES: usize = 128 * 1024 * 1024;

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct SavedTree {
    version: u8,
    project_url: String,
    name: String,
    main: Option<String>,
    entries: Vec<Entry>,
    /// Removed remote entries with retained local bytes remain discoverable on
    /// restart. A new server listing will identify their removed-file conflict.
    retained: Vec<Entry>,
}
impl SavedTree {
    fn tree(&self, project: &ProjectUrl) -> Result<Tree, String> {
        if self.version != VERSION || self.project_url != project.url() {
            return Err("The saved Overleaf session belongs to another project or version.".into());
        }
        let mut entries = self.entries.clone();
        entries.extend(self.retained.iter().cloned());
        Tree::new(self.name.clone(), self.main.clone(), entries)
    }
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct SavedDocument {
    version: u8,
    entry: Entry,
    document: Document,
    /// A durable write-ahead record precedes changing the working file. There
    /// is only one flat predecessor, so repeated saves never nest journals.
    pending: Option<PendingWrite>,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct PendingWrite {
    previous: Option<PreviousDocument>,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct PreviousDocument {
    entry: Entry,
    document: Document,
}
impl SavedDocument {
    fn validate(&self, entry: &Entry) -> Result<(), String> {
        if self.version != DOCUMENT_VERSION
            || self.entry.id != entry.id
            || self.entry.kind != EntryKind::Doc
            || entry.kind != EntryKind::Doc
        {
            return Err("The saved Overleaf document has an invalid identity.".into());
        }
        self.entry.relative_path()?;
        validate_document(&self.document)?;
        if let Some(previous) = self
            .pending
            .as_ref()
            .and_then(|pending| pending.previous.as_ref())
        {
            if previous.entry.id != entry.id || previous.entry.kind != EntryKind::Doc {
                return Err("The saved Overleaf transaction has an invalid predecessor.".into());
            }
            previous.entry.relative_path()?;
            validate_document(&previous.document)?;
        }
        Ok(())
    }
    /// A pending source write is committed exactly when the working bytes
    /// match its desired version. Otherwise its predecessor owns the ancestor;
    /// an external change is then imported as a draft by the ordinary loader.
    fn effective(mut self, disk: Option<&[u8]>) -> Option<Self> {
        let Some(pending) = self.pending.take() else {
            return Some(self);
        };
        if disk == Some(self.document.source().as_bytes()) {
            return Some(self);
        }
        match pending.previous {
            Some(previous) => Some(Self {
                version: DOCUMENT_VERSION,
                entry: previous.entry,
                document: previous.document,
                pending: None,
            }),
            // The first authoritative baseline is still valid if an external
            // editor changed a newly cached file before its next checkpoint.
            None if disk.is_some() => Some(self),
            None => None,
        }
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct SavedAsset {
    version: u8,
    entry: Entry,
    digest: String,
}

pub(crate) struct OfflineStore;
#[derive(Debug)]
pub(crate) struct DocumentReceipt {
    pub(crate) path: std::path::PathBuf,
    pub(crate) checkpoint: [u8; 32],
}
impl OfflineStore {
    /// Restore a saved working copy on an explicit open/reconnect worker. Disk
    /// edits are imported as drafts against the saved remote baseline, never as
    /// acknowledgements. Read work and retained contents are bounded together.
    pub(crate) fn load(
        root: &Path,
        project: &ProjectUrl,
        epoch: u64,
    ) -> Result<Option<Model>, String> {
        #[cfg(unix)]
        {
            let directory = mirror::unix::root_directory_existing(root).map_err(string)?;
            let _lock = mirror::unix::sync_lock(&directory).map_err(string)?;
            let Some(saved): Option<SavedTree> = read_json(
                &directory,
                Path::new(TREE_PATH),
                super::MAX_TREE_MANIFEST_BYTES,
            )?
            else {
                return Ok(None);
            };
            let tree = saved.tree(project)?;
            let mut documents = BTreeMap::new();
            let mut remaining = MAX_RESTORE_BYTES;
            for entry in tree
                .entries()
                .iter()
                .filter(|entry| entry.kind == EntryKind::Doc)
            {
                let path = document_path(&entry.id);
                let Some(journal) =
                    read_optional_budget(&directory, &path, MAX_JOURNAL_BYTES, &mut remaining)?
                else {
                    continue;
                };
                let mut checkpoint = source_hash(&journal);
                let saved: SavedDocument = parse_json(&journal)?;
                let Some(mut saved) = resolve_document(&directory, entry, saved, &mut remaining)?
                else {
                    continue;
                };
                let renamed_from = (saved.entry.path != entry.path).then(|| saved.entry.clone());
                let disk = read_optional_budget(
                    &directory,
                    &entry.relative_path()?,
                    super::MAX_SOURCE_BYTES,
                    &mut remaining,
                )?;
                let disk = match disk {
                    Some(bytes) => Some(String::from_utf8(bytes).map_err(|_| {
                        format!(
                            "{} is no longer UTF-8 text. Its local contents were preserved.",
                            entry.path
                        )
                    })?),
                    None if saved.entry.path != entry.path => {
                        // A completed tree checkpoint can precede the renamed
                        // file's download. Its journal still owns the bytes.
                        let source = match read_optional_budget(
                            &directory,
                            &saved.entry.relative_path()?,
                            super::MAX_SOURCE_BYTES,
                            &mut remaining,
                        )? {
                            Some(bytes) => String::from_utf8(bytes).map_err(|_| format!("{} is no longer UTF-8 text. Its local contents were preserved.", saved.entry.path))?,
                            None => saved.document.source().to_owned(),
                        };
                        mirror::unix::write(&directory, &entry.relative_path()?, source.as_bytes())
                            .map_err(string)?;
                        Some(source)
                    }
                    None => {
                        // Keep its journal for recovery without resurrecting a
                        // deliberately deleted file. Other cached documents
                        // remain usable offline; a remote refresh still has to
                        // pass sync_document's missing-file guard for this ID.
                        continue;
                    }
                };
                if let Some(source) = disk {
                    let changed = saved.document.source() != source;
                    if changed {
                        saved.document.edit(source.clone(), false);
                    }
                    if changed || saved.entry != *entry {
                        saved.entry = entry.clone();
                        // Record the exact observed disk bytes before a later
                        // merge is allowed to replace them.
                        let journal = json_bytes(&saved, MAX_JOURNAL_BYTES)?;
                        mirror::unix::write(&directory, &path, &journal).map_err(string)?;
                        checkpoint = source_hash(&journal);
                    }
                    if let Some(previous) = renamed_from {
                        let previous_path = previous.relative_path()?;
                        #[cfg(target_os = "macos")]
                        let _ = mirror::unix::rename_alias(
                            &directory,
                            &previous_path,
                            &entry.relative_path()?,
                            EntryKind::Doc,
                        );
                        if read_optional_budget(
                            &directory,
                            &previous_path,
                            super::MAX_SOURCE_BYTES,
                            &mut remaining,
                        )?
                        .as_deref()
                            == Some(source.as_bytes())
                            && !mirror::unix::same_file(
                                &directory,
                                &previous_path,
                                &entry.relative_path()?,
                            )
                            .map_err(string)?
                        {
                            mirror::unix::remove(&directory, &previous_path).map_err(string)?;
                        }
                    }
                }
                saved.document.checkpoint = Some(checkpoint);
                documents.insert(entry.id.clone(), saved.document);
            }
            Ok(Some(Model::restore_offline(
                epoch, project, tree, documents,
            )))
        }
        #[cfg(not(unix))]
        {
            let _ = (root, project, epoch);
            Err("Secure Overleaf offline storage is unavailable on this platform.".into())
        }
    }
}

impl Mirror {
    /// A new sync record may be bound to an already populated real folder.
    /// Different local bytes have no recorded common ancestor: retain both
    /// versions for explicit review rather than guessing a three-way merge.
    pub(crate) fn prepare_unbased_document(
        &self,
        entry: &Entry,
        document: &mut Document,
    ) -> Result<bool, String> {
        #[cfg(unix)]
        {
            let _lock = mirror::unix::sync_lock(&self.directory).map_err(string)?;
            if document.checkpoint.is_some()
                || document.draft().is_some()
                || document.conflict().is_some()
            {
                return Ok(false);
            }
            if entry.kind != EntryKind::Doc {
                return Err("The Overleaf entry is not a document.".into());
            }
            validate_document(document)?;
            if read_optional(
                &self.directory,
                &document_path(&entry.id),
                MAX_JOURNAL_BYTES,
            )?
            .is_some()
            {
                return Ok(false);
            }
            let Some(bytes) = read_optional(
                &self.directory,
                &entry.relative_path()?,
                super::MAX_SOURCE_BYTES,
            )?
            else {
                return Ok(false);
            };
            let local = String::from_utf8(bytes).map_err(|_| {
                format!(
                    "{} is not UTF-8 text. Its local contents were preserved.",
                    entry.path
                )
            })?;
            if local == document.baseline {
                return Ok(false);
            }
            document.mark_conflict(Conflict {
                base: String::new(),
                base_known: false,
                remote: document.baseline.clone(),
                local,
                message: "This local file differs from Overleaf. Review both versions before choosing which to keep.".into(),
            }, true);
            Ok(true)
        }
        #[cfg(not(unix))]
        {
            let _ = (entry, document);
            Err("Secure Overleaf offline storage is unavailable on this platform.".into())
        }
    }

    /// Commit a server listing without deleting changed local files. Untracked
    /// generated outputs remain outside this listing and are never uploaded.
    pub(crate) fn sync_tree(&self, project: &ProjectUrl, tree: &Tree) -> Result<(), String> {
        #[cfg(unix)]
        {
            let _lock = mirror::unix::sync_lock(&self.directory).map_err(string)?;
            let previous: Option<SavedTree> = read_json(
                &self.directory,
                Path::new(TREE_PATH),
                super::MAX_TREE_MANIFEST_BYTES,
            )?;
            let previous = match previous {
                Some(previous) => previous.tree(project)?.entries().to_vec(),
                None => Tree::new(
                    String::new(),
                    None,
                    read_json::<Vec<Entry>>(
                        &self.directory,
                        Path::new(".tiptoptyp/overleaf-files.json"),
                        super::MAX_TREE_MANIFEST_BYTES,
                    )?
                    .unwrap_or_default(),
                )?
                .entries()
                .to_vec(),
            };
            // Validate the whole listing and serialized size before effects.
            Tree::new(
                tree.name.clone(),
                tree.main.clone(),
                tree.entries().to_vec(),
            )?;
            let mut retained = Vec::new();
            let mut retire = Vec::new();
            let mut retired_journals = Vec::new();
            let mut attention = Vec::new();
            let mut remaining = MAX_RESTORE_BYTES;
            for entry in previous
                .iter()
                .filter(|entry| entry.kind != EntryKind::Folder)
            {
                if tree
                    .entry(&entry.id)
                    .is_some_and(|new| new.path == entry.path && new.kind == entry.kind)
                {
                    continue;
                }
                let local = read_optional_budget(
                    &self.directory,
                    &entry.relative_path()?,
                    super::MAX_FILE_BYTES,
                    &mut remaining,
                )?;
                let clean = match entry.kind {
                    EntryKind::Doc => {
                        let saved: Option<SavedDocument> = read_json_budget(
                            &self.directory,
                            &document_path(&entry.id),
                            MAX_JOURNAL_BYTES,
                            &mut remaining,
                        )?;
                        let saved = saved
                            .map(|saved| {
                                resolve_document(&self.directory, entry, saved, &mut remaining)
                            })
                            .transpose()?
                            .flatten();
                        if let Some(saved) = saved {
                            if tree.entry(&entry.id).is_some()
                                && local.as_deref() != Some(saved.document.source().as_bytes())
                            {
                                return Err(format!(
                                    "Overleaf renamed {}, which changed or was deleted locally. Reopen the workspace before refreshing; its local contents were preserved.",
                                    entry.path
                                ));
                            }
                            local.is_none()
                                || (!saved.document.is_dirty()
                                    && saved.document.conflict().is_none()
                                    && local.as_deref() == Some(saved.document.baseline.as_bytes()))
                        } else {
                            local.is_none()
                        }
                    }
                    EntryKind::File => {
                        let saved: Option<SavedAsset> = read_json_budget(
                            &self.directory,
                            &asset_path(&entry.id),
                            8192,
                            &mut remaining,
                        )?;
                        match (saved, local.as_ref()) {
                            (Some(saved), Some(local)) => {
                                valid_asset(&saved, entry)?;
                                let clean = saved.digest == digest(local);
                                if !clean && tree.entry(&entry.id).is_some() {
                                    return Err(format!(
                                        "Overleaf renamed {}, which has local asset changes. Keep a separate copy before refreshing; its local contents were preserved.",
                                        entry.path
                                    ));
                                }
                                clean
                            }
                            (_, None) => true,
                            _ => false,
                        }
                    }
                    EntryKind::Folder => unreachable!(),
                };
                if entry.kind == EntryKind::Doc
                    && tree
                        .entry(&entry.id)
                        .is_some_and(|renamed| renamed.kind == EntryKind::Doc)
                {
                    // The old path resolves the write-ahead journal until the
                    // new path and its journal commit. This also applies to a
                    // clean first checkpoint, which has no predecessor journal.
                    // sync_document retires the verified old bytes afterward.
                    continue;
                }
                if clean {
                    retire.push((entry.clone(), local.as_ref().map(|bytes| digest(bytes))));
                    if tree.entry(&entry.id).is_none() {
                        retired_journals.push(match entry.kind {
                            EntryKind::Doc => document_path(&entry.id),
                            EntryKind::File => asset_path(&entry.id),
                            EntryKind::Folder => unreachable!(),
                        });
                    }
                } else if tree
                    .entry(&entry.id)
                    .is_some_and(|new| new.kind != entry.kind)
                {
                    return Err(format!(
                        "Overleaf changed the file type of {}, which has local edits. Keep a separate local copy before refreshing.",
                        entry.path
                    ));
                } else if tree.entry(&entry.id).is_none() {
                    if tree.id_for_path(&entry.path).is_some() {
                        return Err(format!(
                            "Overleaf replaced {}, which has local edits. Keep a separate local copy before refreshing.",
                            entry.path
                        ));
                    }
                    retained.push(entry.clone());
                    if attention.len() < 8 {
                        attention.push(entry.path.clone());
                    }
                }
                // Renamed IDs carry their journal and draft to the new path.
                // Keep the old bytes until sync_document commits that rename.
            }
            let mut saved = SavedTree {
                version: VERSION,
                project_url: project.url().to_owned(),
                name: tree.name.clone(),
                main: tree.main.clone(),
                entries: tree.entries().to_vec(),
                retained,
            };
            saved.tree(project)?;
            let bytes = json_bytes(&saved, super::MAX_TREE_MANIFEST_BYTES)?;
            let listing = json_bytes(&saved.entries, super::MAX_TREE_MANIFEST_BYTES)?;
            // The checkpoint retains source journals across crashes even if a
            // later file rename/download has not completed yet.
            mirror::unix::write(&self.directory, Path::new(TREE_PATH), &bytes).map_err(string)?;
            #[cfg(target_os = "macos")]
            for entry in previous
                .iter()
                .filter(|entry| entry.kind == EntryKind::Folder)
            {
                if let Some(current) = tree.entry(&entry.id)
                    && current.kind == EntryKind::Folder
                    && current.path != entry.path
                {
                    // A same-inode directory alias keeps every old journal
                    // path resolvable, including untracked local children.
                    mirror::unix::rename_alias(
                        &self.directory,
                        &entry.relative_path()?,
                        &current.relative_path()?,
                        EntryKind::Folder,
                    )
                    .map_err(string)?;
                }
            }
            let mut verification_remaining = MAX_RESTORE_BYTES;
            for (entry, expected) in retire {
                let path = entry.relative_path()?;
                let current = read_optional_budget(
                    &self.directory,
                    &path,
                    super::MAX_FILE_BYTES,
                    &mut verification_remaining,
                )?;
                if current.as_ref().map(|bytes| digest(bytes)) != expected {
                    // A large tree preflight may span multiple file saves.
                    // Recheck just before retirement, rather than deleting a
                    // newer local edit based on the earlier observation.
                    if tree.entry(&entry.id).is_none() && tree.id_for_path(&entry.path).is_none() {
                        saved.retained.push(entry.clone());
                        saved.tree(project)?;
                        write_json(
                            &self.directory,
                            Path::new(TREE_PATH),
                            &saved,
                            super::MAX_TREE_MANIFEST_BYTES,
                        )?;
                    }
                    return Err(format!(
                        "{} changed locally while the Overleaf listing was updated. Its local contents were preserved; reopen the workspace before refreshing.",
                        entry.path
                    ));
                }
                if entry.kind == EntryKind::File
                    && let Some(current) = tree.entry(&entry.id)
                    && current.kind == EntryKind::File
                {
                    match mirror::unix::same_file(&self.directory, &path, &current.relative_path()?)
                    {
                        Ok(true) => {
                            #[cfg(target_os = "macos")]
                            mirror::unix::rename_alias(
                                &self.directory,
                                &path,
                                &current.relative_path()?,
                                EntryKind::File,
                            )
                            .map_err(string)?;
                            continue;
                        }
                        Ok(false) => {}
                        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                        Err(error) => return Err(string(error)),
                    }
                }
                mirror::unix::remove(&self.directory, &path).map_err(string)?;
            }
            for path in retired_journals {
                mirror::unix::remove(&self.directory, &path).map_err(string)?;
            }
            let mut retired_folders: Vec<_> = previous
                .iter()
                .filter(|entry| {
                    entry.kind == EntryKind::Folder
                        && tree
                            .id_for_path(&entry.path)
                            .and_then(|id| tree.entry(id))
                            .is_none_or(|new| new.kind != EntryKind::Folder)
                })
                .collect();
            retired_folders.sort_unstable_by_key(|entry| std::cmp::Reverse(entry.path.len()));
            for entry in retired_folders {
                if let Some(current) = tree.entry(&entry.id)
                    && current.kind == EntryKind::Folder
                    && matches!(
                        mirror::unix::same_directory(
                            &self.directory,
                            &entry.relative_path()?,
                            &current.relative_path()?,
                        ),
                        Ok(true)
                    )
                {
                    continue;
                }
                mirror::unix::remove_empty_directory(&self.directory, &entry.relative_path()?)
                    .map_err(string)?;
            }
            for entry in tree.entries() {
                let path = entry.relative_path()?;
                let directory = if entry.kind == EntryKind::Folder {
                    Some(path.as_path())
                } else {
                    path.parent().filter(|path| !path.as_os_str().is_empty())
                };
                if let Some(directory) = directory {
                    mirror::unix::directory(&self.directory, directory, true).map_err(string)?;
                }
            }
            mirror::unix::write(
                &self.directory,
                Path::new(".tiptoptyp/overleaf-files.json"),
                &listing,
            )
            .map_err(string)?;
            if !attention.is_empty() {
                return Err(format!(
                    "Overleaf removed files with local edits. Their local copies are retained: {}. Move these copies to a separate location before refreshing.",
                    attention.join(", ")
                ));
            }
            Ok(())
        }
        #[cfg(not(unix))]
        {
            let _ = (project, tree);
            Err("Secure Overleaf offline storage is unavailable on this platform.".into())
        }
    }

    /// Persist the saved local working version and its independent remote
    /// ancestor. An unrelated disk edit/deletion is never overwritten. This
    /// worker operation does not publish or confirm a save to Overleaf.
    pub(crate) fn sync_document(
        &self,
        entry: &Entry,
        document: &Document,
    ) -> Result<DocumentReceipt, String> {
        self.sync_document_with(entry, document, || {})
    }

    fn sync_document_with(
        &self,
        entry: &Entry,
        document: &Document,
        before_journal: impl FnOnce(),
    ) -> Result<DocumentReceipt, String> {
        #[cfg(unix)]
        {
            let _lock = mirror::unix::sync_lock(&self.directory).map_err(string)?;
            if entry.kind != EntryKind::Doc {
                return Err("The Overleaf entry is not a document.".into());
            }
            validate_document(document)?;
            let path = entry.relative_path()?;
            let journal_path = document_path(&entry.id);
            let previous_journal =
                read_optional(&self.directory, &journal_path, MAX_JOURNAL_BYTES)?;
            let fresh = match (&previous_journal, document.checkpoint) {
                (Some(journal), Some(expected)) => source_hash(journal) == expected,
                (None, None) => true,
                _ => false,
            };
            if !fresh {
                return Err(format!(
                    "{} was updated in another window. Its saved source and sync history were preserved; reopen the workspace before refreshing.",
                    entry.path
                ));
            }
            let previous: Option<SavedDocument> = previous_journal
                .as_ref()
                .map(|journal| parse_json(journal))
                .transpose()?;
            let mut remaining = MAX_RESTORE_BYTES;
            let previous = previous
                .map(|previous| resolve_document(&self.directory, entry, previous, &mut remaining))
                .transpose()?
                .flatten();
            let current = read_optional(&self.directory, &path, super::MAX_SOURCE_BYTES)?;
            let desired = document.source().as_bytes();
            let expected = previous
                .as_ref()
                .map(|saved| saved.document.source().as_bytes());
            let allowed = match current.as_deref() {
                Some(current) => current == desired || expected == Some(current),
                None => match &previous {
                    None => previous_journal.is_none(),
                    Some(previous) if previous.entry.path != entry.path => {
                        let old = read_optional(
                            &self.directory,
                            &previous.entry.relative_path()?,
                            super::MAX_SOURCE_BYTES,
                        )?;
                        old.as_deref().map_or(
                            !previous.document.is_dirty() && previous.document.conflict().is_none(),
                            |old| old == previous.document.source().as_bytes(),
                        )
                    }
                    _ => false,
                },
            };
            if !allowed {
                return Err(format!(
                    "{} changed or was deleted locally during sync. Its local contents were preserved; reopen the workspace before refreshing.",
                    entry.path
                ));
            }
            let saved = SavedDocument {
                version: DOCUMENT_VERSION,
                entry: entry.clone(),
                document: document.clone(),
                pending: Some(PendingWrite {
                    previous: previous.as_ref().map(|previous| PreviousDocument {
                        entry: previous.entry.clone(),
                        document: previous.document.clone(),
                    }),
                }),
            };
            let journal = json_bytes(&saved, MAX_JOURNAL_BYTES)?;
            // Commit the ancestor and desired version before touching source.
            // A failed journal write cannot change the user's file. If source
            // writing fails or the app stops next, the pending record selects
            // its predecessor unless the desired bytes actually reached disk.
            before_journal();
            mirror::unix::write(&self.directory, &journal_path, &journal).map_err(string)?;
            if current.as_deref() != Some(desired) {
                if read_optional(&self.directory, &path, super::MAX_SOURCE_BYTES)? != current {
                    return Err(format!(
                        "{} changed locally while its save was prepared. Its local contents were preserved.",
                        entry.path
                    ));
                }
                mirror::unix::write(&self.directory, &path, desired).map_err(string)?;
            }
            // The write-ahead record plus exact working bytes is a complete
            // durable commit. A second, fallible journal write is unnecessary.
            if let Some(previous) = previous
                && previous.entry.path != entry.path
            {
                let old_path = previous.entry.relative_path()?;
                #[cfg(target_os = "macos")]
                let _ =
                    mirror::unix::rename_alias(&self.directory, &old_path, &path, EntryKind::Doc);
                if matches!(
                    read_optional(&self.directory, &old_path, super::MAX_SOURCE_BYTES),
                    Ok(Some(bytes)) if bytes == previous.document.source().as_bytes()
                ) && matches!(
                    mirror::unix::same_file(&self.directory, &old_path, &path),
                    Ok(false)
                ) {
                    // Both new source and journal are already committed. An
                    // optional stale-path cleanup cannot invalidate that save.
                    let _ = mirror::unix::remove(&self.directory, &old_path);
                }
            }
            Ok(DocumentReceipt {
                path: self.root.join(path),
                checkpoint: source_hash(&journal),
            })
        }
        #[cfg(not(unix))]
        {
            let _ = (entry, document, before_journal);
            Err("Secure Overleaf offline storage is unavailable on this platform.".into())
        }
    }

    pub(crate) fn sync_file(
        &self,
        entry: &Entry,
        encoded: &str,
    ) -> Result<std::path::PathBuf, String> {
        #[cfg(unix)]
        {
            let _lock = mirror::unix::sync_lock(&self.directory).map_err(string)?;
            if entry.kind != EntryKind::File {
                return Err("The Overleaf entry is not an asset.".into());
            }
            let path = entry.relative_path()?;
            let bytes = mirror::decode_base64(encoded)?;
            let digest = digest(&bytes);
            let journal_path = asset_path(&entry.id);
            let previous: Option<SavedAsset> = read_json(&self.directory, &journal_path, 8192)?;
            if let Some(previous) = &previous {
                valid_asset(previous, entry)?;
            }
            let current = read_optional(&self.directory, &path, super::MAX_FILE_BYTES)?;
            let allowed = match current.as_ref() {
                Some(current) => {
                    let current_digest = self::digest(current);
                    current_digest == digest
                        || previous
                            .as_ref()
                            .is_some_and(|saved| saved.digest == current_digest)
                }
                None => previous
                    .as_ref()
                    .is_none_or(|saved| saved.entry.path != entry.path),
            };
            if !allowed {
                return Err(format!(
                    "{} has local asset changes. Keep a separate copy before refreshing; its local contents were preserved.",
                    entry.path
                ));
            }
            let saved = SavedAsset {
                version: VERSION,
                entry: entry.clone(),
                digest,
            };
            let journal = json_bytes(&saved, 8192)?;
            if current.as_deref() != Some(bytes.as_slice()) {
                mirror::unix::write(&self.directory, &path, &bytes).map_err(string)?;
            }
            mirror::unix::write(&self.directory, &journal_path, &journal).map_err(string)?;
            Ok(self.root.join(path))
        }
        #[cfg(not(unix))]
        {
            let _ = (entry, encoded);
            Err("Secure Overleaf offline storage is unavailable on this platform.".into())
        }
    }
}

fn validate_document(document: &Document) -> Result<(), String> {
    let mut sources = vec![document.baseline.as_str()];
    sources.extend(document.draft());
    if let Some(conflict) = document.conflict() {
        sources.extend([
            conflict.base.as_str(),
            conflict.remote.as_str(),
            conflict.local.as_str(),
        ]);
        if conflict.message.len() > super::MAX_CONFLICT_MESSAGE_BYTES
            || conflict.local != document.source()
            || conflict.remote != document.baseline
            || (!conflict.base_known && !conflict.base.is_empty())
        {
            return Err("The saved Overleaf conflict is inconsistent.".into());
        }
    }
    if sources
        .iter()
        .any(|source| source.len() > super::MAX_SOURCE_BYTES)
    {
        return Err("The saved Overleaf document is too large.".into());
    }
    Ok(())
}
fn digest(bytes: &[u8]) -> String {
    hex::encode(source_hash(bytes))
}
fn source_hash(bytes: &[u8]) -> [u8; 32] {
    use sha2::{Digest, Sha256};
    Sha256::digest(bytes).into()
}
fn document_path(id: &str) -> std::path::PathBuf {
    format!(
        ".tiptoptyp/overleaf-sync/documents/{}.json",
        digest(id.as_bytes())
    )
    .into()
}
fn asset_path(id: &str) -> std::path::PathBuf {
    format!(
        ".tiptoptyp/overleaf-sync/assets/{}.json",
        digest(id.as_bytes())
    )
    .into()
}
fn valid_asset(saved: &SavedAsset, entry: &Entry) -> Result<bool, String> {
    if saved.version != VERSION
        || saved.entry.id != entry.id
        || saved.entry.kind != EntryKind::File
        || saved.digest.len() != 64
        || !saved.digest.bytes().all(|byte| byte.is_ascii_hexdigit())
    {
        return Err("The saved Overleaf asset has an invalid identity.".into());
    }
    saved.entry.relative_path()?;
    Ok(true)
}
fn json_bytes(value: &impl Serialize, cap: usize) -> Result<Vec<u8>, String> {
    let bytes = serde_json::to_vec(value).map_err(string)?;
    if bytes.len() > cap {
        return Err("The saved Overleaf session exceeds its storage limit.".into());
    }
    Ok(bytes)
}
fn parse_json<T: serde::de::DeserializeOwned>(bytes: &[u8]) -> Result<T, String> {
    serde_json::from_slice(bytes).map_err(|_| {
        "The saved Overleaf sync record is invalid; its local files were preserved.".into()
    })
}
#[cfg(unix)]
fn resolve_document(
    directory: &fs::File,
    entry: &Entry,
    saved: SavedDocument,
    remaining: &mut usize,
) -> Result<Option<SavedDocument>, String> {
    saved.validate(entry)?;
    if saved.pending.is_none() {
        return Ok(Some(saved));
    }
    let disk = read_optional_budget(
        directory,
        &saved.entry.relative_path()?,
        super::MAX_SOURCE_BYTES,
        remaining,
    )?;
    Ok(saved.effective(disk.as_deref()))
}
#[cfg(unix)]
fn write_json(
    directory: &fs::File,
    path: &Path,
    value: &impl Serialize,
    cap: usize,
) -> Result<(), String> {
    mirror::unix::write(directory, path, &json_bytes(value, cap)?).map_err(string)
}
#[cfg(unix)]
fn read_optional(directory: &fs::File, path: &Path, cap: usize) -> Result<Option<Vec<u8>>, String> {
    match mirror::unix::read(directory, path, cap) {
        Ok(bytes) => Ok(Some(bytes)),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(string(error)),
    }
}
#[cfg(unix)]
fn read_optional_budget(
    directory: &fs::File,
    path: &Path,
    cap: usize,
    remaining: &mut usize,
) -> Result<Option<Vec<u8>>, String> {
    let bytes = read_optional(directory, path, cap.min(*remaining))?;
    if let Some(bytes) = &bytes {
        *remaining = remaining
            .checked_sub(bytes.len())
            .ok_or("The saved Overleaf project is too large to restore safely.")?;
    }
    Ok(bytes)
}
#[cfg(unix)]
fn read_json<T: serde::de::DeserializeOwned>(
    directory: &fs::File,
    path: &Path,
    cap: usize,
) -> Result<Option<T>, String> {
    read_optional(directory, path, cap)?
        .map(|bytes| {
            serde_json::from_slice(&bytes).map_err(|_| {
                "The saved Overleaf sync record is invalid; its local files were preserved.".into()
            })
        })
        .transpose()
}
#[cfg(unix)]
fn read_json_budget<T: serde::de::DeserializeOwned>(
    directory: &fs::File,
    path: &Path,
    cap: usize,
    remaining: &mut usize,
) -> Result<Option<T>, String> {
    read_optional_budget(directory, path, cap, remaining)?
        .map(|bytes| {
            serde_json::from_slice(&bytes).map_err(|_| {
                "The saved Overleaf sync record is invalid; its local files were preserved.".into()
            })
        })
        .transpose()
}
fn string(error: impl std::fmt::Display) -> String {
    error.to_string()
}

#[cfg(all(test, unix))]
mod tests;
