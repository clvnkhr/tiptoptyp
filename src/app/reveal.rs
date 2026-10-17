//! Bounded reveal feedback shared by the source and PDF preview surfaces.
use super::EditorAttention;
use eframe::egui::{self, Color32, Pos2, Rect, Stroke, Ui, Vec2};
use std::time::{Duration, Instant};

pub(super) const DURATION: Duration = Duration::from_millis(900);
const ARM_TIMEOUT: Duration = Duration::from_secs(2);
const RING_DURATION: f32 = 0.66;
const SECOND_RING_DELAY: f32 = 0.24;

#[derive(Clone, Copy, Debug)]
pub(super) struct Ring {
    pub(super) radius: f32,
    pub(super) opacity: f32,
}

#[derive(Clone, Copy, Debug)]
pub(super) struct Frame {
    pub(super) rings: [Ring; 2],
    pub(super) wash_opacity: f32,
    pub(super) marker_opacity: f32,
}

impl Frame {
    pub(super) fn at(elapsed: Duration) -> Option<Self> {
        if elapsed >= DURATION {
            return None;
        }
        let seconds = elapsed.as_secs_f32();
        let ring = |delay: f32| {
            let progress = ((seconds - delay) / RING_DURATION).clamp(0.0, 1.0);
            Ring {
                radius: 12.0 + 54.0 * (1.0 - (1.0 - progress).powi(2)),
                opacity: if seconds < delay {
                    0.0
                } else {
                    (1.0 - progress).powf(0.8)
                },
            }
        };
        let fade = (1.0 - seconds / DURATION.as_secs_f32()).clamp(0.0, 1.0);
        Some(Self {
            rings: [ring(0.0), ring(SECOND_RING_DELAY)],
            wash_opacity: 0.12 * fade,
            marker_opacity: fade.sqrt(),
        })
    }
}

fn with_opacity(color: Color32, opacity: f32) -> Color32 {
    Color32::from_rgba_unmultiplied(
        color.r(),
        color.g(),
        color.b(),
        (opacity.clamp(0.0, 1.0) * 255.0).round() as u8,
    )
}

/// Paint above the surface's text, with no widgets, geometry, or input changes.
/// The return value is false once the animation is completely finished.
pub(super) fn paint(ui: &Ui, viewport: Rect, target: Rect, elapsed: Duration) -> bool {
    let Some(frame) = Frame::at(elapsed) else {
        return false;
    };
    let accent = crate::theme::palette(ui.ctx()).accent;
    let center = target.center();
    if !target.is_finite() || !viewport.expand(68.0).contains(center) {
        return true;
    }
    let painter = ui
        .painter()
        .with_clip_rect(viewport.intersect(ui.clip_rect()));
    let mut wash = egui::epaint::Mesh::default();
    wash.colored_vertex(center, with_opacity(accent, frame.wash_opacity));
    const SEGMENTS: u32 = 32;
    for index in 0..SEGMENTS {
        let angle = index as f32 / SEGMENTS as f32 * std::f32::consts::TAU;
        wash.colored_vertex(center + Vec2::angled(angle) * 52.0, Color32::TRANSPARENT);
    }
    for index in 0..SEGMENTS {
        wash.add_triangle(0, index + 1, (index + 1) % SEGMENTS + 1);
    }
    painter.add(egui::Shape::mesh(wash));
    for ring in frame.rings {
        if ring.opacity > 0.0 {
            for (width, color, strength) in [
                (5.5, Color32::BLACK, 0.72),
                (3.5, Color32::WHITE, 0.90),
                (2.0, accent, 1.0),
            ] {
                painter.circle_stroke(
                    center,
                    ring.radius,
                    Stroke::new(width, with_opacity(color, ring.opacity * strength)),
                );
            }
        }
    }
    let half_height = (target.height() * 0.55).clamp(8.0, 16.0);
    let marker = [
        Pos2::new(center.x, center.y - half_height),
        Pos2::new(center.x, center.y + half_height),
    ];
    for (width, color, strength) in [
        (5.5, Color32::BLACK, 0.72),
        (3.5, Color32::WHITE, 0.90),
        (2.0, accent, 1.0),
    ] {
        painter.line_segment(
            marker,
            Stroke::new(width, with_opacity(color, frame.marker_opacity * strength)),
        );
    }
    true
}

#[derive(Debug, PartialEq, Eq)]
enum Phase {
    Armed,
    Showing(Duration),
    Finished,
}

impl EditorAttention {
    pub(super) fn arm(char_index: usize) -> Self {
        Self {
            char_index,
            armed_at: Instant::now(),
            started: None,
            fixed_elapsed: None,
        }
    }

    pub(super) fn snapshot(char_index: usize, elapsed: Duration) -> Self {
        Self {
            fixed_elapsed: Some(elapsed),
            ..Self::arm(char_index)
        }
    }

    fn phase(&mut self, now: Instant, visible: bool) -> Phase {
        if let Some(elapsed) = self.fixed_elapsed {
            return if elapsed < DURATION {
                Phase::Showing(elapsed)
            } else {
                Phase::Finished
            };
        }
        if let Some(started) = self.started {
            let elapsed = now.saturating_duration_since(started);
            return if elapsed < DURATION {
                Phase::Showing(elapsed)
            } else {
                Phase::Finished
            };
        }
        if now.saturating_duration_since(self.armed_at) >= ARM_TIMEOUT {
            return Phase::Finished;
        }
        if visible {
            self.started = Some(now);
            Phase::Showing(Duration::ZERO)
        } else {
            Phase::Armed
        }
    }

    /// Begin only after deferred scrolling has brought the target into view.
    /// An unresolved offscreen request has one expiry wakeup, not a frame loop.
    pub(super) fn paint(&mut self, ui: &Ui, viewport: Rect, target: Rect) -> bool {
        let now = Instant::now();
        match self.phase(
            now,
            target.is_finite() && viewport.contains(target.center()),
        ) {
            Phase::Armed => {
                ui.ctx().request_repaint_after(
                    ARM_TIMEOUT.saturating_sub(now.saturating_duration_since(self.armed_at)),
                );
                true
            }
            Phase::Showing(elapsed) => {
                let active = paint(ui, viewport, target, elapsed);
                if active && self.fixed_elapsed.is_none() {
                    ui.ctx()
                        .request_repaint_after(crate::theme::METRICS.motion.animation_frame);
                }
                active
            }
            Phase::Finished => false,
        }
    }
}

#[cfg(any(test, feature = "desktop-ui-tests"))]
#[derive(Clone, Copy, Debug, serde::Serialize)]
pub(super) struct EditorObservation {
    pub(super) center: [f32; 2],
    pub(super) target: [f32; 4],
    pub(super) viewport: [f32; 4],
    pub(super) elapsed_ms: u64,
    pub(super) active: bool,
    pub(super) char_index: usize,
    #[serde(skip)]
    frame: u64,
}

#[cfg(any(test, feature = "desktop-ui-tests"))]
pub(super) fn editor_observation(context: &egui::Context) -> Option<EditorObservation> {
    let id = super::source_editor_id(context).with("reveal-observation");
    let frame = context.cumulative_frame_nr();
    context
        .data(|data| data.get_temp::<EditorObservation>(id))
        .filter(|observation| observation.frame.saturating_add(1) >= frame)
}

/// The galley's cursor mapping clamps stale offsets without scanning the source.
pub(super) fn paint_editor(
    ui: &Ui,
    viewport: Rect,
    galley: &egui::Galley,
    galley_pos: Pos2,
    attention: &mut Option<EditorAttention>,
) {
    #[cfg(any(test, feature = "desktop-ui-tests"))]
    let observation_id = super::source_editor_id(ui.ctx()).with("reveal-observation");
    let Some(state) = attention.as_mut() else {
        #[cfg(any(test, feature = "desktop-ui-tests"))]
        ui.ctx()
            .data_mut(|data| data.remove::<EditorObservation>(observation_id));
        return;
    };
    let target = galley
        .pos_from_cursor(egui::text::CCursor::new(state.char_index))
        .translate(galley_pos.to_vec2());
    let active = state.paint(ui, viewport, target);
    #[cfg(any(test, feature = "desktop-ui-tests"))]
    {
        let elapsed = state
            .fixed_elapsed
            .or_else(|| state.started.map(|started| started.elapsed()));
        if let Some(elapsed) = elapsed.filter(|_| {
            active && target.is_finite() && viewport.expand(68.0).contains(target.center())
        }) {
            let center = target.center();
            let frame = ui.ctx().cumulative_frame_nr();
            ui.ctx().data_mut(|data| {
                data.insert_temp(
                    observation_id,
                    EditorObservation {
                        center: [center.x, center.y],
                        target: [target.left(), target.top(), target.right(), target.bottom()],
                        viewport: [
                            viewport.left(),
                            viewport.top(),
                            viewport.right(),
                            viewport.bottom(),
                        ],
                        elapsed_ms: elapsed.as_millis().min(u64::MAX as u128) as u64,
                        active: true,
                        char_index: state.char_index,
                        frame,
                    },
                )
            });
        } else {
            ui.ctx()
                .data_mut(|data| data.remove::<EditorObservation>(observation_id));
        }
    }
    if !active {
        *attention = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reveal_painter_follows_live_theme_accent() {
        let context = egui::Context::default();
        for id in ["catppuccin-latte", "catppuccin-mocha"] {
            let theme = crate::builtin_themes::find(id).unwrap();
            crate::theme::set_imported_palette(&context, Some((theme.dark_mode, theme.palette)));
            let accent = crate::theme::palette(&context).accent;
            let output = context.run_ui(Default::default(), |ui| {
                let viewport = ui.clip_rect();
                let target = Rect::from_center_size(viewport.center(), Vec2::new(1.0, 18.0));
                assert!(paint(ui, viewport, target, Duration::ZERO));
            });
            assert!(output.shapes.iter().any(|shape| {
                matches!(&shape.shape, egui::Shape::Circle(circle) if circle.stroke.color == accent)
            }), "{id}: the ring must use the current theme accent");
            assert!(output.shapes.iter().any(|shape| {
                matches!(&shape.shape, egui::Shape::LineSegment { stroke, .. } if stroke.color == accent)
            }), "{id}: the destination marker must use the same accent");
            output.drop_without_applying_deltas();
        }
    }

    #[test]
    fn two_expanding_pulses_finish_completely_after_nine_hundred_milliseconds() {
        let start = Frame::at(Duration::ZERO).unwrap();
        assert_eq!(start.rings[0].radius, 12.0);
        assert_eq!(start.rings[1].opacity, 0.0);
        let second = Frame::at(Duration::from_millis(240)).unwrap();
        assert!(second.rings[0].radius > start.rings[0].radius);
        assert_eq!(second.rings[1].radius, 12.0);
        assert_eq!(second.rings[1].opacity, 1.0);
        for milliseconds in 0..900 {
            let frame = Frame::at(Duration::from_millis(milliseconds)).unwrap();
            for ring in frame.rings {
                assert!((12.0..=66.0).contains(&ring.radius));
                assert!((0.0..=1.0).contains(&ring.opacity));
            }
            assert!(frame.wash_opacity <= 0.12);
        }
        assert!(Frame::at(DURATION).is_none());
        assert!(Frame::at(Duration::MAX).is_none());
    }

    #[test]
    fn offscreen_scroll_wait_does_not_consume_the_visible_animation() {
        let mut attention = EditorAttention::arm(42);
        let armed = attention.armed_at;
        assert_eq!(
            attention.phase(armed + Duration::from_millis(750), false),
            Phase::Armed
        );
        let revealed = armed + Duration::from_millis(900);
        assert_eq!(
            attention.phase(revealed, true),
            Phase::Showing(Duration::ZERO)
        );
        assert_eq!(
            attention.phase(revealed + Duration::from_millis(899), true),
            Phase::Showing(Duration::from_millis(899))
        );
        assert_eq!(attention.phase(revealed + DURATION, true), Phase::Finished);
    }

    #[test]
    fn source_observation_tracks_the_painted_caret_and_clears_with_the_cue() {
        let context = egui::Context::default();
        let mut attention = Some(EditorAttention::snapshot(2, Duration::from_millis(120)));
        context
            .run_ui(Default::default(), |ui| {
                let viewport = ui.clip_rect();
                let origin = viewport.center();
                let galley = ui.painter().layout_no_wrap(
                    "Source destination".into(),
                    egui::FontId::monospace(16.0),
                    Color32::BLACK,
                );
                ui.painter().galley(origin, galley.clone(), Color32::BLACK);
                paint_editor(ui, viewport, &galley, origin, &mut attention);
                let observation = editor_observation(ui.ctx()).unwrap();
                let target = galley
                    .pos_from_cursor(egui::text::CCursor::new(2))
                    .translate(origin.to_vec2());
                assert_eq!(observation.center, [target.center().x, target.center().y]);
                assert_eq!(observation.elapsed_ms, 120);
                assert_eq!(observation.char_index, 2);
                assert!(observation.active);
                assert_eq!(
                    observation.target,
                    [target.left(), target.top(), target.right(), target.bottom()]
                );
                assert_eq!(
                    observation.viewport,
                    [
                        viewport.left(),
                        viewport.top(),
                        viewport.right(),
                        viewport.bottom()
                    ]
                );
                attention = None;
                paint_editor(ui, viewport, &galley, origin, &mut attention);
                assert!(editor_observation(ui.ctx()).is_none());
            })
            .drop_without_applying_deltas();
    }

    #[test]
    fn snapshot_phase_stays_fixed_without_waiting_or_expiring() {
        let elapsed = Duration::from_millis(120);
        let mut attention = EditorAttention::snapshot(42, elapsed);
        assert_eq!(
            attention.phase(attention.armed_at + ARM_TIMEOUT + DURATION, false),
            Phase::Showing(elapsed)
        );
        assert!(attention.started.is_none());
    }

    #[test]
    fn unresolved_reveals_expire_and_explicit_repeated_jumps_rearm() {
        let mut attention = EditorAttention::arm(42);
        assert_eq!(
            attention.phase(attention.armed_at + ARM_TIMEOUT, false),
            Phase::Finished
        );
        attention = EditorAttention::arm(42);
        assert_eq!(attention.started, None);
        assert_eq!(
            attention.phase(attention.armed_at, true),
            Phase::Showing(Duration::ZERO)
        );
        attention = EditorAttention::arm(42);
        assert_eq!(attention.started, None);
    }
}
