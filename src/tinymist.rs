//! Optional Tinymist language-server and interactive-preview sidecar.
//!
//! The sidecar deliberately speaks only public LSP commands. Tinymist serves
//! its own version-matched preview frontend, so this module never needs to
//! understand Tinymist's private vector-diff protocol.

use std::{
    collections::HashMap,
    ffi::OsString,
    fmt,
    io::{self, BufRead, BufReader, Read, Write},
    path::{Path, PathBuf},
    process::{Child, ChildStdin, Command, ExitStatus, Stdio},
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
        mpsc::{self, Receiver, RecvTimeoutError, Sender, TryRecvError},
    },
    thread,
    time::{Duration, Instant},
};

use eframe::egui;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use url::Url;

use crate::private_workspace::{PrivateTypstDocument, PrivateWorkspace};

const WORKER_POLL_INTERVAL: Duration = Duration::from_millis(10);
const GRACEFUL_SHUTDOWN_TIMEOUT: Duration = Duration::from_millis(300);
const EXIT_TIMEOUT: Duration = Duration::from_millis(200);
const PIPE_READER_SHUTDOWN_TIMEOUT: Duration = Duration::from_millis(250);
const WORKER_SHUTDOWN_TIMEOUT: Duration = Duration::from_millis(700);
const FORCED_WORKER_SHUTDOWN_TIMEOUT: Duration = Duration::from_millis(500);
const INITIALIZE_REQUEST_TIMEOUT: Duration = Duration::from_secs(15);
const PREVIEW_REQUEST_TIMEOUT: Duration = Duration::from_secs(30);
const INTERACTIVE_REQUEST_TIMEOUT: Duration = Duration::from_secs(10);
const FORMAT_REQUEST_TIMEOUT: Duration = Duration::from_secs(120);
const MAX_HEADER_LINE_BYTES: usize = 8 * 1024;
const MAX_HEADER_BYTES: usize = 32 * 1024;
const MAX_MESSAGE_BYTES: usize = 32 * 1024 * 1024;
const DEFAULT_PREVIEW_TASK_ID: &str = "default_preview";

pub type Result<T> = std::result::Result<T, TinymistError>;

/// Identifies one workspace process. Generations are never reused.
///
/// Every document command includes its generation. This prevents a queued
/// edit from one workspace from being applied after the user opens another.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Generation(pub u64);

#[derive(Debug)]
pub enum TinymistError {
    WorkerStopped,
    StaleGeneration {
        requested: Generation,
        current: Option<Generation>,
    },
    InvalidFilePath(PathBuf),
    PrivateDocument {
        project_root: PathBuf,
        message: String,
    },
}

impl fmt::Display for TinymistError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::WorkerStopped => formatter.write_str("the Tinymist worker has stopped"),
            Self::StaleGeneration { requested, current } => write!(
                formatter,
                "Tinymist generation {} is stale (current generation: {})",
                requested.0,
                current
                    .map(|generation| generation.0.to_string())
                    .unwrap_or_else(|| "none".to_owned())
            ),
            Self::InvalidFilePath(path) => {
                write!(formatter, "cannot convert {} to a file URI", path.display())
            }
            Self::PrivateDocument {
                project_root,
                message,
            } => write!(
                formatter,
                "cannot prepare an unsaved Typst document in {}: {message}",
                project_root.display()
            ),
        }
    }
}

impl std::error::Error for TinymistError {}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum InvertColors {
    Never,
    #[default]
    Auto,
    Always,
}

impl InvertColors {
    fn as_arg(self) -> &'static str {
        match self {
            Self::Never => "never",
            Self::Auto => "auto",
            Self::Always => "always",
        }
    }
}

/// Controls when Tinymist's interactive preview consumes synchronized LSP
/// document changes. `OnSave` is also useful as a paused-preview policy in
/// clients which deliberately keep language intelligence live without sending
/// `textDocument/didSave` notifications.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PreviewRefresh {
    #[default]
    OnType,
    OnSave,
}

impl PreviewRefresh {
    const fn as_setting(self) -> &'static str {
        match self {
            Self::OnType => "onType",
            Self::OnSave => "onSave",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct PreviewOptions {
    pub invert_colors: InvertColors,
    pub refresh: PreviewRefresh,
}

impl PreviewOptions {
    fn command_line(&self, entry_path: Option<&Path>) -> Vec<String> {
        let mut arguments = Vec::new();
        // `startDefaultPreview` otherwise follows Tinymist's inferred focused
        // file. Supplying the designated entry to the preview command itself
        // keeps the one embedded viewer pinned when another source is opened.
        if let Some(entry_path) = entry_path {
            arguments.push(entry_path.to_string_lossy().into_owned());
        }
        arguments.extend([
            "--data-plane-host=127.0.0.1:0".to_owned(),
            "--control-plane-host=127.0.0.1:0".to_owned(),
            "--preview-mode=document".to_owned(),
            format!("--invert-colors={}", self.invert_colors.as_arg()),
        ]);
        // Tinymist labels partial rendering experimental. Its visible-page
        // cache can fail to refill after large scroll or zoom jumps, so the
        // desktop editor deliberately requests the complete document.
        arguments.push("--partial-rendering=false".to_owned());
        // This is a native application: opening the system browser would be a
        // surprising side effect and could expose a stale preview tab.
        arguments.push("--no-open".to_owned());
        arguments
    }

    fn settings(&self, entry_path: Option<&Path>) -> Value {
        json!({
            "preview": {
                "refresh": self.refresh.as_setting(),
                "browsing": {
                    "args": self.command_line(entry_path),
                }
            },
            // False tells Tinymist to use standard window/showDocument, which
            // is advertised and handled below.
            "customizedShowDocument": false,
        })
    }
}

/// Configuration for one Tinymist process.
#[derive(Debug, Clone)]
pub struct TinymistConfig {
    pub workspace_root: PathBuf,
    pub preview: PreviewOptions,
    /// Whether this LSP session should also launch Tinymist's interactive
    /// preview server. Document synchronization and formatting remain
    /// available when this is false.
    pub start_preview: bool,
    /// Explicit Typst entry used for diagnostics and the default preview.
    /// Without this, Tinymist follows whichever open document it most
    /// recently inferred as focused.
    entry_path: Option<PathBuf>,
    program: PathBuf,
    arguments: Vec<OsString>,
    request_timeout_override: Option<Duration>,
}

impl TinymistConfig {
    pub fn new(workspace_root: impl Into<PathBuf>) -> Self {
        Self {
            workspace_root: workspace_root.into(),
            preview: PreviewOptions::default(),
            start_preview: true,
            entry_path: None,
            program: PathBuf::from("tinymist"),
            arguments: vec![OsString::from("lsp")],
            request_timeout_override: None,
        }
    }

    /// Overrides the executable while retaining the default `lsp` argument.
    pub fn with_executable(mut self, executable: impl Into<PathBuf>) -> Self {
        self.program = executable.into();
        self
    }

    /// Pin the project entry for this process. Tinymist consumes the final
    /// `typstExtraArgs` value as the entry document before any open-document
    /// focus changes can influence `startDefaultPreview`.
    pub fn with_entry_path(mut self, entry_path: impl Into<PathBuf>) -> Self {
        self.entry_path = Some(entry_path.into());
        self
    }

    /// Add workspace-local font directories using Tinymist's recursive font
    /// search. Joining them into one platform-native argument matches the LSP
    /// sidecar's documented command-line contract.
    pub fn with_font_paths(mut self, paths: &[PathBuf]) -> Self {
        if !paths.is_empty()
            && let Ok(paths) = std::env::join_paths(paths)
        {
            self.arguments.push(OsString::from("--font-path"));
            self.arguments.push(paths);
        }
        self
    }

    #[cfg(test)]
    fn with_command(
        mut self,
        program: impl Into<PathBuf>,
        arguments: impl IntoIterator<Item = OsString>,
    ) -> Self {
        self.program = program.into();
        self.arguments = arguments.into_iter().collect();
        self
    }

    #[cfg(test)]
    fn with_request_timeout(mut self, timeout: Duration) -> Self {
        self.request_timeout_override = Some(timeout);
        self
    }

    fn server_settings(&self) -> Value {
        let mut settings = self.preview.settings(self.entry_path.as_deref());
        if let Some(entry_path) = &self.entry_path {
            settings["typstExtraArgs"] = json!([entry_path.to_string_lossy()]);
        }
        settings
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TextDocument {
    pub uri: String,
    pub language_id: String,
    pub version: i32,
    pub text: String,
}

#[derive(Serialize)]
struct Notification<'a, Params> {
    jsonrpc: &'static str,
    method: &'a str,
    params: Params,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct DidOpenParams<'a> {
    text_document: &'a TextDocument,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct DidChangeParams<'a> {
    text_document: VersionedDocument<'a>,
    content_changes: [ContentChange<'a>; 1],
}

#[derive(Serialize)]
struct VersionedDocument<'a> {
    uri: &'a str,
    version: i32,
}

#[derive(Serialize)]
struct ContentChange<'a> {
    text: &'a str,
}

impl TextDocument {
    pub fn typst(uri: impl Into<String>, version: i32, text: impl Into<String>) -> Self {
        Self {
            uri: uri.into(),
            language_id: "typst".to_owned(),
            version,
            text: text.into(),
        }
    }

    pub fn from_path(path: &Path, version: i32, text: impl Into<String>) -> Result<Self> {
        Ok(Self::typst(path_to_file_uri(path)?, version, text))
    }
}

/// Filesystem backing for an unsaved buffer opened through Tinymist.
///
/// Tinymist can format a `didOpen` buffer only when its `file:` URI resolves to
/// a real file. This guard provides that file below the project's private
/// directory, preserves relative-import layout, and removes the complete
/// mirror when dropped. The app should retain it until after `didClose`.
#[derive(Debug)]
pub struct UnsavedTextDocument {
    project_root: PathBuf,
    backing: PrivateTypstDocument,
    uri: String,
}

impl UnsavedTextDocument {
    pub fn create(
        project_root: impl AsRef<Path>,
        source_dir: impl AsRef<Path>,
        display_name: impl AsRef<std::ffi::OsStr>,
        source: &str,
    ) -> Result<Self> {
        let requested_root = project_root.as_ref();
        let private = PrivateWorkspace::open(requested_root).map_err(|error| {
            TinymistError::PrivateDocument {
                project_root: requested_root.to_owned(),
                message: error.to_string(),
            }
        })?;
        let project_root = private.project_root().to_owned();
        let backing = private
            .mirrored_typst_document(source_dir, display_name, source)
            .map_err(|error| TinymistError::PrivateDocument {
                project_root: project_root.clone(),
                message: error.to_string(),
            })?;
        let uri = path_to_file_uri(backing.path())?;
        Ok(Self {
            project_root,
            backing,
            uri,
        })
    }

    pub fn path(&self) -> &Path {
        self.backing.path()
    }

    #[cfg(test)]
    pub fn uri(&self) -> &str {
        &self.uri
    }

    pub fn text_document(&self, version: i32, text: impl Into<String>) -> TextDocument {
        TextDocument::typst(self.uri.clone(), version, text)
    }

    /// Keeps the real backing file current before a formatting request. LSP
    /// `didChange` remains the authority for Tinymist's in-memory contents.
    pub fn update_backing_source(&self, source: &str) -> Result<()> {
        self.backing
            .update(source)
            .map_err(|error| TinymistError::PrivateDocument {
                project_root: self.project_root.clone(),
                message: error.to_string(),
            })
    }
}

/// Converts an existing or prospective path to an absolute `file:` URI.
/// Unlike canonicalization, this also works for a not-yet-created file.
pub fn path_to_file_uri(path: &Path) -> Result<String> {
    let absolute = if path.is_absolute() {
        path.to_owned()
    } else {
        std::env::current_dir()
            .map_err(|_| TinymistError::InvalidFilePath(path.to_owned()))?
            .join(path)
    };
    Url::from_file_path(&absolute)
        .map(Into::into)
        .map_err(|()| TinymistError::InvalidFilePath(path.to_owned()))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct LspPosition {
    /// Zero-based line number.
    pub line: u32,
    /// Zero-based UTF-16 code-unit offset, as required by LSP.
    pub character: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct LspRange {
    pub start: LspPosition,
    pub end: LspPosition,
}

/// A standard LSP text edit returned by `textDocument/formatting`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LspTextEdit {
    pub range: LspRange,
    pub new_text: String,
}

/// One completion candidate returned by Tinymist.
///
/// The model intentionally keeps only the standard fields the editor can
/// apply safely. Commands attached to completion items are not executed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompletionItem {
    pub label: String,
    pub detail: Option<String>,
    pub documentation: Option<String>,
    pub filter_text: Option<String>,
    pub sort_text: Option<String>,
    pub insert_text: String,
    pub insert_text_is_snippet: bool,
    pub text_edit: Option<LspTextEdit>,
    pub additional_text_edits: Vec<LspTextEdit>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiagnosticSeverity {
    Error,
    Warning,
    Information,
    Hint,
    Other(u64),
}

impl DiagnosticSeverity {
    fn from_lsp(value: u64) -> Self {
        match value {
            1 => Self::Error,
            2 => Self::Warning,
            3 => Self::Information,
            4 => Self::Hint,
            other => Self::Other(other),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct TinymistDiagnostic {
    pub range: LspRange,
    pub severity: Option<DiagnosticSeverity>,
    pub code: Option<Value>,
    pub source: Option<String>,
    pub message: String,
    /// The complete diagnostic, including related information, tags and data.
    pub raw: Value,
}

#[derive(Debug, Clone, PartialEq)]
pub enum TinymistEvent {
    Starting {
        generation: Generation,
    },
    Initialized {
        generation: Generation,
    },
    PreviewReady {
        generation: Generation,
        url: String,
    },
    ShowDocument {
        generation: Generation,
        uri: String,
        selection: Option<LspRange>,
        external: Option<bool>,
        take_focus: Option<bool>,
        raw: Value,
    },
    PublishDiagnostics {
        generation: Generation,
        uri: String,
        version: Option<i32>,
        diagnostics: Vec<TinymistDiagnostic>,
        raw: Value,
    },
    /// The result of formatting one exact version of an open document.
    ///
    /// `None` preserves the LSP distinction between a `null` result (formatting
    /// is unavailable) and `Some(Vec::new())` (the document is already
    /// formatted). The caller must discard results whose version is stale.
    Formatted {
        generation: Generation,
        uri: String,
        version: i32,
        edits: Option<Vec<LspTextEdit>>,
    },
    /// Hover information for one exact document version and UI request.
    Hovered {
        generation: Generation,
        uri: String,
        version: i32,
        request_token: u64,
        contents: Option<String>,
        range: Option<LspRange>,
    },
    /// Completion candidates for one exact document version and UI request.
    Completed {
        generation: Generation,
        uri: String,
        version: i32,
        request_token: u64,
        is_incomplete: bool,
        items: Vec<CompletionItem>,
    },
    /// A malformed completion response for one exact UI request.
    CompletionFailed {
        generation: Generation,
        uri: String,
        version: i32,
        request_token: u64,
        message: String,
    },
    Log {
        generation: Generation,
        level: Option<u32>,
        message: String,
    },
    /// A notification not interpreted by this version of tiptoptyp.
    Notification {
        generation: Generation,
        method: String,
        params: Value,
    },
    Error {
        generation: Generation,
        stage: &'static str,
        message: String,
        fatal: bool,
    },
    Stopped {
        generation: Generation,
        code: Option<i32>,
        reason: String,
    },
}

impl TinymistEvent {
    pub fn generation(&self) -> Generation {
        match self {
            Self::Starting { generation }
            | Self::Initialized { generation }
            | Self::PreviewReady { generation, .. }
            | Self::ShowDocument { generation, .. }
            | Self::PublishDiagnostics { generation, .. }
            | Self::Formatted { generation, .. }
            | Self::Hovered { generation, .. }
            | Self::Completed { generation, .. }
            | Self::CompletionFailed { generation, .. }
            | Self::Log { generation, .. }
            | Self::Notification { generation, .. }
            | Self::Error { generation, .. }
            | Self::Stopped { generation, .. } => *generation,
        }
    }
}

type SharedChild = Arc<Mutex<Child>>;

#[derive(Clone)]
struct ActiveProcess {
    generation: Generation,
    child: SharedChild,
}

/// Process handle shared with the sidecar owner so shutdown never depends on
/// the protocol worker making progress through a potentially blocked pipe.
#[derive(Default)]
struct ProcessSupervisor {
    active: Mutex<Option<ActiveProcess>>,
}

impl ProcessSupervisor {
    fn install(&self, generation: Generation, child: &SharedChild) {
        *self
            .active
            .lock()
            .unwrap_or_else(|error| error.into_inner()) = Some(ActiveProcess {
            generation,
            child: Arc::clone(child),
        });
    }

    fn clear(&self, generation: Generation, child: &SharedChild) {
        let mut active = self
            .active
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if active.as_ref().is_some_and(|active| {
            active.generation == generation && Arc::ptr_eq(&active.child, child)
        }) {
            *active = None;
        }
    }

    fn terminate_active(&self) {
        let child = self
            .active
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .as_ref()
            .map(|active| Arc::clone(&active.child));
        if let Some(child) = child {
            // Never turn the independent shutdown path into another join on
            // the protocol worker. In the blocked-stdin case this lock is
            // free; if the worker is already reaping while holding it, the
            // bounded completion wait below is sufficient.
            let mut child = match child.try_lock() {
                Ok(child) => child,
                Err(std::sync::TryLockError::Poisoned(error)) => error.into_inner(),
                Err(std::sync::TryLockError::WouldBlock) => return,
            };
            if !matches!(child.try_wait(), Ok(Some(_))) {
                let _ = child.kill();
            }
        }
    }

    #[cfg(test)]
    fn has_active_process(&self) -> bool {
        self.active
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .is_some()
    }
}

/// Nonblocking controller for an optional Tinymist process.
pub struct TinymistSidecar {
    commands: Option<Sender<WorkerCommand>>,
    events: Receiver<TinymistEvent>,
    worker: Option<thread::JoinHandle<()>>,
    next_generation: AtomicU64,
    current_generation: Arc<AtomicU64>,
    process_supervisor: Arc<ProcessSupervisor>,
    worker_done: Option<Receiver<()>>,
}

impl TinymistSidecar {
    pub fn new(context: egui::Context) -> Self {
        let (command_tx, command_rx) = mpsc::channel();
        let (event_tx, event_rx) = mpsc::channel();
        let current_generation = Arc::new(AtomicU64::new(0));
        let worker_generation = current_generation.clone();
        let process_supervisor = Arc::new(ProcessSupervisor::default());
        let worker_supervisor = Arc::clone(&process_supervisor);
        let (worker_done_tx, worker_done_rx) = mpsc::channel();
        let worker = thread::Builder::new()
            .name("tiptoptyp-tinymist".to_owned())
            .spawn(move || {
                worker_loop(
                    command_rx,
                    event_tx,
                    context,
                    worker_generation,
                    worker_supervisor,
                );
                let _ = worker_done_tx.send(());
            })
            .expect("failed to start Tinymist worker");

        Self {
            commands: Some(command_tx),
            events: event_rx,
            worker: Some(worker),
            next_generation: AtomicU64::new(1),
            current_generation,
            process_supervisor,
            worker_done: Some(worker_done_rx),
        }
    }

    /// Queues a workspace start and returns immediately.
    pub fn start_workspace(&self, config: TinymistConfig) -> Result<Generation> {
        let generation = Generation(self.next_generation.fetch_add(1, Ordering::Relaxed));
        self.current_generation
            .store(generation.0, Ordering::Release);
        if self
            .send(WorkerCommand::Start { generation, config })
            .is_err()
        {
            self.current_generation.store(0, Ordering::Release);
            return Err(TinymistError::WorkerStopped);
        }
        Ok(generation)
    }

    pub fn did_open(&self, generation: Generation, document: TextDocument) -> Result<()> {
        self.send_for_generation(
            generation,
            WorkerCommand::DidOpen {
                generation,
                document,
            },
        )
    }

    /// Sends a full-buffer LSP change. An identical version and buffer are a
    /// no-op; actual changes must increase the version monotonically.
    pub fn did_change(
        &self,
        generation: Generation,
        uri: impl Into<String>,
        version: i32,
        text: impl Into<String>,
    ) -> Result<()> {
        self.send_for_generation(
            generation,
            WorkerCommand::DidChange {
                generation,
                uri: uri.into(),
                version,
                text: text.into(),
            },
        )
    }

    pub fn did_close(&self, generation: Generation, uri: impl Into<String>) -> Result<()> {
        self.send_for_generation(
            generation,
            WorkerCommand::DidClose {
                generation,
                uri: uri.into(),
            },
        )
    }

    /// Changes only the interactive preview's refresh trigger. The Tinymist
    /// process and its open LSP documents stay intact, so language features do
    /// not disappear while automatic preview updates are paused.
    pub fn set_preview_refresh(
        &self,
        generation: Generation,
        refresh: PreviewRefresh,
    ) -> Result<()> {
        self.send_for_generation(
            generation,
            WorkerCommand::SetPreviewRefresh {
                generation,
                refresh,
            },
        )
    }

    /// Requests whole-document formatting for one exact open-document version.
    ///
    /// The eventual [`TinymistEvent::Formatted`] repeats the URI and version so
    /// the UI can reject an edit set if the buffer changed while Tinymist was
    /// computing it.
    pub fn format_document(
        &self,
        generation: Generation,
        uri: impl Into<String>,
        version: i32,
    ) -> Result<()> {
        self.send_for_generation(
            generation,
            WorkerCommand::FormatDocument {
                generation,
                uri: uri.into(),
                version,
            },
        )
    }

    /// Requests semantic hover information for one exact open-document
    /// version. `request_token` is opaque and lets the UI discard a response
    /// after the pointer moves to another token.
    pub fn hover_document(
        &self,
        generation: Generation,
        uri: impl Into<String>,
        version: i32,
        position: LspPosition,
        request_token: u64,
    ) -> Result<()> {
        self.send_for_generation(
            generation,
            WorkerCommand::HoverDocument {
                generation,
                uri: uri.into(),
                version,
                position,
                request_token,
            },
        )
    }

    /// Requests completion candidates for one exact open-document version.
    /// New requests for the same URI cancel older in-flight completion work in
    /// the worker; `request_token` still lets the UI reject a raced response.
    pub fn complete_document(
        &self,
        generation: Generation,
        uri: impl Into<String>,
        version: i32,
        position: LspPosition,
        request_token: u64,
    ) -> Result<()> {
        self.send_for_generation(
            generation,
            WorkerCommand::CompleteDocument {
                generation,
                uri: uri.into(),
                version,
                position,
                request_token,
            },
        )
    }

    /// Reveals one source position in the bundled interactive preview.
    ///
    /// Tinymist's current preview resolver consumes a zero-based Unicode
    /// scalar column here (despite the LSP transport otherwise using UTF-16).
    pub fn scroll_preview(
        &self,
        generation: Generation,
        path: impl Into<PathBuf>,
        line: u32,
        character: u32,
    ) -> Result<()> {
        self.send_for_generation(
            generation,
            WorkerCommand::ScrollPreview {
                generation,
                path: path.into(),
                line,
                character,
            },
        )
    }

    pub fn stop_workspace(&self, generation: Generation) -> Result<()> {
        self.send_for_generation(generation, WorkerCommand::Stop { generation })
    }

    pub fn current_generation(&self) -> Option<Generation> {
        match self.current_generation.load(Ordering::Acquire) {
            0 => None,
            generation => Some(Generation(generation)),
        }
    }

    /// Returns the next event for the current workspace without blocking.
    /// Events already queued for an older generation are discarded here too.
    pub fn try_recv(&self) -> Option<TinymistEvent> {
        loop {
            let event = self.events.try_recv().ok()?;
            if self.current_generation() == Some(event.generation()) {
                if matches!(event, TinymistEvent::Stopped { .. }) {
                    let _ = self.current_generation.compare_exchange(
                        event.generation().0,
                        0,
                        Ordering::AcqRel,
                        Ordering::Acquire,
                    );
                }
                return Some(event);
            }
        }
    }

    fn send_for_generation(&self, generation: Generation, command: WorkerCommand) -> Result<()> {
        let current = self.current_generation();
        if current != Some(generation) {
            return Err(TinymistError::StaleGeneration {
                requested: generation,
                current,
            });
        }
        self.send(command)
    }

    fn send(&self, command: WorkerCommand) -> Result<()> {
        self.commands
            .as_ref()
            .ok_or(TinymistError::WorkerStopped)?
            .send(command)
            .map_err(|_| TinymistError::WorkerStopped)
    }
}

impl Drop for TinymistSidecar {
    fn drop(&mut self) {
        if let Some(commands) = self.commands.take() {
            let _ = commands.send(WorkerCommand::Shutdown);
            drop(commands);
        }
        if let Some(worker) = self.worker.take() {
            let worker_done = self.worker_done.take();
            let mut finished = worker_done.as_ref().is_some_and(|done| {
                !matches!(
                    done.recv_timeout(WORKER_SHUTDOWN_TIMEOUT),
                    Err(RecvTimeoutError::Timeout)
                )
            });
            if !finished {
                // The protocol worker may be blocked in a pipe write to a
                // server which stopped reading. Kill through the independently
                // shared child handle; this closes the pipe and lets the worker
                // unwind without waiting for it to consume Shutdown first.
                self.process_supervisor.terminate_active();
                finished = worker_done.as_ref().is_some_and(|done| {
                    !matches!(
                        done.recv_timeout(FORCED_WORKER_SHUTDOWN_TIMEOUT),
                        Err(RecvTimeoutError::Timeout)
                    )
                });
            }
            if finished {
                let _ = worker.join();
            }
        }
    }
}

enum WorkerCommand {
    Start {
        generation: Generation,
        config: TinymistConfig,
    },
    DidOpen {
        generation: Generation,
        document: TextDocument,
    },
    DidChange {
        generation: Generation,
        uri: String,
        version: i32,
        text: String,
    },
    DidClose {
        generation: Generation,
        uri: String,
    },
    SetPreviewRefresh {
        generation: Generation,
        refresh: PreviewRefresh,
    },
    FormatDocument {
        generation: Generation,
        uri: String,
        version: i32,
    },
    HoverDocument {
        generation: Generation,
        uri: String,
        version: i32,
        position: LspPosition,
        request_token: u64,
    },
    CompleteDocument {
        generation: Generation,
        uri: String,
        version: i32,
        position: LspPosition,
        request_token: u64,
    },
    ScrollPreview {
        generation: Generation,
        path: PathBuf,
        line: u32,
        character: u32,
    },
    Stop {
        generation: Generation,
    },
    Shutdown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SessionPhase {
    Initializing,
    Initialized,
    StartingPreview,
    Running,
}

impl SessionPhase {
    fn can_sync_documents(self) -> bool {
        self != Self::Initializing
    }
}

#[derive(Debug, Clone)]
enum PendingRequest {
    Initialize,
    StartPreview,
    ScrollPreview,
    FormatDocument {
        uri: String,
        version: i32,
    },
    HoverDocument {
        uri: String,
        version: i32,
        request_token: u64,
    },
    CompleteDocument {
        uri: String,
        version: i32,
        request_token: u64,
    },
}

impl PendingRequest {
    fn method(&self) -> &'static str {
        match self {
            Self::Initialize => "initialize",
            Self::StartPreview | Self::ScrollPreview => "workspace/executeCommand",
            Self::FormatDocument { .. } => "textDocument/formatting",
            Self::HoverDocument { .. } => "textDocument/hover",
            Self::CompleteDocument { .. } => "textDocument/completion",
        }
    }

    fn timeout(&self) -> Duration {
        match self {
            Self::Initialize => INITIALIZE_REQUEST_TIMEOUT,
            Self::StartPreview => PREVIEW_REQUEST_TIMEOUT,
            Self::FormatDocument { .. } => FORMAT_REQUEST_TIMEOUT,
            Self::ScrollPreview | Self::HoverDocument { .. } | Self::CompleteDocument { .. } => {
                INTERACTIVE_REQUEST_TIMEOUT
            }
        }
    }
}

#[derive(Debug, Clone)]
struct PendingCall {
    request: PendingRequest,
    timeout: Duration,
    deadline: Instant,
}

enum Incoming {
    Message(Value),
    Stderr(String),
    EndOfStream,
    ReadError(String),
}

struct Session {
    generation: Generation,
    phase: SessionPhase,
    child: SharedChild,
    process_supervisor: Arc<ProcessSupervisor>,
    stdin: Option<ChildStdin>,
    incoming: Receiver<Incoming>,
    stdout_reader: Option<thread::JoinHandle<()>>,
    stderr_reader: Option<thread::JoinHandle<()>>,
    next_request_id: u64,
    pending: HashMap<u64, PendingCall>,
    documents: HashMap<String, TextDocument>,
    settings: Value,
    start_preview: bool,
    root_uri: String,
    workspace_name: String,
    request_timeout_override: Option<Duration>,
    cleaned_up: bool,
}

impl Session {
    fn spawn(
        generation: Generation,
        config: TinymistConfig,
        process_supervisor: Arc<ProcessSupervisor>,
    ) -> std::result::Result<Self, String> {
        Self::spawn_with_initializer(
            generation,
            config,
            process_supervisor,
            Self::send_initialize,
        )
    }

    fn spawn_with_initializer(
        generation: Generation,
        config: TinymistConfig,
        process_supervisor: Arc<ProcessSupervisor>,
        initialize: impl FnOnce(&mut Self) -> std::result::Result<(), String>,
    ) -> std::result::Result<Self, String> {
        let root = absolute_path(&config.workspace_root)
            .map_err(|error| format!("could not resolve workspace root: {error}"))?;
        let root_uri = Url::from_directory_path(&root)
            .map(String::from)
            .map_err(|()| format!("could not convert {} to a directory URI", root.display()))?;
        let workspace_name = root
            .file_name()
            .and_then(|name| name.to_str())
            .filter(|name| !name.is_empty())
            .unwrap_or("workspace")
            .to_owned();

        let mut command = Command::new(&config.program);
        command
            .args(&config.arguments)
            .current_dir(&root)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child = command.spawn().map_err(|error| {
            format!(
                "could not start `{}`: {error}",
                config.program.to_string_lossy()
            )
        })?;

        let stdio = (child.stdin.take(), child.stdout.take(), child.stderr.take());
        let (Some(stdin), Some(stdout), Some(stderr)) = stdio else {
            // Child does not kill-on-drop. Reap it explicitly even for the
            // theoretically impossible case of a missing piped handle.
            let _ = child.kill();
            let _ = child.wait();
            return Err("could not open Tinymist standard streams".to_owned());
        };

        let (incoming_tx, incoming_rx) = mpsc::channel();
        let stdout_tx = incoming_tx.clone();
        let stdout_reader = match thread::Builder::new()
            .name(format!("tiptoptyp-tinymist-stdout-{}", generation.0))
            .spawn(move || stdout_loop(stdout, stdout_tx))
        {
            Ok(reader) => reader,
            Err(error) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!("could not start Tinymist stdout reader: {error}"));
            }
        };
        let stderr_reader = match thread::Builder::new()
            .name(format!("tiptoptyp-tinymist-stderr-{}", generation.0))
            .spawn(move || stderr_loop(stderr, incoming_tx))
        {
            Ok(reader) => reader,
            Err(error) => {
                let _ = child.kill();
                let _ = child.wait();
                let _ = finish_reader_before(
                    stdout_reader,
                    Instant::now() + PIPE_READER_SHUTDOWN_TIMEOUT,
                );
                return Err(format!("could not start Tinymist stderr reader: {error}"));
            }
        };

        let child = Arc::new(Mutex::new(child));
        process_supervisor.install(generation, &child);
        let mut session = Self {
            generation,
            phase: SessionPhase::Initializing,
            child,
            process_supervisor,
            stdin: Some(stdin),
            incoming: incoming_rx,
            stdout_reader: Some(stdout_reader),
            stderr_reader: Some(stderr_reader),
            next_request_id: 1,
            pending: HashMap::new(),
            documents: HashMap::new(),
            settings: config.server_settings(),
            start_preview: config.start_preview,
            root_uri,
            workspace_name,
            request_timeout_override: config.request_timeout_override,
            cleaned_up: false,
        };
        initialize(&mut session)?;
        Ok(session)
    }

    fn mark_cleaned_up(&mut self) {
        self.cleaned_up = true;
    }

    fn send_initialize(&mut self) -> std::result::Result<(), String> {
        let params = json!({
            "processId": std::process::id(),
            "clientInfo": {
                "name": "tiptoptyp",
                "version": env!("CARGO_PKG_VERSION"),
            },
            "rootUri": self.root_uri,
            "capabilities": {
                "workspace": {
                    "configuration": true,
                    "workspaceFolders": true,
                    "executeCommand": { "dynamicRegistration": false },
                },
                "textDocument": {
                    "synchronization": {
                        "dynamicRegistration": false,
                        "didSave": false,
                    },
                    "formatting": {
                        "dynamicRegistration": false,
                    },
                    "completion": {
                        "dynamicRegistration": false,
                        "contextSupport": true,
                        "completionItem": {
                            "snippetSupport": true,
                            "documentationFormat": ["markdown", "plaintext"],
                            "insertReplaceSupport": true,
                        },
                    },
                    "publishDiagnostics": {
                        "relatedInformation": true,
                        "versionSupport": true,
                        "tagSupport": { "valueSet": [1, 2] },
                        "codeDescriptionSupport": true,
                        "dataSupport": true,
                    },
                },
                "window": {
                    "showDocument": { "support": true },
                    "workDoneProgress": true,
                },
                "general": {
                    "positionEncodings": ["utf-16"],
                },
            },
            "trace": "off",
            "workspaceFolders": [{
                "uri": self.root_uri,
                "name": self.workspace_name,
            }],
        });
        self.send_request("initialize", params, PendingRequest::Initialize)?;
        Ok(())
    }

    fn send_request(
        &mut self,
        method: &str,
        params: Value,
        pending: PendingRequest,
    ) -> std::result::Result<u64, String> {
        let id = self.next_request_id;
        self.next_request_id = self
            .next_request_id
            .checked_add(1)
            .ok_or_else(|| "Tinymist request ID overflow".to_owned())?;
        self.write(&json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params,
        }))?;
        let timeout = self
            .request_timeout_override
            .unwrap_or_else(|| pending.timeout());
        self.pending.insert(
            id,
            PendingCall {
                request: pending,
                timeout,
                deadline: Instant::now() + timeout,
            },
        );
        Ok(id)
    }

    fn cancel_pending_completions(&mut self, uri: &str) -> std::result::Result<(), String> {
        let ids = self
            .pending
            .iter()
            .filter_map(|(id, call)| {
                matches!(
                    &call.request,
                    PendingRequest::CompleteDocument {
                        uri: pending_uri,
                        ..
                    } if pending_uri == uri
                )
                .then_some(*id)
            })
            .collect::<Vec<_>>();
        for id in ids {
            self.pending.remove(&id);
            self.notify("$/cancelRequest", json!({ "id": id }))?;
        }
        Ok(())
    }

    fn expired_request(&self, now: Instant) -> Option<(u64, &PendingCall)> {
        self.pending
            .iter()
            .filter(|(_, pending)| pending.deadline <= now)
            .min_by_key(|(id, pending)| (pending.deadline, **id))
            .map(|(id, pending)| (*id, pending))
    }

    fn notify(&mut self, method: &str, params: Value) -> std::result::Result<(), String> {
        self.write(&json!({
            "jsonrpc": "2.0",
            "method": method,
            "params": params,
        }))
    }

    fn set_preview_refresh(&mut self, refresh: PreviewRefresh) -> std::result::Result<(), String> {
        self.settings["preview"]["refresh"] = Value::String(refresh.as_setting().to_owned());
        if self.phase.can_sync_documents() {
            let settings = self.settings.clone();
            self.notify(
                "workspace/didChangeConfiguration",
                json!({ "settings": settings }),
            )?;
        }
        Ok(())
    }

    fn write(&mut self, message: &impl Serialize) -> std::result::Result<(), String> {
        let stdin = self
            .stdin
            .as_mut()
            .ok_or_else(|| "Tinymist stdin is closed".to_owned())?;
        write_lsp_message(stdin, message)
            .map_err(|error| format!("could not write to Tinymist: {error}"))
    }

    fn send_did_open(&mut self, document: &TextDocument) -> std::result::Result<(), String> {
        self.write(&Notification {
            jsonrpc: "2.0",
            method: "textDocument/didOpen",
            params: DidOpenParams {
                text_document: document,
            },
        })
    }

    fn send_did_change(&mut self, document: &TextDocument) -> std::result::Result<(), String> {
        self.send_did_change_parts(&document.uri, document.version, &document.text)
    }

    fn send_did_change_parts(
        &mut self,
        uri: &str,
        version: i32,
        text: &str,
    ) -> std::result::Result<(), String> {
        self.write(&Notification {
            jsonrpc: "2.0",
            method: "textDocument/didChange",
            params: DidChangeParams {
                text_document: VersionedDocument { uri, version },
                content_changes: [ContentChange { text }],
            },
        })
    }

    fn send_did_close(&mut self, uri: &str) -> std::result::Result<(), String> {
        self.notify(
            "textDocument/didClose",
            json!({ "textDocument": { "uri": uri } }),
        )
    }

    fn reply_result(&mut self, id: Value, result: Value) -> std::result::Result<(), String> {
        self.write(&json!({ "jsonrpc": "2.0", "id": id, "result": result }))
    }

    fn reply_error(
        &mut self,
        id: Value,
        code: i64,
        message: impl Into<String>,
    ) -> std::result::Result<(), String> {
        self.write(&json!({
            "jsonrpc": "2.0",
            "id": id,
            "error": { "code": code, "message": message.into() },
        }))
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        if !self.cleaned_up {
            // `Child` does not terminate on drop. Closing stdin first gives a
            // cooperative server an EOF; bounded reaping then kills a server
            // that remains alive. Reader handles are joined only after the
            // process and its pipe writers are closed.
            self.stdin.take();
            let _ = reap_child(&self.child);
            join_readers(self);
            self.cleaned_up = true;
        }
        self.process_supervisor.clear(self.generation, &self.child);
    }
}

fn absolute_path(path: &Path) -> io::Result<PathBuf> {
    if path.is_absolute() {
        Ok(path.to_owned())
    } else {
        Ok(std::env::current_dir()?.join(path))
    }
}

fn stdout_loop(stdout: impl Read, messages: Sender<Incoming>) {
    let mut reader = BufReader::new(stdout);
    loop {
        match read_lsp_message(&mut reader) {
            Ok(Some(message)) => {
                if messages.send(Incoming::Message(message)).is_err() {
                    break;
                }
            }
            Ok(None) => {
                let _ = messages.send(Incoming::EndOfStream);
                break;
            }
            Err(error) => {
                let _ = messages.send(Incoming::ReadError(error.to_string()));
                break;
            }
        }
    }
}

fn stderr_loop(stderr: impl Read, messages: Sender<Incoming>) {
    let reader = BufReader::new(stderr);
    for line in reader.lines() {
        match line {
            Ok(line) if !line.trim().is_empty() => {
                if messages.send(Incoming::Stderr(line)).is_err() {
                    break;
                }
            }
            Ok(_) => {}
            Err(error) => {
                let _ = messages.send(Incoming::Stderr(format!(
                    "could not read Tinymist stderr: {error}"
                )));
                break;
            }
        }
    }
}

fn worker_loop(
    commands: Receiver<WorkerCommand>,
    events: Sender<TinymistEvent>,
    context: egui::Context,
    current_generation: Arc<AtomicU64>,
    process_supervisor: Arc<ProcessSupervisor>,
) {
    let mut session: Option<Session> = None;

    'worker: loop {
        let mut session_failure = None;
        if let Some(active) = session.as_mut() {
            for _ in 0..128 {
                match active.incoming.try_recv() {
                    Ok(message) => {
                        if let Err((stage, error)) =
                            handle_incoming(active, message, &events, &context, &current_generation)
                        {
                            session_failure = Some((stage, error));
                            break;
                        }
                    }
                    Err(TryRecvError::Empty) => break,
                    Err(TryRecvError::Disconnected) => {
                        session_failure = Some((
                            "protocol",
                            "Tinymist output channels closed unexpectedly".to_owned(),
                        ));
                        break;
                    }
                }
            }
        }

        if session_failure.is_none()
            && let Some(active) = session.as_ref()
            && let Some((id, pending)) = active.expired_request(Instant::now())
        {
            session_failure = Some((
                "timeout",
                format!(
                    "Tinymist did not answer {} request {id} within {:?}",
                    pending.request.method(),
                    pending.timeout
                ),
            ));
        }

        if let Some((stage, error)) = session_failure
            && let Some(active) = session.take()
        {
            emit(
                &events,
                &context,
                &current_generation,
                TinymistEvent::Error {
                    generation: active.generation,
                    stage,
                    message: error.clone(),
                    fatal: true,
                },
            );
            finish_session(
                active,
                false,
                format!("Tinymist {stage} failure: {error}"),
                &events,
                &context,
                &current_generation,
            );
        }

        if let Some(active) = session.as_mut() {
            match try_wait_child(&active.child) {
                Ok(Some(status)) => {
                    let active = session.take().expect("active session disappeared");
                    finish_exited_session(
                        active,
                        status,
                        "Tinymist exited unexpectedly".to_owned(),
                        &events,
                        &context,
                        &current_generation,
                    );
                }
                Ok(None) => {}
                Err(error) => {
                    let active = session.take().expect("active session disappeared");
                    emit(
                        &events,
                        &context,
                        &current_generation,
                        TinymistEvent::Error {
                            generation: active.generation,
                            stage: "process",
                            message: format!("could not inspect Tinymist process: {error}"),
                            fatal: true,
                        },
                    );
                    finish_session(
                        active,
                        false,
                        "could not inspect Tinymist process".to_owned(),
                        &events,
                        &context,
                        &current_generation,
                    );
                }
            }
        }

        let command = match commands.recv_timeout(WORKER_POLL_INTERVAL) {
            Ok(command) => command,
            Err(RecvTimeoutError::Timeout) => continue,
            Err(RecvTimeoutError::Disconnected) => break,
        };

        match command {
            WorkerCommand::Start { generation, config } => {
                if current_generation.load(Ordering::Acquire) != generation.0 {
                    continue;
                }
                if let Some(active) = session.take() {
                    finish_session(
                        active,
                        true,
                        "workspace replaced".to_owned(),
                        &events,
                        &context,
                        &current_generation,
                    );
                }
                emit(
                    &events,
                    &context,
                    &current_generation,
                    TinymistEvent::Starting { generation },
                );
                match Session::spawn(generation, config, Arc::clone(&process_supervisor)) {
                    Ok(active) => session = Some(active),
                    Err(error) => {
                        emit(
                            &events,
                            &context,
                            &current_generation,
                            TinymistEvent::Error {
                                generation,
                                stage: "spawn",
                                message: error.clone(),
                                fatal: true,
                            },
                        );
                        emit(
                            &events,
                            &context,
                            &current_generation,
                            TinymistEvent::Stopped {
                                generation,
                                code: None,
                                reason: error,
                            },
                        );
                    }
                }
            }
            WorkerCommand::DidOpen {
                generation,
                document,
            } => {
                let Some(active) = matching_session(session.as_mut(), generation) else {
                    continue;
                };
                if let Some(previous) = active.documents.get(&document.uri)
                    && document.version <= previous.version
                {
                    emit(
                        &events,
                        &context,
                        &current_generation,
                        TinymistEvent::Error {
                            generation,
                            stage: "document",
                            message: format!(
                                "ignored non-monotonic open version {} for {}; current version is {}",
                                document.version, document.uri, previous.version
                            ),
                            fatal: false,
                        },
                    );
                    continue;
                }
                let uri = document.uri.clone();
                let replacing = active.documents.contains_key(&uri);
                let result = if active.phase.can_sync_documents() {
                    if replacing {
                        active.send_did_change(&document)
                    } else {
                        active.send_did_open(&document)
                    }
                } else {
                    Ok(())
                };
                if let Err(error) = result {
                    fail_active_session(
                        &mut session,
                        "write",
                        error,
                        &events,
                        &context,
                        &current_generation,
                    );
                } else {
                    active.documents.insert(uri, document);
                }
            }
            WorkerCommand::DidChange {
                generation,
                uri,
                version,
                text,
            } => {
                let Some(active) = matching_session(session.as_mut(), generation) else {
                    continue;
                };
                let Some(document) = active.documents.get(&uri) else {
                    emit(
                        &events,
                        &context,
                        &current_generation,
                        TinymistEvent::Error {
                            generation,
                            stage: "document",
                            message: format!("ignored change for unopened document {uri}"),
                            fatal: false,
                        },
                    );
                    continue;
                };
                if version == document.version && text == document.text {
                    continue;
                }
                if version <= document.version {
                    emit(
                        &events,
                        &context,
                        &current_generation,
                        TinymistEvent::Error {
                            generation,
                            stage: "document",
                            message: format!(
                                "ignored non-monotonic version {version} for {uri}; current version is {}",
                                document.version
                            ),
                            fatal: false,
                        },
                    );
                    continue;
                }
                let result = if active.phase.can_sync_documents() {
                    active.send_did_change_parts(&uri, version, &text)
                } else {
                    Ok(())
                };
                if let Err(error) = result {
                    fail_active_session(
                        &mut session,
                        "write",
                        error,
                        &events,
                        &context,
                        &current_generation,
                    );
                } else if let Some(document) = active.documents.get_mut(&uri) {
                    document.version = version;
                    document.text = text;
                }
            }
            WorkerCommand::DidClose { generation, uri } => {
                let Some(active) = matching_session(session.as_mut(), generation) else {
                    continue;
                };
                if active.documents.remove(&uri).is_some()
                    && active.phase.can_sync_documents()
                    && let Err(error) = active.send_did_close(&uri)
                {
                    fail_active_session(
                        &mut session,
                        "write",
                        error,
                        &events,
                        &context,
                        &current_generation,
                    );
                }
            }
            WorkerCommand::SetPreviewRefresh {
                generation,
                refresh,
            } => {
                let Some(active) = matching_session(session.as_mut(), generation) else {
                    continue;
                };
                if let Err(error) = active.set_preview_refresh(refresh) {
                    fail_active_session(
                        &mut session,
                        "write",
                        error,
                        &events,
                        &context,
                        &current_generation,
                    );
                }
            }
            WorkerCommand::FormatDocument {
                generation,
                uri,
                version,
            } => {
                let Some(active) = matching_session(session.as_mut(), generation) else {
                    continue;
                };
                let Some(current_version) =
                    active.documents.get(&uri).map(|document| document.version)
                else {
                    emit(
                        &events,
                        &context,
                        &current_generation,
                        TinymistEvent::Error {
                            generation,
                            stage: "formatting",
                            message: format!(
                                "cannot format unopened document {uri} at version {version}"
                            ),
                            fatal: false,
                        },
                    );
                    continue;
                };
                if current_version != version {
                    emit(
                        &events,
                        &context,
                        &current_generation,
                        TinymistEvent::Error {
                            generation,
                            stage: "formatting",
                            message: format!(
                                "ignored stale formatting request for {uri} at version {version}; current version is {current_version}"
                            ),
                            fatal: false,
                        },
                    );
                    continue;
                }
                if !active.phase.can_sync_documents() {
                    emit(
                        &events,
                        &context,
                        &current_generation,
                        TinymistEvent::Error {
                            generation,
                            stage: "formatting",
                            message: "Tinymist is not initialized yet".to_owned(),
                            fatal: false,
                        },
                    );
                    continue;
                }
                if let Err(error) = active.send_request(
                    "textDocument/formatting",
                    format_document_params(&uri),
                    PendingRequest::FormatDocument { uri, version },
                ) {
                    fail_active_session(
                        &mut session,
                        "write",
                        error,
                        &events,
                        &context,
                        &current_generation,
                    );
                }
            }
            WorkerCommand::HoverDocument {
                generation,
                uri,
                version,
                position,
                request_token,
            } => {
                let Some(active) = matching_session(session.as_mut(), generation) else {
                    continue;
                };
                let current_version = active.documents.get(&uri).map(|document| document.version);
                if current_version != Some(version) || !active.phase.can_sync_documents() {
                    emit(
                        &events,
                        &context,
                        &current_generation,
                        TinymistEvent::Hovered {
                            generation,
                            uri,
                            version,
                            request_token,
                            contents: None,
                            range: None,
                        },
                    );
                    continue;
                }
                if let Err(error) = active.send_request(
                    "textDocument/hover",
                    hover_document_params(&uri, position),
                    PendingRequest::HoverDocument {
                        uri,
                        version,
                        request_token,
                    },
                ) {
                    fail_active_session(
                        &mut session,
                        "write",
                        error,
                        &events,
                        &context,
                        &current_generation,
                    );
                }
            }
            WorkerCommand::CompleteDocument {
                generation,
                uri,
                version,
                position,
                request_token,
            } => {
                let Some(active) = matching_session(session.as_mut(), generation) else {
                    continue;
                };
                let current_version = active.documents.get(&uri).map(|document| document.version);
                if current_version != Some(version) || !active.phase.can_sync_documents() {
                    emit(
                        &events,
                        &context,
                        &current_generation,
                        TinymistEvent::Completed {
                            generation,
                            uri,
                            version,
                            request_token,
                            is_incomplete: false,
                            items: Vec::new(),
                        },
                    );
                    continue;
                }
                let result = active.cancel_pending_completions(&uri).and_then(|()| {
                    active.send_request(
                        "textDocument/completion",
                        completion_document_params(&uri, position),
                        PendingRequest::CompleteDocument {
                            uri,
                            version,
                            request_token,
                        },
                    )?;
                    Ok(())
                });
                if let Err(error) = result {
                    fail_active_session(
                        &mut session,
                        "write",
                        error,
                        &events,
                        &context,
                        &current_generation,
                    );
                }
            }
            WorkerCommand::ScrollPreview {
                generation,
                path,
                line,
                character,
            } => {
                let Some(active) = matching_session(session.as_mut(), generation) else {
                    continue;
                };
                if active.phase != SessionPhase::Running {
                    emit(
                        &events,
                        &context,
                        &current_generation,
                        TinymistEvent::Error {
                            generation,
                            stage: "navigation",
                            message: "the interactive preview is not ready yet".to_owned(),
                            fatal: false,
                        },
                    );
                    continue;
                }
                if let Err(error) = active.send_request(
                    "workspace/executeCommand",
                    scroll_preview_params(&path, line, character),
                    PendingRequest::ScrollPreview,
                ) {
                    fail_active_session(
                        &mut session,
                        "write",
                        error,
                        &events,
                        &context,
                        &current_generation,
                    );
                }
            }
            WorkerCommand::Stop { generation } => {
                if session.as_ref().map(|active| active.generation) == Some(generation) {
                    let active = session.take().expect("matching session disappeared");
                    finish_session(
                        active,
                        true,
                        "stopped".to_owned(),
                        &events,
                        &context,
                        &current_generation,
                    );
                }
            }
            WorkerCommand::Shutdown => break 'worker,
        }
    }

    if let Some(active) = session.take() {
        finish_session(
            active,
            true,
            "application shutdown".to_owned(),
            &events,
            &context,
            &current_generation,
        );
    }
}

fn matching_session(session: Option<&mut Session>, generation: Generation) -> Option<&mut Session> {
    session.filter(|active| active.generation == generation)
}

fn fail_active_session(
    session: &mut Option<Session>,
    stage: &'static str,
    error: String,
    events: &Sender<TinymistEvent>,
    context: &egui::Context,
    current_generation: &AtomicU64,
) {
    let Some(active) = session.take() else {
        return;
    };
    emit(
        events,
        context,
        current_generation,
        TinymistEvent::Error {
            generation: active.generation,
            stage,
            message: error.clone(),
            fatal: true,
        },
    );
    finish_session(
        active,
        false,
        format!("Tinymist {stage} failure: {error}"),
        events,
        context,
        current_generation,
    );
}

fn handle_incoming(
    session: &mut Session,
    incoming: Incoming,
    events: &Sender<TinymistEvent>,
    context: &egui::Context,
    current_generation: &AtomicU64,
) -> std::result::Result<(), (&'static str, String)> {
    match incoming {
        Incoming::Message(message) => {
            handle_rpc_message(session, message, events, context, current_generation)
                .map_err(|error| ("protocol", error))
        }
        Incoming::Stderr(message) => {
            emit(
                events,
                context,
                current_generation,
                TinymistEvent::Log {
                    generation: session.generation,
                    level: None,
                    message,
                },
            );
            Ok(())
        }
        Incoming::EndOfStream => Err(("protocol", "Tinymist closed its output stream".to_owned())),
        Incoming::ReadError(error) => Err(("framing", error)),
    }
}

fn handle_rpc_message(
    session: &mut Session,
    message: Value,
    events: &Sender<TinymistEvent>,
    context: &egui::Context,
    current_generation: &AtomicU64,
) -> std::result::Result<(), String> {
    let object = message
        .as_object()
        .ok_or_else(|| "Tinymist sent a non-object JSON-RPC message".to_owned())?;

    if let Some(method) = object.get("method").and_then(Value::as_str) {
        let params = object.get("params").cloned().unwrap_or(Value::Null);
        if let Some(id) = object.get("id").cloned() {
            return handle_server_request(
                session,
                id,
                method,
                params,
                events,
                context,
                current_generation,
            );
        }
        handle_server_notification(session, method, params, events, context, current_generation);
        return Ok(());
    }

    let id = object
        .get("id")
        .and_then(Value::as_u64)
        .ok_or_else(|| "Tinymist sent a response without a numeric request ID".to_owned())?;
    let Some(pending) = session.pending.remove(&id) else {
        // Late cancellation responses are harmless; do not make an optional
        // sidecar fatal because a server raced a workspace transition.
        return Ok(());
    };
    let pending = pending.request;

    if let Some(error) = object.get("error").filter(|error| !error.is_null()) {
        let message = rpc_error_message(error);
        match pending {
            PendingRequest::Initialize => return Err(format!("initialize failed: {message}")),
            PendingRequest::StartPreview => {
                session.phase = SessionPhase::Initialized;
                emit(
                    events,
                    context,
                    current_generation,
                    TinymistEvent::Error {
                        generation: session.generation,
                        stage: "preview",
                        message,
                        fatal: false,
                    },
                );
                return Ok(());
            }
            PendingRequest::ScrollPreview => {
                emit(
                    events,
                    context,
                    current_generation,
                    TinymistEvent::Error {
                        generation: session.generation,
                        stage: "navigation",
                        message,
                        fatal: false,
                    },
                );
                return Ok(());
            }
            PendingRequest::FormatDocument { .. } => {
                emit(
                    events,
                    context,
                    current_generation,
                    TinymistEvent::Error {
                        generation: session.generation,
                        stage: "formatting",
                        message,
                        fatal: false,
                    },
                );
                return Ok(());
            }
            PendingRequest::HoverDocument {
                uri,
                version,
                request_token,
            } => {
                emit(
                    events,
                    context,
                    current_generation,
                    TinymistEvent::Hovered {
                        generation: session.generation,
                        uri,
                        version,
                        request_token,
                        contents: None,
                        range: None,
                    },
                );
                return Ok(());
            }
            PendingRequest::CompleteDocument {
                uri,
                version,
                request_token,
            } => {
                emit(
                    events,
                    context,
                    current_generation,
                    TinymistEvent::CompletionFailed {
                        generation: session.generation,
                        uri,
                        version,
                        request_token,
                        message,
                    },
                );
                return Ok(());
            }
        }
    }

    let result = object.get("result").cloned().unwrap_or(Value::Null);
    match pending {
        PendingRequest::Initialize => {
            session.notify("initialized", json!({}))?;
            session.notify(
                "workspace/didChangeConfiguration",
                json!({ "settings": session.settings }),
            )?;
            session.phase = SessionPhase::Initialized;

            let documents = std::mem::take(&mut session.documents);
            for document in documents.values() {
                session.send_did_open(document)?;
            }
            session.documents = documents;
            emit(
                events,
                context,
                current_generation,
                TinymistEvent::Initialized {
                    generation: session.generation,
                },
            );

            if session.start_preview {
                session.phase = SessionPhase::StartingPreview;
                session.send_request(
                    "workspace/executeCommand",
                    json!({
                        "command": "tinymist.startDefaultPreview",
                        "arguments": [],
                    }),
                    PendingRequest::StartPreview,
                )?;
            }
        }
        PendingRequest::StartPreview => {
            let port = result
                .get("staticServerPort")
                .and_then(parse_port)
                .ok_or_else(|| {
                    "tinymist.startDefaultPreview returned no valid staticServerPort".to_owned()
                });
            match port {
                Ok(port) => {
                    session.phase = SessionPhase::Running;
                    emit(
                        events,
                        context,
                        current_generation,
                        TinymistEvent::PreviewReady {
                            generation: session.generation,
                            url: format!("http://127.0.0.1:{port}"),
                        },
                    );
                }
                Err(message) => {
                    session.phase = SessionPhase::Initialized;
                    emit(
                        events,
                        context,
                        current_generation,
                        TinymistEvent::Error {
                            generation: session.generation,
                            stage: "preview",
                            message,
                            fatal: false,
                        },
                    );
                }
            }
        }
        PendingRequest::ScrollPreview => {}
        PendingRequest::FormatDocument { uri, version } => {
            match parse_format_document_result(result) {
                Ok(edits) => emit(
                    events,
                    context,
                    current_generation,
                    TinymistEvent::Formatted {
                        generation: session.generation,
                        uri,
                        version,
                        edits,
                    },
                ),
                Err(error) => emit(
                    events,
                    context,
                    current_generation,
                    TinymistEvent::Error {
                        generation: session.generation,
                        stage: "formatting",
                        message: format!("invalid textDocument/formatting response: {error}"),
                        fatal: false,
                    },
                ),
            }
        }
        PendingRequest::HoverDocument {
            uri,
            version,
            request_token,
        } => {
            let (contents, range) = parse_hover_result(&result);
            emit(
                events,
                context,
                current_generation,
                TinymistEvent::Hovered {
                    generation: session.generation,
                    uri,
                    version,
                    request_token,
                    contents,
                    range,
                },
            );
        }
        PendingRequest::CompleteDocument {
            uri,
            version,
            request_token,
        } => match parse_completion_result(&result) {
            Ok((is_incomplete, items)) => emit(
                events,
                context,
                current_generation,
                TinymistEvent::Completed {
                    generation: session.generation,
                    uri,
                    version,
                    request_token,
                    is_incomplete,
                    items,
                },
            ),
            Err(error) => emit(
                events,
                context,
                current_generation,
                TinymistEvent::CompletionFailed {
                    generation: session.generation,
                    uri,
                    version,
                    request_token,
                    message: format!("invalid textDocument/completion response: {error}"),
                },
            ),
        },
    }
    Ok(())
}

fn format_document_params(uri: &str) -> Value {
    json!({
        "textDocument": { "uri": uri },
        "options": {
            "tabSize": 2,
            "insertSpaces": true,
        },
    })
}

fn hover_document_params(uri: &str, position: LspPosition) -> Value {
    json!({
        "textDocument": { "uri": uri },
        "position": position,
    })
}

fn completion_document_params(uri: &str, position: LspPosition) -> Value {
    json!({
        "textDocument": { "uri": uri },
        "position": position,
        "context": { "triggerKind": 1 },
    })
}

fn parse_hover_result(result: &Value) -> (Option<String>, Option<LspRange>) {
    let Some(object) = result.as_object() else {
        return (None, None);
    };
    let range = object
        .get("range")
        .and_then(|range| serde_json::from_value(range.clone()).ok());
    let contents = object
        .get("contents")
        .and_then(flatten_hover_contents)
        .map(|contents| contents.trim().to_owned())
        .filter(|contents| !contents.is_empty());
    (contents, range)
}

fn flatten_hover_contents(contents: &Value) -> Option<String> {
    match contents {
        Value::String(text) => Some(text.clone()),
        Value::Array(parts) => {
            let parts = parts
                .iter()
                .filter_map(flatten_hover_contents)
                .filter(|part| !part.trim().is_empty())
                .collect::<Vec<_>>();
            (!parts.is_empty()).then(|| parts.join("\n\n"))
        }
        Value::Object(object) => {
            let value = object.get("value")?.as_str()?;
            let language = object.get("language").and_then(Value::as_str);
            Some(language.map_or_else(
                || value.to_owned(),
                |language| format!("```{}\n{value}\n```", language.trim()),
            ))
        }
        Value::Null | Value::Bool(_) | Value::Number(_) => None,
    }
}

fn parse_completion_result(
    result: &Value,
) -> std::result::Result<(bool, Vec<CompletionItem>), String> {
    if result.is_null() {
        return Ok((false, Vec::new()));
    }
    let (is_incomplete, items, defaults) = if let Some(items) = result.as_array() {
        (false, items.as_slice(), None)
    } else {
        let object = result
            .as_object()
            .ok_or_else(|| "completion result is neither a list nor CompletionList".to_owned())?;
        let items = object
            .get("items")
            .and_then(Value::as_array)
            .ok_or_else(|| "CompletionList.items is not an array".to_owned())?;
        (
            object
                .get("isIncomplete")
                .and_then(Value::as_bool)
                .unwrap_or(false),
            items.as_slice(),
            object.get("itemDefaults").and_then(Value::as_object),
        )
    };

    let default_range = defaults
        .and_then(|defaults| defaults.get("editRange"))
        .and_then(completion_edit_range);
    let default_snippet = defaults
        .and_then(|defaults| defaults.get("insertTextFormat"))
        .and_then(Value::as_u64)
        == Some(2);
    let mut parsed = Vec::with_capacity(items.len());
    for value in items {
        let object = value
            .as_object()
            .ok_or_else(|| "completion item is not an object".to_owned())?;
        let label = object
            .get("label")
            .and_then(Value::as_str)
            .ok_or_else(|| "completion item has no string label".to_owned())?
            .to_owned();
        let inserted = object
            .get("textEditText")
            .or_else(|| object.get("insertText"))
            .and_then(Value::as_str)
            .unwrap_or(&label)
            .to_owned();
        let text_edit = match object.get("textEdit") {
            Some(value) => Some(parse_completion_text_edit(value)?),
            None => default_range.map(|range| LspTextEdit {
                range,
                new_text: inserted.clone(),
            }),
        };
        let additional_text_edits = object
            .get("additionalTextEdits")
            .map(parse_completion_additional_edits)
            .transpose()?
            .unwrap_or_default();
        parsed.push(CompletionItem {
            label,
            detail: object
                .get("detail")
                .and_then(Value::as_str)
                .map(str::to_owned),
            documentation: object
                .get("documentation")
                .and_then(completion_documentation),
            filter_text: object
                .get("filterText")
                .and_then(Value::as_str)
                .map(str::to_owned),
            sort_text: object
                .get("sortText")
                .and_then(Value::as_str)
                .map(str::to_owned),
            insert_text: inserted,
            insert_text_is_snippet: object
                .get("insertTextFormat")
                .and_then(Value::as_u64)
                .map_or(default_snippet, |format| format == 2),
            text_edit,
            additional_text_edits,
        });
    }
    Ok((is_incomplete, parsed))
}

fn completion_edit_range(value: &Value) -> Option<LspRange> {
    serde_json::from_value(value.clone()).ok().or_else(|| {
        value
            .get("replace")
            .or_else(|| value.get("insert"))
            .and_then(|range| serde_json::from_value(range.clone()).ok())
    })
}

fn parse_completion_text_edit(value: &Value) -> std::result::Result<LspTextEdit, String> {
    let new_text = value
        .get("newText")
        .and_then(Value::as_str)
        .ok_or_else(|| "completion textEdit has no newText".to_owned())?
        .to_owned();
    let range = value
        .get("range")
        .or_else(|| value.get("replace"))
        .or_else(|| value.get("insert"))
        .and_then(|range| serde_json::from_value(range.clone()).ok())
        .ok_or_else(|| "completion textEdit has no valid range".to_owned())?;
    Ok(LspTextEdit { range, new_text })
}

fn parse_completion_additional_edits(
    value: &Value,
) -> std::result::Result<Vec<LspTextEdit>, String> {
    let values = value
        .as_array()
        .ok_or_else(|| "additionalTextEdits is not an array".to_owned())?;
    values.iter().map(parse_completion_text_edit).collect()
}

fn completion_documentation(value: &Value) -> Option<String> {
    match value {
        Value::String(text) => Some(text.clone()),
        Value::Object(object) => object
            .get("value")
            .and_then(Value::as_str)
            .map(str::to_owned),
        _ => None,
    }
}

fn parse_format_document_result(
    result: Value,
) -> std::result::Result<Option<Vec<LspTextEdit>>, serde_json::Error> {
    serde_json::from_value(result)
}

fn scroll_preview_params(path: &Path, line: u32, character: u32) -> Value {
    json!({
        "command": "tinymist.scrollPreview",
        "arguments": [
            DEFAULT_PREVIEW_TASK_ID,
            {
                "event": "panelScrollTo",
                "filepath": path.to_string_lossy(),
                "line": line,
                "character": character,
            }
        ],
    })
}

fn parse_port(value: &Value) -> Option<u16> {
    let port = value
        .as_u64()
        .or_else(|| value.as_str()?.parse::<u64>().ok())?;
    let port = u16::try_from(port).ok()?;
    (port != 0).then_some(port)
}

fn rpc_error_message(error: &Value) -> String {
    let message = error
        .get("message")
        .and_then(Value::as_str)
        .unwrap_or("unknown JSON-RPC error");
    match error.get("code").and_then(Value::as_i64) {
        Some(code) => format!("{message} (JSON-RPC error {code})"),
        None => message.to_owned(),
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ShowDocumentParams {
    uri: String,
    external: Option<bool>,
    take_focus: Option<bool>,
    selection: Option<LspRange>,
}

fn handle_server_request(
    session: &mut Session,
    id: Value,
    method: &str,
    params: Value,
    events: &Sender<TinymistEvent>,
    context: &egui::Context,
    current_generation: &AtomicU64,
) -> std::result::Result<(), String> {
    match method {
        "window/showDocument" => {
            let parsed = serde_json::from_value::<ShowDocumentParams>(params.clone());
            match parsed {
                Ok(request) => {
                    emit(
                        events,
                        context,
                        current_generation,
                        TinymistEvent::ShowDocument {
                            generation: session.generation,
                            uri: request.uri,
                            selection: request.selection,
                            external: request.external,
                            take_focus: request.take_focus,
                            raw: params,
                        },
                    );
                    // Queueing the app event is the successful handling of this
                    // request. The UI applies it on its next frame.
                    session.reply_result(id, json!({ "success": true }))
                }
                Err(error) => {
                    emit(
                        events,
                        context,
                        current_generation,
                        TinymistEvent::Error {
                            generation: session.generation,
                            stage: "showDocument",
                            message: error.to_string(),
                            fatal: false,
                        },
                    );
                    session.reply_error(id, -32602, format!("invalid showDocument params: {error}"))
                }
            }
        }
        "workspace/configuration" => {
            let result = configuration_response(&params, &session.settings);
            session.reply_result(id, result)
        }
        "workspace/workspaceFolders" => session.reply_result(
            id,
            json!([{ "uri": session.root_uri, "name": session.workspace_name }]),
        ),
        "client/registerCapability"
        | "client/unregisterCapability"
        | "window/workDoneProgress/create"
        | "workspace/semanticTokens/refresh"
        | "workspace/diagnostic/refresh"
        | "workspace/codeLens/refresh"
        | "workspace/inlayHint/refresh" => session.reply_result(id, Value::Null),
        "window/showMessageRequest" => session.reply_result(id, Value::Null),
        "workspace/applyEdit" => session.reply_result(
            id,
            json!({
                "applied": false,
                "failureReason": "tiptoptyp does not support server-initiated workspace edits",
            }),
        ),
        _ => session.reply_error(id, -32601, format!("unsupported server request {method}")),
    }
}

fn configuration_response(params: &Value, settings: &Value) -> Value {
    let Some(items) = params.get("items").and_then(Value::as_array) else {
        return Value::Array(Vec::new());
    };
    Value::Array(
        items
            .iter()
            .map(|item| {
                item.get("section")
                    .and_then(Value::as_str)
                    .map(|section| configuration_section(settings, section))
                    .unwrap_or_else(|| settings.clone())
            })
            .collect(),
    )
}

fn configuration_section(settings: &Value, section: &str) -> Value {
    if section.is_empty() || section == "tinymist" {
        return settings.clone();
    }
    let section = section.strip_prefix("tinymist.").unwrap_or(section);
    section
        .split('.')
        .try_fold(settings, |value, component| value.get(component))
        .cloned()
        .unwrap_or(Value::Null)
}

fn handle_server_notification(
    session: &Session,
    method: &str,
    params: Value,
    events: &Sender<TinymistEvent>,
    context: &egui::Context,
    current_generation: &AtomicU64,
) {
    match method {
        "textDocument/publishDiagnostics" => {
            let Some(uri) = params.get("uri").and_then(Value::as_str) else {
                emit(
                    events,
                    context,
                    current_generation,
                    TinymistEvent::Notification {
                        generation: session.generation,
                        method: method.to_owned(),
                        params,
                    },
                );
                return;
            };
            let version = params
                .get("version")
                .and_then(Value::as_i64)
                .and_then(|version| i32::try_from(version).ok());
            let diagnostics = params
                .get("diagnostics")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(parse_diagnostic)
                .collect();
            emit(
                events,
                context,
                current_generation,
                TinymistEvent::PublishDiagnostics {
                    generation: session.generation,
                    uri: uri.to_owned(),
                    version,
                    diagnostics,
                    raw: params,
                },
            );
        }
        "window/logMessage" | "window/showMessage" | "$/logTrace" => {
            let message = params
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned();
            let level = params
                .get("type")
                .and_then(Value::as_u64)
                .and_then(|level| u32::try_from(level).ok());
            emit(
                events,
                context,
                current_generation,
                TinymistEvent::Log {
                    generation: session.generation,
                    level,
                    message,
                },
            );
        }
        _ => emit(
            events,
            context,
            current_generation,
            TinymistEvent::Notification {
                generation: session.generation,
                method: method.to_owned(),
                params,
            },
        ),
    }
}

fn parse_diagnostic(raw: &Value) -> Option<TinymistDiagnostic> {
    let range = serde_json::from_value(raw.get("range")?.clone()).ok()?;
    let message = raw.get("message")?.as_str()?.to_owned();
    let severity = raw
        .get("severity")
        .and_then(Value::as_u64)
        .map(DiagnosticSeverity::from_lsp);
    let code = raw.get("code").filter(|code| !code.is_null()).cloned();
    let source = raw
        .get("source")
        .and_then(Value::as_str)
        .map(ToOwned::to_owned);
    Some(TinymistDiagnostic {
        range,
        severity,
        code,
        source,
        message,
        raw: raw.clone(),
    })
}

fn emit(
    events: &Sender<TinymistEvent>,
    context: &egui::Context,
    current_generation: &AtomicU64,
    event: TinymistEvent,
) {
    if current_generation.load(Ordering::Acquire) != event.generation().0 {
        return;
    }
    if events.send(event).is_ok() {
        context.request_repaint();
    }
}

fn finish_session(
    mut session: Session,
    graceful: bool,
    reason: String,
    events: &Sender<TinymistEvent>,
    context: &egui::Context,
    current_generation: &AtomicU64,
) {
    if graceful && session.phase != SessionPhase::Initializing {
        let shutdown_id = session.next_request_id;
        session.next_request_id = session.next_request_id.saturating_add(1);
        let shutdown_sent = session
            .write(&json!({
                "jsonrpc": "2.0",
                "id": shutdown_id,
                "method": "shutdown",
                "params": null,
            }))
            .is_ok();
        if shutdown_sent {
            let deadline = Instant::now() + GRACEFUL_SHUTDOWN_TIMEOUT;
            let mut acknowledged = false;
            while Instant::now() < deadline {
                let remaining = deadline.saturating_duration_since(Instant::now());
                match session
                    .incoming
                    .recv_timeout(remaining.min(Duration::from_millis(20)))
                {
                    Ok(Incoming::Message(message)) => {
                        if message.get("id").and_then(Value::as_u64) == Some(shutdown_id)
                            && message.get("method").is_none()
                        {
                            acknowledged = true;
                            break;
                        }
                        // Answer server requests while it is winding down so
                        // shutdown cannot deadlock behind one of them.
                        if message.get("method").is_some() && message.get("id").is_some() {
                            let _ = handle_rpc_message(
                                &mut session,
                                message,
                                events,
                                context,
                                current_generation,
                            );
                        }
                    }
                    Ok(Incoming::Stderr(message)) => emit(
                        events,
                        context,
                        current_generation,
                        TinymistEvent::Log {
                            generation: session.generation,
                            level: None,
                            message,
                        },
                    ),
                    Ok(Incoming::EndOfStream | Incoming::ReadError(_))
                    | Err(RecvTimeoutError::Disconnected) => break,
                    Err(RecvTimeoutError::Timeout) => {}
                }
            }
            if acknowledged {
                let _ = session.notify("exit", Value::Null);
            }
        }
    }

    // Closing stdin also releases test/failure servers that wait for EOF.
    session.stdin.take();
    let status = reap_child(&session.child);
    join_readers(&mut session);
    session.mark_cleaned_up();
    emit(
        events,
        context,
        current_generation,
        TinymistEvent::Stopped {
            generation: session.generation,
            code: status.and_then(|status| status.code()),
            reason,
        },
    );
}

fn finish_exited_session(
    mut session: Session,
    status: ExitStatus,
    reason: String,
    events: &Sender<TinymistEvent>,
    context: &egui::Context,
    current_generation: &AtomicU64,
) {
    session.stdin.take();
    let _ = session
        .child
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .wait();
    join_readers(&mut session);
    session.mark_cleaned_up();
    emit(
        events,
        context,
        current_generation,
        TinymistEvent::Stopped {
            generation: session.generation,
            code: status.code(),
            reason,
        },
    );
}

fn try_wait_child(child: &SharedChild) -> io::Result<Option<ExitStatus>> {
    child
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .try_wait()
}

fn reap_child(child: &SharedChild) -> Option<ExitStatus> {
    let deadline = Instant::now() + EXIT_TIMEOUT;
    loop {
        match try_wait_child(child) {
            Ok(Some(status)) => return Some(status),
            Ok(None) if Instant::now() < deadline => thread::sleep(Duration::from_millis(10)),
            Ok(None) => {
                let mut child = child.lock().unwrap_or_else(|error| error.into_inner());
                let _ = child.kill();
                return child.wait().ok();
            }
            Err(_) => {
                let mut child = child.lock().unwrap_or_else(|error| error.into_inner());
                let _ = child.kill();
                return child.wait().ok();
            }
        }
    }
}

fn join_readers(session: &mut Session) {
    // A server wrapper can exit while a descendant keeps inherited stdout or
    // stderr open. Give ordinary readers time to drain all remaining logs, but
    // share one deadline and detach any thread still blocked in `Read` so a
    // stop/restart cannot wedge the protocol worker indefinitely.
    let deadline = Instant::now() + PIPE_READER_SHUTDOWN_TIMEOUT;
    if let Some(reader) = session.stdout_reader.take() {
        let _ = finish_reader_before(reader, deadline);
    }
    if let Some(reader) = session.stderr_reader.take() {
        let _ = finish_reader_before(reader, deadline);
    }
}

fn finish_reader_before<T>(reader: thread::JoinHandle<T>, deadline: Instant) -> Option<T> {
    while !reader.is_finished() && Instant::now() < deadline {
        thread::sleep(Duration::from_millis(5));
    }
    reader.is_finished().then(|| reader.join().ok()).flatten()
}

fn write_lsp_message(writer: &mut impl Write, message: &impl Serialize) -> io::Result<()> {
    let payload = serde_json::to_vec(message)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
    if payload.len() > MAX_MESSAGE_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "JSON-RPC message is too large",
        ));
    }
    write!(writer, "Content-Length: {}\r\n\r\n", payload.len())?;
    writer.write_all(&payload)?;
    writer.flush()
}

fn read_lsp_message(reader: &mut impl BufRead) -> io::Result<Option<Value>> {
    let mut content_length = None;
    let mut total_header_bytes = 0usize;
    let mut saw_header = false;

    loop {
        let mut line = Vec::new();
        let read = reader.read_until(b'\n', &mut line)?;
        if read == 0 {
            if !saw_header {
                return Ok(None);
            }
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "EOF in JSON-RPC headers",
            ));
        }
        saw_header = true;
        total_header_bytes = total_header_bytes.saturating_add(read);
        if line.len() > MAX_HEADER_LINE_BYTES || total_header_bytes > MAX_HEADER_BYTES {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "JSON-RPC headers are too large",
            ));
        }

        while matches!(line.last(), Some(b'\n' | b'\r')) {
            line.pop();
        }
        if line.is_empty() {
            break;
        }
        let header = std::str::from_utf8(&line)
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
        let (name, value) = header.split_once(':').ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidData, "malformed JSON-RPC header")
        })?;
        if name.trim().eq_ignore_ascii_case("content-length") {
            if content_length.is_some() {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "duplicate Content-Length header",
                ));
            }
            let length = value.trim().parse::<usize>().map_err(|_| {
                io::Error::new(io::ErrorKind::InvalidData, "invalid Content-Length header")
            })?;
            if length == 0 || length > MAX_MESSAGE_BYTES {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "JSON-RPC payload length is outside the accepted range",
                ));
            }
            content_length = Some(length);
        }
    }

    let content_length = content_length.ok_or_else(|| {
        io::Error::new(io::ErrorKind::InvalidData, "missing Content-Length header")
    })?;
    let mut payload = vec![0; content_length];
    reader.read_exact(&mut payload)?;
    serde_json::from_slice(&payload)
        .map(Some)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))
}

#[cfg(test)]
mod tests {
    use std::{
        fs,
        io::Cursor,
        sync::{Arc, atomic::AtomicU64, mpsc},
    };

    use super::*;

    const FAKE_SERVER_TIMEOUT: Duration = Duration::from_secs(10);

    fn frame(value: &Value) -> Vec<u8> {
        let mut bytes = Vec::new();
        write_lsp_message(&mut bytes, value).unwrap();
        bytes
    }

    #[test]
    fn lsp_framing_round_trips_multiple_messages() {
        let first = json!({"jsonrpc":"2.0", "id":1, "result":{"hello":"λ"}});
        let second = json!({"jsonrpc":"2.0", "method":"initialized", "params":{}});
        let mut bytes = frame(&first);
        bytes.extend(frame(&second));
        let mut reader = Cursor::new(bytes);

        assert_eq!(read_lsp_message(&mut reader).unwrap(), Some(first));
        assert_eq!(read_lsp_message(&mut reader).unwrap(), Some(second));
        assert_eq!(read_lsp_message(&mut reader).unwrap(), None);
    }

    #[test]
    fn request_deadline_policy_keeps_a_long_formatting_budget() {
        let formatting = PendingRequest::FormatDocument {
            uri: "file:///tmp/main.typ".to_owned(),
            version: 1,
        };
        let hover = PendingRequest::HoverDocument {
            uri: "file:///tmp/main.typ".to_owned(),
            version: 1,
            request_token: 1,
        };

        assert_eq!(
            PendingRequest::Initialize.timeout(),
            Duration::from_secs(15)
        );
        assert_eq!(
            PendingRequest::StartPreview.timeout(),
            Duration::from_secs(30)
        );
        assert_eq!(hover.timeout(), Duration::from_secs(10));
        assert_eq!(formatting.timeout(), Duration::from_secs(120));
        assert!(formatting.timeout() > PendingRequest::StartPreview.timeout());
    }

    #[test]
    fn workspace_font_paths_are_forwarded_as_one_platform_native_argument() {
        let paths = vec![
            PathBuf::from("/workspace/fonts"),
            PathBuf::from("/workspace/assets/type"),
        ];
        let config = TinymistConfig::new("/workspace").with_font_paths(&paths);
        assert_eq!(config.arguments[0], OsString::from("lsp"));
        assert_eq!(config.arguments[1], OsString::from("--font-path"));
        assert_eq!(config.arguments[2], std::env::join_paths(paths).unwrap());
    }

    #[test]
    fn explicit_entry_path_pins_both_lsp_compilation_and_web_preview() {
        let entry = PathBuf::from("/workspace/main.typ");
        let settings = TinymistConfig::new("/workspace")
            .with_entry_path(&entry)
            .server_settings();
        assert_eq!(settings["typstExtraArgs"], json!([entry]));
        assert_eq!(
            settings["preview"]["browsing"]["args"][0],
            json!("/workspace/main.typ")
        );
    }

    #[test]
    fn framing_accepts_case_insensitive_headers_and_lf() {
        let payload = br#"{"jsonrpc":"2.0","id":1,"result":null}"#;
        let bytes = format!(
            "content-type: application/vscode-jsonrpc; charset=utf-8\ncontent-length: {}\n\n",
            payload.len()
        )
        .into_bytes()
        .into_iter()
        .chain(payload.iter().copied())
        .collect::<Vec<_>>();

        let message = read_lsp_message(&mut Cursor::new(bytes)).unwrap().unwrap();
        assert_eq!(message["id"], 1);
    }

    #[test]
    fn framing_rejects_missing_duplicate_and_truncated_lengths() {
        for bytes in [
            b"Content-Type: application/json\r\n\r\n{}".as_slice(),
            b"Content-Length: 2\r\nContent-Length: 2\r\n\r\n{}".as_slice(),
            b"Content-Length: 10\r\n\r\n{}".as_slice(),
        ] {
            assert!(read_lsp_message(&mut Cursor::new(bytes)).is_err());
        }
    }

    #[test]
    fn preview_settings_are_safe_and_interactive() {
        let settings = PreviewOptions::default().settings(None);
        let args = settings["preview"]["browsing"]["args"]
            .as_array()
            .unwrap()
            .iter()
            .map(Value::as_str)
            .collect::<Option<Vec<_>>>()
            .unwrap();
        assert!(args.contains(&"--data-plane-host=127.0.0.1:0"));
        assert!(args.contains(&"--control-plane-host=127.0.0.1:0"));
        assert!(args.contains(&"--preview-mode=document"));
        assert!(args.contains(&"--invert-colors=auto"));
        assert!(args.contains(&"--partial-rendering=false"));
        assert!(args.contains(&"--no-open"));
        assert_eq!(settings["preview"]["refresh"], "onType");
        assert_eq!(settings["customizedShowDocument"], false);
    }

    #[test]
    fn paused_preview_refresh_policy_is_serialized_without_disabling_lsp() {
        let options = PreviewOptions {
            refresh: PreviewRefresh::OnSave,
            ..PreviewOptions::default()
        };
        let settings = options.settings(None);

        assert_eq!(settings["preview"]["refresh"], "onSave");
        assert_eq!(settings["customizedShowDocument"], false);
    }

    #[test]
    fn hover_request_uses_standard_utf16_lsp_position() {
        assert_eq!(
            hover_document_params(
                "file:///project/main.typ",
                LspPosition {
                    line: 3,
                    character: 7,
                },
            ),
            json!({
                "textDocument": { "uri": "file:///project/main.typ" },
                "position": { "line": 3, "character": 7 },
            })
        );
    }

    #[test]
    fn hover_results_accept_markup_marked_strings_arrays_and_null() {
        let markdown = json!({
            "contents": { "kind": "markdown", "value": "`text(body)`\n\nAdds content." },
            "range": {
                "start": { "line": 1, "character": 2 },
                "end": { "line": 1, "character": 6 }
            }
        });
        let (contents, range) = parse_hover_result(&markdown);
        assert_eq!(contents.as_deref(), Some("`text(body)`\n\nAdds content."));
        assert_eq!(range.unwrap().start.line, 1);

        let marked = json!({
            "contents": [
                { "language": "typst", "value": "#let value = 1" },
                "A definition"
            ]
        });
        assert_eq!(
            parse_hover_result(&marked).0.as_deref(),
            Some("```typst\n#let value = 1\n```\n\nA definition")
        );
        assert_eq!(parse_hover_result(&Value::Null), (None, None));
        assert_eq!(parse_hover_result(&json!({ "contents": [] })), (None, None));
    }

    #[test]
    fn completion_request_uses_standard_utf16_lsp_position() {
        assert_eq!(
            completion_document_params(
                "file:///project/main.typ",
                LspPosition {
                    line: 4,
                    character: 11,
                },
            ),
            json!({
                "textDocument": { "uri": "file:///project/main.typ" },
                "position": { "line": 4, "character": 11 },
                "context": { "triggerKind": 1 },
            })
        );
    }

    #[test]
    fn completion_results_accept_lists_and_completion_list_defaults() {
        let (incomplete, plain) = parse_completion_result(&json!([
            { "label": "text", "detail": "function" },
            { "label": "table", "insertText": "table()", "sortText": "01" }
        ]))
        .unwrap();
        assert!(!incomplete);
        assert_eq!(plain.len(), 2);
        assert_eq!(plain[0].insert_text, "text");
        assert_eq!(plain[1].insert_text, "table()");

        let (incomplete, listed) = parse_completion_result(&json!({
            "isIncomplete": true,
            "itemDefaults": {
                "editRange": {
                    "insert": {
                        "start": { "line": 2, "character": 3 },
                        "end": { "line": 2, "character": 5 }
                    },
                    "replace": {
                        "start": { "line": 2, "character": 3 },
                        "end": { "line": 2, "character": 8 }
                    }
                },
                "insertTextFormat": 2
            },
            "items": [{
                "label": "heading",
                "textEditText": "heading(${1:body})$0",
                "documentation": { "kind": "markdown", "value": "A heading." }
            }]
        }))
        .unwrap();
        assert!(incomplete);
        assert_eq!(listed.len(), 1);
        assert!(listed[0].insert_text_is_snippet);
        assert_eq!(listed[0].documentation.as_deref(), Some("A heading."));
        assert_eq!(listed[0].text_edit.as_ref().unwrap().range.end.character, 8);
    }

    #[test]
    fn completion_results_preserve_text_and_additional_edits() {
        let (_, items) = parse_completion_result(&json!({
            "items": [{
                "label": "accent",
                "filterText": "acc",
                "insertTextFormat": 2,
                "textEdit": {
                    "newText": "accent($1)$0",
                    "replace": {
                        "start": { "line": 1, "character": 2 },
                        "end": { "line": 1, "character": 6 }
                    }
                },
                "additionalTextEdits": [{
                    "range": {
                        "start": { "line": 0, "character": 0 },
                        "end": { "line": 0, "character": 0 }
                    },
                    "newText": "#import \"helpers.typ\": accent\n"
                }]
            }]
        }))
        .unwrap();
        assert_eq!(items[0].insert_text, "accent");
        assert_eq!(
            items[0].text_edit.as_ref().unwrap().new_text,
            "accent($1)$0"
        );
        assert_eq!(items[0].additional_text_edits.len(), 1);
        assert!(parse_completion_result(&json!({ "items": [42] })).is_err());
        assert!(parse_completion_result(&json!({ "items": "wrong" })).is_err());
    }

    #[test]
    fn unsaved_document_has_real_private_backing_for_formatting() {
        let project = tempfile::tempdir().unwrap();
        fs::write(project.path().join("shared.typ"), "#let shared = 1").unwrap();
        let private_path = {
            let unsaved = UnsavedTextDocument::create(
                project.path(),
                project.path(),
                "Untitled.typ",
                "#include \"shared.typ\"",
            )
            .unwrap();
            assert!(unsaved.path().is_file());
            assert!(
                unsaved
                    .path()
                    .starts_with(project.path().canonicalize().unwrap().join(".tiptoptyp"))
            );
            assert_eq!(
                fs::read_to_string(unsaved.path().parent().unwrap().join("shared.typ")).unwrap(),
                "#let shared = 1"
            );
            let document = unsaved.text_document(7, "changed in memory");
            assert_eq!(document.uri, unsaved.uri());
            assert_eq!(document.version, 7);
            assert_eq!(document.text, "changed in memory");
            unsaved.update_backing_source("changed on disk").unwrap();
            assert_eq!(
                fs::read_to_string(unsaved.path()).unwrap(),
                "changed on disk"
            );
            unsaved.path().to_owned()
        };
        assert!(!private_path.exists());
    }

    #[test]
    #[ignore = "requires a real Tinymist executable; set TIPTOPTYP_TEST_TINYMIST to override discovery"]
    fn real_tinymist_formats_private_backing_for_unsaved_document() {
        use crate::{
            settings::ToolPreference,
            toolchain::{ToolKind, resolve_tool},
        };

        let program = std::env::var_os("TIPTOPTYP_TEST_TINYMIST")
            .map(PathBuf::from)
            .or_else(|| {
                let resolved = resolve_tool(ToolKind::Tinymist, &ToolPreference::default());
                resolved.is_available().then_some(resolved.program)
            });
        let Some(program) = program else {
            eprintln!("skipping real Tinymist formatting test: no executable is available");
            return;
        };

        let project = tempfile::tempdir().unwrap();
        let source = "#let answer=40+2\n#answer\n";
        let unsaved =
            UnsavedTextDocument::create(project.path(), project.path(), "Untitled.typ", source)
                .unwrap();
        let backing_path = unsaved.path().to_owned();
        assert!(backing_path.is_file());

        let sidecar = TinymistSidecar::new(egui::Context::default());
        let mut config = TinymistConfig::new(project.path()).with_executable(program.clone());
        config.start_preview = false;
        let generation = sidecar.start_workspace(config).unwrap();
        let version = 1;
        let document = unsaved.text_document(version, source);
        let uri = document.uri.clone();
        sidecar.did_open(generation, document).unwrap();

        let initialize_deadline = Instant::now() + Duration::from_secs(15);
        let mut initialized = false;
        while Instant::now() < initialize_deadline && !initialized {
            while let Some(event) = sidecar.try_recv() {
                match event {
                    TinymistEvent::Initialized {
                        generation: event_generation,
                    } if event_generation == generation => initialized = true,
                    TinymistEvent::Error {
                        message,
                        fatal: true,
                        ..
                    } => panic!(
                        "real Tinymist {} failed before formatting: {message}",
                        program.display()
                    ),
                    TinymistEvent::Stopped { reason, .. } => panic!(
                        "real Tinymist {} stopped before formatting: {reason}",
                        program.display()
                    ),
                    _ => {}
                }
            }
            thread::sleep(Duration::from_millis(10));
        }
        assert!(
            initialized,
            "real Tinymist {} did not initialize",
            program.display()
        );

        unsaved.update_backing_source(source).unwrap();
        sidecar
            .format_document(generation, uri.clone(), version)
            .unwrap();
        let format_deadline = Instant::now() + Duration::from_secs(15);
        let edits = 'wait_for_format: loop {
            while let Some(event) = sidecar.try_recv() {
                match event {
                    TinymistEvent::Formatted {
                        generation: event_generation,
                        uri: event_uri,
                        version: event_version,
                        edits,
                    } if event_generation == generation
                        && event_uri == uri
                        && event_version == version =>
                    {
                        break 'wait_for_format edits;
                    }
                    TinymistEvent::Error {
                        stage: "formatting",
                        message,
                        ..
                    } => panic!("real Tinymist formatting failed: {message}"),
                    TinymistEvent::Error {
                        message,
                        fatal: true,
                        ..
                    } => panic!("real Tinymist failed while formatting: {message}"),
                    TinymistEvent::Stopped { reason, .. } => {
                        panic!("real Tinymist stopped while formatting: {reason}")
                    }
                    _ => {}
                }
            }
            assert!(
                Instant::now() < format_deadline,
                "real Tinymist did not return a formatting result for the private unsaved file"
            );
            thread::sleep(Duration::from_millis(10));
        };
        assert!(
            edits.is_some_and(|edits| !edits.is_empty()),
            "real Tinymist returned no edits for deliberately unformatted source"
        );

        sidecar.did_close(generation, uri).unwrap();
        sidecar.stop_workspace(generation).unwrap();
        let stop_deadline = Instant::now() + Duration::from_secs(10);
        let mut stopped = false;
        while Instant::now() < stop_deadline && !stopped {
            while let Some(event) = sidecar.try_recv() {
                if matches!(
                    event,
                    TinymistEvent::Stopped {
                        generation: event_generation,
                        ..
                    } if event_generation == generation
                ) {
                    stopped = true;
                }
            }
            thread::sleep(Duration::from_millis(10));
        }
        assert!(stopped, "real Tinymist did not stop cleanly");
        assert!(backing_path.exists(), "backing vanished before its guard");
        drop(unsaved);
        assert!(!backing_path.exists(), "private backing was not cleaned up");
    }

    #[test]
    fn preview_scroll_uses_tinymists_default_task_and_source_location_schema() {
        let params = scroll_preview_params(Path::new("/tmp/chapter.typ"), 7, 11);
        assert_eq!(params["command"], "tinymist.scrollPreview");
        assert_eq!(params["arguments"][0], DEFAULT_PREVIEW_TASK_ID);
        assert_eq!(params["arguments"][1]["event"], "panelScrollTo");
        assert_eq!(params["arguments"][1]["filepath"], "/tmp/chapter.typ");
        assert_eq!(params["arguments"][1]["line"], 7);
        assert_eq!(params["arguments"][1]["character"], 11);
    }

    #[test]
    fn formatting_request_uses_standard_lsp_params() {
        let params = format_document_params("file:///tmp/chapter.typ");
        assert_eq!(
            params,
            json!({
                "textDocument": { "uri": "file:///tmp/chapter.typ" },
                "options": {
                    "tabSize": 2,
                    "insertSpaces": true,
                },
            })
        );
    }

    #[test]
    fn formatting_results_preserve_null_empty_and_utf16_edits() {
        assert_eq!(parse_format_document_result(Value::Null).unwrap(), None);
        assert_eq!(
            parse_format_document_result(json!([])).unwrap(),
            Some(Vec::new())
        );

        let edits = parse_format_document_result(json!([{
            "range": {
                "start": { "line": 2, "character": 4 },
                "end": { "line": 2, "character": 6 }
            },
            "newText": "🦀 formatted"
        }]))
        .unwrap()
        .unwrap();
        assert_eq!(
            edits,
            vec![LspTextEdit {
                range: LspRange {
                    start: LspPosition {
                        line: 2,
                        character: 4,
                    },
                    end: LspPosition {
                        line: 2,
                        character: 6,
                    },
                },
                new_text: "🦀 formatted".to_owned(),
            }]
        );
        assert!(parse_format_document_result(json!({ "edits": [] })).is_err());
    }

    #[test]
    fn public_formatting_api_routes_uri_and_version_to_the_worker() {
        let (commands, received_commands) = mpsc::channel();
        let (_events, received_events) = mpsc::channel();
        let sidecar = TinymistSidecar {
            commands: Some(commands),
            events: received_events,
            worker: None,
            next_generation: AtomicU64::new(8),
            current_generation: Arc::new(AtomicU64::new(7)),
            process_supervisor: Arc::new(ProcessSupervisor::default()),
            worker_done: None,
        };

        sidecar
            .format_document(Generation(7), "file:///tmp/chapter.typ", 19)
            .unwrap();
        match received_commands.recv().unwrap() {
            WorkerCommand::FormatDocument {
                generation,
                uri,
                version,
            } => {
                assert_eq!(generation, Generation(7));
                assert_eq!(uri, "file:///tmp/chapter.typ");
                assert_eq!(version, 19);
            }
            _ => panic!("format_document queued the wrong worker command"),
        }
    }

    #[test]
    fn configuration_requests_resolve_tinymist_sections() {
        let settings = PreviewOptions::default().settings(None);
        let params = json!({
            "items": [
                {"section":"tinymist"},
                {"section":"tinymist.preview"},
                {"section":"tinymist.preview.browsing"},
                {"section":"tinymist.doesNotExist"},
            ]
        });
        let response = configuration_response(&params, &settings);
        assert_eq!(response[0], settings);
        assert_eq!(response[1], settings["preview"]);
        assert_eq!(response[2], settings["preview"]["browsing"]);
        assert!(response[3].is_null());
    }

    #[test]
    fn diagnostics_keep_typed_fields_and_raw_extensions() {
        let raw = json!({
            "range": {
                "start": {"line": 3, "character": 4},
                "end": {"line": 3, "character": 9}
            },
            "severity": 1,
            "code": "unknown-variable",
            "source": "typst",
            "message": "unknown variable: name",
            "data": {"future": true}
        });
        let diagnostic = parse_diagnostic(&raw).unwrap();
        assert_eq!(diagnostic.range.start.line, 3);
        assert_eq!(diagnostic.severity, Some(DiagnosticSeverity::Error));
        assert_eq!(diagnostic.code, Some(json!("unknown-variable")));
        assert_eq!(diagnostic.raw["data"]["future"], true);
    }

    #[test]
    fn prospective_unicode_paths_become_file_uris_without_existing() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("not created").join("λ.typ");
        let document = TextDocument::from_path(&path, 4, "Hello").unwrap();
        let round_trip = Url::parse(&document.uri).unwrap().to_file_path().unwrap();
        assert_eq!(round_trip, path);
        assert_eq!(document.language_id, "typst");
        assert_eq!(document.version, 4);
    }

    #[test]
    fn stale_events_are_not_delivered_or_repainted() {
        let (tx, rx) = mpsc::channel();
        let generation = Arc::new(AtomicU64::new(2));
        let context = egui::Context::default();
        emit(
            &tx,
            &context,
            &generation,
            TinymistEvent::Starting {
                generation: Generation(1),
            },
        );
        assert!(rx.try_recv().is_err());
        emit(
            &tx,
            &context,
            &generation,
            TinymistEvent::Starting {
                generation: Generation(2),
            },
        );
        assert_eq!(
            rx.try_recv().unwrap(),
            TinymistEvent::Starting {
                generation: Generation(2)
            }
        );
    }

    #[cfg(unix)]
    #[test]
    fn initialization_write_failure_reaps_child_and_readers() {
        let directory = tempfile::tempdir().unwrap();
        let mut pid = None;
        let mut config = TinymistConfig::new(directory.path())
            .with_command("/bin/sleep", [OsString::from("30")]);
        config.start_preview = false;
        let result = Session::spawn_with_initializer(
            Generation(1),
            config,
            Arc::new(ProcessSupervisor::default()),
            |session| {
                pid = Some(
                    session
                        .child
                        .lock()
                        .unwrap_or_else(|error| error.into_inner())
                        .id(),
                );
                session.stdin.take();
                session.send_initialize()
            },
        );
        let error = match result {
            Ok(_) => panic!("injected closed stdin must fail initialization"),
            Err(error) => error,
        };
        assert!(error.contains("stdin is closed"), "{error}");

        let pid = pid.expect("spawned session did not expose its child pid");
        let still_running = Command::new("kill")
            .args(["-0", &pid.to_string()])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .is_ok_and(|status| status.success());
        assert!(!still_running, "startup failure leaked child process {pid}");
    }

    #[cfg(unix)]
    #[test]
    fn dropping_sidecar_terminates_a_server_blocked_on_stdin() {
        use std::os::unix::fs::PermissionsExt;

        let directory = tempfile::tempdir().unwrap();
        let script = directory.path().join("blocked-stdin-tinymist.sh");
        let pid_path = directory.path().join("server.pid");
        let hold_path = directory.path().join("hold.fifo");
        let initialize = json!({"jsonrpc":"2.0","id":1,"result":{"capabilities":{}}});
        let payload = serde_json::to_string(&initialize).unwrap();
        fs::write(
            &script,
            format!(
                "#!/bin/sh\nprintf '%s' \"$$\" > '{}'\nprintf '%s\\r\\n\\r\\n%s' 'Content-Length: {}' '{}'\nmkfifo '{}'\nexec cat '{}'\n",
                pid_path.display(),
                payload.len(),
                payload,
                hold_path.display(),
                hold_path.display(),
            ),
        )
        .unwrap();
        let mut permissions = fs::metadata(&script).unwrap().permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&script, permissions).unwrap();

        let sidecar = TinymistSidecar::new(egui::Context::default());
        let supervisor = Arc::clone(&sidecar.process_supervisor);
        let mut config = TinymistConfig::new(directory.path())
            .with_command(&script, std::iter::empty::<OsString>());
        config.start_preview = false;
        let generation = sidecar.start_workspace(config).unwrap();

        let initialize_deadline = Instant::now() + FAKE_SERVER_TIMEOUT;
        let mut initialized = false;
        while Instant::now() < initialize_deadline && !initialized {
            while let Some(event) = sidecar.try_recv() {
                initialized |= matches!(event, TinymistEvent::Initialized { .. });
            }
            thread::sleep(Duration::from_millis(5));
        }
        assert!(initialized, "fake server never initialized");
        assert!(supervisor.has_active_process());
        assert!(pid_path.is_file(), "fake server did not record its pid");

        // Larger than ordinary pipe capacity: because the fake server never
        // reads stdin, the worker cannot finish this didOpen before shutdown.
        sidecar
            .did_open(
                generation,
                TextDocument::typst("file:///tmp/blocked.typ", 1, "x".repeat(8 * 1024 * 1024)),
            )
            .unwrap();
        let started = Instant::now();
        drop(sidecar);
        let elapsed = started.elapsed();
        assert!(
            elapsed
                < WORKER_SHUTDOWN_TIMEOUT
                    + FORCED_WORKER_SHUTDOWN_TIMEOUT
                    + Duration::from_millis(500),
            "sidecar drop exceeded its termination bound: {elapsed:?}"
        );
        assert!(!supervisor.has_active_process());

        let pid = fs::read_to_string(&pid_path).unwrap();
        let still_running = Command::new("kill")
            .args(["-0", pid.trim()])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .is_ok_and(|status| status.success());
        assert!(!still_running, "blocked fake server survived drop: {pid}");
    }

    #[cfg(unix)]
    #[test]
    fn exited_sidecar_does_not_wait_for_a_descendant_holding_output_pipes() {
        use std::os::unix::fs::PermissionsExt;

        let directory = tempfile::tempdir().unwrap();
        let script = directory.path().join("inherited-pipes-tinymist.sh");
        let pid_path = directory.path().join("descendant.pid");
        let initialize = json!({"jsonrpc":"2.0","id":1,"result":{"capabilities":{}}});
        let payload = serde_json::to_string(&initialize).unwrap();
        fs::write(
            &script,
            format!(
                concat!(
                    "#!/bin/sh\n",
                    "sleep 30 &\n",
                    "printf '%s' \"$!\" > '{}'\n",
                    "printf '%s\\n' 'inherited pipe fixture log' >&2\n",
                    "printf '%s\\r\\n\\r\\n%s' 'Content-Length: {}' '{}'\n",
                    "sleep 0.1\n",
                ),
                pid_path.display(),
                payload.len(),
                payload,
            ),
        )
        .unwrap();
        let mut permissions = fs::metadata(&script).unwrap().permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&script, permissions).unwrap();

        let sidecar = TinymistSidecar::new(egui::Context::default());
        let mut config = TinymistConfig::new(directory.path())
            .with_command(&script, std::iter::empty::<OsString>());
        config.start_preview = false;
        let generation = sidecar.start_workspace(config).unwrap();
        // The repository test suite runs many process-heavy fixtures in
        // parallel. Wait until this wrapper has actually started its pipe-
        // holding descendant before measuring cleanup, otherwise scheduler
        // delay is indistinguishable from a blocked reader and the PID file
        // itself can race this assertion.
        let fixture_deadline = Instant::now() + FAKE_SERVER_TIMEOUT;
        let pid = loop {
            if let Ok(pid) = fs::read_to_string(&pid_path)
                && !pid.trim().is_empty()
            {
                break pid;
            }
            assert!(
                Instant::now() < fixture_deadline,
                "Tinymist inherited-pipe fixture never started"
            );
            thread::sleep(Duration::from_millis(5));
        };
        let cleanup_started = Instant::now();
        let deadline = cleanup_started + Duration::from_secs(4);
        let mut log_seen = false;
        let mut stopped = false;
        while Instant::now() < deadline && !stopped {
            while let Some(event) = sidecar.try_recv() {
                match event {
                    TinymistEvent::Log { message, .. }
                        if message.contains("inherited pipe fixture log") =>
                    {
                        log_seen = true;
                    }
                    TinymistEvent::Stopped {
                        generation: event_generation,
                        ..
                    } if event_generation == generation => stopped = true,
                    _ => {}
                }
            }
            thread::sleep(Duration::from_millis(5));
        }
        let elapsed = cleanup_started.elapsed();

        let kill_status = Command::new("kill")
            .args(["-9", pid.trim()])
            .status()
            .unwrap();
        assert!(
            kill_status.success(),
            "could not terminate fixture pid {pid}"
        );
        assert!(log_seen, "stderr emitted before exit was not delivered");
        assert!(
            stopped,
            "session waited for descendant-owned output pipes for {elapsed:?}"
        );
        assert!(
            elapsed < Duration::from_secs(4),
            "session cleanup exceeded its reader bound: {elapsed:?}"
        );

        // A blocked cleanup must not consume the worker permanently: the next
        // workspace still reaches its normal start transition.
        let replacement = sidecar
            .start_workspace(
                TinymistConfig::new(directory.path())
                    .with_command("/bin/sleep", [OsString::from("30")]),
            )
            .unwrap();
        let replacement_deadline = Instant::now() + Duration::from_secs(2);
        let mut replacement_started = false;
        while Instant::now() < replacement_deadline && !replacement_started {
            while let Some(event) = sidecar.try_recv() {
                replacement_started |= matches!(
                    event,
                    TinymistEvent::Starting {
                        generation: event_generation
                    } if event_generation == replacement
                );
            }
            thread::sleep(Duration::from_millis(5));
        }
        assert!(
            replacement_started,
            "Tinymist worker did not accept a replacement"
        );
    }

    #[cfg(unix)]
    #[test]
    fn unanswered_initialize_request_times_out_and_stops_the_session() {
        let directory = tempfile::tempdir().unwrap();
        let sidecar = TinymistSidecar::new(egui::Context::default());
        let supervisor = Arc::clone(&sidecar.process_supervisor);
        let mut config = TinymistConfig::new(directory.path())
            // `sleep` keeps every pipe open but never reads the initialize
            // request and never writes a response during the test.
            .with_command("/bin/sleep", [OsString::from("30")])
            .with_request_timeout(Duration::from_secs(3));
        config.start_preview = false;
        let generation = sidecar.start_workspace(config).unwrap();

        let deadline = Instant::now() + FAKE_SERVER_TIMEOUT;
        let mut timeout_error = None;
        let mut stopped = false;
        while Instant::now() < deadline && !stopped {
            while let Some(event) = sidecar.try_recv() {
                match event {
                    TinymistEvent::Error {
                        generation: event_generation,
                        stage: "timeout",
                        message,
                        fatal: true,
                    } => {
                        assert_eq!(event_generation, generation);
                        timeout_error = Some(message);
                    }
                    TinymistEvent::Stopped {
                        generation: event_generation,
                        reason,
                        ..
                    } => {
                        assert_eq!(event_generation, generation);
                        assert!(reason.contains("timeout"), "{reason}");
                        stopped = true;
                    }
                    _ => {}
                }
            }
            thread::sleep(Duration::from_millis(5));
        }

        let timeout_error = timeout_error.expect("missing request-timeout event");
        assert!(
            timeout_error.contains("initialize request 1"),
            "{timeout_error}"
        );
        assert!(
            timeout_error.contains("3s"),
            "timeout should report its bound: {timeout_error}"
        );
        assert!(stopped, "timed-out session did not stop");
        assert_eq!(sidecar.current_generation(), None);
        assert!(!supervisor.has_active_process());
    }

    #[cfg(unix)]
    #[test]
    fn preview_refresh_update_keeps_buffer_sync_and_formatting_live() {
        use std::os::unix::fs::PermissionsExt;

        let directory = tempfile::tempdir().unwrap();
        let script = directory.path().join("fake-lsp-only-tinymist.sh");
        let captured_input = directory.path().join("client-input.bin");
        let initialize = json!({"jsonrpc":"2.0","id":1,"result":{"capabilities":{}}});
        let payload = serde_json::to_string(&initialize).unwrap();
        let script_source = format!(
            "#!/bin/sh\nprintf '%s\\r\\n\\r\\n%s' 'Content-Length: {}' '{}'\nwhile IFS= read -r line || [ -n \"$line\" ]; do printf '%s\\n' \"$line\" >> '{}'; done\n",
            payload.len(),
            payload,
            captured_input.display()
        );
        fs::write(&script, script_source).unwrap();
        let mut permissions = fs::metadata(&script).unwrap().permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&script, permissions).unwrap();

        let sidecar = TinymistSidecar::new(egui::Context::default());
        let mut config = TinymistConfig::new(directory.path())
            .with_command(&script, std::iter::empty::<OsString>());
        config.start_preview = false;
        let generation = sidecar.start_workspace(config).unwrap();
        let uri = "file:///tmp/main.typ";
        sidecar
            .did_open(generation, TextDocument::typst(uri, 7, "Hello"))
            .unwrap();

        let initialize_deadline = Instant::now() + FAKE_SERVER_TIMEOUT;
        let mut initialized = false;
        while Instant::now() < initialize_deadline && !initialized {
            while let Some(event) = sidecar.try_recv() {
                match event {
                    TinymistEvent::Initialized { .. } => initialized = true,
                    TinymistEvent::PreviewReady { .. } => {
                        panic!("LSP-only session unexpectedly started a preview")
                    }
                    _ => {}
                }
            }
            thread::sleep(Duration::from_millis(10));
        }
        assert!(initialized, "LSP-only session never initialized");

        sidecar
            .set_preview_refresh(generation, PreviewRefresh::OnSave)
            .unwrap();

        // Resuming without edits must not send a duplicate LSP version or
        // degrade the session. Explicit formatting after paused edits must
        // synchronize its requested version first.
        sidecar.did_change(generation, uri, 7, "Hello").unwrap();
        sidecar
            .did_change(generation, uri, 8, "Edited while paused")
            .unwrap();
        sidecar
            .did_change(generation, uri, 8, "Edited while paused")
            .unwrap();
        sidecar
            .did_change(generation, uri, 8, "Conflicting version")
            .unwrap();
        sidecar
            .did_change(generation, uri, 7, "Edited while paused")
            .unwrap();
        sidecar.format_document(generation, uri, 8).unwrap();
        sidecar.stop_workspace(generation).unwrap();

        let stop_deadline = Instant::now() + FAKE_SERVER_TIMEOUT;
        let mut stopped = false;
        let mut rejected_changes = Vec::new();
        while Instant::now() < stop_deadline && !stopped {
            while let Some(event) = sidecar.try_recv() {
                match event {
                    TinymistEvent::Stopped { .. } => stopped = true,
                    TinymistEvent::PreviewReady { .. } => {
                        panic!("LSP-only session unexpectedly started a preview")
                    }
                    TinymistEvent::Error {
                        stage: "document",
                        message,
                        fatal: false,
                        ..
                    } => rejected_changes.push(message),
                    TinymistEvent::Error { stage, message, .. } => {
                        panic!("LSP-only session {stage} failed: {message}")
                    }
                    _ => {}
                }
            }
            thread::sleep(Duration::from_millis(10));
        }
        assert!(stopped, "LSP-only session never stopped");
        assert_eq!(
            rejected_changes,
            [
                format!("ignored non-monotonic version 8 for {uri}; current version is 8"),
                format!("ignored non-monotonic version 7 for {uri}; current version is 8"),
            ]
        );

        let captured = fs::read_to_string(captured_input).unwrap();
        assert!(captured.contains("textDocument/didOpen"), "{captured:?}");
        assert_eq!(captured.matches("textDocument/didChange").count(), 1);
        assert!(captured.contains("Edited while paused"), "{captured:?}");
        assert!(!captured.contains("Conflicting version"), "{captured:?}");
        assert!(captured.contains("textDocument/formatting"), "{captured:?}");
        assert!(
            captured.find("textDocument/didChange") < captured.find("textDocument/formatting"),
            "{captured:?}"
        );
        assert_eq!(
            captured.matches("workspace/didChangeConfiguration").count(),
            2,
            "the paused policy should update the existing LSP session: {captured:?}"
        );
        assert!(captured.contains("\"refresh\":\"onSave\""), "{captured:?}");
        assert!(!captured.contains("tinymist.startDefaultPreview"));
    }

    #[cfg(unix)]
    #[test]
    fn preinitialize_changes_flush_once_as_the_latest_did_open() {
        use std::os::unix::fs::PermissionsExt;

        let directory = tempfile::tempdir().unwrap();
        let script = directory.path().join("delayed-initialize-tinymist.sh");
        let captured_input = directory.path().join("client-input.bin");
        let initialize = json!({"jsonrpc":"2.0","id":1,"result":{"capabilities":{}}});
        let payload = serde_json::to_string(&initialize).unwrap();
        let script_source = format!(
            "#!/bin/sh\nsleep 0.2\nprintf '%s\\r\\n\\r\\n%s' 'Content-Length: {}' '{}'\nwhile IFS= read -r line || [ -n \"$line\" ]; do printf '%s\\n' \"$line\" >> '{}'; done\n",
            payload.len(),
            payload,
            captured_input.display()
        );
        fs::write(&script, script_source).unwrap();
        let mut permissions = fs::metadata(&script).unwrap().permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&script, permissions).unwrap();

        let sidecar = TinymistSidecar::new(egui::Context::default());
        let mut config = TinymistConfig::new(directory.path())
            .with_command(&script, std::iter::empty::<OsString>());
        config.start_preview = false;
        let generation = sidecar.start_workspace(config).unwrap();
        let uri = "file:///tmp/main.typ";
        let other_uri = "file:///tmp/other.typ";
        sidecar
            .did_open(generation, TextDocument::typst(uri, 7, "old"))
            .unwrap();
        sidecar.did_change(generation, uri, 8, "latest").unwrap();
        sidecar
            .did_open(generation, TextDocument::typst(other_uri, 3, "other"))
            .unwrap();

        let initialize_deadline = Instant::now() + FAKE_SERVER_TIMEOUT;
        let mut initialized = false;
        while Instant::now() < initialize_deadline && !initialized {
            while let Some(event) = sidecar.try_recv() {
                if matches!(event, TinymistEvent::Initialized { .. }) {
                    initialized = true;
                }
            }
            thread::sleep(Duration::from_millis(10));
        }
        assert!(initialized, "delayed fake server never initialized");
        sidecar.stop_workspace(generation).unwrap();

        let stop_deadline = Instant::now() + FAKE_SERVER_TIMEOUT;
        let mut stopped = false;
        while Instant::now() < stop_deadline {
            if matches!(sidecar.try_recv(), Some(TinymistEvent::Stopped { .. })) {
                stopped = true;
                break;
            }
            thread::sleep(Duration::from_millis(10));
        }
        assert!(stopped, "delayed fake server never stopped");
        let captured = fs::read_to_string(captured_input).unwrap();
        assert_eq!(
            captured
                .matches("\"method\":\"textDocument/didOpen\"")
                .count(),
            2,
            "{captured:?}"
        );
        assert!(!captured.contains("\"method\":\"textDocument/didChange\""));
        assert!(captured.contains("\"version\":8"), "{captured:?}");
        assert!(captured.contains("\"text\":\"latest\""), "{captured:?}");
        assert!(captured.contains(other_uri), "{captured:?}");
        assert!(captured.contains("\"text\":\"other\""), "{captured:?}");
    }

    #[cfg(unix)]
    #[test]
    fn fake_server_completes_preview_and_source_mapping_handshake() {
        use std::os::unix::fs::PermissionsExt;

        let directory = tempfile::tempdir().unwrap();
        let script = directory.path().join("fake-tinymist.sh");
        let captured_input = directory.path().join("client-input.bin");
        let messages = [
            json!({"jsonrpc":"2.0","id":1,"result":{"capabilities":{}}}),
            json!({"jsonrpc":"2.0","id":2,"result":{"staticServerPort":41723}}),
            json!({
                "jsonrpc":"2.0",
                "method":"textDocument/publishDiagnostics",
                "params":{
                    "uri":"file:///tmp/main.typ",
                    "version":7,
                    "diagnostics":[{
                        "range":{"start":{"line":1,"character":2},"end":{"line":1,"character":3}},
                        "severity":2,
                        "message":"fake warning"
                    }]
                }
            }),
            json!({
                "jsonrpc":"2.0",
                "id":99,
                "method":"window/showDocument",
                "params":{
                    "uri":"file:///tmp/main.typ",
                    "takeFocus":true,
                    "selection":{"start":{"line":4,"character":5},"end":{"line":4,"character":8}}
                }
            }),
        ];
        let mut script_source = String::from("#!/bin/sh\n");
        for message in messages {
            let payload = serde_json::to_string(&message).unwrap();
            script_source.push_str(&format!(
                "printf '%s\\r\\n\\r\\n%s' 'Content-Length: {}' '{}'\n",
                payload.len(),
                payload
            ));
        }
        script_source.push_str(&format!(
            "while IFS= read -r line || [ -n \"$line\" ]; do printf '%s\\n' \"$line\" >> '{}'; done\n",
            captured_input.display()
        ));
        fs::write(&script, script_source).unwrap();
        let mut permissions = fs::metadata(&script).unwrap().permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&script, permissions).unwrap();

        let sidecar = TinymistSidecar::new(egui::Context::default());
        let pinned_entry = directory.path().join("pinned-main.typ");
        let config = TinymistConfig::new(directory.path())
            .with_entry_path(&pinned_entry)
            .with_command(&script, std::iter::empty::<OsString>());
        let generation = sidecar.start_workspace(config).unwrap();
        sidecar
            .did_open(
                generation,
                TextDocument::typst("file:///tmp/main.typ", 7, "Hello"),
            )
            .unwrap();

        let deadline = Instant::now() + FAKE_SERVER_TIMEOUT;
        let mut preview_url = None;
        let mut diagnostic = None;
        let mut jump = None;
        while Instant::now() < deadline
            && (preview_url.is_none() || diagnostic.is_none() || jump.is_none())
        {
            while let Some(event) = sidecar.try_recv() {
                match event {
                    TinymistEvent::PreviewReady { url, .. } => preview_url = Some(url),
                    TinymistEvent::PublishDiagnostics { diagnostics, .. } => {
                        diagnostic = diagnostics.into_iter().next()
                    }
                    TinymistEvent::ShowDocument { selection, .. } => jump = selection,
                    _ => {}
                }
            }
            thread::sleep(Duration::from_millis(10));
        }

        assert_eq!(preview_url.as_deref(), Some("http://127.0.0.1:41723"));
        assert_eq!(diagnostic.unwrap().message, "fake warning");
        assert_eq!(jump.unwrap().start.line, 4);

        // Add another frame so the shell's line-oriented capture observes the
        // preceding response body even though LSP payloads have no delimiter.
        sidecar
            .did_change(generation, "file:///tmp/main.typ", 8, "Hello again")
            .unwrap();
        sidecar
            .did_change(generation, "file:///tmp/main.typ", 9, "capture flush")
            .unwrap();

        let reply_deadline = Instant::now() + FAKE_SERVER_TIMEOUT;
        loop {
            let captured = fs::read_to_string(&captured_input).unwrap_or_default();
            if captured.contains("\"id\":99")
                && captured.contains("\"success\":true")
                && captured.contains("Hello again")
            {
                assert!(
                    captured.contains("\"method\":\"textDocument/didChange\""),
                    "{captured:?}"
                );
                assert!(captured.contains("\"version\":8"), "{captured:?}");
                assert!(captured.contains("Hello again"), "{captured:?}");
                assert!(
                    captured.contains(&format!(
                        "\"typstExtraArgs\":[\"{}\"]",
                        pinned_entry.display()
                    )),
                    "{captured:?}"
                );
                break;
            }
            assert!(
                Instant::now() < reply_deadline,
                "client never acknowledged window/showDocument; captured {captured:?}"
            );
            thread::sleep(Duration::from_millis(10));
        }
    }
}
