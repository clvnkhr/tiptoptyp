//! Opt-in native submission timing; bounded memory, no rendering or repaint work.
use std::{collections::VecDeque, time::Instant};

const CAPACITY: usize = 256;

#[derive(Clone, Copy)]
pub(super) struct Queued {
    pub(super) at: Instant,
    pub(super) frame: u64,
    pub(super) viewport: u64,
}

#[derive(Default)]
pub(super) struct DrawSubmission {
    pub(super) upload_ms: f64,
}

pub(super) struct Draw {
    pub(super) queued: Queued,
    pub(super) callback: Instant,
    pub(super) initialized: Instant,
    pub(super) draw_counter: u64,
    pub(super) shader_time_s: f32,
    pub(super) draw_submission_ms: f64,
    pub(super) upload_submission_ms: f64,
    pub(super) instances: usize,
    pub(super) uploaded: bool,
}

#[derive(serde::Serialize)]
struct Sample {
    sequence: u64,
    kind: &'static str,
    draw_counter: u64,
    frame: u64,
    viewport: u64,
    callback_ms: f64,
    queued_to_callback_ms: f64,
    shader_time_s: Option<f32>,
    initialize_ms: f64,
    draw_submission_ms: f64,
    upload_submission_ms: f64,
    instances: usize,
    uploaded: bool,
}

struct Ledger {
    epoch: Instant,
    samples: VecDeque<Sample>,
    sample_count: u64,
    dropped: u64,
}

#[derive(Default)]
pub(super) struct Recorder(Option<Ledger>);

fn milliseconds(later: Instant, earlier: Instant) -> f64 {
    later.saturating_duration_since(earlier).as_secs_f64() * 1_000.0
}

impl Recorder {
    fn new(enabled: bool) -> Self {
        if enabled {
            Self::at(Instant::now())
        } else {
            Self::default()
        }
    }

    fn at(epoch: Instant) -> Self {
        Self(Some(Ledger {
            epoch,
            samples: VecDeque::with_capacity(CAPACITY),
            sample_count: 0,
            dropped: 0,
        }))
    }

    #[cfg(all(feature = "desktop-ui-tests", target_os = "macos"))]
    pub(super) fn from_environment() -> Self {
        // Read exactly once when creating this renderer, never in a paint pass.
        Self::new(std::env::var_os("TIPTOPTYP_UI_TEST_FX_TIMING").is_some_and(|v| v == "1"))
    }

    pub(super) fn enabled(&self) -> bool {
        self.0.is_some()
    }

    pub(super) fn record_warm(
        &mut self,
        queued: Queued,
        callback: Instant,
        initialized: Instant,
        draw_counter: u64,
    ) {
        let Some(ledger) = &mut self.0 else {
            return;
        };
        ledger.push(Sample {
            sequence: 0,
            kind: "warm",
            draw_counter,
            frame: queued.frame,
            viewport: queued.viewport,
            callback_ms: milliseconds(callback, ledger.epoch),
            queued_to_callback_ms: milliseconds(callback, queued.at),
            shader_time_s: None,
            initialize_ms: milliseconds(initialized, callback),
            draw_submission_ms: 0.0,
            upload_submission_ms: 0.0,
            instances: 0,
            uploaded: false,
        });
    }

    pub(super) fn record_draw(&mut self, draw: Draw) {
        let Some(ledger) = &mut self.0 else {
            return;
        };
        ledger.push(Sample {
            sequence: 0,
            kind: "draw",
            draw_counter: draw.draw_counter,
            frame: draw.queued.frame,
            viewport: draw.queued.viewport,
            callback_ms: milliseconds(draw.callback, ledger.epoch),
            queued_to_callback_ms: milliseconds(draw.callback, draw.queued.at),
            shader_time_s: Some(draw.shader_time_s),
            initialize_ms: milliseconds(draw.initialized, draw.callback),
            draw_submission_ms: draw.draw_submission_ms,
            upload_submission_ms: draw.upload_submission_ms,
            instances: draw.instances,
            uploaded: draw.uploaded,
        });
    }

    pub(super) fn inspected(&self) -> Option<serde_json::Value> {
        let ledger = self.0.as_ref()?;
        // JSON allocation/serialization happens only when explicitly inspected.
        Some(serde_json::json!({
            "schema": 1,
            "capacity": CAPACITY,
            "sample_count": ledger.sample_count,
            "dropped": ledger.dropped,
            "now_ms": milliseconds(Instant::now(), ledger.epoch),
            "samples": ledger.samples,
        }))
    }
}

impl Ledger {
    fn push(&mut self, mut sample: Sample) {
        self.sample_count = self.sample_count.saturating_add(1);
        sample.sequence = self.sample_count;
        if self.samples.len() == CAPACITY {
            self.samples.pop_front();
            self.dropped = self.dropped.saturating_add(1);
        }
        self.samples.push_back(sample);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn draw(epoch: Instant, index: u64) -> Draw {
        Draw {
            queued: Queued {
                at: epoch + Duration::from_millis(index * 10),
                frame: index,
                viewport: 7,
            },
            callback: epoch + Duration::from_millis(index * 10 + 2),
            initialized: epoch + Duration::from_millis(index * 10 + 3),
            draw_counter: index,
            shader_time_s: index as f32 / 100.0,
            draw_submission_ms: 0.25,
            upload_submission_ms: DrawSubmission { upload_ms: 0.1 }.upload_ms,
            instances: 48,
            uploaded: index == 1,
        }
    }

    #[test]
    fn disabled_recorder_retains_nothing_and_omits_inspection() {
        let epoch = Instant::now();
        let mut recorder = Recorder::new(false);
        assert!(!recorder.enabled());
        for index in 1..=1_000 {
            let draw = draw(epoch, index);
            recorder.record_warm(draw.queued, draw.callback, draw.initialized, 0);
            recorder.record_draw(draw);
        }
        assert!(recorder.0.is_none());
        assert!(recorder.inspected().is_none());
    }

    #[test]
    fn recorder_bounds_retention_without_growing_and_reports_dropped_samples() {
        let epoch = Instant::now();
        let mut recorder = Recorder::at(epoch);
        assert!(recorder.enabled());
        let capacity = recorder.0.as_ref().unwrap().samples.capacity();
        for index in 1..=300 {
            recorder.record_draw(draw(epoch, index));
        }
        let ledger = recorder.0.as_ref().unwrap();
        assert_eq!(ledger.samples.len(), CAPACITY);
        assert_eq!(ledger.samples.capacity(), capacity);
        assert_eq!(ledger.sample_count, 300);
        assert_eq!(ledger.dropped, 44);
        assert_eq!(ledger.samples.front().unwrap().sequence, 45);
        assert_eq!(ledger.samples.back().unwrap().sequence, 300);
        let inspected = recorder.inspected().unwrap();
        assert_eq!(inspected["schema"], 1);
        assert_eq!(inspected["capacity"], 256);
        assert_eq!(inspected["sample_count"], 300);
        assert_eq!(inspected["dropped"], 44);
        assert_eq!(inspected["samples"].as_array().unwrap().len(), CAPACITY);
    }

    #[test]
    fn warm_and_draw_capture_distinct_submission_clocks_and_no_gpu_completion_claim() {
        let epoch = Instant::now();
        let mut recorder = Recorder::at(epoch);
        let first = draw(epoch, 1);
        recorder.record_warm(first.queued, first.callback, first.initialized, 0);
        recorder.record_draw(first);
        let inspected = recorder.inspected().unwrap();
        let warm = &inspected["samples"][0];
        assert_eq!(warm["kind"], "warm");
        assert_eq!(warm["draw_counter"], 0);
        assert!(warm["shader_time_s"].is_null());
        assert_eq!(warm["instances"], 0);
        assert_eq!(warm["uploaded"], false);
        let draw = &inspected["samples"][1];
        assert_eq!(draw["sequence"], 2);
        assert_eq!(draw["kind"], "draw");
        assert_eq!(draw["draw_counter"], 1);
        assert_eq!(draw["frame"], 1);
        assert_eq!(draw["viewport"], 7);
        assert_eq!(draw["callback_ms"], 12.0);
        assert_eq!(draw["queued_to_callback_ms"], 2.0);
        assert_eq!(draw["initialize_ms"], 1.0);
        assert_eq!(draw["draw_submission_ms"], 0.25);
        assert_eq!(draw["upload_submission_ms"], 0.1);
        assert_eq!(draw["instances"], 48);
        assert_eq!(draw["uploaded"], true);
        assert!(draw.get("gpu_completed_ms").is_none());
        assert!(draw.get("presented_ms").is_none());
    }
}
