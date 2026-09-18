use crate::worker::{LatestReceiver, LatestSender, latest_channel};
use std::{
    ffi::OsStr,
    fs,
    io::{BufRead, BufReader},
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::{
        Arc, LazyLock,
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc::{self, Receiver, RecvTimeoutError, Sender},
    },
    thread,
    time::{Duration, Instant},
};

use crate::{
    pdf::{PdfDocumentCatalog, inspect_pdf_with_program},
    process::finish_reader_with_timeout,
};

use crate::private_workspace::{PrivateTypstDocument, PrivateWorkspace};

const WORKER_POLL_INTERVAL: Duration = Duration::from_millis(15);
const WATCH_LOG_QUIET_PERIOD: Duration = Duration::from_millis(40);
const PIPE_READER_SHUTDOWN_TIMEOUT: Duration = Duration::from_millis(250);
const IGNORE_SYSTEM_FONTS_ENV: &str = "TIPTOPTYP_IGNORE_SYSTEM_FONTS";
const TRACE_WATCH_ENV: &str = "TIPTOPTYP_TRACE_WATCH";
static IGNORE_SYSTEM_FONTS: LazyLock<bool> =
    LazyLock::new(|| std::env::var_os(IGNORE_SYSTEM_FONTS_ENV).is_some());
static TRACE_WATCH: LazyLock<bool> = LazyLock::new(|| std::env::var_os(TRACE_WATCH_ENV).is_some());

#[derive(Debug, Clone)]
pub struct CompileRequest {
    pub revision: u64,
    /// Whether this build also needs page pixels. Export-only builds publish
    /// their canonical artifact without invoking the optional rasterizer.
    pub rasterize: bool,
    pub source: String,
    pub source_dir: PathBuf,
    pub project_root: PathBuf,
    pub display_name: String,
    pub typst_executable: PathBuf,
    pub font_paths: Vec<PathBuf>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct WatchContext {
    source_dir: PathBuf,
    project_root: PathBuf,
    typst_executable: PathBuf,
    font_paths: Vec<PathBuf>,
    display_name: String,
}

impl WatchContext {
    fn resolve(request: &CompileRequest) -> Result<Self, String> {
        let source_dir = request.source_dir.canonicalize().map_err(|error| {
            format!(
                "Could not use source directory {}: {error}",
                request.source_dir.display()
            )
        })?;
        let project_root = request.project_root.canonicalize().map_err(|error| {
            format!(
                "Could not use project root {}: {error}",
                request.project_root.display()
            )
        })?;
        if !source_dir.starts_with(&project_root) {
            return Err(format!(
                "Source directory {} is outside project root {}",
                source_dir.display(),
                project_root.display()
            ));
        }
        let mut font_paths = request
            .font_paths
            .iter()
            .filter_map(|path| path.canonicalize().ok())
            .filter(|path| path.starts_with(&project_root))
            .collect::<Vec<_>>();
        font_paths.sort();
        font_paths.dedup();
        Ok(Self {
            source_dir,
            project_root,
            typst_executable: request.typst_executable.clone(),
            font_paths,
            display_name: request.display_name.clone(),
        })
    }
}

#[derive(Debug, Clone)]
pub struct CompileArtifact {
    pub key: ArtifactKey,
    pub pdf: Arc<[u8]>,
    pub diagnostics: String,
}

pub use tiptoptyp_core::preview::ArtifactKey;

#[derive(Debug)]
pub enum CompileEvent {
    Started,
    Failed(String),
    Artifact(CompileArtifact),
    Catalog {
        key: ArtifactKey,
        catalog: PdfDocumentCatalog,
    },
    RasterFailed {
        key: ArtifactKey,
        error: String,
    },
}

#[derive(Debug)]
pub struct CompileResult {
    pub revision: u64,
    pub elapsed: Duration,
    pub event: CompileEvent,
}

enum CompilerCommand {
    Request(CompileRequest),
    Pause,
}

pub struct Compiler {
    requests: Option<LatestSender<CompilerCommand>>,
    results: Receiver<CompileResult>,
    disconnected: AtomicBool,
    worker: Option<thread::JoinHandle<()>>,
    shutdown: Arc<AtomicBool>,
    latest_revision: Arc<AtomicU64>,
}

impl Compiler {
    pub fn new(context: crate::worker::RepaintTarget) -> Self {
        let (request_tx, request_rx) = latest_channel::<CompilerCommand>();
        let (result_tx, result_rx) = mpsc::channel::<CompileResult>();
        let shutdown = Arc::new(AtomicBool::new(false));
        let latest_revision = Arc::new(AtomicU64::new(0));
        let worker_shutdown = shutdown.clone();
        let worker_latest_revision = latest_revision.clone();

        let worker = thread::Builder::new()
            .name("tiptoptyp-compiler".to_owned())
            .spawn(move || {
                worker_loop(
                    request_rx,
                    result_tx,
                    context,
                    worker_shutdown,
                    worker_latest_revision,
                )
            })
            .ok();

        Self {
            requests: worker.as_ref().map(|_| request_tx),
            results: result_rx,
            disconnected: AtomicBool::new(false),
            worker,
            shutdown,
            latest_revision,
        }
    }

    pub fn request(&self, request: CompileRequest) -> Result<(), String> {
        self.latest_revision
            .store(request.revision, Ordering::Release);
        self.requests
            .as_ref()
            .ok_or_else(|| "The preview worker has stopped".to_owned())?
            .send(CompilerCommand::Request(request))
            .map_err(|_| "The preview worker stopped unexpectedly".to_owned())
    }

    pub fn pause(&self, revision: u64) -> Result<(), String> {
        // Interrupt rasterization for the current revision before asking the
        // worker to reap its persistent `typst watch` child.
        self.latest_revision
            .store(revision.wrapping_add(1), Ordering::Release);
        self.requests
            .as_ref()
            .ok_or_else(|| "The preview worker has stopped".to_owned())?
            .send(CompilerCommand::Pause)
            .map_err(|_| "The preview worker stopped unexpectedly".to_owned())
    }

    pub fn try_recv(&self) -> Option<CompileResult> {
        Some(
            crate::worker::poll_service(&self.results, &self.disconnected)?.unwrap_or_else(|()| {
                CompileResult {
                    revision: self.latest_revision.load(Ordering::Acquire),
                    elapsed: Duration::ZERO,
                    event: CompileEvent::Failed(
                        "The preview worker stopped unexpectedly".to_owned(),
                    ),
                }
            }),
        )
    }
}

impl Drop for Compiler {
    fn drop(&mut self) {
        // Closing the only request sender wakes the worker. Joining guarantees
        // WatchSession::drop kills and reaps `typst watch` before the app exits.
        self.shutdown.store(true, Ordering::Release);
        self.requests.take();
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

#[derive(Debug)]
struct WatchLog {
    session_id: u64,
    line: String,
}

struct WatchSession {
    id: u64,
    context: WatchContext,
    // Owns the project-local mirror and removes it after the child is reaped.
    shadow: PrivateTypstDocument,
    pdf_path: PathBuf,
    child: Child,
    reader: Option<thread::JoinHandle<()>>,
    pending_revision: u64,
    pending_rasterize: bool,
    pending_display_name: String,
    active_revision: Option<u64>,
    active_rasterize: bool,
    active_display_name: String,
    active_started: Instant,
    diagnostics: Vec<String>,
    pending_completion: Option<PendingCompletion>,
}

struct PendingCompletion {
    revision: u64,
    rasterize: bool,
    elapsed: Duration,
    succeeded: bool,
    quiet_since: Instant,
}

impl WatchSession {
    fn start(
        id: u64,
        request: &CompileRequest,
        context: WatchContext,
        watch_logs: Sender<WatchLog>,
    ) -> Result<Self, String> {
        let private = PrivateWorkspace::open(&context.project_root).map_err(|error| {
            format!(
                "Could not prepare private editor storage in {}: {error}",
                context.project_root.display()
            )
        })?;
        let shadow = private
            .mirrored_typst_document(
                &context.source_dir,
                &context.display_name,
                &request.source,
            )
            .map_err(|error| {
                format!(
                    "Could not create a private live-preview document in {}: {error}. Ensure the project is writable.",
                    private.path().display()
                )
            })?;
        let output_dir = shadow.session_dir().join("watch-output");
        fs::create_dir(&output_dir)
            .map_err(|error| format!("Could not create a private preview directory: {error}"))?;
        let pdf_path = output_dir.join("preview.pdf");

        let mut command = Command::new(&context.typst_executable);
        command.arg("watch").arg("--diagnostic-format").arg("short");
        for font_path in &context.font_paths {
            command.arg("--font-path").arg(font_path);
        }
        if *IGNORE_SYSTEM_FONTS {
            // This only affects watcher startup; subsequent incremental builds
            // reuse the same font book and stay fast.
            command.arg("--ignore-system-fonts");
        }
        let shadow_path = shadow.path();
        let mut child = command
            .arg("--root")
            .arg(&context.project_root)
            .arg(shadow_path)
            .arg(&pdf_path)
            .current_dir(&context.source_dir)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|error| typst_command_error(&context.typst_executable, error))?;

        let stderr = child
            .stderr
            .take()
            .ok_or_else(|| "Could not read output from `typst watch`".to_owned())?;
        let reader = match thread::Builder::new()
            .name(format!("tiptoptyp-watch-log-{id}"))
            .spawn(move || {
                for line in BufReader::new(stderr).lines() {
                    let Ok(line) = line else {
                        break;
                    };
                    if watch_logs
                        .send(WatchLog {
                            session_id: id,
                            line,
                        })
                        .is_err()
                    {
                        break;
                    }
                }
            }) {
            Ok(reader) => reader,
            Err(error) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!(
                    "Could not start the Typst diagnostics reader: {error}"
                ));
            }
        };

        Ok(Self {
            id,
            context,
            shadow,
            pdf_path,
            child,
            reader: Some(reader),
            pending_revision: request.revision,
            pending_rasterize: request.rasterize,
            pending_display_name: request.display_name.clone(),
            active_revision: None,
            active_rasterize: request.rasterize,
            active_display_name: request.display_name.clone(),
            active_started: Instant::now(),
            diagnostics: Vec::new(),
            pending_completion: None,
        })
    }

    fn update(&mut self, request: &CompileRequest) -> Result<(), String> {
        self.shadow.update(&request.source).map_err(|error| {
            format!("Could not update the private live-preview document: {error}")
        })?;
        self.pending_revision = request.revision;
        self.pending_rasterize = request.rasterize;
        self.pending_display_name.clone_from(&request.display_name);
        Ok(())
    }

    fn matches_context(&self, context: &WatchContext) -> bool {
        self.context == *context
            && self.shadow.path().file_name() == Some(OsStr::new(&context.display_name))
    }

    fn clean_diagnostic(&self, line: &str) -> String {
        let shadow = self.shadow.path();
        let shadow_path = shadow.to_string_lossy();
        let mirror_root = self.shadow.mirror_root().to_string_lossy();
        let project_root = self.context.project_root.to_string_lossy();
        let shadow_name = shadow
            .file_name()
            .map(|name| name.to_string_lossy())
            .unwrap_or_default();
        line.replace(shadow_path.as_ref(), &self.active_display_name)
            .replace(shadow_name.as_ref(), &self.active_display_name)
            .replace(mirror_root.as_ref(), project_root.as_ref())
    }
}

impl Drop for WatchSession {
    fn drop(&mut self) {
        // Killing the child first releases its handles, then the private
        // document's TempDir removes the complete mirrored session. A wrapper
        // may have spawned a descendant which inherited stderr, so never let
        // that unrelated writer turn compiler shutdown into an unbounded join.
        let _ = self.child.kill();
        let _ = self.child.wait();
        if let Some(reader) = self.reader.take() {
            let _ = finish_reader_with_timeout(reader, PIPE_READER_SHUTDOWN_TIMEOUT);
        }
    }
}

fn worker_loop(
    requests: LatestReceiver<CompilerCommand>,
    results: Sender<CompileResult>,
    context: crate::worker::RepaintTarget,
    shutdown: Arc<AtomicBool>,
    latest_revision: Arc<AtomicU64>,
) {
    let (watch_log_tx, watch_log_rx) = mpsc::channel::<WatchLog>();
    let mut next_session_id = 1_u64;
    let mut next_artifact_generation = 1_u64;
    let mut session: Option<WatchSession> = None;

    loop {
        drain_watch_logs(&watch_log_rx, &mut session, &results, &context);
        finish_settled_completion(
            &mut session,
            &results,
            &context,
            &shutdown,
            &latest_revision,
            &mut next_artifact_generation,
        );

        // Only the latest queued command matters, whether it requests a new
        // editor snapshot or pauses the watcher.
        let command = if session.is_some() {
            requests.recv_timeout(WORKER_POLL_INTERVAL)
        } else {
            requests.recv().map_err(|_| RecvTimeoutError::Disconnected)
        };
        match command {
            Ok(CompilerCommand::Pause) => {
                session = None;
            }
            Ok(CompilerCommand::Request(request)) => {
                if *TRACE_WATCH {
                    eprintln!("tiptoptyp watcher request revision {}", request.revision);
                }
                // Finish processing any event already emitted by the old source
                // before changing the shadow file to the new revision.
                drain_watch_logs(&watch_log_rx, &mut session, &results, &context);
                finish_settled_completion(
                    &mut session,
                    &results,
                    &context,
                    &shutdown,
                    &latest_revision,
                    &mut next_artifact_generation,
                );

                // macOS filesystem notifications intentionally suppress
                // updates below dot-directories for this watcher path. All
                // editor-private files must remain in `.tiptoptyp`, so start a
                // fresh watcher for an edited buffer there; its initial build
                // is deterministic. Tinymist remains the low-latency primary
                // preview, while other platforms can keep the watcher alive.
                let resolved_context = match WatchContext::resolve(&request) {
                    Ok(context) => context,
                    Err(error) => {
                        send_result(
                            &results,
                            &context,
                            CompileResult {
                                revision: request.revision,
                                elapsed: Duration::ZERO,
                                event: CompileEvent::Failed(error),
                            },
                        );
                        continue;
                    }
                };
                let reuse = !cfg!(target_os = "macos")
                    && session
                        .as_ref()
                        .is_some_and(|current| current.matches_context(&resolved_context));
                let update_result = if let (true, Some(current)) = (reuse, session.as_mut()) {
                    current.update(&request)
                } else {
                    session = None;
                    let id = next_session_id;
                    next_session_id = next_session_id.wrapping_add(1);
                    WatchSession::start(id, &request, resolved_context, watch_log_tx.clone())
                        .map(|new_session| session = Some(new_session))
                };

                if let Err(error) = update_result {
                    send_result(
                        &results,
                        &context,
                        CompileResult {
                            revision: request.revision,
                            elapsed: Duration::ZERO,
                            event: CompileEvent::Failed(error),
                        },
                    );
                }
            }
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => break,
        }

        let watcher_exit = session.as_mut().and_then(|current| {
            current
                .child
                .try_wait()
                .ok()
                .flatten()
                .map(|status| (current.pending_revision, status))
        });
        if let Some((revision, status)) = watcher_exit {
            send_result(
                &results,
                &context,
                CompileResult {
                    revision,
                    elapsed: Duration::ZERO,
                    event: CompileEvent::Failed(format!(
                        "`typst watch` stopped unexpectedly with {status}"
                    )),
                },
            );
            session = None;
        }
    }
}

fn drain_watch_logs(
    logs: &Receiver<WatchLog>,
    session: &mut Option<WatchSession>,
    results: &Sender<CompileResult>,
    context: &crate::worker::RepaintTarget,
) {
    while let Ok(log) = logs.try_recv() {
        if *TRACE_WATCH {
            eprintln!(
                "tiptoptyp watcher log session {}: {}",
                log.session_id, log.line
            );
        }
        let Some(current) = session.as_mut() else {
            continue;
        };
        if log.session_id != current.id {
            continue;
        }

        if let Some(completion) = &mut current.pending_completion {
            completion.quiet_since = Instant::now();
        }

        match classify_watch_line(&log.line) {
            WatchLine::CompileStarted => {
                // If Typst starts again before a terminal result has settled,
                // that result no longer represents the live output path.
                current.pending_completion = None;
                current.active_revision = Some(current.pending_revision);
                current.active_rasterize = current.pending_rasterize;
                current
                    .active_display_name
                    .clone_from(&current.pending_display_name);
                current.active_started = Instant::now();
                current.diagnostics.clear();
                send_result(
                    results,
                    context,
                    CompileResult {
                        revision: current.pending_revision,
                        elapsed: Duration::ZERO,
                        event: CompileEvent::Started,
                    },
                );
            }
            WatchLine::CompileSucceeded => {
                let revision = current
                    .active_revision
                    .take()
                    .unwrap_or(current.pending_revision);
                current.pending_completion = Some(PendingCompletion {
                    revision,
                    rasterize: current.active_rasterize,
                    elapsed: current.active_started.elapsed(),
                    succeeded: true,
                    quiet_since: Instant::now(),
                });
            }
            WatchLine::CompileFailed => {
                let revision = current
                    .active_revision
                    .take()
                    .unwrap_or(current.pending_revision);
                current.pending_completion = Some(PendingCompletion {
                    revision,
                    rasterize: current.active_rasterize,
                    elapsed: current.active_started.elapsed(),
                    succeeded: false,
                    quiet_since: Instant::now(),
                });
            }
            WatchLine::Ignore => {}
            WatchLine::Diagnostic => {
                current
                    .diagnostics
                    .push(current.clean_diagnostic(&log.line));
            }
        }
    }
}

fn finish_settled_completion(
    session: &mut Option<WatchSession>,
    results: &Sender<CompileResult>,
    context: &crate::worker::RepaintTarget,
    shutdown: &AtomicBool,
    latest_revision: &AtomicU64,
    next_artifact_generation: &mut u64,
) {
    let Some(current) = session.as_mut() else {
        return;
    };
    let settled = current
        .pending_completion
        .as_ref()
        .is_some_and(|completion| completion.quiet_since.elapsed() >= WATCH_LOG_QUIET_PERIOD);
    if !settled {
        return;
    }

    let completion = current.pending_completion.take().unwrap();
    let diagnostics = current.diagnostics.join("\n");
    if completion.succeeded {
        let key = ArtifactKey {
            revision: completion.revision,
            generation: *next_artifact_generation,
        };
        *next_artifact_generation = (*next_artifact_generation).wrapping_add(1).max(1);
        publish_compiled_artifact(
            &current.pdf_path,
            &current.context.project_root,
            diagnostics,
            key,
            completion.elapsed,
            completion.rasterize,
            shutdown,
            latest_revision,
            Path::new("pdfinfo"),
            results,
            context,
        );
        return;
    }
    let error = if diagnostics.is_empty() {
        "Typst could not compile the document".to_owned()
    } else {
        diagnostics
    };
    send_result(
        results,
        context,
        CompileResult {
            revision: completion.revision,
            elapsed: completion.elapsed,
            event: CompileEvent::Failed(error),
        },
    );
}

#[allow(clippy::too_many_arguments)]
fn publish_compiled_artifact(
    pdf_path: &Path,
    project_root: &Path,
    diagnostics: String,
    key: ArtifactKey,
    elapsed: Duration,
    rasterize: bool,
    shutdown: &AtomicBool,
    latest_revision: &AtomicU64,
    inspector: &Path,
    results: &Sender<CompileResult>,
    context: &crate::worker::RepaintTarget,
) {
    // The watcher owns and may atomically replace `pdf_path` again (for example
    // after a dependency edit). Snapshot exactly once and share those immutable
    // bytes with export and rasterization so their artifact identity cannot
    // drift even if the watcher writes a newer output while Poppler is active.
    let pdf: Arc<[u8]> = match fs::read(pdf_path) {
        Ok(pdf) => pdf.into(),
        Err(error) => {
            send_result(
                results,
                context,
                CompileResult {
                    revision: key.revision,
                    elapsed,
                    event: CompileEvent::Failed(format!(
                        "Could not snapshot the compiled PDF: {error}"
                    )),
                },
            );
            return;
        }
    };
    send_result(
        results,
        context,
        CompileResult {
            revision: key.revision,
            elapsed,
            event: CompileEvent::Artifact(CompileArtifact {
                key,
                pdf: pdf.clone(),
                diagnostics,
            }),
        },
    );
    if !rasterize {
        return;
    }

    let event = match inspect_pdf_with_program(&pdf, project_root, inspector, || {
        shutdown.load(Ordering::Acquire) || latest_revision.load(Ordering::Acquire) != key.revision
    }) {
        Ok(catalog) => CompileEvent::Catalog { key, catalog },
        Err(error) => CompileEvent::RasterFailed { key, error },
    };
    send_result(
        results,
        context,
        CompileResult {
            revision: key.revision,
            elapsed,
            event,
        },
    );
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WatchLine {
    CompileStarted,
    CompileSucceeded,
    CompileFailed,
    Diagnostic,
    Ignore,
}

fn classify_watch_line(line: &str) -> WatchLine {
    let trimmed = line.trim();
    let message = if trimmed.starts_with('[') {
        trimmed
            .find("] ")
            .map_or(trimmed, |closing_bracket| &trimmed[closing_bracket + 2..])
    } else {
        trimmed
    };
    let normalized = message.to_ascii_lowercase();
    if normalized.starts_with("compiled successfully")
        || normalized.starts_with("compiled with warnings")
    {
        WatchLine::CompileSucceeded
    } else if normalized.starts_with("compiled with errors")
        || normalized.starts_with("compilation failed")
    {
        WatchLine::CompileFailed
    } else if normalized.starts_with("compiling") {
        WatchLine::CompileStarted
    } else if normalized.starts_with("watching ")
        || normalized.starts_with("writing to ")
        || normalized.is_empty()
    {
        WatchLine::Ignore
    } else {
        WatchLine::Diagnostic
    }
}

fn typst_command_error(program: &Path, error: std::io::Error) -> String {
    format!(
        "Could not start Typst at {}: {error}. Choose a working binary in Settings.",
        program.display()
    )
}

fn send_result(
    results: &Sender<CompileResult>,
    context: &crate::worker::RepaintTarget,
    result: CompileResult,
) {
    if results.send(result).is_ok() {
        context.request_repaint();
    }
}

#[cfg(test)]
mod tests {
    use super::{
        ArtifactKey, CompileEvent, CompileRequest, CompileResult, Compiler, CompilerCommand,
        WatchContext, WatchLine, classify_watch_line, publish_compiled_artifact, worker_loop,
    };
    use crate::worker::latest_channel;
    use std::{
        fs,
        path::PathBuf,
        sync::{
            Arc,
            atomic::{AtomicBool, AtomicU64},
            mpsc,
        },
        time::{Duration, Instant},
    };

    #[test]
    fn recognizes_watch_status_lines() {
        assert_eq!(
            classify_watch_line("[12:00:00] compiling ..."),
            WatchLine::CompileStarted
        );
        assert_eq!(
            classify_watch_line("[12:00:00] compiled successfully in 5.2 ms"),
            WatchLine::CompileSucceeded
        );
        assert_eq!(
            classify_watch_line("[12:00:00] compiled with errors"),
            WatchLine::CompileFailed
        );
        assert_eq!(
            classify_watch_line("document.typ:2:4: error: bad expression"),
            WatchLine::Diagnostic
        );
    }

    #[test]
    fn an_idle_compiler_can_be_paused_and_reaped_cleanly() {
        let compiler = Compiler::new(crate::worker::RepaintTarget::test());
        compiler.pause(7).expect("pause compiler worker");
        drop(compiler);
    }

    #[test]
    fn queued_compiler_commands_apply_only_the_last_request_or_pause() {
        let project = tempfile::tempdir().unwrap();
        for commands in [
            vec![Some(1), Some(2)],
            vec![Some(1), None],
            vec![None, Some(2)],
            vec![Some(1), None, Some(2)],
            vec![None, Some(1), None],
        ] {
            let expected_revision = *commands.last().unwrap();
            let (request_tx, request_rx) = latest_channel();
            let (result_tx, result_rx) = mpsc::channel();
            for revision in commands {
                let command = revision.map_or(CompilerCommand::Pause, |revision| {
                    CompilerCommand::Request(CompileRequest {
                        revision,
                        rasterize: false,
                        source: String::new(),
                        // Resolve fails deterministically before starting a process.
                        source_dir: project.path().join("missing"),
                        project_root: project.path().to_path_buf(),
                        display_name: "main.typ".to_owned(),
                        typst_executable: PathBuf::from("unused-typst"),
                        font_paths: Vec::new(),
                    })
                });
                request_tx.send(command).unwrap();
            }
            drop(request_tx);
            worker_loop(
                request_rx,
                result_tx,
                crate::worker::RepaintTarget::test(),
                Arc::new(AtomicBool::new(false)),
                Arc::new(AtomicU64::new(0)),
            );
            let results = result_rx.try_iter().collect::<Vec<_>>();
            assert_eq!(results.len(), usize::from(expected_revision.is_some()));
            if let Some(revision) = expected_revision {
                assert_eq!(results[0].revision, revision);
                assert!(matches!(results[0].event, CompileEvent::Failed(_)));
            }
        }
    }

    #[test]
    fn successful_artifact_is_published_before_missing_metadata_failure() {
        let project = tempfile::tempdir().unwrap();
        let pdf_path = project.path().join("compiled.pdf");
        let expected = b"%PDF-exact-artifact\0bytes";
        fs::write(&pdf_path, expected).unwrap();
        let (result_tx, result_rx) = mpsc::channel();
        let shutdown = AtomicBool::new(false);
        let latest_revision = AtomicU64::new(7);

        publish_compiled_artifact(
            &pdf_path,
            project.path(),
            "warning diagnostic".to_owned(),
            ArtifactKey {
                revision: 7,
                generation: 11,
            },
            Duration::from_millis(12),
            true,
            &shutdown,
            &latest_revision,
            &project.path().join("missing-pdfinfo"),
            &result_tx,
            &crate::worker::RepaintTarget::test(),
        );

        let results = result_rx.try_iter().collect::<Vec<_>>();
        assert_eq!(results.len(), 2);
        let CompileEvent::Artifact(artifact) = &results[0].event else {
            panic!("artifact must be published before rasterization")
        };
        assert_eq!(artifact.pdf.as_ref(), expected);
        assert_eq!(artifact.key.generation, 11);
        assert_eq!(artifact.diagnostics, "warning diagnostic");
        assert!(matches!(
            &results[1].event,
            CompileEvent::RasterFailed { key, error }
                if key == &artifact.key && error.contains("Poppler was not found")
        ));
    }

    #[cfg(unix)]
    #[test]
    fn a_new_artifact_can_recover_after_a_metadata_failure() {
        use std::os::unix::fs::PermissionsExt;

        let project = tempfile::tempdir().unwrap();
        let pdf_path = project.path().join("compiled.pdf");
        fs::write(&pdf_path, b"%PDF-stable-snapshot").unwrap();
        let inspector = project.path().join("fake-pdfinfo");
        fs::write(
            &inspector,
            "#!/bin/sh\nprintf 'Pages: 1\\nPage 1 size: 1 x 0.5 pts\\n'\n",
        )
        .unwrap();
        fs::set_permissions(&inspector, fs::Permissions::from_mode(0o700)).unwrap();

        let (result_tx, result_rx) = mpsc::channel();
        let shutdown = AtomicBool::new(false);
        let latest_revision = AtomicU64::new(7);
        for (generation, program) in [
            (11, project.path().join("missing-pdfinfo")),
            (12, inspector),
        ] {
            publish_compiled_artifact(
                &pdf_path,
                project.path(),
                String::new(),
                ArtifactKey {
                    revision: 7,
                    generation,
                },
                Duration::ZERO,
                true,
                &shutdown,
                &latest_revision,
                &program,
                &result_tx,
                &crate::worker::RepaintTarget::test(),
            );
        }

        let results = result_rx.try_iter().collect::<Vec<_>>();
        assert_eq!(results.len(), 4);
        assert!(matches!(
            &results[0].event,
            CompileEvent::Artifact(artifact) if artifact.key.generation == 11
        ));
        assert!(matches!(
            &results[1].event,
            CompileEvent::RasterFailed { key, .. } if key.generation == 11
        ));
        assert!(matches!(
            &results[2].event,
            CompileEvent::Artifact(artifact) if artifact.key.generation == 12
        ));
        assert!(matches!(
            &results[3].event,
            CompileEvent::Catalog { key, catalog }
                if key.generation == 12
                    && catalog.pages.len() == 1
                    && catalog.pages[0].size == [2, 1]
        ));
    }

    #[test]
    fn changing_typst_executable_invalidates_the_watch_session() {
        let root = tempfile::tempdir().unwrap();
        let source = root.path().join("source");
        std::fs::create_dir(&source).unwrap();
        let mut request = CompileRequest {
            revision: 1,
            rasterize: false,
            source: String::new(),
            source_dir: source,
            project_root: root.path().to_path_buf(),
            display_name: "main.typ".to_owned(),
            typst_executable: PathBuf::from("/tools/typst-bundled"),
            font_paths: Vec::new(),
        };
        let context = WatchContext::resolve(&request).unwrap();
        assert_eq!(WatchContext::resolve(&request).unwrap(), context);
        request.typst_executable = PathBuf::from("/tools/typst-custom");
        assert_ne!(WatchContext::resolve(&request).unwrap(), context);
    }

    #[test]
    fn watch_context_keeps_only_canonical_project_font_directories() {
        let project = tempfile::tempdir().unwrap();
        let fonts = project.path().join("assets/fonts");
        fs::create_dir_all(&fonts).unwrap();
        let outside = tempfile::tempdir().unwrap();
        let request = CompileRequest {
            revision: 1,
            rasterize: false,
            source: String::new(),
            source_dir: project.path().to_path_buf(),
            project_root: project.path().to_path_buf(),
            display_name: "main.typ".to_owned(),
            typst_executable: PathBuf::from("typst"),
            font_paths: vec![
                fonts.clone(),
                fonts.clone(),
                outside.path().to_path_buf(),
                project.path().join("missing"),
            ],
        };
        let context = WatchContext::resolve(&request).unwrap();
        assert_eq!(context.font_paths, vec![fonts.canonicalize().unwrap()]);
    }

    #[cfg(unix)]
    #[test]
    fn watch_context_accepts_canonical_and_symlink_aliases_but_not_missing_paths() {
        use std::os::unix::fs::symlink;

        let project = tempfile::tempdir().unwrap();
        let source = project.path().join("source");
        fs::create_dir(&source).unwrap();
        let alias = project.path().join("alias");
        symlink(&source, &alias).unwrap();
        let mut request = CompileRequest {
            revision: 1,
            rasterize: false,
            source: String::new(),
            source_dir: source.canonicalize().unwrap(),
            project_root: project.path().canonicalize().unwrap(),
            display_name: "main.typ".to_owned(),
            typst_executable: PathBuf::from("/tools/typst"),
            font_paths: Vec::new(),
        };
        let context = WatchContext::resolve(&request).unwrap();
        request.source_dir = alias;
        request.project_root = project.path().to_path_buf();
        assert_eq!(WatchContext::resolve(&request).unwrap(), context);
        request.source_dir = project.path().join("missing");
        assert!(WatchContext::resolve(&request).is_err());
    }

    #[test]
    #[ignore = "requires typst, pdftoppm, and real filesystem notifications"]
    fn persistent_watcher_compiles_errors_and_recovers() {
        let root = tempfile::tempdir().unwrap();
        let compiler = Compiler::new(crate::worker::RepaintTarget::test());
        let typst_executable = std::env::var_os("TIPTOPTYP_TEST_TYPST")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("typst"));
        let request = |revision, source: &str| CompileRequest {
            revision,
            rasterize: true,
            source: source.to_owned(),
            source_dir: root.path().to_path_buf(),
            project_root: root.path().to_path_buf(),
            display_name: "integration.typ".to_owned(),
            typst_executable: typst_executable.clone(),
            font_paths: Vec::new(),
        };

        compiler.request(request(1, "= First build")).unwrap();
        let first_artifact = wait_for_revision(&compiler, 1, |event| {
            matches!(event, CompileEvent::Artifact(_))
        });
        assert!(matches!(
            first_artifact.event,
            CompileEvent::Artifact(artifact) if artifact.pdf.starts_with(b"%PDF")
        ));
        let first_raster = wait_for_revision(&compiler, 1, |event| {
            matches!(event, CompileEvent::Catalog { .. })
        });
        assert!(matches!(
            first_raster.event,
            CompileEvent::Catalog { catalog, .. } if catalog.pages.len() == 1
        ));

        compiler.request(request(2, "#let broken =")).unwrap();
        let failed = wait_for_revision(&compiler, 2, |event| {
            matches!(event, CompileEvent::Failed(_))
        });
        let CompileEvent::Failed(error) = failed.event else {
            unreachable!()
        };
        assert!(error.contains("expected expression"), "{error}");

        compiler
            .request(request(3, "= Recovered\n#pagebreak()\n= Page two"))
            .unwrap();
        let recovered = wait_for_revision(&compiler, 3, |event| {
            matches!(event, CompileEvent::Catalog { .. })
        });
        assert!(matches!(
            recovered.event,
            CompileEvent::Catalog { catalog, .. } if catalog.pages.len() == 2
        ));
    }

    fn wait_for_revision(
        compiler: &Compiler,
        revision: u64,
        accepts: impl Fn(&CompileEvent) -> bool,
    ) -> CompileResult {
        let deadline = Instant::now() + Duration::from_secs(10);
        while Instant::now() < deadline {
            if let Some(result) = compiler.try_recv()
                && result.revision == revision
                && accepts(&result.event)
            {
                return result;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        panic!("timed out waiting for watcher revision {revision}");
    }
}
