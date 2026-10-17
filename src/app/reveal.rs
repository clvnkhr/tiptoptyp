//! Bounded reveal feedback shared by the source and PDF preview surfaces.
use super::EditorAttention;
use crate::settings::{RevealAnimation, RevealStyle};
use eframe::egui::{self, Color32, Pos2, Rect, Stroke, Ui, Vec2};
use std::{
    sync::{Arc, Weak},
    time::{Duration, Instant},
};

#[cfg(test)]
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
    pub(super) fn at(elapsed: Duration, animation: RevealAnimation) -> Option<Self> {
        let animation = animation.normalized();
        let duration = Duration::from_millis(u64::from(animation.duration_ms));
        if !animation.enabled || elapsed >= duration {
            return None;
        }
        let seconds = if animation.duration_ms == 900 {
            elapsed.as_secs_f32()
        } else {
            elapsed.as_secs_f32() / duration.as_secs_f32() * 0.9
        };
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
        let fade = (1.0 - seconds / 0.9).clamp(0.0, 1.0);
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
pub(super) fn paint(
    ui: &Ui,
    viewport: Rect,
    target: Rect,
    elapsed: Duration,
    animation: RevealAnimation,
) -> bool {
    let animation = animation.normalized();
    let Some(frame) = Frame::at(elapsed, animation) else {
        return false;
    };
    let accent = crate::theme::palette(ui.ctx()).accent;
    let center = target.center();
    let scale = f32::from(animation.size_percent) / 100.0;
    if !target.is_finite() || !viewport.expand(90.0 * scale).contains(center) {
        return true;
    }
    let painter = ui
        .painter()
        .with_clip_rect(viewport.intersect(ui.clip_rect()));
    let mut wash = egui::epaint::Mesh::default();
    let (wash_radius, wash_strength) = match animation.style {
        RevealStyle::Rings => (52.0, 1.0),
        RevealStyle::Pulse => (28.0 + 6.0 * frame.marker_opacity, 2.0),
        RevealStyle::Spotlight => (86.0, 2.5),
    };
    wash.colored_vertex(
        center,
        with_opacity(accent, frame.wash_opacity * wash_strength),
    );
    const SEGMENTS: u32 = 32;
    for index in 0..SEGMENTS {
        let angle = index as f32 / SEGMENTS as f32 * std::f32::consts::TAU;
        wash.colored_vertex(
            center + Vec2::angled(angle) * wash_radius * scale,
            Color32::TRANSPARENT,
        );
    }
    for index in 0..SEGMENTS {
        wash.add_triangle(0, index + 1, (index + 1) % SEGMENTS + 1);
    }
    painter.add(egui::Shape::mesh(wash));
    for ring in frame.rings {
        if animation.style == RevealStyle::Rings && ring.opacity > 0.0 {
            for (width, color, strength) in [
                (5.5, Color32::BLACK, 0.72),
                (3.5, Color32::WHITE, 0.90),
                (2.0, accent, 1.0),
            ] {
                painter.circle_stroke(
                    center,
                    ring.radius * scale,
                    Stroke::new(width, with_opacity(color, ring.opacity * strength)),
                );
            }
        }
    }
    let half_height = (target.height() * 0.55).clamp(8.0, 16.0) * scale;
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
            animation: None,
        }
    }

    pub(super) fn snapshot(char_index: usize, elapsed: Duration) -> Self {
        Self {
            fixed_elapsed: Some(elapsed),
            ..Self::arm(char_index)
        }
    }

    fn phase(&mut self, now: Instant, visible: bool, animation: RevealAnimation) -> Phase {
        let animation = animation.normalized();
        if !animation.enabled || self.animation.is_some_and(|previous| previous != animation) {
            return Phase::Finished;
        }
        self.animation = Some(animation);
        let duration = Duration::from_millis(u64::from(animation.duration_ms));
        if let Some(elapsed) = self.fixed_elapsed {
            return if elapsed < duration {
                Phase::Showing(elapsed)
            } else {
                Phase::Finished
            };
        }
        if let Some(started) = self.started {
            let elapsed = now.saturating_duration_since(started);
            return if elapsed < duration {
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
    pub(super) fn paint(
        &mut self,
        ui: &Ui,
        viewport: Rect,
        target: Rect,
        animation: RevealAnimation,
    ) -> bool {
        let now = Instant::now();
        match self.phase(
            now,
            target.is_finite() && viewport.contains(target.center()),
            animation,
        ) {
            Phase::Armed => {
                ui.ctx().request_repaint_after(
                    ARM_TIMEOUT.saturating_sub(now.saturating_duration_since(self.armed_at)),
                );
                true
            }
            Phase::Showing(elapsed) => {
                let active = paint(ui, viewport, target, elapsed, animation);
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

#[derive(Clone)]
struct CachedLocalTarget {
    galley: Weak<egui::Galley>,
    char_index: usize,
    rect: Rect,
}

fn cached_local_target(
    context: &egui::Context,
    id: egui::Id,
    galley: &Arc<egui::Galley>,
    char_index: usize,
    locate: impl FnOnce() -> Rect,
) -> Rect {
    if let Some(cached) = context.data(|data| data.get_temp::<CachedLocalTarget>(id))
        && cached.char_index == char_index
        && std::ptr::eq(cached.galley.as_ptr(), Arc::as_ptr(galley))
    {
        return cached.rect;
    }
    let rect = locate();
    context.data_mut(|data| {
        data.insert_temp(
            id,
            CachedLocalTarget {
                galley: Arc::downgrade(galley),
                char_index,
                rect,
            },
        );
    });
    rect
}

/// Reuse local cursor geometry while applying the current scroll translation.
pub(super) fn paint_editor(
    ui: &Ui,
    viewport: Rect,
    galley: &Arc<egui::Galley>,
    galley_pos: Pos2,
    attention: &mut Option<EditorAttention>,
    animation: RevealAnimation,
) {
    #[cfg(any(test, feature = "desktop-ui-tests"))]
    let observation_id = super::source_editor_id(ui.ctx()).with("reveal-observation");
    let Some(state) = attention.as_mut() else {
        #[cfg(any(test, feature = "desktop-ui-tests"))]
        ui.ctx()
            .data_mut(|data| data.remove::<EditorObservation>(observation_id));
        return;
    };
    let target = cached_local_target(
        ui.ctx(),
        super::source_editor_id(ui.ctx()).with("reveal-local-target"),
        galley,
        state.char_index,
        || galley.pos_from_cursor(egui::text::CCursor::new(state.char_index)),
    )
    .translate(galley_pos.to_vec2());
    let active = state.paint(ui, viewport, target, animation);
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

    fn source_galley(context: &egui::Context, text: &str) -> Arc<egui::Galley> {
        let mut galley = None;
        context
            .run_ui(Default::default(), |ui| {
                galley = Some(ui.painter().layout_no_wrap(
                    text.to_owned(),
                    egui::FontId::monospace(12.0),
                    Color32::BLACK,
                ));
            })
            .drop_without_applying_deltas();
        galley.unwrap()
    }

    #[test]
    fn unchanged_source_reveal_maps_its_cursor_once_and_invalidates_on_cursor_or_layout_change() {
        let context = egui::Context::default();
        let galley = source_galley(&context, "first line\nsecond line");
        let key = egui::Id::new("source-reveal-cache-regression");
        let calls = std::cell::Cell::new(0);
        let locate = |galley: &egui::Galley, at| {
            calls.set(calls.get() + 1);
            galley.pos_from_cursor(egui::text::CCursor::new(at))
        };
        let expected = galley.pos_from_cursor(egui::text::CCursor::new(13));
        for frame in 0..60 {
            let local = cached_local_target(&context, key, &galley, 13, || locate(&galley, 13));
            let translation = egui::vec2(frame as f32, -frame as f32);
            assert_eq!(
                local.translate(translation),
                expected.translate(translation)
            );
        }
        assert_eq!(
            calls.get(),
            1,
            "animation-only frames must not traverse source rows"
        );
        let moved = cached_local_target(&context, key, &galley, 15, || locate(&galley, 15));
        assert_eq!(moved, galley.pos_from_cursor(egui::text::CCursor::new(15)));
        assert_eq!(calls.get(), 2);
        let replacement = source_galley(&context, "new layout\nwith different row widths");
        let expected = replacement.pos_from_cursor(egui::text::CCursor::new(15));
        assert_eq!(
            cached_local_target(&context, key, &replacement, 15, || locate(&replacement, 15)),
            expected
        );
        assert_eq!(
            calls.get(),
            3,
            "a new galley must invalidate source geometry"
        );
        // Make this Arc independent of the font engine's own layout cache.
        let owned = Arc::new((*replacement).clone());
        let weak = Arc::downgrade(&owned);
        cached_local_target(&context, key, &owned, 15, || locate(&owned, 15));
        assert_eq!(Arc::strong_count(&owned), 1);
        drop(owned);
        assert!(
            weak.upgrade().is_none(),
            "reveal geometry must not retain document layouts"
        );
    }

    #[test]
    #[ignore = "opt-in paired optimized source reveal geometry cache measurement"]
    fn source_reveal_cache_measurement() {
        use std::hint::black_box;
        assert!(
            !cfg!(debug_assertions),
            "use the optimized release test profile"
        );
        const ROWS: usize = 20_000;
        const ITERATIONS: usize = 1_000;
        let context = egui::Context::default();
        let source = "#let value = 1\n".repeat(ROWS);
        let char_index = source.chars().count() - 3;
        let galley = source_galley(&context, &source);
        let id = egui::Id::new("source-reveal-cache-measurement");
        let cursor = egui::text::CCursor::new(char_index);
        for _ in 0..20 {
            black_box(galley.pos_from_cursor(black_box(cursor)));
            black_box(cached_local_target(
                &context,
                id,
                &galley,
                char_index,
                || galley.pos_from_cursor(cursor),
            ));
        }
        let mut reference = Vec::new();
        let mut cached = Vec::new();
        for _ in 0..5 {
            let start = Instant::now();
            for _ in 0..ITERATIONS {
                black_box(black_box(&galley).pos_from_cursor(black_box(cursor)));
            }
            reference.push(start.elapsed().as_nanos() as f64 / ITERATIONS as f64);
            let start = Instant::now();
            for _ in 0..ITERATIONS {
                black_box(cached_local_target(
                    &context,
                    id,
                    black_box(&galley),
                    black_box(char_index),
                    || galley.pos_from_cursor(cursor),
                ));
            }
            cached.push(start.elapsed().as_nanos() as f64 / ITERATIONS as f64);
        }
        eprintln!(
            "{}",
            serde_json::json!({
                "measurement":"source_reveal_local_geometry",
                "build":crate::build_info::VERSION,
                "os":std::env::consts::OS,
                "arch":std::env::consts::ARCH,
                "optimized":!cfg!(debug_assertions),
                "fixture_rows":ROWS,
                "fixture_characters":source.chars().count(),
                "font":"egui default monospace 12px",
                "layout":"no wrap; font layout excluded from timings",
                "iterations_per_sample":ITERATIONS,
                "warmup_lookups":20,
                "reference_ns_per_lookup":reference,
                "cached_ns_per_lookup":cached,
            })
        );
    }

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
                assert!(paint(
                    ui,
                    viewport,
                    target,
                    Duration::ZERO,
                    RevealAnimation::default()
                ));
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
        let start = Frame::at(Duration::ZERO, RevealAnimation::default()).unwrap();
        assert_eq!(start.rings[0].radius, 12.0);
        assert_eq!(start.rings[1].opacity, 0.0);
        let second = Frame::at(Duration::from_millis(240), RevealAnimation::default()).unwrap();
        assert!(second.rings[0].radius > start.rings[0].radius);
        assert_eq!(second.rings[1].radius, 12.0);
        assert_eq!(second.rings[1].opacity, 1.0);
        for milliseconds in 0..900 {
            let frame = Frame::at(
                Duration::from_millis(milliseconds),
                RevealAnimation::default(),
            )
            .unwrap();
            for ring in frame.rings {
                assert!((12.0..=66.0).contains(&ring.radius));
                assert!((0.0..=1.0).contains(&ring.opacity));
            }
            assert!(frame.wash_opacity <= 0.12);
        }
        assert!(Frame::at(DURATION, RevealAnimation::default()).is_none());
        assert!(Frame::at(Duration::MAX, RevealAnimation::default()).is_none());
    }

    #[test]
    fn offscreen_scroll_wait_does_not_consume_the_visible_animation() {
        let mut attention = EditorAttention::arm(42);
        let armed = attention.armed_at;
        assert_eq!(
            attention.phase(
                armed + Duration::from_millis(750),
                false,
                RevealAnimation::default()
            ),
            Phase::Armed
        );
        let revealed = armed + Duration::from_millis(900);
        assert_eq!(
            attention.phase(revealed, true, RevealAnimation::default()),
            Phase::Showing(Duration::ZERO)
        );
        assert_eq!(
            attention.phase(
                revealed + Duration::from_millis(899),
                true,
                RevealAnimation::default()
            ),
            Phase::Showing(Duration::from_millis(899))
        );
        assert_eq!(
            attention.phase(revealed + DURATION, true, RevealAnimation::default()),
            Phase::Finished
        );
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
                paint_editor(
                    ui,
                    viewport,
                    &galley,
                    origin,
                    &mut attention,
                    RevealAnimation::default(),
                );
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
                paint_editor(
                    ui,
                    viewport,
                    &galley,
                    origin,
                    &mut attention,
                    RevealAnimation::default(),
                );
                assert!(editor_observation(ui.ctx()).is_none());
            })
            .drop_without_applying_deltas();
    }

    #[test]
    fn snapshot_phase_stays_fixed_without_waiting_or_expiring() {
        let elapsed = Duration::from_millis(120);
        let mut attention = EditorAttention::snapshot(42, elapsed);
        assert_eq!(
            attention.phase(
                attention.armed_at + ARM_TIMEOUT + DURATION,
                false,
                RevealAnimation::default()
            ),
            Phase::Showing(elapsed)
        );
        assert!(attention.started.is_none());
    }

    #[test]
    fn settings_control_both_timing_and_retirement_without_extending_an_active_cue() {
        for duration_ms in [250, 1500, 2000] {
            let animation = RevealAnimation {
                duration_ms,
                ..Default::default()
            };
            let mut attention = EditorAttention::arm(0);
            let start = attention.armed_at;
            assert_eq!(
                attention.phase(start, true, animation),
                Phase::Showing(Duration::ZERO)
            );
            let duration = Duration::from_millis(u64::from(duration_ms));
            assert!(Frame::at(duration - Duration::from_millis(1), animation).is_some());
            assert_eq!(
                attention.phase(start + duration, true, animation),
                Phase::Finished
            );
            assert!(Frame::at(duration, animation).is_none());
        }
        let disabled = RevealAnimation {
            enabled: false,
            ..Default::default()
        };
        let mut attention = EditorAttention::arm(0);
        assert_eq!(
            attention.phase(attention.armed_at, false, disabled),
            Phase::Finished
        );
        assert!(Frame::at(Duration::ZERO, disabled).is_none());
        assert!(attention.started.is_none());
        let mut attention = EditorAttention::arm(0);
        attention.phase(attention.armed_at, true, Default::default());
        let changed = RevealAnimation {
            duration_ms: 2000,
            ..Default::default()
        };
        assert_eq!(
            attention.phase(
                attention.armed_at + Duration::from_millis(10),
                true,
                changed
            ),
            Phase::Finished
        );
    }

    #[test]
    fn styles_and_size_only_change_painted_shapes_inside_the_same_clip() {
        let context = egui::Context::default();
        for style in RevealStyle::ALL {
            for size_percent in [50, 100, 200] {
                let output = context.run_ui(Default::default(), |ui| {
                    let viewport = Rect::from_min_size(Pos2::new(20.0, 20.0), Vec2::splat(200.0));
                    let target = Rect::from_center_size(viewport.center(), Vec2::new(1.0, 18.0));
                    let before = ui.min_rect();
                    assert!(paint(
                        ui,
                        viewport,
                        target,
                        Duration::ZERO,
                        RevealAnimation {
                            style,
                            size_percent,
                            ..Default::default()
                        }
                    ));
                    assert_eq!(before, ui.min_rect());
                });
                let circles = output
                    .shapes
                    .iter()
                    .filter_map(|shape| {
                        if let egui::Shape::Circle(circle) = &shape.shape {
                            assert_eq!(
                                shape.clip_rect,
                                Rect::from_min_size(Pos2::new(20.0, 20.0), Vec2::splat(200.0))
                            );
                            Some(circle.radius)
                        } else {
                            None
                        }
                    })
                    .collect::<Vec<_>>();
                if style == RevealStyle::Rings {
                    assert_eq!(circles.len(), 3);
                    assert!(
                        circles
                            .iter()
                            .all(|radius| *radius == 12.0 * f32::from(size_percent) / 100.0)
                    );
                } else {
                    assert!(circles.is_empty());
                }
                output.drop_without_applying_deltas();
            }
        }
    }

    #[test]
    fn unresolved_reveals_expire_and_explicit_repeated_jumps_rearm() {
        let mut attention = EditorAttention::arm(42);
        assert_eq!(
            attention.phase(
                attention.armed_at + ARM_TIMEOUT,
                false,
                RevealAnimation::default()
            ),
            Phase::Finished
        );
        attention = EditorAttention::arm(42);
        assert_eq!(attention.started, None);
        assert_eq!(
            attention.phase(attention.armed_at, true, RevealAnimation::default()),
            Phase::Showing(Duration::ZERO)
        );
        attention = EditorAttention::arm(42);
        assert_eq!(attention.started, None);
    }
}
