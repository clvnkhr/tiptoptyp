mod exclusive;
pub(crate) use exclusive::{
    ExclusiveJob, OperationSummary, has_active_operations, take_detached_completions,
};
mod latest_queue;
pub(crate) use latest_queue::{LatestReceiver, LatestSender, latest_channel};
use std::{
    sync::mpsc::{self, Receiver, TryRecvError},
    thread,
};

use eframe::egui;

/// Drains valid events first, then reports service termination exactly once.
/// An empty queue and a dead worker must not both look like "still loading".
pub(crate) fn poll_service<T>(
    receiver: &Receiver<T>,
    disconnected: &std::sync::atomic::AtomicBool,
) -> Option<Result<T, ()>> {
    use std::sync::atomic::Ordering;
    match receiver.try_recv() {
        Ok(event) => Some(Ok(event)),
        Err(TryRecvError::Empty) => None,
        Err(TryRecvError::Disconnected) => {
            (!disconnected.swap(true, Ordering::AcqRel)).then_some(Err(()))
        }
    }
}

/// A worker can wake its owner, but cannot inspect or mutate the active UI.
#[derive(Clone)]
pub(crate) struct RepaintTarget {
    context: egui::Context,
    viewport: egui::ViewportId,
}
impl RepaintTarget {
    pub(crate) fn new(context: &egui::Context, viewport: egui::ViewportId) -> Self {
        Self {
            context: context.clone(),
            viewport,
        }
    }
    pub(crate) fn current(context: &egui::Context) -> Self {
        Self::new(context, context.viewport_id())
    }
    pub(crate) fn request_repaint(&self) {
        self.context.request_repaint_of(self.viewport);
    }
    #[cfg(test)]
    pub(crate) fn test() -> Self {
        Self::current(&egui::Context::default())
    }
}

/// Result of polling the most recently requested one-shot job.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum LatestJobPoll<T> {
    Idle,
    Pending,
    Ready(T),
    Failed(String),
}

/// Owns the lifecycle of replaceable, one-shot background work.
///
/// Starting a new job drops the previous receiver, so an older result can no
/// longer replace newer state. A disconnected worker is reported as a failure
/// rather than being mistaken for an indefinitely pending job.
pub(crate) struct LatestJob<T> {
    receiver: Option<Receiver<Result<T, String>>>,
}

impl<T> Default for LatestJob<T> {
    fn default() -> Self {
        Self { receiver: None }
    }
}

impl<T: Send + 'static> LatestJob<T> {
    pub(crate) fn start(
        &mut self,
        name: impl Into<String>,
        work: impl FnOnce() -> Result<T, String> + Send + 'static,
    ) -> Result<(), String> {
        self.start_with_repaint(name, None, work)
    }

    pub(crate) fn start_and_repaint(
        &mut self,
        name: impl Into<String>,
        context: &egui::Context,
        work: impl FnOnce() -> Result<T, String> + Send + 'static,
    ) -> Result<(), String> {
        self.start_with_repaint(name, Some(RepaintTarget::current(context)), work)
    }

    fn start_with_repaint(
        &mut self,
        name: impl Into<String>,
        repaint: Option<RepaintTarget>,
        work: impl FnOnce() -> Result<T, String> + Send + 'static,
    ) -> Result<(), String> {
        // Context clones share the active viewport. Remember the originating
        // window before leaving the UI thread instead of consulting it later.
        // Replacing the receiver first makes a failed spawn terminal too: the
        // caller never remains stuck polling an obsolete request.
        self.receiver = None;
        let (sender, receiver) = mpsc::channel();
        let spawned = thread::Builder::new().name(name.into()).spawn(move || {
            let _span = crate::performance::span("worker.job");
            let result = work();
            if sender.send(result).is_ok()
                && let Some(target) = repaint
            {
                target.request_repaint();
            }
        });
        self.accept_spawn(receiver, spawned)
    }

    fn accept_spawn(
        &mut self,
        receiver: Receiver<Result<T, String>>,
        spawned: std::io::Result<thread::JoinHandle<()>>,
    ) -> Result<(), String> {
        match spawned {
            Ok(_worker) => {
                self.receiver = Some(receiver);
                Ok(())
            }
            Err(error) => {
                self.receiver = None;
                Err(format!("could not start background job: {error}"))
            }
        }
    }

    pub(crate) fn poll(&mut self) -> LatestJobPoll<T> {
        let Some(receiver) = self.receiver.as_ref() else {
            return LatestJobPoll::Idle;
        };
        match receiver.try_recv() {
            Ok(Ok(value)) => {
                self.receiver = None;
                LatestJobPoll::Ready(value)
            }
            Ok(Err(error)) => {
                self.receiver = None;
                LatestJobPoll::Failed(error)
            }
            Err(TryRecvError::Empty) => LatestJobPoll::Pending,
            Err(TryRecvError::Disconnected) => {
                self.receiver = None;
                LatestJobPoll::Failed("background worker stopped unexpectedly".to_owned())
            }
        }
    }

    pub(crate) fn is_running(&self) -> bool {
        self.receiver.is_some()
    }

    /// Supersede acceptance; this does not promise to interrupt running code.
    pub(crate) fn supersede(&mut self) {
        self.receiver = None;
    }
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use super::*;

    #[test]
    fn service_disconnection_is_terminal_after_queued_events_are_drained() {
        let (sender, receiver) = mpsc::channel();
        let reported = std::sync::atomic::AtomicBool::new(false);
        assert_eq!(poll_service(&receiver, &reported), None);
        sender.send("finished").unwrap();
        drop(sender);
        assert_eq!(poll_service(&receiver, &reported), Some(Ok("finished")));
        assert_eq!(poll_service(&receiver, &reported), Some(Err(())));
        assert_eq!(poll_service(&receiver, &reported), None);
    }

    fn poll_until_settled<T: Send + 'static>(job: &mut LatestJob<T>) -> LatestJobPoll<T> {
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            match job.poll() {
                LatestJobPoll::Pending if Instant::now() < deadline => thread::yield_now(),
                result => return result,
            }
        }
    }

    #[test]
    fn successful_job_becomes_ready_and_then_idle() {
        let mut job = LatestJob::default();
        job.start("latest-job-success", || Ok(42)).unwrap();
        assert_eq!(poll_until_settled(&mut job), LatestJobPoll::Ready(42));
        assert_eq!(job.poll(), LatestJobPoll::Idle);
    }

    #[test]
    fn completion_repaints_its_origin_after_another_window_becomes_current() {
        let context = egui::Context::default();
        let origin = egui::ViewportId::from_hash_of("git-in-second-document");
        let other = egui::ViewportId::from_hash_of("another-document");
        let mut job = LatestJob::default();
        let (release, receiver) = mpsc::channel();
        let mut receiver = Some(receiver);
        // Settle each viewport before observing completion-specific repaints.
        for viewport in [origin, origin, origin, other, other, other, origin] {
            let mut input = egui::RawInput {
                viewport_id: viewport,
                ..Default::default()
            };
            input.viewports.entry(viewport).or_default();
            let mut output = context.run_ui(input, |ui| {
                if viewport == origin
                    && let Some(receiver) = receiver.take()
                {
                    job.start_and_repaint("origin-repaint", ui.ctx(), move || {
                        receiver.recv().unwrap();
                        Ok(42)
                    })
                    .unwrap();
                }
            });
            output.textures_delta.clear();
        }
        let mut input = egui::RawInput {
            viewport_id: other,
            ..Default::default()
        };
        input.viewports.entry(other).or_default();
        let mut output = context.run_ui(input, |_| {});
        output.textures_delta.clear();
        let (repaint_sender, repaint_receiver) = mpsc::channel();
        context.set_request_repaint_callback(move |info| {
            let _ = repaint_sender.send(info.viewport_id);
        });
        release.send(()).unwrap();
        assert_eq!(
            repaint_receiver
                .recv_timeout(Duration::from_secs(2))
                .unwrap(),
            origin
        );
        assert_eq!(poll_until_settled(&mut job), LatestJobPoll::Ready(42));
    }

    #[test]
    fn worker_error_clears_pending_state() {
        let mut job = LatestJob::<()>::default();
        job.start("latest-job-failure", || Err("scan failed".to_owned()))
            .unwrap();
        assert_eq!(
            poll_until_settled(&mut job),
            LatestJobPoll::Failed("scan failed".to_owned())
        );
        assert!(!job.is_running());
    }

    #[test]
    fn disconnected_worker_is_failed_and_clears_pending_state() {
        let (sender, receiver) = mpsc::channel();
        drop(sender);
        let mut job = LatestJob::<()> {
            receiver: Some(receiver),
        };
        assert_eq!(
            job.poll(),
            LatestJobPoll::Failed("background worker stopped unexpectedly".to_owned())
        );
        assert_eq!(job.poll(), LatestJobPoll::Idle);
    }

    #[test]
    fn injected_spawn_failure_does_not_leave_an_old_job_pending() {
        let (_old_sender, old_receiver) = mpsc::channel();
        let mut job = LatestJob::<()> {
            receiver: Some(old_receiver),
        };
        let (_sender, receiver) = mpsc::channel();
        let error = job
            .accept_spawn(
                receiver,
                Err(std::io::Error::other("injected spawn failure")),
            )
            .unwrap_err();
        assert!(error.contains("injected spawn failure"));
        assert_eq!(job.poll(), LatestJobPoll::Idle);
    }

    #[test]
    fn replacing_a_job_rejects_its_stale_result() {
        let mut job = LatestJob::default();
        job.start("latest-job-stale", || Ok(1)).unwrap();
        job.start("latest-job-current", || Ok(2)).unwrap();
        assert_eq!(poll_until_settled(&mut job), LatestJobPoll::Ready(2));
    }
}
