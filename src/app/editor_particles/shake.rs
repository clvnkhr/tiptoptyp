//! Translate only this pass's editor paint, without moving input or scroll geometry.
use eframe::egui::{LayerId, Rect, Shape, Ui, Vec2, ViewportId, layers::ShapeIdx};

const MAX_OFFSET: f32 = 4.0;

pub(in crate::app) struct Marker {
    layer: LayerId,
    viewport: ViewportId,
    pass: u64,
    start: ShapeIdx,
}

pub(in crate::app) struct PaintRange {
    marker: Marker,
    end: ShapeIdx,
}

pub(in crate::app) fn begin(ui: &Ui) -> Marker {
    Marker {
        layer: ui.layer_id(),
        viewport: ui.ctx().viewport_id(),
        pass: ui.ctx().cumulative_pass_nr(),
        start: ui.ctx().graphics(|graphics| {
            graphics
                .get(ui.layer_id())
                .map_or(ShapeIdx(0), |list| list.next_idx())
        }),
    }
}

impl Marker {
    fn belongs_to(&self, ui: &Ui) -> bool {
        self.layer == ui.layer_id()
            && self.viewport == ui.ctx().viewport_id()
            && self.pass == ui.ctx().cumulative_pass_nr()
    }

    pub(in crate::app) fn finish(self, ui: &Ui) -> PaintRange {
        let end = if self.belongs_to(ui) {
            ui.ctx().graphics(|graphics| {
                graphics
                    .get(self.layer)
                    .map_or(self.start, |list| list.next_idx())
            })
        } else {
            self.start
        };
        PaintRange { marker: self, end }
    }
}

impl PaintRange {
    pub(in crate::app) fn apply(self, ui: &Ui, offset: Vec2, viewport: Rect) {
        if !self.marker.belongs_to(ui)
            || !offset.is_finite()
            || offset == Vec2::ZERO
            || offset.abs().max_elem() > MAX_OFFSET
            || !viewport.is_finite()
            || !viewport.is_positive()
        {
            return;
        }
        ui.ctx().graphics_mut(|graphics| {
            let Some(list) = graphics.get_mut(self.marker.layer) else {
                return;
            };
            for index in self.marker.start.0..self.end.0.min(list.next_idx().0) {
                list.mutate_shape(ShapeIdx(index), |clipped| {
                    if translate(&mut clipped.shape, clipped.clip_rect, offset, viewport) {
                        clipped.clip_rect = clipped.clip_rect.intersect(viewport);
                    }
                });
            }
        });
    }
}

// A Shape::Vec shares one clip. Narrowing it after moving only one child
// would crop stationary siblings (for example the old side of a diff).
fn has_visible_child_outside(shape: &Shape, clip: Rect, viewport: Rect) -> bool {
    match shape {
        Shape::Vec(children) => children
            .iter()
            .any(|child| has_visible_child_outside(child, clip, viewport)),
        Shape::Noop => false,
        _ => {
            let visible = shape.visual_bounding_rect().intersect(clip);
            visible.is_positive() && !viewport.contains_rect(visible)
        }
    }
}

fn translate(shape: &mut Shape, clip: Rect, offset: Vec2, viewport: Rect) -> bool {
    if matches!(shape, Shape::Vec(_))
        && !viewport.contains_rect(clip)
        && has_visible_child_outside(shape, clip, viewport)
    {
        return false;
    }
    match shape {
        Shape::Vec(children) => {
            let mut translated = false;
            for child in children {
                translated |= translate(child, clip, offset, viewport);
            }
            return translated;
        }
        // A callback's closure owns its coordinate system; moving its rect is insufficient.
        Shape::Callback(_) | Shape::Noop => return false,
        _ => {}
    }
    let visible = shape.visual_bounding_rect().intersect(clip);
    if !visible.is_positive() || !viewport.intersects(visible) {
        return false;
    }
    match shape {
        // Shape::translate clones and transforms every galley row even at scale 1.
        // Changing only this paint origin preserves the cached document layout.
        Shape::Text(text) => text.pos += offset,
        _ => shape.translate(offset),
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use eframe::egui::{self, Color32, FontId, Pos2, Stroke, epaint::TextShape};
    use std::sync::Arc;

    fn viewport() -> Rect {
        Rect::from_min_max(egui::pos2(30.0, 40.0), egui::pos2(330.0, 240.0))
    }

    fn screen() -> egui::RawInput {
        egui::RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, egui::vec2(400.0, 320.0))),
            ..Default::default()
        }
    }

    #[test]
    fn translating_nested_document_text_preserves_the_galley_and_every_row() {
        let context = egui::Context::default();
        let output = context.run_ui(screen(), |ui| {
            let galley = ui.painter().layout_no_wrap(
                "  let value = substantial_document_content;\n".repeat(4096),
                FontId::monospace(14.0),
                Color32::WHITE,
            );
            let origin = egui::pos2(40.0, 50.0);
            let mut shape = Shape::Vec(vec![Shape::Vec(vec![Shape::Text(TextShape::new(
                origin,
                galley.clone(),
                Color32::WHITE,
            ))])]);
            let offset = egui::vec2(3.0, -2.0);
            translate(&mut shape, viewport(), offset, viewport());
            let Shape::Vec(outer) = shape else {
                panic!("outer group missing")
            };
            let Shape::Vec(inner) = &outer[0] else {
                panic!("inner group missing")
            };
            let Shape::Text(text) = &inner[0] else {
                panic!("text missing")
            };
            assert_eq!(text.pos, origin + offset);
            assert!(Arc::ptr_eq(&galley, &text.galley));
            for (original, shaken) in galley.rows.iter().zip(&text.galley.rows) {
                assert!(Arc::ptr_eq(&original.row, &shaken.row));
            }
        });
        output.drop_without_applying_deltas();
    }

    #[test]
    fn paint_range_preserves_clips_widgets_and_shapes_outside_its_range() {
        let context = egui::Context::default();
        let output = context.run_ui(screen(), |ui| {
            let painter = ui.painter().with_clip_rect(viewport().shrink(8.0));
            let original_rect = Rect::from_min_size(egui::pos2(70.0, 80.0), egui::vec2(30.0, 20.0));
            let before = painter.rect_filled(original_rect, 0.0, Color32::RED);
            let marker = begin(ui);
            let widget = ui.interact(
                original_rect,
                ui.id().with("stationary-widget"),
                egui::Sense::click(),
            );
            let inside = painter.rect_filled(original_rect, 0.0, Color32::GREEN);
            let range = marker.finish(ui);
            let after = painter.rect_filled(original_rect, 0.0, Color32::BLUE);
            range.apply(ui, egui::vec2(3.0, -2.0), viewport());
            assert_eq!(widget.rect, original_rect);
            assert_eq!(widget.interact_rect, original_rect);
            ui.ctx().graphics(|graphics| {
                let list = graphics.get(ui.layer_id()).unwrap();
                let shapes = list.all_entries().collect::<Vec<_>>();
                for index in [before, inside, after] {
                    assert_eq!(shapes[index.0].clip_rect, painter.clip_rect());
                    let Shape::Rect(rect) = &shapes[index.0].shape else {
                        panic!("rectangle missing")
                    };
                    assert_eq!(
                        rect.rect,
                        if index == inside {
                            original_rect.translate(egui::vec2(3.0, -2.0))
                        } else {
                            original_rect
                        }
                    );
                }
            });
        });
        output.drop_without_applying_deltas();
    }

    #[test]
    fn broad_clips_shrink_to_the_viewport_but_outside_shapes_are_untouched() {
        let context = egui::Context::default();
        let output = context.run_ui(screen(), |ui| {
            let clip = viewport().expand(50.0).intersect(ui.clip_rect());
            let painter = ui.painter().with_clip_rect(clip);
            let offset = egui::vec2(4.0, 0.0);
            for (rect, intersects) in [
                (
                    Rect::from_min_max(egui::pos2(20.0, 90.0), egui::pos2(60.0, 110.0)),
                    true,
                ),
                (
                    Rect::from_min_max(egui::pos2(310.0, 90.0), egui::pos2(329.0, 110.0)),
                    true,
                ),
                (
                    Rect::from_min_max(egui::pos2(45.0, 20.0), egui::pos2(60.0, 35.0)),
                    false,
                ),
            ] {
                let marker = begin(ui);
                let index = painter.rect_filled(rect, 0.0, Color32::RED);
                marker.finish(ui).apply(ui, offset, viewport());
                ui.ctx().graphics(|graphics| {
                    let list = graphics.get(ui.layer_id()).unwrap();
                    let shape = list.all_entries().nth(index.0).unwrap();
                    assert_eq!(
                        shape.clip_rect,
                        if intersects {
                            clip.intersect(viewport())
                        } else {
                            clip
                        }
                    );
                    assert!(
                        clip.contains_rect(shape.clip_rect),
                        "shake must never widen a clip"
                    );
                    assert_eq!(
                        shape.shape,
                        Shape::rect_filled(
                            if intersects {
                                rect.translate(offset)
                            } else {
                                rect
                            },
                            0.0,
                            Color32::RED
                        )
                    );
                });
            }
        });
        output.drop_without_applying_deltas();
    }

    #[test]
    fn a_shared_diff_group_cannot_crop_stationary_siblings_in_the_other_column() {
        let context = egui::Context::default();
        let output = context.run_ui(screen(), |ui| {
            let body = viewport();
            let right = Rect::from_min_max(egui::pos2(180.0, body.top()), body.right_bottom());
            let old = Rect::from_min_size(egui::pos2(40.0, 80.0), egui::vec2(90.0, 24.0));
            let new = Rect::from_min_size(egui::pos2(210.0, 80.0), egui::vec2(90.0, 24.0));
            let group = Shape::Vec(vec![
                Shape::rect_filled(old, 0.0, Color32::RED),
                Shape::Vec(vec![Shape::rect_filled(new, 0.0, Color32::GREEN)]),
            ]);
            let painter = ui.painter().with_clip_rect(body);
            let offset = egui::vec2(3.0, -2.0);
            for target in [right, body] {
                let marker = begin(ui);
                let index = painter.add(group.clone());
                marker.finish(ui).apply(ui, offset, target);
                ui.ctx().graphics(|graphics| {
                    let list = graphics.get(ui.layer_id()).unwrap();
                    let entry = list.all_entries().nth(index.0).unwrap();
                    assert_eq!(
                        entry.clip_rect, body,
                        "both columns retain their shared clip"
                    );
                    if target == right {
                        assert_eq!(
                            entry.shape, group,
                            "reject the entire group before mutating any child"
                        );
                    } else {
                        assert_eq!(
                            entry.shape,
                            Shape::Vec(vec![
                                Shape::rect_filled(old.translate(offset), 0.0, Color32::RED),
                                Shape::Vec(vec![Shape::rect_filled(
                                    new.translate(offset),
                                    0.0,
                                    Color32::GREEN
                                )]),
                            ])
                        );
                    }
                });
            }
        });
        output.drop_without_applying_deltas();
    }

    #[test]
    fn callback_coordinates_are_not_rewritten() {
        let callback = egui::PaintCallback {
            rect: viewport(),
            callback: Arc::new(()),
        };
        let original = callback.clone();
        let mut shape = Shape::Callback(callback);
        translate(&mut shape, viewport(), egui::vec2(3.0, 2.0), viewport());
        let Shape::Callback(callback) = shape else {
            panic!("callback missing")
        };
        assert_eq!(callback.rect, original.rect);
        assert!(Arc::ptr_eq(&callback.callback, &original.callback));
    }

    #[test]
    fn invalid_offsets_and_viewports_leave_paint_unchanged() {
        let context = egui::Context::default();
        let output = context.run_ui(screen(), |ui| {
            for (offset, viewport) in [
                (Vec2::ZERO, viewport()),
                (egui::vec2(f32::NAN, 0.0), viewport()),
                (egui::vec2(0.0, f32::INFINITY), viewport()),
                (egui::vec2(4.1, 0.0), viewport()),
                (egui::vec2(2.0, 1.0), Rect::NOTHING),
                (egui::vec2(2.0, 1.0), Rect::EVERYTHING),
            ] {
                let original = Shape::line_segment(
                    [egui::pos2(70.0, 80.0), egui::pos2(90.0, 100.0)],
                    Stroke::new(1.0, Color32::WHITE),
                );
                let marker = begin(ui);
                let index = ui
                    .painter()
                    .with_clip_rect(self::viewport())
                    .add(original.clone());
                marker.finish(ui).apply(ui, offset, viewport);
                ui.ctx().graphics(|graphics| {
                    assert_eq!(
                        graphics
                            .get(ui.layer_id())
                            .unwrap()
                            .all_entries()
                            .nth(index.0)
                            .unwrap()
                            .shape,
                        original
                    );
                });
            }
        });
        output.drop_without_applying_deltas();
    }
}
