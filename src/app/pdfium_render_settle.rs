//! Keep scaling resident PDF textures while viewport geometry is changing.
use std::time::Duration;

const QUIET_SECONDS: f64 = 0.090;

/// Rounded physical viewport dimensions and physical pixels per PDF point,
/// expressed in hundredths. Include height even when fit-width scale is stable.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Geometry {
    pub size: [u32; 2],
    pub scale: u32,
}

#[derive(Default)]
enum Wait {
    #[default]
    Ready,
    Resizing,
    QuietUntil(f64),
}

/// A passive deadline: this state never starts a timer or requests a repaint.
/// `now` is the caller's finite monotonic clock, in seconds.
#[derive(Default)]
pub(super) struct SettleState {
    geometry: Option<Geometry>,
    actively_resizing: bool,
    wait: Wait,
}

impl SettleState {
    pub fn observe(&mut self, geometry: Geometry, actively_resizing: bool, now: f64) {
        let released = self.actively_resizing && !actively_resizing;
        self.actively_resizing = actively_resizing;
        self.wait = match self.geometry.replace(geometry) {
            // The initial document must not wait for a resize deadline. The
            // caller also bypasses settling when there are no reusable pixels.
            None => Wait::Ready,
            Some(_) if actively_resizing => Wait::Resizing,
            // Native resize-end and divider release are explicit completion
            // signals, including when the pointer was held still before release.
            Some(_) if released => Wait::Ready,
            Some(previous) if previous != geometry => Wait::QuietUntil(now + QUIET_SECONDS),
            // Repaints with unchanged geometry must not restart the deadline.
            Some(_) => return,
        };
    }

    pub fn deferred(&self, now: f64) -> bool {
        match self.wait {
            Wait::Ready => false,
            Wait::Resizing => true,
            Wait::QuietUntil(deadline) => now < deadline,
        }
    }

    /// One finite wakeup at the quiet deadline. Held resizes rely on their
    /// ordinary input/end events; settled and idle previews need no wakeup.
    pub fn wake_after(&self, now: f64) -> Option<Duration> {
        match self.wait {
            Wait::QuietUntil(deadline) if deadline > now => {
                Some(Duration::from_secs_f64(deadline - now))
            }
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn geometry(width: u32) -> Geometry {
        Geometry {
            size: [width, 900],
            scale: width / 4,
        }
    }

    #[test]
    fn initial_geometry_renders_immediately_without_an_idle_timer() {
        for active in [false, true] {
            let mut state = SettleState::default();
            assert!(!state.deferred(0.0));
            assert!(state.wake_after(0.0).is_none());
            state.observe(geometry(1200), active, 0.0);
            assert!(!state.deferred(0.0));
            assert!(state.wake_after(0.0).is_none());
        }
    }

    #[test]
    fn a_hundred_drag_widths_only_render_after_release() {
        let mut state = SettleState::default();
        state.observe(geometry(1200), false, 0.0);
        let mut new_renders = 0;
        for step in 1..=100 {
            let now = f64::from(step) / 1000.0;
            state.observe(geometry(1200 + step), true, now);
            new_renders += usize::from(!state.deferred(now));
            assert!(state.wake_after(now).is_none());
        }
        assert_eq!(new_renders, 0);
        state.observe(geometry(1300), false, 0.101);
        new_renders += usize::from(!state.deferred(0.101));
        assert_eq!(new_renders, 1);
        assert!(state.wake_after(0.101).is_none());
    }

    #[test]
    fn stationary_held_resize_cannot_expire_or_poll() {
        let mut state = SettleState::default();
        state.observe(geometry(1200), false, 0.0);
        // Starting a drag without moving also blocks sharp replacements.
        state.observe(geometry(1200), true, 0.001);
        for now in [0.100, 1.0, 600.0] {
            state.observe(geometry(1200), true, now);
            assert!(state.deferred(now));
            assert!(state.wake_after(now).is_none());
        }
        state.observe(geometry(1200), false, 600.001);
        assert!(!state.deferred(600.001));
        assert!(state.wake_after(600.001).is_none());
    }

    #[test]
    fn release_renders_final_geometry_without_a_second_quiet_delay() {
        let mut state = SettleState::default();
        state.observe(geometry(1200), false, 0.0);
        state.observe(geometry(1300), true, 0.010);
        state.observe(geometry(1400), false, 0.020);
        assert!(!state.deferred(0.020));
        assert!(state.wake_after(0.020).is_none());
        for now in [0.021, 1.0, 10.0] {
            state.observe(geometry(1400), false, now);
            assert!(!state.deferred(now));
            assert!(state.wake_after(now).is_none());
        }
    }

    #[test]
    fn unclassified_resize_and_pinch_wait_for_one_fixed_quiet_deadline() {
        let mut state = SettleState::default();
        let first = geometry(1200);
        state.observe(first, false, 0.0);
        let zoomed = Geometry {
            scale: 360,
            ..first
        };
        state.observe(zoomed, false, 0.010);
        assert!(state.deferred(0.010));
        let wake = state.wake_after(0.010).unwrap().as_secs_f64();
        assert!((wake - QUIET_SECONDS).abs() < 1e-9);
        state.observe(zoomed, false, 0.050);
        assert!(state.deferred(0.050));
        assert!((state.wake_after(0.050).unwrap().as_secs_f64() - 0.050).abs() < 1e-9);
        assert!(!state.deferred(0.101));
        assert!(state.wake_after(0.101).is_none());
        state.observe(zoomed, false, 10.0);
        assert!(!state.deferred(10.0));
        assert!(state.wake_after(10.0).is_none());
    }

    #[test]
    fn new_motion_restarts_only_the_quiet_deadline_and_active_drag_cancels_it() {
        let mut state = SettleState::default();
        state.observe(geometry(1200), false, 0.0);
        state.observe(geometry(1210), false, 0.010);
        state.observe(geometry(1220), false, 0.080);
        assert!(state.deferred(0.101));
        assert!(state.wake_after(0.101).is_some());
        assert!(!state.deferred(0.171));
        state.observe(geometry(1230), false, 0.200);
        state.observe(geometry(1230), true, 0.210);
        assert!(state.deferred(1.0));
        assert!(state.wake_after(1.0).is_none());
        state.observe(geometry(1230), false, 1.001);
        assert!(!state.deferred(1.001));
        assert!(state.wake_after(1.001).is_none());
    }

    #[test]
    fn physical_height_and_pixel_scale_changes_are_geometry_changes() {
        for updated in [
            Geometry {
                size: [1200, 901],
                scale: 300,
            },
            Geometry {
                size: [1200, 900],
                scale: 600,
            },
            Geometry {
                size: [2400, 1800],
                scale: 600,
            },
        ] {
            let mut state = SettleState::default();
            state.observe(geometry(1200), false, 0.0);
            state.observe(updated, false, 0.010);
            assert!(state.deferred(0.010));
            assert!(state.wake_after(0.010).is_some());
            assert!(!state.deferred(0.101));
            assert!(state.wake_after(0.101).is_none());
        }
    }
}
