//! Mutations cannot be superseded. Completion outlives the initiating window.
use super::*;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

fn detached(context: &egui::Context) -> Arc<Mutex<Vec<String>>> {
    context.data_mut(|data| {
        data.get_temp_mut_or_default::<Arc<Mutex<Vec<String>>>>(egui::Id::new(
            "detached-operation-completions",
        ))
        .clone()
    })
}
static ACTIVE: AtomicUsize = AtomicUsize::new(0);

pub(crate) fn has_active_operations() -> bool {
    ACTIVE.load(Ordering::Acquire) != 0
}
struct ActiveOperation {
    shell: RepaintTarget,
}
impl ActiveOperation {
    fn new(shell: RepaintTarget) -> Self {
        ACTIVE.fetch_add(1, Ordering::AcqRel);
        Self { shell }
    }
}
impl Drop for ActiveOperation {
    fn drop(&mut self) {
        ACTIVE.fetch_sub(1, Ordering::AcqRel);
        self.shell.request_repaint();
    }
}

pub(crate) fn take_detached_completions(context: &egui::Context) -> Vec<String> {
    std::mem::take(&mut *detached(context).lock().unwrap_or_else(|e| e.into_inner()))
}

#[cfg(test)]
pub(crate) fn take_matching_detached_completion(
    context: &egui::Context,
    needle: &str,
) -> Option<String> {
    let queue = detached(context);
    let mut messages = queue.lock().unwrap();
    let index = messages
        .iter()
        .position(|message| message.contains(needle))?;
    Some(messages.remove(index))
}
/// Mutating operations retain a useful outcome even if their UI owner is gone.
pub(crate) trait OperationSummary {
    fn completion_summary(&self) -> String;
}
impl OperationSummary for String {
    fn completion_summary(&self) -> String {
        self.clone()
    }
}
#[cfg(test)]
impl OperationSummary for () {
    fn completion_summary(&self) -> String {
        "Done".to_owned()
    }
}

fn retain_completion<T: OperationSummary>(
    context: &egui::Context,
    name: &str,
    result: Result<T, String>,
) {
    let message = match result {
        Ok(value) => format!(
            "{name} completed after its window closed: {}",
            value.completion_summary()
        ),
        Err(error) => format!("{name} failed after its window closed: {error}"),
    };
    detached(context)
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .push(message);
}
/// Owner closure and completion race under one lock. A finished result is
/// either delivered to the owner or retained by the shell, exactly once.
enum Completion<T> {
    Working,
    Ready(Result<T, String>),
    Detached,
}

impl<T> Completion<T> {
    fn finish(&mut self, result: Result<T, String>) -> Option<Result<T, String>> {
        match self {
            Self::Working => {
                *self = Self::Ready(result);
                None
            }
            Self::Detached => Some(result),
            Self::Ready(_) => unreachable!("an exclusive worker completes exactly once"),
        }
    }

    fn take_ready(&mut self) -> Option<Result<T, String>> {
        if !matches!(self, Self::Ready(_)) {
            return None;
        }
        let Self::Ready(result) = std::mem::replace(self, Self::Detached) else {
            unreachable!()
        };
        Some(result)
    }

    fn detach(&mut self) -> Option<Result<T, String>> {
        let previous = std::mem::replace(self, Self::Detached);
        if let Self::Ready(result) = previous {
            Some(result)
        } else {
            None
        }
    }
}

pub(crate) struct ExclusiveJob<T: OperationSummary> {
    state: ExclusiveJobState<T>,
}

enum ExclusiveJobState<T> {
    Idle,
    Running(RunningOperation<T>),
    Failed(String),
}

/// Metadata required to retain an operation's outcome cannot outlive or be
/// installed separately from the operation it describes.
struct RunningOperation<T> {
    completion: Arc<Mutex<Completion<T>>>,
    name: String,
    shell: RepaintTarget,
}
impl<T: OperationSummary> Default for ExclusiveJob<T> {
    fn default() -> Self {
        Self {
            state: ExclusiveJobState::Idle,
        }
    }
}
impl<T: OperationSummary + Send + 'static> ExclusiveJob<T> {
    pub(crate) fn start_and_repaint(
        &mut self,
        name: impl Into<String>,
        context: &egui::Context,
        work: impl FnOnce() -> Result<T, String> + Send + 'static,
    ) -> Result<(), String> {
        if self.is_running() {
            return Err("An operation is already in progress".to_owned());
        }
        self.state = ExclusiveJobState::Idle;
        let name = name.into();
        let owner = RepaintTarget::current(context);
        let shell = RepaintTarget::new(context, egui::ViewportId::ROOT);
        let completion = Arc::new(Mutex::new(Completion::Working));
        let worker_completion = completion.clone();
        let worker_name = name.clone();
        let worker_shell = shell.clone();
        // The lease is created before spawn and is dropped on spawn failure,
        // worker unwind, or completion. Closing a window cannot release it.
        let active = ActiveOperation::new(shell.clone());
        thread::Builder::new()
            .name(name.clone())
            .spawn(move || {
                let _active = active;
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(work))
                    .unwrap_or_else(|_| Err("background worker panicked".to_owned()));
                let mut completion = worker_completion.lock().unwrap_or_else(|e| e.into_inner());
                if let Some(result) = completion.finish(result) {
                    retain_completion(&worker_shell.context, &worker_name, result);
                    drop(completion);
                    worker_shell.request_repaint();
                } else {
                    drop(completion);
                    owner.request_repaint();
                }
            })
            .map_err(|error| {
                let message = format!("could not start background operation: {error}");
                self.state = ExclusiveJobState::Failed(message.clone());
                message
            })?;
        self.state = ExclusiveJobState::Running(RunningOperation {
            completion,
            name,
            shell,
        });
        Ok(())
    }
    pub(crate) fn poll(&mut self) -> LatestJobPoll<T> {
        let ExclusiveJobState::Running(active) = &self.state else {
            return LatestJobPoll::Idle;
        };
        let result = active
            .completion
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .take_ready();
        match result {
            None => LatestJobPoll::Pending,
            Some(result) => match result {
                Ok(value) => {
                    self.state = ExclusiveJobState::Idle;
                    LatestJobPoll::Ready(value)
                }
                Err(error) => {
                    self.state = ExclusiveJobState::Failed(error.clone());
                    LatestJobPoll::Failed(error)
                }
            },
        }
    }
    pub(crate) fn activity(&self) -> crate::activity::Activity {
        match &self.state {
            ExclusiveJobState::Idle => crate::activity::Activity::Idle,
            ExclusiveJobState::Running(_) => crate::activity::Activity::Running,
            ExclusiveJobState::Failed(error) => crate::activity::Activity::Failed(error.clone()),
        }
    }
    pub(crate) fn is_running(&self) -> bool {
        matches!(self.state, ExclusiveJobState::Running(_))
    }
}
impl<T: OperationSummary> Drop for ExclusiveJob<T> {
    fn drop(&mut self) {
        if let ExclusiveJobState::Running(active) = &self.state {
            let mut completion = active.completion.lock().unwrap_or_else(|e| e.into_inner());
            if let Some(result) = completion.detach() {
                retain_completion(&active.shell.context, &active.name, result);
                active.shell.request_repaint();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn poll_until_settled<T: OperationSummary + Send + 'static>(
        job: &mut ExclusiveJob<T>,
    ) -> LatestJobPoll<T> {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        loop {
            match job.poll() {
                LatestJobPoll::Pending => {
                    assert!(std::time::Instant::now() < deadline);
                    thread::yield_now();
                }
                result => return result,
            }
        }
    }

    #[test]
    fn completion_and_owner_close_have_exactly_one_delivery_in_either_order() {
        let mut completion = Completion::Working;
        assert!(completion.take_ready().is_none());
        assert_eq!(completion.finish(Ok(42)), None);
        assert_eq!(completion.take_ready(), Some(Ok(42)));
        assert_eq!(completion.take_ready(), None);
        assert_eq!(completion.detach(), None);

        let mut completion = Completion::<usize>::Working;
        assert_eq!(completion.detach(), None);
        assert_eq!(
            completion.finish(Err("detached failure".into())),
            Some(Err("detached failure".into()))
        );
        assert_eq!(completion.take_ready(), None);
        assert_eq!(completion.detach(), None);

        let mut completion = Completion::Working;
        assert_eq!(completion.finish(Ok(42)), None);
        assert_eq!(completion.detach(), Some(Ok(42)));
        assert_eq!(completion.take_ready(), None);
        assert_eq!(completion.detach(), None);
    }

    #[test]
    fn exclusive_failure_is_terminal_and_new_admission_clears_the_old_outcome() {
        let context = egui::Context::default();
        let mut job = ExclusiveJob::<String>::default();
        job.start_and_repaint("exclusive-failure-state", &context, || Err("failed".into()))
            .unwrap();
        assert_eq!(
            poll_until_settled(&mut job),
            LatestJobPoll::Failed("failed".into())
        );
        assert!(!job.is_running());
        assert_eq!(job.poll(), LatestJobPoll::Idle);
        assert_eq!(
            job.activity(),
            crate::activity::Activity::Failed("failed".into())
        );
        job.start_and_repaint("exclusive-recovery-state", &context, || {
            Ok("recovered".into())
        })
        .unwrap();
        assert_eq!(job.activity(), crate::activity::Activity::Running);
        assert_eq!(
            poll_until_settled(&mut job),
            LatestJobPoll::Ready("recovered".into())
        );
        assert_eq!(job.activity(), crate::activity::Activity::Idle);
    }
    #[test]
    fn detached_completions_belong_to_their_application_context() {
        let first = egui::Context::default();
        let second = egui::Context::default();
        retain_completion(&first, "first", Ok(()));
        retain_completion(&second, "second", Ok(()));
        assert_eq!(
            take_detached_completions(&first),
            vec!["first completed after its window closed: Done"]
        );
        assert!(take_detached_completions(&first).is_empty());
        assert_eq!(
            take_detached_completions(&second),
            vec!["second completed after its window closed: Done"]
        );
    }

    #[test]
    fn second_mutation_is_rejected_and_closed_owner_completion_reaches_shell() {
        let context = egui::Context::default();
        let (release, wait) = mpsc::channel();
        let (repaint_tx, repaint_rx) = mpsc::channel();
        context.set_request_repaint_callback(move |info| {
            let _ = repaint_tx.send(info.viewport_id);
        });
        let mut job = ExclusiveJob::default();
        job.start_and_repaint("exclusive-test-mutation", &context, move || {
            wait.recv().unwrap();
            Ok(())
        })
        .unwrap();
        assert!(
            job.start_and_repaint("duplicate", &context, || panic!("must not run"))
                .is_err()
        );
        drop(job);
        assert!(has_active_operations());
        release.send(()).unwrap();
        assert_eq!(
            repaint_rx
                .recv_timeout(std::time::Duration::from_secs(5))
                .unwrap(),
            egui::ViewportId::ROOT
        );
        assert!(
            take_matching_detached_completion(&context, "exclusive-test-mutation completed")
                .is_some()
        );
    }
}
