//! Editor repaint work must scale with the visible viewport, not document length.
use super::*;
use std::cell::Cell;

#[test]
fn status_coordinates_follow_edits_and_clamp_stale_carets_without_source_scans() {
    let directory = tempfile::tempdir().unwrap();
    let context = egui::Context::default();
    let mut app = EditorApp::dormant_for_tests(&context, directory.path().into());
    let editor = source_editor_id(&context);
    for source in ["α🦀\n中\r\n", "short", "\n", ""] {
        app.document_mut().replace_unprojected_untitled(source);
        app.prepare_editor_source_data();
        for at in [0, 1, 2, 3, source.chars().count(), usize::MAX] {
            let mut state = egui::text_edit::TextEditState::default();
            state
                .cursor
                .set_char_range(Some(CCursorRange::one(CCursor::new(at))));
            state.store(&context, editor);
            assert_eq!(
                app.cursor_coordinates(&context),
                Some(line_column_at_char(source, at))
            );
        }
    }
}

#[test]
fn visible_row_selection_matches_full_walk_and_skips_large_folded_runs() {
    let mut rows = Vec::new();
    for line in 0..100_000 {
        // A long collapsed body lies between two groups of visible rows.
        let (y, height) = if line < 400 {
            (line as f32 * 17.0, 17.0)
        } else if line < 99_600 {
            (6800.0, 0.0)
        } else {
            (6800.0 + (line - 99_600) as f32 * 17.0, 17.0)
        };
        rows.push(Rect::from_min_size(
            Pos2::new(0.0, y),
            Vec2::new(30.0, height),
        ));
    }
    for (top, bottom) in [
        (-1.0, 0.0),
        (0.0, 40.0),
        (401.25, 1001.0),
        (6783.0, 6800.0),
        (6800.0, 6840.0),
        (13590.0, 14000.0),
        (15000.0, 17000.0),
        (900.0, 500.0),
    ] {
        let expected: Vec<_> = rows
            .iter()
            .enumerate()
            .filter(|(_, rect)| rect.height() > 0.0 && rect.bottom() >= top && rect.top() <= bottom)
            .map(|(index, _)| index)
            .collect();
        let queries = Cell::new(0);
        let selected: Vec<_> = visible_row_indices(
            &rows,
            |&rect| {
                queries.set(queries.get() + 1);
                rect
            },
            top,
            bottom,
        )
        .collect();
        assert_eq!(selected, expected, "clip {top}..{bottom}");
        assert!(
            queries.get() <= selected.len() + 100,
            "viewport lookup must skip offscreen and folded rows: {} geometry queries for {} visible lines",
            queries.get(),
            selected.len()
        );
    }
    assert!(
        visible_row_indices::<Rect>(&[], |&rect| rect, 0.0, 100.0)
            .next()
            .is_none()
    );
}

#[test]
fn visible_line_numbers_follow_real_wrapping_and_folded_geometry() {
    let source = "#let nested = {\n  α🦀 and a long line repeated to wrap in the narrow viewport\n  another very long line with mathematical notation and ordinary prose\n}\n\nAfter the block\n";
    let parsed = typst_syntax::Source::detached(source);
    let mut folding = crate::folding::Folding::default();
    folding.prepare(
        DocumentKey::new(tiptoptyp_core::document::WindowSessionId::new(1), 0, 0),
        Arc::from(source),
        &crate::editor_features::context_regions(&parsed, source),
    );
    let context = egui::Context::default();
    context
        .run_ui(Default::default(), |ui| {
            let original = ui.painter().layout(
                source.into(),
                egui::FontId::monospace(14.0),
                Color32::WHITE,
                110.0,
            );
            assert!(original.rows.len() > source.lines().count());
            for collapsed in [false, true] {
                if collapsed {
                    folding.collapse_all();
                }
                let galley = folding.layout(Arc::clone(&original));
                let logical = logical_line_row_ranges(&galley.rows);
                for top in [-10.0, 0.0, 16.0, 35.0, 80.5, galley.rect.bottom()] {
                    let bottom = top + 40.0;
                    let actual: Vec<_> = visible_row_indices(
                        &logical,
                        |rows| galley.rows[rows.start].rect(),
                        top,
                        bottom,
                    )
                    .collect();
                    let expected: Vec<_> = logical
                        .iter()
                        .enumerate()
                        .filter(|(_, range)| {
                            let row = &galley.rows[range.start];
                            row.size.y > 0.0
                                && row.rect().bottom() >= top
                                && row.rect().top() <= bottom
                        })
                        .map(|(line, _)| line)
                        .collect();
                    assert_eq!(
                        actual, expected,
                        "collapsed={collapsed}, clip={top}..{bottom}"
                    );
                }
            }
        })
        .drop_without_applying_deltas();
}
