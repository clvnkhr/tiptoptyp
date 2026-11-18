//! Bounded event emission; all motion and cooling are evaluated by the GPU.
use super::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Secondary {
    Smoke,
    Embers,
    Afterburner,
}

impl Secondary {
    pub(super) fn lifetime(self) -> f64 {
        match self {
            Self::Smoke => SMOKE_LIFETIME,
            Self::Embers => EMBER_LIFETIME,
            Self::Afterburner => AFTERBURNER_LIFETIME,
        }
    }

    pub(super) fn code(self) -> u8 {
        match self {
            Self::Smoke => 5,
            Self::Embers => 6,
            Self::Afterburner => 7,
        }
    }

    #[cfg(any(test, all(feature = "desktop-ui-tests", target_os = "macos")))]
    pub(super) fn label(self) -> &'static str {
        match self {
            Self::Smoke => "Smoke",
            Self::Embers => "Embers",
            Self::Afterburner => "Afterburner",
        }
    }
}

impl From<LingeringStyle> for Secondary {
    fn from(style: LingeringStyle) -> Self {
        match style {
            LingeringStyle::Smoke => Self::Smoke,
            LingeringStyle::Embers => Self::Embers,
        }
    }
}

#[derive(Clone, Copy)]
pub(super) struct CursorSample {
    pub(super) index: usize,
    pub(super) point: Option<Pos2>,
    pub(super) galley_pos: Pos2,
    pub(super) viewport: Rect,
    pub(super) frame: u64,
}

impl State {
    pub(super) fn linger(
        &mut self,
        style: LingeringStyle,
        origin: Pos2,
        density: ParticleDensity,
        frame: u64,
        now: f64,
    ) {
        if self.lingering_frame == Some(frame) || !now.is_finite() {
            return;
        }
        let count = density_count(density) / 4;
        let primary_emitted = self.emitted_frame == Some(frame);
        self.secondary_particles(style.into(), origin, Vec2::ZERO, count, frame, now);
        self.lingering_frame = Some(frame);
        if !primary_emitted {
            self.bursts = self.bursts.wrapping_add(1);
        }
    }

    /// Scroll/reflow/focus changes establish a baseline, rather than creating a
    /// jet across unrelated screen coordinates. Duplicate layout passes cannot
    /// emit twice. Keep only one scalar cursor sample when the pool is idle.
    pub(super) fn move_cursor(
        &mut self,
        sample: Option<CursorSample>,
        density: ParticleDensity,
        now: f64,
    ) {
        let previous = self.cursor;
        self.cursor = sample.filter(|_| now.is_finite());
        let Some((previous, current)) = previous.zip(self.cursor) else {
            return;
        };
        if previous.index == current.index
            || current.frame != previous.frame.saturating_add(1)
            || previous.galley_pos != current.galley_pos
            || previous.viewport != current.viewport
        {
            return;
        }
        let Some((from, to)) = previous.point.zip(current.point) else {
            return;
        };
        let delta = to - from;
        if !delta.is_finite() || !(0.5..=180.0).contains(&delta.length()) {
            return;
        }
        self.secondary_particles(
            Secondary::Afterburner,
            to,
            delta,
            density_count(density) / 4,
            current.frame,
            now,
        );
        self.cursor_bursts = self.cursor_bursts.wrapping_add(1);
    }

    fn secondary_particles(
        &mut self,
        kind: Secondary,
        origin: Pos2,
        motion: Vec2,
        count: usize,
        frame: u64,
        now: f64,
    ) {
        if !origin.is_finite() || !now.is_finite() {
            return;
        }
        if self.particles.is_empty() {
            self.base_time = now;
        }
        let count = count.min(24);
        let excess = (self.particles.len() + count).saturating_sub(LIMIT);
        self.particles.drain(..excess);
        if self.particles.capacity() == 0 {
            self.particles.reserve_exact(LIMIT);
        }
        for index in 0..count {
            let spread = self.random() - 0.5;
            let energy = self.random();
            let size = self.random();
            let seed = self.random();
            let (point, velocity, radius) = match kind {
                Secondary::Smoke => (
                    origin,
                    egui::vec2(spread * 22.0, -12.0 - energy * 25.0),
                    2.0 + size * 3.0,
                ),
                Secondary::Embers => (
                    origin,
                    egui::vec2(spread * 95.0, -24.0 - energy * 80.0),
                    0.6 + size * 0.8,
                ),
                Secondary::Afterburner => {
                    let axis = -motion.normalized();
                    let across = egui::vec2(-axis.y, axis.x);
                    let point = origin - motion * (index as f32 / count.max(1) as f32);
                    (
                        point,
                        axis * (55.0 + energy * 110.0) + across * spread * 35.0,
                        0.8 + size * 1.4,
                    )
                }
            };
            self.particles.push(Particle {
                origin: point,
                velocity,
                radius,
                color_seed: seed,
                flash: false,
                style: self.style,
                secondary: Some(kind),
                born: now,
                frame,
            });
        }
        self.gpu_instances = None;
        self.emitted_frame = Some(frame);
        self.emission_origin = Some(origin);
    }
}

#[cfg(test)]
mod tests {
    use super::super::tests::identity;
    use super::*;

    #[test]
    fn old_smoke_outlives_new_sparks_and_reuses_its_upload_until_expiry() {
        let mut state = State::default();
        let key = identity(1, 0, View::Source);
        state.update(key, 0, 0.0, None, 0);
        state.linger(
            LingeringStyle::Smoke,
            Pos2::ZERO,
            ParticleDensity::High,
            0,
            0.0,
        );
        state.update(key, 1, 0.1, Some(Pos2::ZERO), 10);
        state.update(key, 2, 0.9, None, 0);
        assert_eq!(state.particles.len(), 20);
        assert!(
            state
                .particles
                .iter()
                .all(|p| p.secondary == Some(Secondary::Smoke))
        );
        let upload = state.instances();
        state.update(key, 3, 1.5, None, 0);
        assert!(Arc::ptr_eq(&upload, &state.instances()));
        state.update(key, 4, SMOKE_LIFETIME, None, 0);
        assert!(state.particles.is_empty());
    }

    fn sample(index: usize, x: f32, frame: u64) -> CursorSample {
        CursorSample {
            index,
            point: Some(egui::pos2(x, 50.0)),
            galley_pos: Pos2::ZERO,
            viewport: Rect::from_min_max(Pos2::ZERO, egui::pos2(300.0, 200.0)),
            frame,
        }
    }

    #[test]
    fn cursor_jets_require_motion_and_skip_scroll_reflow_teleports_and_duplicate_passes() {
        let mut state = State::default();
        state.move_cursor(Some(sample(0, 10.0, 1)), ParticleDensity::High, 0.0);
        state.move_cursor(Some(sample(1, 20.0, 2)), ParticleDensity::High, 0.01);
        assert_eq!(state.cursor_bursts, 1);
        assert!(
            state
                .particles
                .iter()
                .all(|p| p.velocity.x < 0.0 && p.secondary == Some(Secondary::Afterburner))
        );
        state.move_cursor(Some(sample(1, 20.0, 2)), ParticleDensity::High, 0.01);
        let mut scrolled = sample(2, 30.0, 3);
        scrolled.galley_pos.y = -30.0;
        state.move_cursor(Some(scrolled), ParticleDensity::High, 0.02);
        state.move_cursor(Some(sample(3, 290.0, 4)), ParticleDensity::High, 0.03);
        state.move_cursor(Some(sample(4, 10.0, 5)), ParticleDensity::High, 0.04);
        state.move_cursor(None, ParticleDensity::High, 0.05);
        state.move_cursor(Some(sample(5, 20.0, 6)), ParticleDensity::High, 0.06);
        assert_eq!(state.cursor_bursts, 1);
        assert!(state.particles.len() <= LIMIT);
    }

    #[test]
    fn secondary_emission_is_bounded_independent_and_realigns_without_collapsing_a_trail() {
        let key = identity(1, 0, View::Source);
        let mut state = State::default();
        for frame in 0..200 {
            let now = frame as f64 * 0.01;
            state.update(key, frame, now, Some(Pos2::ZERO), 80);
            state.linger(
                LingeringStyle::Smoke,
                Pos2::ZERO,
                ParticleDensity::High,
                frame,
                now,
            );
            state.linger(
                LingeringStyle::Smoke,
                Pos2::ZERO,
                ParticleDensity::High,
                frame,
                now,
            );
            assert_eq!(state.bursts, frame + 1);
            assert!(state.particles.len() <= LIMIT);
        }
        state.update(key, 200, 2.0, None, 0);
        state.move_cursor(Some(sample(0, 10.0, 200)), ParticleDensity::High, 2.0);
        state.move_cursor(Some(sample(1, 20.0, 201)), ParticleDensity::High, 2.01);
        let before: Vec<_> = state
            .particles
            .iter()
            .filter(|p| p.frame == 201)
            .map(|p| p.origin)
            .collect();
        state.update(key, 201, 2.01, Some(egui::pos2(30.0, 60.0)), 0);
        let after: Vec<_> = state
            .particles
            .iter()
            .filter(|p| p.frame == 201)
            .map(|p| p.origin)
            .collect();
        assert_eq!(before.len(), after.len());
        for (a, b) in before.iter().zip(after) {
            assert_eq!(b - *a, egui::vec2(10.0, 10.0));
        }
    }
}
