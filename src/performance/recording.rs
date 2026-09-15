use eframe::egui;
use serde::Serialize;
use std::{
    collections::BTreeMap,
    fs::{File, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
    sync::{Mutex, OnceLock},
    time::{Duration, Instant},
};

const MAX_SCOPES: usize = 64;
const CLOSE_GRACE: Duration = Duration::from_secs(2);
static RECORDER: OnceLock<Recorder> = OnceLock::new();

pub(crate) struct Session(Option<File>);

impl Session {
    pub(crate) fn from_env() -> Result<Self, String> {
        let Some(directory) = std::env::var_os("TIPTOPTYP_PROFILE_DIR") else {
            return Ok(Self(None));
        };
        let directory = PathBuf::from(directory);
        let seconds = |key: &str, default: u64, minimum: u64| {
            parse_seconds(std::env::var(key).ok().as_deref(), default, minimum)
                .map_err(|error| format!("{key}: {error}"))
        };
        let warmup = seconds("TIPTOPTYP_PROFILE_WARMUP", 3, 0)?;
        let duration = seconds("TIPTOPTYP_PROFILE_SECONDS", 10, 1)?;
        if !directory.is_dir() || directory.join("ready").exists() {
            return Err("profile directory must exist and must not contain an earlier run".into());
        }
        let output = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(directory.join("summary.json"))
            .map_err(|error| error.to_string())?;
        let storage = create_profile_storage(&directory)?;
        RECORDER
            .set(Recorder {
                directory,
                storage,
                warmup,
                duration,
                window: OnceLock::new(),
                stats: Mutex::new(Statistics::default()),
            })
            .map_err(|_| "only one profiling session is allowed per process")?;
        Ok(Self(Some(output)))
    }

    pub(crate) fn persistence_path(&self) -> Option<PathBuf> {
        self.0
            .as_ref()
            .map(|_| RECORDER.get().unwrap().storage.clone())
    }

    pub(crate) fn finish(self) -> Result<(), String> {
        let Some(mut file) = self.0 else {
            return Ok(());
        };
        let recorder = RECORDER.get().expect("profiling session was initialized");
        let now = Instant::now();
        let measured = recorder.window.get().map_or(Duration::ZERO, |start| {
            now.saturating_duration_since(*start + recorder.warmup)
                .min(recorder.duration)
        });
        let stats = recorder
            .stats
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let report = Report {
            schema_version: 1,
            complete: measured == recorder.duration,
            measured_seconds: measured.as_secs_f64(),
            dropped_scopes: stats.dropped_scopes,
            root_repaint_requests: stats
                .repaint_requests
                .iter()
                .map(|(&(file, line), &requests)| RepaintReport {
                    file,
                    line,
                    requests,
                })
                .collect(),
            dropped_repaint_locations: stats.dropped_repaint_locations,
            scopes: stats
                .scopes
                .iter()
                .map(|(name, stat)| stat.report(name))
                .collect(),
        };
        serde_json::to_writer_pretty(&mut file, &report).map_err(|error| error.to_string())?;
        file.write_all(b"\n")
            .and_then(|_| file.flush())
            .map_err(|error| error.to_string())?;
        if !report.complete {
            return Err("profiling session ended before its measurement window completed".into());
        }
        Ok(())
    }
}

fn create_profile_storage(directory: &Path) -> Result<PathBuf, String> {
    let storage = directory.join("app-state");
    std::fs::create_dir(&storage)
        .map_err(|error| format!("cannot create fresh profiling app state: {error}"))?;
    Ok(storage)
}

fn parse_seconds(value: Option<&str>, default: u64, minimum: u64) -> Result<Duration, String> {
    let seconds = value
        .map_or(Ok(default), str::parse::<u64>)
        .map_err(|_| "expected whole seconds".to_owned())?;
    if !(minimum..=300).contains(&seconds) {
        return Err(format!("expected {minimum}..=300 seconds"));
    }
    Ok(Duration::from_secs(seconds))
}

struct Recorder {
    directory: PathBuf,
    storage: PathBuf,
    warmup: Duration,
    duration: Duration,
    window: OnceLock<Instant>,
    stats: Mutex<Statistics>,
}

impl Recorder {
    fn admits(&self, start: Instant, end: Instant) -> bool {
        self.window.get().is_some_and(|origin| {
            let begin = *origin + self.warmup;
            start >= begin && end <= begin + self.duration
        })
    }
}

/// Wait for the initial QA capture to finish before warming up. Arm one timer,
/// rather than relying on another UI pass in a fully idle native viewport.
pub(crate) fn tick(context: &egui::Context, ready: impl FnOnce() -> bool) {
    let Some(recorder) = RECORDER.get() else {
        return;
    };
    let now = Instant::now();
    if recorder.admits(now, now) {
        // Record call sites, not free-form reasons (which may include user
        // text). Bounded aggregation only; write once at session completion.
        let causes = context.repaint_causes();
        let mut stats = recorder
            .stats
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        for cause in causes {
            stats.observe_repaint(cause.file, cause.line);
        }
    }
    if recorder.window.get().is_some() || !ready() {
        return;
    }
    let now = Instant::now();
    if recorder.window.set(now).is_err() {
        return;
    }
    if let Err(error) = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(recorder.directory.join("ready"))
        .and_then(|mut file| file.write_all(b"ready\n"))
        .and_then(|()| {
            arm_close_deadline(
                context,
                now + recorder.warmup + recorder.duration + CLOSE_GRACE,
            )
            .map(drop)
        })
    {
        eprintln!("profiling readiness/timer failed: {error}");
        context.send_viewport_cmd_to(egui::ViewportId::ROOT, egui::ViewportCommand::Close);
    }
}

fn arm_close_deadline(
    context: &egui::Context,
    deadline: Instant,
) -> std::io::Result<std::thread::JoinHandle<()>> {
    let context = context.clone();
    std::thread::Builder::new()
        .name("profiling-deadline".into())
        .spawn(move || {
            std::thread::sleep(deadline.saturating_duration_since(Instant::now()));
            // send_viewport_cmd_to also wakes the native event loop. The timer
            // sleeps once, outside the measured work; it never drives UI frames.
            context.send_viewport_cmd_to(egui::ViewportId::ROOT, egui::ViewportCommand::Close);
        })
}

pub(crate) struct Span {
    name: &'static str,
    start: Option<Instant>,
}

#[inline]
pub(crate) fn span(name: &'static str) -> Span {
    let start = RECORDER.get().and_then(|recorder| {
        let now = Instant::now();
        recorder.admits(now, now).then_some(now)
    });
    Span { name, start }
}

impl Drop for Span {
    fn drop(&mut self) {
        let Some(start) = self.start else {
            return;
        };
        let end = Instant::now();
        let recorder = RECORDER.get().expect("an active span has a recorder");
        if recorder.admits(start, end) {
            recorder
                .stats
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .observe(self.name, end - start);
        }
    }
}

#[derive(Default)]
struct Statistics {
    repaint_requests: BTreeMap<(&'static str, u32), u64>,
    dropped_repaint_locations: u64,
    scopes: BTreeMap<&'static str, Distribution>,
    dropped_scopes: u64,
}
impl Statistics {
    fn observe_repaint(&mut self, file: &'static str, line: u32) {
        if self.repaint_requests.len() >= MAX_SCOPES
            && !self.repaint_requests.contains_key(&(file, line))
        {
            self.dropped_repaint_locations = self.dropped_repaint_locations.saturating_add(1);
            return;
        }
        let count = self.repaint_requests.entry((file, line)).or_default();
        *count = count.saturating_add(1);
    }
    fn observe(&mut self, name: &'static str, elapsed: Duration) {
        if self.scopes.len() == MAX_SCOPES && !self.scopes.contains_key(name) {
            self.dropped_scopes += 1;
            return;
        }
        self.scopes.entry(name).or_default().observe(elapsed);
    }
}

struct Distribution {
    count: u64,
    total_ns: u128,
    max_ns: u64,
    buckets: [u64; 65],
}
impl Default for Distribution {
    fn default() -> Self {
        Self {
            count: 0,
            total_ns: 0,
            max_ns: 0,
            buckets: [0; 65],
        }
    }
}
impl Distribution {
    fn observe(&mut self, elapsed: Duration) {
        let ns = elapsed.as_nanos().min(u64::MAX as u128) as u64;
        self.count += 1;
        self.total_ns += u128::from(ns);
        self.max_ns = self.max_ns.max(ns);
        self.buckets[(64 - ns.leading_zeros()) as usize] += 1;
    }
    fn percentile_upper_ns(&self, percentile: u64) -> u64 {
        let rank = (self.count * percentile).div_ceil(100);
        let mut cumulative = 0;
        for (index, count) in self.buckets.iter().enumerate() {
            cumulative += count;
            if cumulative >= rank {
                return ((1_u128 << index) - 1).min(u64::MAX as u128) as u64;
            }
        }
        0
    }
    fn report(&self, name: &'static str) -> ScopeReport {
        ScopeReport {
            name,
            calls: self.count,
            total_ms: self.total_ns as f64 / 1_000_000.0,
            mean_us: self.total_ns as f64 / self.count.max(1) as f64 / 1_000.0,
            max_us: self.max_ns as f64 / 1_000.0,
            p50_upper_us: self.percentile_upper_ns(50) as f64 / 1_000.0,
            p95_upper_us: self.percentile_upper_ns(95) as f64 / 1_000.0,
        }
    }
}

#[derive(Serialize)]
struct Report {
    root_repaint_requests: Vec<RepaintReport>,
    dropped_repaint_locations: u64,
    schema_version: u8,
    complete: bool,
    measured_seconds: f64,
    dropped_scopes: u64,
    scopes: Vec<ScopeReport>,
}

#[derive(Serialize)]
struct RepaintReport {
    file: &'static str,
    line: u32,
    requests: u64,
}
#[derive(Serialize)]
struct ScopeReport {
    name: &'static str,
    calls: u64,
    total_ms: f64,
    mean_us: f64,
    max_us: f64,
    p50_upper_us: f64,
    p95_upper_us: f64,
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn repaint_call_site_storage_is_bounded_and_known_sites_keep_counting() {
        let mut stats = Statistics::default();
        for line in 0..100 {
            stats.observe_repaint("source.rs", line);
        }
        stats.observe_repaint("source.rs", 0);
        assert_eq!(stats.repaint_requests.len(), MAX_SCOPES);
        assert_eq!(stats.repaint_requests[&("source.rs", 0)], 2);
        assert_eq!(stats.dropped_repaint_locations, 36);
    }
    #[test]
    fn measurement_excludes_startup_warmup_and_boundary_crossing_work() {
        let origin = Instant::now();
        let recorder = Recorder {
            directory: PathBuf::new(),
            storage: PathBuf::new(),
            warmup: Duration::from_secs(2),
            duration: Duration::from_secs(3),
            window: OnceLock::new(),
            stats: Mutex::default(),
        };
        assert!(!recorder.admits(origin, origin));
        recorder.window.set(origin).unwrap();
        let at = |seconds| origin + Duration::from_secs(seconds);
        assert!(!recorder.admits(at(1), at(3)));
        assert!(recorder.admits(at(2), at(5)));
        assert!(!recorder.admits(at(4), at(6)));
    }
    #[test]
    fn distributions_are_bounded_and_report_explicit_quantile_upper_bounds() {
        let mut distribution = Distribution::default();
        for ns in [0, 1, 2, 3, 100] {
            distribution.observe(Duration::from_nanos(ns));
        }
        assert_eq!(distribution.count, 5);
        assert_eq!(distribution.total_ns, 106);
        assert_eq!(distribution.percentile_upper_ns(50), 3);
        assert_eq!(distribution.percentile_upper_ns(95), 127);
        let json = serde_json::to_value(distribution.report("existing")).unwrap();
        assert_eq!(json["calls"], 5);
        assert!(json.get("p95_upper_us").is_some());
        let mut stats = Statistics::default();
        for name in [
            "scope.0", "scope.1", "scope.2", "scope.3", "scope.4", "scope.5", "scope.6", "scope.7",
            "scope.8", "scope.9", "scope.10", "scope.11", "scope.12", "scope.13", "scope.14",
            "scope.15", "scope.16", "scope.17", "scope.18", "scope.19", "scope.20", "scope.21",
            "scope.22", "scope.23", "scope.24", "scope.25", "scope.26", "scope.27", "scope.28",
            "scope.29", "scope.30", "scope.31", "scope.32", "scope.33", "scope.34", "scope.35",
            "scope.36", "scope.37", "scope.38", "scope.39", "scope.40", "scope.41", "scope.42",
            "scope.43", "scope.44", "scope.45", "scope.46", "scope.47", "scope.48", "scope.49",
            "scope.50", "scope.51", "scope.52", "scope.53", "scope.54", "scope.55", "scope.56",
            "scope.57", "scope.58", "scope.59", "scope.60", "scope.61", "scope.62", "scope.63",
            "scope.64",
        ] {
            stats.observe(name, Duration::ZERO);
        }
        assert_eq!(stats.scopes.len(), MAX_SCOPES);
        assert_eq!(stats.dropped_scopes, 1);
        stats.observe("scope.0", Duration::from_nanos(1));
        assert_eq!(stats.scopes["scope.0"].count, 2);
    }
    #[test]
    fn timing_configuration_rejects_invalid_and_unbounded_runs() {
        assert_eq!(parse_seconds(None, 3, 0).unwrap(), Duration::from_secs(3));
        for value in ["", "-1", "1.5", "301", "no"] {
            assert!(parse_seconds(Some(value), 3, 0).is_err());
        }
        assert!(parse_seconds(Some("0"), 10, 1).is_err());
    }

    #[test]
    fn profile_state_is_empty_local_and_never_reuses_existing_settings() {
        let directory = tempfile::tempdir().unwrap();
        let storage = create_profile_storage(directory.path()).unwrap();
        assert_eq!(storage.parent(), Some(directory.path()));
        assert_eq!(std::fs::read_dir(&storage).unwrap().count(), 0);
        let sentinel = storage.join("settings");
        std::fs::write(&sentinel, "existing state").unwrap();
        assert!(create_profile_storage(directory.path()).is_err());
        assert_eq!(std::fs::read_to_string(sentinel).unwrap(), "existing state");
    }

    #[test]
    fn close_deadline_does_not_need_an_intervening_ui_frame() {
        let context = egui::Context::default();
        let mut initial = context.run_ui(Default::default(), |_| {});
        initial.textures_delta.clear();
        arm_close_deadline(&context, Instant::now())
            .unwrap()
            .join()
            .unwrap();
        // No UI frames were driven while the timer ran. Its one close command
        // must already be queued for the root, independent of input or repaint.
        let mut output = context.run_ui(Default::default(), |_| {});
        output.textures_delta.clear();
        assert_eq!(
            output.viewport_output[&egui::ViewportId::ROOT]
                .commands
                .iter()
                .filter(|command| matches!(command, egui::ViewportCommand::Close))
                .count(),
            1
        );
    }
}
