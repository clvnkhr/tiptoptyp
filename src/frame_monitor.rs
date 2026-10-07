//! Bounded per-document frame telemetry, independent of the profiling recorder.
//!
//! Continuous redraws can be legitimate (terminal output or a long build), so
//! the notice describes observed activity without diagnosing a stuck app. Human
//! input resets the no-input interval. The monitor neither requests repaints nor
//! retains input/document contents; ordinary passes only update a few counters.

use std::time::{Duration, Instant};

use eframe::egui;

const REDRAW_NOTICE_AFTER: Duration = Duration::from_secs(60);
const BURST_GAP: Duration = Duration::from_secs(1);
const SLOW_FRAME_MS: f64 = 50.0;
const DEV_STATS_SECS: u64 = 5;

pub(crate) fn dev_mode() -> bool {
    std::env::var_os("TIPTOPTYP_DEV").is_some_and(|value| !value.is_empty() && value != "0")
}

pub(crate) struct FrameMonitor {
    dev: bool,
    burst_frames: u64,
    burst_start: Option<Instant>,
    last_frame: Option<Instant>,
    idle_start: Option<Instant>,
    idle_frames: u64,
    warned: bool,
    redraw_notice: Option<String>,
    dev_stats_second: u64,
    #[cfg(test)]
    observed_frames: u64,
}

impl FrameMonitor {
    pub(crate) fn new() -> Self {
        Self {
            dev: dev_mode(),
            burst_frames: 0,
            burst_start: None,
            last_frame: None,
            idle_start: None,
            idle_frames: 0,
            warned: false,
            redraw_notice: None,
            dev_stats_second: 0,
            #[cfg(test)]
            observed_frames: 0,
        }
    }

    /// Observe input before widgets consume it, including a held drag/key even
    /// when the platform did not send another event on this pass.
    pub(crate) fn begin_frame(&mut self, context: &egui::Context) -> Instant {
        let active_input = context.input(|input| {
            !input.raw.events.is_empty() || input.pointer.any_down() || !input.keys_down.is_empty()
        });
        self.begin_frame_at(Instant::now(), active_input)
    }

    fn begin_frame_at(&mut self, now: Instant, active_input: bool) -> Instant {
        let settled = self
            .last_frame
            .is_none_or(|previous| now.saturating_duration_since(previous) >= BURST_GAP);
        if settled {
            self.burst_frames = 0;
            self.burst_start = Some(now);
            self.dev_stats_second = 0;
        }
        if settled || active_input {
            self.idle_start = None;
            self.idle_frames = 0;
            self.warned = false;
            self.redraw_notice = None;
        }
        if !active_input {
            self.idle_start.get_or_insert(now);
            self.idle_frames += 1;
        }
        self.burst_frames += 1;
        #[cfg(test)]
        {
            self.observed_frames += 1;
        }
        self.last_frame = Some(now);
        now
    }

    pub(crate) fn end_frame(&mut self, started: Instant) {
        let now = Instant::now();
        self.finish_frame_at(now, now.saturating_duration_since(started));
    }

    pub(crate) fn take_redraw_notice(&mut self) -> Option<String> {
        self.redraw_notice.take()
    }

    fn finish_frame_at(&mut self, now: Instant, elapsed: Duration) {
        let ms = elapsed.as_secs_f64() * 1_000.0;
        if self.dev {
            if ms >= SLOW_FRAME_MS {
                eprintln!("tiptoptyp[dev] slow frame: {ms:.1} ms");
            }
            let second = self
                .burst_start
                .map(|start| now.saturating_duration_since(start).as_secs())
                .unwrap_or(0);
            if second / DEV_STATS_SECS != self.dev_stats_second / DEV_STATS_SECS {
                self.dev_stats_second = second;
                eprintln!(
                    "tiptoptyp[dev] {} frames in {}s burst",
                    self.burst_frames, second
                );
            }
        }
        let idle = self
            .idle_start
            .map(|start| now.saturating_duration_since(start))
            .unwrap_or_default();
        if idle >= REDRAW_NOTICE_AFTER && !self.warned {
            self.warned = true;
            let frames = self.idle_frames;
            let secs = idle.as_secs();
            eprintln!(
                "tiptoptyp[warn] sustained redraw: {frames} frames in {secs}s without input; \
                 background output or animation may be expected; use a profiling build for attribution"
            );
            self.redraw_notice = Some(format!(
                "This window has redrawn continuously for {secs}s without input. Background work or terminal output may explain it; check Activity for ongoing work."
            ));
        }
    }

    #[cfg(test)]
    pub(crate) fn observed_frames(&self) -> u64 {
        self.observed_frames
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn quiet_monitor() -> FrameMonitor {
        let mut monitor = FrameMonitor::new();
        monitor.dev = false;
        monitor
    }

    fn pass(monitor: &mut FrameMonitor, now: Instant, active_input: bool) {
        monitor.begin_frame_at(now, active_input);
        monitor.finish_frame_at(now, Duration::ZERO);
    }

    #[test]
    fn new_burst_resets_development_stats_cadence() {
        let mut monitor = quiet_monitor();
        monitor.dev_stats_second = 60;
        monitor.last_frame = Some(Instant::now() - BURST_GAP * 2);
        monitor.begin_frame(&egui::Context::default());
        assert_eq!(monitor.dev_stats_second, 0);
    }

    #[test]
    fn active_typing_is_not_an_idle_repaint_storm() {
        let mut monitor = quiet_monitor();
        let context = egui::Context::default();
        let start = Instant::now() - Duration::from_secs(61);
        monitor.burst_start = Some(start);
        monitor.idle_start = Some(start);
        monitor.last_frame = Some(Instant::now());
        context
            .run_ui(
                egui::RawInput {
                    events: vec![egui::Event::Text("x".into())],
                    ..Default::default()
                },
                |ui| {
                    let started = monitor.begin_frame(ui.ctx());
                    monitor.end_frame(started);
                },
            )
            .drop_without_applying_deltas();
        assert!(monitor.take_redraw_notice().is_none());
        assert!(monitor.idle_start.is_none());
    }

    #[test]
    fn held_keys_and_drag_keep_input_active_without_new_events() {
        for event in [
            egui::Event::Key {
                key: egui::Key::ArrowDown,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: egui::Modifiers::NONE,
            },
            egui::Event::PointerButton {
                pos: egui::pos2(1.0, 1.0),
                button: egui::PointerButton::Primary,
                pressed: true,
                modifiers: egui::Modifiers::NONE,
            },
        ] {
            let context = egui::Context::default();
            let mut monitor = quiet_monitor();
            context
                .run_ui(
                    egui::RawInput {
                        events: vec![event],
                        ..Default::default()
                    },
                    |ui| {
                        monitor.begin_frame(ui.ctx());
                    },
                )
                .drop_without_applying_deltas();
            monitor.idle_start = Some(Instant::now() - REDRAW_NOTICE_AFTER);
            context
                .run_ui(egui::RawInput::default(), |ui| {
                    let started = monitor.begin_frame(ui.ctx());
                    monitor.end_frame(started);
                })
                .drop_without_applying_deltas();
            assert!(monitor.take_redraw_notice().is_none());
            assert!(monitor.idle_start.is_none());
        }
    }

    #[test]
    fn no_input_redraw_notice_is_once_per_interval_and_rearms_after_input() {
        let mut monitor = quiet_monitor();
        let start = Instant::now();
        for tick in 0..=240 {
            pass(
                &mut monitor,
                start + Duration::from_millis(tick * 250),
                false,
            );
            if tick < 240 {
                assert!(monitor.take_redraw_notice().is_none());
            }
        }
        let notice = monitor.take_redraw_notice().unwrap();
        assert!(!notice.contains("stuck"));
        assert!(!notice.contains("restart"));
        assert!(notice.contains("Background work or terminal output"));
        for tick in 241..=480 {
            pass(
                &mut monitor,
                start + Duration::from_millis(tick * 250),
                false,
            );
            assert!(monitor.take_redraw_notice().is_none());
        }
        pass(&mut monitor, start + Duration::from_secs(121), true);
        for tick in 0..=240 {
            pass(
                &mut monitor,
                start + Duration::from_secs(122) + Duration::from_millis(tick * 250),
                false,
            );
        }
        assert!(monitor.take_redraw_notice().is_some());
    }

    #[test]
    fn sparse_frames_never_trip_the_watchdog() {
        let mut monitor = quiet_monitor();
        let start = Instant::now();
        for tick in 0..=120 {
            pass(&mut monitor, start + Duration::from_secs(tick * 2), false);
            assert!(monitor.take_redraw_notice().is_none());
            assert_eq!(monitor.burst_frames, 1);
        }
    }

    #[test]
    fn continuous_input_for_minutes_never_trips_and_monitors_are_independent() {
        let start = Instant::now();
        let mut active = quiet_monitor();
        let mut background = quiet_monitor();
        for tick in 0..=480 {
            let now = start + Duration::from_millis(tick * 250);
            pass(&mut active, now, tick % 4 == 0);
            assert!(active.take_redraw_notice().is_none());
            pass(&mut background, now, false);
        }
        assert!(background.take_redraw_notice().is_some());
        assert!(!active.warned);
    }

    #[test]
    fn dev_mode_follows_the_environment() {
        assert_eq!(FrameMonitor::new().dev, dev_mode());
    }
}
