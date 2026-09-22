//! Shared vector icons, hit targets and pure geometry. No application state.
use super::tooltips::native_hover_text;
use crate::theme::METRICS;
use eframe::egui::{self, Color32, Pos2, Rect, Sense, Stroke, Vec2};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum UiIcon {
    Check,
    Close,
    Down,
    FitWidth,
    Maximize,
    Restore,
    Panel,
    Next,
    Previous,
    Refresh,
    Eye,
    EyeClosed,
    Up,
    Warning,
    Waiting,
    ZoomIn,
    ZoomOut,
}

pub(super) fn square_icon_button(
    ui: &mut egui::Ui,
    icon: UiIcon,
    label: &str,
    size: f32,
) -> egui::Response {
    // Allocate exactly: even an empty Button lays out a font-height row plus
    // padding, which would silently grow dense Explorer headers.
    let (rect, response) = ui.allocate_exact_size(Vec2::splat(size), Sense::click());
    let visuals = ui.style().interact(&response);
    ui.painter().rect(
        rect,
        visuals.corner_radius,
        visuals.weak_bg_fill,
        visuals.bg_stroke,
        egui::StrokeKind::Inside,
    );
    response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, label));
    paint_ui_icon(
        ui.painter(),
        response.rect.shrink(5.0),
        icon,
        ui.style().interact(&response).fg_stroke.color,
    );
    response
}

pub(super) fn icon_button(ui: &mut egui::Ui, icon: UiIcon, tooltip: &str) -> egui::Response {
    icon_button_enabled(ui, true, icon, tooltip)
}

pub(super) fn icon_button_enabled(
    ui: &mut egui::Ui,
    enabled: bool,
    icon: UiIcon,
    tooltip: &str,
) -> egui::Response {
    let response = native_hover_text(
        ui.add_enabled(
            enabled,
            egui::Button::new("").min_size(METRICS.icon.button_size),
        ),
        tooltip,
    );
    let color = ui.style().interact(&response).fg_stroke.color;
    paint_ui_icon(
        ui.painter(),
        response.rect.shrink(METRICS.icon.button_icon_shrink),
        icon,
        color,
    );
    response
}

pub(super) fn static_icon(ui: &mut egui::Ui, icon: UiIcon, color: Color32) -> egui::Response {
    let (rect, response) =
        ui.allocate_exact_size(Vec2::splat(METRICS.icon.static_size), Sense::hover());
    paint_ui_icon(
        ui.painter(),
        rect.shrink(METRICS.icon.static_shrink),
        icon,
        color,
    );
    response
}

pub(super) struct RefreshIconGeometry {
    pub(super) arc: Vec<Pos2>,
    pub(super) shaft: [Pos2; 2],
    pub(super) wing: [Pos2; 2],
}

pub(super) fn refresh_icon_geometry(rect: Rect) -> RefreshIconGeometry {
    let center = rect.center();
    let radius = rect.width().min(rect.height()) * 0.34;
    let start_angle = 0.55_f32;
    let end_angle = 5.55_f32;
    let arc = (0..=20)
        .map(|index| {
            let angle = start_angle + (end_angle - start_angle) * index as f32 / 20.0;
            center + Vec2::new(angle.cos(), angle.sin()) * radius
        })
        .collect::<Vec<_>>();
    let junction = *arc.last().expect("refresh arc has an endpoint");
    let tangent = Vec2::new(-end_angle.sin(), end_angle.cos()).normalized();
    let radial = Vec2::new(end_angle.cos(), end_angle.sin()).normalized();
    let tip = junction + tangent * 2.1;
    let outer_corner = tip - tangent * 2.2 + radial * 1.5;
    RefreshIconGeometry {
        arc,
        shaft: [junction, tip],
        wing: [tip, outer_corner],
    }
}

pub(super) struct EyeIconGeometry {
    pub(super) upper: [Pos2; 4],
    pub(super) lower: [Pos2; 4],
    pub(super) pupil_radius: f32,
}

pub(super) fn eye_icon_geometry(rect: Rect) -> EyeIconGeometry {
    let center = rect.center();
    let half_width = rect.width() * 0.44;
    let control_lift = rect.height() * 0.34;
    let control_inset = half_width * 0.48;
    let left = Pos2::new(center.x - half_width, center.y);
    let right = Pos2::new(center.x + half_width, center.y);
    EyeIconGeometry {
        upper: [
            left,
            Pos2::new(center.x - control_inset, center.y - control_lift),
            Pos2::new(center.x + control_inset, center.y - control_lift),
            right,
        ],
        lower: [
            left,
            Pos2::new(center.x - control_inset, center.y + control_lift),
            Pos2::new(center.x + control_inset, center.y + control_lift),
            right,
        ],
        pupil_radius: rect.width().min(rect.height()) * 0.11,
    }
}

pub(super) fn closed_eye_icon_geometry(rect: Rect) -> ([Pos2; 4], [[Pos2; 2]; 3]) {
    // Center the visible lid + lashes, not the (otherwise invisible) full eye.
    let shift = rect.height() * (0.34 * 0.75 + 0.16) * 0.5;
    let lid = eye_icon_geometry(rect.translate(Vec2::new(0.0, -shift))).lower;
    let lashes = [0.2_f32, 0.5, 0.8].map(|t| {
        let s = 1.0 - t;
        let root = (lid[0].to_vec2() * s.powi(3)
            + lid[1].to_vec2() * (3.0 * s * s * t)
            + lid[2].to_vec2() * (3.0 * s * t * t)
            + lid[3].to_vec2() * t.powi(3))
        .to_pos2();
        [
            root,
            root + Vec2::new((t - 0.5) * rect.width() * 0.35, rect.height() * 0.16),
        ]
    });
    (lid, lashes)
}

pub(super) fn paint_ui_icon(painter: &egui::Painter, rect: Rect, icon: UiIcon, color: Color32) {
    let center = rect.center();
    let stroke = Stroke::new(METRICS.icon.stroke_width, color);
    match icon {
        UiIcon::Check => {
            painter.line_segment(
                [
                    Pos2::new(rect.left() + rect.width() * 0.12, center.y),
                    Pos2::new(rect.left() + rect.width() * 0.42, rect.bottom() - 2.0),
                ],
                stroke,
            );
            painter.line_segment(
                [
                    Pos2::new(rect.left() + rect.width() * 0.42, rect.bottom() - 2.0),
                    Pos2::new(rect.right() - 1.0, rect.top() + 2.0),
                ],
                stroke,
            );
        }
        UiIcon::Close => {
            painter.line_segment([rect.left_top(), rect.right_bottom()], stroke);
            painter.line_segment([rect.right_top(), rect.left_bottom()], stroke);
        }
        UiIcon::Up | UiIcon::Previous => {
            let (a, b, c) = if icon == UiIcon::Up {
                (
                    Pos2::new(rect.left() + 1.0, rect.bottom() - 2.0),
                    Pos2::new(center.x, rect.top() + 2.0),
                    Pos2::new(rect.right() - 1.0, rect.bottom() - 2.0),
                )
            } else {
                (
                    Pos2::new(rect.right() - 2.0, rect.top() + 1.0),
                    Pos2::new(rect.left() + 2.0, center.y),
                    Pos2::new(rect.right() - 2.0, rect.bottom() - 1.0),
                )
            };
            painter.line_segment([a, b], stroke);
            painter.line_segment([b, c], stroke);
        }
        UiIcon::Down | UiIcon::Next => {
            let (a, b, c) = if icon == UiIcon::Down {
                (
                    Pos2::new(rect.left() + 1.0, rect.top() + 2.0),
                    Pos2::new(center.x, rect.bottom() - 2.0),
                    Pos2::new(rect.right() - 1.0, rect.top() + 2.0),
                )
            } else {
                (
                    Pos2::new(rect.left() + 2.0, rect.top() + 1.0),
                    Pos2::new(rect.right() - 2.0, center.y),
                    Pos2::new(rect.left() + 2.0, rect.bottom() - 1.0),
                )
            };
            painter.line_segment([a, b], stroke);
            painter.line_segment([b, c], stroke);
        }
        UiIcon::Refresh => {
            // Continue the arc into one side of the arrowhead, then place its
            // outer wing beyond the circle. This keeps the small glyph open
            // instead of layering a large chevron over its own body.
            let geometry = refresh_icon_geometry(rect);
            painter.add(egui::Shape::line(geometry.arc, stroke));
            painter.line_segment(geometry.shaft, stroke);
            painter.line_segment(geometry.wing, stroke);
        }
        UiIcon::Maximize => {
            painter.rect_stroke(rect, 0.0, stroke, egui::StrokeKind::Inside);
        }
        UiIcon::Restore => {
            let shift = Vec2::splat(rect.width() * 0.25);
            let front = Rect::from_min_max(
                rect.min + Vec2::new(0.0, shift.y),
                rect.max - Vec2::new(shift.x, 0.0),
            );
            let back = Rect::from_min_max(
                rect.min + Vec2::new(shift.x, 0.0),
                rect.max - Vec2::new(0.0, shift.y),
            );
            painter.line_segment([back.left_top(), back.right_top()], stroke);
            painter.line_segment([back.right_top(), back.right_bottom()], stroke);
            painter.rect_stroke(front, 0.0, stroke, egui::StrokeKind::Inside);
        }
        UiIcon::Panel => {
            painter.rect_stroke(rect, 1.0, stroke, egui::StrokeKind::Inside);
            let y = rect.bottom() - rect.height() * 0.35;
            painter.line_segment(
                [Pos2::new(rect.left(), y), Pos2::new(rect.right(), y)],
                stroke,
            );
        }
        UiIcon::Waiting => {
            painter.circle_stroke(center, rect.width().min(rect.height()) * 0.36, stroke);
        }
        UiIcon::Eye => {
            let geometry = eye_icon_geometry(rect);
            painter.add(egui::Shape::CubicBezier(
                egui::epaint::CubicBezierShape::from_points_stroke(
                    geometry.upper,
                    false,
                    Color32::TRANSPARENT,
                    stroke,
                ),
            ));
            painter.add(egui::Shape::CubicBezier(
                egui::epaint::CubicBezierShape::from_points_stroke(
                    geometry.lower,
                    false,
                    Color32::TRANSPARENT,
                    stroke,
                ),
            ));
            painter.circle_filled(center, geometry.pupil_radius, color);
        }
        UiIcon::EyeClosed => {
            let (lid, lashes) = closed_eye_icon_geometry(rect);
            painter.add(egui::Shape::CubicBezier(
                egui::epaint::CubicBezierShape::from_points_stroke(
                    lid,
                    false,
                    Color32::TRANSPARENT,
                    stroke,
                ),
            ));
            for lash in lashes {
                painter.line_segment(lash, stroke);
            }
        }
        UiIcon::ZoomIn | UiIcon::ZoomOut => {
            painter.line_segment(
                [
                    Pos2::new(rect.left() + 1.0, center.y),
                    Pos2::new(rect.right() - 1.0, center.y),
                ],
                stroke,
            );
            if icon == UiIcon::ZoomIn {
                painter.line_segment(
                    [
                        Pos2::new(center.x, rect.top() + 1.0),
                        Pos2::new(center.x, rect.bottom() - 1.0),
                    ],
                    stroke,
                );
            }
        }
        UiIcon::FitWidth => {
            painter.line_segment(
                [
                    Pos2::new(rect.left(), rect.top()),
                    Pos2::new(rect.left(), rect.bottom()),
                ],
                stroke,
            );
            painter.line_segment(
                [
                    Pos2::new(rect.right(), rect.top()),
                    Pos2::new(rect.right(), rect.bottom()),
                ],
                stroke,
            );
            painter.line_segment(
                [
                    Pos2::new(rect.left() + 3.0, center.y),
                    Pos2::new(rect.right() - 3.0, center.y),
                ],
                stroke,
            );
            painter.line_segment(
                [
                    Pos2::new(rect.left() + 3.0, center.y),
                    Pos2::new(rect.left() + 6.0, center.y - 3.0),
                ],
                stroke,
            );
            painter.line_segment(
                [
                    Pos2::new(rect.right() - 3.0, center.y),
                    Pos2::new(rect.right() - 6.0, center.y - 3.0),
                ],
                stroke,
            );
        }
        UiIcon::Warning => {
            painter.add(egui::Shape::convex_polygon(
                vec![
                    Pos2::new(center.x, rect.top()),
                    rect.right_bottom(),
                    rect.left_bottom(),
                ],
                Color32::TRANSPARENT,
                stroke,
            ));
            painter.line_segment(
                [
                    Pos2::new(center.x, rect.top() + 4.0),
                    Pos2::new(center.x, rect.bottom() - 4.0),
                ],
                stroke,
            );
            painter.circle_filled(Pos2::new(center.x, rect.bottom() - 2.0), 1.0, color);
        }
    }
}
