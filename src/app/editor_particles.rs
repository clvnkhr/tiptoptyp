//! Finite typing feedback over the existing editor, with no input or layout widgets.
use crate::{
    document::DocumentKey,
    settings::{EditorParticleSettings, ParticleDensity},
    theme,
};
use eframe::egui::{self, Id, Pos2, Rect, Ui, Vec2, text_edit::TextEditOutput};
use std::sync::{Arc, Mutex};

#[allow(unsafe_code)]
mod gpu;
pub(crate) use gpu::Renderer;
#[cfg(test)]
mod gpu_tests;

const LIMIT: usize = 256;
const MAX_INPUT_EVENTS: usize = 64;
const LIFETIME: f64 = 0.64;

#[derive(Clone, Copy, Debug, Default)]
pub(super) struct TypingIntent {
    events: u8,
    ime_commit: bool,
}

fn typing_events(events: &[egui::Event], enabled: bool, focused: bool) -> TypingIntent {
    if !enabled || !focused || events.len() > MAX_INPUT_EVENTS {
        return TypingIntent::default();
    }
    let mut intent = TypingIntent::default();
    for event in events {
        let (text, commit) = match event {
            // Clipboard insertion must not turn into a typing burst, including
            // native batches which also contain text/key events.
            egui::Event::Paste(_) => return TypingIntent::default(),
            egui::Event::Text(text) => (text, false),
            egui::Event::Ime(egui::ImeEvent::Commit(text)) => (text, true),
            _ => continue,
        };
        // Real keystrokes and ordinary IME phrases are small. Conservatively
        // ignore bulk synthetic text rather than walking arbitrary input data.
        if text.len() <= 256 && text.chars().take(16).any(|ch| !ch.is_control()) {
            intent.events = intent.events.saturating_add(1).min(4);
            intent.ime_commit |= commit;
        }
    }
    intent
}

/// Capture before TextEdit consumes events. An actual edit must still admit it.
pub(super) fn capture(ui: &Ui, editor: Id, enabled: bool) -> TypingIntent {
    if !enabled {
        return TypingIntent::default();
    }
    let focused = ui.memory(|memory| memory.has_focus(editor));
    ui.input(|input| typing_events(&input.events, enabled, focused))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum View {
    Source,
    Diff,
}

pub(super) struct Surface<'a> {
    pub(super) edit: &'a TextEditOutput,
    pub(super) document: DocumentKey,
    pub(super) viewport: Rect,
    pub(super) view: View,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Identity {
    document: DocumentKey,
    view: View,
}
impl Identity {
    fn new(mut document: DocumentKey, view: View) -> Self {
        // Ordinary typing must retain earlier sparks from the same document.
        document.revision = 0;
        Self { document, view }
    }
}

#[derive(Clone, Copy, Debug)]
struct Particle {
    origin: Pos2,
    velocity: Vec2,
    radius: f32,
    color_seed: f32,
    flash: bool,
    born: f64,
    frame: u64,
}
#[cfg(test)]
impl Particle {
    // Keep the original state-only measurement workload comparable. Runtime
    // motion, drag, trails and glow are calculated only by the GPU shaders.
    fn position(self, now: f64) -> Pos2 {
        let age = (now - self.born).max(0.0) as f32;
        self.origin + self.velocity * age + egui::vec2(0.0, 75.0 * age * age)
    }
}

#[derive(Default)]
struct State {
    identity: Option<Identity>,
    particles: Vec<Particle>,
    last_frame: Option<u64>,
    last_time: f64,
    emitted_frame: Option<u64>,
    bursts: u64,
    seed: u32,
    base_time: f64,
    gpu_instances: Option<Arc<[gpu::Instance]>>,
}
impl State {
    fn random(&mut self) -> f32 {
        self.seed = self
            .seed
            .wrapping_mul(1_664_525)
            .wrapping_add(1_013_904_223);
        (self.seed >> 8) as f32 / 16_777_215.0
    }

    fn update(
        &mut self,
        identity: Identity,
        frame: u64,
        now: f64,
        caret: Option<Pos2>,
        emission: usize,
    ) {
        if !now.is_finite() {
            self.particles.clear();
            self.gpu_instances = None;
            return;
        }
        if self.identity != Some(identity)
            || self
                .last_frame
                .is_some_and(|last| frame > last.saturating_add(1))
            || now < self.last_time
        {
            self.particles.clear();
            self.gpu_instances = None;
            self.emitted_frame = None;
        }
        self.identity = Some(identity);
        self.last_frame = Some(frame);
        self.last_time = now;
        let previous_count = self.particles.len();
        self.particles
            .retain(|particle| now - particle.born < LIFETIME);
        if previous_count != self.particles.len() {
            self.gpu_instances = None;
        }
        // A discarded layout pass must neither emit twice nor leave its new
        // particles at the first pass's old caret coordinates.
        if self.emitted_frame == Some(frame) {
            if let Some(caret) = caret {
                for particle in self
                    .particles
                    .iter_mut()
                    .filter(|particle| particle.frame == frame)
                {
                    if particle.origin != caret {
                        particle.origin = caret;
                        self.gpu_instances = None;
                    }
                }
            }
            return;
        }
        let Some(origin) = caret.filter(|_| emission > 0) else {
            return;
        };
        if self.particles.is_empty() {
            self.base_time = now;
        }
        let count = emission.min(LIMIT);
        let excess = (self.particles.len() + count).saturating_sub(LIMIT);
        self.particles.drain(..excess);
        if self.particles.capacity() == 0 {
            self.particles.reserve_exact(LIMIT);
        }
        for index in 0..count {
            // Uneven full-circle velocities give the burst the ballistic,
            // chaotic character of small game explosions, without a rosette.
            let angle = self.random() * std::f32::consts::TAU;
            let speed = 65.0 + self.random().powi(2) * 255.0;
            let radius = 0.7 + self.random() * 1.0;
            let color_seed = self.random();
            self.particles.push(Particle {
                origin,
                velocity: if index == 0 {
                    Vec2::ZERO
                } else {
                    Vec2::angled(angle) * speed
                },
                radius: if index == 0 { 18.0 } else { radius },
                color_seed,
                flash: index == 0,
                born: now,
                frame,
            });
        }
        self.gpu_instances = None;
        self.emitted_frame = Some(frame);
        self.bursts = self.bursts.wrapping_add(1);
    }

    fn instances(&mut self) -> Arc<[gpu::Instance]> {
        self.gpu_instances
            .get_or_insert_with(|| {
                self.particles
                    .iter()
                    .map(|particle| {
                        [
                            particle.origin.x,
                            particle.origin.y,
                            particle.velocity.x,
                            particle.velocity.y,
                            particle.radius,
                            (particle.born - self.base_time) as f32,
                            particle.color_seed,
                            if particle.flash {
                                1.0
                            } else if particle.color_seed > 0.9 {
                                3.0
                            } else if particle.color_seed > 0.68 {
                                2.0
                            } else {
                                0.0
                            },
                        ]
                    })
                    .collect::<Vec<_>>()
                    .into()
            })
            .clone()
    }
}

type SharedState = Arc<Mutex<State>>;
fn state_id(editor: Id) -> Id {
    editor.with("typing-particles")
}
fn density_count(density: ParticleDensity) -> usize {
    match density {
        ParticleDensity::Low => 24,
        ParticleDensity::Medium => 48,
        ParticleDensity::High => 80,
    }
}

// Cursor lookup can walk the galley, so animation-only frames must never do it.
fn caret_for_frame(
    enabled: bool,
    accepted: bool,
    realigning: bool,
    locate: impl FnOnce() -> Option<Pos2>,
) -> Option<Pos2> {
    if enabled && (accepted || realigning) {
        locate()
    } else {
        None
    }
}

pub(super) fn paint(
    ui: &Ui,
    surface: Surface<'_>,
    settings: EditorParticleSettings,
    intent: TypingIntent,
    document_changed: bool,
) {
    let context = ui.ctx();
    let id = state_id(surface.edit.response.id);
    if !settings.enabled {
        context.data_mut(|data| data.remove::<SharedState>(id));
        return;
    }
    let existing = context.data(|data| data.get_temp::<SharedState>(id));
    let focused = surface.edit.response.has_focus();
    let accepted = focused
        && intent.events > 0
        && (document_changed || (intent.ime_commit && surface.edit.response.changed()));
    if existing.is_none() && !accepted {
        Renderer::for_context(context).warm(ui, surface.viewport.intersect(ui.clip_rect()));
        return;
    }
    let state = existing.unwrap_or_else(|| {
        let state = SharedState::default();
        context.data_mut(|data| data.insert_temp(id, state.clone()));
        state
    });
    let mut state = state.lock().unwrap();
    let frame = context.cumulative_frame_nr();
    let viewport = surface.viewport.intersect(ui.clip_rect());
    let caret = caret_for_frame(
        settings.enabled,
        accepted,
        state.emitted_frame == Some(frame) && !state.particles.is_empty(),
        || {
            surface
                .edit
                .state
                .cursor
                .char_range()
                .map(|range| {
                    surface
                        .edit
                        .galley
                        .pos_from_cursor(range.primary)
                        .translate(surface.edit.galley_pos.to_vec2())
                        .center()
                })
                .filter(|point| point.is_finite() && viewport.contains(*point))
        },
    );
    let now = context.input(|input| input.time);
    #[cfg(all(feature = "desktop-ui-tests", target_os = "macos"))]
    let bursts_before = state.bursts;
    state.update(
        Identity::new(surface.document, surface.view),
        frame,
        now,
        caret,
        if accepted {
            usize::from(intent.events) * density_count(settings.density)
        } else {
            0
        },
    );
    #[cfg(all(feature = "desktop-ui-tests", target_os = "macos"))]
    if state.bursts != bursts_before {
        record_emission(context, id, &state, now, viewport);
    }
    if state.particles.is_empty() {
        drop(state);
        context.data_mut(|data| data.remove::<SharedState>(id));
        return;
    }
    let instances = state.instances();
    let time = (now - state.base_time) as f32;
    drop(state);
    Renderer::for_context(context).submit(
        ui,
        viewport,
        instances,
        time,
        theme::palette(context).accent,
        ui.visuals().dark_mode,
    );
    context.request_repaint_after(theme::METRICS.motion.animation_frame);
}

#[cfg(all(feature = "desktop-ui-tests", target_os = "macos"))]
#[derive(Clone)]
struct Trace {
    identity: Identity,
    burst_count: u64,
    emitted_count: u64,
    max_live: usize,
    last_emission: f64,
    origin: Pos2,
    clip: Rect,
}

#[cfg(all(feature = "desktop-ui-tests", target_os = "macos"))]
fn record_emission(context: &egui::Context, id: Id, state: &State, now: f64, clip: Rect) {
    let Some(identity) = state.identity else {
        return;
    };
    let Some(particle) = state.particles.last() else {
        return;
    };
    let emitted = state
        .particles
        .iter()
        .filter(|p| p.frame == particle.frame)
        .count();
    context.data_mut(|data| {
        let mut trace = data
            .get_temp::<Trace>(id.with("trace"))
            .filter(|trace| trace.identity.document == identity.document)
            .unwrap_or(Trace {
                identity,
                burst_count: 0,
                emitted_count: 0,
                max_live: 0,
                last_emission: now,
                origin: particle.origin,
                clip,
            });
        trace.identity = identity;
        trace.burst_count = trace.burst_count.saturating_add(1);
        trace.emitted_count = trace.emitted_count.saturating_add(emitted as u64);
        trace.max_live = trace.max_live.max(state.particles.len());
        trace.last_emission = now;
        trace.origin = particle.origin;
        trace.clip = clip;
        data.insert_temp(id.with("trace"), trace);
    });
}

/// Read-only native evidence. Counts survive expiry so paste/navigation checks
/// cannot pass merely by observing a short animation after it has finished.
#[cfg(all(feature = "desktop-ui-tests", target_os = "macos"))]
pub(super) fn inspected(
    context: &egui::Context,
    editor: Id,
    document: DocumentKey,
) -> serde_json::Value {
    let id = state_id(editor);
    let identity = Identity::new(document, View::Source).document;
    let (state, trace) = context.data(|data| {
        (
            data.get_temp::<SharedState>(id),
            data.get_temp::<Trace>(id.with("trace"))
                .filter(|trace| trace.identity.document == identity),
        )
    });
    let now = context.input(|input| input.time);
    let active_count = state.map_or(0, |state| {
        let state = state.lock().unwrap();
        if state.identity.is_none_or(|key| key.document != identity) {
            return 0;
        }
        state
            .particles
            .iter()
            .filter(|particle| (0.0..LIFETIME).contains(&(now - particle.born)))
            .count()
    });
    serde_json::json!({
        "gpu": Renderer::for_context(context).inspected(),
        "active_count": active_count,
        "burst_count": trace.as_ref().map_or(0, |trace| trace.burst_count),
        "emitted_count": trace.as_ref().map_or(0, |trace| trace.emitted_count),
        "max_live": trace.as_ref().map_or(0, |trace| trace.max_live),
        "pool_limit": LIMIT,
        "lifetime_ms": (LIFETIME * 1000.0) as u64,
        "last_emission_age_ms": trace.as_ref().map(|trace| (now - trace.last_emission).max(0.0) * 1000.0),
        "origin": trace.as_ref().map(|trace| [trace.origin.x, trace.origin.y]),
        "clip": trace.as_ref().map(|trace| [trace.clip.left(), trace.clip.top(), trace.clip.right(), trace.clip.bottom()]),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::{EditorApp, EditorSelection, source_editor_id};
    use egui_kittest::Harness;
    use tiptoptyp_core::document::WindowSessionId;

    fn identity(epoch: u64, revision: u64, view: View) -> Identity {
        Identity::new(
            DocumentKey::new(WindowSessionId::new(1), epoch, revision),
            view,
        )
    }

    #[test]
    fn cursor_geometry_is_lazy_for_disabled_idle_and_animation_only_frames() {
        let calls = std::cell::Cell::new(0);
        let locate = || {
            calls.set(calls.get() + 1);
            Some(egui::pos2(8.0, 12.0))
        };
        // Disabled even with pending input, enabled idle, and active particles
        // advancing on later frames all skip cursor/galley geometry entirely.
        for (enabled, accepted, realigning) in [
            (false, true, true),
            (false, false, false),
            (true, false, false),
        ] {
            assert_eq!(caret_for_frame(enabled, accepted, realigning, locate), None);
        }
        assert_eq!(calls.get(), 0);
        for (accepted, realigning) in [(true, false), (false, true), (true, true)] {
            assert_eq!(
                caret_for_frame(true, accepted, realigning, locate),
                Some(egui::pos2(8.0, 12.0))
            );
        }
        assert_eq!(calls.get(), 3, "one lookup per edit or final layout pass");
    }

    #[test]
    fn bounded_pool_retains_typing_history_without_duplicate_passes_and_expires() {
        let mut state = State::default();
        for frame in 0..100 {
            state.update(
                identity(1, frame, View::Source),
                frame,
                frame as f64 * 0.001,
                Some(egui::pos2(80.0, 90.0)),
                density_count(ParticleDensity::High),
            );
            assert!(state.particles.len() <= LIMIT);
        }
        assert_eq!(state.particles.len(), LIMIT);
        let bursts = state.bursts;
        state.update(
            identity(1, 99, View::Source),
            99,
            0.099,
            Some(egui::pos2(91.0, 95.0)),
            12,
        );
        assert_eq!(state.bursts, bursts);
        assert!(
            state
                .particles
                .iter()
                .filter(|p| p.frame == 99)
                .all(|p| p.origin == egui::pos2(91.0, 95.0))
        );
        state.update(identity(1, 100, View::Source), 100, 0.8, None, 0);
        assert!(state.particles.is_empty());
    }

    #[test]
    fn explosion_covers_all_quadrants_and_instance_snapshots_only_change_with_particles() {
        let mut state = State::default();
        let key = identity(1, 0, View::Source);
        state.update(
            key,
            0,
            500_000.0,
            Some(egui::pos2(80.0, 90.0)),
            density_count(ParticleDensity::Medium),
        );
        let first = state.instances();
        assert_eq!(first.len(), 48);
        assert_eq!(first.iter().filter(|p| p[7] == 1.0).count(), 1);
        assert!(first.iter().flatten().all(|value| value.is_finite()));
        assert!(
            first.iter().all(|p| p[5] == 0.0),
            "birth times stay relative even after a long-running session"
        );
        let mut quadrants = [false; 4];
        for particle in state.particles.iter().filter(|particle| !particle.flash) {
            let x = usize::from(particle.velocity.x >= 0.0);
            let y = usize::from(particle.velocity.y >= 0.0);
            quadrants[x + y * 2] = true;
        }
        assert!(quadrants.into_iter().all(|present| present));
        state.update(key, 1, 500_000.1, None, 0);
        assert!(
            Arc::ptr_eq(&first, &state.instances()),
            "animation advances only the GPU time uniform"
        );
        state.update(key, 1, 500_000.1, Some(egui::pos2(82.0, 92.0)), 12);
        let second = state.instances();
        assert!(!Arc::ptr_eq(&first, &second));
        assert_eq!(second.len(), 60);
        state.update(key, 2, 500_000.65, None, 0);
        let pruned = state.instances();
        assert!(!Arc::ptr_eq(&second, &pruned));
        assert_eq!(pruned.len(), 12);
        state.update(key, 3, 500_001.0, None, 0);
        assert!(state.instances().is_empty());
    }

    #[test]
    fn document_surface_and_hidden_frame_transitions_retire_previous_particles() {
        let mut state = State::default();
        for (frame, key) in [
            identity(1, 0, View::Source),
            identity(2, 0, View::Source),
            identity(2, 0, View::Diff),
        ]
        .into_iter()
        .enumerate()
        {
            state.update(
                key,
                frame as u64,
                0.01 * frame as f64,
                Some(egui::pos2(10.0, 10.0)),
                7,
            );
            assert_eq!(state.particles.len(), 7);
        }
        state.update(identity(2, 1, View::Diff), 4, 0.05, None, 0);
        assert!(state.particles.is_empty());
    }

    #[test]
    fn only_small_typing_and_commits_are_admitted_not_paste_preedit_or_bulk_input() {
        let typed = egui::Event::Text("α".into());
        assert_eq!(
            typing_events(std::slice::from_ref(&typed), true, true).events,
            1
        );
        assert!(
            typing_events(
                &[egui::Event::Ime(egui::ImeEvent::Commit("日本".into()))],
                true,
                true
            )
            .ime_commit
        );
        for (events, enabled, focused) in [
            (vec![typed.clone()], false, true),
            (vec![typed.clone()], true, false),
            (
                vec![typed.clone(), egui::Event::Paste("clipboard".into())],
                true,
                true,
            ),
            (
                vec![egui::Event::Ime(egui::ImeEvent::Preedit {
                    text: "日本".into(),
                    active_range_chars: None,
                })],
                true,
                true,
            ),
            (vec![egui::Event::Text("x".repeat(257))], true, true),
            (vec![typed; MAX_INPUT_EVENTS + 1], true, true),
        ] {
            assert_eq!(typing_events(&events, enabled, focused).events, 0);
        }
        assert!(density_count(ParticleDensity::Low) < density_count(ParticleDensity::Medium));
        assert!(density_count(ParticleDensity::Medium) < density_count(ParticleDensity::High));
    }

    fn editor(diff: bool) -> Harness<'static, EditorApp> {
        let directory = tempfile::tempdir().unwrap();
        let context = egui::Context::default();
        let mut app = EditorApp::dormant_for_tests(&context, directory.path().into());
        app.snapshot_scene = None;
        app.settings.auto_pair_delimiters = false;
        app.settings.ascii_punctuation = false;
        app.settings.sticky_context_rows = false;
        app.settings.auto_save = false;
        app.settings.editor_particles.enabled = true;
        app.document_mut().replace_unprojected_untitled("prefix ");
        if diff {
            app.document_diff = Some(crate::git::editor::document_diff::DocumentDiff::new(
                "prefix ".to_owned(),
                "prefix ",
            ));
        }
        let mut ready = false;
        let mut harness = Harness::builder()
            .with_size(egui::vec2(850.0, 500.0))
            .with_step_dt(1.0 / 60.0)
            .with_max_steps(80)
            .build_ui_state(
                move |ui, app: &mut EditorApp| {
                    if !ready {
                        theme::configure_test_fonts(ui.ctx());
                        ready = true;
                        return;
                    }
                    app.show_editor(ui);
                },
                app,
            );
        harness.run_steps(3);
        harness.state_mut().pending_editor_selection = Some(EditorSelection::Focus(7..7));
        harness.run();
        harness
    }

    fn counts(harness: &Harness<'_, EditorApp>) -> (usize, u64) {
        harness
            .ctx
            .data(|data| data.get_temp::<SharedState>(state_id(source_editor_id(&harness.ctx))))
            .map_or((0, 0), |state| {
                let state = state.lock().unwrap();
                (state.particles.len(), state.bursts)
            })
    }

    #[test]
    fn real_source_and_editable_diff_typing_emit_at_the_caret_and_disable_clears() {
        for diff in [false, true] {
            let mut harness = editor(diff);
            harness
                .input_mut()
                .events
                .push(egui::Event::Text("α".into()));
            harness.step();
            assert_eq!(harness.state().document().source(), "prefix α");
            assert_eq!(
                counts(&harness),
                (density_count(ParticleDensity::Medium), 1)
            );
            let callbacks = harness
                .output()
                .shapes
                .iter()
                .filter_map(|shape| {
                    matches!(shape.shape, egui::Shape::Callback(_)).then_some(shape.clip_rect)
                })
                .collect::<Vec<_>>();
            assert_eq!(callbacks.len(), 1, "one GPU batch for the whole explosion");
            assert!(callbacks.iter().all(|clip| clip.is_positive()));
            if diff {
                assert!(
                    callbacks.iter().all(|clip| clip.left() > 400.0),
                    "diff particles must be clipped to the editable right column"
                );
            }
            let state = harness
                .ctx
                .data(|data| data.get_temp::<SharedState>(state_id(source_editor_id(&harness.ctx))))
                .unwrap();
            let expected = harness.state().last_editor_caret.unwrap().rect.center();
            assert!(
                state
                    .lock()
                    .unwrap()
                    .particles
                    .iter()
                    .all(|p| p.origin.distance(expected) <= 1.0)
            );
            harness.state_mut().settings.editor_particles.enabled = false;
            harness.step();
            assert_eq!(counts(&harness), (0, 0));
        }
    }

    #[test]
    fn ime_commit_emits_once_even_when_it_keeps_preedit_bytes_and_paste_stays_quiet() {
        for diff in [false, true] {
            let mut harness = editor(diff);
            harness
                .input_mut()
                .events
                .push(egui::Event::Ime(egui::ImeEvent::Preedit {
                    text: "日本".into(),
                    active_range_chars: Some(0..2),
                }));
            harness.step();
            assert_eq!(counts(&harness), (0, 0));
            harness
                .input_mut()
                .events
                .push(egui::Event::Ime(egui::ImeEvent::Commit("日本".into())));
            harness.step();
            assert_eq!(harness.state().document().source(), "prefix 日本");
            assert_eq!(
                counts(&harness),
                (density_count(ParticleDensity::Medium), 1)
            );
            harness
                .input_mut()
                .events
                .push(egui::Event::Paste(" paste".into()));
            harness.step();
            assert_eq!(harness.state().document().source(), "prefix 日本 paste");
            assert_eq!(counts(&harness).1, 1);
            harness.run_steps(45);
            assert_eq!(counts(&harness), (0, 0));
        }
    }

    #[test]
    #[ignore = "opt-in optimized bounded particle-state timing; excludes editor layout and GPU"]
    fn particle_state_measurement() {
        use std::{hint::black_box, time::Instant};
        let events = [egui::Event::Text("x".into())];
        let iterations = 100_000;
        let start = Instant::now();
        for _ in 0..iterations {
            black_box(typing_events(black_box(&events), black_box(false), true));
        }
        let disabled = start.elapsed();
        let mut state = State::default();
        let start = Instant::now();
        for frame in 0..iterations {
            let now = frame as f64 / 120.0;
            state.update(
                identity(1, frame, View::Source),
                frame,
                now,
                Some(egui::pos2(400.0, 300.0)),
                if frame % 3 == 0 { 7 } else { 0 },
            );
            black_box(
                state
                    .particles
                    .iter()
                    .fold(0.0, |sum, p| sum + p.position(now).x),
            );
            assert!(state.particles.len() <= LIMIT);
        }
        eprintln!(
            "particle state: iterations={iterations}, disabled_ns_per_frame={:.1}, active_ns_per_frame={:.1}; excludes egui layout/paint/GPU",
            disabled.as_nanos() as f64 / iterations as f64,
            start.elapsed().as_nanos() as f64 / iterations as f64
        );
    }
}
