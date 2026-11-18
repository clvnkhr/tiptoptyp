use super::*;

mod merge;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Conflict {
    pub(crate) base: String,
    pub(crate) base_known: bool,
    pub(crate) remote: String,
    pub(crate) local: String,
    pub(crate) message: String,
}
/// Exact versions shown in a conflict review. A choice cannot be applied after
/// editing, receiving another remote revision, or starting another session.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ConflictReview {
    pub(crate) id: String,
    pub(crate) base: String,
    pub(crate) local: String,
    pub(crate) remote: String,
    pub(crate) message: String,
    epoch: u64,
    generation: u64,
}
/// The local synchronization state is authoritative. A conflict always owns
/// its comparison; clean/draft states cannot retain stale conflict metadata.
#[derive(Clone, Debug, PartialEq, Eq)]
enum LocalState {
    Synced,
    Draft(String),
    Conflicted {
        draft: Option<String>,
        comparison: Conflict,
    },
}
#[derive(Clone, Debug)]
pub(crate) struct Document {
    /// Last authoritative browser snapshot/acknowledgement, not mirror bytes.
    pub(crate) baseline: String,
    local: LocalState,
    authority: DocumentAuthority,
    /// Exact sync-journal bytes observed by this model. This guards both the
    /// working source and its remote ancestor across independent app windows.
    pub(crate) checkpoint: Option<[u8; 32]>,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum DocumentAuthority {
    Cached,
    Verified,
}
// Equality describes the retained document/checkpoint, not whether this process
// has freshly verified it. Verification never survives journal serialization.
impl PartialEq for Document {
    fn eq(&self, other: &Self) -> bool {
        self.baseline == other.baseline
            && self.local == other.local
            && self.checkpoint == other.checkpoint
    }
}
impl Eq for Document {}
impl Document {
    pub(crate) fn new(baseline: String) -> Self {
        Self {
            baseline,
            local: LocalState::Synced,
            authority: DocumentAuthority::Cached,
            checkpoint: None,
        }
    }
    pub(crate) fn source(&self) -> &str {
        self.draft().unwrap_or(&self.baseline)
    }
    pub(crate) fn draft(&self) -> Option<&str> {
        match &self.local {
            LocalState::Synced => None,
            LocalState::Draft(source) => Some(source),
            LocalState::Conflicted { draft, .. } => draft.as_deref(),
        }
    }
    pub(crate) fn conflict(&self) -> Option<&Conflict> {
        match &self.local {
            LocalState::Conflicted { comparison, .. } => Some(comparison),
            LocalState::Synced | LocalState::Draft(_) => None,
        }
    }
    pub(crate) fn is_dirty(&self) -> bool {
        self.draft().is_some_and(|draft| draft != self.baseline)
    }
    /// An edit during publication keeps explicit undo-to-baseline intent until
    /// the outstanding acknowledgement has retired its exact source snapshot.
    pub(super) fn edit(&mut self, source: String, publishing: bool) {
        let draft = (publishing || source != self.baseline).then_some(source);
        self.local = match std::mem::replace(&mut self.local, LocalState::Synced) {
            LocalState::Conflicted { mut comparison, .. } => {
                comparison.local = draft.as_deref().unwrap_or(&self.baseline).to_owned();
                LocalState::Conflicted { draft, comparison }
            }
            _ => draft.map_or(LocalState::Synced, LocalState::Draft),
        };
    }
    fn resolve(&mut self, source: String) {
        self.local = if source == self.baseline {
            LocalState::Synced
        } else {
            LocalState::Draft(source)
        };
    }
    pub(super) fn mark_conflict(&mut self, comparison: Conflict, preserve_draft: bool) {
        debug_assert_eq!(comparison.remote, self.baseline);
        let draft =
            (preserve_draft || comparison.local != self.baseline).then(|| comparison.local.clone());
        self.local = LocalState::Conflicted { draft, comparison };
    }
    fn clear_local(&mut self) {
        self.local = LocalState::Synced;
    }
    fn acknowledge(&mut self, source: &str) {
        let draft = self
            .draft()
            .filter(|draft| *draft != source)
            .map(str::to_owned);
        self.local = match std::mem::replace(&mut self.local, LocalState::Synced) {
            LocalState::Conflicted { mut comparison, .. } => {
                comparison.remote = source.to_owned();
                comparison.local = draft.as_deref().unwrap_or(source).to_owned();
                LocalState::Conflicted { draft, comparison }
            }
            _ => draft.map_or(LocalState::Synced, LocalState::Draft),
        };
        self.baseline = source.to_owned();
    }
}

/// The durable journal representation stays independent of runtime phases.
/// Serde borrows on writes so state modeling does not copy whole documents.
#[derive(Serialize)]
struct DocumentRecordRef<'a> {
    baseline: &'a str,
    draft: Option<&'a str>,
    conflict: Option<&'a Conflict>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct DocumentRecord {
    baseline: String,
    draft: Option<String>,
    conflict: Option<Conflict>,
}
impl Serialize for Document {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        DocumentRecordRef {
            baseline: &self.baseline,
            draft: self.draft(),
            conflict: self.conflict(),
        }
        .serialize(serializer)
    }
}
impl<'de> Deserialize<'de> for Document {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let record = DocumentRecord::deserialize(deserializer)?;
        let source = record.draft.as_deref().unwrap_or(&record.baseline);
        if record.baseline.len() > MAX_SOURCE_BYTES || source.len() > MAX_SOURCE_BYTES {
            return Err(serde::de::Error::custom(
                "The saved Overleaf document is too large.",
            ));
        }
        if let Some(comparison) = &record.conflict {
            if comparison.local != source
                || comparison.remote != record.baseline
                || (!comparison.base_known && !comparison.base.is_empty())
                || comparison.message.len() > MAX_CONFLICT_MESSAGE_BYTES
            {
                return Err(serde::de::Error::custom(
                    "The saved Overleaf conflict is inconsistent.",
                ));
            }
            if [
                comparison.base.len(),
                comparison.remote.len(),
                comparison.local.len(),
            ]
            .into_iter()
            .any(|len| len > MAX_SOURCE_BYTES)
            {
                return Err(serde::de::Error::custom(
                    "The saved Overleaf document is too large.",
                ));
            }
        }
        let local = match record.conflict {
            Some(comparison) => LocalState::Conflicted {
                draft: record.draft,
                comparison,
            },
            None => record.draft.map_or(LocalState::Synced, LocalState::Draft),
        };
        Ok(Self {
            baseline: record.baseline,
            local,
            authority: DocumentAuthority::Cached,
            checkpoint: None,
        })
    }
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct InFlight {
    pub(crate) id: String,
    pub(crate) request: u64,
    pub(crate) base: String,
    pub(crate) source: String,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum ModelChange {
    Ignored,
    Project,
    Presence,
    Document {
        id: String,
        source: String,
        conflict: bool,
    },
    Acknowledged {
        id: String,
        request: u64,
        source: String,
        dirty: bool,
    },
    Conflict {
        id: String,
    },
    File {
        id: String,
        path: String,
        base64: String,
    },
    Status {
        state: String,
        message: Option<String>,
    },
    Error(String),
}

/// Binding identity and its listing advance together; an unbound model can
/// never contain a ready tree from a different project.
enum ProjectState {
    Unbound,
    AwaitingTree { id: String },
    Ready { id: String, tree: Tree },
}
impl ProjectState {
    fn id(&self) -> Option<&str> {
        match self {
            Self::Unbound => None,
            Self::AwaitingTree { id } | Self::Ready { id, .. } => Some(id),
        }
    }
    fn tree(&self) -> Option<&Tree> {
        match self {
            Self::Ready { tree, .. } => Some(tree),
            Self::Unbound | Self::AwaitingTree { .. } => None,
        }
    }
}
enum Publication {
    Idle,
    AwaitingAcknowledgement(InFlight),
}
impl Publication {
    fn current(&self) -> Option<&InFlight> {
        match self {
            Self::Idle => None,
            Self::AwaitingAcknowledgement(request) => Some(request),
        }
    }
    fn take(&mut self) -> Option<InFlight> {
        match std::mem::replace(self, Self::Idle) {
            Self::Idle => None,
            Self::AwaitingAcknowledgement(request) => Some(request),
        }
    }
}

/// One publisher for a browser session. Unacknowledged edits remain drafts;
/// another edit coalesces behind the in-flight snapshot without changing it.
pub(crate) struct Model {
    epoch: u64,
    project: ProjectState,
    documents: BTreeMap<String, Document>,
    publication: Publication,
    next_request: u64,
    presence: presence::Presence,
    review_generation: Option<u64>,
}
impl Model {
    /// Rebase an explicitly saved local version independently of a newer,
    /// unsaved native draft. Both paths use the same conservative work budget.
    pub(crate) fn reconcile_saved_source(base: &str, local: &str, remote: &str) -> Option<String> {
        if [base, local, remote]
            .iter()
            .any(|source| source.len() > MAX_SOURCE_BYTES)
        {
            return None;
        }
        merge::reconcile(base, local, remote).filter(|source| source.len() <= MAX_SOURCE_BYTES)
    }

    pub(crate) fn new(epoch: u64) -> Self {
        Self {
            epoch,
            project: ProjectState::Unbound,
            documents: BTreeMap::new(),
            publication: Publication::Idle,
            next_request: 0,
            presence: Default::default(),
            review_generation: Some(0),
        }
    }
    pub(crate) fn for_project(epoch: u64, project: &ProjectUrl) -> Self {
        Self {
            project: ProjectState::AwaitingTree {
                id: project.id().into(),
            },
            ..Self::new(epoch)
        }
    }
    /// A disk checkpoint is not a server acknowledgement. A restored document
    /// cannot publish or authorize a conflict choice until a fresh browser
    /// snapshot verifies its identity and contents in the new session.
    pub(super) fn restore_offline(
        epoch: u64,
        project: &ProjectUrl,
        tree: Tree,
        mut documents: BTreeMap<String, Document>,
    ) -> Self {
        documents
            .values_mut()
            .for_each(|document| document.authority = DocumentAuthority::Cached);
        Self {
            project: ProjectState::Ready {
                id: project.id().into(),
                tree,
            },
            documents,
            ..Self::new(epoch)
        }
    }
    pub(crate) fn epoch(&self) -> u64 {
        self.epoch
    }
    pub(crate) fn tree(&self) -> Option<&Tree> {
        self.project.tree()
    }
    pub(crate) fn presence(&self) -> &presence::Presence {
        &self.presence
    }
    pub(crate) fn clear_presence(&mut self) {
        self.presence.clear();
    }
    pub(crate) fn document(&self, id: &str) -> Option<&Document> {
        self.documents.get(id)
    }
    #[cfg(test)]
    pub(crate) fn record_checkpoint(&mut self, id: &str, checkpoint: [u8; 32]) -> bool {
        let Some(document) = self.documents.get_mut(id) else {
            return false;
        };
        document.checkpoint = Some(checkpoint);
        true
    }
    /// Observe a physical write without granting its source fresh authority.
    /// An older/repeated receipt cannot roll back a later observed journal.
    pub(crate) fn advance_checkpoint(
        &mut self,
        id: &str,
        previous: Option<[u8; 32]>,
        written: [u8; 32],
    ) -> bool {
        let Some(document) = self.documents.get_mut(id) else {
            return false;
        };
        if document.checkpoint != previous {
            return false;
        }
        document.checkpoint = Some(written);
        true
    }
    /// Adopt a worker-discovered existing local file only while the freshly
    /// read remote document is still clean. Verified browser identity remains
    /// intact, but publication waits for an explicit conflict choice and Save.
    pub(crate) fn import_local_conflict(
        &mut self,
        id: &str,
        expected_baseline: &str,
        local: &Document,
    ) -> Result<bool, String> {
        let Some(current) = self.documents.get(id) else {
            return Ok(false);
        };
        if current.baseline != expected_baseline
            || current.draft().is_some()
            || current.conflict().is_some()
            || self
                .publication
                .current()
                .is_some_and(|pending| pending.id == id)
        {
            return Ok(false);
        }
        let conflict = local
            .conflict()
            .ok_or("The existing local file has no conflict to review.")?;
        if conflict.base_known
            || !conflict.base.is_empty()
            || local.baseline != expected_baseline
            || conflict.remote != expected_baseline
            || conflict.local != local.source()
            || conflict.message.len() > MAX_CONFLICT_MESSAGE_BYTES
        {
            return Err(
                "The existing local comparison no longer matches the Overleaf version.".into(),
            );
        }
        for source in [
            local.baseline.as_str(),
            local.source(),
            conflict.base.as_str(),
            conflict.remote.as_str(),
            conflict.local.as_str(),
        ] {
            check_source(source)?;
        }
        let current = self.documents.get_mut(id).expect("validated document");
        current.local.clone_from(&local.local);
        current.checkpoint = local.checkpoint.or(current.checkpoint);
        self.invalidate_reviews();
        Ok(true)
    }
    pub(crate) fn conflict_ids(&self) -> impl Iterator<Item = &str> {
        self.documents
            .iter()
            .filter(|(_, document)| document.conflict().is_some())
            .map(|(id, _)| id.as_str())
    }
    pub(crate) fn unsynced_documents(&self) -> impl Iterator<Item = (&str, &Document)> {
        self.documents
            .iter()
            .filter(|(id, document)| {
                document.is_dirty()
                    || document.conflict().is_some()
                    || self
                        .publication
                        .current()
                        .is_some_and(|pending| pending.id == id.as_str())
            })
            .map(|(id, document)| (id.as_str(), document))
    }
    #[cfg(test)]
    pub(crate) fn in_flight(&self) -> Option<&InFlight> {
        self.publication.current()
    }
    pub(crate) fn has_unsynced(&self) -> bool {
        self.publication.current().is_some()
            || self
                .documents
                .values()
                .any(|doc| doc.is_dirty() || doc.conflict().is_some())
    }
    /// Reconnection invalidates replies but retains unsent local work.
    pub(crate) fn reconnect(&mut self, epoch: u64) {
        self.epoch = epoch;
        self.publication = Publication::Idle;
        self.presence.clear();
        self.documents
            .values_mut()
            .for_each(|document| document.authority = DocumentAuthority::Cached);
        self.invalidate_reviews();
    }
    pub(crate) fn set_draft(&mut self, id: &str, source: String) -> Result<bool, String> {
        check_source(&source)?;
        let document = self
            .documents
            .get_mut(id)
            .ok_or("Wait for the Overleaf document to load before editing it.")?;
        let changed = document.source() != source;
        // Keep undo-to-baseline while a different snapshot is in flight: it
        // becomes a real follow-up edit after that acknowledgement arrives.
        let publishing = self
            .publication
            .current()
            .is_some_and(|pending| pending.id == id);
        document.edit(source, publishing);
        if changed {
            self.invalidate_reviews();
        }
        Ok(changed)
    }
    /// Publish exactly the document owning the native SaveRequest. Another
    /// parked draft cannot consume that request or its eventual receipt.
    /// Explicit saves also admit an unchanged source: only a fresh browser
    /// acknowledgement may retire the native document's outstanding receipt.
    pub(crate) fn publish(&mut self, id: &str) -> Option<BrowserCommand> {
        if self.publication.current().is_some() {
            return None;
        }
        let tree = self.project.tree()?;
        let document = self.documents.get(id)?;
        if document.conflict().is_some()
            || !self
                .documents
                .get(id)
                .is_some_and(|document| document.authority == DocumentAuthority::Verified)
            || !tree
                .entry(id)
                .is_some_and(|entry| entry.kind == EntryKind::Doc)
        {
            return None;
        }
        self.next_request = self.next_request.checked_add(1)?;
        let pending = InFlight {
            id: id.to_owned(),
            request: self.next_request,
            base: document.baseline.clone(),
            source: document.source().to_owned(),
        };
        let command = BrowserCommand::Edit {
            id: pending.id.clone(),
            base: pending.base.clone(),
            source: pending.source.clone(),
            request: pending.request,
            epoch: self.epoch,
        };
        self.publication = Publication::AwaitingAcknowledgement(pending);
        Some(command)
    }
    pub(crate) fn conflict_review(&self, id: &str) -> Option<ConflictReview> {
        if !self
            .documents
            .get(id)
            .is_some_and(|document| document.authority == DocumentAuthority::Verified)
            || !self
                .project
                .tree()?
                .entry(id)
                .is_some_and(|entry| entry.kind == EntryKind::Doc)
        {
            return None;
        }
        let conflict = self.documents.get(id)?.conflict()?;
        Some(ConflictReview {
            id: id.into(),
            base: conflict.base.clone(),
            local: conflict.local.clone(),
            remote: conflict.remote.clone(),
            message: conflict.message.clone(),
            epoch: self.epoch,
            generation: self.review_generation?,
        })
    }
    /// Cheap frame-time validity check; source equality is checked again when
    /// applying a choice, rather than cloning all three versions while painting.
    pub(crate) fn conflict_review_current(&self, review: &ConflictReview) -> bool {
        self.epoch == review.epoch
            && self.review_generation == Some(review.generation)
            && self
                .documents
                .get(&review.id)
                .is_some_and(|document| document.authority == DocumentAuthority::Verified)
            && self.project.tree().is_some_and(|tree| {
                tree.entry(&review.id)
                    .is_some_and(|entry| entry.kind == EntryKind::Doc)
            })
            && self
                .documents
                .get(&review.id)
                .is_some_and(|document| document.conflict().is_some())
    }
    /// Accept only the exact versions reviewed. This creates a draft; publishing
    /// it still needs the ordinary explicit save and durable acknowledgement.
    pub(crate) fn resolve_conflict(
        &mut self,
        review: &ConflictReview,
        source: String,
    ) -> Result<(), String> {
        check_source(&source)?;
        if !self.conflict_review_current(review)
            || self.conflict_review(&review.id).as_ref() != Some(review)
        {
            return Err(
                "The Overleaf versions changed. Reopen the comparison before choosing a version."
                    .into(),
            );
        }
        if self
            .publication
            .current()
            .is_some_and(|pending| pending.id == review.id)
        {
            return Err(
                "Wait for the outstanding Overleaf save before resolving this conflict.".into(),
            );
        }
        let document = self
            .documents
            .get_mut(&review.id)
            .expect("reviewed document");
        document.resolve(source);
        self.invalidate_reviews();
        Ok(())
    }
    /// Release a native editor's explicitly discarded/exported draft when its
    /// last binding ends. A live publish still owns that draft until its reply.
    pub(crate) fn release_native_draft(&mut self, id: &str) -> bool {
        if self
            .publication
            .current()
            .is_some_and(|pending| pending.id == id)
        {
            return false;
        }
        let Some(document) = self.documents.get_mut(id) else {
            return true;
        };
        document.clear_local();
        self.invalidate_reviews();
        if self
            .project
            .tree()
            .is_none_or(|tree| tree.entry(id).is_none())
        {
            self.documents.remove(id);
        }
        true
    }

    pub(crate) fn apply(&mut self, event: BrowserEvent) -> Result<ModelChange, String> {
        if event.epoch() != self.epoch {
            return Ok(ModelChange::Ignored);
        }
        match event {
            BrowserEvent::FileAccess { .. } | BrowserEvent::FileAction { .. } => {
                Ok(ModelChange::Ignored)
            }
            BrowserEvent::Project {
                project_id,
                name,
                entries,
                root_doc,
                ..
            } => {
                if self
                    .project
                    .id()
                    .is_some_and(|expected| expected != project_id)
                {
                    return Err("The browser reported a different Overleaf project.".into());
                }
                if project_id.len() != 24
                    || !project_id.bytes().all(|byte| byte.is_ascii_hexdigit())
                {
                    return Err("Invalid Overleaf project ID.".into());
                }
                let tree = Tree::new(name, root_doc, entries)?;
                for (id, document) in &mut self.documents {
                    if tree.entry(id).is_none()
                        && (document.is_dirty()
                            || document.conflict().is_some()
                            || self
                                .publication
                                .current()
                                .is_some_and(|pending| &pending.id == id))
                    {
                        document.mark_conflict(Conflict { base: document.conflict().map_or_else(|| document.baseline.clone(), |conflict| conflict.base.clone()), base_known: document.conflict().is_none_or(|conflict| conflict.base_known), remote: document.baseline.clone(), local: document.source().to_owned(), message: "This document was removed from Overleaf. Your local draft is retained.".into() }, document.draft().is_some());
                    }
                }
                // Server churn must not retain every previously downloaded
                // document. Only unsent native work or an active publish owns
                // contents after the corresponding server entry disappears.
                let publishing = self
                    .publication
                    .current()
                    .map(|pending| pending.id.as_str());
                self.documents.retain(|id, document| {
                    tree.entry(id).is_some()
                        || document.is_dirty()
                        || document.conflict().is_some()
                        || publishing == Some(id.as_str())
                });
                for (id, document) in &mut self.documents {
                    if tree.entry(id).is_none() {
                        document.authority = DocumentAuthority::Cached;
                    }
                }
                self.invalidate_reviews();
                self.presence.retain_tree(&tree);
                self.project = ProjectState::Ready {
                    id: project_id,
                    tree,
                };
                Ok(ModelChange::Project)
            }
            BrowserEvent::Document {
                id, path, source, ..
            } => {
                self.validate_entry(&id, &path, EntryKind::Doc)?;
                check_source(&source)?;
                let pending = self
                    .publication
                    .current()
                    .filter(|pending| pending.id == id);
                if !self.documents.contains_key(&id) {
                    self.documents.insert(
                        id.clone(),
                        Document {
                            authority: DocumentAuthority::Verified,
                            ..Document::new(source.clone())
                        },
                    );
                    self.invalidate_reviews();
                    return Ok(ModelChange::Document {
                        id,
                        source,
                        conflict: false,
                    });
                }
                let document = self.documents.get_mut(&id).expect("existing document");
                document.authority = DocumentAuthority::Verified;
                if let Some(pending) = pending {
                    // Store changes preceding doc:saved are not durable ACKs.
                    if source == pending.source || source == pending.base {
                        return Ok(ModelChange::Ignored);
                    }
                }
                if source == document.baseline {
                    return Ok(ModelChange::Ignored);
                }
                let local = document.source().to_owned();
                let base = document.conflict().map_or_else(
                    || document.baseline.clone(),
                    |conflict| conflict.base.clone(),
                );
                let base_known = document
                    .conflict()
                    .is_none_or(|conflict| conflict.base_known);
                let merged = if pending.is_some() || !base_known {
                    None
                } else {
                    merge::reconcile(&base, &local, &source)
                        .filter(|merged| merged.len() <= MAX_SOURCE_BYTES)
                };
                let conflicting = merged.is_none();
                document.baseline.clone_from(&source);
                if let Some(merged) = merged {
                    document.resolve(merged);
                } else {
                    // Even a remote snapshot matching the newer local draft
                    // cannot consume it while an older publish owns the ACK.
                    document.mark_conflict(Conflict {
                        base,
                        base_known,
                        remote: source.clone(),
                        local,
                        message: "Overleaf changed while this document had local edits. Review both versions before publishing.".into(),
                    }, pending.is_some());
                }
                self.invalidate_reviews();
                Ok(ModelChange::Document {
                    id,
                    source,
                    conflict: conflicting,
                })
            }
            BrowserEvent::Ack {
                id,
                request,
                source,
                ..
            } => {
                check_source(&source)?;
                if !self.matches(&id, request) {
                    return Ok(ModelChange::Ignored);
                }
                let pending = self.publication.take().expect("matched pending publish");
                let document = self
                    .documents
                    .get_mut(&id)
                    .ok_or("The acknowledged document is no longer open.")?;
                let local = document.source().to_owned();
                let base = document
                    .conflict()
                    .map_or_else(|| pending.base.clone(), |conflict| conflict.base.clone());
                let base_known = document
                    .conflict()
                    .is_none_or(|conflict| conflict.base_known);
                document.baseline.clone_from(&source);
                document.authority = DocumentAuthority::Verified;
                if source != pending.source {
                    document.mark_conflict(Conflict { base, base_known, remote: source, local, message: "Overleaf acknowledged different contents. Your local draft is retained.".into() }, false);
                    self.invalidate_reviews();
                    return Ok(ModelChange::Conflict { id });
                }
                // A prior conflict remains a review obligation even after a
                // matching ACK; its comparison follows the acknowledged bytes.
                document.acknowledge(&source);
                let dirty = document.is_dirty();
                self.invalidate_reviews();
                Ok(ModelChange::Acknowledged {
                    id,
                    request,
                    source,
                    dirty,
                })
            }
            BrowserEvent::Conflict {
                id,
                request,
                source,
                message,
                ..
            } => {
                check_source(&source)?;
                if !self.matches(&id, request) {
                    return Ok(ModelChange::Ignored);
                }
                // Accepted browser state must remain writable by the offline
                // checkpoint store. Validate before retiring the pending save.
                if message.len() > MAX_CONFLICT_MESSAGE_BYTES {
                    return Err("The Overleaf conflict message is too long.".into());
                }
                let pending = self.publication.take().expect("matched pending publish");
                let document = self
                    .documents
                    .get_mut(&id)
                    .ok_or("The conflicting document is no longer open.")?;
                let local = document.source().to_owned();
                let base = document
                    .conflict()
                    .map_or_else(|| pending.base.clone(), |conflict| conflict.base.clone());
                let base_known = document
                    .conflict()
                    .is_none_or(|conflict| conflict.base_known);
                document.baseline.clone_from(&source);
                document.mark_conflict(
                    Conflict {
                        base,
                        base_known,
                        remote: source,
                        local,
                        message,
                    },
                    false,
                );
                document.authority = DocumentAuthority::Verified;
                self.invalidate_reviews();
                Ok(ModelChange::Conflict { id })
            }
            BrowserEvent::File {
                id, path, base64, ..
            } => {
                self.validate_entry(&id, &path, EntryKind::File)?;
                if base64.len() > MAX_FILE_BYTES.div_ceil(3) * 4 {
                    return Err("The Overleaf asset is too large.".into());
                }
                Ok(ModelChange::File { id, path, base64 })
            }
            BrowserEvent::Presence { snapshot, .. } => {
                Ok(if self.presence.replace(snapshot, self.project.tree()) {
                    ModelChange::Presence
                } else {
                    ModelChange::Ignored
                })
            }
            BrowserEvent::Error { message, .. } => Ok(ModelChange::Error(message)),
            BrowserEvent::Status { state, message, .. } => {
                if !matches!(state.as_str(), "connected" | "saving") {
                    self.presence.clear();
                    self.documents
                        .values_mut()
                        .for_each(|document| document.authority = DocumentAuthority::Cached);
                    self.invalidate_reviews();
                }
                Ok(ModelChange::Status { state, message })
            }
        }
    }
    fn invalidate_reviews(&mut self) {
        // Exhaustion permanently disables review tokens instead of reusing one.
        self.review_generation = self
            .review_generation
            .and_then(|value| value.checked_add(1));
    }
    fn matches(&self, id: &str, request: u64) -> bool {
        self.publication
            .current()
            .is_some_and(|pending| pending.id == id && pending.request == request)
    }
    fn validate_entry(&self, id: &str, path: &str, kind: EntryKind) -> Result<(), String> {
        let entry = self
            .project
            .tree()
            .and_then(|tree| tree.entry(id))
            .ok_or("The Overleaf file is absent from the current project listing.")?;
        if entry.path != path || entry.kind != kind {
            return Err("The Overleaf file identity does not match its project path.".into());
        }
        Ok(())
    }
}
fn check_source(source: &str) -> Result<(), String> {
    if source.len() > MAX_SOURCE_BYTES {
        Err("The Overleaf document is too large to edit.".into())
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod saved_source_tests {
    use super::*;

    #[test]
    fn saved_local_intent_rebases_without_adopting_newer_unsaved_edits() {
        let project =
            ProjectUrl::parse("https://www.overleaf.com/project/0123456789abcdef01234567").unwrap();
        let mut model = Model::for_project(1, &project);
        model
            .apply(BrowserEvent::Project {
                epoch: 1,
                project_id: project.id().into(),
                name: "Paper".into(),
                root_doc: Some("doc".into()),
                entries: vec![Entry {
                    id: "doc".into(),
                    path: "main.tex".into(),
                    kind: EntryKind::Doc,
                }],
            })
            .unwrap();
        let snapshot = |source: &str| BrowserEvent::Document {
            epoch: 1,
            id: "doc".into(),
            path: "main.tex".into(),
            source: source.into(),
        };
        let base = "Opening base.\nMiddle base.\nClosing base.\n";
        let saved_local = "Opening saved.\nMiddle base.\nClosing base.\n";
        let unsaved_local = "Opening saved.\nMiddle unsaved.\nClosing base.\n";
        let remote = "Opening base.\nMiddle base.\nClosing remote.\n";
        model.apply(snapshot(base)).unwrap();
        model.set_draft("doc", unsaved_local.into()).unwrap();

        let saved = Model::reconcile_saved_source(base, saved_local, remote).unwrap();
        model.apply(snapshot(remote)).unwrap();

        assert_eq!(saved, "Opening saved.\nMiddle base.\nClosing remote.\n");
        assert_eq!(
            model.document("doc").unwrap().source(),
            "Opening saved.\nMiddle unsaved.\nClosing remote.\n"
        );
    }

    #[test]
    fn saved_rebase_rejects_oversized_inputs_before_merge_work() {
        let oversized = "x".repeat(MAX_SOURCE_BYTES + 1);
        for (base, local, remote) in [
            (oversized.as_str(), "small", "small"),
            ("small", oversized.as_str(), "small"),
            ("small", "small", oversized.as_str()),
        ] {
            assert!(Model::reconcile_saved_source(base, local, remote).is_none());
        }
    }
}

#[cfg(test)]
mod explicit_state_tests {
    use super::*;

    fn conflict(local: &str) -> Conflict {
        Conflict {
            base: "ancestor".into(),
            base_known: true,
            remote: "base".into(),
            local: local.into(),
            message: "Review both versions".into(),
        }
    }

    #[test]
    fn local_state_preserves_revert_intent_until_its_exact_acknowledgement() {
        let mut document = Document::new("base".into());
        document.edit("base".into(), true);
        assert!(matches!(document.local, LocalState::Draft(_)));
        assert!(!document.is_dirty());
        document.acknowledge("first publication");
        assert_eq!(document.source(), "base");
        assert!(document.is_dirty());
        document.acknowledge("base");
        assert!(matches!(document.local, LocalState::Synced));
    }

    #[test]
    fn conflicts_remain_explicit_review_obligations_when_local_matches_remote() {
        let mut document = Document::new("base".into());
        document.mark_conflict(conflict("local"), false);
        document.edit("base".into(), false);
        assert!(matches!(
            document.local,
            LocalState::Conflicted { draft: None, .. }
        ));
        assert_eq!(document.conflict().unwrap().local, "base");
        document.acknowledge("base");
        assert!(
            document.conflict().is_some(),
            "an ACK cannot choose a conflict resolution"
        );
        document.resolve("base".into());
        assert!(matches!(document.local, LocalState::Synced));
    }

    #[test]
    fn journal_shape_is_stable_and_never_restores_browser_authority() {
        let mut document = Document::new("base".into());
        document.edit("local".into(), false);
        document.authority = DocumentAuthority::Verified;
        let encoded = serde_json::to_value(&document).unwrap();
        assert_eq!(
            encoded,
            serde_json::json!({"baseline":"base","draft":"local","conflict":null})
        );
        let decoded: Document = serde_json::from_value(encoded).unwrap();
        assert_eq!(decoded, document);
        assert_eq!(decoded.authority, DocumentAuthority::Cached);
        document.mark_conflict(conflict("local"), true);
        let encoded = serde_json::to_value(&document).unwrap();
        assert_eq!(
            serde_json::from_value::<Document>(encoded.clone()).unwrap(),
            document
        );
        for field in ["local", "remote"] {
            let mut invalid = encoded.clone();
            invalid["conflict"][field] = "unrelated".into();
            assert!(
                serde_json::from_value::<Document>(invalid).is_err(),
                "{field}"
            );
        }
    }
}
