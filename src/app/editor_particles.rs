//! Finite editor-action feedback over the existing editor, with no input or layout widgets.
use crate::{
    document::DocumentKey,
    settings::{ActionEffectSettings, EditorEffectSettings, ParticleDensity, ParticleStyle},
    theme,
};
use eframe::egui::{self, Id, Pos2, Rect, Ui, Vec2, text_edit::TextEditOutput};
use std::sync::{Arc, Mutex};

#[allow(unsafe_code)]
mod gpu;
pub(crate) use gpu::Renderer;
mod feedback;
#[cfg(test)]
mod gpu_tests;
pub(super) mod shake;

const LIMIT: usize = 256;
const MAX_INPUT_EVENTS: usize = 64;
const LIFETIME: f64 = 0.64;

pub(super) use crate::settings::EditorAction as Action;
impl Action {
    fn count(self, density: ParticleDensity, events: u8) -> usize {
        let base = density_count(density);
        match self {
            Self::Typing => usize::from(events) * base,
            Self::Paste | Self::Cut => base * 2,
            Self::Copy | Self::Newline | Self::Indent | Self::Delete => base / 2,
            Self::Comment | Self::Undo | Self::Redo => base,
        }
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub(super) struct InputIntent {
    events: u8,
    ime_commit: bool,
    action: Option<Action>,
    copy_output_start: usize,
    commands_enabled: bool,
}

fn input_intent(events: &[egui::Event], enabled: bool, focused: bool) -> InputIntent {
    let mut intent = InputIntent {
        commands_enabled: enabled,
        ..Default::default()
    };
    if !enabled || !focused || events.len() > MAX_INPUT_EVENTS {
        return intent;
    }
    for event in events {
        let action = match event {
            egui::Event::Paste(text) if !text.is_empty() => Some(Action::Paste),
            egui::Event::Cut => Some(Action::Cut),
            egui::Event::Copy => Some(Action::Copy),
            egui::Event::Key {
                key,
                pressed: true,
                modifiers,
                ..
            } => match key {
                egui::Key::Z if modifiers.command => Some(if modifiers.shift {
                    Action::Redo
                } else {
                    Action::Undo
                }),
                egui::Key::Y if modifiers.command => Some(Action::Redo),
                egui::Key::Backspace | egui::Key::Delete => Some(Action::Delete),
                egui::Key::Enter if !modifiers.command => Some(Action::Newline),
                egui::Key::Tab if !modifiers.command => Some(Action::Indent),
                _ => None,
            },
            _ => None,
        };
        if let Some(action) = action
            && intent
                .action
                .is_none_or(|previous| action as u8 > previous as u8)
        {
            intent.action = Some(action);
        }
        let (text, commit) = match event {
            egui::Event::Text(text) => (text, false),
            egui::Event::Ime(egui::ImeEvent::Commit(text)) => (text, true),
            _ => continue,
        };
        // Never scan clipboard contents or document-sized synthetic events.
        if text.len() <= 256 && text.chars().take(16).any(|ch| !ch.is_control()) {
            intent.events = intent.events.saturating_add(1).min(4);
            intent.ime_commit |= commit;
        }
    }
    intent
}

/// Capture before TextEdit consumes events; confirm the actual result afterward.
pub(super) fn capture(ui: &Ui, editor: Id, enabled: bool) -> InputIntent {
    if !enabled {
        return InputIntent::default();
    }
    let focused = ui.memory(|memory| memory.has_focus(editor));
    let mut intent = ui.input(|input| input_intent(&input.events, enabled, focused));
    if intent.action == Some(Action::Copy) {
        intent.copy_output_start = ui.output(|output| output.commands.len());
    }
    intent
}

#[derive(Clone, Copy)]
struct Receipt {
    identity: Identity,
    frame: u64,
    action: Action,
    focus: Option<Id>,
}

fn receipt_id(editor: Id) -> Id {
    editor.with("particle-action-receipt")
}

/// A successful app command can run before or after the editor is painted.
/// Keep only one receipt, scoped to this document/view and the next root frame.
pub(super) fn queue(
    context: &egui::Context,
    editor: Id,
    document: DocumentKey,
    view: View,
    action: Action,
) {
    let receipt = Receipt {
        identity: Identity::new(document, view),
        frame: context.cumulative_frame_nr(),
        action,
        focus: context.memory(|memory| memory.focused()),
    };
    context.data_mut(|data| data.insert_temp(receipt_id(editor), receipt));
    context.request_repaint();
}

#[cfg(test)]
pub(super) fn queued_action(context: &egui::Context, editor: Id) -> Option<Action> {
    context.data(|data| {
        data.get_temp::<Receipt>(receipt_id(editor))
            .map(|receipt| receipt.action)
    })
}

fn receipt_frame_valid(created: u64, current: u64) -> bool {
    (created..=created.saturating_add(1)).contains(&current)
}

fn take_receipt(
    context: &egui::Context,
    editor: Id,
    identity: Identity,
    enabled: bool,
) -> Option<Action> {
    let receipt = context.data_mut(|data| {
        let receipt = data.get_temp::<Receipt>(receipt_id(editor));
        data.remove::<Receipt>(receipt_id(editor));
        receipt
    })?;
    let frame = context.cumulative_frame_nr();
    let viewport = super::focused_input_viewport(context);
    let focus = context.memory(|memory| memory.focused());
    (enabled
        && (focus.is_none() || focus == Some(editor) || focus == receipt.focus)
        && receipt.identity == identity
        && receipt_frame_valid(receipt.frame, frame)
        && ((viewport == context.viewport_id() && context.input(|input| input.focused))
            || viewport == super::scoped_child_viewport_id(context, "tiptoptyp-popup-overlay")))
    .then_some(receipt.action)
}

fn accepted_input(
    intent: InputIntent,
    focused: bool,
    changed: bool,
    response_changed: bool,
    copied: bool,
) -> Option<Action> {
    if !focused {
        return None;
    }
    if let Some(action) = intent.action
        && (if action == Action::Copy {
            copied
        } else {
            changed
        })
    {
        return Some(action);
    }
    (intent.events > 0 && (changed || (intent.ime_commit && response_changed)))
        .then_some(Action::Typing)
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
    pub(super) paint_range: shake::PaintRange,
    pub(super) shake_viewport: Rect,
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
    style: ParticleStyle,
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
    style: ParticleStyle,
    action: Action,
    particles: Vec<Particle>,
    last_frame: Option<u64>,
    last_time: f64,
    emitted_frame: Option<u64>,
    bursts: u64,
    seed: u32,
    base_time: f64,
    gpu_instances: Option<Arc<[gpu::Instance]>>,
    feedback: Option<feedback::Pulse>,
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
            self.feedback = None;
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
            self.feedback = None;
            self.gpu_instances = None;
            self.emitted_frame = None;
        }
        if self.feedback.is_some_and(|pulse| !pulse.alive(now)) {
            self.feedback = None;
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
            let angle = self.random() * std::f32::consts::TAU;
            let energy = self.random();
            let size = self.random();
            let color_seed = self.random();
            let (velocity, radius) = launch(self.style, index, angle, energy, size, color_seed);
            let velocity = action_velocity(self.action, velocity);
            self.particles.push(Particle {
                origin,
                velocity,
                radius,
                color_seed,
                flash: index == 0,
                style: self.style,
                born: now,
                frame,
            });
        }
        self.gpu_instances = None;
        self.emitted_frame = Some(frame);
        self.bursts = self.bursts.wrapping_add(1);
    }

    fn update_feedback(
        &mut self,
        caret: Option<Pos2>,
        effects: Option<ActionEffectSettings>,
        now: f64,
        frame: u64,
    ) -> bool {
        let Some(origin) = caret else {
            return false;
        };
        // Discarded passes may consume the action receipt on the first pass.
        // Realign the existing envelope even if preferences changed meanwhile.
        if let Some(pulse) = &mut self.feedback
            && pulse.frame == frame
        {
            pulse.origin = origin;
            return false;
        }
        if let Some(effects) = effects
            && (effects.shake || effects.flash || effects.glow)
        {
            self.feedback = Some(feedback::Pulse::new(origin, effects, now, frame));
            return true;
        }
        false
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
                            packed_kind(particle),
                        ]
                    })
                    .collect::<Vec<_>>()
                    .into()
            })
            .clone()
    }
}

// Keep the original eight-float instance stride: kind occupies the low two
// bits, and the burst's immutable style occupies the remaining small integer.
fn packed_kind(particle: &Particle) -> f32 {
    let kind = if particle.flash {
        1
    } else if particle.color_seed > 0.9 {
        if particle.style == ParticleStyle::Railgun {
            2
        } else {
            3
        }
    } else if particle.color_seed > 0.68 && particle.style != ParticleStyle::Flamethrower {
        2
    } else {
        0
    };
    let style = match particle.style {
        ParticleStyle::Explosion => 0,
        ParticleStyle::Shotgun => 1,
        ParticleStyle::Flamethrower => 2,
        ParticleStyle::Railgun => 3,
        ParticleStyle::Plasma => 4,
    };
    (style * 4 + kind) as f32
}

fn launch(
    style: ParticleStyle,
    index: usize,
    angle: f32,
    energy: f32,
    size: f32,
    seed: f32,
) -> (Vec2, f32) {
    if index == 0 {
        return (Vec2::ZERO, 18.0);
    }
    let spread = angle / std::f32::consts::TAU - 0.5;
    match style {
        ParticleStyle::Explosion => (
            Vec2::angled(angle) * (65.0 + energy.powi(2) * 255.0),
            0.7 + size,
        ),
        ParticleStyle::Shotgun if seed > 0.68 && seed <= 0.9 => (
            egui::vec2(-35.0 - energy * 65.0, -55.0 - size * 90.0),
            1.0 + size,
        ),
        ParticleStyle::Shotgun => (
            Vec2::angled(-0.2 + spread * 0.8) * (180.0 + energy.powi(2) * 270.0),
            0.7 + size * 0.7,
        ),
        ParticleStyle::Flamethrower => (
            Vec2::angled(-std::f32::consts::FRAC_PI_2 + spread * 1.1) * (65.0 + energy * 145.0),
            1.5 + size * 2.4,
        ),
        ParticleStyle::Railgun => (
            Vec2::angled(spread * 0.06) * (400.0 + energy * 420.0),
            0.7 + size * 0.4,
        ),
        ParticleStyle::Plasma => (
            Vec2::angled(angle) * (90.0 + energy * 135.0),
            1.7 + size * 1.7,
        ),
    }
}

type SharedState = Arc<Mutex<State>>;
fn state_id(editor: Id) -> Id {
    editor.with("typing-particles")
}
// The GPU still owns motion, trails and fading. Actions only steer the
// existing weapon-style launch once; they add no per-frame CPU simulation.
fn action_velocity(action: Action, velocity: Vec2) -> Vec2 {
    match action {
        Action::Typing => velocity,
        Action::Copy => egui::vec2(velocity.x * 0.65, -velocity.y.abs() - 65.0),
        Action::Paste => egui::vec2(velocity.x * 1.35, -velocity.y.abs() * 0.75 - 35.0),
        Action::Cut => egui::vec2(velocity.x * 1.2, velocity.y.abs() + 70.0),
        Action::Comment | Action::Redo | Action::Indent => {
            egui::vec2(velocity.x.abs() + 75.0, velocity.y * 0.35)
        }
        Action::Undo => egui::vec2(-velocity.x.abs() - 75.0, velocity.y * 0.35),
        Action::Delete => egui::vec2(velocity.x * 0.6, velocity.y.abs() + 45.0),
        Action::Newline => egui::vec2(velocity.x * 0.8, -velocity.y.abs() - 85.0),
    }
}

fn density_count(density: ParticleDensity) -> usize {
    match density {
        ParticleDensity::Low => 24,
        ParticleDensity::Medium => 48,
        ParticleDensity::High => 80,
    }
}

fn visible_origin(point: Pos2, viewport: Rect, action: Action) -> Option<Pos2> {
    if !point.is_finite() || !viewport.is_finite() || !viewport.is_positive() {
        return None;
    }
    if viewport.contains(point) {
        return Some(point);
    }
    if action == Action::Typing {
        return None;
    }
    let inset = viewport.size() * 0.5;
    Some(viewport.shrink2(inset.min(Vec2::splat(4.0))).clamp(point))
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
    settings: EditorEffectSettings,
    intent: InputIntent,
    document_changed: bool,
) {
    let context = ui.ctx();
    let id = state_id(surface.edit.response.id);
    if !settings.enabled {
        context.data_mut(|data| {
            data.remove::<SharedState>(id);
            data.remove::<Receipt>(receipt_id(surface.edit.response.id));
        });
        return;
    }
    let renderer = Renderer::for_context(context);
    let identity = Identity::new(surface.document, surface.view);
    let receipt = take_receipt(
        context,
        surface.edit.response.id,
        identity,
        intent.commands_enabled,
    );
    let existing = context.data(|data| data.get_temp::<SharedState>(id));
    let copied = intent.action == Some(Action::Copy)
        && ui.output(|output| {
            output.commands.iter().skip(intent.copy_output_start).any(|command| {
            matches!(command, egui::OutputCommand::CopyText(text) if !text.is_empty())
        })
        });
    let action = receipt.or_else(|| {
        accepted_input(
            intent,
            context.input(|input| input.focused)
                && super::focused_input_viewport(context) == context.viewport_id(),
            document_changed,
            surface.edit.response.changed(),
            copied,
        )
    });
    let action = action.filter(|&action| {
        let effects = settings.actions.get(action);
        effects.enabled()
            && (!renderer.is_unavailable() || effects.shake || effects.flash || effects.glow)
    });
    let accepted = action.is_some();
    if existing.is_none() && !accepted {
        if Action::ALL
            .into_iter()
            .any(|action| settings.actions.get(action).particles.is_some())
        {
            renderer.warm(ui, surface.viewport.intersect(ui.clip_rect()));
        }
        return;
    }
    let state = existing.unwrap_or_else(|| {
        let state = SharedState::default();
        context.data_mut(|data| data.insert_temp(id, state.clone()));
        state
    });
    let mut state = state.lock().unwrap();
    if renderer.is_unavailable() {
        // A shader can fail on its first actual paint. Retire invisible particles
        // immediately while keeping independent CPU feedback alive.
        state.particles.clear();
        state.gpu_instances = None;
    }
    let frame = context.cumulative_frame_nr();
    let viewport = surface.viewport.intersect(ui.clip_rect());
    let action = action.unwrap_or(state.action);
    let caret = caret_for_frame(
        settings.enabled,
        accepted,
        (state.emitted_frame == Some(frame) && !state.particles.is_empty())
            || state.feedback.is_some_and(|pulse| pulse.frame == frame),
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
                .and_then(|point| visible_origin(point, viewport, action))
        },
    );
    let now = context.input(|input| input.time);
    #[cfg(any(test, all(feature = "desktop-ui-tests", target_os = "macos")))]
    let bursts_before = state.bursts;
    let effects = *settings.actions.get(action);
    if let Some(style) = effects.particles {
        state.style = style;
    }
    state.action = action;
    state.update(
        Identity::new(surface.document, surface.view),
        frame,
        now,
        caret,
        if accepted && effects.particles.is_some() && !renderer.is_unavailable() {
            action.count(settings.density, intent.events.max(1))
        } else {
            0
        },
    );
    let feedback_emitted = state.update_feedback(caret, accepted.then_some(effects), now, frame);
    #[cfg(any(test, all(feature = "desktop-ui-tests", target_os = "macos")))]
    if feedback_emitted {
        feedback::record(context, id, identity, action, effects, now);
    }
    let _ = feedback_emitted;
    #[cfg(any(test, all(feature = "desktop-ui-tests", target_os = "macos")))]
    if state.bursts != bursts_before {
        record_emission(context, id, &state, now, viewport);
    }
    let pulse = state.feedback;
    if state.particles.is_empty() && pulse.is_none() {
        drop(state);
        context.data_mut(|data| data.remove::<SharedState>(id));
        return;
    }
    let instances =
        (!state.particles.is_empty() && !renderer.is_unavailable()).then(|| state.instances());
    let time = (now - state.base_time) as f32;
    drop(state);
    let accent = theme::palette(context).accent;
    if let Some(pulse) = pulse {
        let frame = pulse.sample(now);
        surface.paint_range.apply(
            ui,
            frame.shake,
            surface.shake_viewport.intersect(ui.clip_rect()),
        );
        feedback::paint(ui, viewport, pulse.origin, accent, frame);
    }
    if let Some(instances) = instances {
        renderer.submit(
            ui,
            viewport,
            instances,
            time,
            accent,
            ui.visuals().dark_mode,
        );
    }
    context.request_repaint_after(theme::METRICS.motion.animation_frame);
}

#[cfg(any(test, all(feature = "desktop-ui-tests", target_os = "macos")))]
#[derive(Clone)]
struct Trace {
    identity: Identity,
    burst_count: u64,
    last_style: ParticleStyle,
    last_action: Action,
    action_bursts: [u64; 10],
    emitted_count: u64,
    max_live: usize,
    last_emission: f64,
    origin: Pos2,
    clip: Rect,
}

#[cfg(any(test, all(feature = "desktop-ui-tests", target_os = "macos")))]
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
                last_style: particle.style,
                last_action: state.action,
                action_bursts: [0; 10],
                emitted_count: 0,
                max_live: 0,
                last_emission: now,
                origin: particle.origin,
                clip,
            });
        trace.identity = identity;
        trace.last_style = particle.style;
        trace.last_action = state.action;
        trace.action_bursts[state.action as usize] =
            trace.action_bursts[state.action as usize].saturating_add(1);
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
#[cfg(any(test, all(feature = "desktop-ui-tests", target_os = "macos")))]
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
    let mut live_styles = std::collections::BTreeMap::new();
    let active_count = state.map_or(0, |state| {
        let state = state.lock().unwrap();
        if state.identity.is_none_or(|key| key.document != identity) {
            return 0;
        }
        for particle in state
            .particles
            .iter()
            .filter(|particle| (0.0..LIFETIME).contains(&(now - particle.born)))
        {
            *live_styles.entry(particle.style.label()).or_insert(0usize) += 1;
        }
        live_styles.values().sum::<usize>()
    });
    serde_json::json!({
        "feedback": feedback::inspected(context, id, identity, now),
        "gpu": Renderer::for_context(context).inspected(),
        "active_count": active_count,
        "live_styles": live_styles,
        "last_style": trace.as_ref().map(|trace| trace.last_style.label()),
        "last_action": trace.as_ref().map(|trace| trace.last_action.label()),
        "action_bursts": Action::ALL.into_iter().filter_map(|action| {
            let count = trace.as_ref().map_or(0, |trace| trace.action_bursts[action as usize]);
            (count > 0).then_some((action.label(), count))
        }).collect::<std::collections::BTreeMap<_, _>>(),
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
pub(super) mod tests {
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
    fn discarded_feedback_pass_realigns_without_a_second_action_or_changing_the_envelope() {
        let mut state = State::default();
        let effects = ActionEffectSettings {
            particles: None,
            glow: true,
            flash: false,
            shake: false,
        };
        assert!(state.update_feedback(Some(egui::pos2(10.0, 20.0)), Some(effects), 5.0, 100));
        assert!(!state.update_feedback(Some(egui::pos2(30.0, 40.0)), None, 5.1, 100));
        let pulse = state.feedback.unwrap();
        assert_eq!(pulse.origin, egui::pos2(30.0, 40.0));
        assert!(pulse.sample(5.1).glow > 0.0);
        assert!(
            !pulse.alive(5.5),
            "realignment must not restart the envelope"
        );
        assert!(!state.update_feedback(None, Some(effects), 5.5, 101));
        assert!(state.update_feedback(Some(egui::pos2(50.0, 60.0)), Some(effects), 5.6, 101));
    }

    #[test]
    fn action_origins_stay_inside_even_tiny_clips_without_forcing_hidden_typing() {
        for width in [1.0, 2.0, 7.0, 8.0, 100.0] {
            for height in [1.0, 2.0, 7.0, 8.0, 100.0] {
                let viewport = Rect::from_min_size(Pos2::new(20.0, 30.0), Vec2::new(width, height));
                for point in [Pos2::ZERO, Pos2::new(1000.0, 1000.0)] {
                    assert_eq!(visible_origin(point, viewport, Action::Typing), None);
                    for action in [Action::Copy, Action::Paste, Action::Undo, Action::Comment] {
                        assert!(
                            viewport.contains(visible_origin(point, viewport, action).unwrap())
                        );
                    }
                }
            }
        }
        assert_eq!(
            visible_origin(Pos2::ZERO, Rect::NOTHING, Action::Copy),
            None
        );
    }

    #[test]
    fn action_receipts_are_one_shot_owner_bound_and_do_not_survive_focus_changes() {
        let context = egui::Context::default();
        context.begin_pass(egui::RawInput::default());
        context.end_pass().drop_without_applying_deltas();
        let editor = Id::new("receipt-editor");
        let key = identity(1, 1, View::Source);
        queue(&context, editor, key.document, View::Source, Action::Copy);
        assert_eq!(
            take_receipt(&context, editor, key, true),
            Some(Action::Copy)
        );
        assert_eq!(take_receipt(&context, editor, key, true), None);
        for other in [identity(2, 1, View::Source), identity(1, 1, View::Diff)] {
            queue(&context, editor, key.document, View::Source, Action::Copy);
            assert_eq!(take_receipt(&context, editor, other, true), None);
            assert_eq!(take_receipt(&context, editor, key, true), None);
        }
        queue(&context, editor, key.document, View::Source, Action::Copy);
        assert_eq!(take_receipt(&context, editor, key, false), None);
        context.memory_mut(|memory| memory.request_focus(editor));
        queue(&context, editor, key.document, View::Source, Action::Copy);
        context.memory_mut(|memory| memory.request_focus(Id::new("another-field")));
        assert_eq!(take_receipt(&context, editor, key, true), None);
        queue(&context, editor, key.document, View::Source, Action::Copy);
        let info = egui::ViewportInfo {
            focused: Some(true),
            ..Default::default()
        };
        let settings = super::super::scoped_child_viewport_id(&context, "tiptoptyp-settings");
        let mut input = egui::RawInput::default();
        input.viewports.insert(settings, info);
        context.begin_pass(input);
        assert_eq!(take_receipt(&context, editor, key, true), None);
        context.end_pass().drop_without_applying_deltas();
        queue(&context, editor, key.document, View::Source, Action::Copy);
        context.begin_pass(egui::RawInput {
            focused: false,
            ..Default::default()
        });
        assert_eq!(take_receipt(&context, editor, key, true), None);
        context.end_pass().drop_without_applying_deltas();
        assert!(receipt_frame_valid(10, 10));
        assert!(receipt_frame_valid(10, 11));
        assert!(!receipt_frame_valid(10, 12));
        assert!(!receipt_frame_valid(10, 9));
    }

    #[test]
    fn command_motion_has_direction_and_bulk_input_never_scales_its_emission() {
        let velocity = egui::vec2(-100.0, 90.0);
        assert_eq!(action_velocity(Action::Typing, velocity), velocity);
        assert!(action_velocity(Action::Copy, velocity).y < 0.0);
        assert!(action_velocity(Action::Cut, velocity).y > 0.0);
        assert!(action_velocity(Action::Paste, velocity).y < 0.0);
        assert!(action_velocity(Action::Undo, velocity).x < 0.0);
        assert!(action_velocity(Action::Redo, velocity).x > 0.0);
        let intent = input_intent(&[egui::Event::Paste("x".repeat(1_000_000))], true, true);
        assert_eq!(intent.events, 0);
        assert_eq!(intent.action, Some(Action::Paste));
        let mut state = State::default();
        for frame in 0..100 {
            state.action = [
                Action::Paste,
                Action::Copy,
                Action::Cut,
                Action::Undo,
                Action::Redo,
                Action::Comment,
            ][frame as usize % 6];
            state.update(
                identity(1, frame, View::Source),
                frame,
                frame as f64 * 0.001,
                Some(Pos2::new(50.0, 50.0)),
                state.action.count(ParticleDensity::High, 1),
            );
            assert!(state.particles.len() <= LIMIT);
        }
        let instances = state.instances();
        let bursts = state.bursts;
        state.update(
            identity(1, 99, View::Source),
            99,
            0.099,
            Some(Pos2::ZERO),
            160,
        );
        assert_eq!(state.bursts, bursts);
        state.update(identity(1, 100, View::Source), 100, 0.1, None, 0);
        let idle_instances = state.instances();
        state.update(identity(1, 101, View::Source), 101, 0.101, None, 0);
        assert!(Arc::ptr_eq(&idle_instances, &state.instances()));
        assert_eq!(instances.len(), LIMIT);
        state.update(identity(1, 102, View::Source), 102, 1.0, None, 0);
        assert!(state.particles.is_empty());
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
    fn styles_keep_fixed_gpu_stride_and_distinct_launch_patterns() {
        assert_eq!(std::mem::size_of::<gpu::Instance>(), 32);
        let mut batches = Vec::new();
        for (code, style) in ParticleStyle::ALL.into_iter().enumerate() {
            let mut state = State {
                style,
                ..Default::default()
            };
            state.update(identity(1, 0, View::Source), 0, 0.0, Some(Pos2::ZERO), 80);
            let batch = state.instances();
            assert_eq!(batch.len(), 80);
            assert!(batch.iter().flatten().all(|value| value.is_finite()));
            assert!(batch.iter().all(|p| (p[7] / 4.0).floor() as usize == code));
            let moving = state.particles.iter().filter(|p| !p.flash);
            match style {
                ParticleStyle::Shotgun => {
                    assert!(moving.filter(|p| p.velocity.x > 0.0).count() > 50)
                }
                ParticleStyle::Flamethrower => assert!(moving.clone().all(|p| p.velocity.y < 0.0)),
                ParticleStyle::Railgun => assert!(
                    moving
                        .clone()
                        .all(|p| p.velocity.x > 399.0 && p.velocity.y.abs() < p.velocity.x * 0.031)
                ),
                ParticleStyle::Explosion | ParticleStyle::Plasma => {
                    let mut quadrants = [false; 4];
                    for p in moving {
                        quadrants[usize::from(p.velocity.x >= 0.0)
                            + 2 * usize::from(p.velocity.y >= 0.0)] = true;
                    }
                    assert!(quadrants.into_iter().all(|present| present));
                }
            }
            batches.push(batch);
            for frame in 1..100 {
                state.update(
                    identity(1, frame, View::Source),
                    frame,
                    frame as f64 * 0.001,
                    Some(Pos2::ZERO),
                    320,
                );
                assert_eq!(state.particles.len(), LIMIT, "{style:?}");
            }
            state.update(identity(1, 100, View::Source), 100, 1.0, None, 0);
            assert!(
                state.particles.is_empty(),
                "every style must stop repainting after expiry"
            );
        }
        for pair in batches.windows(2) {
            assert_ne!(pair[0], pair[1]);
        }
    }

    #[test]
    fn switching_style_changes_only_future_bursts_and_reuses_unchanged_gpu_batch() {
        let mut state = State::default();
        let key = identity(1, 0, View::Source);
        state.update(key, 0, 0.0, Some(Pos2::ZERO), 48);
        let explosion = state.instances();
        state.style = ParticleStyle::Plasma;
        state.update(key, 1, 0.01, None, 0);
        assert_eq!(state.bursts, 1, "selection must not emit");
        assert!(
            Arc::ptr_eq(&explosion, &state.instances()),
            "selection must not upload or restyle an existing batch"
        );
        state.update(key, 2, 0.02, Some(Pos2::new(10.0, 0.0)), 48);
        let mixed = state.instances();
        assert_eq!(
            &mixed[..48],
            &*explosion,
            "old trajectories and shapes must remain immutable"
        );
        assert!(mixed[48..].iter().all(|p| (p[7] / 4.0).floor() == 4.0));
        state.update(key, 3, 0.65, None, 0);
        let tail = state.instances();
        assert_eq!(
            &*tail,
            &mixed[48..],
            "the older style expires independently"
        );
        state.update(key, 4, 0.7, None, 0);
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
    fn typing_and_actions_are_bounded_and_require_confirmed_results() {
        let typed = egui::Event::Text("α".into());
        assert_eq!(
            input_intent(std::slice::from_ref(&typed), true, true).events,
            1
        );
        assert!(
            input_intent(
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
                vec![egui::Event::Ime(egui::ImeEvent::Preedit {
                    text: "日本".into(),
                    active_range_chars: None,
                })],
                true,
                true,
            ),
            (vec![egui::Event::Text("x".repeat(257))], true, true),
            (vec![typed.clone(); MAX_INPUT_EVENTS + 1], true, true),
        ] {
            assert_eq!(input_intent(&events, enabled, focused).events, 0);
        }
        let paste = input_intent(
            &[typed.clone(), egui::Event::Paste("clipboard".into())],
            true,
            true,
        );
        assert_eq!(paste.action, Some(Action::Paste));
        assert_eq!(
            accepted_input(paste, true, true, true, false),
            Some(Action::Paste)
        );
        assert_eq!(accepted_input(paste, true, false, false, false), None);
        let copy = input_intent(&[egui::Event::Copy], true, true);
        assert_eq!(
            accepted_input(copy, true, false, false, true),
            Some(Action::Copy)
        );
        assert_eq!(accepted_input(copy, true, false, false, false), None);
        assert_eq!(accepted_input(copy, false, false, false, true), None);
        assert!(density_count(ParticleDensity::Low) < density_count(ParticleDensity::Medium));
        assert!(density_count(ParticleDensity::Medium) < density_count(ParticleDensity::High));
    }

    pub(in crate::app) fn editor(diff: bool) -> Harness<'static, EditorApp> {
        let directory = tempfile::tempdir().unwrap();
        let context = egui::Context::default();
        let mut app = EditorApp::dormant_for_tests(&context, directory.path().into());
        app.snapshot_scene = None;
        app.settings.auto_pair_delimiters = false;
        app.settings.ascii_punctuation = false;
        app.settings.sticky_context_rows = false;
        app.settings.auto_save = false;
        app.settings.editor_effects.enabled = true;
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
            harness.state_mut().settings.editor_effects.enabled = false;
            harness.step();
            assert_eq!(counts(&harness), (0, 0));
        }
    }

    fn current_action(harness: &Harness<'_, EditorApp>) -> Option<Action> {
        harness
            .ctx
            .data(|data| data.get_temp::<SharedState>(state_id(source_editor_id(&harness.ctx))))
            .map(|state| state.lock().unwrap().action)
    }

    #[test]
    fn real_source_and_diff_widget_actions_confirm_copy_and_changed_text_once() {
        let key = |key| egui::Event::Key {
            key,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        };
        for diff in [false, true] {
            for (action, event, selection, expected) in [
                (Action::Copy, egui::Event::Copy, 0..6, "prefix "),
                (Action::Cut, egui::Event::Cut, 0..6, " "),
                (
                    Action::Paste,
                    egui::Event::Paste("pasted".into()),
                    7..7,
                    "prefix pasted",
                ),
                (Action::Delete, key(egui::Key::Backspace), 7..7, "prefix"),
                (Action::Delete, key(egui::Key::Delete), 0..0, "refix "),
                (Action::Newline, key(egui::Key::Enter), 7..7, "prefix \n"),
                (Action::Indent, key(egui::Key::Tab), 7..7, "prefix   "),
            ] {
                let mut harness = editor(diff);
                harness.state_mut().pending_editor_selection =
                    Some(EditorSelection::Focus(selection));
                harness.step();
                harness.input_mut().events.push(event);
                harness.step();
                assert_eq!(
                    harness.state().document().source(),
                    expected,
                    "{diff}: {action:?}"
                );
                assert_eq!(counts(&harness).1, 1, "{diff}: {action:?}");
                assert_eq!(current_action(&harness), Some(action));
                if action == Action::Copy {
                    assert!(harness.output().platform_output.commands.iter().any(|command|
                        matches!(command, egui::OutputCommand::CopyText(text) if text == "prefix")));
                }
                harness.run_steps(45);
                assert_eq!(counts(&harness), (0, 0), "{action:?} must expire");
                assert!(
                    harness
                        .ctx
                        .repaint_causes()
                        .iter()
                        .all(|cause| cause.file != file!())
                );
            }
            for event in [
                egui::Event::Copy,
                egui::Event::Cut,
                egui::Event::Paste(String::new()),
                key(egui::Key::Delete),
            ] {
                let mut harness = editor(diff);
                harness.input_mut().events.push(event);
                harness.step();
                assert_eq!(harness.state().document().source(), "prefix ");
                assert_eq!(counts(&harness), (0, 0), "no-op actions stay quiet");
            }
        }
    }

    #[test]
    fn no_op_copy_does_not_suppress_successful_typing_in_the_same_frame() {
        for diff in [false, true] {
            let mut harness = editor(diff);
            harness
                .input_mut()
                .events
                .extend([egui::Event::Copy, egui::Event::Text("x".into())]);
            harness.step();
            assert_eq!(harness.state().document().source(), "prefix x");
            assert_eq!(counts(&harness).1, 1);
            assert_eq!(current_action(&harness), Some(Action::Typing));
        }
    }

    #[test]
    fn successful_menu_commands_emit_in_both_views_without_a_text_change_intent() {
        for diff in [false, true] {
            let mut harness = editor(diff);
            let context = harness.ctx.clone();
            harness.state_mut().pending_editor_selection = Some(EditorSelection::Focus(0..6));
            harness.step();
            harness.state_mut().copy_editor_selection(&context);
            // A duplicate native semantic event and command receipt still admit one burst.
            harness.input_mut().events.push(egui::Event::Copy);
            harness.step();
            assert_eq!(counts(&harness).1, 1);
            assert_eq!(current_action(&harness), Some(Action::Copy));
            assert_eq!(harness.state().document().source(), "prefix ");
            harness.state_mut().toggle_comments(&context);
            harness.step();
            assert_eq!(current_action(&harness), Some(Action::Comment));
            assert!(harness.state().document().source().starts_with("// "));
            harness.state_mut().undo_editor(&context, false);
            harness.step();
            assert_eq!(current_action(&harness), Some(Action::Undo));
            assert_eq!(harness.state().document().source(), "prefix ");
            harness.state_mut().undo_editor(&context, true);
            harness.step();
            assert_eq!(current_action(&harness), Some(Action::Redo));
            assert_eq!(counts(&harness).1, 4);
        }
    }

    #[test]
    fn unavailable_gpu_keeps_editing_but_never_emits_or_repaints_particle_state() {
        for diff in [false, true] {
            let mut harness = editor(diff);
            let renderer = Renderer::for_context(&harness.ctx);
            renderer.simulate_unavailable_for_tests();
            harness
                .input_mut()
                .events
                .push(egui::Event::Text("x".into()));
            harness.step();
            assert_eq!(harness.state().document().source(), "prefix x");
            assert_eq!(
                counts(&harness),
                (0, 0),
                "an unavailable effect must not animate an invisible pool"
            );
            assert!(
                harness
                    .output()
                    .shapes
                    .iter()
                    .all(|shape| !matches!(shape.shape, egui::Shape::Callback(_)))
            );
            harness.step();
            assert!(
                harness
                    .ctx
                    .repaint_causes()
                    .iter()
                    .all(|cause| cause.file != file!()),
                "unavailable particles must not schedule animation repaints"
            );
        }
    }

    #[test]
    fn ime_commit_and_clipboard_paste_emit_distinct_single_bursts() {
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
            assert_eq!(counts(&harness).1, 2);
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
            black_box(input_intent(black_box(&events), black_box(false), true));
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
