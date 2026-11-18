//! Shared immutable workspace snapshots and coalesced filesystem observation.
use std::{
    collections::{BTreeSet, HashMap, HashSet},
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum QueuedImpact {
    Content,
    Structure,
}

struct ChangeBatch {
    paths: BTreeSet<PathBuf>,
    impact: QueuedImpact,
    deadline: Instant,
}

/// A quiet deadline always owns the exact paths and scan requirement it will
/// flush. Content events can upgrade the batch to structure but never demote it.
#[derive(Default)]
enum PendingChanges {
    #[default]
    Idle,
    Queued(ChangeBatch),
}

impl PendingChanges {
    fn deadline(&self) -> Option<Instant> {
        match self {
            Self::Idle => None,
            Self::Queued(batch) => Some(batch.deadline),
        }
    }

    /// Returns true only for a newly queued structural batch, preserving the
    /// existing activity announcement policy while coalescing later events.
    fn queue(
        &mut self,
        paths: impl IntoIterator<Item = PathBuf>,
        impact: QueuedImpact,
        deadline: Instant,
    ) -> bool {
        let first_structure = matches!(self, Self::Idle) && impact == QueuedImpact::Structure;
        match self {
            Self::Idle => {
                *self = Self::Queued(ChangeBatch {
                    paths: paths.into_iter().collect(),
                    impact,
                    deadline,
                })
            }
            Self::Queued(batch) => {
                batch.paths.extend(paths);
                if impact == QueuedImpact::Structure {
                    batch.impact = QueuedImpact::Structure;
                }
                batch.deadline = deadline;
            }
        }
        first_structure
    }

    fn take_due(&mut self, now: Instant) -> Option<ChangeBatch> {
        if !self.deadline().is_some_and(|deadline| deadline <= now) {
            return None;
        }
        let Self::Queued(batch) = std::mem::take(self) else {
            unreachable!()
        };
        Some(batch)
    }
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
    Activity(crate::activity::Activity),
    Watcher(crate::activity::Activity),
    Snapshot {
        snapshot: Arc<WorkspaceSnapshot>,
        scan_serial: u64,
    },
    PathsChanged(Vec<PathBuf>),
    VerifyActiveFile,
    Error(String),
}

struct Subscriber {
    events: mpsc::Sender<RoutedWorkspaceEvent>,
    repaint: RepaintTarget,
    subscription: u64,
}

struct RoutedWorkspaceEvent {
    subscription: u64,
    event: WorkspaceEvent,
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
    owners: HashSet<WindowSessionId>,
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
        mut subscriber: Subscriber,
    ) -> Result<(), String> {
        if let Some(handle) = self.roots.get_mut(&root) {
            let command = RootCommand::Subscribe { owner, subscriber };
            match handle.commands.send(command) {
                Ok(()) => {
                    handle.owners.insert(owner);
                    return Ok(());
                }
                Err(error) => {
                    let RootCommand::Subscribe {
                        subscriber: recovered,
                        ..
                    } = error.0
                    else {
                        unreachable!("only subscribe commands are sent here")
                    };
                    subscriber = recovered;
                }
            }
            self.roots.remove(&root);
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
                owners: HashSet::from([owner]),
            },
        );
        Ok(())
    }

    fn unsubscribe(&mut self, root: &Path, owner: WindowSessionId) {
        let mut remove = false;
        if let Some(handle) = self.roots.get_mut(root) {
            if handle.owners.remove(&owner) {
                let _ = handle.commands.send(RootCommand::Unsubscribe(owner));
            }
            remove = handle.owners.is_empty();
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
    let watcher = notify::recommended_watcher(move |event| {
        let _ = watcher_commands.send(RootCommand::Notification(event));
    });
    let (mut _watcher, watch_error) = match watcher {
        Ok(mut watcher) => match watcher.watch(&root, RecursiveMode::Recursive) {
            Ok(()) => (Some(watcher), None),
            Err(error) => (None, Some(format!("Could not watch workspace: {error}"))),
        },
        Err(error) => (None, Some(format!("Could not watch workspace: {error}"))),
    };

    let mut subscribers = HashMap::from([(first_owner, first_subscriber)]);
    let mut snapshot = None::<Arc<WorkspaceSnapshot>>;
    let mut scan_serial = 0_u64;
    scan_and_publish(&root, &mut snapshot, &mut scan_serial, &subscribers, true);
    let mut watcher_state = watch_error.map_or(
        crate::activity::Activity::Idle,
        crate::activity::Activity::Failed,
    );
    publish(&subscribers, || {
        WorkspaceEvent::Watcher(watcher_state.clone())
    });

    let mut changes = PendingChanges::default();
    let mut verify_at = Instant::now() + VERIFICATION_INTERVAL;
    loop {
        let now = Instant::now();
        let deadline = changes
            .deadline()
            .map_or(verify_at, |flush| flush.min(verify_at));
        let timeout = deadline.saturating_duration_since(now);
        match receiver.recv_timeout(timeout) {
            Ok(RootCommand::Subscribe { owner, subscriber }) => {
                if let Some(snapshot) = snapshot.clone() {
                    let _ = subscriber.events.send(RoutedWorkspaceEvent {
                        subscription: subscriber.subscription,
                        event: WorkspaceEvent::Snapshot {
                            snapshot,
                            scan_serial,
                        },
                    });
                    subscriber.repaint.request_repaint();
                }
                let _ = subscriber.events.send(RoutedWorkspaceEvent {
                    subscription: subscriber.subscription,
                    event: WorkspaceEvent::Watcher(watcher_state.clone()),
                });
                subscribers.insert(owner, subscriber);
            }
            Ok(RootCommand::Unsubscribe(owner)) => {
                subscribers.remove(&owner);
            }
            Ok(RootCommand::Refresh) => {
                publish(&subscribers, || WorkspaceEvent::VerifyActiveFile);
                changes.queue(
                    [],
                    QueuedImpact::Structure,
                    Instant::now() + EVENT_QUIET_PERIOD,
                );
            }
            Ok(RootCommand::Notification(Ok(mut event))) => {
                if matches!(watcher_state, crate::activity::Activity::Failed(_)) {
                    watcher_state = crate::activity::Activity::Idle;
                    publish(&subscribers, || {
                        WorkspaceEvent::Watcher(watcher_state.clone())
                    });
                }
                let impact = event_impact(&event.kind);
                event.paths.retain(|path| observed_path(&root, path));
                if impact != EventImpact::Ignore && !event.paths.is_empty() {
                    let queued_impact = match impact {
                        EventImpact::Content => QueuedImpact::Content,
                        EventImpact::Structure => QueuedImpact::Structure,
                        EventImpact::Ignore => {
                            unreachable!("ignored events are excluded before admission")
                        }
                    };
                    if changes.queue(
                        event.paths,
                        queued_impact,
                        Instant::now() + EVENT_QUIET_PERIOD,
                    ) {
                        publish(&subscribers, || {
                            WorkspaceEvent::Activity(crate::activity::Activity::Pending(
                                "Changes queued",
                            ))
                        });
                    }
                }
            }
            Ok(RootCommand::Notification(Err(error))) => {
                watcher_state = crate::activity::Activity::Failed(format!(
                    "Workspace notification failed: {error}"
                ));
                publish(&subscribers, || {
                    WorkspaceEvent::Watcher(watcher_state.clone())
                });
                changes.queue(
                    [],
                    QueuedImpact::Structure,
                    Instant::now() + EVENT_QUIET_PERIOD,
                );
            }
            Ok(RootCommand::Shutdown) | Err(mpsc::RecvTimeoutError::Disconnected) => break,
            Err(mpsc::RecvTimeoutError::Timeout) => {}
        }

        let now = Instant::now();
        if let Some(batch) = changes.take_due(now) {
            let paths = batch.paths.into_iter().collect::<Vec<_>>();
            if !paths.is_empty() {
                publish(&subscribers, || WorkspaceEvent::PathsChanged(paths.clone()));
            }
            if batch.impact == QueuedImpact::Structure {
                scan_and_publish(&root, &mut snapshot, &mut scan_serial, &subscribers, false);
            }
        }
        if verify_at <= now {
            publish(&subscribers, || WorkspaceEvent::VerifyActiveFile);
            scan_and_publish(&root, &mut snapshot, &mut scan_serial, &subscribers, false);
            verify_at = now + VERIFICATION_INTERVAL;
        }
    }
}

fn observed_path(root: &Path, path: &Path) -> bool {
    let Ok(relative) = path.strip_prefix(root) else {
        return false;
    };
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
    publish(subscribers, || {
        WorkspaceEvent::Activity(crate::activity::Activity::Running)
    });
    *serial = serial.wrapping_add(1);
    match WorkspaceSnapshot::scan(root) {
        Ok(next) => {
            publish(subscribers, || {
                WorkspaceEvent::Activity(crate::activity::Activity::Idle)
            });
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
        if subscriber
            .events
            .send(RoutedWorkspaceEvent {
                subscription: subscriber.subscription,
                event: event(),
            })
            .is_ok()
        {
            subscriber.repaint.request_repaint();
        }
    }
}

pub(crate) struct WorkspaceClient {
    owner: WindowSessionId,
    repaint: RepaintTarget,
    root: Option<PathBuf>,
    subscription: u64,
    sender: mpsc::Sender<RoutedWorkspaceEvent>,
    events: mpsc::Receiver<RoutedWorkspaceEvent>,
}

impl WorkspaceClient {
    pub(crate) fn new(owner: WindowSessionId, repaint: RepaintTarget) -> Self {
        let (sender, events) = mpsc::channel();
        Self {
            owner,
            repaint,
            root: None,
            subscription: 0,
            sender,
            events,
        }
    }

    pub(crate) fn subscribe(&mut self, root: &Path) -> Result<(), String> {
        let root = root
            .canonicalize()
            .map_err(|error| format!("Could not use workspace {}: {error}", root.display()))?;
        if self.root.as_ref() == Some(&root) {
            // A consumer may have reset its local tree. Replay the cached
            // snapshot even when the directory contents have not changed.
            SERVICE.lock().unwrap().subscribe(
                root,
                self.owner,
                Subscriber {
                    events: self.sender.clone(),
                    repaint: self.repaint.clone(),
                    subscription: self.subscription,
                },
            )?;
            return Ok(());
        }
        self.unsubscribe();
        let subscription = self.subscription;
        SERVICE.lock().unwrap().subscribe(
            root.clone(),
            self.owner,
            Subscriber {
                events: self.sender.clone(),
                repaint: self.repaint.clone(),
                subscription,
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
        loop {
            match self.events.try_recv() {
                Ok(routed) if self.root.is_some() && routed.subscription == self.subscription => {
                    return Some(routed.event);
                }
                Ok(_) => continue,
                Err(_) => return None,
            }
        }
    }

    pub(crate) fn unsubscribe(&mut self) {
        if let Some(root) = self.root.take() {
            SERVICE.lock().unwrap().unsubscribe(&root, self.owner);
        }
        self.subscription = self.subscription.wrapping_add(1).max(1);
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

    #[test]
    fn queued_workspace_changes_own_one_deadline_and_preserve_upgraded_scan_work() {
        let now = Instant::now();
        let mut changes = PendingChanges::default();
        assert_eq!(changes.deadline(), None);
        assert!(!changes.queue(["a.typ".into()], QueuedImpact::Content, now));
        assert!(!changes.queue(
            ["b.typ".into()],
            QueuedImpact::Structure,
            now + EVENT_QUIET_PERIOD
        ));
        assert!(!changes.queue(
            ["a.typ".into()],
            QueuedImpact::Content,
            now + EVENT_QUIET_PERIOD * 2
        ));
        assert!(changes.take_due(now + EVENT_QUIET_PERIOD).is_none());
        let batch = changes.take_due(now + EVENT_QUIET_PERIOD * 2).unwrap();
        assert_eq!(
            batch.paths,
            BTreeSet::from(["a.typ".into(), "b.typ".into()])
        );
        assert_eq!(batch.impact, QueuedImpact::Structure);
        assert_eq!(changes.deadline(), None);
        assert!(changes.take_due(now + EVENT_QUIET_PERIOD * 3).is_none());
        assert!(changes.queue([], QueuedImpact::Structure, now));
        let refresh = changes.take_due(now).unwrap();
        assert!(
            refresh.paths.is_empty(),
            "a requested scan need not invent changed paths"
        );
        assert_eq!(refresh.impact, QueuedImpact::Structure);
    }

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
    fn resubscribe_replays_unchanged_tree_without_rescanning() {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join("first.typ"), "= First").unwrap();
        let mut client = WorkspaceClient::new(WindowSessionId::new(89), RepaintTarget::test());
        client.subscribe(root.path()).unwrap();
        let (original, serial) = next_snapshot(&client);
        for _ in 0..3 {
            client.subscribe(root.path()).unwrap();
            let (replayed, next_serial) = next_snapshot(&client);
            assert!(Arc::ptr_eq(&original, &replayed));
            assert_eq!(serial, next_serial);
        }
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

    #[test]
    fn stale_subscription_events_are_discarded_after_root_switch() {
        let mut client = WorkspaceClient::new(WindowSessionId::new(83), RepaintTarget::test());
        client.root = Some(PathBuf::from("/new-root"));
        client.subscription = 7;
        client
            .sender
            .send(RoutedWorkspaceEvent {
                subscription: 6,
                event: WorkspaceEvent::Error("old root".to_owned()),
            })
            .unwrap();
        client
            .sender
            .send(RoutedWorkspaceEvent {
                subscription: 7,
                event: WorkspaceEvent::Error("current root".to_owned()),
            })
            .unwrap();

        assert!(matches!(
            client.poll(),
            Some(WorkspaceEvent::Error(error)) if error == "current root"
        ));
        assert!(client.poll().is_none());
        client.root = None;
    }

    #[test]
    fn paths_outside_the_watched_root_are_not_observed() {
        assert!(!observed_path(
            Path::new("/workspace"),
            Path::new("/unrelated/main.typ")
        ));
        assert!(observed_path(
            Path::new("/workspace"),
            Path::new("/workspace/main.typ")
        ));
    }
}
