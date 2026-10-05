//! Paint-only divider grips: no widgets, extra spacing, or changed hit areas.
use eframe::egui::{self, Pos2, Rect, Stroke, StrokeKind};

/// The stroke a resizable egui panel paints its separator with while the user
/// hovers or drags it; `None` while idle. Panels register their resize handle
/// under `<panel id>.with("__resize")`, so this mirrors the separator's own
/// active/hovered stroke choice exactly.
pub(super) fn panel_divider(ui: &egui::Ui, panel_id: egui::Id) -> Option<Stroke> {
    let response = ui.ctx().read_response(panel_id.with("__resize"))?;
    if response.dragged() {
        Some(ui.style().visuals.widgets.active.fg_stroke)
    } else if response.hovered() {
        Some(ui.style().visuals.widgets.hovered.fg_stroke)
    } else {
        None
    }
}

pub(super) fn segments(center: Pos2, horizontal: bool) -> [(Pos2, Pos2); 2] {
    [-2.0, 2.0].map(|offset| {
        if horizontal {
            (
                center + egui::vec2(-9.0, offset),
                center + egui::vec2(9.0, offset),
            )
        } else {
            (
                center + egui::vec2(offset, -9.0),
                center + egui::vec2(offset, 9.0),
            )
        }
    })
}
/// egui reserves a separator-width outer margin, then draws its rule halfway
/// through that margin. Use the rule's center, rather than the panel's edge.
pub(super) fn panel_center(ui: &egui::Ui, rect: Rect, horizontal: bool) -> Pos2 {
    let stroke = ui.visuals().widgets.noninteractive.bg_stroke.width;
    let inset = stroke.round() - 0.5 * stroke;
    if horizontal {
        egui::pos2(rect.center().x, rect.top() + inset)
    } else {
        egui::pos2(rect.right() - inset, rect.center().y)
    }
}
pub(super) fn paint(ui: &egui::Ui, center: Pos2, horizontal: bool, highlight: Option<Stroke>) {
    let bounds = Rect::from_center_size(
        center,
        if horizontal {
            egui::vec2(24.0, 8.0)
        } else {
            egui::vec2(8.0, 24.0)
        },
    );
    let painter = ui
        .ctx()
        .layer_painter(egui::LayerId::new(
            egui::Order::Middle,
            egui::Id::new("divider-grips"),
        ))
        .with_clip_rect(ui.ctx().content_rect());
    painter.rect_filled(bounds, 0.0, ui.visuals().panel_fill);
    // The bars stay dim; only the outline carries the highlight.
    let bars = Stroke::new(1.5, ui.visuals().widgets.noninteractive.bg_stroke.color);
    for (a, b) in segments(center, horizontal) {
        painter.line_segment([a, b], bars);
    }
    if let Some(stroke) = highlight {
        // A highlighted divider must not lose its handle behind the grip's
        // masking fill; outline the handle in the divider's own color.
        painter.rect_stroke(
            bounds,
            2.0,
            Stroke::new(1.5, stroke.color),
            StrokeKind::Inside,
        );
    }
}
pub(super) fn editor_width(width: f32, explorer_delta: f32, minimum: f32, maximum: f32) -> f32 {
    (width - explorer_delta).clamp(minimum, maximum.max(minimum))
}

#[cfg(all(feature = "desktop-ui-tests", target_os = "macos"))]
pub(super) fn observe(name: &str, response: &egui::Response, horizontal: bool, top: bool) {
    let mut target = response.clone();
    let rect = target.rect;
    target.rect = if horizontal {
        Rect::from_center_size(
            egui::pos2(
                rect.center().x,
                if top { rect.top() } else { rect.bottom() },
            ),
            egui::vec2(rect.width(), 6.0),
        )
    } else {
        Rect::from_center_size(
            egui::pos2(rect.right(), rect.center().y),
            egui::vec2(6.0, rect.height()),
        )
    };
    target.interact_rect = target.rect;
    crate::desktop_test::observe(name, &target);
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn independent_divider_preserves_preview_edge_until_a_minimum_is_hit() {
        assert_eq!(
            editor_width(600.0, 70.0, 100.0, 800.0) + 370.0,
            600.0 + 300.0
        );
        assert_eq!(editor_width(120.0, 70.0, 100.0, 800.0), 100.0);
        assert_eq!(editor_width(600.0, -70.0, 100.0, 800.0), 670.0);
    }
    #[test]
    fn grips_are_centered_rotated_versions_of_the_same_geometry() {
        let a = segments(Pos2::ZERO, true);
        let b = segments(Pos2::ZERO, false);
        for ((p, q), (r, s)) in a.into_iter().zip(b) {
            assert_eq!(egui::vec2(p.x, p.y), egui::vec2(r.y, r.x));
            assert_eq!(egui::vec2(q.x, q.y), egui::vec2(s.y, s.x));
        }
    }
    #[test]
    fn panel_grips_straddle_the_separator_not_the_outer_edge() {
        let context = egui::Context::default();
        let mut output = context.run_ui(egui::RawInput::default(), |ui| {
            ui.visuals_mut().widgets.noninteractive.bg_stroke.width = 1.0;
            let rect = Rect::from_min_max(egui::pos2(0.0, 30.0), egui::pos2(200.0, 300.0));
            for (horizontal, expected) in [
                (true, egui::pos2(100.0, 30.5)),
                (false, egui::pos2(199.5, 165.0)),
            ] {
                let center = panel_center(ui, rect, horizontal);
                assert_eq!(center, expected);
                let bars = segments(center, horizontal);
                let midpoint = bars[0]
                    .0
                    .lerp(bars[0].1, 0.5)
                    .lerp(bars[1].0.lerp(bars[1].1, 0.5), 0.5);
                assert_eq!(midpoint, expected);
            }
        });
        output.textures_delta.clear();
    }

    #[test]
    fn highlighted_grips_outline_the_handle_bounds_and_leave_their_bars_dim() {
        let context = egui::Context::default();
        let highlight = Stroke::new(2.0, egui::Color32::RED);
        for highlight in [None, Some(highlight)] {
            let mut output = context.run_ui(egui::RawInput::default(), move |ui| {
                ui.ctx().data_mut(|data| {
                    data.insert_temp(
                        egui::Id::new("grip-test-dim"),
                        ui.visuals().widgets.noninteractive.bg_stroke.color,
                    )
                });
                paint(ui, Pos2::new(50.0, 50.0), true, highlight);
            });
            output.textures_delta.clear();
            let dim: egui::Color32 = context
                .data(|data| data.get_temp(egui::Id::new("grip-test-dim")))
                .unwrap();
            let mut bars = 0;
            let mut outlines = 0;
            for shape in &output.shapes {
                match &shape.shape {
                    egui::Shape::LineSegment { stroke, .. } => {
                        bars += 1;
                        assert_eq!(stroke.color, dim, "bars never carry the highlight");
                    }
                    egui::Shape::Rect(rect) => {
                        if let Some(stroke) = highlight
                            && rect.stroke.color == stroke.color
                        {
                            outlines += 1;
                            assert_eq!(rect.rect.size(), egui::vec2(24.0, 8.0));
                        }
                    }
                    _ => {}
                }
            }
            assert_eq!(bars, 2);
            assert_eq!(
                outlines,
                usize::from(highlight.is_some()),
                "only a highlighted divider outlines its handle"
            );
        }
    }

    #[test]
    fn panel_resize_states_reach_the_grip_through_hover_and_drag() {
        use egui_kittest::Harness;
        let mut harness = Harness::builder()
            .with_size(egui::vec2(400.0, 300.0))
            .build_ui_state(
                |ui, state: &mut (Option<Stroke>, egui::Color32, egui::Color32, f32)| {
                    let shown = egui::Panel::left("grip-chain")
                        .resizable(true)
                        .default_size(150.0)
                        .show(ui, |ui| {
                            ui.label("side");
                        });
                    state.0 = panel_divider(ui, egui::Id::new("grip-chain"));
                    state.1 = ui.visuals().widgets.hovered.fg_stroke.color;
                    state.2 = ui.visuals().widgets.active.fg_stroke.color;
                    state.3 = shown.response.rect.right();
                },
                (
                    None,
                    egui::Color32::TRANSPARENT,
                    egui::Color32::TRANSPARENT,
                    0.0,
                ),
            );
        harness.run_steps(2);
        assert!(harness.state().0.is_none());
        let edge = harness.state().3;
        harness.hover_at(Pos2::new(edge, 150.0));
        harness.run_steps(2);
        assert_eq!(
            harness.state().0.map(|stroke| stroke.color),
            Some(harness.state().1),
            "hovering the divider reports the hovered stroke"
        );
        harness.drag_at(Pos2::new(edge - 2.0, 150.0));
        harness.run_steps(1);
        harness.hover_at(Pos2::new(edge + 12.0, 150.0));
        harness.run_steps(2);
        assert_eq!(
            harness.state().0.map(|stroke| stroke.color),
            Some(harness.state().2),
            "dragging the divider reports the active stroke"
        );
        harness.drop_at(Pos2::new(edge + 2.0, 150.0));
        harness.hover_at(Pos2::new(10.0, 10.0));
        harness.run_steps(3);
        assert!(harness.state().0.is_none());
    }
}
