//! Visible editor-surface geometry and retained overlay contracts.

use eframe::egui::{Pos2, Rect};

/// A logical row reaches both visible edges, regardless of the text's own
/// width or scroll offset. Folded/offscreen rows produce no painted area.
pub(super) fn line_background_rect(viewport: Rect, row: Rect) -> Rect {
    Rect::from_min_max(
        Pos2::new(viewport.left(), row.top()),
        Pos2::new(viewport.right(), row.bottom()),
    )
    .intersect(viewport)
}

/// Sticky rows cover the root editor's border. Redraw its three covered edges
/// on the overlay's own layer; the bottom remains the normal sticky separator.
pub(super) fn sticky_border_segments(overlay: Rect, viewport: Rect, width: f32) -> [[Pos2; 2]; 3] {
    let visible = overlay.intersect(viewport);
    let inset = (width * 0.5)
        .max(0.0)
        .min(visible.width().max(0.0) * 0.5)
        .min(visible.height().max(0.0) * 0.5);
    let left = visible.left() + inset;
    let right = visible.right() - inset;
    let top = visible.top() + inset;
    let bottom = visible.bottom() - inset;
    [
        [Pos2::new(left, top), Pos2::new(right, top)],
        [Pos2::new(left, top), Pos2::new(left, bottom)],
        [Pos2::new(right, top), Pos2::new(right, bottom)],
    ]
}

#[cfg(test)]
mod tests {
    use super::super::*;

    #[test]
    fn row_backgrounds_clip_to_viewport_and_skip_folded_or_offscreen_rows() {
        let viewport = Rect::from_min_size(Pos2::new(12.0, 20.0), Vec2::new(600.0, 180.0));
        for row in [
            Rect::from_min_size(Pos2::new(-100.0, 18.0), Vec2::new(20.0, 10.0)),
            Rect::from_min_size(Pos2::new(400.0, 198.0), Vec2::new(1000.0, 10.0)),
        ] {
            let fill = super::line_background_rect(viewport, row);
            assert!(viewport.contains_rect(fill));
            assert_eq!(fill.x_range(), viewport.x_range());
            assert_eq!(
                fill.y_range(),
                row.y_range().intersection(viewport.y_range())
            );
        }
        for row in [
            Rect::from_min_size(Pos2::new(10.0, 220.0), Vec2::new(80.0, 10.0)),
            Rect::from_min_size(Pos2::new(10.0, 40.0), Vec2::new(80.0, 0.0)),
        ] {
            assert!(!super::line_background_rect(viewport, row).is_positive());
        }
    }

    #[test]
    fn sticky_outline_edges_stay_inside_the_overlay_at_any_scale() {
        let viewport = Rect::from_min_size(Pos2::new(12.0, 20.0), Vec2::new(600.0, 180.0));
        let overlay = Rect::from_min_size(viewport.min, Vec2::new(viewport.width(), 24.0));
        for width in [0.5, 1.0, 2.0] {
            let edges = super::sticky_border_segments(overlay, viewport, width);
            for edge in edges {
                assert!(overlay.contains(edge[0]) && overlay.contains(edge[1]));
            }
            assert_eq!(edges[0][0].y, overlay.top() + width * 0.5);
            assert_eq!(edges[0][1].y, edges[0][0].y);
            assert_eq!(edges[1][0].x, overlay.left() + width * 0.5);
            assert_eq!(edges[2][0].x, overlay.right() - width * 0.5);
            assert_eq!(edges[1][1].y, overlay.bottom() - width * 0.5);
        }
        let tiny = Rect::from_min_size(viewport.min, Vec2::splat(0.25));
        for edge in super::sticky_border_segments(tiny, viewport, 2.0) {
            assert!(tiny.contains(edge[0]) && tiny.contains(edge[1]));
        }
    }

    #[test]
    fn editor_row_fill_reaches_both_visible_edges_for_short_and_scrolled_text() {
        let context = egui::Context::default();
        theme::configure_test_fonts(&context);
        let viewport = Rect::from_min_size(Pos2::new(12.0, 20.0), Vec2::new(600.0, 180.0));
        let mut source = "short\nsecond".to_owned();
        for offset in [0.0, 180.0] {
            context
                .run_ui(Default::default(), |ui| {
                    let mut output = egui::TextEdit::multiline(&mut source)
                        .desired_width(80.0)
                        .frame(egui::Frame::new())
                        .show(ui);
                    let translation =
                        viewport.min - output.response.rect.min - Vec2::new(offset, 0.0);
                    output.response.rect = output.response.rect.translate(translation);
                    output.galley_pos += translation;
                    let rows = logical_line_row_ranges(&output.galley.rows);
                    let mut shapes = Vec::new();
                    push_editor_line_fill(
                        &mut shapes,
                        &output,
                        viewport,
                        &rows,
                        0,
                        Color32::GREEN,
                        None,
                    );
                    let rect = shapes
                        .iter()
                        .find_map(|shape| match shape {
                            egui::Shape::Rect(shape) => Some(shape.rect),
                            _ => None,
                        })
                        .unwrap();
                    assert_eq!(rect.left(), viewport.left());
                    assert_eq!(rect.right(), viewport.right());
                })
                .drop_without_applying_deltas();
        }
    }

    #[test]
    fn sticky_rows_paint_the_editor_outline_above_the_opaque_cover() {
        let context = egui::Context::default();
        theme::configure_test_fonts(&context);
        let viewport = Rect::from_min_size(Pos2::new(12.0, 20.0), Vec2::new(600.0, 180.0));
        let mut source = "= Section\nBody".to_owned();
        for _ in 0..3 {
            let output = context.run_ui(Default::default(), |ui| {
                let output = egui::TextEdit::multiline(&mut source)
                    .desired_width(viewport.width())
                    .frame(egui::Frame::new())
                    .show(ui);
                let snapshot = StickyContextEditorSnapshot {
                    line_rows: logical_line_row_ranges(&output.galley.rows),
                    galley: Arc::clone(&output.galley),
                    galley_pos: Pos2::new(viewport.left() + 40.0, viewport.top() - 40.0),
                    scroll_lines: Vec::new(),
                };
                let rows = vec![StickyContextRow {
                    kind: crate::editor_features::StickyContextKind::Heading { level: 1 },
                    line: 1,
                    char_index: 0,
                    end_line: 100,
                    text: "= Section".to_owned(),
                }];
                show_sticky_context_overlay(
                    ui.ctx(),
                    viewport,
                    None,
                    &rows,
                    &snapshot,
                    false,
                    ui.visuals().selection.stroke,
                );
            });
            let stroke = context.style_of(context.theme()).visuals.selection.stroke;
            let top_edge = output.shapes.iter().any(|clipped| match &clipped.shape {
                egui::Shape::LineSegment {
                    points,
                    stroke: actual,
                } => {
                    *actual == stroke
                        && points[0].y == points[1].y
                        && (points[0].y - viewport.top()).abs() <= stroke.width
                }
                _ => false,
            });
            output.drop_without_applying_deltas();
            if context.cumulative_frame_nr() >= 3 {
                assert!(
                    top_edge,
                    "sticky context cover obscures the code-panel outline"
                );
            }
        }
    }
}
