use std::{
    ffi::OsStr,
    fs,
    io::{BufRead, BufReader, Read},
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

use eframe::egui;
use quick_xml::{
    Decoder, Reader, XmlVersion,
    events::{BytesStart, Event},
};

use crate::private_workspace::{PrivateTypstDocument, PrivateWorkspace};

/// Resolution used only by the native recovery viewer. The primary Tinymist
/// viewer is vector-based. 144 DPI keeps the fallback crisp at 100% on a 2×
/// display without making every incremental build excessively expensive.
pub const PREVIEW_DPI: f32 = 144.0;
const WORKER_POLL_INTERVAL: Duration = Duration::from_millis(15);
const WATCH_LOG_QUIET_PERIOD: Duration = Duration::from_millis(40);
const IGNORE_SYSTEM_FONTS_ENV: &str = "TIPTOPTYP_IGNORE_SYSTEM_FONTS";
const TRACE_WATCH_ENV: &str = "TIPTOPTYP_TRACE_WATCH";
static IGNORE_SYSTEM_FONTS: LazyLock<bool> =
    LazyLock::new(|| std::env::var_os(IGNORE_SYSTEM_FONTS_ENV).is_some());
static TRACE_WATCH: LazyLock<bool> = LazyLock::new(|| std::env::var_os(TRACE_WATCH_ENV).is_some());

#[derive(Debug, Clone)]
pub struct CompileRequest {
    pub revision: u64,
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

#[derive(Debug)]
pub struct PreviewPage {
    pub size: [usize; 2],
    pub rgba: Vec<u8>,
    pub links: Vec<PreviewLink>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct PreviewLink {
    /// Link rectangle normalized to the page's width and height.
    pub rect: [f32; 4],
    pub target: String,
}

#[derive(Debug)]
pub struct CompiledDocument {
    pub pdf: Vec<u8>,
    pub pages: Vec<PreviewPage>,
    pub diagnostics: String,
}

#[derive(Debug)]
pub struct CompileResult {
    pub revision: u64,
    pub elapsed: Duration,
    /// `None` announces that Typst started rebuilding (including after an
    /// imported dependency changed). `Some` is a terminal success or failure.
    pub output: Option<Result<CompiledDocument, String>>,
}

enum CompilerCommand {
    Request(CompileRequest),
    Pause,
}

pub struct Compiler {
    requests: Option<Sender<CompilerCommand>>,
    results: Receiver<CompileResult>,
    worker: Option<thread::JoinHandle<()>>,
    shutdown: Arc<AtomicBool>,
    latest_revision: Arc<AtomicU64>,
}

impl Compiler {
    pub fn new(context: egui::Context) -> Self {
        let (request_tx, request_rx) = mpsc::channel::<CompilerCommand>();
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
            .expect("failed to start compiler worker");

        Self {
            requests: Some(request_tx),
            results: result_rx,
            worker: Some(worker),
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
        self.results.try_recv().ok()
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
    pending_display_name: String,
    active_revision: Option<u64>,
    active_display_name: String,
    active_started: Instant,
    diagnostics: Vec<String>,
    pending_completion: Option<PendingCompletion>,
}

struct PendingCompletion {
    revision: u64,
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
            pending_display_name: request.display_name.clone(),
            active_revision: None,
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
        // document's TempDir removes the complete mirrored session.
        let _ = self.child.kill();
        let _ = self.child.wait();
        if let Some(reader) = self.reader.take() {
            let _ = reader.join();
        }
    }
}

fn worker_loop(
    requests: Receiver<CompilerCommand>,
    results: Sender<CompileResult>,
    context: egui::Context,
    shutdown: Arc<AtomicBool>,
    latest_revision: Arc<AtomicU64>,
) {
    let (watch_log_tx, watch_log_rx) = mpsc::channel::<WatchLog>();
    let mut next_session_id = 1_u64;
    let mut session: Option<WatchSession> = None;

    loop {
        drain_watch_logs(&watch_log_rx, &mut session, &results, &context);
        finish_settled_completion(
            &mut session,
            &results,
            &context,
            &shutdown,
            &latest_revision,
        );

        // Only the latest queued command matters, whether it requests a new
        // editor snapshot or pauses the watcher.
        let command = requests
            .recv_timeout(WORKER_POLL_INTERVAL)
            .map(|first| requests.try_iter().last().unwrap_or(first));
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
                                output: Some(Err(error)),
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
                            output: Some(Err(error)),
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
                    output: Some(Err(format!(
                        "`typst watch` stopped unexpectedly with {status}"
                    ))),
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
    context: &egui::Context,
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
                        output: None,
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
    context: &egui::Context,
    shutdown: &AtomicBool,
    latest_revision: &AtomicU64,
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
    let output = if completion.succeeded {
        render_pdf(
            &current.pdf_path,
            &current.context.project_root,
            diagnostics,
            completion.revision,
            shutdown,
            latest_revision,
        )
    } else if diagnostics.is_empty() {
        Err("Typst could not compile the document".to_owned())
    } else {
        Err(diagnostics)
    };
    send_result(
        results,
        context,
        CompileResult {
            revision: completion.revision,
            elapsed: completion.elapsed,
            output: Some(output),
        },
    );
}

fn render_pdf(
    pdf_path: &Path,
    project_root: &Path,
    diagnostics: String,
    revision: u64,
    shutdown: &AtomicBool,
    latest_revision: &AtomicU64,
) -> Result<CompiledDocument, String> {
    // The watcher owns and may atomically replace `pdf_path` again (for example
    // after a dependency edit). Snapshot once so page textures and exported
    // bytes are guaranteed to describe the exact same build.
    let pdf = fs::read(pdf_path)
        .map_err(|error| format!("Could not snapshot the compiled PDF: {error}"))?;
    let pages = rasterize_pdf(&pdf, project_root, || {
        shutdown.load(Ordering::Acquire) || latest_revision.load(Ordering::Acquire) != revision
    })?;
    Ok(CompiledDocument {
        pdf,
        pages,
        diagnostics,
    })
}

/// Rasterize already-snapshotted PDF bytes for either a Typst build or a PDF
/// opened directly from the project tree. The caller owns cancellation, which
/// lets the compiler and the asset loader discard obsolete long documents.
pub(crate) fn rasterize_pdf(
    pdf: &[u8],
    project_root: &Path,
    mut cancelled: impl FnMut() -> bool,
) -> Result<Vec<PreviewPage>, String> {
    let private = PrivateWorkspace::open(project_root).map_err(|error| {
        format!(
            "Could not prepare private PDF-rendering storage in {}: {error}",
            project_root.display()
        )
    })?;
    let render_dir = private
        .temp_dir("pages-")
        .map_err(|error| format!("Could not create a private page-rendering directory: {error}"))?;
    let snapshot_path = render_dir.path().join("snapshot.pdf");
    fs::write(&snapshot_path, pdf)
        .map_err(|error| format!("Could not stage the PDF preview: {error}"))?;
    let page_prefix = render_dir.path().join("page");
    let mut render_child = Command::new("pdftoppm")
        .arg("-png")
        .arg("-r")
        .arg(PREVIEW_DPI.to_string())
        .arg(&snapshot_path)
        .arg(&page_prefix)
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| command_error("pdftoppm", error))?;

    let stderr = render_child
        .stderr
        .take()
        .ok_or_else(|| "Could not read output from `pdftoppm`".to_owned())?;
    let stderr_reader = match thread::Builder::new()
        .name("tiptoptyp-pdf-render-log".to_owned())
        .spawn(move || {
            let mut bytes = Vec::new();
            let _ = BufReader::new(stderr).read_to_end(&mut bytes);
            bytes
        }) {
        Ok(reader) => reader,
        Err(error) => {
            let _ = render_child.kill();
            let _ = render_child.wait();
            return Err(format!(
                "Could not start the PDF renderer log reader: {error}"
            ));
        }
    };

    let render_status = loop {
        if cancelled() {
            let _ = render_child.kill();
            let _ = render_child.wait();
            let _ = stderr_reader.join();
            return Err("Preview rendering was superseded by a newer edit".to_owned());
        }
        match render_child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) => thread::sleep(Duration::from_millis(10)),
            Err(error) => {
                let _ = render_child.kill();
                let _ = render_child.wait();
                let _ = stderr_reader.join();
                return Err(format!("Could not wait for the PDF renderer: {error}"));
            }
        }
    };
    let render_stderr = stderr_reader.join().unwrap_or_default();

    if !render_status.success() {
        let details = String::from_utf8_lossy(&render_stderr);
        return Err(format!("PDF preview rendering failed: {}", details.trim()));
    }

    let mut page_paths = fs::read_dir(render_dir.path())
        .map_err(|error| format!("Could not read rendered preview pages: {error}"))?
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| {
            path.extension().is_some_and(|extension| extension == "png")
                && path
                    .file_stem()
                    .is_some_and(|stem| stem.to_string_lossy().starts_with("page-"))
        })
        .collect::<Vec<_>>();
    page_paths.sort_by_key(|path| preview_page_number(path));

    if page_paths.is_empty() {
        return Err("The PDF renderer produced no preview pages".to_owned());
    }

    // Link extraction is best-effort: raster rendering remains useful when an
    // older/minimal Poppler installation lacks `pdftohtml`.
    let mut page_links = extract_pdf_links(&snapshot_path, render_dir.path(), &mut cancelled);
    let mut pages = Vec::with_capacity(page_paths.len());
    for (index, page_path) in page_paths.into_iter().enumerate() {
        if cancelled() {
            return Err("Preview decoding was superseded by a newer edit".to_owned());
        }
        let encoded = fs::read(&page_path)
            .map_err(|error| format!("Could not read a preview page: {error}"))?;
        let decoded = image::load_from_memory_with_format(&encoded, image::ImageFormat::Png)
            .map_err(|error| format!("Could not decode a preview page: {error}"))?
            .into_rgba8();
        let (width, height) = decoded.dimensions();
        pages.push(PreviewPage {
            size: [width as usize, height as usize],
            rgba: decoded.into_raw(),
            links: page_links
                .get_mut(index)
                .map(std::mem::take)
                .unwrap_or_default(),
        });
    }

    Ok(pages)
}

fn extract_pdf_links(
    snapshot_path: &Path,
    render_dir: &Path,
    cancelled: &mut impl FnMut() -> bool,
) -> Vec<Vec<PreviewLink>> {
    let xml_path = render_dir.join("links.xml");
    let mut child = match Command::new("pdftohtml")
        .arg("-xml")
        .arg("-hidden")
        .arg("-i")
        .arg("-q")
        .arg("-zoom")
        .arg("1")
        .arg(snapshot_path)
        .arg(&xml_path)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
    {
        Ok(child) => child,
        Err(_) => return Vec::new(),
    };

    let status = loop {
        if cancelled() {
            let _ = child.kill();
            let _ = child.wait();
            return Vec::new();
        }
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) => thread::sleep(Duration::from_millis(10)),
            Err(_) => {
                let _ = child.kill();
                let _ = child.wait();
                return Vec::new();
            }
        }
    };
    if !status.success() {
        return Vec::new();
    }

    let Ok(xml) = fs::read_to_string(&xml_path) else {
        return Vec::new();
    };
    let generated_stem = xml_path
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or("links");
    parse_pdf_links(&xml, generated_stem).unwrap_or_default()
}

fn parse_pdf_links(xml: &str, generated_stem: &str) -> Result<Vec<Vec<PreviewLink>>, String> {
    let mut reader = Reader::from_str(xml);
    reader.config_mut().trim_text(true);
    let mut pages = Vec::<Vec<PreviewLink>>::new();
    let mut current_page = None;
    let mut page_size = [0.0_f32; 2];
    let mut text_rect = None;

    loop {
        match reader.read_event() {
            Ok(Event::Start(element)) => match element.name().as_ref() {
                b"page" => {
                    let number = xml_attr(&element, b"number", reader.decoder())
                        .and_then(|value| value.parse::<usize>().ok())
                        .unwrap_or(pages.len() + 1)
                        .saturating_sub(1);
                    let width = xml_attr(&element, b"width", reader.decoder())
                        .and_then(|value| value.parse::<f32>().ok())
                        .unwrap_or(0.0);
                    let height = xml_attr(&element, b"height", reader.decoder())
                        .and_then(|value| value.parse::<f32>().ok())
                        .unwrap_or(0.0);
                    if pages.len() <= number {
                        pages.resize_with(number + 1, Vec::new);
                    }
                    current_page = Some(number);
                    page_size = [width, height];
                }
                b"text" => {
                    let left = xml_f32_attr(&element, b"left", reader.decoder());
                    let top = xml_f32_attr(&element, b"top", reader.decoder());
                    let width = xml_f32_attr(&element, b"width", reader.decoder());
                    let height = xml_f32_attr(&element, b"height", reader.decoder());
                    text_rect = match (left, top, width, height) {
                        (Some(left), Some(top), Some(width), Some(height))
                            if page_size[0] > 0.0
                                && page_size[1] > 0.0
                                && width > 0.0
                                && height > 0.0 =>
                        {
                            Some([
                                (left / page_size[0]).clamp(0.0, 1.0),
                                (top / page_size[1]).clamp(0.0, 1.0),
                                ((left + width) / page_size[0]).clamp(0.0, 1.0),
                                ((top + height) / page_size[1]).clamp(0.0, 1.0),
                            ])
                        }
                        _ => None,
                    };
                }
                b"a" => {
                    if let (Some(page), Some(rect), Some(target)) = (
                        current_page,
                        text_rect,
                        xml_attr(&element, b"href", reader.decoder()),
                    ) {
                        pages[page].push(PreviewLink {
                            rect,
                            target: normalize_pdftohtml_target(&target, generated_stem),
                        });
                    }
                }
                _ => {}
            },
            Ok(Event::End(element)) => match element.name().as_ref() {
                b"text" => text_rect = None,
                b"page" => {
                    current_page = None;
                    page_size = [0.0; 2];
                }
                _ => {}
            },
            Ok(Event::Empty(element)) if element.name().as_ref() == b"page" => {
                let number = xml_attr(&element, b"number", reader.decoder())
                    .and_then(|value| value.parse::<usize>().ok())
                    .unwrap_or(pages.len() + 1)
                    .saturating_sub(1);
                if pages.len() <= number {
                    pages.resize_with(number + 1, Vec::new);
                }
            }
            Ok(Event::Eof) => break,
            Ok(_) => {}
            Err(error) => return Err(format!("Could not parse PDF links: {error}")),
        }
    }
    Ok(pages)
}

fn xml_attr(element: &BytesStart<'_>, name: &[u8], decoder: Decoder) -> Option<String> {
    element
        .attributes()
        .flatten()
        .find(|attribute| attribute.key.as_ref() == name)
        .and_then(|attribute| {
            attribute
                .decoded_and_normalized_value(XmlVersion::Implicit1_0, decoder)
                .ok()
        })
        .map(|value| value.into_owned())
}

fn xml_f32_attr(element: &BytesStart<'_>, name: &[u8], decoder: Decoder) -> Option<f32> {
    xml_attr(element, name, decoder)?.parse().ok()
}

fn normalize_pdftohtml_target(target: &str, generated_stem: &str) -> String {
    let Some((path, fragment)) = target.rsplit_once('#') else {
        return target.to_owned();
    };
    let generated_internal = Path::new(path)
        .extension()
        .is_some_and(|extension| extension.eq_ignore_ascii_case("html"))
        && Path::new(path)
            .file_stem()
            .is_some_and(|stem| stem == generated_stem);
    if generated_internal && fragment.parse::<usize>().is_ok() {
        format!("#page={fragment}")
    } else {
        target.to_owned()
    }
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

fn preview_page_number(path: &Path) -> u32 {
    path.file_stem()
        .and_then(|stem| stem.to_str())
        .and_then(|stem| stem.strip_prefix("page-"))
        .and_then(|number| number.parse().ok())
        .unwrap_or(u32::MAX)
}

fn command_error(command: &str, error: std::io::Error) -> String {
    if error.kind() == std::io::ErrorKind::NotFound {
        match command {
            "typst" => "Typst was not found. Install the `typst` CLI and make sure it is on PATH."
                .to_owned(),
            "pdftoppm" => "Poppler was not found. Install `pdftoppm` (usually provided by the `poppler` package) and make sure it is on PATH."
                .to_owned(),
            _ => format!("Could not start `{command}`: {error}"),
        }
    } else {
        format!("Could not start `{command}`: {error}")
    }
}

fn typst_command_error(program: &Path, error: std::io::Error) -> String {
    format!(
        "Could not start Typst at {}: {error}. Choose a working binary in Settings.",
        program.display()
    )
}

fn send_result(results: &Sender<CompileResult>, context: &egui::Context, result: CompileResult) {
    if results.send(result).is_ok() {
        context.request_repaint();
    }
}

#[cfg(test)]
mod tests {
    use super::{
        CompileRequest, CompileResult, Compiler, CompilerCommand, WatchContext, WatchLine,
        classify_watch_line, parse_pdf_links, preview_page_number, worker_loop,
    };
    use std::{
        fs,
        path::{Path, PathBuf},
        sync::{
            Arc,
            atomic::{AtomicBool, AtomicU64},
            mpsc,
        },
        time::{Duration, Instant},
    };

    #[test]
    fn preview_pages_sort_numerically() {
        assert_eq!(preview_page_number(Path::new("page-2.png")), 2);
        assert_eq!(preview_page_number(Path::new("page-11.png")), 11);
        assert_eq!(preview_page_number(Path::new("preview.pdf")), u32::MAX);
    }

    #[test]
    fn poppler_link_rectangles_are_normalized_and_internal_pages_are_preserved() {
        let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<pdf2xml>
  <page number="1" top="0" left="0" height="200" width="100">
    <text top="40" left="10" width="30" height="20"><a href="https://example.com/?a=1&amp;b=2">External</a></text>
    <text top="80" left="10" width="30" height="20"><a href="links.html#2">Internal</a></text>
  </page>
  <page number="2" top="0" left="0" height="200" width="100" />
</pdf2xml>"#;
        let pages = parse_pdf_links(xml, "links").unwrap();

        assert_eq!(pages.len(), 2);
        assert_eq!(pages[0][0].rect, [0.1, 0.2, 0.4, 0.3]);
        assert_eq!(pages[0][0].target, "https://example.com/?a=1&b=2");
        assert_eq!(pages[0][1].target, "#page=2");
        assert!(pages[1].is_empty());
    }

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
        let compiler = Compiler::new(eframe::egui::Context::default());
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
            let (request_tx, request_rx) = mpsc::channel();
            let (result_tx, result_rx) = mpsc::channel();
            for revision in commands {
                let command = revision.map_or(CompilerCommand::Pause, |revision| {
                    CompilerCommand::Request(CompileRequest {
                        revision,
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
                eframe::egui::Context::default(),
                Arc::new(AtomicBool::new(false)),
                Arc::new(AtomicU64::new(0)),
            );
            let results = result_rx.try_iter().collect::<Vec<_>>();
            assert_eq!(results.len(), usize::from(expected_revision.is_some()));
            if let Some(revision) = expected_revision {
                assert_eq!(results[0].revision, revision);
                assert!(matches!(results[0].output, Some(Err(_))));
            }
        }
    }

    #[test]
    fn changing_typst_executable_invalidates_the_watch_session() {
        let root = tempfile::tempdir().unwrap();
        let source = root.path().join("source");
        std::fs::create_dir(&source).unwrap();
        let mut request = CompileRequest {
            revision: 1,
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
        let compiler = Compiler::new(eframe::egui::Context::default());
        let typst_executable = std::env::var_os("TIPTOPTYP_TEST_TYPST")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("typst"));
        let request = |revision, source: &str| CompileRequest {
            revision,
            source: source.to_owned(),
            source_dir: root.path().to_path_buf(),
            project_root: root.path().to_path_buf(),
            display_name: "integration.typ".to_owned(),
            typst_executable: typst_executable.clone(),
            font_paths: Vec::new(),
        };

        compiler.request(request(1, "= First build")).unwrap();
        let first = wait_for_revision(&compiler, 1);
        assert_eq!(first.output.unwrap().unwrap().pages.len(), 1);

        compiler.request(request(2, "#let broken =")).unwrap();
        let error = wait_for_revision(&compiler, 2).output.unwrap().unwrap_err();
        assert!(error.contains("expected expression"), "{error}");

        compiler
            .request(request(3, "= Recovered\n#pagebreak()\n= Page two"))
            .unwrap();
        let recovered = wait_for_revision(&compiler, 3).output.unwrap();
        let recovered = recovered.unwrap();
        assert_eq!(recovered.pages.len(), 2);
        assert!(recovered.pdf.starts_with(b"%PDF"));
    }

    fn wait_for_revision(compiler: &Compiler, revision: u64) -> CompileResult {
        let deadline = Instant::now() + Duration::from_secs(10);
        while Instant::now() < deadline {
            if let Some(result) = compiler.try_recv()
                && result.revision == revision
                && result.output.is_some()
            {
                return result;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        panic!("timed out waiting for watcher revision {revision}");
    }
}
