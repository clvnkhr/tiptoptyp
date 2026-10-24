//! Rebase native editor ownership only after the authoritative working-copy
//! write succeeds. The merged draft remains distinct from that saved baseline.
use super::*;
use crate::overleaf::ConflictReview;
use tiptoptyp::mitex_document::CanonicalSnapshot;
use tiptoptyp_core::text::{
    AppliedTextEdits, LspPosition, LspRange, LspTextEdit, apply_text_edits,
};

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
struct PreparedNative {
    tab: u64,
    document: DocumentSession,
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
        if document.conflict.is_some() || document.baseline != remote {
            return Ok(());
        }
        let Some(entry) = connection.model.tree().and_then(|tree| tree.entry(id)) else {
            return Ok(());
        };
        let path = connection.root.join(&entry.path);
        let source = document.source().to_owned();
        let epoch = connection.model.epoch();
        let session = connection.hydration.session;
        let mut targets = Vec::new();
        let mut error = None;
        for tab in self.overleaf_native_tabs(id, &path) {
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
            || document.conflict.is_some()
        {
            // A newer generation owns any replacement intent. Do not retire it
            // because an older worker finished in the meantime.
            return Ok(false);
        }
        if self
            .overleaf
            .pending
            .as_ref()
            .is_some_and(|save| save.id == id)
            || self.overleaf.save_entity.as_deref() == Some(id)
            || self
                .pending_save
                .as_ref()
                .is_some_and(|save| intent.targets.iter().any(|target| target.tab == save.tab))
        {
            return Err(
                "Wait for the outstanding save before reconciling Overleaf changes.".into(),
            );
        }
        let mut prepared = Vec::new();
        let bound: BTreeSet<_> = self.overleaf_native_tabs(id, path).into_iter().collect();
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
        if source.len() > crate::overleaf::MAX_SOURCE_BYTES {
            return Err("The Overleaf document is too large to edit.".into());
        }
        let connection = self
            .overleaf
            .connection
            .as_ref()
            .ok_or("Reconnect to Overleaf before choosing a version.")?;
        if !connection.online || connection.hydration.pending {
            return Err("Wait for Overleaf to finish syncing before choosing a version.".into());
        }
        if self.overleaf.pending.is_some()
            || self.overleaf.save_active
            || self.document_workflow.save_in_flight
            || self.save_job.is_running()
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
        let tabs = self.overleaf_native_tabs(&review.id, &path);
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
        self.overleaf
            .connection
            .as_mut()
            .unwrap()
            .model
            .resolve_conflict(review, source)?;
        let changed = self.commit_overleaf_native(prepared, context);
        self.overleaf.rebases.remove(&review.id);
        self.overleaf.blocked.remove(&review.id);
        if self.overleaf.blocked.is_empty()
            && self.overleaf.error.as_deref() == Some(CONFLICT_ERROR)
        {
            self.overleaf.error = None;
        }
        self.overleaf.queue(MirrorWork {
            entry,
            data: MirrorData::Document(review.remote.clone()),
            save: None,
        });
        self.sync_parked_tinymist();
        self.schedule_project_index();
        if changed {
            self.reset_document_services();
        }
        if changed || self.designated_preview_path().is_some() {
            self.schedule_compile_now();
        }
        context.request_repaint();
        Ok(())
    }

    fn overleaf_native_tabs(&self, id: &str, path: &Path) -> Vec<u64> {
        self.tabs
            .ids()
            .filter(|&tab| {
                self.document_for_tab(tab)
                    .and_then(|document| document.path().as_deref())
                    == Some(path)
                    || self.overleaf_tab_matches(tab, id)
            })
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
        for mut prepared in prepared {
            let tab = prepared.tab;
            // This function only runs synchronously after preflighting all
            // owners, so no tab or document can change between these steps.
            let _ = prepared.document.take_edit();
            let dirty = prepared.document.is_dirty();
            *self.tabs.document_mut(tab).expect("prepared native owner") = prepared.document;
            let record = self.tabs.pdf_record_mut(Some(tab));
            record.pdf_output.invalidate();
            if let Some(state) = &mut record.editor {
                state.clear_undoer();
                state.cursor.set_char_range(Some(prepared.cursor));
            }
            if self.tabs.active_id() == Some(tab) {
                self.store_editor_cursor(context, prepared.cursor);
                self.pending_editor_selection = None;
                active_changed = true;
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
    let current = native
        .canonical_snapshot()
        .map_err(|failure| failure.to_string())?;
    let offsets = [cursor.primary.index.0, cursor.secondary.index.0].map(|offset| {
        current
            .canonical_scalar_cursor(ScalarOffset::new(offset))
            .unwrap_or(ScalarOffset::new(0))
    });
    let mapped = canonical_transition(current.source(), source, offsets)?;
    // Use a separate owner-local candidate so projection failures are atomic
    // even when multiple native tabs refer to this entity.
    let mut document = DocumentSession::new(native.key().owner, "", native.kind());
    document
        .replace_loaded(
            remote.into(),
            path.into(),
            native.kind(),
            Some(fingerprint(remote.as_bytes())),
        )
        .map_err(|failure| failure.to_string())?;
    if let Some(config) = native.config() {
        document
            .enable(config.clone())
            .map_err(|failure| failure.to_string())?;
    }
    document.reactivate_after(native.key());
    let applied = document
        .project_canonical_change(document.key(), mapped)
        .map_err(|failure| failure.to_string())?;
    let cursor = CCursorRange {
        primary: CCursor::new(applied.mapped_offsets[0].get()),
        secondary: CCursor::new(applied.mapped_offsets[1].get()),
        h_pos: cursor.h_pos,
    };
    if source != remote {
        let undo_cursor = clamp_cursor_range(cursor, document.source().chars().count());
        document.edit(undo_cursor, |text| *text = applied.text);
    }
    Ok(PreparedNative {
        tab,
        document,
        cursor,
    })
}

/// Reuse the Git viewer's bounded line alignment, then the editor's canonical
/// edit mapper. Separate changed groups preserve a caret on an unchanged line
/// between edits; scalar trimming limits remapping to edited spans within lines.
fn canonical_transition(
    before: &str,
    after: &str,
    offsets: [ScalarOffset; 2],
) -> Result<AppliedTextEdits, String> {
    const MAX_LINES: usize = 100_000;
    let old: Vec<_> = before.split_inclusive('\n').take(MAX_LINES + 1).collect();
    let new: Vec<_> = after.split_inclusive('\n').take(MAX_LINES + 1).collect();
    let mut edits = Vec::new();
    if old.len() > MAX_LINES || new.len() > MAX_LINES {
        if let Some(edit) = cursor_edit(before, after, 0) {
            edits.push(edit);
        }
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
                .chain(std::iter::once(lines.iter().map(|line| line.len()).sum()))
                .collect::<Vec<_>>()
        };
        let old_starts = starts(&old);
        let new_starts = starts(&new);
        let pairs = crate::git::editor::document_diff::align_lines(before, after);
        let (mut a, mut b, mut row) = (0, 0, 0);
        while row < pairs.len() {
            if !pairs[row].changed {
                a += usize::from(pairs[row].old.is_some());
                b += usize::from(pairs[row].new.is_some());
                row += 1;
                continue;
            }
            let (start_a, start_b) = (a, b);
            while row < pairs.len() && pairs[row].changed {
                a += usize::from(pairs[row].old.is_some());
                b += usize::from(pairs[row].new.is_some());
                row += 1;
            }
            if let Some(edit) = cursor_edit(
                &before[old_starts[start_a]..old_starts[a]],
                &after[new_starts[start_b]..new_starts[b]],
                start_a as u32,
            ) {
                edits.push(edit);
            }
        }
    }
    let applied = apply_text_edits(before, &edits, offsets)?;
    if applied.text != after {
        return Err("The Overleaf cursor mapping did not reproduce the selected version.".into());
    }
    Ok(applied)
}

fn cursor_edit(before: &str, after: &str, first_line: u32) -> Option<LspTextEdit> {
    if before == after {
        return None;
    }
    let mut prefix: usize = before
        .chars()
        .zip(after.chars())
        .take_while(|(a, b)| a == b)
        .map(|(character, _)| character.len_utf8())
        .sum();
    let mut suffix: usize = before[prefix..]
        .chars()
        .rev()
        .zip(after[prefix..].chars().rev())
        .take_while(|(a, b)| a == b)
        .map(|(character, _)| character.len_utf8())
        .sum();
    // LSP positions cannot address the interior of a CRLF pair. Include the
    // complete line break in a replacement instead of splitting its two bytes.
    if prefix > 0
        && before.as_bytes()[prefix - 1] == b'\r'
        && before.as_bytes().get(prefix) == Some(&b'\n')
    {
        prefix -= 1;
    }
    let end = before.len() - suffix;
    if end > 0 && before.as_bytes()[end - 1] == b'\r' && before.as_bytes().get(end) == Some(&b'\n')
    {
        suffix -= 1;
    }
    let position = |byte: usize| {
        let local =
            lsp_position_at_scalar(before, ScalarOffset::new(before[..byte].chars().count()));
        LspPosition::new(first_line + local.line.get(), local.character.get())
    };
    Some(LspTextEdit {
        range: LspRange {
            start: position(prefix),
            end: position(before.len() - suffix),
        },
        new_text: after[prefix..after.len() - suffix].into(),
    })
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
            online: true,
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
            assert!(app.overleaf.pending.is_none());
            assert!(
                app.apply_overleaf_review(&context, &review, source)
                    .is_err()
            );
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
                0 => app.overleaf.connection.as_mut().unwrap().online = false,
                1 => app.overleaf.connection.as_mut().unwrap().hydration.pending = true,
                _ => app.document_workflow.save_in_flight = true,
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
        app.overleaf.save_entity = Some("another-document".into());
        app.overleaf.save_active = true;
        app.document_workflow.save_in_flight = true;
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
        app.overleaf.save_entity = Some("a".into());
        app.overleaf.save_active = true;
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
            assert_eq!(&prepared.document.source()[..], after);
        }
    }
    #[test]
    fn unchanged_line_keeps_its_logical_selection_between_two_remote_changes() {
        let before = "first\n文😀 target\nlast\n";
        let after = "inserted\nFIRST\n文😀 target\nLAST\n";
        let offsets = [9, 6].map(ScalarOffset::new);
        let result = canonical_transition(before, after, offsets).unwrap();
        assert_eq!(result.mapped_offsets.map(ScalarOffset::get), [18, 15]);
        assert_eq!(result.text, after);
    }
    #[test]
    fn cursor_mapping_retains_crlf_and_no_final_newline() {
        for (before, after) in [
            ("first\r\n文😀 target", "added\r\nfirst\r\n文😀 target"),
            ("first\r\n文😀 target", "first\n文😀 target"),
            ("", "first\r\n"),
        ] {
            let end = before.chars().count();
            let result = canonical_transition(before, after, [ScalarOffset::new(end); 2]).unwrap();
            assert_eq!(result.text, after);
            assert_eq!(
                result.mapped_offsets,
                [ScalarOffset::new(after.chars().count()); 2]
            );
        }
    }
}
