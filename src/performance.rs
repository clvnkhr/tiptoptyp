//! Opt-in, bounded wall-time summaries. Ordinary builds compile spans to nothing.
//! Native CPU sampling is separate: inclusive elapsed time is not CPU time.

#[cfg(feature = "profiling")]
mod recording;
#[cfg(feature = "profiling")]
pub(crate) use recording::{Session, span, take_no_window_request, tick};

#[cfg(not(feature = "profiling"))]
pub(crate) struct Session;
#[cfg(not(feature = "profiling"))]
impl Session {
    pub(crate) fn from_env() -> Result<Self, String> {
        if std::env::var_os("TIPTOPTYP_PROFILE_DIR").is_some() {
            Err("profiling requires a build with --features profiling".into())
        } else {
            Ok(Self)
        }
    }
    pub(crate) fn finish(self) -> Result<(), String> {
        Ok(())
    }
    pub(crate) fn persistence_path(&self) -> Option<std::path::PathBuf> {
        None
    }
}

#[cfg(not(feature = "profiling"))]
pub(crate) struct Span;
#[cfg(not(feature = "profiling"))]
#[inline(always)]
pub(crate) fn span(_name: &'static str) -> Span {
    Span
}

#[cfg(not(feature = "profiling"))]
#[inline(always)]
pub(crate) fn tick(_context: &eframe::egui::Context, _ready: impl FnOnce() -> bool) {}

#[cfg(not(feature = "profiling"))]
pub(crate) fn take_no_window_request() -> bool {
    false
}

#[cfg(test)]
mod tests {
    #[test]
    fn dormant_profiling_does_not_even_query_capture_state() {
        // Runs both with and without the feature. No session is started by tests.
        super::tick(&eframe::egui::Context::default(), || {
            panic!("dormant profiling must not acquire the capture-state lock")
        });
    }
}
