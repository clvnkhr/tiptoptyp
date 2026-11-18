//! Bounded service recovery. The caller supplies both generation and time.
use std::{ops::Add, time::Duration};

const FAILURE_LIMIT: u8 = 5;
pub const RETRY_DELAY: Duration = Duration::from_secs(1);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Failure<T> {
    Ignored,
    Waiting { deadline: T, failures: u8 },
    Exhausted,
    Permanent,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum Phase<G, T> {
    #[default]
    Offline,
    Running(G),
    Waiting(T),
    Exhausted,
    Permanent,
}

/// One failure per attempt, four delayed retries, then fallback on failure five.
pub struct Recovery<G, T> {
    failures: u8,
    phase: Phase<G, T>,
}

impl<G, T> Default for Recovery<G, T> {
    fn default() -> Self {
        Self {
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
        // Exhaustion is terminal until an explicit restart resets the budget.
        // An extra service-start notification cannot bypass fallback, create
        // unbounded retries, or eventually overflow the failure counter.
        if matches!(self.phase, Phase::Exhausted | Phase::Permanent) {
            return;
        }
        self.phase = Phase::Running(generation);
    }

    pub fn accepts(&self, generation: G) -> bool {
        matches!(self.phase, Phase::Running(active) if active == generation)
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

    /// Invalid configuration cannot recover unchanged. Stop immediately without
    /// spending a transient retry or allowing a later start event to reopen it.
    pub fn failed_permanently(&mut self, generation: G) -> Failure<T> {
        if !self.accepts(generation) {
            return Failure::Ignored;
        }
        self.phase = Phase::Permanent;
        Failure::Permanent
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
    fn permanent_failures_do_not_spend_or_schedule_transient_retries() {
        let mut recovery = Recovery::<u64, Duration>::default();
        recovery.started(1);
        assert_eq!(recovery.failed_permanently(0), Failure::Ignored);
        assert!(recovery.accepts(1));
        assert_eq!(recovery.failed_permanently(1), Failure::Permanent);
        assert_eq!(recovery.failures, 0);
        assert_eq!(recovery.deadline(), None);
        for generation in 1..=10 {
            recovery.started(generation);
            assert!(!recovery.accepts(generation));
            assert_eq!(
                recovery.failed(generation, Duration::ZERO),
                Failure::Ignored
            );
            assert_eq!(recovery.failed_permanently(generation), Failure::Ignored);
            assert!(!recovery.recovered(generation));
            assert!(!recovery.take_retry(Duration::from_secs(60)));
        }
        recovery.reset();
        recovery.started(11);
        assert!(recovery.accepts(11));
        assert!(recovery.recovered(11));
        assert!(matches!(
            recovery.failed(11, Duration::ZERO),
            Failure::Waiting { failures: 1, .. }
        ));
    }

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

    #[test]
    fn exhausted_budget_cannot_be_reopened_without_an_explicit_reset() {
        let mut recovery = Recovery::default();
        for generation in 1..=5 {
            recovery.started(generation);
            recovery.failed(generation, Duration::ZERO);
            if generation < 5 {
                assert!(recovery.take_retry(RETRY_DELAY));
            }
        }
        for generation in 6..=300 {
            recovery.started(generation);
            assert!(!recovery.accepts(generation));
            assert_eq!(recovery.failed(generation, RETRY_DELAY), Failure::Ignored);
            assert_eq!(recovery.deadline(), None);
        }
        recovery.reset();
        recovery.started(301);
        assert!(recovery.accepts(301));
        assert!(matches!(
            recovery.failed(301, RETRY_DELAY),
            Failure::Waiting { failures: 1, .. }
        ));
    }
}
