//! Native TeX tools. One coordinator per window, started lazily for TeX.
mod formatter;
mod session;
pub(crate) mod settings;
pub(crate) mod tools;

use crate::{
    lsp::{
        Generation,
        protocol::{CompletionItem, LspDiagnostic},
    },
    worker::{LatestSender, RepaintTarget, latest_channel},
};
use settings::{Formatter, TexSettings};
use std::{
    collections::VecDeque,
    path::PathBuf,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, SyncSender},
    },
    thread,
    time::Duration,
};
use tiptoptyp_core::{
    document::DocumentKey,
    text::{LspPosition, LspTextEdit},
};
use tools::TexTools;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Provider {
    Texlab,
    Badness,
}
impl Provider {
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Texlab => "TexLab",
            Self::Badness => "Badness",
        }
    }
}

#[derive(Clone)]
pub(crate) struct Snapshot {
    pub(crate) generation: Generation,
    pub(crate) key: DocumentKey,
    pub(crate) uri: String,
    pub(crate) source: Arc<str>,
    pub(crate) root: PathBuf,
    pub(crate) settings: TexSettings,
    pub(crate) tools: TexTools,
}
impl Snapshot {
    fn version(&self) -> i32 {
        i32::try_from(self.key.revision).unwrap_or(i32::MAX)
    }
    fn identity(&self) -> Identity {
        Identity {
            generation: self.generation,
            key: self.key,
            uri: self.uri.clone(),
        }
    }
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Identity {
    pub(crate) generation: Generation,
    pub(crate) key: DocumentKey,
    pub(crate) uri: String,
}
#[derive(Debug, Clone)]
pub(crate) enum RequestKind {
    Format,
    Completion { position: LspPosition, token: u64 },
    Hover { position: LspPosition, token: u64 },
}
#[derive(Debug, Clone)]
pub(crate) struct Request {
    pub(crate) identity: Identity,
    pub(crate) kind: RequestKind,
}
impl Request {
    fn matches(&self, snapshot: &Snapshot) -> bool {
        self.identity == snapshot.identity()
    }
}

pub(crate) enum Event {
    Ready {
        snapshot: Identity,
        provider: Provider,
    },
    Failed {
        snapshot: Identity,
        provider: Provider,
        message: String,
    },
    Diagnostics {
        snapshot: Identity,
        provider: Provider,
        diagnostics: Vec<LspDiagnostic>,
    },
    Formatted {
        request: Request,
        edits: Option<Vec<LspTextEdit>>,
    },
    Completed {
        request: Request,
        is_incomplete: bool,
        items: Vec<CompletionItem>,
    },
    Hovered {
        request: Request,
        contents: Option<String>,
    },
    RequestFailed {
        request: Request,
        message: String,
    },
}
impl Event {
    pub(crate) fn identity(&self) -> &Identity {
        match self {
            Self::Ready { snapshot, .. }
            | Self::Failed { snapshot, .. }
            | Self::Diagnostics { snapshot, .. } => snapshot,
            Self::Formatted { request, .. }
            | Self::Completed { request, .. }
            | Self::Hovered { request, .. }
            | Self::RequestFailed { request, .. } => &request.identity,
        }
    }
}

#[derive(Default)]
pub(crate) struct TexService {
    current: Option<Snapshot>,
    generation: u64,
    worker: Option<Worker>,
    pub(crate) texlab_ready: bool,
    pub(crate) badness_ready: bool,
    pub(crate) texlab_error: Option<String>,
    pub(crate) badness_error: Option<String>,
}
struct Worker {
    snapshots: LatestSender<Option<Snapshot>>,
    requests: SyncSender<Request>,
    events: Arc<Mutex<VecDeque<Event>>>,
    shutdown: Arc<AtomicBool>,
    thread: Option<thread::JoinHandle<()>>,
}
impl TexService {
    pub(crate) fn identity(&self) -> Option<&Snapshot> {
        self.current.as_ref()
    }
    pub(crate) fn accepts(&self, identity: &Identity, key: DocumentKey) -> bool {
        key == identity.key
            && self
                .current
                .as_ref()
                .is_some_and(|s| s.identity() == *identity)
    }
    pub(crate) fn synchronize(&mut self, mut snapshot: Snapshot, repaint: RepaintTarget) {
        let restart = self.current.as_ref().is_none_or(|old| {
            old.root != snapshot.root
                || old.settings != snapshot.settings
                || old.tools != snapshot.tools
        });
        if restart {
            self.generation = self.generation.wrapping_add(1).max(1);
            self.texlab_ready = false;
            self.badness_ready = false;
            self.texlab_error = None;
            self.badness_error = None;
        }
        snapshot.generation = Generation(self.generation);
        if self
            .current
            .as_ref()
            .is_some_and(|old| old.identity() == snapshot.identity())
        {
            return;
        }
        let worker = self.worker.get_or_insert_with(|| Worker::new(repaint));
        if worker.snapshots.send(Some(snapshot.clone())).is_ok() {
            self.current = Some(snapshot);
        }
    }
    pub(crate) fn stop(&mut self) {
        if self.current.take().is_some() {
            if let Some(worker) = &self.worker {
                let _ = worker.snapshots.send(None);
            }
            self.texlab_ready = false;
            self.badness_ready = false;
        }
    }
    pub(crate) fn request(&self, kind: RequestKind) -> Result<(), String> {
        let snapshot = self.current.as_ref().ok_or("No active TeX document")?;
        let available = match kind {
            RequestKind::Completion { .. } => {
                snapshot.settings.texlab_enabled
                    && snapshot.settings.completion
                    && self.texlab_ready
            }
            RequestKind::Hover { .. } => {
                snapshot.settings.texlab_enabled && snapshot.settings.hover && self.texlab_ready
            }
            RequestKind::Format => match snapshot.settings.formatter {
                Formatter::Badness => self.badness_ready,
                Formatter::TexFmt => snapshot.tools.tex_fmt.is_available(),
                Formatter::Disabled => false,
            },
        };
        if !available {
            return Err("This TeX feature is disabled, unavailable, or still starting; see Settings → Tools".into());
        }
        self.worker
            .as_ref()
            .ok_or("TeX worker stopped")?
            .requests
            .try_send(Request {
                identity: snapshot.identity(),
                kind,
            })
            .map_err(|_| "TeX request queue is full or stopped".into())
    }
    pub(crate) fn try_recv(&mut self) -> Option<Event> {
        loop {
            let event = self
                .worker
                .as_ref()?
                .events
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .pop_front()?;
            let identity = event.identity();
            if !self.current.as_ref().is_some_and(|s| {
                s.generation == identity.generation
                    && s.key.owner == identity.key.owner
                    && s.key.epoch == identity.key.epoch
                    && s.uri == identity.uri
            }) {
                continue;
            }
            match &event {
                Event::Ready { provider, .. } => match provider {
                    Provider::Texlab => self.texlab_ready = true,
                    Provider::Badness => self.badness_ready = true,
                },
                Event::Failed {
                    provider, message, ..
                } => match provider {
                    Provider::Texlab => {
                        self.texlab_ready = false;
                        self.texlab_error = Some(message.clone());
                    }
                    Provider::Badness => {
                        self.badness_ready = false;
                        self.badness_error = Some(message.clone());
                    }
                },
                _ => {}
            }
            return Some(event);
        }
    }
}
impl Worker {
    fn new(repaint: RepaintTarget) -> Self {
        let (snapshots, receiver) = latest_channel::<Option<Snapshot>>();
        let (requests, request_rx) = mpsc::sync_channel::<Request>(16);
        let events = Arc::new(Mutex::new(VecDeque::new()));
        let output = events.clone();
        let shutdown = Arc::new(AtomicBool::new(false));
        let stop = shutdown.clone();
        let worker = thread::Builder::new()
            .name("tiptoptyp-tex".into())
            .spawn(move || {
                let emit = |event: Event| {
                    let mut queue = output.lock().unwrap_or_else(|e| e.into_inner());
                    if queue.len() == 32 {
                        queue.pop_front();
                    }
                    queue.push_back(event);
                    drop(queue);
                    repaint.request_repaint();
                };
                let mut current: Option<Snapshot> = None;
                let mut texlab: Option<session::Session> = None;
                let mut badness: Option<session::Session> = None;
                let mut formatter: Option<formatter::Job> = None;
                while !stop.load(Ordering::Acquire) {
                    let received = if current.is_some() {
                        receiver.recv_timeout(Duration::from_millis(10))
                    } else {
                        receiver
                            .recv()
                            .map_err(|_| mpsc::RecvTimeoutError::Disconnected)
                    };
                    if let Ok(next) = received {
                        if let Some(snapshot) = next {
                            let restart = current
                                .as_ref()
                                .is_none_or(|s| s.generation != snapshot.generation);
                            if restart {
                                texlab = None;
                                badness = None;
                                formatter = None;
                            }
                            if current.as_ref().is_some_and(|s| s.key != snapshot.key) {
                                formatter = None;
                            }
                            for (provider, slot, enabled) in [
                                (
                                    Provider::Texlab,
                                    &mut texlab,
                                    snapshot.settings.texlab_enabled,
                                ),
                                (
                                    Provider::Badness,
                                    &mut badness,
                                    snapshot.settings.needs_badness(),
                                ),
                            ] {
                                if restart && enabled {
                                    match session::Session::start(provider, &snapshot) {
                                        Ok(session) => *slot = Some(session),
                                        Err(message) => emit(Event::Failed {
                                            snapshot: snapshot.identity(),
                                            provider,
                                            message,
                                        }),
                                    }
                                }
                                if let Some(session) = slot
                                    && let Err(message) = session.sync(&snapshot)
                                {
                                    *slot = None;
                                    emit(Event::Failed {
                                        snapshot: snapshot.identity(),
                                        provider,
                                        message,
                                    });
                                }
                            }
                            current = Some(snapshot);
                        } else {
                            texlab = None;
                            badness = None;
                            formatter = None;
                            current = None;
                        }
                    }
                    let Some(snapshot) = &current else {
                        continue;
                    };
                    for request in request_rx.try_iter() {
                        if !request.matches(snapshot) {
                            continue;
                        }
                        let result = match request.kind {
                            RequestKind::Format
                                if snapshot.settings.formatter == Formatter::TexFmt =>
                            {
                                formatter = None;
                                formatter::Job::start(snapshot, request.clone())
                                    .map(|job| formatter = Some(job))
                            }
                            RequestKind::Format
                                if snapshot.settings.formatter == Formatter::Badness =>
                            {
                                badness
                                    .as_mut()
                                    .ok_or("Badness is unavailable".to_owned())
                                    .and_then(|s| s.request(request.clone()))
                            }
                            RequestKind::Format => Err("TeX formatting is disabled".into()),
                            _ => texlab
                                .as_mut()
                                .ok_or("TexLab is unavailable".to_owned())
                                .and_then(|s| s.request(request.clone())),
                        };
                        if let Err(message) = result {
                            emit(Event::RequestFailed { request, message });
                        }
                    }
                    for (provider, slot) in [
                        (Provider::Texlab, &mut texlab),
                        (Provider::Badness, &mut badness),
                    ] {
                        if let Some(session) = slot {
                            match session.poll() {
                                Ok(events) => {
                                    for event in events {
                                        emit(event);
                                    }
                                }
                                Err(message) => {
                                    *slot = None;
                                    emit(Event::Failed {
                                        snapshot: snapshot.identity(),
                                        provider,
                                        message,
                                    });
                                }
                            }
                        }
                    }
                    if let Some(job) = &mut formatter
                        && let Some(event) = job.poll()
                    {
                        formatter = None;
                        emit(event);
                    }
                }
            })
            .ok();
        Self {
            snapshots,
            requests,
            events,
            shutdown,
            thread: worker,
        }
    }
}
impl Drop for Worker {
    fn drop(&mut self) {
        self.shutdown.store(true, Ordering::Release);
        let _ = self.snapshots.send(None);
        if let Some(worker) = self.thread.take() {
            let _ = worker.join();
        }
    }
}

/// Resolve a batch in one forward scan, preserving server order. Diagnostics
/// must not rebuild a whole-document index for every underline on the UI thread.
pub(crate) fn diagnostic_locations(
    source: &str,
    diagnostics: &[LspDiagnostic],
) -> Vec<Option<crate::diagnostics::DiagnosticLocation>> {
    let mut order: Vec<_> = (0..diagnostics.len()).collect();
    order.sort_unstable_by_key(|&i| {
        (
            diagnostics[i].range.start.line.get(),
            diagnostics[i].range.start.character.get(),
        )
    });
    let mut locations = vec![None; diagnostics.len()];
    let mut chars = source.chars().peekable();
    let (mut line, mut utf16, mut column) = (0usize, 0usize, 0usize);
    for index in order {
        let position = diagnostics[index].range.start;
        let (target_line, target_column) = (
            position.line.get() as usize,
            position.character.get() as usize,
        );
        while line < target_line || (line == target_line && utf16 < target_column) {
            let Some(&ch) = chars.peek() else {
                break;
            };
            if line == target_line
                && (ch == '\n' || (ch == '\r' && chars.clone().nth(1) == Some('\n')))
            {
                break;
            }
            chars.next();
            if ch == '\n' {
                line += 1;
                utf16 = 0;
                column = 0;
            } else {
                utf16 += ch.len_utf16();
                column += 1;
            }
        }
        if line == target_line && utf16 == target_column {
            locations[index] = Some(crate::diagnostics::DiagnosticLocation {
                line: line + 1,
                column: column + 1,
            });
        }
    }
    locations
}

#[cfg(test)]
mod tests {
    use super::*;
    use tiptoptyp_core::document::WindowSessionId;
    fn snapshot(root: &std::path::Path, source: &str) -> Snapshot {
        let settings = TexSettings::default();
        Snapshot {
            generation: Generation(0),
            key: DocumentKey::new(WindowSessionId::new(1), 2, 0),
            uri: url::Url::from_file_path(root.join("main.tex"))
                .unwrap()
                .into(),
            source: Arc::from(source),
            root: root.into(),
            tools: TexTools::resolve(&settings),
            settings,
        }
    }
    #[test]
    fn diagnostic_batch_preserves_order_and_rejects_split_surrogates_and_invalid_lines() {
        let positions = [(1, 1), (0, 3), (0, 2), (5, 0), (0, 3), (0, 99)];
        let diagnostics = positions.map(|(line, column)| LspDiagnostic {
            range: tiptoptyp_core::text::LspRange {
                start: LspPosition::new(line, column),
                end: LspPosition::new(line, column),
            },
            severity: None,
            code: None,
            source: None,
            message: String::new(),
            raw: serde_json::Value::Null,
        });
        let locations = diagnostic_locations("é😀 a\r\nβ\n", &diagnostics);
        assert_eq!(
            locations
                .iter()
                .map(|p| p.map(|p| (p.line, p.column)))
                .collect::<Vec<_>>(),
            vec![Some((2, 2)), Some((1, 3)), None, None, Some((1, 3)), None]
        );
    }

    #[test]
    fn reply_identity_rejects_edits_tabs_windows_and_configuration_changes() {
        let project = tempfile::tempdir().unwrap();
        let snapshot = snapshot(project.path(), "text");
        let mut service = TexService {
            current: Some(snapshot.clone()),
            ..Default::default()
        };
        assert!(service.accepts(&snapshot.identity(), snapshot.key));
        for key in [
            DocumentKey::new(snapshot.key.owner, 2, 1),
            DocumentKey::new(snapshot.key.owner, 3, 0),
            DocumentKey::new(WindowSessionId::new(2), 2, 0),
        ] {
            assert!(!service.accepts(&snapshot.identity(), key));
        }
        service.current.as_mut().unwrap().generation = Generation(2);
        assert!(!service.accepts(&snapshot.identity(), snapshot.key));
    }
    #[test]
    #[ignore = "executes pinned TexLab, Badness and tex-fmt sidecars"]
    fn real_tex_services_format_complete_hover_and_lint() {
        let project = tempfile::tempdir().unwrap();
        let source =
            "\\documentclass{article}\n\\begin{document}\nHello   world.\n\\end{document}\n";
        std::fs::write(project.path().join("main.tex"), source).unwrap();
        let mut service = TexService::default();
        let mut input = snapshot(project.path(), source);
        service.synchronize(input.clone(), RepaintTarget::test());
        let deadline = std::time::Instant::now() + Duration::from_secs(20);
        let mut saw_lint = false;
        while !service.texlab_ready || !service.badness_ready || !saw_lint {
            while let Some(event) = service.try_recv() {
                match event {
                    Event::Failed {
                        provider, message, ..
                    } => panic!("{}: {message}", provider.label()),
                    Event::Diagnostics {
                        provider: Provider::Badness,
                        ..
                    } => saw_lint = true,
                    _ => {}
                }
            }
            assert!(
                std::time::Instant::now() < deadline,
                "servers did not become ready / publish diagnostics"
            );
            thread::sleep(Duration::from_millis(10));
        }
        service
            .request(RequestKind::Completion {
                position: LspPosition::new(0, 1),
                token: 1,
            })
            .unwrap();
        service
            .request(RequestKind::Hover {
                position: LspPosition::new(0, 16),
                token: 2,
            })
            .unwrap();
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        let (mut completed, mut hovered) = (false, false);
        while !completed || !hovered {
            match service.try_recv() {
                Some(Event::Completed { items, .. }) => {
                    assert!(!items.is_empty(), "TexLab supplies command completions");
                    completed = true;
                }
                Some(Event::Hovered { contents, .. }) => {
                    eprintln!(
                        "TexLab hover returned documentation: {}",
                        contents.is_some()
                    );
                    hovered = true;
                }
                Some(Event::RequestFailed { message, .. }) => panic!("{message}"),
                _ => {}
            }
            assert!(
                std::time::Instant::now() < deadline,
                "editor intelligence timed out"
            );
            thread::sleep(Duration::from_millis(10));
        }
        for formatter in [Formatter::Badness, Formatter::TexFmt] {
            if input.settings.formatter != formatter {
                input.settings.formatter = formatter;
                service.synchronize(input.clone(), RepaintTarget::test());
            }
            service.request(RequestKind::Format).unwrap();
            let deadline = std::time::Instant::now() + Duration::from_secs(10);
            loop {
                match service.try_recv() {
                    Some(Event::Formatted { request, edits }) => {
                        assert!(service.accepts(&request.identity, input.key));
                        let edits = edits.expect("formatter supports TeX");
                        let result = tiptoptyp_core::text::apply_text_edits(
                            source,
                            &edits,
                            [tiptoptyp_core::text::ScalarOffset::new(0); 2],
                        )
                        .unwrap();
                        assert!(result.text.contains("\\documentclass{article}"));
                        break;
                    }
                    Some(Event::RequestFailed { message, .. }) => panic!("{message}"),
                    _ => {}
                }
                assert!(std::time::Instant::now() < deadline, "formatting timed out");
                thread::sleep(Duration::from_millis(10));
            }
        }
        assert_eq!(
            std::fs::read_to_string(project.path().join("main.tex")).unwrap(),
            source
        );
        service.stop();
    }
}
