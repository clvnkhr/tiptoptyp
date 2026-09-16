//! Preflight canonical edits as one transaction, then return displayed text.
//! Navigation maps alone are insufficient: replacement payloads are canonical.
use super::{Document, DocumentKey, Error, Projection};
use tiptoptyp_core::text::{AppliedTextEdits, LspTextEdit, ScalarOffset, apply_text_edits};

impl<C> Document<C> {
    pub fn prepare_canonical_edits(
        &self,
        key: DocumentKey,
        edits: &[LspTextEdit],
        editor_cursors: [ScalarOffset; 2],
    ) -> Result<AppliedTextEdits, Error> {
        if key != self.editor.key() {
            return Err(Error::StaleSnapshot);
        }
        if self.active.is_none() {
            return apply_text_edits(self.editor.source(), edits, editor_cursors)
                .map_err(Error::ServiceEdit);
        }
        let snapshot = self.canonical_snapshot()?;
        let cursors = editor_cursors.map(|cursor| snapshot.canonical_scalar_cursor(cursor));
        let [Some(start), Some(end)] = cursors else {
            return Err(Error::ServiceEdit(
                "Editor cursor is outside the document".into(),
            ));
        };
        let applied =
            apply_text_edits(snapshot.source(), edits, [start, end]).map_err(Error::ServiceEdit)?;
        self.project_canonical_change(key, applied)
    }

    /// Accept a complete canonical replacement (including completion/snippet
    /// expansion) with cursor offsets in that replacement's scalar domain.
    /// The caller commits returned text/cursors as one ordinary editor edit.
    pub fn project_canonical_change(
        &self,
        key: DocumentKey,
        applied: AppliedTextEdits,
    ) -> Result<AppliedTextEdits, Error> {
        if key != self.editor.key() {
            return Err(Error::StaleSnapshot);
        }
        let Some(active) = &self.active else {
            return Ok(applied);
        };
        let current = self.canonical_snapshot()?;
        if current.source() == applied.text {
            let cursors = applied
                .mapped_offsets
                .map(|cursor| current.editor_scalar_cursor(cursor));
            let [Some(start), Some(end)] = cursors else {
                return Err(Error::ServiceEdit(
                    "Service cursor is outside the document".into(),
                ));
            };
            return Ok(AppliedTextEdits {
                text: current.editor_source().into(),
                mapped_offsets: [start, end],
            });
        }
        let projected = Projection::open(&applied.text, active.config.clone())?;
        let map = projected.view();
        // Undo owns displayed text. Accept only changes fully representable in
        // that text under the current spelling anchor, so undo/save cannot lose
        // invisible formatter edits or silently substitute native math.
        if active.projection.encode(map.output())?.output() != applied.text {
            return Err(Error::UnrepresentableEdit);
        }
        let cursors = applied.mapped_offsets.map(|cursor| {
            let byte = applied
                .text
                .char_indices()
                .map(|(byte, _)| byte)
                .chain([applied.text.len()])
                .nth(cursor.get())?;
            let byte = map.input_to_output(byte)?;
            Some(ScalarOffset::new(map.output()[..byte].chars().count()))
        });
        let [Some(start), Some(end)] = cursors else {
            return Err(Error::ServiceEdit(
                "Service cursor is outside the document".into(),
            ));
        };
        Ok(AppliedTextEdits {
            text: map.output().into(),
            mapped_offsets: [start, end],
        })
    }
}
