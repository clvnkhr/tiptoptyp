//! Rebase native editor ownership only after the authoritative working-copy
//! write succeeds. The merged draft remains distinct from that saved baseline.
use super::*;
use crate::overleaf::ConflictReview;
use tiptoptyp::mitex_document::CanonicalSnapshot;

const LOCAL_CHANGED: &str =
    "The local draft changed during reconciliation. Save a local copy before reconnecting.";

pub(super) struct NativeRebase {
    epoch: u64,
    session: u64,
    remote: String,
    source: String,
    targets: Vec<NativeTarget>,
}
struct NativeTarget {
    tab: u64,
    key: DocumentKey,
    source: Option<CanonicalSnapshot>,
}
#[derive(Clone, Copy, PartialEq, Eq)]
enum UpdateMode {
    Preserve,
    Clean,
    Rename,
}

struct PreparedNative {
    tab: u64,
    update: tiptoptyp::mitex_document::ExternalChange,
    path: PathBuf,
    kind: DocumentKind,
    cursor: CCursorRange,
}

impl EditorApp {
    pub(in crate::app) fn stage_overleaf_rebase(
        &mut self,
        id: &str,
        remote: &str,
    ) -> Result<(), String> {
        let Some(connection) = &self.overleaf.connection else {
            return Ok(());
        };
        let Some(document) = connection.model.document(id) else {
            return Ok(());
        };
        if document.conflict().is_some() || document.baseline != remote {
            return Ok(());
        }
        if connection
            .model
            .tree()
            .and_then(|tree| tree.entry(id))
            .is_none()
        {
            return Ok(());
        }
        let source = document.source().to_owned();
        let epoch = connection.model.epoch();
        let session = connection.hydration.session;
        let mut targets = Vec::new();
        let mut error = None;
        for tab in self.overleaf_native_tabs(id) {
            let document = self.document_for_tab(tab).expect("bound native document");
            // A fresh remote snapshot can also confirm that an offline draft
            // already exists remotely. Its native dirty baseline still needs
            // updating, although there is no model draft left to apply.
            if source == remote && !document.is_dirty() {
                continue;
            }
            let snapshot = document.canonical_snapshot();
            if let Err(failure) = &snapshot {
                error = Some(failure.to_string());
            }
            targets.push(NativeTarget {
                tab,
                key: document.key(),
                source: snapshot.ok(),
            });
        }
        if targets.is_empty() {
            self.overleaf.rebases.remove(id);
            return Ok(());
        }
        if let Some(previous) = self.overleaf.rebases.remove(id) {
            // Reconnect and tree churn can refresh the mirror generation, but
            // must not silently recapture a newer, unrelated native edit.
            for target in &mut targets {
                if let Some(old) = previous.targets.iter().find(|old| old.tab == target.tab) {
                    target.key = old.key;
                    target.source.clone_from(&old.source);
                }
            }
        }
        self.overleaf.rebases.insert(
            id.into(),
            NativeRebase {
                epoch,
                session,
                remote: remote.into(),
                source,
                targets,
            },
        );
        error.map_or(Ok(()), Err)
    }

    /// Called after the passive mirror's epoch, entry, source and write receipt
    /// have been checked. No filesystem work or browser publish happens here.
    pub(in crate::app) fn apply_overleaf_rebase(
        &mut self,
        id: &str,
        path: &Path,
        remote: &str,
        context: &egui::Context,
    ) -> Result<bool, String> {
        let Some(intent) = self.overleaf.rebases.get(id) else {
            return Ok(false);
        };
        let Some(connection) = &self.overleaf.connection else {
            return Ok(false);
        };
        let Some(document) = connection.model.document(id) else {
            return Ok(false);
        };
        let expected = connection
            .model
            .tree()
            .and_then(|tree| tree.entry(id))
            .map(|entry| connection.root.join(&entry.path));
        if intent.epoch != connection.model.epoch()
            || intent.session != connection.hydration.session
            || intent.remote != remote
            || expected.as_deref() != Some(path)
            || document.baseline != remote
            || document.source() != intent.source
            || document.conflict().is_some()
        {
            // A newer generation owns any replacement intent. Do not retire it
            // because an older worker finished in the meantime.
            return Ok(false);
        }
        if self
            .overleaf
            .save
            .pending()
            .is_some_and(|save| save.id == id)
            || self.overleaf.save.entity() == Some(id)
            || self
                .local_save
                .pending()
                .is_some_and(|save| intent.targets.iter().any(|target| target.tab == save.tab))
        {
            return Err(
                "Wait for the outstanding save before reconciling Overleaf changes.".into(),
            );
        }
        let mut prepared = Vec::new();
        let bound: BTreeSet<_> = self.overleaf_native_tabs(id).into_iter().collect();
        for target in &intent.targets {
            let Some(native) = self.document_for_tab(target.tab) else {
                continue;
            };
            if !bound.contains(&target.tab) {
                return Err(LOCAL_CHANGED.into());
            }
            let Some(previous) = &target.source else {
                return Err(LOCAL_CHANGED.into());
            };
            let current = native.canonical_snapshot().map_err(|_| LOCAL_CHANGED)?;
            if native.key().owner != target.key.owner || current.source() != previous.source() {
                return Err(LOCAL_CHANGED.into());
            }
            prepared.push(prepare_native(
                target.tab,
                native,
                path,
                remote,
                &intent.source,
                self.overleaf_native_cursor(target.tab, context),
            )?);
        }
        let changed = self.commit_overleaf_native(prepared, context);
        self.overleaf.rebases.remove(id);
        self.overleaf.blocked.remove(id);
        Ok(changed)
    }

    pub(in crate::app) fn apply_overleaf_review(
        &mut self,
        context: &egui::Context,
        review: &ConflictReview,
        source: String,
    ) -> Result<(), String> {
        if self.overleaf.authentication.required() {
            return Err("Sign in to Overleaf before choosing a version.".into());
        }
        if source.len() > crate::overleaf::MAX_SOURCE_BYTES {
            return Err("The Overleaf document is too large to edit.".into());
        }
        let connection = self
            .overleaf
            .connection
            .as_ref()
            .ok_or("Reconnect to Overleaf before choosing a version.")?;
        if !connection.online() || connection.hydration.pending() {
            return Err("Wait for Overleaf to finish syncing before choosing a version.".into());
        }
        if self.overleaf.save.pending().is_some()
            || self.overleaf.save.active()
            || self.document_workflow.save_in_flight()
            || self.local_save.is_running()
        {
            return Err("Wait for the outstanding save before choosing a version.".into());
        }
        if !connection.model.conflict_review_current(review) {
            return Err(
                "The Overleaf versions changed. Reopen the comparison before choosing a version."
                    .into(),
            );
        }
        let entry = connection
            .model
            .tree()
            .and_then(|tree| tree.entry(&review.id))
            .ok_or(
                "This document was removed from Overleaf. Save a local copy to keep your draft.",
            )?
            .clone();
        let path = connection.root.join(&entry.path);
        let tabs = self.overleaf_native_tabs(&review.id);
        if tabs.is_empty() {
            return Err("Open this Overleaf document before choosing a version.".into());
        }
        let mut prepared = Vec::with_capacity(tabs.len());
        for tab in tabs {
            let native = self.document_for_tab(tab).expect("bound native document");
            if native
                .canonical_snapshot()
                .map_err(|_| LOCAL_CHANGED)?
                .source()
                != review.local
            {
                return Err(LOCAL_CHANGED.into());
            }
            prepared.push(prepare_native(
                tab,
                native,
                &path,
                &review.remote,
                &source,
                self.overleaf_native_cursor(tab, context),
            )?);
        }
        // Every displayed/canonical projection has succeeded before the model
        // or any native editor is mutated. Applying a choice never publishes it.
        let use_remote = source == review.remote;
        self.overleaf
            .connection
            .as_mut()
            .unwrap()
            .model
            .resolve_conflict(review, source)?;
        if use_remote {
            self.overleaf.offline.saved_sources.remove(&review.id);
        }
        let changed = self.commit_overleaf_native(prepared, context);
        self.overleaf.rebases.remove(&review.id);
        self.overleaf.blocked.remove(&review.id);
        if self.overleaf.blocked.is_empty()
            && self.overleaf.error.as_deref() == Some(CONFLICT_ERROR)
        {
            self.overleaf.error = None;
        }
        self.overleaf.queue(MirrorWork {
            checkpoint: false,
            entry,
            data: MirrorData::Document(review.remote.clone()),
            save: None,
        });
        self.schedule_project_index();
        if changed || self.designated_preview_path().is_some() {
            self.schedule_compile_now();
        }
        context.request_repaint();
        Ok(())
    }

    pub(in crate::app) fn apply_overleaf_clean_update(
        &mut self,
        tab: u64,
        path: &Path,
        source: &str,
        context: &egui::Context,
    ) -> Result<bool, String> {
        let native = self
            .document_for_tab(tab)
            .ok_or("The Overleaf editor was closed.")?;
        let prepared = prepare_native_update(
            tab,
            native,
            path,
            source,
            source,
            self.overleaf_native_cursor(tab, context),
            UpdateMode::Clean,
        )?;
        Ok(self.commit_overleaf_native(vec![prepared], context))
    }

    pub(in crate::app) fn rename_overleaf_native_tab(
        &mut self,
        tab: u64,
        path: &Path,
        context: &egui::Context,
    ) -> Result<bool, String> {
        let native = self
            .document_for_tab(tab)
            .ok_or("The Overleaf editor was closed.")?;
        let source = native
            .canonical_snapshot()
            .map_err(|failure| failure.to_string())?;
        let prepared = prepare_native_update(
            tab,
            native,
            path,
            native.saved_canonical_source(),
            source.source(),
            self.overleaf_native_cursor(tab, context),
            UpdateMode::Rename,
        )?;
        Ok(self.commit_overleaf_native(vec![prepared], context))
    }

    /// Source revisions flow through the existing providers; none is restarted.
    /// Their published diagnostics remain visible until a current reply arrives.
    pub(in crate::app) fn notify_overleaf_document_changed(
        &mut self,
        tab: u64,
        previous_path: Option<&Path>,
    ) {
        let active = self.tabs.active_id() == Some(tab);
        if active {
            self.activity.completion = None;
            self.activity.hover = None;
            self.editor_completion = None;
            self.clear_editor_hover();
            self.formatting.cancel();
        }
        let Some(document) = self.document_for_tab(tab) else {
            return;
        };
        let kind = document.kind();
        let path = document
            .path()
            .clone()
            .unwrap_or_else(|| self.untitled_tab_path(tab));
        if previous_path.is_some_and(|previous| previous != path.as_path())
            && self.tabs.preview_id() == Some(tab)
        {
            // The hosted renderer is pinned by launch configuration. This
            // includes an entry changing language, such as Typst to TeX.
            self.restart_tinymist_for_preview_entry();
            return;
        }
        if let Some(previous_path) = previous_path
            && let Ok(uri) = crate::tinymist::path_to_file_uri(previous_path)
            && let Some(effect) = self.tinymist_sync.close_uri(&uri)
        {
            let _ = self.apply_tinymist_sync_batch(crate::tinymist_sync::Batch {
                source: String::new(),
                effects: vec![effect],
            });
        }
        if !kind.is_typst() {
            if active {
                let batch =
                    self.tinymist_sync
                        .switch(None, false, self.preview.connection.is_ready());
                let _ = self.apply_tinymist_sync_batch(batch);
            }
            return;
        }
        if active && previous_path.is_some() && self.tinymist_sync.generation().is_none() {
            self.restart_tinymist_preserving_preview();
            return;
        }
        let input = match crate::tinymist_sync::collect(self.document_for_tab(tab).unwrap(), &path)
        {
            Ok(input) => input,
            Err(error) => {
                self.preview.tinymist_state = ServiceState::Degraded(error);
                return;
            }
        };
        let batch = if active {
            self.tinymist_sync
                .switch(Some(input), false, self.preview.connection.is_ready())
        } else {
            self.tinymist_sync.change_background(input)
        };
        if let Err(error) = self.apply_tinymist_sync_batch(batch) {
            self.preview.tinymist_state = ServiceState::Degraded(error);
        }
    }

    fn overleaf_native_tabs(&self, id: &str) -> Vec<u64> {
        self.tabs
            .ids()
            .filter(|&tab| self.overleaf_tab_matches(tab, id))
            .collect()
    }
    fn overleaf_native_cursor(&self, tab: u64, context: &egui::Context) -> CCursorRange {
        if self.tabs.active_id() == Some(tab) {
            egui::text_edit::TextEditState::load(context, source_editor_id(context))
                .and_then(|state| state.cursor.char_range())
                .unwrap_or_default()
        } else {
            self.tabs
                .pdf_record(Some(tab))
                .editor
                .as_ref()
                .and_then(|state| state.cursor.char_range())
                .unwrap_or_default()
        }
    }
    fn commit_overleaf_native(
        &mut self,
        prepared: Vec<PreparedNative>,
        context: &egui::Context,
    ) -> bool {
        let mut active_changed = false;
        let delay = Duration::from_millis(self.settings.auto_save_delay_ms.max(100));
        for prepared in prepared {
            let tab = prepared.tab;
            // All owners and projections were preflighted synchronously.
            let document = self.tabs.document_mut(tab).expect("prepared native owner");
            let previous_path = document.path().clone();
            let renamed = previous_path.as_deref() != Some(prepared.path.as_path())
                || document.kind() != prepared.kind;
            let undo_cursor = clamp_cursor_range(
                prepared.cursor,
                prepared.update.saved_source().chars().count(),
            );
            let changed = document
                .commit_external_change(prepared.update, undo_cursor)
                .expect("preflighted native owner");
            if renamed {
                document
                    .rename(prepared.path, prepared.kind)
                    .expect("preflighted document kind");
            }
            let _ = document.take_edit();
            document.set_history_reset(false);
            let dirty = document.is_dirty();
            let record = self.tabs.pdf_record_mut(Some(tab));
            if changed {
                record.pdf_output.invalidate();
            }
            if let Some(state) = &mut record.editor {
                if changed {
                    state.clear_undoer();
                }
                state.cursor.set_char_range(Some(prepared.cursor));
            }
            if self.tabs.active_id() == Some(tab) {
                self.store_editor_cursor(context, prepared.cursor);
                self.pending_editor_selection = None;
                active_changed |= changed;
            }
            if changed || renamed {
                self.notify_overleaf_document_changed(
                    tab,
                    renamed.then_some(previous_path.as_deref()).flatten(),
                );
            }
            let deadline = (dirty && self.settings.auto_save).then(|| Instant::now() + delay);
            self.set_tab_autosave(tab, deadline);
            if deadline.is_some() {
                context.request_repaint_after(delay);
            }
        }
        active_changed
    }
}

fn prepare_native(
    tab: u64,
    native: &DocumentSession,
    path: &Path,
    remote: &str,
    source: &str,
    cursor: CCursorRange,
) -> Result<PreparedNative, String> {
    prepare_native_update(
        tab,
        native,
        path,
        remote,
        source,
        cursor,
        UpdateMode::Preserve,
    )
}

fn prepare_native_update(
    tab: u64,
    native: &DocumentSession,
    path: &Path,
    remote: &str,
    source: &str,
    cursor: CCursorRange,
    mode: UpdateMode,
) -> Result<PreparedNative, String> {
    let current = native
        .canonical_snapshot()
        .map_err(|failure| failure.to_string())?;
    let offsets = if native.config().is_none() {
        // Ordinary TeX/Typst has identical source domains. Constructing two
        // complete coordinate indices merely to apply this identity map made
        // small incoming edits scale with several extra document-wide scans.
        let length = current.source().chars().count();
        [cursor.primary.index.0, cursor.secondary.index.0]
            .map(|offset| ScalarOffset::new(offset.min(length)))
    } else {
        [cursor.primary.index.0, cursor.secondary.index.0].map(|offset| {
            current
                .canonical_scalar_cursor(ScalarOffset::new(offset))
                .unwrap_or(ScalarOffset::new(0))
        })
    };
    let mapped = map_canonical_cursors(current.source(), source, offsets);
    let kind = if (native.path().as_deref() == Some(path) && mode != UpdateMode::Rename)
        || native.kind().preview_only()
    {
        native.kind()
    } else {
        crate::document::detect_document(path, source.as_bytes()).unwrap_or(DocumentKind::Text)
    };
    let prepare = if !kind.is_typst() && native.config().is_some() {
        native.prepare_external_change_without_projection(
            source,
            remote,
            fingerprint(remote.as_bytes()),
        )
    } else {
        native.prepare_external_change(source, remote, fingerprint(remote.as_bytes()))
    };
    let update = prepare
        .or_else(|failure| {
            if mode == UpdateMode::Clean && native.config().is_some() && !native.is_dirty() {
                native.prepare_external_change_without_projection(
                    source,
                    remote,
                    fingerprint(remote.as_bytes()),
                )
            } else {
                Err(failure)
            }
        })
        .map_err(|failure| failure.to_string())?;
    let offsets = mapped.map(|offset| update.editor_scalar_cursor(offset));
    let [Some(primary), Some(secondary)] = offsets else {
        return Err("The Overleaf selection is outside the updated document.".into());
    };
    let cursor = CCursorRange {
        primary: CCursor::new(primary.get()),
        secondary: CCursor::new(secondary.get()),
        h_pos: cursor.h_pos,
    };
    Ok(PreparedNative {
        tab,
        update,
        path: path.into(),
        kind,
        cursor,
    })
}

struct CursorEdit<'a> {
    range: std::ops::Range<usize>,
    replacement: &'a str,
}

fn trimmed_cursor_edit<'a>(before: &str, after: &'a str, start: usize) -> Option<CursorEdit<'a>> {
    if before == after {
        return None;
    }
    let mut prefix = before
        .bytes()
        .zip(after.bytes())
        .take_while(|(a, b)| a == b)
        .count();
    while !before.is_char_boundary(prefix) {
        prefix -= 1;
    }
    let suffix = before[prefix..]
        .bytes()
        .rev()
        .zip(after[prefix..].bytes().rev())
        .take_while(|(a, b)| a == b)
        .count();
    let mut end = before.len() - suffix;
    while !before.is_char_boundary(end) {
        end += 1;
    }
    let mut suffix = before.len() - end;
    if prefix > 0
        && before.as_bytes()[prefix - 1] == b'\r'
        && before.as_bytes().get(prefix) == Some(&b'\n')
    {
        prefix -= 1;
    }
    if end > 0 && before.as_bytes()[end - 1] == b'\r' && before.as_bytes().get(end) == Some(&b'\n')
    {
        end += 1;
        suffix -= 1;
    }
    Some(CursorEdit {
        range: start + prefix..start + end,
        replacement: &after[prefix..after.len() - suffix],
    })
}

fn cursor_line_body(line: &str) -> &str {
    line.strip_suffix("\r\n")
        .or_else(|| line.strip_suffix('\n'))
        .unwrap_or(line)
}

/// Map selections directly through the source diff. Unlike service edits, a
/// native cursor needs neither UTF-16 positions nor a rebuilt source string.
/// Each untouched segment is counted once; there is no document-wide index.
fn map_canonical_cursors(
    before: &str,
    after: &str,
    offsets: [ScalarOffset; 2],
) -> [ScalarOffset; 2] {
    let Some(single) = trimmed_cursor_edit(before, after, 0) else {
        return offsets;
    };
    let mut edits = Vec::new();
    if !before[single.range.clone()].contains('\n') && !single.replacement.contains('\n') {
        edits.push(single);
    } else {
        const MAX_LINES: usize = 100_000;
        let old: Vec<_> = before.split_inclusive('\n').take(MAX_LINES + 1).collect();
        let new: Vec<_> = after.split_inclusive('\n').take(MAX_LINES + 1).collect();
        if old.len() > MAX_LINES || new.len() > MAX_LINES {
            edits.push(single);
        } else {
            let starts = |lines: &[&str]| {
                let mut byte = 0;
                lines
                    .iter()
                    .map(|line| {
                        let start = byte;
                        byte += line.len();
                        start
                    })
                    .chain([lines.iter().map(|line| line.len()).sum()])
                    .collect::<Vec<_>>()
            };
            let old_starts = starts(&old);
            let new_starts = starts(&new);
            let pairs = crate::git::editor::document_diff::align_line_slices(&old, &new, |a, b| {
                cursor_line_body(a) == cursor_line_body(b)
            });
            let (mut old_line, mut new_line, mut row) = (0, 0, 0);
            while row < pairs.len() {
                if !pairs[row].changed {
                    // Bodies match, so differing raw lengths mean only the
                    // terminator changed. Keep that edit to account for CRLF
                    // deltas and right-affinity at an appended EOF newline.
                    // Equal lengths skip another scan of unchanged text.
                    if old[old_line].len() != new[new_line].len()
                        && let Some(edit) =
                            trimmed_cursor_edit(old[old_line], new[new_line], old_starts[old_line])
                    {
                        edits.push(edit);
                    }
                    old_line += usize::from(pairs[row].old.is_some());
                    new_line += usize::from(pairs[row].new.is_some());
                    row += 1;
                    continue;
                }
                let (old_first, new_first) = (old_line, new_line);
                while row < pairs.len() && pairs[row].changed {
                    old_line += usize::from(pairs[row].old.is_some());
                    new_line += usize::from(pairs[row].new.is_some());
                    row += 1;
                }
                let start = old_starts[old_first];
                if let Some(edit) = trimmed_cursor_edit(
                    &before[start..old_starts[old_line]],
                    &after[new_starts[new_first]..new_starts[new_line]],
                    start,
                ) {
                    edits.push(edit);
                }
            }
        }
    }
    let mut result = offsets.map(ScalarOffset::get);
    let mut finished = [false; 2];
    let (mut byte, mut scalar, mut delta) = (0, 0, 0isize);
    for edit in edits {
        scalar += before[byte..edit.range.start].chars().count();
        let end = scalar + before[edit.range.clone()].chars().count();
        let inserted = edit.replacement.chars().count();
        for index in 0..2 {
            if finished[index] {
                continue;
            }
            let original = offsets[index].get();
            if original < scalar {
                result[index] = original.saturating_add_signed(delta);
                finished[index] = true;
            } else if scalar != end && original < end {
                result[index] = scalar.saturating_add_signed(delta)
                    + if original == scalar { 0 } else { inserted };
                finished[index] = true;
            }
        }
        delta += inserted as isize - (end - scalar) as isize;
        byte = edit.range.end;
        scalar = end;
    }
    for index in 0..2 {
        if !finished[index] {
            result[index] = offsets[index].get().saturating_add_signed(delta);
        }
    }
    result.map(ScalarOffset::new)
}

#[cfg(test)]
mod tests {
    use super::*;

    const PROJECT: &str = "0123456789abcdef01234567";
    const BASE: &str = "Opening baseline.\nMiddle baseline.\nClosing baseline.\n";
    const LOCAL: &str = "Opening local.\nMiddle baseline.\nClosing baseline.\n";
    const REMOTE: &str = "Opening baseline.\nMiddle baseline.\nClosing remote.\n";
    const MERGED: &str = "Opening local.\nMiddle baseline.\nClosing remote.\n";
    fn fixture(root: &Path, context: &egui::Context) -> EditorApp {
        let mut app = EditorApp::dormant_for_tests(context, root.into());
        let project =
            ProjectUrl::parse(&format!("https://www.overleaf.com/project/{PROJECT}")).unwrap();
        let mut model = Model::for_project(7, &project);
        model
            .apply(BrowserEvent::Project {
                epoch: 7,
                project_id: PROJECT.into(),
                name: "Paper".into(),
                entries: vec![Entry {
                    id: "a".into(),
                    path: "main.tex".into(),
                    kind: EntryKind::Doc,
                }],
                root_doc: Some("a".into()),
            })
            .unwrap();
        model.apply(snapshot(BASE)).unwrap();
        app.overleaf.epoch = 7;
        app.overleaf.connection = Some(Connection {
            project,
            root: root.into(),
            model,
            status: "Connected".into(),
            transport: Transport::Online,
            storage: None,
            hydration: Hydration::default(),
        });
        app.document_mut().replace_loaded_unprojected(
            BASE.into(),
            root.join("main.tex"),
            DocumentKind::Tex,
            Some(fingerprint(BASE.as_bytes())),
        );
        app.settings.auto_save = false;
        let tab = app.tabs.active_id().unwrap();
        app.overleaf.tab_entities.insert(tab, "a".into());
        app
    }
    fn snapshot(source: &str) -> BrowserEvent {
        BrowserEvent::Document {
            epoch: 7,
            id: "a".into(),
            path: "main.tex".into(),
            source: source.into(),
        }
    }
    fn stage_merge(app: &mut EditorApp) {
        app.document_mut()
            .edit(CCursorRange::default(), |text| *text = LOCAL.into());
        let model = &mut app.overleaf.connection.as_mut().unwrap().model;
        model.set_draft("a", LOCAL.into()).unwrap();
        model.apply(snapshot(REMOTE)).unwrap();
        app.stage_overleaf_rebase("a", REMOTE).unwrap();
    }
    fn mirror_now(app: &mut EditorApp, root: &Path, source: &str, context: &egui::Context) {
        app.overleaf
            .connection
            .as_mut()
            .unwrap()
            .model
            .apply(snapshot(source))
            .unwrap();
        let hydration_session = app.overleaf.connection.as_ref().unwrap().hydration.session;
        app.apply_overleaf_mirror(
            Mirrored {
                origin: MirrorOrigin {
                    root: app.overleaf.connection.as_ref().unwrap().root.clone(),
                    profile: app.overleaf.browser_profile,
                },
                heads: BTreeMap::new(),
                documents: BTreeMap::new(),
                epoch: 7,
                hydration_session,
                tree: None,
                error: None,
                files: vec![(
                    MirrorWork {
                        checkpoint: false,
                        entry: Entry {
                            id: "a".into(),
                            path: "main.tex".into(),
                            kind: EntryKind::Doc,
                        },
                        data: MirrorData::Document(source.into()),
                        save: None,
                    },
                    Ok(root.join("main.tex")),
                )],
            },
            context,
        );
    }

    #[test]
    fn incoming_sync_keeps_session_selection_inline_diagnostics_and_index_until_fresh_results() {
        let directory = tempfile::tempdir().unwrap();
        let context = egui::Context::default();
        let mut app = fixture(directory.path(), &context);
        app.lifecycle = DocumentLifecycle::Active;
        app.settings.english_grammar = true;
        app.update_writing_checks(&context);
        let diagnostic = |provider: &str| Diagnostic {
            correction: None,
            provider: Some(provider.into()),
            severity: DiagnosticSeverity::Warning,
            source: DiagnosticSource::File(directory.path().join("main.tex")),
            location: Some(crate::diagnostics::DiagnosticLocation { line: 2, column: 1 }),
            message: format!("{provider} previous result"),
            details: Vec::new(),
        };
        app.writing.diagnostics = vec![diagnostic("Harper")];
        app.tex_diagnostics[0] = vec![diagnostic("TexLab")];
        app.ltex.diagnostics = vec![diagnostic("LTeX+")];
        app.preview.tinymist_diagnostics = vec![diagnostic("Tinymist")];
        app.update_tex_diagnostics();
        app.prepare_editor_data();
        let inline_before = app.editor_data.line_diagnostics();
        assert_eq!(inline_before.len(), 1);
        app.project_index.packages = vec!["unchanged package index".into()];
        app.tinymist_sync
            .fixture_session(Generation(77), None, None);
        app.preview.recovery.started(Generation(77));
        let key = app.document().key();
        let cursor = CCursorRange {
            primary: CCursor::new(21),
            secondary: CCursor::new(19),
            ..Default::default()
        };
        app.store_editor_cursor(&context, cursor);
        let remote = format!("Added remote line.\n{BASE}");
        mirror_now(&mut app, directory.path(), &remote, &context);
        assert_eq!(app.document().key(), key.after_edit());
        assert_eq!(app.tinymist_sync.generation(), Some(Generation(77)));
        assert!(app.preview.recovery.accepts(Generation(77)));
        assert_eq!(app.project_index.packages, ["unchanged package index"]);
        let expected = CCursorRange {
            primary: CCursor::new(40),
            secondary: CCursor::new(38),
            ..Default::default()
        };
        assert_eq!(
            app.overleaf_native_cursor(app.tabs.active_id().unwrap(), &context),
            expected
        );
        app.update_writing_checks(&context);
        app.prepare_editor_data();
        assert_eq!(app.writing.diagnostics.len(), 1);
        assert_eq!(app.tex_diagnostics[0].len(), 1);
        assert_eq!(app.ltex.diagnostics.len(), 1);
        assert_eq!(app.preview.tinymist_diagnostics.len(), 1);
        let inline_after = app.editor_data.line_diagnostics();
        assert_eq!(inline_before[0].summary, inline_after[0].summary);
        assert!(
            app.preview
                .editor_diagnostics
                .iter()
                .any(|diagnostic| diagnostic.provider.as_deref() == Some("Harper"))
        );
    }

    #[test]
    fn clean_unprojectable_remote_source_falls_back_atomically_without_stale_editable_text() {
        let directory = tempfile::tempdir().unwrap();
        let context = egui::Context::default();
        let mut app = fixture(directory.path(), &context);
        let canonical = "#import \"@preview/mitex:0.2.7\": mitex\n#mitex(\"x\")\n";
        app.document_mut().replace_loaded_unprojected(
            canonical.into(),
            directory.path().join("main.tex"),
            DocumentKind::Typst,
            None,
        );
        app.document_mut()
            .enable(tiptoptyp::mitex_projection::Config::default())
            .unwrap();
        let key = app.document().key();
        let remote = format!("{canonical} $ y $");
        mirror_now(&mut app, directory.path(), &remote, &context);
        assert_eq!(app.document().key(), key.after_edit());
        assert_eq!(app.document().source(), &remote);
        assert_eq!(
            app.document().canonical_snapshot().unwrap().source(),
            remote
        );
        assert!(app.document().config().is_none());
        assert_eq!(
            app.document().kind(),
            DocumentKind::Typst,
            "ordinary source sync does not reclassify a document from its path"
        );
        assert!(!app.document().is_dirty());
        assert!(!app.overleaf_source_locked());
        assert!(app.overleaf.error.is_none());
        let request = app
            .document()
            .prepare_save(directory.path().join("main.tex"), DocumentKind::Typst)
            .unwrap();
        assert_eq!(request.source(), remote);
    }

    fn open_created_tex(app: &mut EditorApp, root: &Path, context: &egui::Context) {
        let connection = app.overleaf.connection.as_mut().unwrap();
        let mut entries = connection.model.tree().unwrap().entries().to_vec();
        entries.push(Entry {
            id: "new".into(),
            path: "draft.tex".into(),
            kind: EntryKind::Doc,
        });
        connection
            .model
            .apply(BrowserEvent::Project {
                epoch: 7,
                project_id: PROJECT.into(),
                name: "Paper".into(),
                entries,
                root_doc: Some("a".into()),
            })
            .unwrap();
        app.workspace = Some(WorkspaceTree::from_snapshot(remote_snapshot(
            root,
            connection.model.tree().unwrap(),
        )));
        app.overleaf.open_after = Some("new".into());
        app.update_overleaf_event(
            BrowserEvent::Document {
                epoch: 7,
                id: "new".into(),
                path: "draft.tex".into(),
                source: "New TeX source.\n".into(),
            },
            context,
        )
        .unwrap();
        let index = app
            .overleaf
            .queued
            .iter()
            .position(|work| work.entry.id == "new")
            .unwrap();
        let work = app.overleaf.queued.remove(index).unwrap();
        // Active owners append a tab; dormant fixtures intentionally reuse
        // their sole synthetic document instead of exercising that policy.
        app.lifecycle = DocumentLifecycle::Active;
        super::super::tests::apply_work(app, work, root, 7, context);
        app.lifecycle = DocumentLifecycle::Dormant;
    }

    #[test]
    fn opening_managed_tex_preserves_explicit_typst_preview_and_worker() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().canonicalize().unwrap();
        let context = egui::Context::default();
        let mut app = fixture(&root, &context);
        // Live document routing must not use Main's fixed screenshot preview.
        app.snapshot_scene = None;
        let pinned_path = root.join("pinned.typ");
        app.document_mut().replace_loaded_unprojected(
            "= Pinned preview\n".into(),
            pinned_path.clone(),
            DocumentKind::Typst,
            None,
        );
        let pinned = app.tabs.active_id().unwrap();
        app.select_preview_tab(pinned, &context);
        app.preview
            .set_requested_backend(PreviewPreference::InteractivePartial);
        let pinned_key = app.document().key();
        let uri = crate::tinymist::path_to_file_uri(&pinned_path).unwrap();
        let input = crate::tinymist_sync::collect(app.document(), &pinned_path).unwrap();
        app.tinymist_sync.begin(Generation(77), uri.clone(), input);
        app.preview.connection.start(Generation(77));
        app.preview.connection.initialized(Generation(77));
        app.preview.recovery.started(Generation(77));

        open_created_tex(&mut app, &root, &context);

        assert_ne!(app.tabs.active_id(), Some(pinned));
        assert_eq!(app.document().kind(), DocumentKind::Tex);
        assert_eq!(
            app.document().path().as_deref(),
            Some(root.join("draft.tex").as_path())
        );
        assert_eq!(app.tabs.preview_id(), Some(pinned));
        assert_eq!(
            app.designated_preview_path().as_deref(),
            Some(pinned_path.as_path())
        );
        assert_eq!(app.preview_document_kind(), DocumentKind::Typst);
        assert_eq!(app.document_for_tab(pinned).unwrap().key(), pinned_key);
        assert_eq!(app.tinymist_sync.generation(), Some(Generation(77)));
        assert_eq!(app.tinymist_sync.preview_uri(), Some(uri.as_str()));
        assert!(app.preview.recovery.accepts(Generation(77)));
        assert_eq!(
            app.preview_status_snapshot().effective_backend,
            crate::preview::PreviewBackend::Interactive
        );
    }

    #[test]
    fn opening_first_managed_source_adopts_preview_from_nonbuildable_launch_tab() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().canonicalize().unwrap();
        let context = egui::Context::default();
        let mut app = fixture(&root, &context);
        // Live document routing must not use Main's fixed screenshot preview.
        app.snapshot_scene = None;
        app.document_mut().replace_loaded_unprojected(
            "Launch notes\n".into(),
            root.join("notes.txt"),
            DocumentKind::Text,
            None,
        );
        let launch_tab = app.tabs.active_id();

        open_created_tex(&mut app, &root, &context);

        assert_ne!(app.tabs.active_id(), launch_tab);
        assert_eq!(app.tabs.preview_id(), app.tabs.active_id());
        assert_eq!(app.document().kind(), DocumentKind::Tex);
        assert_eq!(app.preview_document_kind(), DocumentKind::Tex);
        assert_eq!(
            app.designated_preview_path().as_deref(),
            Some(root.join("draft.tex").as_path())
        );
        assert_eq!(app.document().source(), "New TeX source.\n");
    }

    #[test]
    fn remote_extension_renames_update_active_and_parked_languages_without_restarting_other_preview()
     {
        for (old_kind, old_name, new_name, expected) in [
            (
                DocumentKind::Text,
                "main.txt",
                "renamed.tex",
                DocumentKind::Tex,
            ),
            (
                DocumentKind::Tex,
                "main.tex",
                "renamed.typ",
                DocumentKind::Typst,
            ),
            (
                DocumentKind::Tex,
                "main.tex",
                "renamed.txt",
                DocumentKind::Text,
            ),
            (
                DocumentKind::Typst,
                "main.typ",
                "other.typ",
                DocumentKind::Typst,
            ),
        ] {
            for parked in [false, true] {
                for dirty in [false, true] {
                    let directory = tempfile::tempdir().unwrap();
                    let context = egui::Context::default();
                    let mut app = fixture(directory.path(), &context);
                    let old_path = directory.path().join(old_name);
                    let new_path = directory.path().join(new_name);
                    app.document_mut().replace_loaded_unprojected(
                        BASE.into(),
                        old_path.clone(),
                        old_kind,
                        None,
                    );
                    let (tab, other) =
                        super::super::tests::park_main(&mut app, directory.path(), &context);
                    app.select_preview_tab(other, &context);
                    if !parked {
                        app.activate_tab(tab, &context);
                    }
                    let active = app.tabs.active_id();
                    let other_key = app.document_for_tab(other).unwrap().key();
                    let source = if dirty {
                        format!("{BASE}local draft\n")
                    } else {
                        BASE.into()
                    };
                    if dirty {
                        app.tabs
                            .document_mut(tab)
                            .unwrap()
                            .edit(CCursorRange::default(), |text| *text = source.clone());
                    }
                    app.overleaf.tab_entities.insert(tab, "a".into());
                    let entry = Entry {
                        id: "a".into(),
                        path: new_name.into(),
                        kind: EntryKind::Doc,
                    };
                    let other_entry = Entry {
                        id: "b".into(),
                        path: "chapter.tex".into(),
                        kind: EntryKind::Doc,
                    };
                    let connection = app.overleaf.connection.as_mut().unwrap();
                    if dirty {
                        connection.model.set_draft("a", source.clone()).unwrap();
                    }
                    connection
                        .model
                        .apply(BrowserEvent::Project {
                            epoch: 7,
                            project_id: PROJECT.into(),
                            name: "Paper".into(),
                            entries: vec![entry.clone(), other_entry],
                            root_doc: Some("a".into()),
                        })
                        .unwrap();
                    connection
                        .model
                        .apply(BrowserEvent::Document {
                            epoch: 7,
                            id: "a".into(),
                            path: new_name.into(),
                            source: BASE.into(),
                        })
                        .unwrap();
                    let hydration_session = connection.hydration.session;
                    let old_uri = crate::tinymist::path_to_file_uri(&old_path).unwrap();
                    let new_uri = crate::tinymist::path_to_file_uri(&new_path).unwrap();
                    let preview_uri = "file:///unrelated-preview.typ".to_owned();
                    app.tinymist_sync.fixture_session(
                        Generation(77),
                        None,
                        Some(preview_uri.clone()),
                    );
                    if old_kind.is_typst() {
                        app.tinymist_sync.fixture_open_uri(old_uri.clone());
                        if !parked {
                            app.tinymist_sync.fixture_select_current(old_uri.clone());
                            app.tinymist_sync.confirm_open(&old_uri);
                        }
                    }
                    app.preview.connection.start(Generation(77));
                    app.preview.connection.initialized(Generation(77));
                    app.preview.recovery.started(Generation(77));
                    app.apply_overleaf_mirror(
                        Mirrored {
                            origin: MirrorOrigin {
                                root: app.overleaf.connection.as_ref().unwrap().root.clone(),
                                profile: app.overleaf.browser_profile,
                            },
                            heads: BTreeMap::new(),
                            documents: BTreeMap::new(),
                            epoch: 7,
                            hydration_session,
                            tree: None,
                            error: None,
                            files: vec![(
                                MirrorWork {
                                    checkpoint: false,
                                    entry,
                                    data: MirrorData::Document(BASE.into()),
                                    save: None,
                                },
                                Ok(new_path.clone()),
                            )],
                        },
                        &context,
                    );
                    let document = app.document_for_tab(tab).unwrap();
                    assert_eq!(
                        document.kind(),
                        expected,
                        "{old_name} -> {new_name}, parked={parked}, dirty={dirty}"
                    );
                    assert_eq!(document.path().as_deref(), Some(new_path.as_path()));
                    assert_eq!(document.source(), &source);
                    assert_eq!(document.saved_source(), BASE);
                    assert_eq!(document.is_dirty(), dirty);
                    assert_eq!(app.overleaf_entity_for_tab(tab), Some("a"));
                    assert_eq!(app.tabs.active_id(), active);
                    assert_eq!(app.document_for_tab(other).unwrap().key(), other_key);
                    assert_eq!(app.tinymist_sync.generation(), Some(Generation(77)));
                    assert_eq!(app.tinymist_sync.preview_uri(), Some(preview_uri.as_str()));
                    assert!(app.preview.recovery.accepts(Generation(77)));
                    assert!(!app.tinymist_sync.is_uri_open(&old_uri));
                    assert_eq!(app.tinymist_sync.is_uri_open(&new_uri), expected.is_typst());
                    if !parked {
                        assert_eq!(
                            app.tinymist_sync.current_uri(),
                            expected.is_typst().then_some(new_uri.as_str())
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn save_receipt_reclassifies_a_destination_already_renamed_by_record_save() {
        let directory = tempfile::tempdir().unwrap();
        let context = egui::Context::default();
        let mut app = fixture(directory.path(), &context);
        let tab = app.tabs.active_id().unwrap();
        let path = directory.path().join("renamed.txt");
        let key = app.document().key();
        let request = app
            .document()
            .prepare_save(path.clone(), DocumentKind::Tex)
            .unwrap();
        let entry = Entry {
            id: "a".into(),
            path: "renamed.txt".into(),
            kind: EntryKind::Doc,
        };
        let connection = app.overleaf.connection.as_mut().unwrap();
        connection
            .model
            .apply(BrowserEvent::Project {
                epoch: 7,
                project_id: PROJECT.into(),
                name: "Paper".into(),
                entries: vec![entry.clone()],
                root_doc: Some("a".into()),
            })
            .unwrap();
        connection
            .model
            .apply(BrowserEvent::Document {
                epoch: 7,
                id: "a".into(),
                path: "renamed.txt".into(),
                source: BASE.into(),
            })
            .unwrap();
        let hydration_session = connection.hydration.session;
        let operation = begin_test_save(&mut app);
        app.overleaf.save = SavePhase::WritingMirror {
            entity: "a".into(),
            operation,
        };
        app.apply_overleaf_mirror(
            Mirrored {
                origin: MirrorOrigin {
                    root: app.overleaf.connection.as_ref().unwrap().root.clone(),
                    profile: app.overleaf.browser_profile,
                },
                heads: BTreeMap::new(),
                documents: BTreeMap::new(),
                epoch: 7,
                hydration_session,
                tree: None,
                error: None,
                files: vec![(
                    MirrorWork {
                        checkpoint: false,
                        entry,
                        data: MirrorData::Document(BASE.into()),
                        save: Some(RemoteSave {
                            operation,
                            id: "a".into(),
                            wire: 1,
                            tab,
                            key,
                            request,
                            continuation: None,
                            started: Instant::now(),
                        }),
                    },
                    Ok(path.clone()),
                )],
            },
            &context,
        );
        assert_eq!(app.document().kind(), DocumentKind::Text);
        assert_eq!(app.document().path().as_deref(), Some(path.as_path()));
        assert_eq!(app.document().source(), BASE);
        assert_eq!(app.document().saved_source(), BASE);
        assert!(!app.document().is_dirty());
        assert!(!app.document_workflow.save_in_flight());
        assert_eq!(app.overleaf_entity_for_tab(tab), Some("a"));
        assert!(app.overleaf.error.is_none(), "{:?}", app.overleaf.error);
    }

    #[test]
    fn projected_dirty_rename_to_tex_keeps_authored_source_baseline_and_mapped_selection() {
        let directory = tempfile::tempdir().unwrap();
        let context = egui::Context::default();
        let mut app = fixture(directory.path(), &context);
        let canonical = "#import \"@preview/mitex:0.2.7\": mitex\n#mitex(\"x\")\n";
        app.document_mut().replace_loaded_unprojected(
            canonical.into(),
            directory.path().join("main.typ"),
            DocumentKind::Typst,
            None,
        );
        app.document_mut()
            .enable(tiptoptyp::mitex_projection::Config::default())
            .unwrap();
        app.document_mut().edit(CCursorRange::default(), |source| {
            source.push_str("local draft\n")
        });
        let authored = app
            .document()
            .canonical_snapshot()
            .unwrap()
            .source()
            .to_owned();
        let end = app.document().source().chars().count();
        app.store_editor_cursor(&context, CCursorRange::one(CCursor::new(end)));
        app.rename_overleaf_native_tab(
            app.tabs.active_id().unwrap(),
            &directory.path().join("main.tex"),
            &context,
        )
        .unwrap();
        assert_eq!(app.document().kind(), DocumentKind::Tex);
        assert!(app.document().config().is_none());
        assert_eq!(app.document().source(), &authored);
        assert_eq!(app.document().saved_source(), canonical);
        assert!(app.document().is_dirty());
        assert_eq!(
            app.document()
                .prepare_save(directory.path().join("main.tex"), DocumentKind::Tex)
                .unwrap()
                .source(),
            authored
        );
        assert_eq!(
            app.overleaf_entity_for_tab(app.tabs.active_id().unwrap()),
            Some("a")
        );
        assert_eq!(
            app.overleaf_native_cursor(app.tabs.active_id().unwrap(), &context)
                .primary
                .index
                .0,
            authored.chars().count()
        );
    }

    #[test]
    fn rename_only_sync_requests_new_pinned_entry_without_replacing_source() {
        let directory = tempfile::tempdir().unwrap();
        let context = egui::Context::default();
        let mut app = fixture(directory.path(), &context);
        let old_path = directory.path().join("main.typ");
        let new_path = directory.path().join("renamed.typ");
        app.document_mut().replace_loaded_unprojected(
            BASE.into(),
            old_path.clone(),
            DocumentKind::Typst,
            None,
        );
        app.document_mut().edit(CCursorRange::default(), |source| {
            source.push_str("saved local line\n")
        });
        let source = app.document().source().clone();
        let request = app
            .document()
            .prepare_save(old_path.clone(), DocumentKind::Typst)
            .unwrap();
        app.document_mut()
            .record_save(request.committed(5))
            .unwrap();
        app.document_mut().set_history_reset(false);
        let buffer = app.document().snapshot();
        let old_uri = crate::tinymist::path_to_file_uri(&old_path).unwrap();
        app.preview.recovery.started(Generation(77));
        app.tinymist_sync.begin(
            Generation(77),
            old_uri.clone(),
            crate::tinymist_sync::VersionedInput {
                key: app.document().key(),
                uri: old_uri.clone(),
                version: revision_as_i32(app.document().revision()),
                source: source.clone(),
            },
        );
        let entry = Entry {
            id: "a".into(),
            path: "renamed.typ".into(),
            kind: EntryKind::Doc,
        };
        let connection = app.overleaf.connection.as_mut().unwrap();
        connection
            .model
            .apply(BrowserEvent::Project {
                epoch: 7,
                project_id: PROJECT.into(),
                name: "Paper".into(),
                entries: vec![entry.clone()],
                root_doc: Some("a".into()),
            })
            .unwrap();
        connection
            .model
            .apply(BrowserEvent::Document {
                epoch: 7,
                id: "a".into(),
                path: "renamed.typ".into(),
                source: source.clone(),
            })
            .unwrap();
        let hydration_session = connection.hydration.session;
        app.apply_overleaf_mirror(
            Mirrored {
                origin: MirrorOrigin {
                    root: app.overleaf.connection.as_ref().unwrap().root.clone(),
                    profile: app.overleaf.browser_profile,
                },
                heads: BTreeMap::new(),
                documents: BTreeMap::new(),
                epoch: 7,
                hydration_session,
                tree: None,
                error: None,
                files: vec![(
                    MirrorWork {
                        checkpoint: false,
                        entry,
                        data: MirrorData::Document(source.clone()),
                        save: None,
                    },
                    Ok(new_path.clone()),
                )],
            },
            &context,
        );
        assert_eq!(app.document().path().as_deref(), Some(new_path.as_path()));
        assert_eq!(app.document().source(), &source);
        assert_eq!(
            app.document().snapshot().source().as_ptr(),
            buffer.source().as_ptr()
        );
        assert_eq!(app.document().history_availability(), (true, false));
        // The dormant fixture applies the entry-change transition but never
        // launches a subprocess. Its recovery reset proves restart intent;
        // URI bookkeeping alone would not retarget the launched preview.
        assert!(!app.preview.recovery.accepts(Generation(77)));
        assert_eq!(app.tinymist_sync.generation(), Some(Generation(77)));
    }

    #[test]
    fn identical_sync_does_not_invalidate_history_cache_or_queue_builds() {
        let directory = tempfile::tempdir().unwrap();
        let context = egui::Context::default();
        let mut app = fixture(directory.path(), &context);
        app.document_mut().edit(CCursorRange::default(), |source| {
            source.push_str("saved local line\n")
        });
        let source = app.document().source().clone();
        let request = app
            .document()
            .prepare_save(directory.path().join("main.tex"), DocumentKind::Tex)
            .unwrap();
        app.document_mut()
            .record_save(request.committed(9))
            .unwrap();
        app.document_mut().set_history_reset(false);
        let key = app.document().key();
        let buffer = app.document().snapshot();
        app.compile_deadline = None;
        app.project_index_deadline.clear();
        app.lifecycle = DocumentLifecycle::Active;
        mirror_now(&mut app, directory.path(), &source, &context);
        assert_eq!(app.document().key(), key);
        assert_eq!(
            app.document().snapshot().source().as_ptr(),
            buffer.source().as_ptr()
        );
        assert_eq!(app.document().history_availability(), (true, false));
        assert!(!app.document().reset_editor_history);
        assert!(app.compile_deadline.is_none());
        assert!(!app.project_index_deadline.is_pending());
    }

    #[test]
    #[ignore = "opt-in same-profile incoming-sync measurement"]
    fn incoming_overleaf_sync_probe() {
        use std::hint::black_box;
        let directory = tempfile::tempdir().unwrap();
        let context = egui::Context::default();
        let mut app = fixture(directory.path(), &context);
        let baseline: String = (0..8_000)
            .map(|line| format!("Line {line}: A mathematical sentence with α and 😀.\n"))
            .collect();
        let changed = baseline.replacen("Line 4000:", "Line 4000 updated:", 1);
        let path = directory.path().join("main.tex");
        app.document_mut().replace_loaded_unprojected(
            baseline.clone(),
            path.clone(),
            DocumentKind::Tex,
            None,
        );
        app.overleaf
            .connection
            .as_mut()
            .unwrap()
            .model
            .apply(snapshot(&baseline))
            .unwrap();
        // Keep this adapter probe free of filesystem, subprocess and desktop
        // effects. Startup/worker latency needs the separate native journey.
        let run = |app: &mut EditorApp, source: &str| {
            app.overleaf
                .connection
                .as_mut()
                .unwrap()
                .model
                .apply(snapshot(source))
                .unwrap();
            let hydration_session = app.overleaf.connection.as_ref().unwrap().hydration.session;
            app.apply_overleaf_mirror(
                Mirrored {
                    origin: MirrorOrigin {
                        root: app.overleaf.connection.as_ref().unwrap().root.clone(),
                        profile: app.overleaf.browser_profile,
                    },
                    heads: BTreeMap::new(),
                    documents: BTreeMap::new(),
                    epoch: 7,
                    hydration_session,
                    tree: None,
                    error: None,
                    files: vec![(
                        MirrorWork {
                            checkpoint: false,
                            entry: Entry {
                                id: "a".into(),
                                path: "main.tex".into(),
                                kind: EntryKind::Doc,
                            },
                            data: MirrorData::Document(source.into()),
                            save: None,
                        },
                        Ok(path.clone()),
                    )],
                },
                &context,
            );
            black_box(app.document().source());
        };
        for step in 0..8 {
            run(&mut app, if step % 2 == 0 { &changed } else { &baseline });
        }
        let started = Instant::now();
        for step in 0..40 {
            run(&mut app, if step % 2 == 0 { &changed } else { &baseline });
        }
        println!(
            "overleaf incoming adapter: lines=8000 bytes={} warmup=8 updates=40 elapsed_us={} lifecycle=dormant theme=none viewport=none",
            baseline.len(),
            started.elapsed().as_micros()
        );
    }

    #[test]
    fn native_merge_retains_remote_saved_baseline_until_explicit_save_ack() {
        let directory = tempfile::tempdir().unwrap();
        let context = egui::Context::default();
        let mut app = fixture(directory.path(), &context);
        stage_merge(&mut app);
        let path = directory.path().join("main.tex");
        assert!(
            app.apply_overleaf_rebase("a", &path, REMOTE, &context)
                .unwrap()
        );
        assert_eq!(app.document().source(), MERGED);
        assert_eq!(app.document().saved_source(), REMOTE);
        assert!(app.document().is_dirty());
        assert!(app.overleaf.rebases.is_empty());
        let request = app
            .document()
            .prepare_save(path, DocumentKind::Tex)
            .unwrap();
        let Some(BrowserCommand::Edit {
            request: wire,
            source,
            ..
        }) = app.overleaf.connection.as_mut().unwrap().model.publish("a")
        else {
            panic!()
        };
        app.overleaf
            .connection
            .as_mut()
            .unwrap()
            .model
            .apply(BrowserEvent::Ack {
                epoch: 7,
                id: "a".into(),
                request: wire,
                source,
            })
            .unwrap();
        assert!(
            app.document().is_dirty(),
            "model ACK alone cannot retire native save receipt"
        );
        app.document_mut()
            .record_save(request.committed(fingerprint(MERGED.as_bytes())))
            .unwrap();
        assert!(!app.document().is_dirty());
    }
    #[test]
    fn stale_native_edits_are_never_overwritten_and_keep_reconciliation_locked() {
        let directory = tempfile::tempdir().unwrap();
        let context = egui::Context::default();
        let mut app = fixture(directory.path(), &context);
        stage_merge(&mut app);
        app.document_mut().edit(CCursorRange::default(), |text| {
            text.push_str("New local work.")
        });
        let source = app.document().source().clone();
        assert_eq!(
            app.apply_overleaf_rebase("a", &directory.path().join("main.tex"), REMOTE, &context),
            Err(LOCAL_CHANGED.into())
        );
        assert_eq!(app.document().source(), &source);
        assert!(app.overleaf.rebases.contains_key("a"));
        assert_eq!(
            app.overleaf
                .connection
                .as_ref()
                .unwrap()
                .model
                .document("a")
                .unwrap()
                .source(),
            MERGED
        );
    }
    #[test]
    fn pure_tab_reactivation_can_refresh_key_without_losing_the_merge() {
        let directory = tempfile::tempdir().unwrap();
        let context = egui::Context::default();
        let mut app = fixture(directory.path(), &context);
        stage_merge(&mut app);
        let key = app.document().key();
        app.document_mut().reactivate_after(key);
        assert!(
            app.apply_overleaf_rebase("a", &directory.path().join("main.tex"), REMOTE, &context)
                .unwrap()
        );
        assert_eq!(app.document().source(), MERGED);
    }
    #[test]
    fn same_file_reconnect_preserves_selection_on_next_active_and_parked_editor_frames() {
        for parked in [false, true] {
            let directory = tempfile::tempdir().unwrap();
            let mut harness = editor_particles::tests::editor(false);
            let context = harness.ctx.clone();
            *harness.state_mut() = fixture(directory.path(), &context);
            harness.run_steps(2);
            let original_tab = harness.state().tabs.active_id().unwrap();
            let expected = CCursorRange {
                primary: CCursor::new(LOCAL.chars().count() - 2),
                secondary: CCursor::new(7),
                ..Default::default()
            };
            {
                let app = harness.state_mut();
                app.document_mut()
                    .edit(CCursorRange::default(), |text| *text = LOCAL.into());
                app.store_editor_cursor(&context, expected);
                if parked {
                    app.lifecycle = DocumentLifecycle::Active;
                    app.new_tab(&context);
                    app.lifecycle = DocumentLifecycle::Dormant;
                }
                let model = &mut app.overleaf.connection.as_mut().unwrap().model;
                model.set_draft("a", LOCAL.into()).unwrap();
                model.reconnect(7);
                model.apply(snapshot(BASE)).unwrap();
                app.stage_overleaf_rebase("a", BASE).unwrap();
                app.apply_overleaf_rebase("a", &directory.path().join("main.tex"), BASE, &context)
                    .unwrap();
                let immediate = app.overleaf_native_cursor(original_tab, &context);
                assert_eq!(immediate.primary.index, expected.primary.index);
                assert_eq!(immediate.secondary.index, expected.secondary.index);
                if parked {
                    app.activate_tab(original_tab, &context);
                }
            }
            // Reconciliation already installs the mapped cursor. Painting the
            // freshly prepared DocumentSession must not reset it a second time.
            harness.run_steps(3);
            let app = harness.state_mut();
            let painted = app.editor_snapshot(&context).cursor;
            assert_eq!(
                painted.primary.index, expected.primary.index,
                "parked={parked}"
            );
            assert_eq!(
                painted.secondary.index, expected.secondary.index,
                "parked={parked}"
            );
            assert_eq!(app.document().source(), LOCAL);
            assert_eq!(app.document().saved_source(), BASE);
            assert!(app.document().is_dirty());
            assert!(app.overleaf_tab_matches(original_tab, "a"));
            assert!(app.overleaf.rebases.is_empty());
        }
    }

    #[test]
    fn reconnect_refresh_keeps_merged_model_draft_and_original_native_owner() {
        let directory = tempfile::tempdir().unwrap();
        let context = egui::Context::default();
        let mut app = fixture(directory.path(), &context);
        stage_merge(&mut app);
        let connection = app.overleaf.connection.as_mut().unwrap();
        connection.hydration.restart();
        connection.model.reconnect(7);
        connection.model.apply(snapshot(REMOTE)).unwrap();
        app.stage_overleaf_rebase("a", REMOTE).unwrap();
        assert!(
            app.apply_overleaf_rebase("a", &directory.path().join("main.tex"), REMOTE, &context)
                .unwrap()
        );
        assert_eq!(app.document().source(), MERGED);
    }
    #[test]
    fn fresh_snapshot_already_matching_local_draft_cleans_native_baseline() {
        let directory = tempfile::tempdir().unwrap();
        let context = egui::Context::default();
        let mut app = fixture(directory.path(), &context);
        app.document_mut()
            .edit(CCursorRange::default(), |text| *text = LOCAL.into());
        let model = &mut app.overleaf.connection.as_mut().unwrap().model;
        model.set_draft("a", LOCAL.into()).unwrap();
        model.apply(snapshot(LOCAL)).unwrap();
        app.stage_overleaf_rebase("a", LOCAL).unwrap();
        app.apply_overleaf_rebase("a", &directory.path().join("main.tex"), LOCAL, &context)
            .unwrap();
        assert_eq!(app.document().source(), LOCAL);
        assert_eq!(app.document().saved_source(), LOCAL);
        assert!(!app.document().is_dirty());
    }
    #[test]
    fn review_choices_stay_atomic_and_never_start_a_publish() {
        for use_remote in [false, true] {
            let directory = tempfile::tempdir().unwrap();
            let context = egui::Context::default();
            let mut app = fixture(directory.path(), &context);
            app.document_mut()
                .edit(CCursorRange::default(), |text| *text = "local".into());
            let model = &mut app.overleaf.connection.as_mut().unwrap().model;
            model.set_draft("a", "local".into()).unwrap();
            model.apply(snapshot("remote")).unwrap();
            let review = model.conflict_review("a").unwrap();
            let source = if use_remote {
                review.remote.clone()
            } else {
                review.local.clone()
            };
            app.apply_overleaf_review(&context, &review, source.clone())
                .unwrap();
            assert_eq!(app.document().source(), &source);
            assert_eq!(app.document().saved_source(), "remote");
            assert_eq!(app.document().is_dirty(), !use_remote);
            assert!(
                app.overleaf
                    .connection
                    .as_ref()
                    .unwrap()
                    .model
                    .in_flight()
                    .is_none()
            );
            assert!(app.overleaf.save.pending().is_none());
            assert!(
                app.apply_overleaf_review(&context, &review, source)
                    .is_err()
            );
        }
    }
    #[test]
    fn sign_in_required_blocks_all_review_choices_despite_stale_online_readiness() {
        for choice in ["local", "remote", "merged"] {
            let directory = tempfile::tempdir().unwrap();
            let context = egui::Context::default();
            let mut app = fixture(directory.path(), &context);
            app.document_mut()
                .edit(CCursorRange::default(), |text| *text = "local".into());
            let model = &mut app.overleaf.connection.as_mut().unwrap().model;
            model.set_draft("a", "local".into()).unwrap();
            model.apply(snapshot("remote")).unwrap();
            let review = model.conflict_review("a").unwrap();
            let selection = CCursorRange::two(CCursor::new(4), CCursor::new(1));
            app.store_editor_cursor(&context, selection);
            app.overleaf.authentication.set_required(true);
            let key = app.document().key();
            let epoch = app.overleaf.epoch;
            assert!(app.overleaf.connection.as_ref().unwrap().online());
            assert!(
                !app.overleaf
                    .connection
                    .as_ref()
                    .unwrap()
                    .hydration
                    .pending()
            );
            assert!(app.overleaf_source_locked());

            let error = app
                .apply_overleaf_review(&context, &review, choice.into())
                .unwrap_err();
            assert!(error.contains("Sign in"), "{error}");
            assert_eq!(app.document().key(), key);
            assert_eq!(app.document().source(), "local");
            assert_eq!(app.document().saved_source(), BASE);
            assert_eq!(app.editor_snapshot(&context).cursor, selection);
            assert_eq!(app.overleaf.epoch, epoch);
            assert!(app.overleaf.save.pending().is_none());
            assert!(!app.overleaf.save.active());
            assert!(!app.local_save.is_running());
            let model = &app.overleaf.connection.as_ref().unwrap().model;
            assert!(model.conflict_review_current(&review));
            assert_eq!(model.document("a").unwrap().source(), "local");
            assert!(model.in_flight().is_none());
        }
    }

    #[test]
    fn native_merge_undo_restores_the_fresh_remote_baseline() {
        let directory = tempfile::tempdir().unwrap();
        let context = egui::Context::default();
        let mut app = fixture(directory.path(), &context);
        stage_merge(&mut app);
        app.apply_overleaf_rebase("a", &directory.path().join("main.tex"), REMOTE, &context)
            .unwrap();
        app.document_mut()
            .history_step(false, CCursorRange::default())
            .unwrap();
        assert_eq!(app.document().source(), REMOTE);
        assert!(!app.document().is_dirty());
    }
    #[test]
    fn unrepresentable_projection_does_not_change_the_native_document() {
        let path = PathBuf::from("/tmp/paper.typ");
        let canonical = r#"#import "@preview/mitex:0.2.7": mitex
#mitex("x")
"#;
        let mut native = DocumentSession::new(
            tiptoptyp_core::document::WindowSessionId::new(42),
            "",
            DocumentKind::Typst,
        );
        native
            .replace_loaded(canonical.into(), path.clone(), DocumentKind::Typst, None)
            .unwrap();
        native
            .enable(tiptoptyp::mitex_projection::Config::default())
            .unwrap();
        let key = native.key();
        let before = native.source().clone();
        assert!(
            prepare_native(
                1,
                &native,
                &path,
                canonical,
                &format!("{canonical} $ y $"),
                CCursorRange::default()
            )
            .is_err()
        );
        assert_eq!(native.key(), key);
        assert_eq!(native.source(), &before);
        assert!(native.config().is_some());
    }
    #[test]
    fn unavailable_or_pending_save_blocks_review_without_mutation() {
        for phase in 0..3 {
            let directory = tempfile::tempdir().unwrap();
            let context = egui::Context::default();
            let mut app = fixture(directory.path(), &context);
            app.document_mut()
                .edit(CCursorRange::default(), |text| *text = "local".into());
            let model = &mut app.overleaf.connection.as_mut().unwrap().model;
            model.set_draft("a", "local".into()).unwrap();
            model.apply(snapshot("remote")).unwrap();
            let review = model.conflict_review("a").unwrap();
            match phase {
                0 => app.overleaf.connection.as_mut().unwrap().transport = Transport::Offline,
                1 => app.overleaf.connection.as_mut().unwrap().hydration = Hydration::waiting(),
                _ => {
                    begin_test_save(&mut app);
                }
            }
            let key = app.document().key();
            assert!(
                app.apply_overleaf_review(&context, &review, review.remote.clone())
                    .is_err()
            );
            assert_eq!(app.document().key(), key);
            assert_eq!(app.document().source(), "local");
            assert!(
                app.overleaf
                    .connection
                    .as_ref()
                    .unwrap()
                    .model
                    .conflict_review_current(&review)
            );
        }
    }
    #[test]
    fn another_documents_save_does_not_block_a_completed_rebase() {
        let directory = tempfile::tempdir().unwrap();
        let context = egui::Context::default();
        let mut app = fixture(directory.path(), &context);
        stage_merge(&mut app);
        hold_test_save(&mut app, "another-document");
        begin_test_save(&mut app);
        assert!(
            app.apply_overleaf_rebase("a", &directory.path().join("main.tex"), REMOTE, &context)
                .unwrap()
        );
        assert_eq!(app.document().source(), MERGED);
    }
    #[test]
    fn the_same_documents_save_receipt_keeps_its_native_owner() {
        let directory = tempfile::tempdir().unwrap();
        let context = egui::Context::default();
        let mut app = fixture(directory.path(), &context);
        stage_merge(&mut app);
        hold_test_save(&mut app, "a");
        let key = app.document().key();
        assert!(
            app.apply_overleaf_rebase("a", &directory.path().join("main.tex"), REMOTE, &context)
                .is_err()
        );
        assert_eq!(app.document().key(), key);
        assert_eq!(app.document().source(), LOCAL);
        assert!(app.overleaf.rebases.contains_key("a"));
    }
    #[test]
    fn cursor_handoff_tracks_insertions_and_deletions_before_unicode_selection() {
        let owner = tiptoptyp_core::document::WindowSessionId::new(42);
        for (before, after, primary, secondary, expected) in [
            ("α😀 target\n", "added\nα😀 target\n", 3, 9, [9, 15]),
            ("α😀 target", "α😀 inserted target", 3, 9, [12, 18]),
            ("α😀 inserted target", "α😀 target", 12, 18, [3, 9]),
            ("remove\nα😀 target\n", "α😀 target\n", 10, 16, [3, 9]),
        ] {
            let path = PathBuf::from("/tmp/cursor.tex");
            let mut native = DocumentSession::new(owner, "", DocumentKind::Tex);
            native
                .replace_loaded(before.into(), path.clone(), DocumentKind::Tex, None)
                .unwrap();
            let cursor = CCursorRange {
                primary: CCursor::new(primary),
                secondary: CCursor::new(secondary),
                h_pos: Some(12.0),
            };
            let prepared = prepare_native(1, &native, &path, after, after, cursor).unwrap();
            assert_eq!(
                [
                    prepared.cursor.primary.index.0,
                    prepared.cursor.secondary.index.0
                ],
                expected
            );
            assert_eq!(prepared.cursor.h_pos, Some(12.0));
            assert_eq!(prepared.update.source(), after);
        }
    }
    #[test]
    fn generated_remote_splices_preserve_selection_affinity_and_native_ownership() {
        // Distinct source/replacement alphabets make the intended edit unique:
        // the oracle knows the splice, without reimplementing a source diff.
        let owner = tiptoptyp_core::document::WindowSessionId::new(42);
        let path = PathBuf::from("/tmp/generated-cursor.tex");
        for before in ["", "abc", "α😀β文γ🦀δ", "e\u{301}文"] {
            let original = before.chars().collect::<Vec<_>>();
            for start in 0..=original.len() {
                for end in start..=original.len() {
                    for replacement in ["", "Ω", "😁中", "\r\n"] {
                        let mut after = original.clone();
                        after.splice(start..end, replacement.chars());
                        let after = after.iter().collect::<String>();
                        // An empty insertion is a no-op, preserving both endpoints.
                        let expected = |offset: usize| {
                            if offset < start || (offset == start && start != end) {
                                offset
                            } else if offset >= end {
                                offset - (end - start) + replacement.chars().count()
                            } else {
                                start + replacement.chars().count()
                            }
                        };
                        for primary in 0..=original.len() {
                            let secondary = original.len() - primary;
                            let mut native = DocumentSession::new(owner, "", DocumentKind::Tex);
                            native
                                .replace_loaded(
                                    before.into(),
                                    path.clone(),
                                    DocumentKind::Tex,
                                    None,
                                )
                                .unwrap();
                            let key = native.key();
                            let cursor = CCursorRange {
                                primary: CCursor::new(primary),
                                secondary: CCursor::new(secondary),
                                h_pos: Some(12.0),
                            };
                            let prepared =
                                prepare_native(1, &native, &path, &after, &after, cursor).unwrap();
                            assert_eq!(
                                [
                                    prepared.cursor.primary.index.0,
                                    prepared.cursor.secondary.index.0
                                ],
                                [expected(primary), expected(secondary)],
                                "before={before:?}, splice={start}..{end}, replacement={replacement:?}, cursor={cursor:?}"
                            );
                            assert_eq!(prepared.cursor.h_pos, cursor.h_pos);
                            let changed = native
                                .commit_external_change(prepared.update, prepared.cursor)
                                .unwrap();
                            assert_eq!(changed, before != after);
                            assert_eq!(native.key().epoch, key.epoch);
                            assert_eq!(native.source(), &after);
                            assert!(!native.is_dirty());
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn remote_edits_around_previous_eof_preserve_interior_carets() {
        // The text survives, but its old EOF gains a line terminator. A diff
        // that includes terminators in line equality loses this anchor.
        assert_eq!(
            map_canonical_cursors(
                "middle",
                "before\nmiddle\nafter",
                [2, 4].map(ScalarOffset::new)
            ),
            [9, 11].map(ScalarOffset::new)
        );
    }

    #[test]
    fn matched_line_terminators_preserve_later_offsets_and_eof_affinity() {
        for (before, after, offsets, expected) in [
            (
                "p\r\none\r\ntwo\r\ns",
                "PREFIX\none\ntwo\nSUFFIX",
                [9, 10],
                [12, 13],
            ),
            (
                "p\none\ntwo\ns",
                "PREFIX\r\none\r\ntwo\r\nSUFFIX",
                [7, 8],
                [14, 15],
            ),
            ("middle", "before\nmiddle\nafter", [6, 0], [19, 7]),
            ("middle\r\n", "before\nmiddle", [8, 2], [13, 9]),
        ] {
            assert_eq!(
                map_canonical_cursors(before, after, offsets.map(ScalarOffset::new)),
                expected.map(ScalarOffset::new),
                "before={before:?}, after={after:?}, offsets={offsets:?}"
            );
        }
    }

    #[test]
    fn generated_disjoint_remote_changes_keep_positions_inside_unchanged_anchors() {
        // 4,096 independent documents: different prefix/suffix lengths,
        // LF/CRLF conversions, and adding/removing a final newline. The unique
        // anchor survives both changes; construction supplies its position.
        const ANCHOR: &str = "§anchor😀文e\u{301}§";
        for old_ending in ["\n", "\r\n"] {
            for new_ending in ["\n", "\r\n"] {
                for (old_final, new_final) in
                    [(false, false), (false, true), (true, false), (true, true)]
                {
                    for old_prefix in 0..4 {
                        for new_prefix in 0..4 {
                            for old_suffix in 0..4 {
                                for new_suffix in 0..4 {
                                    let rows = |side: &str, count, ending: &str| {
                                        (0..count)
                                            .map(|line| format!("{side}-{line}{ending}"))
                                            .collect::<String>()
                                    };
                                    let old_prefix = rows("old-prefix", old_prefix, old_ending);
                                    let new_prefix = rows("new-prefix", new_prefix, new_ending);
                                    let document =
                                        |prefix: &str,
                                         side,
                                         suffix,
                                         ending: &str,
                                         final_newline: bool| {
                                            let mut source = format!(
                                                "{prefix}{ANCHOR}{ending}{}",
                                                rows(side, suffix, ending)
                                            );
                                            if !final_newline {
                                                source.truncate(source.len() - ending.len());
                                            }
                                            source
                                        };
                                    let before = document(
                                        &old_prefix,
                                        "old-suffix",
                                        old_suffix,
                                        old_ending,
                                        old_final,
                                    );
                                    let after = document(
                                        &new_prefix,
                                        "new-suffix",
                                        new_suffix,
                                        new_ending,
                                        new_final,
                                    );
                                    // Appends legitimately move the trailing
                                    // boundary right; the splice property
                                    // covers it. Here the caret is inside text.
                                    for anchor_offset in 1..ANCHOR.chars().count() {
                                        let old = old_prefix.chars().count() + anchor_offset;
                                        let new = new_prefix.chars().count() + anchor_offset;
                                        assert_eq!(
                                            map_canonical_cursors(
                                                &before,
                                                &after,
                                                [ScalarOffset::new(old); 2]
                                            ),
                                            [ScalarOffset::new(new); 2],
                                            "before={before:?}, after={after:?}, anchor_offset={anchor_offset}"
                                        );
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}
