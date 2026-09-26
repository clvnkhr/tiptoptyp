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

#[derive(Default)]
struct State {
    context: Option<egui::Context>,
    requested: u64,
    published: u64,
    document: Value,
    renderer: Option<(Value, Instant)>,
    renderer_pending: bool,
    renderer_epoch: u64,
    targets: BTreeMap<String, (Value, Instant)>,
    observed_targets: BTreeMap<String, (Value, Instant)>,
}
impl State {
    fn publish_document(&mut self, document: Value) {
        self.document = document;
        // Freeze hit targets with their document frame. The next native pass
        // may start before the socket reader wakes; it must not mix frames.
        self.targets.clone_from(&self.observed_targets);
        self.published = self.requested;
    }
    fn reset_renderer(&mut self) {
        self.renderer_epoch += 1;
        self.renderer_pending = false;
        self.renderer = None;
    }
    fn request_renderer(&mut self) -> Option<u64> {
        if self.renderer_pending {
            return None;
        }
        self.renderer_pending = true;
        Some(self.renderer_epoch)
    }
    fn renderer_observed(&mut self, epoch: u64, value: String) {
        if epoch != self.renderer_epoch {
            return;
        }
        self.renderer = Some((
            serde_json::from_str(&value).unwrap_or(Value::Null),
            Instant::now(),
        ));
        self.renderer_pending = false;
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
                let response = if read.is_ok() && command == "snapshot\n" {
                    snapshot()
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

fn snapshot() -> Value {
    let probe = PROBE.get().unwrap();
    let (context, request) = {
        let mut state = probe.state.lock().unwrap();
        state.requested += 1;
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
        "schema": 1, "pid": std::process::id(), "build": crate::build_info::VERSION,
        "request": request, "renderer_generation": state.renderer_epoch, "document": state.document, "targets": targets,
        "renderer": state.renderer.as_ref().map(|(value, time)| json!({"value": value, "age_ms": time.elapsed().as_millis()})),
    })
}

pub(crate) fn reset_renderer() {
    if let Some(probe) = PROBE.get() {
        probe.state.lock().unwrap().reset_renderer();
    }
}

pub(crate) fn request_renderer() -> Option<u64> {
    PROBE.get()?.state.lock().unwrap().request_renderer()
}

pub(crate) fn renderer_observed(epoch: u64, value: String) {
    if let Some(probe) = PROBE.get() {
        probe.state.lock().unwrap().renderer_observed(epoch, value);
    }
}

/// Stable non-cryptographic comparison of disposable fixture contents; never logs text.
pub(crate) fn fingerprint(text: &str) -> String {
    let hash = text.bytes().fold(0xcbf29ce484222325_u64, |hash, byte| {
        (hash ^ u64::from(byte)).wrapping_mul(0x100000001b3)
    });
    format!("{hash:016x}")
}

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
    let point = screen_point(origin, response.interact_rect, context.zoom_factor());
    let target = json!({"x": point.x, "y": point.y, "enabled": response.enabled(), "hovered": response.hovered(),
        "viewport": format!("{:?}", context.viewport_id()), "frame": context.cumulative_frame_nr()});
    probe
        .state
        .lock()
        .unwrap()
        .observed_targets
        .insert(name.into(), (target, Instant::now()));
}

fn screen_point(origin: egui::Pos2, hit: egui::Rect, zoom: f32) -> egui::Pos2 {
    (origin + hit.center().to_vec2()) * zoom
}

#[cfg(test)]
mod tests {
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
        let old = state.request_renderer().unwrap();
        assert!(state.request_renderer().is_none());
        state.reset_renderer(); // Pending pre-load callback was discarded by WebView.
        let current = state.request_renderer().unwrap();
        state.renderer_observed(old, "{}".into());
        assert!(state.renderer.is_none());
        assert!(state.request_renderer().is_none());
        state.renderer_observed(current, "{\"ready\":true}".into());
        assert_eq!(state.renderer.as_ref().unwrap().0["ready"], true);
        assert!(state.request_renderer().is_some());
    }

    #[test]
    fn fixture_fingerprints_match_runner_vectors() {
        assert_eq!(super::fingerprint(""), "cbf29ce484222325");
        assert_eq!(super::fingerprint("hello"), "a430d84680aabd0b");
        assert_eq!(super::fingerprint("é🙂z"), "3a046d85bff8a56f");
    }

    #[test]
    fn disabled_probe_does_not_request_work() {
        assert!(!super::requested());
    }
}
