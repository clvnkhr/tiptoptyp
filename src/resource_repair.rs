//! Explicit, shared resource recovery jobs. No filesystem or network work runs
//! while Settings borrows its presentation snapshot.
use crate::worker::{ExclusiveJob, LatestJobPoll};
use eframe::egui;
use std::sync::{
    Arc, LazyLock, Mutex,
    atomic::{AtomicBool, AtomicU64, Ordering},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Component {
    Ltex,
    Liquid,
}
#[derive(Clone, Default)]
pub(crate) struct Status {
    pub(crate) component: Option<Component>,
    pub(crate) busy: bool,
    pub(crate) message: Option<String>,
}
#[derive(Default)]
struct State {
    component: Option<Component>,
    job: ExclusiveJob<String>,
    cancel: Arc<AtomicBool>,
    message: Option<String>,
}
static STATE: LazyLock<Mutex<State>> = LazyLock::new(|| Mutex::new(State::default()));
static LTEX_GENERATION: AtomicU64 = AtomicU64::new(0);

pub(crate) fn ltex_generation() -> u64 {
    LTEX_GENERATION.load(Ordering::Acquire)
}
pub(crate) fn status() -> Status {
    STATE
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .snapshot()
}
pub(crate) fn cancel() {
    STATE
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .cancel
        .store(true, Ordering::Release);
}
pub(crate) fn start(
    component: Component,
    settings: &crate::settings::AppSettings,
    context: &egui::Context,
) -> Result<(), String> {
    let config = match component {
        Component::Liquid => Some(crate::liquid_runtime::Config::from_settings(settings)?),
        Component::Ltex => None,
    };
    let mut state = STATE.lock().unwrap_or_else(|error| error.into_inner());
    let message = match component {
        Component::Ltex => "Downloading LTeX+…",
        Component::Liquid => "Downloading Liquid…",
    };
    state.start(component, context, message, move |cancel| match component {
        Component::Ltex => {
            crate::ltex_resources::redownload(&cancel)?;
            LTEX_GENERATION.fetch_add(1, Ordering::AcqRel);
            Ok("LTeX+ downloaded".into())
        }
        Component::Liquid => {
            let client = crate::liquid_runtime::shared_client();
            let mut client = crate::liquid_runtime::lock_client(&client, &cancel)?;
            client.redownload(config.as_ref().unwrap(), &cancel)?;
            Ok("Liquid downloaded".into())
        }
    })
}
impl State {
    fn snapshot(&mut self) -> Status {
        match self.job.poll() {
            LatestJobPoll::Ready(message) | LatestJobPoll::Failed(message) => {
                self.message = Some(message)
            }
            _ => {}
        }
        Status {
            component: self.component,
            busy: self.job.is_running(),
            message: self.message.clone(),
        }
    }
    fn start(
        &mut self,
        component: Component,
        context: &egui::Context,
        message: &str,
        work: impl FnOnce(Arc<AtomicBool>) -> Result<String, String> + Send + 'static,
    ) -> Result<(), String> {
        if self.snapshot().busy {
            return Err("A resource task is already running".into());
        }
        self.cancel = Arc::new(AtomicBool::new(false));
        let cancel = self.cancel.clone();
        self.job
            .start_and_repaint("Resource recovery", context, move || work(cancel))?;
        self.component = Some(component);
        self.message = Some(message.into());
        context.request_repaint();
        Ok(())
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn recovery_is_exclusive_cancellable_and_publishes_its_terminal_status() {
        let mut state = State::default();
        let context = egui::Context::default();
        assert_eq!(state.snapshot().component, None);
        let (entered, received) = std::sync::mpsc::channel();
        state
            .start(Component::Liquid, &context, "working", move |cancel| {
                entered.send(()).unwrap();
                while !cancel.load(Ordering::Acquire) {
                    std::thread::yield_now();
                }
                Err("cancelled".into())
            })
            .unwrap();
        received
            .recv_timeout(std::time::Duration::from_secs(5))
            .unwrap();
        assert!(state.snapshot().busy);
        assert_eq!(state.snapshot().component, Some(Component::Liquid));
        assert!(
            state
                .start(Component::Ltex, &context, "duplicate", |_| panic!(
                    "duplicate job must not run"
                ))
                .is_err()
        );
        assert_eq!(state.snapshot().component, Some(Component::Liquid));
        state.cancel.store(true, Ordering::Release);
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        loop {
            let status = state.snapshot();
            if !status.busy {
                assert_eq!(status.message.as_deref(), Some("cancelled"));
                assert_eq!(status.component, Some(Component::Liquid));
                break;
            }
            assert!(std::time::Instant::now() < deadline);
            std::thread::yield_now();
        }
        assert!(!state.snapshot().busy);
        state
            .start(Component::Ltex, &context, "next", |_| Ok("ready".into()))
            .unwrap();
        assert_eq!(state.snapshot().component, Some(Component::Ltex));
    }
}
