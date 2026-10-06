//! Always-on UI frame monitor: slow-frame logging in dev mode and a
//! repaint-storm watchdog, inspired by Zed's frame-duration tracking.
//!
//! egui only runs a UI pass when something requested a repaint, so a burst of
//! back-to-back passes means the app never settles. That is normal while
//! typing or scrolling, but a burst running for a minute means a stuck
//! spinner, pending state, or animation loop is burning a core. The monitor
//! costs a few counters per pass and logs nothing in the common case. Like
//! the profiling recorder, it never logs document contents.

use std::time::{Duration, Instant};

/// A burst becomes a storm after this many continuous seconds. Long enough
/// that sustained scrolling or typing cannot trip it: frames stop within a
/// second of the last input, which resets the burst.
const STORM_BURST_SECS: f64 = 60.0;
/// A gap this long ends the burst; the app settled.
const BURST_GAP: Duration = Duration::from_secs(1);
/// Slow-frame log threshold (dev mode only).
const SLOW_FRAME_MS: f64 = 50.0;
/// Dev burst stats cadence.
const DEV_STATS_SECS: u64 = 5;

pub(crate) fn dev_mode() -> bool {
    std::env::var_os("TIPTOPTYP_DEV").is_some_and(|value| !value.is_empty() && value != "0")
}

/// Pure storm decision for tests: warn once per burst that outlasts the bound.
fn storm_tripped(burst_secs: f64, warned: bool) -> bool {
    burst_secs >= STORM_BURST_SECS && !warned
}

pub(crate) struct FrameMonitor {
    dev: bool,
    burst_frames: u64,
    burst_start: Option<Instant>,
    last_frame: Option<Instant>,
    warned: bool,
    storm_notice: Option<String>,
    dev_stats_second: u64,
}

impl FrameMonitor {
    pub(crate) fn new() -> Self {
        Self {
            dev: dev_mode(),
            burst_frames: 0,
            burst_start: None,
            last_frame: None,
            warned: false,
            storm_notice: None,
            dev_stats_second: 0,
        }
    }

    /// Call at the start of each root UI pass. Returns the frame start for
    /// [`Self::end_frame`]; split so neither borrows across the pass.
    pub(crate) fn begin_frame(&mut self) -> Instant {
        let now = Instant::now();
        let settled = self
            .last_frame
            .is_none_or(|previous| now.duration_since(previous) >= BURST_GAP);
        if settled {
            self.burst_frames = 0;
            self.burst_start = Some(now);
            self.warned = false;
        }
        self.burst_frames += 1;
        self.last_frame = Some(now);
        now
    }

    /// Call at the end of each root UI pass with the elapsed time.
    pub(crate) fn end_frame(&mut self, started: Instant) {
        self.finish_frame(started.elapsed());
    }

    /// A storm warning for the shell to surface in-app, if any.
    pub(crate) fn take_storm_notice(&mut self) -> Option<String> {
        self.storm_notice.take()
    }

    fn finish_frame(&mut self, elapsed: Duration) {
        let ms = elapsed.as_secs_f64() * 1_000.0;
        if self.dev {
            if ms >= SLOW_FRAME_MS {
                eprintln!("tiptoptyp[dev] slow frame: {ms:.1} ms");
            }
            let second = self
                .burst_start
                .map(|start| start.elapsed().as_secs())
                .unwrap_or(0);
            if second / DEV_STATS_SECS != self.dev_stats_second / DEV_STATS_SECS {
                self.dev_stats_second = second;
                eprintln!(
                    "tiptoptyp[dev] {} frames in {}s burst",
                    self.burst_frames, second
                );
            }
        }
        let burst_secs = self
            .burst_start
            .map(|start| start.elapsed().as_secs_f64())
            .unwrap_or(0.0);
        if storm_tripped(burst_secs, self.warned) {
            self.warned = true;
            let frames = self.burst_frames;
            let secs = burst_secs as u64;
            eprintln!(
                "tiptoptyp[warn] repaint storm: {frames} frames in {secs}s without settling; \
                 run with TIPTOPTYP_DEV=1 and a profiling build for attribution"
            );
            self.storm_notice = Some(format!(
                "Interface has repainted continuously for {secs}s; restarting the app clears a stuck spinner or pending state"
            ));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn storm_trips_once_per_overlong_burst() {
        assert!(!storm_tripped(0.0, false));
        assert!(!storm_tripped(STORM_BURST_SECS - 0.1, false));
        assert!(storm_tripped(STORM_BURST_SECS, false));
        assert!(storm_tripped(STORM_BURST_SECS + 3600.0, false));
        // Already warned for this burst: stay quiet.
        assert!(!storm_tripped(STORM_BURST_SECS + 1.0, true));
    }

    #[test]
    fn sparse_frames_never_trip_the_watchdog() {
        let mut monitor = FrameMonitor::new();
        // Rapid successive passes stay far below the duration bound, and no
        // notice is produced without a storm.
        for _ in 0..10 {
            let started = monitor.begin_frame();
            monitor.end_frame(started);
            assert!(monitor.take_storm_notice().is_none());
        }
        assert_eq!(monitor.burst_frames, 10);
    }

    #[test]
    fn dev_mode_follows_the_environment() {
        // Cannot mutate the process environment portably here; at least the
        // constructor agrees with the accessor.
        assert_eq!(FrameMonitor::new().dev, dev_mode());
    }
}
