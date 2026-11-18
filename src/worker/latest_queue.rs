//! A bounded mailbox for replaceable reads. Never use it for mutations.
use std::sync::{
    Arc, Condvar, Mutex,
    mpsc::{RecvError, SendError, TryRecvError},
};
/// Closing the sender drains its final request. Closing the receiver discards
/// it and permanently rejects sends; there is no closed receiver with queued
/// work left alive by a mismatched flag.
enum State<T> {
    Open { pending: Option<T> },
    SenderClosed { pending: Option<T> },
    ReceiverClosed,
}

impl<T> State<T> {
    fn take(&mut self) -> Result<T, TryRecvError> {
        match self {
            Self::Open { pending } => pending.take().ok_or(TryRecvError::Empty),
            Self::SenderClosed { pending } => pending.take().ok_or(TryRecvError::Disconnected),
            Self::ReceiverClosed => Err(TryRecvError::Disconnected),
        }
    }
}
struct Shared<T> {
    state: Mutex<State<T>>,
    changed: Condvar,
}
pub(crate) struct LatestSender<T>(Arc<Shared<T>>);
pub(crate) struct LatestReceiver<T>(Arc<Shared<T>>);

pub(crate) fn latest_channel<T>() -> (LatestSender<T>, LatestReceiver<T>) {
    let shared = Arc::new(Shared {
        state: Mutex::new(State::Open { pending: None }),
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
        let State::Open { pending } = &mut *state else {
            return Err(SendError(request));
        };
        *pending = Some(request);
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
            match state.take() {
                Ok(request) => return Ok(request),
                Err(TryRecvError::Disconnected) => return Err(RecvTimeoutError::Disconnected),
                Err(TryRecvError::Empty) => {}
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
            match state.take() {
                Ok(request) => return Ok(request),
                Err(TryRecvError::Disconnected) => return Err(RecvError),
                Err(TryRecvError::Empty) => {}
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
        state.take()
    }
}
impl<T> Drop for LatestSender<T> {
    fn drop(&mut self) {
        let mut state = self
            .0
            .state
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if let State::Open { pending } = &mut *state {
            *state = State::SenderClosed {
                pending: pending.take(),
            };
        }
        drop(state);
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
        *state = State::ReceiverClosed;
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

    #[test]
    fn sender_close_drains_only_its_final_request_before_disconnect() {
        let (sender, receiver) = latest_channel();
        sender.send("superseded").unwrap();
        sender.send("final").unwrap();
        drop(sender);
        assert_eq!(receiver.try_recv(), Ok("final"));
        assert_eq!(receiver.try_recv(), Err(TryRecvError::Disconnected));
        assert_eq!(receiver.recv(), Err(RecvError));
        assert_eq!(
            receiver.recv_timeout(std::time::Duration::ZERO),
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected)
        );
    }

    #[test]
    fn receiver_close_drops_the_pending_payload_and_rejects_later_payloads() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        struct Payload(Arc<AtomicUsize>);
        impl Drop for Payload {
            fn drop(&mut self) {
                self.0.fetch_add(1, Ordering::Relaxed);
            }
        }
        let dropped = Arc::new(AtomicUsize::new(0));
        let (sender, receiver) = latest_channel();
        assert!(sender.send(Payload(dropped.clone())).is_ok());
        drop(receiver);
        assert_eq!(dropped.load(Ordering::Relaxed), 1);
        let rejected = sender.send(Payload(dropped.clone()));
        assert!(rejected.is_err());
        drop(rejected);
        assert_eq!(dropped.load(Ordering::Relaxed), 2);
    }
}
