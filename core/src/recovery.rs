//! Bounded service recovery. The caller supplies both generation and time.
use std::{ops::Add, time::Duration};

const FAILURE_LIMIT: u8 = 5;
pub const RETRY_DELAY: Duration = Duration::from_secs(1);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Failure<T> {
    Ignored,
    Waiting { deadline: T, failures: u8 },
    Exhausted,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum Phase<T> {
    #[default]
    Offline,
    Running,
    Waiting(T),
    Exhausted,
}

/// One failure per attempt, four delayed retries, then fallback on failure five.
pub struct Recovery<G, T> {
    generation: Option<G>,
    failures: u8,
    phase: Phase<T>,
}

impl<G, T> Default for Recovery<G, T> {
    fn default() -> Self {
        Self {
            generation: None,
            failures: 0,
            phase: Phase::Offline,
        }
    }
}

impl<G: Copy + Eq, T: Copy + Ord + Add<Duration, Output = T>> Recovery<G, T> {
    /// A user-requested restart or changed document begins a fresh budget.
    pub fn reset(&mut self) {
        *self = Self::default();
    }

    /// Starting a retry preserves its failure budget.
    pub fn started(&mut self, generation: G) {
        self.generation = Some(generation);
        self.phase = Phase::Running;
    }

    pub fn accepts(&self, generation: G) -> bool {
        self.generation == Some(generation) && self.phase == Phase::Running
    }

    pub fn recovered(&mut self, generation: G) -> bool {
        if !self.accepts(generation) {
            return false;
        }
        self.failures = 0;
        true
    }

    pub fn failed(&mut self, generation: G, now: T) -> Failure<T> {
        if !self.accepts(generation) {
            return Failure::Ignored;
        }
        self.failures += 1;
        if self.failures == FAILURE_LIMIT {
            self.phase = Phase::Exhausted;
            Failure::Exhausted
        } else {
            let deadline = now + RETRY_DELAY;
            self.phase = Phase::Waiting(deadline);
            Failure::Waiting {
                deadline,
                failures: self.failures,
            }
        }
    }

    pub fn deadline(&self) -> Option<T> {
        match self.phase {
            Phase::Waiting(deadline) => Some(deadline),
            _ => None,
        }
    }

    /// Consumes the scheduled restart once, even if the caller ticks repeatedly.
    pub fn take_retry(&mut self, now: T) -> bool {
        if self.deadline().is_some_and(|deadline| now >= deadline) {
            self.phase = Phase::Offline;
            self.generation = None;
            true
        } else {
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn five_failed_attempts_have_four_delayed_restarts_then_exhaust() {
        let mut recovery = Recovery::default();
        let mut now = Duration::ZERO;
        for generation in 1..=5 {
            recovery.started(generation);
            let outcome = recovery.failed(generation, now);
            assert_eq!(recovery.failed(generation, now), Failure::Ignored);
            assert!(!recovery.recovered(generation));
            if generation == 5 {
                assert_eq!(outcome, Failure::Exhausted);
                assert_eq!(recovery.deadline(), None);
                assert!(!recovery.take_retry(now + Duration::from_secs(100)));
            } else {
                let deadline = now + RETRY_DELAY;
                assert_eq!(
                    outcome,
                    Failure::Waiting {
                        deadline,
                        failures: generation
                    }
                );
                assert!(!recovery.take_retry(deadline - Duration::from_nanos(1)));
                assert!(recovery.take_retry(deadline));
                assert!(!recovery.take_retry(deadline));
                now = deadline;
            }
        }
    }

    #[test]
    fn recovery_and_explicit_restart_reset_budget_and_reject_old_events() {
        let mut recovery = Recovery::default();
        recovery.started(1);
        recovery.failed(1, Duration::ZERO);
        assert!(recovery.take_retry(RETRY_DELAY));
        recovery.started(2);
        assert_eq!(recovery.failed(1, RETRY_DELAY), Failure::Ignored);
        assert!(!recovery.recovered(1));
        assert!(recovery.recovered(2));
        assert!(matches!(
            recovery.failed(2, RETRY_DELAY),
            Failure::Waiting { failures: 1, .. }
        ));
        recovery.reset();
        assert!(!recovery.take_retry(RETRY_DELAY * 10));
        recovery.started(3);
        assert!(matches!(
            recovery.failed(3, RETRY_DELAY),
            Failure::Waiting { failures: 1, .. }
        ));
    }
}
