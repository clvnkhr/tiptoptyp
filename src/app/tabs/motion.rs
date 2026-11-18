//! Content-coordinate tab motion. Scrolling changes the strip's origin, not
//! these positions. Drop slots stay fixed; controls follow the visible tabs.
const DURATION: f64 = 0.14;

/// Foreground ownership belongs only to pointer dragging and its landing. A
/// settled tab has no obsolete animation clock, and a neighbor slide is never
/// mistaken for a grabbed tab.
#[derive(Clone, Copy)]
pub(super) enum Motion {
    Settled {
        x: f32,
    },
    Sliding {
        x: f32,
        from: f32,
        target: f32,
        started: f64,
    },
    Grabbed {
        x: f32,
        target: f32,
    },
    Landing {
        x: f32,
        from: f32,
        target: f32,
        started: f64,
    },
}
impl Motion {
    pub(super) fn new(x: f32) -> Self {
        Self::Settled { x }
    }

    pub(super) fn update(&mut self, target: f32, grabbed: Option<f32>, now: f64) -> f32 {
        if let Some(x) = grabbed {
            *self = Self::Grabbed { x, target };
            return x;
        }
        let (x, from, previous_target, started, lifted, was_grabbed) = match *self {
            Self::Settled { x } => (x, x, x, 0.0, false, false),
            Self::Sliding {
                x,
                from,
                target,
                started,
            } => (x, from, target, started, false, false),
            Self::Grabbed { x, target } => (x, x, target, now, true, true),
            Self::Landing {
                x,
                from,
                target,
                started,
            } => (x, from, target, started, true, false),
        };
        let (from, started) = if was_grabbed || target != previous_target {
            (x, now)
        } else {
            (from, started)
        };
        let t = ((now - started) / DURATION).clamp(0.0, 1.0) as f32;
        let x = if t >= 1.0 {
            target
        } else {
            from + (target - from) * (1.0 - (1.0 - t).powi(3))
        };
        *self = if x == target {
            Self::Settled { x }
        } else if lifted {
            Self::Landing {
                x,
                from,
                target,
                started,
            }
        } else {
            Self::Sliding {
                x,
                from,
                target,
                started,
            }
        };
        x
    }

    pub(super) fn lifted(&self) -> bool {
        matches!(self, Self::Grabbed { .. } | Self::Landing { .. })
    }
    pub(super) fn moving(&self) -> bool {
        matches!(self, Self::Sliding { .. } | Self::Landing { .. })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retargeted_landings_keep_foreground_ownership_until_the_final_slot_settles() {
        let mut motion = Motion::new(0.0);
        assert!(matches!(motion, Motion::Settled { .. }));
        motion.update(120.0, Some(80.0), 1.0);
        assert!(matches!(motion, Motion::Grabbed { .. }));
        motion.update(120.0, None, 1.02);
        assert!(matches!(motion, Motion::Landing { .. }));
        let current = motion.update(120.0, None, 1.05);
        assert_eq!(motion.update(240.0, None, 1.05), current);
        assert!(motion.lifted());
        assert!(motion.moving());
        assert_eq!(motion.update(240.0, None, 1.20), 240.0);
        assert!(matches!(motion, Motion::Settled { .. }));
        assert!(!motion.lifted());
        assert!(!motion.moving());
        motion.update(0.0, None, 1.21);
        assert!(matches!(motion, Motion::Sliding { .. }));
        assert!(!motion.lifted());
    }
    #[test]
    fn neighbors_slide_without_overshoot_then_stop_requesting_frames() {
        let mut motion = Motion::new(100.0);
        assert_eq!(motion.update(0.0, None, 1.0), 100.0);
        let middle = motion.update(0.0, None, 1.07);
        assert!(middle > 0.0 && middle < 100.0);
        assert!(motion.moving());
        assert_eq!(motion.update(0.0, None, 1.15), 0.0);
        assert!(!motion.moving());
        assert_eq!(motion.update(0.0, None, 20.0), 0.0);
        assert!(!motion.moving());
    }
    #[test]
    fn grabbed_tab_tracks_pointer_immediately_and_settles_after_release() {
        let mut motion = Motion::new(0.0);
        assert_eq!(motion.update(150.0, Some(120.0), 1.0), 120.0);
        assert_eq!(motion.update(150.0, Some(135.0), 1.02), 135.0);
        assert!(
            !motion.moving(),
            "pointer events, not a repaint loop, drive dragging"
        );
        assert_eq!(motion.update(150.0, None, 1.03), 135.0);
        assert!(motion.moving());
        assert!(
            motion.lifted(),
            "landing retains its opaque foreground until settled"
        );
        assert_eq!(motion.update(150.0, None, 1.18), 150.0);
        assert!(!motion.lifted());
        assert!(!motion.moving());
    }
    #[test]
    fn rapid_retargets_are_continuous_and_new_tabs_start_in_their_slot() {
        let mut motion = Motion::new(80.0);
        assert_eq!(motion.update(80.0, None, 5.0), 80.0);
        motion.update(0.0, None, 5.0);
        let x = motion.update(0.0, None, 5.04);
        assert_eq!(motion.update(160.0, None, 5.04), x);
        assert_eq!(motion.update(160.0, None, 5.2), 160.0);
    }
}
