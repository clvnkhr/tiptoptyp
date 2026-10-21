//! Explicit maintenance work never touches the compiler's UI frame.
use super::*;
use std::sync::atomic::{AtomicBool, Ordering};

#[derive(Default)]
pub(super) struct BiberCache {
    job: LatestJob<PathBuf>,
    cancel: Arc<AtomicBool>,
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
