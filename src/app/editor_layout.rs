//! Share logical and sticky row indexes while the final folded galley is unchanged.
use eframe::egui::Galley;
use std::{ops::Range, sync::Arc};

#[derive(Clone)]
pub(super) struct Rows {
    pub logical: Arc<[Range<usize>]>,
    /// Galley-relative tops; apply its current paint origin when querying.
    pub sticky: Arc<[super::StickyContextScrollLine]>,
}

#[derive(Default)]
pub(super) struct RowIndex {
    rows: Option<(Arc<Galley>, Rows)>,
}

impl RowIndex {
    pub fn rows(&mut self, galley: &Arc<Galley>) -> Rows {
        if let Some((cached_galley, rows)) = &self.rows
            && Arc::ptr_eq(cached_galley, galley)
        {
            return rows.clone();
        }
        // Index the final folded galley: hidden row heights and y positions
        // must not be inferred from the unfurled text layout.
        let logical = super::logical_line_row_ranges(&galley.rows);
        let sticky = super::sticky_context_scroll_lines(
            &logical,
            |index| galley.rows.get(index).map(|row| row.rect().top()),
            |index| {
                galley
                    .rows
                    .get(index)
                    .map(|row| row.char_count_including_newline().0)
            },
            |index| galley.rows.get(index).map(|row| row.ends_with_newline),
        )
        .unwrap_or_default();
        let rows = Rows {
            logical: logical.into(),
            sticky: sticky.into(),
        };
        self.rows = Some((Arc::clone(galley), rows.clone()));
        rows
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use eframe::egui::{self, Color32, FontId, text::LayoutJob};
    fn job() -> LayoutJob {
        LayoutJob::simple(
            "alpha beta gamma delta epsilon zeta eta theta iota kappa\nsecond line".into(),
            FontId::monospace(14.0),
            Color32::WHITE,
            f32::INFINITY,
        )
    }
    #[test]
    fn row_indexes_reuse_the_final_galley_and_keep_sticky_tops_relative() {
        let context = egui::Context::default();
        let mut cache = RowIndex::default();
        let output = context.run_ui(Default::default(), |ui| {
            let galley = ui.fonts_mut(|fonts| {
                let mut text = job();
                text.wrap.max_width = 130.0;
                fonts.layout_job(text)
            });
            let rows = cache.rows(&galley);
            let reused = cache.rows(&galley);
            assert!(Arc::ptr_eq(&rows.logical, &reused.logical));
            assert!(Arc::ptr_eq(&rows.sticky, &reused.sticky));
            assert_eq!(rows.logical.len(), 2);
            assert!(
                rows.logical[0].len() > 1,
                "fixture must soft-wrap the first logical line"
            );
            for (logical, sticky) in rows.logical.iter().zip(rows.sticky.iter()) {
                assert_eq!(sticky.top, galley.rows[logical.start].rect().top());
            }
            let second = rows.logical[1].start;
            let mut folded = (*galley).clone();
            for row in &mut folded.rows[second..] {
                row.pos.y -= 7.0;
            }
            let changed = cache.rows(&Arc::new(folded));
            assert!(!Arc::ptr_eq(&rows.logical, &changed.logical));
            assert!(!Arc::ptr_eq(&rows.sticky, &changed.sticky));
            assert_eq!(rows.logical.as_ref(), changed.logical.as_ref());
            assert_eq!(changed.sticky[1].top, rows.sticky[1].top - 7.0);
            assert_eq!(changed.sticky[1].anchor, rows.sticky[1].anchor);
            // Moving the editor does not rebuild the row index: callers query
            // a relative boundary instead of storing absolute screen positions.
            let anchor =
                super::super::sticky_context_scroll_anchor(&changed.sticky, changed.sticky[1].top);
            assert_eq!(anchor, Some(changed.sticky[1].anchor));
        });
        output.drop_without_applying_deltas();
    }
}

#[cfg(test)]
mod perf_tests;
