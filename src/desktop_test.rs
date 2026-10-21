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
    requested_viewport: Option<String>,
    document: Value,
    renderer: Option<(Value, Instant)>,
    renderer_pending: bool,
    renderer_viewport: Option<String>,
    renderer_epoch: u64,
    targets: BTreeMap<String, (Value, Instant)>,
    observed_targets: BTreeMap<String, (Value, Instant)>,
    scrolls: BTreeMap<String, Value>,
    observed_scrolls: BTreeMap<String, Value>,
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
        self.published = self.requested;
    }
    fn reset_renderer(&mut self) {
        self.renderer_epoch += 1;
        self.renderer_pending = false;
        self.renderer = None;
    }
    fn request_renderer(&mut self, viewport: &str) -> Option<u64> {
        if self
            .requested_viewport
            .as_deref()
            .is_some_and(|owner| owner != viewport)
        {
            return None;
        }
        if self.renderer_viewport.as_deref() != Some(viewport) {
            self.reset_renderer();
            self.renderer_viewport = Some(viewport.to_owned());
        }
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
        "schema": 1, "pid": std::process::id(), "build": crate::build_info::VERSION,
        "request": request, "renderer_generation": state.renderer_epoch, "document": state.document, "targets": targets, "scrolls": state.scrolls,
        "renderer": state.renderer.as_ref().filter(|_| state.document["viewport"].as_str() == state.renderer_viewport.as_deref())
            .map(|(value, time)| json!({"viewport": state.renderer_viewport, "value": value, "age_ms": time.elapsed().as_millis()})),
    })
}

pub(crate) fn reset_renderer() {
    if let Some(probe) = PROBE.get() {
        probe.state.lock().unwrap().reset_renderer();
    }
}

pub(crate) fn request_renderer(viewport: &str) -> Option<u64> {
    PROBE
        .get()?
        .state
        .lock()
        .unwrap()
        .request_renderer(viewport)
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

fn screen_point(origin: egui::Pos2, hit: egui::Rect, zoom: f32) -> egui::Pos2 {
    (origin + hit.center().to_vec2()) * zoom
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::PermissionsExt;

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
        assert!(state.renderer.is_none());
        assert!(state.request_renderer("FFFF").is_none());
        state.renderer_observed(current, "{\"ready\":true}".into());
        assert_eq!(state.renderer.as_ref().unwrap().0["ready"], true);
        assert!(state.request_renderer("FFFF").is_some());
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
        assert!(root_epoch > secondary_epoch);
        assert!(state.renderer.is_none());
        state.renderer_observed(secondary_epoch, "{\"stale\":true}".into());
        assert!(state.renderer.is_none());
        state.renderer_observed(root_epoch, "{\"ready\":true}".into());
        assert_eq!(state.renderer_viewport.as_deref(), Some("FFFF"));
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
