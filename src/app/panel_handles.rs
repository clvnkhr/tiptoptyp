//! Paint-only divider grips: no widgets, extra spacing, or changed hit areas.
use eframe::egui::{self, Pos2, Rect, Stroke};

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
pub(super) fn paint(ui: &egui::Ui, center: Pos2, horizontal: bool) {
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
    for (a, b) in segments(center, horizontal) {
        painter.line_segment(
            [a, b],
            Stroke::new(1.5, ui.visuals().widgets.noninteractive.bg_stroke.color),
        );
    }
}
pub(super) fn editor_width(width: f32, explorer_delta: f32, minimum: f32, maximum: f32) -> f32 {
    (width - explorer_delta).clamp(minimum, maximum.max(minimum))
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
