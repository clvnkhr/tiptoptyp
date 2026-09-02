use std::{
    fs,
    io::{BufRead, BufReader, Read, Write},
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc::{self, Receiver, RecvTimeoutError, Sender},
    },
    thread,
    time::{Duration, Instant},
};

use eframe::egui;

/// Resolution used only by the native recovery viewer. The primary Tinymist
/// viewer is vector-based. 144 DPI keeps the fallback crisp at 100% on a 2×
/// display without making every incremental build excessively expensive.
pub const PREVIEW_DPI: f32 = 144.0;
const WORKER_POLL_INTERVAL: Duration = Duration::from_millis(15);
const WATCH_LOG_QUIET_PERIOD: Duration = Duration::from_millis(40);

#[derive(Debug, Clone)]
pub struct CompileRequest {
    pub revision: u64,
    pub source: String,
    pub source_dir: PathBuf,
    pub project_root: PathBuf,
    pub display_name: String,
    pub typst_executable: PathBuf,
}

#[derive(Debug)]
pub struct PreviewPage {
    pub size: [usize; 2],
    pub rgba: Vec<u8>,
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

pub struct Compiler {
    requests: Option<Sender<CompileRequest>>,
    results: Receiver<CompileResult>,
    worker: Option<thread::JoinHandle<()>>,
    shutdown: Arc<AtomicBool>,
    latest_revision: Arc<AtomicU64>,
}

impl Compiler {
    pub fn new(context: egui::Context) -> Self {
        let (request_tx, request_rx) = mpsc::channel::<CompileRequest>();
        let (result_tx, result_rx) = mpsc::channel::<CompileResult>();
        let shutdown = Arc::new(AtomicBool::new(false));
        let latest_revision = Arc::new(AtomicU64::new(0));
        let worker_shutdown = shutdown.clone();
        let worker_latest_revision = latest_revision.clone();

        let worker = thread::Builder::new()
            .name("mytypst-compiler".to_owned())
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
            .send(request)
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
    source_dir: PathBuf,
    project_root: PathBuf,
    typst_executable: PathBuf,
    shadow: tempfile::TempPath,
    // Kept alive for the lifetime of the watcher. Its PDF is updated in-place.
    _output_dir: tempfile::TempDir,
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
        watch_logs: Sender<WatchLog>,
    ) -> Result<Self, String> {
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
        let shadow_file = tempfile::Builder::new()
            // Some filesystem watcher backends suppress hidden-file events.
            // Keep this private by filtering it from the project tree instead
            // of giving it a leading dot.
            .prefix("mytypst-preview-")
            .suffix(".typ")
            .tempfile_in(&source_dir)
            .map_err(|error| {
                format!(
                    "Could not create a live-preview file in {}: {error}. Save the document in a writable folder.",
                    source_dir.display()
                )
            })?;
        // TempPath keeps cleanup ownership without holding an open descriptor.
        // Updates must retain this inode: Typst 0.15 watches the source file
        // itself on macOS, so atomically replacing it detaches the watcher.
        let shadow = shadow_file.into_temp_path();
        write_shadow(&shadow, &request.source)?;

        let output_dir = tempfile::Builder::new()
            .prefix("mytypst-watch-")
            .tempdir()
            .map_err(|error| format!("Could not create a preview directory: {error}"))?;
        let pdf_path = output_dir.path().join("preview.pdf");

        let mut command = Command::new(&request.typst_executable);
        command.arg("watch").arg("--diagnostic-format").arg("short");
        if std::env::var_os("MYTYPST_IGNORE_SYSTEM_FONTS").is_some() {
            // This only affects watcher startup; subsequent incremental builds
            // reuse the same font book and stay fast.
            command.arg("--ignore-system-fonts");
        }
        let shadow_path: &Path = shadow.as_ref();
        let mut child = command
            .arg("--root")
            .arg(&project_root)
            .arg(shadow_path)
            .arg(&pdf_path)
            .current_dir(&source_dir)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|error| typst_command_error(&request.typst_executable, error))?;

        let stderr = child
            .stderr
            .take()
            .ok_or_else(|| "Could not read output from `typst watch`".to_owned())?;
        let reader = match thread::Builder::new()
            .name(format!("mytypst-watch-log-{id}"))
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
            source_dir,
            project_root,
            typst_executable: request.typst_executable.clone(),
            shadow,
            _output_dir: output_dir,
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
        write_shadow(&self.shadow, &request.source)?;
        self.pending_revision = request.revision;
        self.pending_display_name.clone_from(&request.display_name);
        Ok(())
    }

    fn matches_context(
        &self,
        source_dir: &Path,
        project_root: &Path,
        typst_executable: &Path,
    ) -> bool {
        watch_context_matches(
            &self.source_dir,
            &self.project_root,
            &self.typst_executable,
            source_dir,
            project_root,
            typst_executable,
        )
    }

    fn clean_diagnostic(&self, line: &str) -> String {
        let shadow: &Path = self.shadow.as_ref();
        let shadow_path = shadow.to_string_lossy();
        let shadow_name = shadow
            .file_name()
            .map(|name| name.to_string_lossy())
            .unwrap_or_default();
        line.replace(shadow_path.as_ref(), &self.active_display_name)
            .replace(shadow_name.as_ref(), &self.active_display_name)
    }
}

fn watch_context_matches(
    current_source_dir: &Path,
    current_project_root: &Path,
    current_typst_executable: &Path,
    requested_source_dir: &Path,
    requested_project_root: &Path,
    requested_typst_executable: &Path,
) -> bool {
    requested_source_dir
        .canonicalize()
        .is_ok_and(|source_dir| source_dir == current_source_dir)
        && requested_project_root
            .canonicalize()
            .is_ok_and(|project_root| project_root == current_project_root)
        && requested_typst_executable == current_typst_executable
}

impl Drop for WatchSession {
    fn drop(&mut self) {
        // Both files belong to this session. Killing the child first releases
        // its handles, then NamedTempFile/TempDir remove the private artifacts.
        let _ = self.child.kill();
        let _ = self.child.wait();
        if let Some(reader) = self.reader.take() {
            let _ = reader.join();
        }
    }
}

fn worker_loop(
    requests: Receiver<CompileRequest>,
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

        match requests.recv_timeout(WORKER_POLL_INTERVAL) {
            Ok(mut request) => {
                if std::env::var_os("MYTYPST_TRACE_WATCH").is_some() {
                    eprintln!("mytypst watcher request revision {}", request.revision);
                }
                // Never make the watcher step through obsolete editor snapshots.
                while let Ok(newer) = requests.try_recv() {
                    request = newer;
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

                let reuse = session.as_ref().is_some_and(|current| {
                    current.matches_context(
                        &request.source_dir,
                        &request.project_root,
                        &request.typst_executable,
                    )
                });
                let update_result = if reuse {
                    session.as_mut().unwrap().update(&request)
                } else {
                    session = None;
                    let id = next_session_id;
                    next_session_id = next_session_id.wrapping_add(1);
                    WatchSession::start(id, &request, watch_log_tx.clone()).map(|new_session| {
                        session = Some(new_session);
                    })
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
        if std::env::var_os("MYTYPST_TRACE_WATCH").is_some() {
            eprintln!(
                "mytypst watcher log session {}: {}",
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
    let render_dir = tempfile::Builder::new()
        .prefix("mytypst-pages-")
        .tempdir()
        .map_err(|error| format!("Could not create a page-rendering directory: {error}"))?;
    let snapshot_path = render_dir.path().join("snapshot.pdf");
    fs::write(&snapshot_path, &pdf)
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
        .name("mytypst-pdf-render-log".to_owned())
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
        let cancelled =
            shutdown.load(Ordering::Acquire) || latest_revision.load(Ordering::Acquire) != revision;
        if cancelled {
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

    let mut pages = Vec::with_capacity(page_paths.len());
    for page_path in page_paths {
        if shutdown.load(Ordering::Acquire) || latest_revision.load(Ordering::Acquire) != revision {
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
        });
    }

    Ok(CompiledDocument {
        pdf,
        pages,
        diagnostics,
    })
}

fn write_shadow(shadow: &Path, source: &str) -> Result<(), String> {
    let mut file = fs::OpenOptions::new()
        .write(true)
        .truncate(true)
        .open(shadow)
        .map_err(|error| format!("Could not open the live-preview file: {error}"))?;
    file.write_all(source.as_bytes())
        .map_err(|error| format!("Could not update the live-preview file: {error}"))?;
    file.flush()
        .map_err(|error| format!("Could not flush the live-preview file: {error}"))?;
    file.sync_data()
        .map_err(|error| format!("Could not sync the live-preview file: {error}"))?;
    if std::env::var_os("MYTYPST_TRACE_WATCH").is_some() {
        let metadata = fs::metadata(shadow).ok();
        eprintln!(
            "mytypst wrote {} bytes to {} (modified {:?})",
            source.len(),
            shadow.display(),
            metadata.and_then(|metadata| metadata.modified().ok())
        );
    }
    Ok(())
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
        CompileRequest, CompileResult, Compiler, WatchLine, classify_watch_line,
        preview_page_number, watch_context_matches,
    };
    use std::{
        path::{Path, PathBuf},
        time::{Duration, Instant},
    };

    #[test]
    fn preview_pages_sort_numerically() {
        assert_eq!(preview_page_number(Path::new("page-2.png")), 2);
        assert_eq!(preview_page_number(Path::new("page-11.png")), 11);
        assert_eq!(preview_page_number(Path::new("preview.pdf")), u32::MAX);
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
    fn changing_typst_executable_invalidates_the_watch_session() {
        let root = tempfile::tempdir().unwrap();
        let source = root.path().join("source");
        std::fs::create_dir(&source).unwrap();
        let source = source.canonicalize().unwrap();
        let root = root.path().canonicalize().unwrap();
        let bundled = PathBuf::from("/tools/typst-bundled");
        let custom = PathBuf::from("/tools/typst-custom");

        assert!(watch_context_matches(
            &source, &root, &bundled, &source, &root, &bundled
        ));
        assert!(!watch_context_matches(
            &source, &root, &bundled, &source, &root, &custom
        ));
    }

    #[test]
    #[ignore = "requires typst, pdftoppm, and real filesystem notifications"]
    fn persistent_watcher_compiles_errors_and_recovers() {
        let root = tempfile::tempdir().unwrap();
        let compiler = Compiler::new(eframe::egui::Context::default());
        let typst_executable = std::env::var_os("MYTYPST_TEST_TYPST")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("typst"));
        let request = |revision, source: &str| CompileRequest {
            revision,
            source: source.to_owned(),
            source_dir: root.path().to_path_buf(),
            project_root: root.path().to_path_buf(),
            display_name: "integration.typ".to_owned(),
            typst_executable: typst_executable.clone(),
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
