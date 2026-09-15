//! View-only folding: retain source character counts for editing, undo, search,
//! diagnostics and LSP positions. Hidden rows have zero height and no mesh.
use crate::{document::DocumentKey, editor_features::ContextRegion};
use eframe::egui::{self, Galley};
use std::{collections::BTreeSet, ops::Range, sync::Arc};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct FoldRegion {
    pub(crate) line: usize,
    pub(crate) end_line: usize,
    pub(crate) header: usize,
    pub(crate) hidden_chars: Range<usize>,
    header_byte: usize,
    end_byte: usize,
}

/// Vertical arrow navigation skips concealed rows. Other destinations (find,
/// horizontal navigation, diagnostics) are revealed instead of retargeted.
pub(crate) fn skip_hidden_row(
    galley: &Galley,
    cursor: egui::text::CCursor,
    down: bool,
) -> Option<egui::text::CCursor> {
    let row = galley.layout_from_cursor(cursor).row;
    if galley.rows.get(row)?.size.y > 0.0 {
        return None;
    }
    let next = if down {
        (row + 1..galley.rows.len()).find(|&i| galley.rows[i].size.y > 0.0)
    } else {
        (0..row).rev().find(|&i| galley.rows[i].size.y > 0.0)
    };
    let x = galley.pos_from_cursor(cursor).center().x;
    let destination = next.or_else(|| (0..row).rev().find(|&i| galley.rows[i].size.y > 0.0))?;
    let rect = galley.rows[destination].rect();
    Some(galley.cursor_from_pos(egui::vec2(
        if next.is_some() { x } else { rect.right() },
        rect.center().y,
    )))
}

#[derive(Default)]
pub(crate) struct Folding {
    key: Option<DocumentKey>,
    source: Arc<str>,
    char_count: usize,
    pub(crate) regions: Vec<FoldRegion>,
    collapsed: BTreeSet<usize>,
    marker_width: f32,
    cached: Option<(Arc<Galley>, Arc<Galley>)>,
}

impl Folding {
    /// One wrap width governs a TextEdit galley. Reserve a small suffix lane
    /// while folded, rather than letting a marker clip at a soft-wrap edge.
    pub(crate) fn set_marker_width(&mut self, width: f32) {
        let width = if self.collapsed.is_empty() {
            0.0
        } else {
            width
        };
        if self.marker_width != width {
            self.marker_width = width;
            self.cached = None;
        }
    }

    pub(crate) fn text_wrap_width(&self, available: f32) -> f32 {
        (available - self.marker_width).max(1.0)
    }

    pub(crate) fn prepare(
        &mut self,
        key: DocumentKey,
        source: Arc<str>,
        contexts: &[ContextRegion],
    ) {
        if self.key == Some(key) {
            return;
        }
        let same_document = self
            .key
            .is_some_and(|old| old.owner == key.owner && old.epoch == key.epoch);
        let retained = if same_document && !self.collapsed.is_empty() {
            let prefix = self
                .source
                .bytes()
                .zip(source.bytes())
                .take_while(|(a, b)| a == b)
                .count();
            let suffix = self.source.as_bytes()[prefix..]
                .iter()
                .rev()
                .zip(source.as_bytes()[prefix..].iter().rev())
                .take_while(|(a, b)| a == b)
                .count();
            let old_end = self.source.len() - suffix;
            self.regions
                .iter()
                .filter(|region| self.collapsed.contains(&region.line))
                .filter_map(|region| {
                    if region.end_byte <= prefix {
                        Some(region.header_byte)
                    } else if region.header_byte >= old_end {
                        Some(source.len() - (self.source.len() - region.header_byte))
                    } else {
                        None
                    }
                })
                .collect::<BTreeSet<_>>()
        } else {
            BTreeSet::new()
        };
        let mut starts = vec![(0, 0)];
        for (character, (byte, ch)) in source.char_indices().enumerate() {
            if ch == '\n' {
                starts.push((byte + 1, character + 1));
            }
        }
        let eof = (source.len(), source.chars().count());
        self.char_count = eof.1;
        self.regions.clear();
        for context in contexts {
            let Some(row) = context.rows.first() else {
                continue;
            };
            let line = row.line - 1;
            let end_line = (row.end_line - 1).min(starts.len());
            if end_line <= line + 1 {
                continue;
            }
            let end = starts.get(end_line).copied().unwrap_or(eof);
            self.regions.push(FoldRegion {
                line,
                end_line,
                header: row.char_index,
                hidden_chars: starts[line + 1].1..end.1,
                header_byte: starts[line].0,
                end_byte: end.0,
            });
        }
        self.regions
            .sort_by_key(|r| (r.line, std::cmp::Reverse(r.end_line)));
        self.regions.dedup_by_key(|r| r.line);
        self.collapsed = self
            .regions
            .iter()
            .filter(|r| retained.contains(&r.header_byte))
            .map(|r| r.line)
            .collect();
        self.key = Some(key);
        self.source = source;
        self.cached = None;
    }

    pub(crate) fn is_collapsed(&self, line: usize) -> bool {
        self.collapsed.contains(&line)
    }

    pub(crate) fn expand_all(&mut self) {
        self.collapsed.clear();
        self.cached = None;
    }

    pub(crate) fn toggle(&mut self, line: usize) {
        if !self.collapsed.remove(&line) {
            self.collapsed.insert(line);
        }
        self.cached = None;
    }

    /// Search, keyboard navigation and explicit jumps reveal their destination.
    pub(crate) fn reveal(&mut self, cursor: usize) -> bool {
        let before = self.collapsed.len();
        self.collapsed.retain(|line| {
            !self.regions.iter().any(|r| {
                r.line == *line
                    && (r.hidden_chars.contains(&cursor)
                        || (cursor == self.char_count && r.hidden_chars.end == cursor))
            })
        });
        if before != self.collapsed.len() {
            self.cached = None;
            true
        } else {
            false
        }
    }

    pub(crate) fn layout(&mut self, original: Arc<Galley>) -> Arc<Galley> {
        if self.collapsed.is_empty() {
            return original;
        }
        if let Some((input, output)) = &self.cached
            && Arc::ptr_eq(input, &original)
        {
            return Arc::clone(output);
        }
        // TextEdit may lay out again during an edit, before prepare has seen
        // the new revision. Never apply old line boundaries to new text.
        if original.job.text.as_str() != self.source.as_ref() {
            return original;
        }
        let mut hidden: Vec<Range<usize>> = Vec::new();
        for region in self
            .regions
            .iter()
            .filter(|r| self.collapsed.contains(&r.line))
        {
            if let Some(last) = hidden.last_mut()
                && region.line < last.end
            {
                last.end = last.end.max(region.end_line);
            } else {
                hidden.push(region.line + 1..region.end_line);
            }
        }
        let mut hidden_index = 0;
        let mut galley = (*original).clone();
        let mut line = 0;
        let mut removed_height = 0.0;
        let mut width: f32 = 0.0;
        galley.mesh_bounds = egui::Rect::NOTHING;
        galley.num_vertices = 0;
        galley.num_indices = 0;
        for (index, row) in galley.rows.iter_mut().enumerate() {
            row.pos.y -= removed_height;
            while hidden
                .get(hidden_index)
                .is_some_and(|range| range.end <= line)
            {
                hidden_index += 1;
            }
            if hidden
                .get(hidden_index)
                .is_some_and(|range| range.contains(&line))
            {
                removed_height += original
                    .rows
                    .get(index + 1)
                    .map_or(row.size.y, |next| next.pos.y - original.rows[index].pos.y);
                let row = Arc::make_mut(&mut row.row);
                row.size = egui::Vec2::ZERO;
                row.visuals = Default::default();
            } else {
                if row.ends_with_newline && self.collapsed.contains(&line) {
                    Arc::make_mut(&mut row.row).size.x += self.marker_width;
                }
                width = width.max(row.rect().right());
                galley.mesh_bounds = galley
                    .mesh_bounds
                    .union(row.visuals.mesh_bounds.translate(row.pos.to_vec2()));
                galley.num_vertices += row.visuals.mesh.vertices.len();
                galley.num_indices += row.visuals.mesh.indices.len();
            }
            if row.ends_with_newline {
                line += 1;
            }
        }
        galley.rect.max.y -= removed_height;
        galley.rect.max.x = width;
        let result = Arc::new(galley);
        self.cached = Some((original, Arc::clone(&result)));
        result
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::editor_features::context_regions;
    use tiptoptyp_core::document::WindowSessionId;

    fn prepare(folding: &mut Folding, source: &str, revision: u64) {
        let parsed = typst_syntax::Source::detached(source);
        folding.prepare(
            DocumentKey::new(WindowSessionId::new(1), 0, revision),
            Arc::from(source),
            &context_regions(&parsed, source),
        );
    }

    #[test]
    fn nested_folds_survive_parent_toggle_and_reveal_only_ancestors() {
        let source = "= Outer\n#let f(x) = {\n  [αβ\n   body]\n}\n= Sibling\ntail";
        let mut folding = Folding::default();
        prepare(&mut folding, source, 0);
        folding.toggle(2);
        folding.toggle(1);
        folding.toggle(0);
        folding.toggle(0);
        assert!(folding.is_collapsed(1) && folding.is_collapsed(2));
        let cursor = source[..source.find("body").unwrap()].chars().count();
        assert!(folding.reveal(cursor));
        assert!(!folding.is_collapsed(1) && !folding.is_collapsed(2));
        assert!(!folding.reveal(cursor));
    }

    #[test]
    fn folds_track_unaffected_edits_but_not_document_replacements_or_touched_bodies() {
        let source = "intro\n#let f() = {\n  α\n}\ntail";
        let mut folding = Folding::default();
        prepare(&mut folding, source, 0);
        folding.toggle(1);
        let edited = format!("é\n{source}");
        prepare(&mut folding, &edited, 1);
        assert!(folding.is_collapsed(2));
        prepare(&mut folding, &edited.replace('α', "β"), 2);
        assert!(folding.collapsed.is_empty());
        folding.toggle(2);
        folding.prepare(
            DocumentKey::new(WindowSessionId::new(2), 0, 2),
            Arc::from(source),
            &context_regions(&typst_syntax::Source::detached(source), source),
        );
        assert!(folding.collapsed.is_empty());
    }

    #[test]
    fn folded_layout_preserves_unicode_offsets_wrapping_hit_testing_and_cache() {
        let source = "#let f(x) = {\n  αβγ and a long line that wraps several times\n}\nVisible Ω after fold";
        for width in [90.0, 900.0] {
            let context = egui::Context::default();
            context
                .run_ui(Default::default(), |ui| {
                    let mut folding = Folding::default();
                    prepare(&mut folding, source, 0);
                    folding.toggle(0);
                    let original = ui.painter().layout(
                        source.into(),
                        egui::FontId::monospace(14.0),
                        egui::Color32::WHITE,
                        width,
                    );
                    let folded = folding.layout(Arc::clone(&original));
                    assert_eq!(folded.text(), source);
                    assert_eq!(folded.end().index.0, source.chars().count());
                    assert!(folded.size().y < original.size().y);
                    assert!(folded.num_vertices < original.num_vertices);
                    let cursor = source[..source.find("Visible").unwrap()].chars().count();
                    let visible = folded.pos_from_cursor(egui::text::CCursor::new(cursor));
                    assert_eq!(
                        folded.cursor_from_pos(visible.center().to_vec2()).index.0,
                        cursor
                    );
                    assert!(Arc::ptr_eq(&folded, &folding.layout(Arc::clone(&original))));
                    assert!(folding.reveal(cursor - 2));
                    assert!(Arc::ptr_eq(
                        &original,
                        &folding.layout(Arc::clone(&original))
                    ));
                })
                .drop_without_applying_deltas();
        }
    }

    #[test]
    fn edited_text_never_receives_stale_fold_boundaries() {
        let context = egui::Context::default();
        context
            .run_ui(Default::default(), |ui| {
                let mut folding = Folding::default();
                prepare(&mut folding, "= Title\nbody\n", 0);
                folding.toggle(0);
                let edited = ui.painter().layout_no_wrap(
                    "= Title\nnew\nbody\n".into(),
                    egui::FontId::monospace(14.0),
                    egui::Color32::WHITE,
                );
                assert!(Arc::ptr_eq(&edited, &folding.layout(Arc::clone(&edited))));
            })
            .drop_without_applying_deltas();
    }

    #[test]
    fn folded_suffix_fits_after_wrapping_and_vertical_navigation_skips_hidden_rows() {
        let source = "#let long_name(argument, another) = {\n  αβ\n}\nvisible\n";
        egui::Context::default()
            .run_ui(Default::default(), |ui| {
                let mut folding = Folding::default();
                prepare(&mut folding, source, 0);
                folding.toggle(0);
                folding.set_marker_width(30.0);
                let galley = ui.painter().layout(
                    source.into(),
                    egui::FontId::monospace(14.0),
                    egui::Color32::WHITE,
                    folding.text_wrap_width(160.0),
                );
                let folded = folding.layout(galley);
                let header_end = folded
                    .rows
                    .iter()
                    .position(|r| r.ends_with_newline)
                    .unwrap();
                assert!(header_end > 0, "exercise a wrapped header");
                assert!(folded.rows[header_end].rect().right() <= 160.5);
                let body = source[..source.find("αβ").unwrap()].chars().count();
                let down = skip_hidden_row(&folded, egui::text::CCursor::new(body), true).unwrap();
                let visible = source[..source.find("visible").unwrap()].chars().count();
                assert!(down.index.0 >= visible);
                let up = skip_hidden_row(&folded, egui::text::CCursor::new(body), false).unwrap();
                assert!(up.index.0 < source.find('\n').unwrap());
                assert!(folding.is_collapsed(0));
            })
            .drop_without_applying_deltas();
    }

    #[test]
    fn end_of_file_destinations_and_hiding_line_numbers_reveal_folded_text() {
        let source = "= Header\nαβ";
        let mut folding = Folding::default();
        prepare(&mut folding, source, 0);
        folding.toggle(0);
        assert!(folding.reveal(source.chars().count()));
        folding.toggle(0);
        folding.expand_all();
        assert!(!folding.is_collapsed(0));
    }

    #[test]
    #[ignore = "manual optimized folding microbenchmark; no timing threshold in CI"]
    fn folding_large_document_measurement() {
        use std::{hint::black_box, time::Instant};
        let source = (0..500)
            .map(|i| {
                format!(
                    "#let f{i}(x) = {{\n{}\n  x\n}}\n",
                    "  // Unicode αβ and long source lines\n".repeat(12)
                )
            })
            .collect::<String>();
        let start = Instant::now();
        let mut folding = Folding::default();
        prepare(&mut folding, &source, 0);
        let index_time = start.elapsed();
        let lines = folding.regions.iter().map(|r| r.line).collect::<Vec<_>>();
        for line in lines {
            folding.toggle(line);
        }
        folding.set_marker_width(30.0);
        egui::Context::default().run_ui(Default::default(), |ui| {
            let original = ui.painter().layout(source.clone(), egui::FontId::monospace(14.0), egui::Color32::WHITE, folding.text_wrap_width(600.0));
            let start = Instant::now();
            let projected = folding.layout(Arc::clone(&original));
            let cold = start.elapsed();
            let start = Instant::now();
            for _ in 0..10_000 { black_box(folding.layout(Arc::clone(&original))); }
            eprintln!("folding: bytes={} source_rows={} visible_rows={} index_us={} cold_projection_us={} cached_mean_ns={}", source.len(), original.rows.len(), projected.rows.iter().filter(|r| r.size.y > 0.0).count(), index_time.as_micros(), cold.as_micros(), start.elapsed().as_nanos() / 10_000);
        }).drop_without_applying_deltas();
    }
}
