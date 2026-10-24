//! Bounded decoding of native Overleaf messages away from the UI thread.
use crate::overleaf::BrowserEvent;
use crate::worker::RepaintTarget;
use eframe::egui;
use std::sync::{
    Arc, Mutex,
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
type Typed = (u64, Decoded);
#[derive(Default)]
struct Ingress {
    sequence: u64,
    reliable_sequence: u64,
    presence: Option<(u64, String)>,
}
#[derive(Default)]
struct TypedState {
    presence: Option<(u64, BrowserEvent)>,
    reliable_pending: usize,
}

enum Raw {
    Message { sequence: u64, message: String },
    Wake,
}
struct Shared {
    senders: AtomicUsize,
    senders_closed: AtomicBool,
    receiver_alive: AtomicBool,
    failure: AtomicU8,
    repaint: RepaintTarget,
    // Ephemeral full snapshots never displace authoritative source/save events.
    ingress: Mutex<Ingress>,
    raw_messages: AtomicUsize,
    wake_pending: AtomicBool,
    typed: Mutex<TypedState>,
}
impl Shared {
    fn wake(&self, raw: &SyncSender<Raw>) {
        if !self.wake_pending.swap(true, Ordering::AcqRel) && raw.try_send(Raw::Wake).is_err() {
            self.wake_pending.store(false, Ordering::Release);
        }
    }
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
            self.shared.wake(&self.raw);
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
            self.shared.wake(&self.raw);
            return Err(failure_message(TOO_LARGE).into());
        }
        // Serialize just sequence assignment and nonblocking enqueue. The
        // decoder never holds this lock while parsing JSON or posting events.
        let mut ingress = self.shared.ingress.lock().unwrap();
        let Some(sequence) = ingress.sequence.checked_add(1) else {
            self.shared.fail(WORKER_STOPPED);
            self.shared.wake(&self.raw);
            return Err(failure_message(WORKER_STOPPED).into());
        };
        ingress.sequence = sequence;
        // This exact prefix is emitted by the bridge's event-first envelope.
        // It is only a routing hint; the worker still validates the entire JSON.
        if message.starts_with(r#"{"event":"presence","#) {
            let was_empty = ingress.presence.replace((sequence, message)).is_none();
            if was_empty {
                self.shared.wake(&self.raw);
            }
            return Ok(());
        }
        if self
            .shared
            .raw_messages
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |count| {
                (count < RAW_CAPACITY).then_some(count + 1)
            })
            .is_err()
        {
            self.shared.fail(RAW_OVERFLOW);
            return Err(failure_message(RAW_OVERFLOW).into());
        }
        match self.raw.try_send(Raw::Message { sequence, message }) {
            Ok(()) => {
                ingress.reliable_sequence = sequence;
                Ok(())
            }
            Err(TrySendError::Full(_)) => {
                self.shared.raw_messages.fetch_sub(1, Ordering::AcqRel);
                self.shared.fail(RAW_OVERFLOW);
                Err(failure_message(RAW_OVERFLOW).into())
            }
            Err(TrySendError::Disconnected(_)) => {
                self.shared.raw_messages.fetch_sub(1, Ordering::AcqRel);
                self.shared.fail(WORKER_STOPPED);
                Err(failure_message(WORKER_STOPPED).into())
            }
        }
    }
}

pub(crate) struct Inbox {
    raw: SyncSender<Raw>,
    typed: Receiver<Typed>,
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

    fn channels(context: &egui::Context) -> (Self, Receiver<Raw>, SyncSender<Typed>) {
        let (raw_sender, raw_receiver) = mpsc::sync_channel(RAW_CAPACITY + 1);
        let (typed_sender, typed_receiver) = mpsc::sync_channel(TYPED_CAPACITY);
        let shared = Arc::new(Shared {
            senders: AtomicUsize::new(0),
            senders_closed: AtomicBool::new(false),
            receiver_alive: AtomicBool::new(true),
            failure: AtomicU8::new(0),
            repaint: RepaintTarget::current(context),
            ingress: Mutex::default(),
            raw_messages: AtomicUsize::new(0),
            wake_pending: AtomicBool::new(false),
            typed: Mutex::default(),
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
            Ok((_, result)) => {
                self.shared.typed.lock().unwrap().reliable_pending -= 1;
                Some(result)
            }
            Err(TryRecvError::Empty) => self.poll_presence(),
            Err(TryRecvError::Disconnected) => {
                self.stopped = true;
                Some(Err(
                    "The Overleaf message decoder stopped. Reconnect to continue.".into(),
                ))
            }
        }
    }
    fn poll_presence(&self) -> Option<Decoded> {
        let mut typed = self.shared.typed.lock().unwrap();
        // A reliable message can arrive between try_recv(Empty) and this lock.
        // Its reservation and enqueue share this lock, so it still wins.
        if typed.reliable_pending != 0 {
            return None;
        }
        typed.presence.take().map(|(_, event)| Ok(event))
    }
}
impl Drop for Inbox {
    fn drop(&mut self) {
        self.shared.receiver_alive.store(false, Ordering::Release);
        self.shared.senders_closed.store(true, Ordering::Release);
        self.shared.wake(&self.raw);
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
    typed: SyncSender<Typed>,
    shared: Arc<Shared>,
    mut decode: impl FnMut(&str) -> Decoded,
) {
    let mut reliable_sequence = 0;
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
        let message = match next {
            Raw::Message { sequence, message } => {
                shared.raw_messages.fetch_sub(1, Ordering::AcqRel);
                reliable_sequence = sequence;
                Some((sequence, message))
            }
            Raw::Wake => {
                shared.wake_pending.store(false, Ordering::Release);
                None
            }
        };
        if let Some((sequence, message)) = message
            && !deliver_decoded(sequence, decode(&message), &typed, &shared)
        {
            return;
        }
        let presence = {
            let mut ingress = shared.ingress.lock().unwrap();
            // A Wake can precede a Project/ACK still waiting in the channel.
            // Never publish its later presence snapshot ahead of that state.
            (ingress.reliable_sequence <= reliable_sequence)
                .then(|| ingress.presence.take())
                .flatten()
        };
        if let Some((sequence, message)) = presence
            && !deliver_decoded(sequence, decode(&message), &typed, &shared)
        {
            return;
        }
    }
}

fn deliver_decoded(
    sequence: u64,
    decoded: Decoded,
    typed: &SyncSender<Typed>,
    shared: &Shared,
) -> bool {
    if !shared.receiver_alive.load(Ordering::Acquire) || shared.failure.load(Ordering::Acquire) != 0
    {
        return false;
    }
    let mut state = shared.typed.lock().unwrap();
    if decoded.is_err()
        || matches!(&decoded, Ok(BrowserEvent::Status { state, .. }) if !matches!(state.as_str(), "connected" | "saving"))
        || matches!(&decoded, Ok(BrowserEvent::Error { .. }))
    {
        // Discard older snapshots, but preserve a reconnect snapshot accepted
        // AFTER this barrier while it waited in the reliable queue.
        let mut ingress = shared.ingress.lock().unwrap();
        if ingress
            .presence
            .as_ref()
            .is_some_and(|(stamp, _)| *stamp <= sequence)
        {
            ingress.presence.take();
        }
        if state
            .presence
            .as_ref()
            .is_some_and(|(stamp, _)| *stamp <= sequence)
        {
            state.presence.take();
        }
    }
    if let Ok(event @ BrowserEvent::Presence { .. }) = decoded {
        let previous = state.presence.replace((sequence, event));
        drop(state);
        if previous.is_none() {
            shared.repaint.request_repaint();
        }
        return true;
    }
    match typed.try_send((sequence, decoded)) {
        Ok(()) => {
            state.reliable_pending += 1;
            drop(state);
            shared.repaint.request_repaint();
            true
        }
        Err(TrySendError::Full(_)) => {
            shared.fail(TYPED_OVERFLOW);
            false
        }
        Err(TrySendError::Disconnected(_)) => false,
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

    fn presence_message(epoch: u64) -> String {
        format!(
            r#"{{"event":"presence","epoch":{epoch},"own_id":null,"self_user_id":null,"peers":[],"documents":[]}}"#
        )
    }

    #[test]
    fn raw_presence_bursts_keep_only_latest_and_reserve_all_reliable_slots() {
        let (inbox, raw, _typed) = Inbox::channels(&egui::Context::default());
        let sender = inbox.sender();
        for epoch in 0..1000 {
            sender.send(presence_message(epoch)).unwrap();
        }
        for epoch in 0..RAW_CAPACITY as u64 {
            sender.send(message(epoch)).unwrap();
        }
        assert_eq!(inbox.shared.failure.load(Ordering::Acquire), 0);
        assert_eq!(
            inbox
                .shared
                .ingress
                .lock()
                .unwrap()
                .presence
                .as_ref()
                .map(|(_, s)| s.as_str()),
            Some(presence_message(999).as_str())
        );
        let queued: Vec<_> = raw.try_iter().collect();
        assert_eq!(queued.len(), RAW_CAPACITY + 1);
        assert_eq!(
            queued
                .iter()
                .filter(|item| matches!(item, Raw::Wake))
                .count(),
            1
        );
        assert_eq!(
            queued
                .iter()
                .filter(|item| matches!(item, Raw::Message { .. }))
                .count(),
            RAW_CAPACITY
        );
    }

    #[test]
    fn decoded_presence_never_overflows_or_reorders_reliable_messages() {
        let (mut inbox, _raw, typed) = Inbox::channels(&egui::Context::default());
        for epoch in 0..1000 {
            assert!(deliver_decoded(
                epoch,
                decode_message(&presence_message(epoch)),
                &typed,
                &inbox.shared
            ));
        }
        for epoch in 0..TYPED_CAPACITY as u64 {
            assert!(deliver_decoded(
                1000 + epoch,
                decode_message(&message(epoch)),
                &typed,
                &inbox.shared
            ));
        }
        for epoch in 0..TYPED_CAPACITY as u64 {
            assert!(
                matches!(inbox.poll().unwrap().unwrap(), BrowserEvent::Status { epoch: actual, .. } if actual == epoch)
            );
        }
        assert!(matches!(
            inbox.poll().unwrap().unwrap(),
            BrowserEvent::Presence { epoch: 999, .. }
        ));
        assert!(inbox.poll().is_none());
        assert_eq!(inbox.shared.failure.load(Ordering::Acquire), 0);
    }

    #[test]
    fn disconnect_discards_older_coalesced_presence() {
        let (mut inbox, _raw, typed) = Inbox::channels(&egui::Context::default());
        let sender = inbox.sender();
        sender.send(presence_message(1)).unwrap();
        assert!(deliver_decoded(
            1,
            decode_message(&presence_message(1)),
            &typed,
            &inbox.shared
        ));
        assert!(deliver_decoded(
            2,
            decode_message(r#"{"event":"status","epoch":1,"state":"disconnected"}"#),
            &typed,
            &inbox.shared
        ));
        assert!(inbox.shared.ingress.lock().unwrap().presence.is_none());
        assert!(
            matches!(inbox.poll().unwrap().unwrap(), BrowserEvent::Status { state, .. } if state == "disconnected")
        );
        assert!(inbox.poll().is_none());
    }

    #[test]
    fn reconnect_barrier_preserves_presence_accepted_after_the_new_connection() {
        let (mut inbox, raw, typed) = Inbox::channels(&egui::Context::default());
        let sender = inbox.sender();
        sender
            .send(r#"{"event":"status","epoch":7,"state":"disconnected"}"#.into())
            .unwrap();
        sender.send(message(7)).unwrap();
        sender.send(presence_message(7)).unwrap();
        let shared = Arc::clone(&inbox.shared);
        let worker = thread::spawn(move || decode_loop(raw, typed, shared, decode_message));
        assert!(
            matches!(next(&mut inbox).unwrap(),BrowserEvent::Status{state,..} if state=="disconnected")
        );
        assert!(
            matches!(next(&mut inbox).unwrap(),BrowserEvent::Status{state,..} if state=="connected")
        );
        assert!(matches!(
            next(&mut inbox).unwrap(),
            BrowserEvent::Presence { epoch: 7, .. }
        ));
        drop(sender);
        worker.join().unwrap();
    }

    #[test]
    fn pending_wake_cannot_decode_new_presence_before_its_queued_project_listing() {
        let (mut inbox, raw, typed) = Inbox::channels(&egui::Context::default());
        let sender = inbox.sender();
        // This old Wake remains first in the channel while its mailbox is
        // replaced by presence from AFTER the queued project publication.
        sender.send(presence_message(6)).unwrap();
        sender.send(r#"{"event":"project","epoch":7,"project_id":"0123456789abcdef01234567","name":"Paper","entries":[]}"#.into()).unwrap();
        sender.send(presence_message(7)).unwrap();
        let shared = Arc::clone(&inbox.shared);
        let observed = Arc::new(Mutex::new(Vec::new()));
        let seen = observed.clone();
        let worker = thread::spawn(move || {
            decode_loop(raw, typed, shared, |message| {
                let decoded = decode_message(message);
                seen.lock().unwrap().push(match &decoded {
                    Ok(BrowserEvent::Project { .. }) => "project",
                    Ok(BrowserEvent::Presence { .. }) => "presence",
                    _ => "unexpected",
                });
                decoded
            })
        });
        assert!(matches!(
            next(&mut inbox).unwrap(),
            BrowserEvent::Project { epoch: 7, .. }
        ));
        assert!(matches!(
            next(&mut inbox).unwrap(),
            BrowserEvent::Presence { epoch: 7, .. }
        ));
        assert_eq!(*observed.lock().unwrap(), vec!["project", "presence"]);
        drop(sender);
        worker.join().unwrap();
    }

    #[test]
    fn reliable_enqueue_between_empty_poll_and_mailbox_take_still_wins() {
        let (mut inbox, _raw, typed) = Inbox::channels(&egui::Context::default());
        assert!(deliver_decoded(
            1,
            decode_message(&presence_message(7)),
            &typed,
            &inbox.shared
        ));
        assert!(matches!(inbox.typed.try_recv(), Err(TryRecvError::Empty)));
        // Reproduce the race without timing: a reliable event arrives just
        // after poll observed Empty but before it takes the presence mailbox.
        assert!(deliver_decoded(
            2,
            decode_message(&message(8)),
            &typed,
            &inbox.shared
        ));
        assert!(inbox.poll_presence().is_none());
        assert!(matches!(
            inbox.poll().unwrap().unwrap(),
            BrowserEvent::Status { epoch: 8, .. }
        ));
        assert!(matches!(
            inbox.poll().unwrap().unwrap(),
            BrowserEvent::Presence { epoch: 7, .. }
        ));
        assert_eq!(inbox.shared.typed.lock().unwrap().reliable_pending, 0);
    }

    #[test]
    fn coalesced_presence_still_requires_worker_validation() {
        let mut inbox = Inbox::new(&egui::Context::default());
        let sender = inbox.sender();
        sender
            .send(r#"{"event":"presence","epoch":3,"peers":"private text"}"#.into())
            .unwrap();
        let error = next(&mut inbox).unwrap_err();
        assert!(error.contains("invalid bridge message"));
        assert!(!error.contains("private"));
        sender.send(presence_message(4)).unwrap();
        assert!(matches!(
            next(&mut inbox).unwrap(),
            BrowserEvent::Presence { epoch: 4, .. }
        ));
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
