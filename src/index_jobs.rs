//! Process-wide, bounded execution for replaceable project-index reads.
//!
//! This is deliberately not a general executor: the only admitted payload is
//! a project-index request, and protected mutations and protocol supervisors
//! remain on their existing workers.
use std::{
    collections::{HashMap, VecDeque},
    path::PathBuf,
    sync::{
        Arc, Condvar, LazyLock, Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc::{self, Receiver, Sender, TryRecvError},
    },
    thread,
};

use tiptoptyp_core::document::WindowSessionId;

use crate::{project_index::ProjectIndex, worker::RepaintTarget};

const CONCURRENCY: usize = 2;
const MAX_PENDING_BYTES: usize = 64 * 1024 * 1024;
const MAX_REQUEST_BYTES: usize = 32 * 1024 * 1024;

#[derive(Debug)]
pub(crate) struct ProjectIndexInput {
    pub(crate) root: PathBuf,
    pub(crate) main: PathBuf,
    pub(crate) overrides: std::collections::BTreeMap<PathBuf, String>,
}

impl ProjectIndexInput {
    fn payload_bytes(&self) -> usize {
        self.root.as_os_str().as_encoded_bytes().len()
            + self.main.as_os_str().as_encoded_bytes().len()
            + self
                .overrides
                .iter()
                .map(|(path, source)| {
                    path.as_os_str()
                        .as_encoded_bytes()
                        .len()
                        .saturating_add(source.len())
                })
                .sum::<usize>()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct JobKey {
    owner: WindowSessionId,
    source: PathBuf,
}

struct Job {
    key: JobKey,
    request: u64,
    bytes: usize,
    input: ProjectIndexInput,
    cancelled: Arc<AtomicBool>,
    completion: Sender<Completion>,
    repaint: RepaintTarget,
}

struct ActiveJob {
    request: u64,
    cancelled: Arc<AtomicBool>,
}

#[derive(Default)]
struct Queue {
    order: VecDeque<JobKey>,
    pending: HashMap<JobKey, Job>,
    active: HashMap<JobKey, ActiveJob>,
    pending_bytes: usize,
}

impl Queue {
    fn cancel_owner(&mut self, owner: WindowSessionId) {
        let pending = self
            .pending
            .keys()
            .filter(|key| key.owner == owner)
            .cloned()
            .collect::<Vec<_>>();
        for key in pending {
            if let Some(job) = self.pending.remove(&key) {
                self.pending_bytes = self.pending_bytes.saturating_sub(job.bytes);
                job.cancelled.store(true, Ordering::Release);
            }
        }
        self.order.retain(|key| key.owner != owner);
        for (key, active) in &self.active {
            if key.owner == owner {
                active.cancelled.store(true, Ordering::Release);
            }
        }
    }

    fn push(&mut self, job: Job) -> Result<(), String> {
        if job.bytes > MAX_REQUEST_BYTES {
            return Err(format!(
                "project index input is too large ({} bytes; limit {MAX_REQUEST_BYTES})",
                job.bytes
            ));
        }
        let replaced_bytes = self
            .pending
            .get(&job.key)
            .map_or(0, |replaced| replaced.bytes);
        let projected_bytes = self
            .pending_bytes
            .saturating_sub(replaced_bytes)
            .saturating_add(job.bytes);
        if projected_bytes > MAX_PENDING_BYTES {
            return Err(format!(
                "project index queue is full ({} byte limit)",
                MAX_PENDING_BYTES
            ));
        }
        if let Some(active) = self.active.get(&job.key) {
            active.cancelled.store(true, Ordering::Release);
        }
        if let Some(replaced) = self.pending.remove(&job.key) {
            self.pending_bytes = self.pending_bytes.saturating_sub(replaced.bytes);
            replaced.cancelled.store(true, Ordering::Release);
        } else {
            self.order.push_back(job.key.clone());
        }
        self.pending_bytes = self.pending_bytes.saturating_add(job.bytes);
        self.pending.insert(job.key.clone(), job);
        Ok(())
    }

    /// Atomically replace all queued/running work for one window. Admission is
    /// checked before any existing work is cancelled, so a rejected request
    /// cannot destroy the last usable index update for that window.
    fn replace_owner(&mut self, job: Job) -> Result<(), String> {
        if job.bytes > MAX_REQUEST_BYTES {
            return Err(format!(
                "project index input is too large ({} bytes; limit {MAX_REQUEST_BYTES})",
                job.bytes
            ));
        }
        let reclaimable = self
            .pending
            .iter()
            .filter(|(key, _)| key.owner == job.key.owner)
            .map(|(_, pending)| pending.bytes)
            .sum::<usize>();
        let projected_bytes = self
            .pending_bytes
            .saturating_sub(reclaimable)
            .saturating_add(job.bytes);
        if projected_bytes > MAX_PENDING_BYTES {
            return Err(format!(
                "project index queue is full ({} byte limit)",
                MAX_PENDING_BYTES
            ));
        }
        self.cancel_owner(job.key.owner);
        self.push(job)
    }

    fn pop(&mut self) -> Option<Job> {
        while let Some(key) = self.order.pop_front() {
            let Some(job) = self.pending.remove(&key) else {
                continue;
            };
            self.pending_bytes = self.pending_bytes.saturating_sub(job.bytes);
            self.active.insert(
                key,
                ActiveJob {
                    request: job.request,
                    cancelled: job.cancelled.clone(),
                },
            );
            return Some(job);
        }
        None
    }

    fn finish(&mut self, key: &JobKey, request: u64) {
        if self
            .active
            .get(key)
            .is_some_and(|active| active.request == request)
        {
            self.active.remove(key);
        }
    }
}

#[derive(Debug)]
struct Completion {
    owner: WindowSessionId,
    source: PathBuf,
    request: u64,
    output: Result<ProjectIndex, String>,
}

struct Runner {
    queue: Arc<(Mutex<Queue>, Condvar)>,
}

impl Runner {
    fn new() -> Self {
        let queue = Arc::new((Mutex::new(Queue::default()), Condvar::new()));
        for index in 0..CONCURRENCY {
            let queue = queue.clone();
            thread::Builder::new()
                .name(format!("tiptoptyp-index-read-{index}"))
                .spawn(move || worker_loop(&queue))
                .expect("project-index worker must start");
        }
        Self { queue }
    }

    fn submit(&self, job: Job) -> Result<(), String> {
        let (queue, ready) = &*self.queue;
        queue.lock().unwrap().replace_owner(job)?;
        ready.notify_one();
        Ok(())
    }

    fn cancel_owner(&self, owner: WindowSessionId) {
        self.queue.0.lock().unwrap().cancel_owner(owner);
    }
}

fn worker_loop(shared: &Arc<(Mutex<Queue>, Condvar)>) {
    loop {
        let job = {
            let (queue, ready) = &**shared;
            let mut queue = queue.lock().unwrap();
            while queue.order.is_empty() {
                queue = ready.wait(queue).unwrap();
            }
            queue.pop().expect("non-empty order has a pending job")
        };
        let output = crate::project_index::analyze_project_cancellable(
            &job.input.root,
            &job.input.main,
            &job.input.overrides,
            || job.cancelled.load(Ordering::Acquire),
        )
        .ok_or_else(|| "project indexing was superseded".to_owned());
        let cancelled = job.cancelled.load(Ordering::Acquire);
        {
            let mut queue = shared.0.lock().unwrap();
            queue.finish(&job.key, job.request);
        }
        if !cancelled
            && job
                .completion
                .send(Completion {
                    owner: job.key.owner,
                    source: job.key.source,
                    request: job.request,
                    output,
                })
                .is_ok()
        {
            job.repaint.request_repaint();
        }
    }
}

static RUNNER: LazyLock<Runner> = LazyLock::new(Runner::new);

pub(crate) enum Poll {
    Idle,
    Pending,
    Ready(ProjectIndex),
    Failed(String),
}

pub(crate) struct ProjectIndexClient {
    owner: WindowSessionId,
    latest_source: Option<PathBuf>,
    latest_request: u64,
    next_request: AtomicU64,
    completion_tx: Sender<Completion>,
    completions: Receiver<Completion>,
    pending: bool,
}

impl ProjectIndexClient {
    pub(crate) fn new(owner: WindowSessionId) -> Self {
        let (completion_tx, completions) = mpsc::channel();
        Self {
            owner,
            latest_source: None,
            latest_request: 0,
            next_request: AtomicU64::new(1),
            completion_tx,
            completions,
            pending: false,
        }
    }

    pub(crate) fn request(
        &mut self,
        input: ProjectIndexInput,
        repaint: RepaintTarget,
    ) -> Result<(), String> {
        let source = input.main.clone();
        let request = self.next_request.fetch_add(1, Ordering::Relaxed);
        let bytes = input.payload_bytes();
        RUNNER.submit(Job {
            key: JobKey {
                owner: self.owner,
                source: source.clone(),
            },
            request,
            bytes,
            input,
            cancelled: Arc::new(AtomicBool::new(false)),
            completion: self.completion_tx.clone(),
            repaint,
        })?;
        self.latest_source = Some(source);
        self.latest_request = request;
        self.pending = true;
        Ok(())
    }

    pub(crate) fn poll(&mut self) -> Poll {
        loop {
            match self.completions.try_recv() {
                Ok(completion)
                    if completion.owner == self.owner
                        && completion.request == self.latest_request
                        && self.latest_source.as_ref() == Some(&completion.source) =>
                {
                    self.pending = false;
                    return match completion.output {
                        Ok(index) => Poll::Ready(index),
                        Err(error) => Poll::Failed(error),
                    };
                }
                Ok(_) => continue,
                Err(TryRecvError::Empty) => {
                    return if self.pending {
                        Poll::Pending
                    } else {
                        Poll::Idle
                    };
                }
                Err(TryRecvError::Disconnected) => {
                    self.pending = false;
                    return Poll::Failed("project-index runner stopped unexpectedly".to_owned());
                }
            }
        }
    }

    pub(crate) fn supersede(&mut self) {
        RUNNER.cancel_owner(self.owner);
        self.pending = false;
        self.latest_source = None;
        while self.completions.try_recv().is_ok() {}
    }

    pub(crate) fn is_running(&self) -> bool {
        self.pending
    }
}

impl Drop for ProjectIndexClient {
    fn drop(&mut self) {
        RUNNER.cancel_owner(self.owner);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    fn job(owner: u64, source: &str, request: u64, bytes: usize) -> Job {
        let (completion, _) = mpsc::channel();
        Job {
            key: JobKey {
                owner: WindowSessionId::new(owner),
                source: source.into(),
            },
            request,
            bytes,
            input: ProjectIndexInput {
                root: PathBuf::from("/project"),
                main: source.into(),
                overrides: Default::default(),
            },
            cancelled: Arc::new(AtomicBool::new(false)),
            completion,
            repaint: RepaintTarget::test(),
        }
    }

    #[test]
    fn one_hundred_superseding_requests_leave_one_pending_job() {
        let mut queue = Queue::default();
        for request in 1..=100 {
            queue.push(job(1, "main.typ", request, 128)).unwrap();
        }
        assert_eq!(queue.pending.len(), 1);
        assert_eq!(queue.order.len(), 1);
        assert_eq!(queue.pending_bytes, 128);
        assert_eq!(queue.pop().unwrap().request, 100);
    }

    #[test]
    fn admission_is_fifo_between_source_keys_and_owner_close_cancels_work() {
        let mut queue = Queue::default();
        queue.push(job(1, "a.typ", 1, 20)).unwrap();
        queue.push(job(2, "b.typ", 2, 30)).unwrap();
        queue.push(job(3, "c.typ", 3, 40)).unwrap();
        assert_eq!(queue.pop().unwrap().key.source, PathBuf::from("a.typ"));
        let active = queue.active.values().next().unwrap().cancelled.clone();
        queue.cancel_owner(WindowSessionId::new(1));
        assert!(active.load(Ordering::Acquire));
        assert_eq!(queue.pop().unwrap().key.source, PathBuf::from("b.typ"));
        assert_eq!(queue.pop().unwrap().key.source, PathBuf::from("c.typ"));
        assert_eq!(queue.pending_bytes, 0);
    }

    #[test]
    fn payload_limits_are_enforced_before_work_starts() {
        let mut queue = Queue::default();
        let error = queue
            .push(job(1, "large.typ", 1, MAX_REQUEST_BYTES + 1))
            .unwrap_err();
        assert!(error.contains("too large"));
        assert!(queue.pending.is_empty());
        assert!(queue.active.is_empty());
    }

    #[test]
    fn rejected_same_key_replacement_preserves_the_queued_job() {
        let mut queue = Queue::default();
        queue.push(job(1, "main.typ", 1, 1)).unwrap();
        queue
            .push(job(2, "other.typ", 2, MAX_REQUEST_BYTES))
            .unwrap();
        queue
            .push(job(
                3,
                "third.typ",
                3,
                MAX_PENDING_BYTES - MAX_REQUEST_BYTES - 1,
            ))
            .unwrap();

        assert!(queue.push(job(1, "main.typ", 4, 2)).is_err());
        assert_eq!(
            queue.pending[&JobKey {
                owner: WindowSessionId::new(1),
                source: PathBuf::from("main.typ"),
            }]
                .request,
            1
        );
        assert_eq!(queue.pending_bytes, MAX_PENDING_BYTES);
    }

    #[test]
    fn rejected_owner_replacement_does_not_cancel_active_work() {
        let mut queue = Queue::default();
        queue.push(job(1, "main.typ", 1, 1)).unwrap();
        let active = queue.pop().unwrap().cancelled;
        queue
            .push(job(2, "other.typ", 2, MAX_REQUEST_BYTES))
            .unwrap();
        queue
            .push(job(3, "third.typ", 3, MAX_REQUEST_BYTES))
            .unwrap();

        assert!(queue.replace_owner(job(1, "main.typ", 4, 1)).is_err());
        assert!(!active.load(Ordering::Acquire));
        assert_eq!(queue.pending_bytes, MAX_PENDING_BYTES);
    }

    #[test]
    #[ignore = "opt-in optimized comparison of superseded spawning and bounded multi-window indexing"]
    fn profile_repeated_multi_window_indexing() {
        let project = tempfile::tempdir().unwrap();
        let main = project.path().join("main.typ");
        let source = (0..2_000)
            .map(|index| format!("= Section {index}\n#let value{index} = {index}\n"))
            .collect::<String>();
        std::fs::write(&main, &source).unwrap();
        let input = || ProjectIndexInput {
            root: project.path().to_owned(),
            main: main.clone(),
            overrides: std::collections::BTreeMap::from([(main.clone(), source.clone())]),
        };

        let baseline_started = Arc::new(AtomicU64::new(0));
        let baseline_finished = Arc::new(AtomicU64::new(0));
        let baseline_start = Instant::now();
        for _ in 0..100 {
            let started = baseline_started.clone();
            let finished = baseline_finished.clone();
            let input = input();
            thread::spawn(move || {
                started.fetch_add(1, Ordering::Relaxed);
                let _ = crate::project_index::analyze_project_cancellable(
                    &input.root,
                    &input.main,
                    &input.overrides,
                    || false,
                );
                finished.fetch_add(1, Ordering::Release);
            });
        }
        while baseline_finished.load(Ordering::Acquire) != 100 {
            thread::yield_now();
        }
        let baseline = baseline_start.elapsed();

        let bounded_start = Instant::now();
        let mut clients = (0..4)
            .map(|owner| ProjectIndexClient::new(WindowSessionId::new(10_000 + owner)))
            .collect::<Vec<_>>();
        for client in &mut clients {
            for _ in 0..25 {
                client
                    .request(input(), RepaintTarget::test())
                    .expect("bounded request");
            }
        }
        let deadline = Instant::now() + Duration::from_secs(30);
        let mut ready = vec![false; clients.len()];
        while ready.iter().any(|ready| !ready) && Instant::now() < deadline {
            for (index, client) in clients.iter_mut().enumerate() {
                match client.poll() {
                    Poll::Ready(_) => ready[index] = true,
                    Poll::Failed(error) => panic!("bounded index failed: {error}"),
                    Poll::Idle | Poll::Pending => {}
                }
            }
            thread::yield_now();
        }
        assert!(ready.into_iter().all(|ready| ready));
        let bounded = bounded_start.elapsed();
        println!(
            "project_index_repeated,multi_window=4,requests=100,baseline_threads={},baseline_ns={},bounded_workers={CONCURRENCY},bounded_ns={}",
            baseline_started.load(Ordering::Relaxed),
            baseline.as_nanos(),
            bounded.as_nanos(),
        );
    }
}
