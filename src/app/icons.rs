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
    Diff,
    DiffAll,
    Commit,
    Push,
    Pull,
    Fetch,
    Revert,
    InitializeGit,
    Pause,
    Play,
    Compile,
    CompileFilled,
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
        paint_compile_icon(ui.painter(), rect, color, angle, compiling);
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

/// Round a contour as one path, so joins never depend on independently
/// rasterized line/curve endpoints. Deliberate branches remain separate paths.
fn rounded_contour(points: &[Pos2], closed: bool, radius: f32) -> Vec<Pos2> {
    let mut result = Vec::with_capacity(points.len() * 5);
    for (index, &corner) in points.iter().enumerate() {
        if !closed && (index == 0 || index + 1 == points.len()) {
            result.push(corner);
            continue;
        }
        let before = points[(index + points.len() - 1) % points.len()] - corner;
        let after = points[(index + 1) % points.len()] - corner;
        let distance = radius.min(before.length() * 0.5).min(after.length() * 0.5);
        let start = corner + before.normalized() * distance;
        let end = corner + after.normalized() * distance;
        for step in 0..=4 {
            let t = step as f32 / 4.0;
            result.push(
                ((1.0 - t).powi(2) * start.to_vec2()
                    + 2.0 * (1.0 - t) * t * corner.to_vec2()
                    + t * t * end.to_vec2())
                .to_pos2(),
            );
        }
    }
    result
}

fn curved_path(
    painter: &egui::Painter,
    points: &[Pos2],
    closed: bool,
    radius: f32,
    stroke: Stroke,
) {
    painter.add(egui::epaint::PathShape {
        points: rounded_contour(points, closed, radius),
        closed,
        fill: Color32::TRANSPARENT,
        stroke: stroke.into(),
    });
}

fn plus_minus(painter: &egui::Painter, center: Pos2, extent: f32, plus: bool, color: Color32) {
    let stroke = Stroke::new(1.0, color);
    painter.line_segment(
        [
            center - Vec2::new(extent, 0.0),
            center + Vec2::new(extent, 0.0),
        ],
        stroke,
    );
    if plus {
        painter.line_segment(
            [
                center - Vec2::new(0.0, extent),
                center + Vec2::new(0.0, extent),
            ],
            stroke,
        );
    }
}

fn cubic_points(points: [Pos2; 4]) -> Vec<Pos2> {
    (0..=20)
        .map(|step| {
            let t = step as f32 / 20.0;
            let s = 1.0 - t;
            (points[0].to_vec2() * s.powi(3)
                + points[1].to_vec2() * (3.0 * s * s * t)
                + points[2].to_vec2() * (3.0 * s * t * t)
                + points[3].to_vec2() * t.powi(3))
            .to_pos2()
        })
        .collect()
}

fn paint_compile_icon(
    painter: &egui::Painter,
    rect: Rect,
    color: Color32,
    angle: f32,
    filled: bool,
) {
    let radius = rect.width().min(rect.height()) * 0.43;
    let points: Vec<_> = (0..64)
        .map(|i| {
            let theta = angle + i as f32 * std::f32::consts::TAU / 64.0;
            let r = radius * [0.78, 0.80, 0.97, 1.0, 1.0, 0.97, 0.80, 0.78][i % 8];
            rect.center() + Vec2::angled(theta) * r
        })
        .collect();
    let hole_radius = radius * 0.32;
    if filled {
        // A triangulated ring preserves the transparent axle hole on every
        // button background. Convex-polygon filling is invalid for gear teeth.
        let mut mesh = egui::Mesh::default();
        for (i, &outer) in points.iter().enumerate() {
            let theta = angle + i as f32 * std::f32::consts::TAU / 64.0;
            mesh.colored_vertex(outer, color);
            mesh.colored_vertex(rect.center() + Vec2::angled(theta) * hole_radius, color);
        }
        for i in 0..64u32 {
            let next = (i + 1) % 64;
            mesh.add_triangle(i * 2, next * 2, i * 2 + 1);
            mesh.add_triangle(i * 2 + 1, next * 2, next * 2 + 1);
        }
        painter.add(mesh);
    }
    painter.add(egui::Shape::closed_line(points, Stroke::new(1.2, color)));
    painter.circle_stroke(rect.center(), hole_radius, Stroke::new(1.2, color));
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
            plus_minus(painter, center, 1.5, true, color);
        }
        UiIcon::Copy => {
            // Only paint the exposed edge of the back sheet. A transparent
            // foreground fill cannot erase lines already painted behind it.
            let back =
                Rect::from_min_max(rect.min + Vec2::splat(0.625), rect.max - Vec2::splat(4.0));
            curved_path(
                painter,
                &[
                    Pos2::new(back.left(), rect.bottom() - 4.0),
                    back.left_top(),
                    back.right_top(),
                    Pos2::new(back.right(), rect.top() + 2.0),
                ],
                false,
                1.0,
                stroke,
            );
            let front = Rect::from_min_max(rect.min + Vec2::splat(3.5), rect.max);
            painter.rect_stroke(front, 1.5, stroke, egui::StrokeKind::Inside);
        }
        UiIcon::Trash => {
            let p = |x, y| rect.min + rect.size() * Vec2::new(x, y);
            curved_path(
                painter,
                &[p(0.23, 0.27), p(0.28, 0.92), p(0.72, 0.92), p(0.77, 0.27)],
                false,
                0.8,
                stroke,
            );
            // The handle and can meet the lid exactly; no floating fragments.
            curved_path(
                painter,
                &[p(0.36, 0.27), p(0.36, 0.08), p(0.64, 0.08), p(0.64, 0.27)],
                false,
                0.5,
                stroke,
            );
            painter.line_segment([p(0.12, 0.27), p(0.88, 0.27)], stroke);
            for x in [0.43, 0.57] {
                painter.line_segment([p(x, 0.43), p(x, 0.76)], Stroke::new(0.9, color));
            }
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
            let tip = rect.right_top() + Vec2::new(-1.0, 1.0);
            painter.line_segment([center, tip], stroke);
            painter.add(egui::Shape::line(
                vec![tip + Vec2::new(-5.0, 0.0), tip, tip + Vec2::new(0.0, 5.0)],
                stroke,
            ));
        }
        UiIcon::Search => {
            let radius = rect.width().min(rect.height()) * 0.32;
            let center = rect.min + Vec2::splat(radius + 1.0);
            let anchor_angle = std::f32::consts::FRAC_PI_4;
            let mut points = vec![rect.max - Vec2::splat(1.0)];
            points.extend((0..=40).map(|i| {
                center
                    + Vec2::angled(anchor_angle + i as f32 * std::f32::consts::TAU / 40.0) * radius
            }));
            painter.add(egui::Shape::line(points, stroke));
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
        UiIcon::Compile | UiIcon::CompileFilled => {
            paint_compile_icon(painter, rect, color, 0.0, icon == UiIcon::CompileFilled)
        }
        UiIcon::Spanner => {
            let points = [
                (0.60, 0.08),
                (0.45, 0.18),
                (0.43, 0.34),
                (0.47, 0.44),
                (0.11, 0.78),
                (0.08, 0.87),
                (0.13, 0.93),
                (0.22, 0.91),
                (0.59, 0.54),
                (0.73, 0.56),
                (0.88, 0.49),
                (0.94, 0.35),
                (0.93, 0.24),
                (0.76, 0.38),
                (0.62, 0.25),
                (0.74, 0.07),
            ]
            .map(|(x, y)| rect.min + rect.size() * Vec2::new(x, y));
            curved_path(painter, &points, true, 0.8, stroke);
        }
        UiIcon::Check
        | UiIcon::Close
        | UiIcon::Up
        | UiIcon::Down
        | UiIcon::Next
        | UiIcon::Previous => {
            // Half-size marks in unchanged hit targets. Already compact tab
            // crosses use the same small glyph rather than shrinking twice.
            let small = Rect::from_center_size(center, rect.size().min(Vec2::splat(7.0)));
            let p = |x, y| small.min + small.size() * Vec2::new(x, y);
            let points = match icon {
                UiIcon::Check => vec![p(0.12, 0.50), p(0.42, 0.86), p(0.93, 0.14)],
                UiIcon::Up => vec![p(0.07, 0.86), p(0.50, 0.14), p(0.93, 0.86)],
                UiIcon::Down => vec![p(0.07, 0.14), p(0.50, 0.86), p(0.93, 0.14)],
                UiIcon::Next => vec![p(0.14, 0.07), p(0.86, 0.50), p(0.14, 0.93)],
                UiIcon::Previous => vec![p(0.86, 0.07), p(0.14, 0.50), p(0.86, 0.93)],
                _ => {
                    painter.line_segment([small.left_top(), small.right_bottom()], stroke);
                    painter.line_segment([small.right_top(), small.left_bottom()], stroke);
                    return;
                }
            };
            painter.add(egui::Shape::line(points, stroke));
        }
        UiIcon::Refresh => {
            let geometry = refresh_icon_geometry(rect);
            let mut points = geometry.arc;
            points.extend([geometry.shaft[1], geometry.wing[1]]);
            painter.add(egui::Shape::line(points, stroke));
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
            let back = back.shrink(back_stroke.width * 0.5);
            curved_path(
                painter,
                &[back.left_top(), back.right_top(), back.right_bottom()],
                false,
                1.8,
                back_stroke,
            );
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
            plus_minus(painter, center, 1.75, icon == UiIcon::Stage, color);
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
                painter.line_segment(
                    [
                        rect.left_bottom() + Vec2::new(0.75, -0.75),
                        rect.right_bottom() - Vec2::splat(0.75),
                    ],
                    stroke,
                );
            }
        }
        UiIcon::Commit => {
            painter.line_segment(
                [
                    rect.left_center() + Vec2::new(0.75, 0.0),
                    center - Vec2::new(3.0, 0.0),
                ],
                stroke,
            );
            painter.line_segment(
                [
                    center + Vec2::new(3.0, 0.0),
                    rect.right_center() - Vec2::new(0.75, 0.0),
                ],
                stroke,
            );
            painter.circle_stroke(center, 3.0, stroke);
        }
        UiIcon::Diff | UiIcon::DiffAll => {
            // Opposing replacement arrows are independent of the boxed
            // stage/unstage controls. All adds a second head, never a smaller
            // or shifted base symbol.
            let arrow_stroke = Stroke::new(1.1, color);
            let heads = [
                (Pos2::new(rect.right() - 2.0, center.y - 2.5), -1.0),
                (Pos2::new(rect.left() + 2.0, center.y + 2.5), 1.0),
            ];
            for (tip, direction) in heads {
                let tail = Pos2::new(
                    if direction < 0.0 {
                        rect.left() + 2.0
                    } else {
                        rect.right() - 2.0
                    },
                    tip.y,
                );
                painter.line_segment([tail, tip], arrow_stroke);
                painter.add(egui::Shape::line(
                    vec![
                        tip + Vec2::new(direction * 2.0, -2.0),
                        tip,
                        tip + Vec2::new(direction * 2.0, 2.0),
                    ],
                    arrow_stroke,
                ));
            }
            if icon == UiIcon::DiffAll {
                for (tip, direction) in heads {
                    let tip = tip + Vec2::new(direction * 3.0, 0.0);
                    painter.add(egui::Shape::line(
                        vec![
                            tip + Vec2::new(direction * 2.0, -2.0),
                            tip,
                            tip + Vec2::new(direction * 2.0, 2.0),
                        ],
                        arrow_stroke,
                    ));
                }
            }
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
            let mut points = cubic_points(geometry.upper);
            let mut lower = cubic_points(geometry.lower);
            lower.reverse();
            points.extend(lower.into_iter().skip(1).take(19));
            painter.add(egui::Shape::closed_line(points, stroke));
            painter.circle_filled(center, geometry.pupil_radius, color);
        }
        UiIcon::EyeClosed => {
            let (lid, lashes) = closed_eye_icon_geometry(rect);
            painter.add(egui::Shape::line(cubic_points(lid), stroke));
            for lash in lashes {
                painter.line_segment(lash, stroke);
            }
        }
        UiIcon::ZoomIn | UiIcon::ZoomOut => {
            plus_minus(
                painter,
                center,
                (rect.width().min(rect.height()) - 2.0) * 0.25,
                icon == UiIcon::ZoomIn,
                color,
            );
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
            let inset = rect.shrink(1.5);
            curved_path(
                painter,
                &[
                    Pos2::new(center.x, inset.top()),
                    inset.right_bottom(),
                    inset.left_bottom(),
                ],
                true,
                0.8,
                stroke,
            );
            painter.line_segment(
                [
                    Pos2::new(center.x, rect.top() + rect.height() * 0.36),
                    Pos2::new(center.x, rect.top() + rect.height() * 0.58),
                ],
                stroke,
            );
            painter.circle_filled(
                Pos2::new(center.x, rect.top() + rect.height() * 0.74),
                0.65,
                color,
            );
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
        "Replace" | "Replace draft from Markdown" => UiIcon::Diff,
        "Replace all" => UiIcon::DiffAll,
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
        curved_path(
            painter,
            &[
                outline.left_bottom(),
                outline.left_top(),
                Pos2::new(outline.left() + 4.0, outline.top()),
                Pos2::new(outline.left() + 6.0, outline.top() + 3.0),
                Pos2::new(outline.right(), outline.top() + 3.0),
                outline.right_bottom(),
            ],
            true,
            0.8,
            stroke,
        );
        painter.line_segment(
            [
                Pos2::new(outline.left(), outline.top() + 3.0),
                Pos2::new(outline.left() + 6.8, outline.top() + 3.0),
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

    fn glyph_shapes(icon: UiIcon) -> Vec<egui::Shape> {
        let ctx = egui::Context::default();
        let mut output = ctx.run_ui(Default::default(), |ui| {
            paint_ui_icon(
                &ui.ctx().layer_painter(egui::LayerId::background()),
                Rect::from_min_size(Pos2::new(20.0, 20.0), Vec2::splat(14.0)),
                icon,
                Color32::BLACK,
            );
        });
        output.textures_delta.clear();
        output.shapes.into_iter().map(|shape| shape.shape).collect()
    }

    #[test]
    fn replace_all_adds_arrowheads_without_scaling_or_moving_diff() {
        let single = glyph_shapes(UiIcon::Diff);
        let all = glyph_shapes(UiIcon::DiffAll);
        assert_eq!(all.len(), single.len() + 2);
        assert_eq!(&all[..single.len()], single.as_slice());
        assert!(
            all.iter()
                .all(|shape| !matches!(shape, egui::Shape::Rect(_))),
            "replacement icons must not reuse stage/unstage boxes"
        );
    }

    #[test]
    fn replacement_arrows_and_warning_keep_their_entire_stroke_inside_the_icon_box() {
        let safe = Rect::from_min_max(egui::pos2(20.5, 20.5), egui::pos2(33.5, 33.5));
        for icon in [UiIcon::Diff, UiIcon::DiffAll, UiIcon::Warning] {
            for shape in glyph_shapes(icon) {
                assert!(
                    safe.contains_rect(shape.visual_bounding_rect()),
                    "{icon:?} reaches the icon boundary"
                );
            }
        }
        let warning = glyph_shapes(UiIcon::Warning);
        let egui::Shape::Path(outline) = &warning[0] else {
            panic!("warning requires a path outline")
        };
        assert!(
            outline.closed,
            "the warning contour must join its final edge"
        );
        assert_eq!(outline.fill.a(), 0);
    }

    #[test]
    fn continuous_contours_are_never_split_into_independent_strokes() {
        for icon in [
            UiIcon::Check,
            UiIcon::Up,
            UiIcon::Down,
            UiIcon::Next,
            UiIcon::Previous,
            UiIcon::Refresh,
            UiIcon::Search,
            UiIcon::Spanner,
        ] {
            let shapes = glyph_shapes(icon);
            assert_eq!(shapes.len(), 1, "{icon:?} must be one joined path");
            assert!(matches!(shapes[0], egui::Shape::Path(_)));
        }
        assert_eq!(
            glyph_shapes(UiIcon::Eye).len(),
            2,
            "one closed eyelid + pupil"
        );
        assert_eq!(
            glyph_shapes(UiIcon::Maximize).len(),
            2,
            "one contour per window"
        );
    }

    #[test]
    fn small_marks_stay_centered_and_leave_the_button_hit_target_alone() {
        for icon in [
            UiIcon::Check,
            UiIcon::Close,
            UiIcon::Up,
            UiIcon::Down,
            UiIcon::Next,
            UiIcon::Previous,
            UiIcon::ZoomIn,
            UiIcon::ZoomOut,
        ] {
            let bounds = glyph_shapes(icon)
                .iter()
                .fold(Rect::NOTHING, |bounds, shape| {
                    bounds.union(shape.visual_bounding_rect())
                });
            assert!(
                bounds.width() <= 8.5 && bounds.height() <= 8.5,
                "{icon:?}: {bounds:?}"
            );
            assert!(
                bounds.center().distance(egui::pos2(27.0, 27.0)) < 1.0,
                "{icon:?}: {bounds:?}"
            );
        }
    }

    #[test]
    fn compiling_toolbar_uses_filled_geometry_and_keeps_its_axle_transparent() {
        for compiling in [false, true] {
            let ctx = egui::Context::default();
            let mut output = ctx.run_ui(Default::default(), |ui| {
                toolbar_button(
                    ui,
                    true,
                    false,
                    UiIcon::Compile,
                    "Compile",
                    crate::settings::ToolbarStyle::Icons,
                    compiling,
                );
            });
            output.textures_delta.clear();
            let meshes: Vec<_> = output
                .shapes
                .iter()
                .filter_map(|shape| match &shape.shape {
                    egui::Shape::Mesh(mesh) => Some(mesh),
                    _ => None,
                })
                .collect();
            assert_eq!(meshes.len(), usize::from(compiling));
            if let Some(mesh) = meshes.first() {
                let center = mesh.calc_bounds().center();
                assert!(mesh.is_valid());
                for indices in mesh.indices.as_chunks::<3>().0 {
                    let points: [Pos2; 3] =
                        std::array::from_fn(|i| mesh.vertices[indices[i] as usize].pos);
                    let mut signs = [0.0; 3];
                    for i in 0..3 {
                        let edge = points[(i + 1) % 3] - points[i];
                        let relative = center - points[i];
                        signs[i] = edge.x * relative.y - edge.y * relative.x;
                    }
                    assert!(
                        signs.iter().any(|s| *s < 0.0) && signs.iter().any(|s| *s > 0.0),
                        "a gear triangle fills the axle hole"
                    );
                }
            }
        }
    }

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
                        rect.expand(0.001).contains_rect(bounds),
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
        for clipped in &harness.output().shapes {
            if matches!(clipped.shape, egui::Shape::Path(_)) {
                assert!(
                    clipped
                        .clip_rect
                        .contains_rect(clipped.shape.visual_bounding_rect()),
                    "the sheet must not clip ink outside the icon's local origin"
                );
            }
        }
        assert!(76.0 + UiIcon::ALL.len().div_ceil(6) as f32 * 80.0 + 152.0 < 900.0);
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
