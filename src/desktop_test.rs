//! Opt-in desktop-test observation. No command dispatch, document writes, or
//! production endpoint. Requests wake the real UI; snapshots come from its next pass.
use eframe::egui;
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    io::{BufRead, BufReader, Read, Write},
    os::unix::{fs::PermissionsExt, net::UnixListener},
    path::PathBuf,
    sync::{Condvar, Mutex, OnceLock},
    time::{Duration, Instant},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct RendererRequest {
    generation: u64,
    id: u64,
}

struct RendererSample {
    value: Value,
    observed: Instant,
}

#[derive(Default)]
enum RendererPhase {
    #[default]
    Idle,
    Pending(RendererRequest),
}

struct RendererView {
    viewport: String,
    phase: RendererPhase,
    // An older sample remains useful while the next observation is pending.
    sample: Option<RendererSample>,
}

#[derive(Default)]
struct RendererProbe {
    generation: u64,
    next_request: u64,
    view: Option<RendererView>,
}

const WHEEL_VIEWPORT_LIMIT: usize = 8;

/// Retain only the latest receipt per viewport and two frames of table routing.
/// This is diagnostic metadata, never an event queue or an input consumer.
#[derive(Default)]
struct WheelProbe {
    sequence: u64,
    next_route: u64,
    raw: BTreeMap<String, Value>,
    table_input: Option<Value>,
    table_smooth: Option<Value>,
}

#[derive(Clone, Copy)]
pub(crate) struct TableWheelPass(u64);

pub(crate) struct WheelCellBefore {
    input: Value,
    delta: egui::Vec2,
    hovered: bool,
}

#[derive(Clone, Copy)]
struct WheelContextId(u64);

fn wheel_context_id(context: &egui::Context) -> u64 {
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(1);
    context.data_mut(|data| {
        let key = egui::Id::new("desktop-wheel-context");
        let id = data.get_temp::<WheelContextId>(key).unwrap_or_else(|| {
            let id = WheelContextId(
                NEXT.fetch_update(Ordering::Relaxed, Ordering::Relaxed, |next| {
                    next.checked_add(1)
                })
                .expect("wheel context identity exhausted"),
            );
            data.insert_temp(key, id);
            id
        });
        id.0
    })
}

impl WheelProbe {
    fn record_raw(&mut self, viewport: &str, frame: u64, time: f64, mut receipt: Value) {
        if self.raw.get(viewport).is_some_and(|previous| {
            previous["frame"] == frame
                && previous["time"] == time
                && previous["context"] == receipt["context"]
        }) {
            return;
        }
        self.sequence = self
            .sequence
            .checked_add(1)
            .expect("wheel receipt exhausted");
        receipt["sequence"] = self.sequence.into();
        receipt["viewport"] = viewport.into();
        receipt["frame"] = frame.into();
        receipt["time"] = time.into();
        self.raw.insert(viewport.into(), receipt);
        if self.raw.len() > WHEEL_VIEWPORT_LIMIT {
            let oldest = self
                .raw
                .iter()
                .min_by_key(|(_, receipt)| receipt["sequence"].as_u64())
                .map(|(viewport, _)| viewport.clone())
                .expect("the bounded receipt map is nonempty");
            self.raw.remove(&oldest);
        }
    }

    fn update_table(&mut self, pass: TableWheelPass, field: &str, value: Value) {
        for sample in [&mut self.table_input, &mut self.table_smooth]
            .into_iter()
            .flatten()
        {
            if sample["route"] == pass.0 {
                if field == "first_consumer" && !sample[field].is_null() {
                    continue;
                }
                sample[field] = value.clone();
            }
        }
    }

    fn snapshot(&self) -> Value {
        json!({"raw": self.raw, "table_input": self.table_input, "table_smooth": self.table_smooth})
    }
}

impl RendererProbe {
    fn reset(&mut self) {
        self.generation = self
            .generation
            .checked_add(1)
            .expect("desktop renderer generation exhausted");
        if let Some(view) = &mut self.view {
            view.phase = RendererPhase::Idle;
            view.sample = None;
        }
    }

    fn request(&mut self, viewport: &str) -> Option<RendererRequest> {
        if self
            .view
            .as_ref()
            .is_none_or(|view| view.viewport != viewport)
        {
            self.reset();
            self.view = Some(RendererView {
                viewport: viewport.to_owned(),
                phase: RendererPhase::Idle,
                sample: None,
            });
        }
        let view = self.view.as_mut().expect("renderer view was just bound");
        if matches!(view.phase, RendererPhase::Pending(_)) {
            return None;
        }
        self.next_request = self
            .next_request
            .checked_add(1)
            .expect("desktop renderer observation identity exhausted");
        let request = RendererRequest {
            generation: self.generation,
            id: self.next_request,
        };
        view.phase = RendererPhase::Pending(request);
        Some(request)
    }

    fn observe(&mut self, request: RendererRequest, value: String) {
        let Some(view) = &mut self.view else {
            return;
        };
        if !matches!(view.phase, RendererPhase::Pending(pending) if pending == request) {
            return;
        }
        view.sample = Some(RendererSample {
            value: serde_json::from_str(&value).unwrap_or(Value::Null),
            observed: Instant::now(),
        });
        view.phase = RendererPhase::Idle;
    }

    fn sample(&self) -> Option<(&str, &RendererSample)> {
        let view = self.view.as_ref()?;
        Some((&view.viewport, view.sample.as_ref()?))
    }

    fn phase(&self) -> &'static str {
        match self.view.as_ref().map(|view| &view.phase) {
            None => "unbound",
            Some(RendererPhase::Idle) => "idle",
            Some(RendererPhase::Pending(_)) => "pending",
        }
    }
}

#[derive(Default)]
struct State {
    context: Option<egui::Context>,
    requested: u64,
    published: u64,
    requested_viewport: Option<String>,
    document: Value,
    renderer: RendererProbe,
    targets: BTreeMap<String, (Value, Instant)>,
    observed_targets: BTreeMap<String, (Value, Instant)>,
    scrolls: BTreeMap<String, Value>,
    observed_scrolls: BTreeMap<String, Value>,
    observed_wheel: WheelProbe,
    wheel: Value,
}
impl State {
    fn requested_for(&self, viewport: &str) -> bool {
        self.requested > self.published
            && self
                .requested_viewport
                .as_deref()
                .is_none_or(|owner| owner == viewport)
    }
    fn publish_document(&mut self, document: Value) {
        if self
            .requested_viewport
            .as_deref()
            .is_some_and(|owner| document["viewport"] != owner)
        {
            return;
        }
        self.document = document;
        // Freeze hit targets with their document frame. The next native pass
        // may start before the socket reader wakes; it must not mix frames.
        self.targets.clone_from(&self.observed_targets);
        self.scrolls.clone_from(&self.observed_scrolls);
        self.wheel = self.observed_wheel.snapshot();
        self.published = self.requested;
    }
    fn reset_renderer(&mut self) {
        self.renderer.reset();
    }
    fn request_renderer(&mut self, viewport: &str) -> Option<RendererRequest> {
        if self
            .requested_viewport
            .as_deref()
            .is_some_and(|owner| owner != viewport)
        {
            return None;
        }
        self.renderer.request(viewport)
    }
    fn renderer_observed(&mut self, request: RendererRequest, value: String) {
        self.renderer.observe(request, value);
    }
}
struct Probe {
    state: Mutex<State>,
    changed: Condvar,
}
static PROBE: OnceLock<Probe> = OnceLock::new();

pub(crate) fn start() -> Result<Option<PathBuf>, String> {
    let Some(directory) = std::env::var_os("TIPTOPTYP_DESKTOP_TEST_DIR") else {
        return Ok(None);
    };
    let directory = PathBuf::from(directory);
    let metadata = std::fs::symlink_metadata(&directory).map_err(|e| e.to_string())?;
    if !directory.is_absolute() || !metadata.is_dir() || metadata.permissions().mode() & 0o077 != 0
    {
        return Err(
            "desktop test directory must be an absolute, private directory (mode 0700)".into(),
        );
    }
    let persistence = directory.join("settings.ron");
    if persistence.symlink_metadata().is_ok() {
        return Err("desktop journeys require a fresh settings store".into());
    }
    let socket = directory.join("inspect.sock");
    let listener = UnixListener::bind(&socket).map_err(|e| e.to_string())?;
    std::fs::set_permissions(&socket, std::fs::Permissions::from_mode(0o600))
        .map_err(|e| e.to_string())?;
    PROBE
        .set(Probe {
            state: Mutex::new(State::default()),
            changed: Condvar::new(),
        })
        .map_err(|_| "desktop inspection already started")?;
    std::thread::Builder::new()
        .name("desktop-inspection".into())
        .spawn(move || {
            for mut stream in listener.incoming().flatten() {
                let timeout = Some(Duration::from_secs(3));
                let _ = stream.set_read_timeout(timeout);
                let _ = stream.set_write_timeout(timeout);
                let mut command = String::new();
                let read = BufReader::new((&mut stream).take(32)).read_line(&mut command);
                let response = if read.is_ok()
                    && let Some(viewport) = snapshot_viewport(&command)
                {
                    snapshot(viewport.map(str::to_owned))
                } else {
                    json!({"error": "only snapshot is supported"})
                };
                let _ = serde_json::to_writer(&mut stream, &response);
                let _ = stream.write_all(b"\n");
            }
        })
        .map_err(|e| e.to_string())?;
    Ok(Some(persistence))
}

pub(crate) fn attach(context: &egui::Context) {
    if let Some(probe) = PROBE.get() {
        probe.state.lock().unwrap().context = Some(context.clone());
    }
}

fn snapshot_viewport(command: &str) -> Option<Option<&str>> {
    if command == "snapshot\n" {
        return Some(None);
    }
    let viewport = command.strip_prefix("snapshot ")?.strip_suffix('\n')?;
    // egui's Debug identifier includes quotes; preserve them for exact owner
    // matching while validating only the bounded hexadecimal payload.
    let identifier = viewport
        .strip_prefix('"')
        .and_then(|text| text.strip_suffix('"'))
        .unwrap_or(viewport);
    (!identifier.is_empty()
        && identifier.len() <= 16
        && identifier.bytes().all(|byte| byte.is_ascii_hexdigit()))
    .then_some(Some(viewport))
}

fn snapshot(viewport: Option<String>) -> Value {
    let probe = PROBE.get().unwrap();
    let (context, request) = {
        let mut state = probe.state.lock().unwrap();
        state.requested += 1;
        state.requested_viewport = viewport;
        (state.context.clone(), state.requested)
    };
    let Some(context) = context else {
        return json!({"error": "UI not initialized"});
    };
    // Never enter egui with the probe mutex held: rendering also observes state.
    let viewports = context.input(|input| input.raw.viewports.keys().copied().collect::<Vec<_>>());
    for id in viewports {
        context.request_repaint_of(id);
        // A snapshot needs a new native UI pass even when egui coalesces the
        // request with the pass currently finishing (e.g. a theme change).
        // This is one read-only observation wakeup, never an idle repaint loop.
        if let Some(window) = eframe::window_host::window(&context, id) {
            window.request_redraw();
        }
    }
    let deadline = Instant::now() + Duration::from_secs(2);
    let mut state = probe.state.lock().unwrap();
    while state.published < request {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return json!({"error": "UI did not publish a fresh frame"});
        }
        state = probe.changed.wait_timeout(state, remaining).unwrap().0;
    }
    let targets: BTreeMap<_, _> = state
        .targets
        .iter()
        .map(|(name, (target, observed))| {
            let mut target = target.clone();
            target["age_ms"] = json!(observed.elapsed().as_millis());
            (name.clone(), target)
        })
        .collect();
    json!({
        "schema": 1, "pid": std::process::id(), "build": crate::build_info::version(),
        "request": request, "renderer_generation": state.renderer.generation, "renderer_phase": state.renderer.phase(), "document": state.document, "targets": targets, "scrolls": state.scrolls, "wheel": state.wheel,
        "renderer": state.renderer.sample().filter(|(viewport, _)| state.document["viewport"].as_str() == Some(*viewport))
            .map(|(viewport, sample)| json!({"viewport": viewport, "value": sample.value, "age_ms": sample.observed.elapsed().as_millis()})),
    })
}

pub(crate) fn reset_renderer() {
    if let Some(probe) = PROBE.get() {
        probe.state.lock().unwrap().reset_renderer();
    }
}

pub(crate) fn request_renderer(viewport: &str) -> Option<RendererRequest> {
    PROBE
        .get()?
        .state
        .lock()
        .unwrap()
        .request_renderer(viewport)
}

pub(crate) fn renderer_observed(request: RendererRequest, value: String) {
    if let Some(probe) = PROBE.get() {
        probe
            .state
            .lock()
            .unwrap()
            .renderer_observed(request, value);
    }
}

/// Stable non-cryptographic comparison of disposable fixture contents; never logs text.
pub(crate) fn fingerprint(text: &str) -> String {
    let hash = text.bytes().fold(0xcbf29ce484222325_u64, |hash, byte| {
        (hash ^ u64::from(byte)).wrapping_mul(0x100000001b3)
    });
    format!("{hash:016x}")
}

/// The explicitly opted-in desktop session remains enabled between snapshots.
/// Workers must not use `requested`, which only describes unpublished UI work.
pub(crate) fn enabled() -> bool {
    PROBE.get().is_some()
}

pub(crate) fn requested_for(viewport: &str) -> bool {
    PROBE
        .get()
        .is_some_and(|probe| probe.state.lock().unwrap().requested_for(viewport))
}

#[cfg(test)]
pub(crate) fn requested() -> bool {
    PROBE.get().is_some_and(|probe| {
        let state = probe.state.lock().unwrap();
        state.requested > state.published
    })
}

pub(crate) fn publish(document: Value) {
    if let Some(probe) = PROBE.get() {
        let mut state = probe.state.lock().unwrap();
        if state.document["viewport"] == document["viewport"]
            && state.document["frame"]
                .as_u64()
                .zip(document["frame"].as_u64())
                .is_some_and(|(previous, current)| current <= previous)
        {
            return;
        }
        state.publish_document(document);
        probe.changed.notify_all();
    }
}

/// Read the actual hit rectangle; the external driver posts a native mouse event.
/// Coordinates are global macOS screen points, including egui interface zoom.
pub(crate) fn observe(name: &str, response: &egui::Response) {
    let Some(probe) = PROBE.get() else {
        return;
    };
    let context = &response.ctx;
    let origin = context.input(|input| input.viewport().inner_rect.map(|r| r.min));
    let Some(origin) = origin else {
        return;
    };
    let zoom = context.zoom_factor();
    let point = screen_point(origin, response.interact_rect, zoom);
    let size = response.interact_rect.size() * zoom;
    let selection = egui::text_edit::TextEditState::load(context, response.id)
        .and_then(|state| state.cursor.char_range())
        .map(|range| [range.primary.index.0, range.secondary.index.0]);
    let target = json!({"x": point.x, "y": point.y, "width": size.x, "height": size.y, "enabled": response.enabled(), "hovered": response.hovered(),
        "focused": response.has_focus() && context.input(|input| input.focused), "selection": selection,
        "viewport": format!("{:?}", context.viewport_id()), "frame": context.cumulative_frame_nr()});
    probe
        .state
        .lock()
        .unwrap()
        .observed_targets
        .insert(name.into(), (target, Instant::now()));
}

/// Read-only scroll geometry for a real wheel-input journey. No document text
/// or off-screen rows enter the inspection stream.
pub(crate) fn observe_scroll(
    name: &str,
    context: &egui::Context,
    viewport: egui::Rect,
    content_height: f32,
    offset: f32,
    sticky_search: bool,
) {
    let Some(probe) = PROBE.get() else {
        return;
    };
    let origin = context.input(|input| input.viewport().inner_rect.map(|rect| rect.min));
    let Some(origin) = origin else {
        return;
    };
    let point = screen_point(origin, viewport, context.zoom_factor());
    probe.state.lock().unwrap().observed_scrolls.insert(
        name.into(),
        json!({"x": point.x, "y": point.y, "offset": offset, "sticky_search": sticky_search,
            "content_height": content_height, "viewport_height": viewport.height(),
            "frame": context.cumulative_frame_nr()}),
    );
}

/// Observe untouched platform input even when no snapshot is pending. The
/// latest fixed-size receipt survives later idle paints and snapshot wakeups.
pub(crate) fn observe_wheel_input(context: &egui::Context) {
    let Some(probe) = PROBE.get() else {
        return;
    };
    let receipt = context.input(|input| {
        let mut count = 0;
        let mut delta = egui::Vec2::ZERO;
        let mut last = None;
        for event in &input.raw.events {
            if let egui::Event::MouseWheel {
                unit,
                delta: movement,
                phase,
                modifiers,
            } = event
            {
                count += 1;
                delta += *movement;
                last = Some((unit, movement, phase, modifiers));
            }
        }
        let (unit, movement, phase, modifiers) = last?;
        Some((
            input.time,
            json!({"count": count, "delta_sum": [delta.x, delta.y],
                "last": {"unit": format!("{unit:?}"), "delta": [movement.x, movement.y],
                    "phase": format!("{phase:?}"), "modifiers": {"alt": modifiers.alt,
                        "ctrl": modifiers.ctrl, "shift": modifiers.shift,
                        "mac_cmd": modifiers.mac_cmd, "command": modifiers.command}},
                "pointer": input.pointer.latest_pos().map(|p| [p.x, p.y]),
                "focused": input.focused}),
        ))
    });
    if let Some((time, mut receipt)) = receipt {
        receipt["dragged_id_present"] = context.dragged_id().is_some().into();
        receipt["context"] = wheel_context_id(context).into();
        probe.state.lock().unwrap().observed_wheel.record_raw(
            &format!("{:?}", context.viewport_id()),
            context.cumulative_frame_nr(),
            time,
            receipt,
        );
    }
}

fn wheel_stage(context: &egui::Context) -> Value {
    let mut sample = context.input(|input| {
        json!({"smooth_delta": [input.smooth_scroll_delta.x, input.smooth_scroll_delta.y],
            "pointer": input.pointer.latest_pos().map(|p| [p.x, p.y]),
            "focused": input.focused})
    });
    sample["dragged_id_present"] = context.dragged_id().is_some().into();
    sample
}

pub(crate) fn begin_table_wheel(context: &egui::Context) -> Option<TableWheelPass> {
    let probe = PROBE.get()?;
    observe_wheel_input(context);
    let (raw, smooth) = context.input(|input| {
        (
            input
                .raw
                .events
                .iter()
                .any(|event| matches!(event, egui::Event::MouseWheel { .. })),
            input.smooth_scroll_delta != egui::Vec2::ZERO,
        )
    });
    if !raw && !smooth {
        return None;
    }
    let viewport = format!("{:?}", context.viewport_id());
    let mut sample = json!({"viewport": viewport, "context": wheel_context_id(context),
        "frame": context.cumulative_frame_nr(), "raw_this_frame": raw,
        "outer_before": wheel_stage(context), "hovered_cell": null,
        "first_consumer": null, "outer_after": null});
    let mut state = probe.state.lock().unwrap();
    let wheel = &mut state.observed_wheel;
    wheel.next_route = wheel
        .next_route
        .checked_add(1)
        .expect("wheel route exhausted");
    let pass = TableWheelPass(wheel.next_route);
    sample["route"] = pass.0.into();
    sample["raw_sequence"] = wheel
        .raw
        .get(&viewport)
        .map(|receipt| receipt["sequence"].clone())
        .unwrap_or(Value::Null);
    if raw {
        wheel.table_input = Some(sample.clone());
    }
    if smooth {
        wheel.table_smooth = Some(sample);
    }
    Some(pass)
}

pub(crate) fn table_wheel_cell_before(
    ui: &egui::Ui,
    inner: egui::Rect,
    pass: Option<TableWheelPass>,
) -> Option<WheelCellBefore> {
    pass?;
    Some(WheelCellBefore {
        input: wheel_stage(ui.ctx()),
        delta: ui.input(|input| input.smooth_scroll_delta),
        hovered: ui.rect_contains_pointer(inner),
    })
}

pub(crate) fn observe_table_wheel_cell(
    ui: &egui::Ui,
    pass: TableWheelPass,
    cell: (usize, usize),
    before: WheelCellBefore,
    scroll: &egui::scroll_area::ScrollAreaOutput<egui::Response>,
) {
    let Some(probe) = PROBE.get() else {
        return;
    };
    let delta = ui.input(|input| input.smooth_scroll_delta);
    let consumed = before.delta != delta;
    if !before.hovered && !consumed {
        return;
    }
    let viewport = scroll.inner_rect;
    let clip = ui.clip_rect();
    let value = json!({"cell": [cell.0, cell.1], "logical_hover": before.hovered,
        "before": before.input, "after": wheel_stage(ui.ctx()),
        "viewport": [viewport.left(), viewport.top(), viewport.right(), viewport.bottom()],
        "viewport_size": [viewport.width(), viewport.height()],
        "clip": [clip.left(), clip.top(), clip.right(), clip.bottom()],
        "content_size": [scroll.content_size.x, scroll.content_size.y],
        "offset": [scroll.state.offset.x, scroll.state.offset.y],
        "response_hovered": scroll.inner.hovered()});
    let mut state = probe.state.lock().unwrap();
    if before.hovered {
        state
            .observed_wheel
            .update_table(pass, "hovered_cell", value.clone());
    }
    if consumed {
        state
            .observed_wheel
            .update_table(pass, "first_consumer", value);
    }
}

pub(crate) fn end_table_wheel(
    context: &egui::Context,
    pass: Option<TableWheelPass>,
    viewport: egui::Rect,
    content: egui::Vec2,
    offset: egui::Vec2,
) {
    let (Some(probe), Some(pass)) = (PROBE.get(), pass) else {
        return;
    };
    let value = json!({"input": wheel_stage(context),
        "viewport": [viewport.left(), viewport.top(), viewport.right(), viewport.bottom()],
        "viewport_size": [viewport.width(), viewport.height()],
        "content_size": [content.x, content.y], "offset": [offset.x, offset.y]});
    probe
        .state
        .lock()
        .unwrap()
        .observed_wheel
        .update_table(pass, "outer_after", value);
}

fn screen_point(origin: egui::Pos2, hit: egui::Rect, zoom: f32) -> egui::Pos2 {
    (origin + hit.center().to_vec2()) * zoom
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn wheel_receipts_are_bounded_and_do_not_count_the_same_input_pass_twice() {
        let mut probe = super::WheelProbe::default();
        for viewport in 0..20 {
            probe.record_raw(
                &format!("{viewport:04x}"),
                1,
                2.0,
                serde_json::json!({"count": 1}),
            );
        }
        assert_eq!(probe.sequence, 20);
        assert_eq!(probe.raw.len(), super::WHEEL_VIEWPORT_LIMIT);
        assert!(!probe.raw.contains_key("000b"));
        assert!(probe.raw.contains_key("000c"));
        probe.record_raw("0013", 1, 2.0, serde_json::json!({"count": 1}));
        assert_eq!(probe.sequence, 20);
        probe.record_raw("0013", 2, 3.0, serde_json::json!({"count": 2}));
        assert_eq!(probe.sequence, 21);
        assert_eq!(probe.raw["0013"]["count"], 2);
        probe.record_raw("0013", 2, 3.0, serde_json::json!({"context": 1}));
        probe.record_raw("0013", 2, 3.0, serde_json::json!({"context": 1}));
        assert_eq!(probe.sequence, 22);
        probe.record_raw("0013", 2, 3.0, serde_json::json!({"context": 2}));
        assert_eq!(
            probe.sequence, 23,
            "a new context may reuse both frame and time"
        );
    }

    #[test]
    fn table_wheel_stages_update_only_their_own_retained_frame() {
        let mut probe = super::WheelProbe {
            table_input: Some(serde_json::json!({"viewport": "table", "frame": 2, "route": 2})),
            table_smooth: Some(serde_json::json!({"viewport": "table", "frame": 3, "route": 3})),
            ..Default::default()
        };
        probe.update_table(super::TableWheelPass(4), "outer_after", true.into());
        probe.update_table(super::TableWheelPass(1), "outer_after", true.into());
        assert!(probe.table_input.as_ref().unwrap()["outer_after"].is_null());
        probe.update_table(super::TableWheelPass(3), "outer_after", true.into());
        assert!(probe.table_input.as_ref().unwrap()["outer_after"].is_null());
        assert_eq!(probe.table_smooth.as_ref().unwrap()["outer_after"], true);
        probe.update_table(super::TableWheelPass(3), "first_consumer", 1.into());
        probe.update_table(super::TableWheelPass(3), "first_consumer", 2.into());
        assert_eq!(probe.table_smooth.as_ref().unwrap()["first_consumer"], 1);
        probe.update_table(super::TableWheelPass(2), "outer_after", false.into());
        assert_eq!(probe.table_input.as_ref().unwrap()["outer_after"], false);
        assert_eq!(probe.table_smooth.as_ref().unwrap()["outer_after"], true);
    }

    #[test]
    fn published_wheel_receipts_remain_frozen_with_the_document_frame() {
        let mut state = super::State::default();
        state
            .observed_wheel
            .record_raw("root", 1, 1.0, serde_json::json!({"count": 1}));
        state.publish_document(serde_json::json!({"viewport": "root", "frame": 1}));
        state
            .observed_wheel
            .record_raw("root", 2, 2.0, serde_json::json!({"count": 1}));
        assert_eq!(state.wheel["raw"]["root"]["sequence"], 1);
        assert_eq!(
            state.observed_wheel.snapshot()["raw"]["root"]["sequence"],
            2
        );
    }

    #[test]
    fn an_idle_reopened_viewport_cannot_rewrite_a_retained_wheel_route() {
        const CHILD: &str = "TIPTOPTYP_WHEEL_REOPEN_PROBE_CHILD";
        if std::env::var_os(CHILD).is_none() {
            let output = std::process::Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "desktop_test::tests::an_idle_reopened_viewport_cannot_rewrite_a_retained_wheel_route",
                ])
                .env(CHILD, "1")
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "isolated wheel check failed: {}{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
            return;
        }
        assert!(
            super::PROBE
                .set(super::Probe {
                    state: std::sync::Mutex::new(super::State::default()),
                    changed: std::sync::Condvar::new(),
                })
                .is_ok()
        );
        let first = eframe::egui::Context::default();
        let reopened = eframe::egui::Context::default();
        let area = eframe::egui::Rect::from_min_size(
            eframe::egui::Pos2::ZERO,
            eframe::egui::vec2(300.0, 200.0),
        );
        for (context, wheel, offset) in [(&first, true, 100.0), (&reopened, false, 0.0)] {
            let events = if wheel {
                vec![eframe::egui::Event::MouseWheel {
                    unit: eframe::egui::MouseWheelUnit::Line,
                    delta: eframe::egui::vec2(0.0, -8.0),
                    phase: eframe::egui::TouchPhase::Move,
                    modifiers: eframe::egui::Modifiers::NONE,
                }]
            } else {
                Vec::new()
            };
            context
                .run_ui(
                    eframe::egui::RawInput {
                        screen_rect: Some(area),
                        time: Some(1.0),
                        events,
                        ..Default::default()
                    },
                    |ui| {
                        let pass = super::begin_table_wheel(ui.ctx());
                        super::end_table_wheel(
                            ui.ctx(),
                            pass,
                            area,
                            eframe::egui::vec2(300.0, 1000.0),
                            eframe::egui::vec2(0.0, offset),
                        );
                    },
                )
                .drop_without_applying_deltas();
        }
        let state = super::PROBE.get().unwrap().state.lock().unwrap();
        assert_eq!(
            state.observed_wheel.table_input.as_ref().unwrap()["outer_after"]["offset"][1],
            100.0,
            "an idle new context reused the old frame number and overwrote its receipt"
        );
    }

    #[test]
    fn hit_coordinates_include_native_origin_and_interface_zoom() {
        use eframe::egui::{Rect, pos2};
        let hit = Rect::from_min_max(pos2(20.0, 40.0), pos2(60.0, 80.0));
        assert_eq!(
            super::screen_point(pos2(100.0, 200.0), hit, 1.0),
            pos2(140.0, 260.0)
        );
        assert_eq!(
            super::screen_point(pos2(-100.0, 200.0), hit, 1.5),
            pos2(-90.0, 390.0)
        );
    }

    #[test]
    fn document_and_hit_targets_are_published_as_one_frame() {
        let mut state = super::State::default();
        state.observed_targets.insert(
            "button".into(),
            (serde_json::json!({"frame": 1}), std::time::Instant::now()),
        );
        state.publish_document(serde_json::json!({"frame": 1}));
        state.observed_targets.get_mut("button").unwrap().0["frame"] = 2.into();
        assert_eq!(state.document["frame"], state.targets["button"].0["frame"]);
        state.publish_document(serde_json::json!({"frame": 2}));
        assert_eq!(state.document["frame"], state.targets["button"].0["frame"]);
    }

    #[test]
    fn navigation_releases_dropped_callback_and_rejects_previous_renderer() {
        let mut state = super::State::default();
        let old = state.request_renderer("FFFF").unwrap();
        assert!(state.request_renderer("FFFF").is_none());
        state.reset_renderer(); // Pending pre-load callback was discarded by WebView.
        let current = state.request_renderer("FFFF").unwrap();
        state.renderer_observed(old, "{}".into());
        assert!(state.renderer.sample().is_none());
        assert!(state.request_renderer("FFFF").is_none());
        state.renderer_observed(current, "{\"ready\":true}".into());
        assert_eq!(state.renderer.sample().unwrap().1.value["ready"], true);
        assert!(state.request_renderer("FFFF").is_some());
    }

    #[test]
    fn renderer_observations_consume_only_their_own_pending_request() {
        let mut state = super::State::default();
        assert_eq!(state.renderer.phase(), "unbound");
        let old = state.request_renderer("FFFF").unwrap();
        assert_eq!(state.renderer.phase(), "pending");
        state.renderer_observed(old, "{\"sample\":1}".into());
        assert_eq!(state.renderer.phase(), "idle");
        let current = state.request_renderer("FFFF").unwrap();
        assert_ne!(old, current);
        assert_eq!(old.generation, current.generation);
        // Retain a usable old observation while the next one is pending.
        assert_eq!(state.renderer.sample().unwrap().1.value["sample"], 1);
        state.renderer_observed(old, "{\"sample\":99}".into());
        assert_eq!(state.renderer.phase(), "pending");
        assert!(state.request_renderer("FFFF").is_none());
        assert_eq!(state.renderer.sample().unwrap().1.value["sample"], 1);
        state.renderer_observed(current, "{\"sample\":2}".into());
        assert_eq!(state.renderer.phase(), "idle");
        state.renderer_observed(current, "{\"sample\":99}".into());
        assert_eq!(state.renderer.sample().unwrap().1.value["sample"], 2);
    }

    #[test]
    fn renderer_reset_invalidates_pending_observation_without_changing_viewport() {
        let mut state = super::State::default();
        let old = state.request_renderer("FFFF").unwrap();
        state.reset_renderer();
        assert_eq!(state.renderer.phase(), "idle");
        let generation = state.renderer.generation;
        let current = state.request_renderer("FFFF").unwrap();
        assert_eq!(current.generation, generation);
        assert!(current.generation > old.generation);
        state.renderer_observed(old, "{\"stale\":true}".into());
        assert_eq!(state.renderer.phase(), "pending");
        assert!(state.renderer.sample().is_none());
        state.renderer_observed(current, "not JSON".into());
        assert_eq!(state.renderer.phase(), "idle");
        assert!(state.renderer.sample().unwrap().1.value.is_null());
    }

    #[test]
    fn targeted_observation_rejects_another_window_and_its_renderer() {
        let mut state = super::State {
            requested: 1,
            requested_viewport: Some("17F2".into()),
            ..Default::default()
        };
        assert!(state.requested_for("17F2"));
        assert!(!state.requested_for("FFFF"));
        assert!(state.request_renderer("FFFF").is_none());
        state.publish_document(serde_json::json!({"viewport": "FFFF", "frame": 1}));
        assert_eq!(state.published, 0);
        let secondary_epoch = state.request_renderer("17F2").unwrap();
        state.renderer_observed(secondary_epoch, "{\"ready\":true}".into());
        state.publish_document(serde_json::json!({"viewport": "17F2", "frame": 1}));
        assert_eq!(state.published, 1);
        assert!(!state.requested_for("17F2"));
        state.requested += 1;
        state.requested_viewport = Some("FFFF".into());
        let root_epoch = state.request_renderer("FFFF").unwrap();
        assert!(root_epoch.generation > secondary_epoch.generation);
        assert!(state.renderer.sample().is_none());
        state.renderer_observed(secondary_epoch, "{\"stale\":true}".into());
        assert!(state.renderer.sample().is_none());
        state.renderer_observed(root_epoch, "{\"ready\":true}".into());
        assert_eq!(state.renderer.sample().unwrap().0, "FFFF");
    }

    #[test]
    fn snapshot_commands_accept_only_bounded_viewport_identifiers() {
        assert_eq!(super::snapshot_viewport("snapshot\n"), Some(None));
        assert_eq!(
            super::snapshot_viewport("snapshot 17F2\n"),
            Some(Some("17F2"))
        );
        assert_eq!(
            super::snapshot_viewport("snapshot \"17F2\"\n"),
            Some(Some("\"17F2\""))
        );
        for invalid in [
            "snapshot \n",
            "snapshot 17F2",
            "snapshot nope\n",
            "snapshot 17 F2\n",
            "snapshot 00000000000000000\n",
            "snapshot 17F2\nextra\n",
        ] {
            assert!(super::snapshot_viewport(invalid).is_none(), "{invalid:?}");
        }
    }

    #[test]
    fn fixture_fingerprints_match_runner_vectors() {
        assert_eq!(super::fingerprint(""), "cbf29ce484222325");
        assert_eq!(super::fingerprint("hello"), "a430d84680aabd0b");
        assert_eq!(super::fingerprint("é🙂z"), "3a046d85bff8a56f");
    }

    #[test]
    fn disabled_probe_does_not_request_work() {
        assert!(!super::enabled());
        assert!(!super::requested());
    }

    #[test]
    fn enabled_session_survives_snapshot_publication() {
        const CHILD: &str = "TIPTOPTYP_TEST_ENABLED_PROBE_CHILD";
        if std::env::var_os(CHILD).is_none() {
            // Isolate the process-wide OnceLock so this regression cannot enable
            // the test endpoint in unrelated tests running in parallel.
            let directory = tempfile::tempdir().unwrap();
            std::fs::set_permissions(directory.path(), std::fs::Permissions::from_mode(0o700))
                .unwrap();
            let output = std::process::Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "desktop_test::tests::enabled_session_survives_snapshot_publication",
                ])
                .env(CHILD, "1")
                .env("TIPTOPTYP_DESKTOP_TEST_DIR", directory.path())
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "isolated session check failed: {}{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
            return;
        }
        assert!(!super::enabled());
        assert!(super::start().unwrap().is_some());
        assert!(super::enabled());
        assert!(!super::requested());
        super::PROBE.get().unwrap().state.lock().unwrap().requested = 1;
        assert!(super::requested());
        super::publish(serde_json::json!({"viewport": "root", "frame": 1}));
        assert!(!super::requested(), "the snapshot was completely published");
        assert!(
            super::enabled(),
            "asynchronous fixture workers must remain enabled between inspection requests"
        );
    }
}
