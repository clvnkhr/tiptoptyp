//! Shared immutable workspace snapshots and coalesced filesystem observation.
use std::{
    collections::{BTreeSet, HashMap},
    path::{Path, PathBuf},
    sync::{Arc, LazyLock, Mutex, mpsc},
    thread,
    time::{Duration, Instant},
};

use notify::{Event, EventKind, RecursiveMode, Watcher, event::ModifyKind};
use tiptoptyp_core::document::WindowSessionId;

use crate::{worker::RepaintTarget, workspace::WorkspaceSnapshot};

const EVENT_QUIET_PERIOD: Duration = Duration::from_millis(120);
const VERIFICATION_INTERVAL: Duration = Duration::from_secs(30);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum EventImpact {
    Ignore,
    Content,
    Structure,
}

fn event_impact(kind: &EventKind) -> EventImpact {
    match kind {
        EventKind::Access(_) => EventImpact::Ignore,
        EventKind::Modify(ModifyKind::Data(_) | ModifyKind::Metadata(_)) => EventImpact::Content,
        EventKind::Create(_)
        | EventKind::Remove(_)
        | EventKind::Modify(ModifyKind::Name(_) | ModifyKind::Any | ModifyKind::Other)
        | EventKind::Any
        | EventKind::Other => EventImpact::Structure,
    }
}

#[derive(Debug)]
pub(crate) enum WorkspaceEvent {
    Snapshot {
        snapshot: Arc<WorkspaceSnapshot>,
        scan_serial: u64,
    },
    PathsChanged(Vec<PathBuf>),
    VerifyActiveFile,
    Error(String),
}

struct Subscriber {
    events: mpsc::Sender<WorkspaceEvent>,
    repaint: RepaintTarget,
}

enum RootCommand {
    Subscribe {
        owner: WindowSessionId,
        subscriber: Subscriber,
    },
    Unsubscribe(WindowSessionId),
    Refresh,
    Notification(notify::Result<Event>),
    Shutdown,
}

#[derive(Clone)]
struct RootHandle {
    commands: mpsc::Sender<RootCommand>,
    subscribers: usize,
}

#[derive(Default)]
struct Service {
    roots: HashMap<PathBuf, RootHandle>,
}

impl Service {
    fn subscribe(
        &mut self,
        root: PathBuf,
        owner: WindowSessionId,
        subscriber: Subscriber,
    ) -> Result<(), String> {
        if let Some(handle) = self.roots.get_mut(&root) {
            handle
                .commands
                .send(RootCommand::Subscribe { owner, subscriber })
                .map_err(|_| "workspace observer stopped unexpectedly".to_owned())?;
            handle.subscribers += 1;
            return Ok(());
        }
        let (commands, receiver) = mpsc::channel();
        let thread_commands = commands.clone();
        let thread_root = root.clone();
        thread::Builder::new()
            .name("tiptoptyp-workspace-observer".to_owned())
            .spawn(move || root_worker(thread_root, thread_commands, receiver, owner, subscriber))
            .map_err(|error| format!("could not start workspace observer: {error}"))?;
        self.roots.insert(
            root,
            RootHandle {
                commands,
                subscribers: 1,
            },
        );
        Ok(())
    }

    fn unsubscribe(&mut self, root: &Path, owner: WindowSessionId) {
        let mut remove = false;
        if let Some(handle) = self.roots.get_mut(root) {
            let _ = handle.commands.send(RootCommand::Unsubscribe(owner));
            handle.subscribers = handle.subscribers.saturating_sub(1);
            remove = handle.subscribers == 0;
            if remove {
                let _ = handle.commands.send(RootCommand::Shutdown);
            }
        }
        if remove {
            self.roots.remove(root);
        }
    }

    fn refresh(&self, root: &Path) {
        if let Some(handle) = self.roots.get(root) {
            let _ = handle.commands.send(RootCommand::Refresh);
        }
    }
}

static SERVICE: LazyLock<Mutex<Service>> = LazyLock::new(|| Mutex::new(Service::default()));

fn root_worker(
    root: PathBuf,
    commands: mpsc::Sender<RootCommand>,
    receiver: mpsc::Receiver<RootCommand>,
    first_owner: WindowSessionId,
    first_subscriber: Subscriber,
) {
    let watcher_commands = commands.clone();
    let mut watcher = notify::recommended_watcher(move |event| {
        let _ = watcher_commands.send(RootCommand::Notification(event));
    });
    if let Ok(watcher) = &mut watcher {
        let _ = watcher.watch(&root, RecursiveMode::Recursive);
    }

    let mut subscribers = HashMap::from([(first_owner, first_subscriber)]);
    let mut snapshot = None::<Arc<WorkspaceSnapshot>>;
    let mut scan_serial = 0_u64;
    scan_and_publish(&root, &mut snapshot, &mut scan_serial, &subscribers, true);
    if let Err(error) = &watcher {
        publish(&subscribers, || {
            WorkspaceEvent::Error(format!("Could not watch workspace: {error}"))
        });
    }

    let mut changed_paths = BTreeSet::new();
    let mut structural_change = false;
    let mut flush_at = None::<Instant>;
    let mut verify_at = Instant::now() + VERIFICATION_INTERVAL;
    loop {
        let now = Instant::now();
        let deadline = flush_at.map_or(verify_at, |flush| flush.min(verify_at));
        let timeout = deadline.saturating_duration_since(now);
        match receiver.recv_timeout(timeout) {
            Ok(RootCommand::Subscribe { owner, subscriber }) => {
                if let Some(snapshot) = snapshot.clone() {
                    let _ = subscriber.events.send(WorkspaceEvent::Snapshot {
                        snapshot,
                        scan_serial,
                    });
                    subscriber.repaint.request_repaint();
                }
                subscribers.insert(owner, subscriber);
            }
            Ok(RootCommand::Unsubscribe(owner)) => {
                subscribers.remove(&owner);
            }
            Ok(RootCommand::Refresh) => {
                publish(&subscribers, || WorkspaceEvent::VerifyActiveFile);
                structural_change = true;
                flush_at = Some(Instant::now() + EVENT_QUIET_PERIOD);
            }
            Ok(RootCommand::Notification(Ok(mut event))) => {
                let impact = event_impact(&event.kind);
                event.paths.retain(|path| observed_path(&root, path));
                if impact != EventImpact::Ignore && !event.paths.is_empty() {
                    structural_change |= impact == EventImpact::Structure;
                    changed_paths.extend(event.paths);
                    flush_at = Some(Instant::now() + EVENT_QUIET_PERIOD);
                }
            }
            Ok(RootCommand::Notification(Err(error))) => {
                publish(&subscribers, || {
                    WorkspaceEvent::Error(format!("Workspace notification failed: {error}"))
                });
                structural_change = true;
                flush_at = Some(Instant::now() + EVENT_QUIET_PERIOD);
            }
            Ok(RootCommand::Shutdown) | Err(mpsc::RecvTimeoutError::Disconnected) => break,
            Err(mpsc::RecvTimeoutError::Timeout) => {}
        }

        let now = Instant::now();
        if flush_at.is_some_and(|deadline| deadline <= now) {
            let paths = std::mem::take(&mut changed_paths)
                .into_iter()
                .collect::<Vec<_>>();
            if !paths.is_empty() {
                publish(&subscribers, || WorkspaceEvent::PathsChanged(paths.clone()));
            }
            if std::mem::take(&mut structural_change) {
                scan_and_publish(&root, &mut snapshot, &mut scan_serial, &subscribers, false);
            }
            flush_at = None;
        }
        if verify_at <= now {
            publish(&subscribers, || WorkspaceEvent::VerifyActiveFile);
            scan_and_publish(&root, &mut snapshot, &mut scan_serial, &subscribers, false);
            verify_at = now + VERIFICATION_INTERVAL;
        }
    }
}

fn observed_path(root: &Path, path: &Path) -> bool {
    let relative = path.strip_prefix(root).unwrap_or(path);
    !relative.components().any(|component| {
        matches!(
            component.as_os_str().to_str(),
            Some(".git" | "target" | ".tiptoptyp")
        )
    })
}

fn scan_and_publish(
    root: &Path,
    current: &mut Option<Arc<WorkspaceSnapshot>>,
    serial: &mut u64,
    subscribers: &HashMap<WindowSessionId, Subscriber>,
    publish_unchanged: bool,
) {
    *serial = serial.wrapping_add(1);
    match WorkspaceSnapshot::scan(root) {
        Ok(next) => {
            let unchanged = current
                .as_ref()
                .is_some_and(|snapshot| snapshot.as_ref() == &next);
            if unchanged && !publish_unchanged {
                return;
            }
            let next = if unchanged {
                current.as_ref().unwrap().clone()
            } else {
                Arc::new(next)
            };
            *current = Some(next.clone());
            publish(subscribers, || WorkspaceEvent::Snapshot {
                snapshot: next.clone(),
                scan_serial: *serial,
            });
        }
        Err(error) => publish(subscribers, || {
            WorkspaceEvent::Error(format!("Could not scan workspace: {error}"))
        }),
    }
}

fn publish(subscribers: &HashMap<WindowSessionId, Subscriber>, event: impl Fn() -> WorkspaceEvent) {
    for subscriber in subscribers.values() {
        if subscriber.events.send(event()).is_ok() {
            subscriber.repaint.request_repaint();
        }
    }
}

pub(crate) struct WorkspaceClient {
    owner: WindowSessionId,
    repaint: RepaintTarget,
    root: Option<PathBuf>,
    sender: mpsc::Sender<WorkspaceEvent>,
    events: mpsc::Receiver<WorkspaceEvent>,
}

impl WorkspaceClient {
    pub(crate) fn new(owner: WindowSessionId, repaint: RepaintTarget) -> Self {
        let (sender, events) = mpsc::channel();
        Self {
            owner,
            repaint,
            root: None,
            sender,
            events,
        }
    }

    pub(crate) fn subscribe(&mut self, root: &Path) -> Result<(), String> {
        let root = root
            .canonicalize()
            .map_err(|error| format!("Could not use workspace {}: {error}", root.display()))?;
        if self.root.as_ref() == Some(&root) {
            SERVICE.lock().unwrap().refresh(&root);
            return Ok(());
        }
        self.unsubscribe();
        SERVICE.lock().unwrap().subscribe(
            root.clone(),
            self.owner,
            Subscriber {
                events: self.sender.clone(),
                repaint: self.repaint.clone(),
            },
        )?;
        self.root = Some(root);
        Ok(())
    }

    pub(crate) fn refresh(&self) {
        if let Some(root) = &self.root {
            SERVICE.lock().unwrap().refresh(root);
        }
    }

    pub(crate) fn is_subscribed(&self) -> bool {
        self.root.is_some()
    }

    pub(crate) fn poll(&self) -> Option<WorkspaceEvent> {
        self.events.try_recv().ok()
    }

    pub(crate) fn unsubscribe(&mut self) {
        if let Some(root) = self.root.take() {
            SERVICE.lock().unwrap().unsubscribe(&root, self.owner);
        }
        while self.events.try_recv().is_ok() {}
    }

    #[cfg(test)]
    pub(crate) fn is_running(&self) -> bool {
        self.root.is_some()
    }
}

impl Drop for WorkspaceClient {
    fn drop(&mut self) {
        self.unsubscribe();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use notify::event::{CreateKind, DataChange, ModifyKind, RemoveKind, RenameMode};

    fn next_snapshot(client: &WorkspaceClient) -> (Arc<WorkspaceSnapshot>, u64) {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            if let Some(WorkspaceEvent::Snapshot {
                snapshot,
                scan_serial,
            }) = client.poll()
            {
                return (snapshot, scan_serial);
            }
            assert!(Instant::now() < deadline, "workspace snapshot timed out");
            thread::yield_now();
        }
    }

    #[test]
    fn two_windows_reuse_one_immutable_snapshot_and_survive_one_close() {
        let project = tempfile::tempdir().unwrap();
        std::fs::write(project.path().join("main.typ"), "= Main").unwrap();
        let mut first = WorkspaceClient::new(WindowSessionId::new(81), RepaintTarget::test());
        let mut second = WorkspaceClient::new(WindowSessionId::new(82), RepaintTarget::test());
        first.subscribe(project.path()).unwrap();
        let (first_snapshot, first_serial) = next_snapshot(&first);
        second.subscribe(project.path()).unwrap();
        let (second_snapshot, second_serial) = next_snapshot(&second);
        assert!(Arc::ptr_eq(&first_snapshot, &second_snapshot));
        assert_eq!(first_serial, second_serial);

        drop(first);
        std::fs::write(project.path().join("new.typ"), "= New").unwrap();
        second.refresh();
        let (updated, updated_serial) = next_snapshot(&second);
        assert!(updated.find("new.typ").is_some());
        assert!(updated_serial > second_serial);
    }

    #[test]
    fn event_classification_filters_content_but_keeps_atomic_save_delete_and_rename() {
        assert_eq!(
            event_impact(&EventKind::Modify(ModifyKind::Data(DataChange::Content))),
            EventImpact::Content
        );
        for kind in [
            EventKind::Create(CreateKind::File),
            EventKind::Remove(RemoveKind::File),
            EventKind::Modify(ModifyKind::Name(RenameMode::Both)),
        ] {
            assert_eq!(event_impact(&kind), EventImpact::Structure);
        }
    }
}
