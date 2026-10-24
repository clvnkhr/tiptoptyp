use super::*;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Conflict {
    pub(crate) remote: String,
    pub(crate) local: String,
    pub(crate) message: String,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Document {
    /// Last authoritative browser snapshot/acknowledgement, not mirror bytes.
    pub(crate) baseline: String,
    pub(crate) draft: Option<String>,
    pub(crate) conflict: Option<Conflict>,
}
impl Document {
    pub(crate) fn source(&self) -> &str {
        self.draft.as_deref().unwrap_or(&self.baseline)
    }
    pub(crate) fn is_dirty(&self) -> bool {
        self.draft
            .as_ref()
            .is_some_and(|draft| draft != &self.baseline)
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

/// One publisher for a browser session. Unacknowledged edits remain drafts;
/// another edit coalesces behind the in-flight snapshot without changing it.
pub(crate) struct Model {
    epoch: u64,
    project_id: Option<String>,
    tree: Option<Tree>,
    documents: BTreeMap<String, Document>,
    in_flight: Option<InFlight>,
    next_request: u64,
    presence: presence::Presence,
}
impl Model {
    pub(crate) fn new(epoch: u64) -> Self {
        Self {
            epoch,
            project_id: None,
            tree: None,
            documents: BTreeMap::new(),
            in_flight: None,
            next_request: 0,
            presence: Default::default(),
        }
    }
    pub(crate) fn for_project(epoch: u64, project: &ProjectUrl) -> Self {
        Self {
            project_id: Some(project.id().into()),
            ..Self::new(epoch)
        }
    }
    pub(crate) fn epoch(&self) -> u64 {
        self.epoch
    }
    pub(crate) fn tree(&self) -> Option<&Tree> {
        self.tree.as_ref()
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
    pub(crate) fn in_flight(&self) -> Option<&InFlight> {
        self.in_flight.as_ref()
    }
    pub(crate) fn has_unsynced(&self) -> bool {
        self.in_flight.is_some()
            || self
                .documents
                .values()
                .any(|doc| doc.is_dirty() || doc.conflict.is_some())
    }
    /// Reconnection invalidates replies but retains unsent local work.
    pub(crate) fn reconnect(&mut self, epoch: u64) {
        self.epoch = epoch;
        self.in_flight = None;
        self.presence.clear();
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
            .in_flight
            .as_ref()
            .is_some_and(|pending| pending.id == id);
        document.draft = (publishing || source != document.baseline).then_some(source);
        let local = document.source().to_owned();
        if let Some(conflict) = &mut document.conflict {
            conflict.local = local;
        }
        Ok(changed)
    }
    /// Publish exactly the document owning the native SaveRequest. Another
    /// parked draft cannot consume that request or its eventual receipt.
    /// Explicit saves also admit an unchanged source: only a fresh browser
    /// acknowledgement may retire the native document's outstanding receipt.
    pub(crate) fn publish(&mut self, id: &str) -> Option<BrowserCommand> {
        if self.in_flight.is_some() {
            return None;
        }
        let tree = self.tree.as_ref()?;
        let document = self.documents.get(id)?;
        if document.conflict.is_some()
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
        self.in_flight = Some(pending);
        Some(command)
    }
    /// Explicitly discard the retained local draft after conflict review.
    pub(crate) fn resolve_server(&mut self, id: &str) -> bool {
        let Some(document) = self.documents.get_mut(id) else {
            return false;
        };
        if self
            .in_flight
            .as_ref()
            .is_some_and(|pending| pending.id == id)
        {
            return false;
        }
        document.draft = None;
        document.conflict = None;
        true
    }
    /// Release a native editor's explicitly discarded/exported draft when its
    /// last binding ends. A live publish still owns that draft until its reply.
    pub(crate) fn release_native_draft(&mut self, id: &str) -> bool {
        if self
            .in_flight
            .as_ref()
            .is_some_and(|pending| pending.id == id)
        {
            return false;
        }
        let Some(document) = self.documents.get_mut(id) else {
            return true;
        };
        document.draft = None;
        document.conflict = None;
        if self
            .tree
            .as_ref()
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
            BrowserEvent::Project {
                project_id,
                name,
                entries,
                root_doc,
                ..
            } => {
                if self
                    .project_id
                    .as_ref()
                    .is_some_and(|expected| expected != &project_id)
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
                            || self
                                .in_flight
                                .as_ref()
                                .is_some_and(|pending| &pending.id == id))
                    {
                        document.conflict = Some(Conflict { remote: document.baseline.clone(), local: document.source().to_owned(), message: "This document was removed from Overleaf. Your local draft is retained.".into() });
                    }
                }
                // Server churn must not retain every previously downloaded
                // document. Only unsent native work or an active publish owns
                // contents after the corresponding server entry disappears.
                let publishing = self.in_flight.as_ref().map(|pending| pending.id.as_str());
                self.documents.retain(|id, document| {
                    tree.entry(id).is_some()
                        || document.is_dirty()
                        || publishing == Some(id.as_str())
                });
                self.presence.retain_tree(&tree);
                self.project_id = Some(project_id);
                self.tree = Some(tree);
                Ok(ModelChange::Project)
            }
            BrowserEvent::Document {
                id, path, source, ..
            } => {
                self.validate_entry(&id, &path, EntryKind::Doc)?;
                check_source(&source)?;
                let pending = self.in_flight.as_ref().filter(|pending| pending.id == id);
                if !self.documents.contains_key(&id) {
                    self.documents.insert(
                        id.clone(),
                        Document {
                            baseline: source.clone(),
                            draft: None,
                            conflict: None,
                        },
                    );
                    return Ok(ModelChange::Document {
                        id,
                        source,
                        conflict: false,
                    });
                }
                let document = self.documents.get_mut(&id).expect("existing document");
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
                let conflicting = (document.is_dirty() || pending.is_some()) && local != source;
                document.baseline.clone_from(&source);
                if conflicting {
                    document.conflict = Some(Conflict { remote: source.clone(), local, message: "Overleaf changed while this document had local edits. Review both versions before publishing.".into() });
                } else {
                    document.draft = None;
                    document.conflict = None;
                }
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
                let pending = self.in_flight.take().expect("matched pending publish");
                let document = self
                    .documents
                    .get_mut(&id)
                    .ok_or("The acknowledged document is no longer open.")?;
                let local = document.source().to_owned();
                document.baseline.clone_from(&source);
                if source != pending.source {
                    document.conflict = Some(Conflict { remote: source, local, message: "Overleaf acknowledged different contents. Your local draft is retained.".into() });
                    return Ok(ModelChange::Conflict { id });
                }
                if document.draft.as_ref() == Some(&source) {
                    document.draft = None;
                }
                // A previously detected concurrent edit still requires review.
                let dirty = document.is_dirty();
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
                self.in_flight = None;
                let document = self
                    .documents
                    .get_mut(&id)
                    .ok_or("The conflicting document is no longer open.")?;
                let local = document.source().to_owned();
                document.baseline.clone_from(&source);
                document.draft = (local != source).then_some(local.clone());
                document.conflict = Some(Conflict {
                    remote: source,
                    local,
                    message,
                });
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
                Ok(if self.presence.replace(snapshot, self.tree.as_ref()) {
                    ModelChange::Presence
                } else {
                    ModelChange::Ignored
                })
            }
            BrowserEvent::Error { message, .. } => Ok(ModelChange::Error(message)),
            BrowserEvent::Status { state, message, .. } => {
                if !matches!(state.as_str(), "connected" | "saving") {
                    self.presence.clear();
                }
                Ok(ModelChange::Status { state, message })
            }
        }
    }
    fn matches(&self, id: &str, request: u64) -> bool {
        self.in_flight
            .as_ref()
            .is_some_and(|pending| pending.id == id && pending.request == request)
    }
    fn validate_entry(&self, id: &str, path: &str, kind: EntryKind) -> Result<(), String> {
        let entry = self
            .tree
            .as_ref()
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
