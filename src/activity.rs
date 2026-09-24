//! Small, read-only snapshots of work owned by each subsystem. No polling timer.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) enum Activity {
    #[default]
    Idle,
    Pending(&'static str),
    Running,
    Failed(String),
    Inactive(&'static str),
}
impl Activity {
    pub(crate) fn work(running: bool, pending: bool, error: Option<&str>) -> Self {
        if running {
            Self::Running
        } else if let Some(error) = error {
            Self::Failed(error.into())
        } else if pending {
            Self::Pending("Queued")
        } else {
            Self::Idle
        }
    }
    pub(crate) fn label(&self) -> &str {
        match self {
            Self::Idle => "Up to date / idle",
            Self::Pending(reason) | Self::Inactive(reason) => reason,
            Self::Running => "Working",
            Self::Failed(_) => "Failed",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn state_priority_preserves_failure_until_a_retry_starts() {
        assert_eq!(Activity::work(false, false, None), Activity::Idle);
        assert_eq!(
            Activity::work(false, true, None),
            Activity::Pending("Queued")
        );
        assert_eq!(
            Activity::work(false, false, Some("failed")),
            Activity::Failed("failed".into())
        );
        assert_eq!(
            Activity::work(true, true, Some("failed")),
            Activity::Running
        );
    }
}
