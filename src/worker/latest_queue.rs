//! A bounded mailbox for replaceable reads. Never use it for mutations.
use std::sync::{
    Arc, Condvar, Mutex,
    mpsc::{RecvError, SendError, TryRecvError},
};
struct State<T> {
    pending: Option<T>,
    sender_alive: bool,
    receiver_alive: bool,
}
struct Shared<T> {
    state: Mutex<State<T>>,
    changed: Condvar,
}
pub(crate) struct LatestSender<T>(Arc<Shared<T>>);
pub(crate) struct LatestReceiver<T>(Arc<Shared<T>>);

pub(crate) fn latest_channel<T>() -> (LatestSender<T>, LatestReceiver<T>) {
    let shared = Arc::new(Shared {
        state: Mutex::new(State {
            pending: None,
            sender_alive: true,
            receiver_alive: true,
        }),
        changed: Condvar::new(),
    });
    (LatestSender(shared.clone()), LatestReceiver(shared))
}
impl<T> LatestSender<T> {
    pub(crate) fn send(&self, request: T) -> Result<(), SendError<T>> {
        let mut state = self
            .0
            .state
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if !state.receiver_alive {
            return Err(SendError(request));
        }
        state.pending = Some(request);
        self.0.changed.notify_one();
        Ok(())
    }
}
impl<T> LatestReceiver<T> {
    pub(crate) fn recv_timeout(
        &self,
        timeout: std::time::Duration,
    ) -> Result<T, std::sync::mpsc::RecvTimeoutError> {
        use std::{sync::mpsc::RecvTimeoutError, time::Instant};
        let started = Instant::now();
        let mut state = self
            .0
            .state
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        loop {
            if let Some(request) = state.pending.take() {
                return Ok(request);
            }
            if !state.sender_alive {
                return Err(RecvTimeoutError::Disconnected);
            }
            let Some(remaining) = timeout
                .checked_sub(started.elapsed())
                .filter(|remaining| !remaining.is_zero())
            else {
                return Err(RecvTimeoutError::Timeout);
            };
            state = self
                .0
                .changed
                .wait_timeout(state, remaining)
                .unwrap_or_else(|error| error.into_inner())
                .0;
        }
    }
    pub(crate) fn recv(&self) -> Result<T, RecvError> {
        let mut state = self
            .0
            .state
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        loop {
            if let Some(request) = state.pending.take() {
                return Ok(request);
            }
            if !state.sender_alive {
                return Err(RecvError);
            }
            state = self
                .0
                .changed
                .wait(state)
                .unwrap_or_else(|error| error.into_inner());
        }
    }
    pub(crate) fn try_recv(&self) -> Result<T, TryRecvError> {
        let mut state = self
            .0
            .state
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        state.pending.take().ok_or(if state.sender_alive {
            TryRecvError::Empty
        } else {
            TryRecvError::Disconnected
        })
    }
}
impl<T> Drop for LatestSender<T> {
    fn drop(&mut self) {
        self.0
            .state
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .sender_alive = false;
        self.0.changed.notify_one();
    }
}
impl<T> Drop for LatestReceiver<T> {
    fn drop(&mut self) {
        let mut state = self
            .0
            .state
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        state.receiver_alive = false;
        state.pending = None;
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn saturated_mailbox_keeps_only_the_latest_request() {
        let (sender, receiver) = latest_channel();
        for revision in 0..100_000 {
            sender.send(revision).unwrap();
        }
        assert_eq!(receiver.recv(), Ok(99_999));
        assert_eq!(receiver.try_recv(), Err(TryRecvError::Empty));
        assert_eq!(
            receiver.recv_timeout(std::time::Duration::ZERO),
            Err(std::sync::mpsc::RecvTimeoutError::Timeout)
        );
        drop(sender);
        assert_eq!(receiver.recv(), Err(RecvError));
        assert_eq!(
            receiver.recv_timeout(std::time::Duration::ZERO),
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected)
        );
    }
    #[test]
    fn owner_close_rejects_new_work_and_sender_close_wakes_worker() {
        let (sender, receiver) = latest_channel::<usize>();
        drop(receiver);
        assert!(sender.send(1).is_err());
        let (sender, receiver) = latest_channel::<usize>();
        let worker = std::thread::spawn(move || receiver.recv());
        drop(sender);
        assert_eq!(worker.join().unwrap(), Err(RecvError));
    }
}
