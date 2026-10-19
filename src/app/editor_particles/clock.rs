//! Particle, feedback and GPU draws share a monotonic clock sampled after editing.
use std::time::Instant;

use eframe::egui::{Context, Id};

#[derive(Clone, Copy)]
pub(super) enum Clock {
    Live(Instant),
    #[cfg(test)]
    Manual,
}

impl Clock {
    pub(super) fn for_context(context: &Context) -> Self {
        context.data_mut(|data| {
            let id = Id::new("editor-effects-clock");
            if let Some(clock) = data.get_temp::<Self>(id) {
                clock
            } else {
                let clock = Self::Live(Instant::now());
                data.insert_temp(id, clock);
                clock
            }
        })
    }

    pub(super) fn now(self, _context: &Context) -> f64 {
        #[cfg(test)]
        if matches!(self, Self::Manual) {
            return _context.input(|input| input.time);
        }
        self.sample(0.0, Instant::now())
    }

    fn sample(self, _logical_time: f64, at: Instant) -> f64 {
        match self {
            Self::Live(epoch) => at.saturating_duration_since(epoch).as_secs_f64(),
            #[cfg(test)]
            Self::Manual => _logical_time,
        }
    }

    pub(super) fn gpu_time(self, _now: f64, base_time: f64) -> super::gpu::FrameTime {
        match self {
            Self::Live(epoch) => super::gpu::FrameTime::Live { epoch, base_time },
            #[cfg(test)]
            Self::Manual => super::gpu::FrameTime::Fixed((_now - base_time) as f32),
        }
    }

    #[cfg(test)]
    pub(in crate::app) fn use_manual(context: &Context) {
        context.data_mut(|data| data.insert_temp(Id::new("editor-effects-clock"), Self::Manual));
    }
}

impl Default for Clock {
    fn default() -> Self {
        Self::Live(Instant::now())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn editing_work_does_not_compress_the_start_of_a_burst() {
        let epoch = Instant::now();
        let clock = Clock::Live(epoch);
        // Layout takes 14ms in the edit frame. The next callback comes 6ms
        // later even though egui's frame-start clock advances by 20ms.
        let birth = clock.sample(1.0, epoch + Duration::from_millis(14));
        let second = clock.sample(1.020, epoch + Duration::from_millis(20));
        assert!((second - birth - 0.006).abs() < 1e-9);
        let effects = crate::settings::ActionEffectSettings {
            flash: true,
            glow: true,
            shake: true,
            particles: None,
        };
        let pulse = super::super::feedback::Pulse::new(Default::default(), effects, birth, 1);
        assert!(pulse.alive(second));
        let expired = clock.sample(1.020, epoch + Duration::from_secs(2));
        assert!(
            !pulse.alive(expired),
            "feedback expires even without more input events"
        );
    }

    #[test]
    fn manual_clock_is_an_explicit_deterministic_fixture_dependency() {
        let context = Context::default();
        Clock::use_manual(&context);
        assert!(matches!(Clock::for_context(&context), Clock::Manual));
        let at = Instant::now();
        assert_eq!(Clock::Manual.sample(12.5, at), 12.5);
        assert_eq!(Clock::Manual.sample(13.0, at), 13.0);
    }
}
