//! Shared vector icons, hit targets and pure geometry. No application state.
use super::tooltips::native_hover_text;
use crate::theme::METRICS;
use eframe::egui::{self, Color32, Pos2, Rect, Sense, Stroke, Vec2};

// One registry drives both the exhaustive painter and the visual audit sheet.
macro_rules! ui_icons {
    ($($name:ident),* $(,)?) => {
        #[derive(Debug, Clone, Copy, PartialEq, Eq)]
        pub(crate) enum UiIcon { $($name),* }
        impl UiIcon {
            pub(crate) const ALL: &'static [Self] = &[$(Self::$name),*];
        }
    };
}
ui_icons! {
    Check,
    Save,
    Document,
    Copy,
    Trash,
    Link,
    Close,
    Down,
    FitWidth,
    Search,
    Explorer,
    Code,
    Split,
    Preview,
    Stage,
    Unstage,
    Commit,
    Diff,
    Push,
    Pull,
    Fetch,
    Revert,
    InitializeGit,
    Pause,
    Play,
    Compile,
    Spanner,
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
    Folder,
    File,
}

pub(crate) fn square_icon_button(
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

pub(crate) fn icon_button(ui: &mut egui::Ui, icon: UiIcon, tooltip: &str) -> egui::Response {
    icon_button_enabled(ui, true, icon, tooltip)
}

pub(crate) fn icon_button_enabled(
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
    response.widget_info(|| {
        egui::WidgetInfo::labeled(egui::WidgetType::Button, response.enabled(), tooltip)
    });
    paint_ui_icon(
        ui.painter(),
        response.rect.shrink(METRICS.icon.button_icon_shrink),
        icon,
        color,
    );
    response
}

pub(crate) fn static_icon(ui: &mut egui::Ui, icon: UiIcon, color: Color32) -> egui::Response {
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

pub(crate) fn toolbar_button(
    ui: &mut egui::Ui,
    enabled: bool,
    selected: bool,
    icon: UiIcon,
    label: &str,
    style: crate::settings::ToolbarStyle,
    compiling: bool,
) -> egui::Response {
    use crate::settings::ToolbarStyle;
    if style == ToolbarStyle::Text {
        return ui.add_enabled(enabled, egui::Button::new(label).selected(selected));
    }
    let text = (style == ToolbarStyle::TextAndIcons).then(|| {
        ui.painter().layout_no_wrap(
            label.into(),
            egui::TextStyle::Button.resolve(ui.style()),
            ui.visuals().text_color(),
        )
    });
    let icon_size = egui::TextStyle::Button.resolve(ui.style()).size.min(14.0);
    let width = text.as_ref().map_or(0.0, |text| text.size().x + 6.0)
        + icon_size
        + 2.0 * ui.spacing().button_padding.x;
    let response = ui.add_enabled(
        enabled,
        egui::Button::new("")
            .selected(selected)
            .min_size(Vec2::new(width, 0.0)),
    );
    response.widget_info(|| {
        egui::WidgetInfo::labeled(egui::WidgetType::Button, response.enabled(), label)
    });
    let color = ui.style().interact(&response).fg_stroke.color;
    let rect = Rect::from_center_size(
        Pos2::new(
            response.rect.left() + ui.spacing().button_padding.x + icon_size * 0.5,
            response.rect.center().y,
        ),
        Vec2::splat(icon_size),
    );
    if icon == UiIcon::Compile {
        let angle = if compiling {
            ui.input(|input| input.time as f32 * 2.5)
        } else {
            0.0
        };
        paint_compile_icon(ui.painter(), rect, color, angle);
        if compiling && ui.is_rect_visible(response.rect) {
            ui.ctx()
                .request_repaint_after(std::time::Duration::from_millis(33));
        }
    } else {
        paint_ui_icon(ui.painter(), rect, icon, color);
    }
    if let Some(text) = text {
        ui.painter().galley_with_override_text_color(
            Pos2::new(
                rect.right() + 6.0,
                response.rect.center().y - text.size().y * 0.5,
            ),
            text,
            color,
        );
    }
    response
}

fn paint_compile_icon(painter: &egui::Painter, rect: Rect, color: Color32, angle: f32) {
    let radius = rect.width().min(rect.height()) * 0.45;
    let points = (0..32)
        .map(|i| {
            let theta = angle + i as f32 * std::f32::consts::TAU / 32.0;
            let r = radius * if i % 4 < 2 { 1.0 } else { 0.75 };
            rect.center() + Vec2::angled(theta) * r
        })
        .collect();
    painter.add(egui::Shape::closed_line(points, Stroke::new(1.2, color)));
    painter.circle_stroke(rect.center(), radius * 0.32, Stroke::new(1.2, color));
}

pub(crate) struct RefreshIconGeometry {
    pub(crate) arc: Vec<Pos2>,
    pub(crate) shaft: [Pos2; 2],
    pub(crate) wing: [Pos2; 2],
}

pub(crate) fn refresh_icon_geometry(rect: Rect) -> RefreshIconGeometry {
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

pub(crate) struct EyeIconGeometry {
    pub(crate) upper: [Pos2; 4],
    pub(crate) lower: [Pos2; 4],
    pub(crate) pupil_radius: f32,
}

pub(crate) fn eye_icon_geometry(rect: Rect) -> EyeIconGeometry {
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

pub(crate) fn closed_eye_icon_geometry(rect: Rect) -> ([Pos2; 4], [[Pos2; 2]; 3]) {
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

fn panel_size_icon_rects(rect: Rect) -> [Rect; 2] {
    let shift = rect.size() * 0.25;
    [
        Rect::from_min_max(
            rect.min + Vec2::new(shift.x, 0.0),
            rect.max - Vec2::new(0.0, shift.y),
        ),
        Rect::from_min_max(
            rect.min + Vec2::new(0.0, shift.y),
            rect.max - Vec2::new(shift.x, 0.0),
        ),
    ]
}

pub(crate) fn paint_ui_icon(painter: &egui::Painter, rect: Rect, icon: UiIcon, color: Color32) {
    let center = rect.center();
    let stroke = Stroke::new(1.25, color);
    match icon {
        UiIcon::Folder | UiIcon::File => {
            paint_tree_glyph(painter, rect, icon == UiIcon::Folder, color);
        }
        UiIcon::Save => {
            painter.rect_stroke(rect, 1.5, stroke, egui::StrokeKind::Inside);
            painter.rect_stroke(
                Rect::from_min_max(
                    rect.min + Vec2::new(3.0, 0.0),
                    rect.min + Vec2::new(rect.width() - 3.0, rect.height() * 0.38),
                ),
                0.5,
                stroke,
                egui::StrokeKind::Inside,
            );
            painter.rect_stroke(
                Rect::from_min_max(
                    rect.min + Vec2::new(3.0, rect.height() * 0.58),
                    rect.max - Vec2::new(3.0, 0.0),
                ),
                0.5,
                stroke,
                egui::StrokeKind::Inside,
            );
        }
        UiIcon::Document => {
            painter.rect_stroke(
                rect.shrink2(Vec2::new(2.0, 0.0)),
                1.5,
                stroke,
                egui::StrokeKind::Inside,
            );
            painter.line_segment(
                [center - Vec2::new(3.0, 0.0), center + Vec2::new(3.0, 0.0)],
                stroke,
            );
            painter.line_segment(
                [center - Vec2::new(0.0, 3.0), center + Vec2::new(0.0, 3.0)],
                stroke,
            );
        }
        UiIcon::Copy => {
            // Only paint the exposed edge of the back sheet. A transparent
            // foreground fill cannot erase lines already painted behind it.
            let back =
                Rect::from_min_max(rect.min + Vec2::splat(0.625), rect.max - Vec2::splat(4.0));
            painter.add(egui::Shape::line(
                vec![
                    Pos2::new(back.left(), rect.bottom() - 4.0),
                    back.left_top(),
                    back.right_top(),
                    Pos2::new(back.right(), rect.top() + 2.0),
                ],
                stroke,
            ));
            let front = Rect::from_min_max(rect.min + Vec2::splat(3.5), rect.max);
            painter.rect_stroke(front, 1.5, stroke, egui::StrokeKind::Inside);
        }
        UiIcon::Trash => {
            painter.rect_stroke(
                Rect::from_min_max(
                    rect.min + Vec2::new(3.0, 4.0),
                    rect.max - Vec2::new(3.0, 0.0),
                ),
                1.5,
                stroke,
                egui::StrokeKind::Inside,
            );
            painter.line_segment(
                [
                    rect.min + Vec2::new(1.0, 3.0),
                    rect.right_top() + Vec2::new(-1.0, 3.0),
                ],
                stroke,
            );
            painter.line_segment(
                [
                    rect.min + Vec2::new(5.0, 0.0),
                    rect.right_top() + Vec2::new(-5.0, 0.0),
                ],
                stroke,
            );
        }
        UiIcon::Link => {
            painter.rect_stroke(
                Rect::from_min_max(
                    rect.min + Vec2::new(0.0, 5.0),
                    rect.max - Vec2::new(5.0, 0.0),
                ),
                1.5,
                stroke,
                egui::StrokeKind::Inside,
            );
            painter.line_segment([center, rect.right_top()], stroke);
            painter.line_segment(
                [rect.right_top() + Vec2::new(-5.0, 0.0), rect.right_top()],
                stroke,
            );
            painter.line_segment(
                [rect.right_top() + Vec2::new(0.0, 5.0), rect.right_top()],
                stroke,
            );
        }
        UiIcon::Search => {
            let radius = rect.width().min(rect.height()) * 0.32;
            let center = rect.min + Vec2::splat(radius + 1.0);
            painter.circle_stroke(center, radius, stroke);
            painter.line_segment(
                [
                    center + Vec2::splat(radius * 0.7),
                    rect.max - Vec2::splat(1.0),
                ],
                stroke,
            );
        }
        UiIcon::Pause => {
            for x in [0.28, 0.72] {
                painter.line_segment(
                    [
                        Pos2::new(rect.left() + rect.width() * x, rect.top() + 2.0),
                        Pos2::new(rect.left() + rect.width() * x, rect.bottom() - 2.0),
                    ],
                    Stroke::new(2.0, color),
                );
            }
        }
        UiIcon::Play => {
            painter.add(egui::Shape::convex_polygon(
                vec![
                    rect.left_top() + Vec2::splat(2.0),
                    rect.left_bottom() + Vec2::new(2.0, -2.0),
                    rect.right_center() - Vec2::new(1.0, 0.0),
                ],
                color,
                Stroke::NONE,
            ));
        }
        UiIcon::Compile => paint_compile_icon(painter, rect, color, 0.0),
        UiIcon::Spanner => {
            let points = [
                (0.55, 0.08),
                (0.48, 0.28),
                (0.53, 0.43),
                (0.09, 0.80),
                (0.09, 0.91),
                (0.20, 0.91),
                (0.61, 0.50),
                (0.77, 0.52),
                (0.94, 0.39),
                (0.95, 0.22),
                (0.76, 0.36),
                (0.64, 0.24),
                (0.76, 0.06),
            ]
            .into_iter()
            .map(|(x, y)| rect.min + rect.size() * Vec2::new(x, y))
            .collect();
            painter.add(egui::Shape::closed_line(points, stroke));
        }
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
        UiIcon::Maximize | UiIcon::Restore => {
            let [back, front] = panel_size_icon_rects(rect);
            let strong = Stroke::new(stroke.width, color);
            let quiet = Stroke::new(stroke.width * 0.65, color.gamma_multiply(0.45));
            let (back_stroke, front_stroke) = if icon == UiIcon::Maximize {
                (strong, quiet)
            } else {
                (quiet, strong)
            };
            // The same overlapping windows describe both actions. Emphasize
            // the upper window to expand, the lower window to restore.
            let inset = back_stroke.width * 0.5;
            let back = back.shrink(inset);
            let r = 1.8;
            let top_end = egui::pos2(back.right() - r, back.top());
            let right_start = egui::pos2(back.right(), back.top() + r);
            painter.line_segment([back.left_top(), top_end], back_stroke);
            painter.add(egui::Shape::CubicBezier(
                egui::epaint::CubicBezierShape::from_points_stroke(
                    [
                        top_end,
                        top_end + Vec2::new(r * 0.5523, 0.0),
                        right_start - Vec2::new(0.0, r * 0.5523),
                        right_start,
                    ],
                    false,
                    Color32::TRANSPARENT,
                    back_stroke,
                ),
            ));
            painter.line_segment([right_start, back.right_bottom()], back_stroke);
            painter.rect_stroke(front, 2.0, front_stroke, egui::StrokeKind::Inside);
        }
        UiIcon::Panel | UiIcon::Explorer | UiIcon::Code | UiIcon::Split | UiIcon::Preview => {
            painter.rect_stroke(
                rect,
                2.0,
                Stroke::new(1.0, color.gamma_multiply(0.4)),
                egui::StrokeKind::Inside,
            );
            let inner = rect.shrink(2.0);
            let left =
                Rect::from_min_max(inner.min, Pos2::new(inner.center().x - 1.0, inner.bottom()));
            let right =
                Rect::from_min_max(Pos2::new(inner.center().x + 1.0, inner.top()), inner.max);
            match icon {
                UiIcon::Panel => {
                    painter.rect_filled(
                        Rect::from_min_max(
                            Pos2::new(inner.left(), inner.bottom() - inner.height() / 3.0),
                            inner.max,
                        ),
                        0.8,
                        color,
                    );
                }
                UiIcon::Explorer => {
                    painter.rect_filled(
                        Rect::from_min_size(
                            inner.min,
                            Vec2::new(inner.width() / 4.0, inner.height()),
                        ),
                        0.8,
                        color,
                    );
                }
                _ => {
                    painter.rect_filled(
                        left,
                        0.8,
                        color.gamma_multiply(if icon == UiIcon::Preview { 0.18 } else { 1.0 }),
                    );
                    painter.rect_filled(
                        right,
                        0.8,
                        color.gamma_multiply(if icon == UiIcon::Code { 0.18 } else { 1.0 }),
                    );
                }
            }
        }
        UiIcon::Stage | UiIcon::Unstage => {
            painter.rect_stroke(
                rect.shrink(1.0),
                2.0,
                Stroke::new(1.0, color.gamma_multiply(0.4)),
                egui::StrokeKind::Inside,
            );
            painter.line_segment(
                [center - Vec2::new(3.5, 0.0), center + Vec2::new(3.5, 0.0)],
                stroke,
            );
            if icon == UiIcon::Stage {
                painter.line_segment(
                    [center - Vec2::new(0.0, 3.5), center + Vec2::new(0.0, 3.5)],
                    stroke,
                );
            }
        }
        UiIcon::Push | UiIcon::Pull | UiIcon::Fetch => {
            let up = icon == UiIcon::Push;
            let tip = Pos2::new(
                center.x,
                if up {
                    rect.top() + 1.0
                } else {
                    rect.bottom() - 2.0
                },
            );
            let sign = if up { 1.0 } else { -1.0 };
            painter.line_segment([tip, tip + Vec2::new(0.0, sign * 8.0)], stroke);
            painter.add(egui::Shape::line(
                vec![
                    tip + Vec2::new(-3.0, sign * 3.0),
                    tip,
                    tip + Vec2::new(3.0, sign * 3.0),
                ],
                stroke,
            ));
            if icon == UiIcon::Fetch {
                painter.line_segment([rect.left_bottom(), rect.right_bottom()], stroke);
            }
        }
        UiIcon::Commit => {
            painter.line_segment([rect.left_center(), center - Vec2::new(3.0, 0.0)], stroke);
            painter.line_segment([center + Vec2::new(3.0, 0.0), rect.right_center()], stroke);
            painter.circle_stroke(center, 3.0, stroke);
        }
        UiIcon::Diff => {
            painter.rect_stroke(
                rect.shrink(1.0),
                2.0,
                Stroke::new(1.0, color.gamma_multiply(0.4)),
                egui::StrokeKind::Inside,
            );
            painter.line_segment(
                [
                    center + Vec2::new(-3.0, -2.0),
                    center + Vec2::new(3.0, -2.0),
                ],
                stroke,
            );
            painter.line_segment(
                [center + Vec2::new(-3.0, 2.0), center + Vec2::new(3.0, 2.0)],
                stroke,
            );
            painter.line_segment(
                [center + Vec2::new(0.0, 0.0), center + Vec2::new(0.0, 4.0)],
                stroke,
            );
        }
        UiIcon::Revert => {
            painter.add(egui::Shape::line(
                vec![
                    rect.right_bottom() - Vec2::splat(2.0),
                    Pos2::new(rect.right() - 2.0, center.y),
                    Pos2::new(rect.left() + 1.0, center.y),
                ],
                stroke,
            ));
            painter.add(egui::Shape::line(
                vec![
                    Pos2::new(rect.left() + 4.0, center.y - 3.0),
                    Pos2::new(rect.left() + 1.0, center.y),
                    Pos2::new(rect.left() + 4.0, center.y + 3.0),
                ],
                stroke,
            ));
        }
        UiIcon::InitializeGit => {
            painter.rect_stroke(rect.shrink(1.0), 2.0, stroke, egui::StrokeKind::Inside);
            painter.line_segment(
                [
                    Pos2::new(rect.left() + 4.0, rect.top() + 1.0),
                    Pos2::new(rect.left() + 4.0, rect.bottom() - 1.0),
                ],
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
            for x in [rect.left() + 0.625, rect.right() - 0.625] {
                painter.line_segment(
                    [
                        Pos2::new(x, rect.top() + 1.0),
                        Pos2::new(x, rect.bottom() - 1.0),
                    ],
                    stroke,
                );
            }
            let left = Pos2::new(rect.left() + 3.0, center.y);
            let right = Pos2::new(rect.right() - 3.0, center.y);
            painter.line_segment([left, right], stroke);
            for (tip, direction) in [(left, 1.0), (right, -1.0)] {
                painter.add(egui::Shape::line(
                    vec![
                        tip + Vec2::new(direction * 2.0, -2.0),
                        tip,
                        tip + Vec2::new(direction * 2.0, 2.0),
                    ],
                    stroke,
                ));
            }
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

/// Compact actions retain a semantic accessible label and native hover text.
pub(crate) fn action_button(ui: &mut egui::Ui, label: &str) -> egui::Response {
    action_button_enabled(ui, true, label)
}
pub(crate) fn action_button_enabled(
    ui: &mut egui::Ui,
    enabled: bool,
    label: &str,
) -> egui::Response {
    let icon = match label {
        "Cancel" | "Disable" => UiIcon::Close,
        "Delete" | "Discard" | "Uninstall" | "Remove from Recents" => UiIcon::Trash,
        "Save" | "Overwrite" => UiIcon::Save,
        "Copy import" => UiIcon::Copy,
        "Website" => UiIcon::Link,
        "Create document" | "New document" | "Open converted copy" => UiIcon::Document,
        "Add row" | "Add column" => UiIcon::Stage,
        "Selected row" | "Last column" => UiIcon::Unstage,
        "Apply" | "Apply command" | "OK" => UiIcon::Check,
        "Refresh" | "Retry Tinymist" => UiIcon::Refresh,
        "Reset"
        | "Reset all"
        | "Reset command"
        | "Reset colors"
        | "Reset panel order"
        | "Reset this appearance"
        | "Revert" => UiIcon::Revert,
        "Choose…" | "Choose Folder…" | "Choose folder…" | "Browse…" | "Open file…" | "Import…" => {
            UiIcon::Explorer
        }
        "Replace" | "All" | "Replace draft from Markdown" => UiIcon::Diff,
        "Main" => UiIcon::Code,
        "Both" => UiIcon::Split,
        _ => UiIcon::Spanner,
    };
    icon_button_enabled(ui, enabled, icon, label)
}

fn paint_tree_glyph(painter: &egui::Painter, rect: Rect, folder: bool, color: Color32) {
    let stroke = Stroke::new(METRICS.explorer.tree_icon_stroke, color);
    if folder {
        // A flat tab reads as a folder even at 14×12; avoid a peaked roof.
        let outline = rect.shrink(stroke.width * 0.5);
        painter.add(egui::Shape::closed_line(
            vec![
                outline.left_bottom(),
                outline.left_top(),
                Pos2::new(outline.left() + 4.0, outline.top()),
                Pos2::new(outline.left() + 6.0, outline.top() + 3.0),
                Pos2::new(outline.right(), outline.top() + 3.0),
                outline.right_bottom(),
            ],
            stroke,
        ));
        painter.line_segment(
            [
                Pos2::new(outline.left(), outline.top() + 3.0),
                Pos2::new(outline.left() + 6.0, outline.top() + 3.0),
            ],
            stroke,
        );
    } else {
        let rect = rect.shrink2(Vec2::new(2.0, 0.0));
        painter.rect_stroke(rect, 1.2, stroke, egui::StrokeKind::Inside);
        painter.line_segment(
            [
                Pos2::new(rect.left() + 2.5, rect.top() + 4.0),
                Pos2::new(rect.right() - 2.5, rect.top() + 4.0),
            ],
            stroke,
        );
        painter.line_segment(
            [
                Pos2::new(rect.left() + 2.5, rect.top() + 7.0),
                Pos2::new(rect.right() - 2.5, rect.top() + 7.0),
            ],
            stroke,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_registered_icon_has_finite_bounded_ink_at_control_sizes() {
        for size in [Vec2::new(14.0, 12.0), Vec2::splat(14.0)] {
            for &icon in UiIcon::ALL {
                let ctx = egui::Context::default();
                let rect = Rect::from_min_size(Pos2::new(20.0, 20.0), size);
                let mut output = ctx.run_ui(Default::default(), |ui| {
                    let painter = ui.ctx().layer_painter(egui::LayerId::background());
                    paint_ui_icon(&painter, rect, icon, Color32::BLACK);
                });
                output.textures_delta.clear();
                assert!(!output.shapes.is_empty(), "{icon:?} has no ink");
                for shape in output.shapes {
                    let bounds = shape.shape.visual_bounding_rect();
                    assert!(bounds.is_finite(), "{icon:?}: {bounds:?}");
                    assert!(
                        rect.expand(1.0).contains_rect(bounds),
                        "{icon:?}: {bounds:?} outside {rect:?}"
                    );
                }
            }
        }
    }

    #[test]
    fn contact_sheet_renders_every_registered_icon_without_clipping() {
        let mut harness = egui_kittest::Harness::builder()
            .with_size(Vec2::new(1400.0, 900.0))
            .build_ui(super::super::icon_sheet::show);
        harness.run();
        assert_eq!(harness.ctx.pixels_per_point(), 1.0);
        assert!(76.0 + UiIcon::ALL.len().div_ceil(6) as f32 * 88.0 + 152.0 < 900.0);
    }

    #[test]
    fn panel_size_windows_share_bounded_geometry_at_small_and_scaled_sizes() {
        for size in [Vec2::splat(8.0), Vec2::splat(16.0), Vec2::new(24.0, 18.0)] {
            let rect = Rect::from_min_size(Pos2::new(3.0, 7.0), size);
            let [back, front] = panel_size_icon_rects(rect);
            assert!(rect.contains_rect(back) && rect.contains_rect(front));
            assert_eq!(back.size(), front.size());
            assert!(back.top() < front.top() && back.left() > front.left());
            assert!(back.intersects(front));
        }
    }
}
