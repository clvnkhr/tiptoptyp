//! Mutations cannot be superseded. Completion outlives the initiating window.
use super::*;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
static DETACHED: OnceLock<Mutex<Vec<String>>> = OnceLock::new();
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

pub(crate) fn take_detached_completions() -> Vec<String> {
    std::mem::take(
        &mut *DETACHED
            .get_or_init(Default::default)
            .lock()
            .unwrap_or_else(|e| e.into_inner()),
    )
}

#[cfg(test)]
pub(crate) fn take_matching_detached_completion(needle: &str) -> Option<String> {
    let mut messages = DETACHED.get_or_init(Default::default).lock().unwrap();
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

fn retain_completion<T: OperationSummary>(name: &str, result: Result<T, String>) {
    let message = match result {
        Ok(value) => format!(
            "{name} completed after its window closed: {}",
            value.completion_summary()
        ),
        Err(error) => format!("{name} failed after its window closed: {error}"),
    };
    DETACHED
        .get_or_init(Default::default)
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .push(message);
}
struct Completion<T> {
    owner_open: bool,
    result: Option<Result<T, String>>,
}
pub(crate) struct ExclusiveJob<T: OperationSummary> {
    active: Option<Arc<Mutex<Completion<T>>>>,
    name: String,
    shell: Option<RepaintTarget>,
}
impl<T: OperationSummary> Default for ExclusiveJob<T> {
    fn default() -> Self {
        Self {
            active: None,
            name: String::new(),
            shell: None,
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
        self.name = name.into();
        let name = self.name.clone();
        let owner = RepaintTarget::current(context);
        let shell = RepaintTarget::new(context, egui::ViewportId::ROOT);
        self.shell = Some(shell.clone());
        let completion = Arc::new(Mutex::new(Completion {
            owner_open: true,
            result: None,
        }));
        let worker_completion = completion.clone();
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
                if completion.owner_open {
                    completion.result = Some(result);
                    drop(completion);
                    owner.request_repaint();
                } else {
                    retain_completion(&name, result);
                    drop(completion);
                    shell.request_repaint();
                }
            })
            .map_err(|error| format!("could not start background operation: {error}"))?;
        self.active = Some(completion);
        Ok(())
    }
    pub(crate) fn poll(&mut self) -> LatestJobPoll<T> {
        let Some(active) = &self.active else {
            return LatestJobPoll::Idle;
        };
        let result = active
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .result
            .take();
        match result {
            None => LatestJobPoll::Pending,
            Some(result) => {
                self.active = None;
                match result {
                    Ok(value) => LatestJobPoll::Ready(value),
                    Err(error) => LatestJobPoll::Failed(error),
                }
            }
        }
    }
    pub(crate) fn is_running(&self) -> bool {
        self.active.is_some()
    }
}
impl<T: OperationSummary> Drop for ExclusiveJob<T> {
    fn drop(&mut self) {
        if let Some(active) = self.active.take() {
            let mut completion = active.lock().unwrap_or_else(|e| e.into_inner());
            completion.owner_open = false;
            if let Some(result) = completion.result.take() {
                retain_completion(&self.name, result);
                if let Some(shell) = &self.shell {
                    shell.request_repaint();
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
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
        assert!(take_matching_detached_completion("exclusive-test-mutation completed").is_some());
    }
}
