//! Content-coordinate tab motion. Scrolling changes the strip's origin, not
//! these positions. Drop slots stay fixed; controls follow the visible tabs.
const DURATION: f64 = 0.14;

pub(super) struct Motion {
    x: f32,
    from: f32,
    target: f32,
    started: f64,
    grabbed: bool,
    lifted: bool,
}
impl Motion {
    pub(super) fn new(x: f32) -> Self {
        Self {
            x,
            from: x,
            target: x,
            started: 0.0,
            grabbed: false,
            lifted: false,
        }
    }
    pub(super) fn update(&mut self, target: f32, grabbed: Option<f32>, now: f64) -> f32 {
        if let Some(x) = grabbed {
            self.x = x;
            self.grabbed = true;
            self.lifted = true;
            self.target = target;
            return x;
        }
        if self.grabbed || target != self.target {
            self.from = self.x;
            self.started = now;
            self.target = target;
            self.grabbed = false;
        }
        let t = ((now - self.started) / DURATION).clamp(0.0, 1.0) as f32;
        self.x = if t >= 1.0 {
            target
        } else {
            self.from + (target - self.from) * (1.0 - (1.0 - t).powi(3))
        };
        if self.x == self.target {
            self.lifted = false;
        }
        self.x
    }
    pub(super) fn lifted(&self) -> bool {
        self.lifted
    }
    pub(super) fn moving(&self) -> bool {
        !self.grabbed && self.x != self.target
    }
}

#[cfg(test)]
mod tests {
    use super::*;
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
