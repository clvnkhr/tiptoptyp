//! Explicit maintenance work never touches the compiler's UI frame.
use super::*;
use std::sync::atomic::{AtomicBool, Ordering};

#[derive(Default)]
pub(super) struct BiberCache {
    job: LatestJob<PathBuf>,
    cancel: Arc<AtomicBool>,
    attempted: bool,
}
impl BiberCache {
    pub(super) fn activity(&self) -> crate::activity::Activity {
        if self.attempted {
            self.job.activity()
        } else {
            crate::activity::Activity::Inactive("Not requested")
        }
    }
}
impl Drop for BiberCache {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Release);
    }
}
impl EditorApp {
    pub(super) fn clear_biber_cache(&mut self, context: &egui::Context) {
        if self.biber_cache.job.is_running() {
            return;
        }
        let Some(executable) = self.tex_tools.biber.clone() else {
            self.notice = Some(Notice {
                message: "Biber was not found. Set its executable in TeX compiler options.".into(),
                kind: NoticeKind::Error,
            });
            return;
        };
        self.biber_cache.cancel = Arc::default();
        let cancel = self.biber_cache.cancel.clone();
        let root = self.project_root();
        let environment = self.settings.tex.system_command.environment.clone();
        self.biber_cache.attempted = true;
        let result =
            self.biber_cache
                .job
                .start_and_repaint("Clear Biber cache", context, move || {
                    crate::tex::tools::clear_biber_cache(&executable, &root, &environment, &cancel)
                });
        let kind = if result.is_err() {
            NoticeKind::Error
        } else {
            NoticeKind::Info
        };
        self.notice = Some(Notice {
            message: result
                .err()
                .unwrap_or_else(|| "Clearing Biber cache…".into()),
            kind,
        });
    }
    pub(super) fn poll_biber_cache(&mut self, _context: &egui::Context) {
        match self.biber_cache.job.poll() {
            LatestJobPoll::Ready(_) => {
                self.notice = Some(Notice {
                    message: "Biber cache cleared".into(),
                    kind: NoticeKind::Success,
                });
                if self.preview_document_kind() == DocumentKind::Tex
                    && self.settings.tex.build_engine != crate::tex::settings::BuildEngine::Tectonic
                {
                    self.request_compile();
                }
            }
            LatestJobPoll::Failed(error) => {
                self.notice = Some(Notice {
                    message: error,
                    kind: NoticeKind::Error,
                })
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::activity::Activity;

    #[test]
    fn biber_cache_is_not_reported_ready_before_any_request() {
        let cache = BiberCache::default();
        assert_eq!(cache.activity(), Activity::Inactive("Not requested"));
        assert!(!cache.attempted);
        assert!(!cache.job.is_running());
    }

    #[test]
    fn biber_cache_observation_preserves_failures_and_never_retires_work() {
        let mut cache = BiberCache {
            attempted: true,
            job: Default::default(),
            cancel: Arc::default(),
        };
        cache
            .job
            .start("test-biber-failure", || Err("cache is locked".into()))
            .unwrap();
        cache.job.wait_until_ready_for_test().unwrap();
        assert_eq!(cache.activity(), Activity::Running);
        assert!(
            cache.job.is_running(),
            "observation consumed a completed worker result"
        );
        assert_eq!(
            cache.job.poll(),
            LatestJobPoll::Failed("cache is locked".into())
        );
        for _ in 0..2 {
            assert_eq!(cache.activity(), Activity::Failed("cache is locked".into()));
        }
        cache
            .job
            .start("test-biber-retry", || Ok(PathBuf::new()))
            .unwrap();
        cache.job.wait_until_ready_for_test().unwrap();
        assert_eq!(cache.activity(), Activity::Running);
        assert_eq!(cache.job.poll(), LatestJobPoll::Ready(PathBuf::new()));
        assert_eq!(cache.activity(), Activity::Idle);
    }
}
