//! Time enters through the caller, never through a wall-clock read.
use std::{ops::Sub, time::Duration};

pub struct Debounce<T> {
    deadline: Option<T>,
}
impl<T> Default for Debounce<T> {
    fn default() -> Self {
        Self { deadline: None }
    }
}
impl<T: Copy + Ord + Sub<T, Output = Duration>> Debounce<T> {
    pub fn at(deadline: T) -> Self {
        Self {
            deadline: Some(deadline),
        }
    }
    pub fn schedule(&mut self, deadline: T) {
        self.deadline = Some(deadline);
    }
    pub fn clear(&mut self) {
        self.deadline = None;
    }
    pub fn is_pending(&self) -> bool {
        self.deadline.is_some()
    }
    pub fn remaining(&self, now: T) -> Option<Duration> {
        self.deadline.map(|deadline| {
            if deadline > now {
                deadline - now
            } else {
                Duration::ZERO
            }
        })
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn edits_extend_deadline_and_cancellation_removes_scheduled_effect() {
        let mut debounce = Debounce::at(Duration::from_millis(200));
        assert_eq!(
            debounce.remaining(Duration::from_millis(199)),
            Some(Duration::from_millis(1))
        );
        debounce.schedule(Duration::from_millis(350));
        assert_eq!(
            debounce.remaining(Duration::from_millis(200)),
            Some(Duration::from_millis(150))
        );
        assert_eq!(
            debounce.remaining(Duration::from_millis(350)),
            Some(Duration::ZERO)
        );
        debounce.clear();
        assert_eq!(debounce.remaining(Duration::from_secs(20)), None);
    }
}
