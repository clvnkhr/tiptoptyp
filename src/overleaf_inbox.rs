//! Bounded decoding of native Overleaf messages away from the UI thread.
use crate::overleaf::BrowserEvent;
use crate::worker::RepaintTarget;
use eframe::egui;
use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicU8, AtomicUsize, Ordering},
    mpsc::{self, Receiver, SyncSender, TryRecvError, TrySendError},
};

const RAW_CAPACITY: usize = 4;
const TYPED_CAPACITY: usize = 8;
const MAX_MESSAGE_BYTES: usize = 16 * 1024 * 1024;
const RAW_OVERFLOW: u8 = 1;
const TYPED_OVERFLOW: u8 = 2;
const TOO_LARGE: u8 = 3;
const START_FAILED: u8 = 4;
const WORKER_STOPPED: u8 = 5;
type Decoded = Result<BrowserEvent, String>;

enum Raw {
    Message(String),
    Wake,
}
struct Shared {
    senders: AtomicUsize,
    senders_closed: AtomicBool,
    receiver_alive: AtomicBool,
    failure: AtomicU8,
    repaint: RepaintTarget,
}
impl Shared {
    fn fail(&self, failure: u8) {
        if self.receiver_alive.load(Ordering::Acquire)
            && self
                .failure
                .compare_exchange(0, failure, Ordering::AcqRel, Ordering::Acquire)
                .is_ok()
        {
            self.senders_closed.store(true, Ordering::Release);
            self.repaint.request_repaint();
        }
    }
}

/// The native callback can enqueue a bounded string, but never wait for decoding.
pub(crate) struct Sender {
    raw: SyncSender<Raw>,
    shared: Arc<Shared>,
}
impl Clone for Sender {
    fn clone(&self) -> Self {
        self.shared.senders.fetch_add(1, Ordering::Relaxed);
        Self {
            raw: self.raw.clone(),
            shared: Arc::clone(&self.shared),
        }
    }
}
impl Drop for Sender {
    fn drop(&mut self) {
        if self.shared.senders.fetch_sub(1, Ordering::AcqRel) == 1 {
            self.shared.senders_closed.store(true, Ordering::Release);
            // A full queue already guarantees that the worker has work to drain.
            let _ = self.raw.try_send(Raw::Wake);
        }
    }
}
impl Sender {
    pub(crate) fn send(&self, message: String) -> Result<(), String> {
        if !self.shared.receiver_alive.load(Ordering::Acquire)
            || self.shared.senders_closed.load(Ordering::Acquire)
        {
            return Err("The Overleaf connection is closed.".into());
        }
        if message.len() > MAX_MESSAGE_BYTES {
            self.shared.fail(TOO_LARGE);
            let _ = self.raw.try_send(Raw::Wake);
            return Err(failure_message(TOO_LARGE).into());
        }
        match self.raw.try_send(Raw::Message(message)) {
            Ok(()) => Ok(()),
            Err(TrySendError::Full(_)) => {
                self.shared.fail(RAW_OVERFLOW);
                // No retry or blocking send from WebKit's UI callback.
                Err(failure_message(RAW_OVERFLOW).into())
            }
            Err(TrySendError::Disconnected(_)) => {
                self.shared.fail(WORKER_STOPPED);
                Err(failure_message(WORKER_STOPPED).into())
            }
        }
    }
}

pub(crate) struct Inbox {
    raw: SyncSender<Raw>,
    typed: Receiver<Decoded>,
    shared: Arc<Shared>,
    stopped: bool,
}
impl Inbox {
    pub(crate) fn new(context: &egui::Context) -> Self {
        let (inbox, raw, typed) = Self::channels(context);
        let shared = Arc::clone(&inbox.shared);
        if std::thread::Builder::new()
            .name("tiptoptyp-overleaf-decode".into())
            .spawn(move || decode_loop(raw, typed, shared, decode_message))
            .is_err()
        {
            inbox.shared.fail(START_FAILED);
        }
        inbox
    }

    fn channels(context: &egui::Context) -> (Self, Receiver<Raw>, SyncSender<Decoded>) {
        let (raw_sender, raw_receiver) = mpsc::sync_channel(RAW_CAPACITY);
        let (typed_sender, typed_receiver) = mpsc::sync_channel(TYPED_CAPACITY);
        let shared = Arc::new(Shared {
            senders: AtomicUsize::new(0),
            senders_closed: AtomicBool::new(false),
            receiver_alive: AtomicBool::new(true),
            failure: AtomicU8::new(0),
            repaint: RepaintTarget::current(context),
        });
        (
            Self {
                raw: raw_sender,
                typed: typed_receiver,
                shared,
                stopped: false,
            },
            raw_receiver,
            typed_sender,
        )
    }

    pub(crate) fn sender(&self) -> Sender {
        self.shared.senders.fetch_add(1, Ordering::Relaxed);
        Sender {
            raw: self.raw.clone(),
            shared: Arc::clone(&self.shared),
        }
    }

    pub(crate) fn poll(&mut self) -> Option<Decoded> {
        if self.stopped {
            return None;
        }
        let failure = self.shared.failure.load(Ordering::Acquire);
        if failure != 0 {
            // Missing an event invalidates the stream. Do not subsequently apply
            // buffered messages that could make this connection appear live again.
            self.stopped = true;
            return Some(Err(failure_message(failure).into()));
        }
        match self.typed.try_recv() {
            Ok(result) => Some(result),
            Err(TryRecvError::Empty) => None,
            Err(TryRecvError::Disconnected) => {
                self.stopped = true;
                Some(Err(
                    "The Overleaf message decoder stopped. Reconnect to continue.".into(),
                ))
            }
        }
    }
}
impl Drop for Inbox {
    fn drop(&mut self) {
        self.shared.receiver_alive.store(false, Ordering::Release);
        self.shared.senders_closed.store(true, Ordering::Release);
        let _ = self.raw.try_send(Raw::Wake);
        // The worker wakes or observes the full queue; the UI never joins it.
    }
}

fn decode_message(message: &str) -> Decoded {
    if message.len() > MAX_MESSAGE_BYTES {
        return Err(failure_message(TOO_LARGE).into());
    }
    serde_json::from_str(message).map_err(|_| "Overleaf returned an invalid bridge message.".into())
}
fn decode_loop(
    raw: Receiver<Raw>,
    typed: SyncSender<Decoded>,
    shared: Arc<Shared>,
    mut decode: impl FnMut(&str) -> Decoded,
) {
    loop {
        if !shared.receiver_alive.load(Ordering::Acquire)
            || shared.failure.load(Ordering::Acquire) != 0
        {
            return;
        }
        // The last external Sender closes the stream. Drain accepted messages
        // before exiting, even though Inbox retains its channel construction handle.
        let next = if shared.senders_closed.load(Ordering::Acquire) {
            raw.try_recv().ok()
        } else {
            raw.recv().ok()
        };
        let Some(next) = next else {
            return;
        };
        let Raw::Message(message) = next else {
            continue;
        };
        let decoded = decode(&message);
        if !shared.receiver_alive.load(Ordering::Acquire)
            || shared.failure.load(Ordering::Acquire) != 0
        {
            return;
        }
        match typed.try_send(decoded) {
            Ok(()) => shared.repaint.request_repaint(),
            Err(TrySendError::Full(_)) => {
                shared.fail(TYPED_OVERFLOW);
                return;
            }
            Err(TrySendError::Disconnected(_)) => return,
        }
    }
}
fn failure_message(failure: u8) -> &'static str {
    match failure {
        RAW_OVERFLOW | TYPED_OVERFLOW => {
            "The Overleaf message queue is full. Your local draft is retained; reconnect to continue."
        }
        TOO_LARGE => "Overleaf sent a message larger than 16 MiB. Reconnect to continue.",
        START_FAILED => "Could not start the Overleaf message decoder. Reconnect to retry.",
        _ => "The Overleaf message decoder stopped. Reconnect to continue.",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        thread,
        time::{Duration, Instant},
    };

    fn message(epoch: u64) -> String {
        format!(r#"{{"event":"status","epoch":{epoch},"state":"connected"}}"#)
    }
    fn wait_for(mut predicate: impl FnMut() -> bool) {
        let deadline = Instant::now() + Duration::from_secs(5);
        while !predicate() {
            assert!(Instant::now() < deadline, "decoder did not finish");
            thread::yield_now();
        }
    }
    fn next(inbox: &mut Inbox) -> Decoded {
        let mut result = None;
        wait_for(|| {
            result = inbox.poll();
            result.is_some()
        });
        result.unwrap()
    }

    #[test]
    fn decoder_preserves_order_and_reports_bad_json_without_echoing_payload() {
        let mut inbox = Inbox::new(&egui::Context::default());
        let sender = inbox.sender();
        sender.send(message(7)).unwrap();
        sender
            .send(r#"{"event":"secret manuscript text"}"#.into())
            .unwrap();
        sender.send(message(8)).unwrap();
        assert_eq!(next(&mut inbox).unwrap().epoch(), 7);
        let error = next(&mut inbox).unwrap_err();
        assert!(error.contains("invalid bridge message"));
        assert!(!error.contains("secret"));
        assert_eq!(next(&mut inbox).unwrap().epoch(), 8);
    }

    #[test]
    fn raw_overflow_is_nonblocking_terminal_and_visible_exactly_once() {
        let (mut inbox, _raw, _typed) = Inbox::channels(&egui::Context::default());
        let sender = inbox.sender();
        for epoch in 0..RAW_CAPACITY as u64 {
            sender.send(message(epoch)).unwrap();
        }
        assert!(sender.send(message(100)).is_err());
        assert!(inbox.poll().unwrap().unwrap_err().contains("queue is full"));
        assert!(inbox.poll().is_none());
        assert!(sender.send(message(101)).is_err());
        assert!(inbox.poll().is_none());
    }

    #[test]
    fn oversized_messages_never_enter_the_raw_queue() {
        let (mut inbox, raw, _typed) = Inbox::channels(&egui::Context::default());
        let sender = inbox.sender();
        assert!(sender.send("x".repeat(MAX_MESSAGE_BYTES + 1)).is_err());
        assert!(matches!(raw.try_recv(), Ok(Raw::Wake)));
        assert!(matches!(raw.try_recv(), Err(TryRecvError::Empty)));
        assert!(inbox.poll().unwrap().unwrap_err().contains("16 MiB"));
        assert!(inbox.poll().is_none());
    }

    #[test]
    fn typed_overflow_is_bounded_and_cannot_reapply_buffered_events() {
        let (mut inbox, raw, typed) = Inbox::channels(&egui::Context::default());
        let sender = inbox.sender();
        let shared = Arc::clone(&inbox.shared);
        let decoded = Arc::new(AtomicUsize::new(0));
        let counter = Arc::clone(&decoded);
        let worker = thread::spawn(move || {
            decode_loop(raw, typed, shared, |text| {
                counter.fetch_add(1, Ordering::Release);
                decode_message(text)
            });
        });
        for epoch in 0..=TYPED_CAPACITY as u64 {
            sender.send(message(epoch)).unwrap();
            wait_for(|| decoded.load(Ordering::Acquire) > epoch as usize);
        }
        wait_for(|| inbox.shared.failure.load(Ordering::Acquire) != 0);
        assert_eq!(inbox.shared.failure.load(Ordering::Acquire), TYPED_OVERFLOW);
        assert!(inbox.poll().unwrap().is_err());
        assert!(inbox.poll().is_none());
        worker.join().unwrap();
    }

    #[test]
    fn decoding_runs_off_caller_thread_and_final_sender_drop_stops_the_worker() {
        let (mut inbox, raw, typed) = Inbox::channels(&egui::Context::default());
        let sender = inbox.sender();
        let remaining = sender.clone();
        let shared = Arc::clone(&inbox.shared);
        let caller = thread::current().id();
        let (done, finished) = mpsc::channel();
        let worker = thread::spawn(move || {
            decode_loop(raw, typed, shared, |text| {
                assert_ne!(thread::current().id(), caller);
                decode_message(text)
            });
            done.send(()).unwrap();
        });
        drop(sender);
        remaining.send(message(4)).unwrap();
        assert_eq!(next(&mut inbox).unwrap().epoch(), 4);
        remaining.send(message(5)).unwrap();
        drop(remaining);
        assert_eq!(next(&mut inbox).unwrap().epoch(), 5);
        finished.recv_timeout(Duration::from_secs(5)).unwrap();
        worker.join().unwrap();
    }

    #[test]
    fn dropping_inbox_wakes_an_idle_worker_even_with_a_native_sender_alive() {
        let (inbox, raw, typed) = Inbox::channels(&egui::Context::default());
        let sender = inbox.sender();
        let shared = Arc::clone(&inbox.shared);
        let (done, finished) = mpsc::channel();
        let worker = thread::spawn(move || {
            decode_loop(raw, typed, shared, decode_message);
            done.send(()).unwrap();
        });
        drop(inbox);
        finished.recv_timeout(Duration::from_secs(5)).unwrap();
        assert!(sender.send(message(1)).is_err());
        worker.join().unwrap();
    }
}
