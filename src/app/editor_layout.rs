//! Share logical and sticky row indexes while the final folded galley is unchanged.
use eframe::egui::{
    Galley, Rect,
    epaint::text::cursor::LayoutCursor,
    text::{CCursor, CharIndex},
};
use std::{ops::Range, sync::Arc};

#[derive(Clone)]
pub(super) struct Rows {
    pub logical: Arc<[Range<usize>]>,
    pub cursor: Arc<CursorIndex>,
    /// Galley-relative tops; apply its current paint origin when querying.
    pub sticky: Arc<[super::StickyContextScrollLine]>,
}

// Scalar row ends are monotone even through wrapping/folding. Keep this next
// to the already-cached final galley, rather than scanning all preceding rows
// every time a visible caret, popup or effect needs a screen coordinate.
#[derive(Clone, Copy)]
struct CursorRowEnd {
    chars: usize,
    newline: bool,
}

pub(super) struct CursorIndex {
    rows: Box<[CursorRowEnd]>,
}

impl CursorIndex {
    pub(super) fn new(galley: &Galley) -> Self {
        let mut chars = 0;
        let rows = galley
            .rows
            .iter()
            .map(|row| {
                let end = chars + row.char_count_excluding_newline().0;
                chars += row.char_count_including_newline().0;
                CursorRowEnd {
                    chars: end,
                    newline: row.ends_with_newline,
                }
            })
            .collect();
        Self { rows }
    }

    pub(super) fn layout(&self, galley: &Galley, cursor: CCursor) -> LayoutCursor {
        debug_assert_eq!(self.rows.len(), galley.rows.len());
        let index = cursor.index.0;
        let row = self.rows.partition_point(|end| {
            end.chars < index || (end.chars == index && cursor.prefer_next_row && !end.newline)
        });
        if let Some(current) = self.rows.get(row) {
            let start = row.checked_sub(1).map_or(0, |previous| {
                let previous = self.rows[previous];
                previous.chars + usize::from(previous.newline)
            });
            debug_assert!(index <= current.chars);
            LayoutCursor {
                row,
                column: CharIndex(index.saturating_sub(start)),
            }
        } else if let Some(last) = galley.rows.last() {
            LayoutCursor {
                row: galley.rows.len() - 1,
                column: last.char_count_including_newline(),
            }
        } else {
            LayoutCursor::default()
        }
    }

    pub(super) fn rect(&self, galley: &Galley, cursor: CCursor) -> Rect {
        galley.pos_from_layout_cursor(&self.layout(galley, cursor))
    }
}

#[derive(Default)]
pub(super) struct RowIndex {
    galley: Option<Arc<Galley>>,
    cursor: Option<Arc<CursorIndex>>,
    rows: Option<Rows>,
}

impl RowIndex {
    pub(super) fn cursor_index(&mut self, galley: &Arc<Galley>) -> Arc<CursorIndex> {
        if self
            .galley
            .as_ref()
            .is_none_or(|previous| !Arc::ptr_eq(previous, galley))
        {
            self.galley = Some(Arc::clone(galley));
            self.cursor = Some(Arc::new(CursorIndex::new(galley)));
            self.rows = None;
        }
        Arc::clone(self.cursor.as_ref().expect("galley owns a cursor index"))
    }

    pub fn rows(&mut self, galley: &Arc<Galley>) -> Rows {
        let cursor = self.cursor_index(galley);
        if let Some(rows) = &self.rows {
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
            cursor,
            logical: logical.into(),
            sticky: sticky.into(),
        };
        self.rows = Some(rows.clone());
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
    fn indexed_caret_rectangles_match_wrapped_unicode_empty_and_folded_galleys() {
        let context = egui::Context::default();
        let output = context.run_ui(Default::default(), |ui| {
            for source in ["", "\n", "αβ🙂 x\r\ny\n", "alpha beta gamma delta\n\n中文 words \t end", "a\nb\nc"] {
                for width in [0.0, 5.0, 41.0, f32::INFINITY] {
                    let galley = ui.fonts_mut(|fonts| fonts.layout_job(LayoutJob::simple(
                        source.into(), FontId::monospace(14.0), Color32::WHITE, width)));
                    for folded in [false, true] {
                        let mut galley = (*galley).clone();
                        if folded {
                            for row in galley.rows.iter_mut().skip(1).take(2) {
                                row.pos.y = 0.0;
                                Arc::make_mut(&mut row.row).size.y = 0.0;
                            }
                        }
                        let index = CursorIndex::new(&galley);
                        for at in 0..=source.chars().count() + 2 {
                            for prefer_next_row in [false, true] {
                                let cursor = CCursor { index: CharIndex(at), prefer_next_row };
                                assert_eq!(index.layout(&galley, cursor), galley.layout_from_cursor(cursor),
                                    "{source:?} width={width} at={at} prefer_next={prefer_next_row} folded={folded}");
                                assert_eq!(index.rect(&galley, cursor), galley.pos_from_cursor(cursor),
                                    "{source:?} width={width} at={at} prefer_next={prefer_next_row} folded={folded}");
                            }
                        }
                    }
                }
            }
        });
        output.drop_without_applying_deltas();
    }

    #[test]
    fn cursor_only_views_reuse_geometry_without_building_unneeded_line_and_sticky_indexes() {
        let context = egui::Context::default();
        let mut cache = RowIndex::default();
        context
            .run_ui(Default::default(), |ui| {
                let galley = ui.fonts_mut(|fonts| fonts.layout_job(job()));
                let first = cache.cursor_index(&galley);
                assert!(cache.rows.is_none());
                assert!(Arc::ptr_eq(&first, &cache.cursor_index(&galley)));
                let full = cache.rows(&galley);
                assert!(Arc::ptr_eq(&first, &full.cursor));
                let changed = ui.fonts_mut(|fonts| {
                    let mut text = job();
                    text.wrap.max_width = 90.0;
                    fonts.layout_job(text)
                });
                assert!(!Arc::ptr_eq(&first, &cache.cursor_index(&changed)));
                assert!(cache.rows.is_none());
            })
            .drop_without_applying_deltas();
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
            assert!(Arc::ptr_eq(&rows.cursor, &reused.cursor));
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
            assert!(!Arc::ptr_eq(&rows.cursor, &changed.cursor));
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
