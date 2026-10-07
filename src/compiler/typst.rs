//! Typst watch adapter. No UI, scheduling policy, or PDF rendering ownership.
mod diagnostics;
use super::{CompileInput, CompileRequest, EngineEvent, EngineResult};
use crate::{
    diagnostics::DiagnosticReport,
    private_workspace::{PrivateSourceMirror, PrivateWorkspace},
    process::finish_reader_with_timeout,
};
use diagnostics::report;
use std::{
    ffi::OsStr,
    fs,
    io::{BufRead, BufReader},
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::{
        LazyLock,
        mpsc::{self, Receiver, Sender},
    },
    thread,
    time::{Duration, Instant},
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TypstOptions {
    pub(crate) command: std::sync::Arc<crate::tool_command::CommandCustomization>,
    pub(crate) executable: PathBuf,
    pub(crate) font_paths: Vec<PathBuf>,
}
const WATCH_LOG_QUIET_PERIOD: Duration = Duration::from_millis(40);
const PIPE_READER_SHUTDOWN_TIMEOUT: Duration = Duration::from_millis(250);
const IGNORE_SYSTEM_FONTS_ENV: &str = "TIPTOPTYP_IGNORE_SYSTEM_FONTS";
const TRACE_WATCH_ENV: &str = "TIPTOPTYP_TRACE_WATCH";
static IGNORE_SYSTEM_FONTS: LazyLock<bool> =
    LazyLock::new(|| std::env::var_os(IGNORE_SYSTEM_FONTS_ENV).is_some());
static TRACE_WATCH: LazyLock<bool> = LazyLock::new(|| std::env::var_os(TRACE_WATCH_ENV).is_some());

#[derive(Debug, Clone, PartialEq, Eq)]
struct WatchContext {
    command: std::sync::Arc<crate::tool_command::CommandCustomization>,
    source_dir: PathBuf,
    project_root: PathBuf,
    working_directory: PathBuf,
    typst_executable: PathBuf,
    font_paths: Vec<PathBuf>,
    display_name: String,
}

impl WatchContext {
    fn resolve(request: &CompileInput, options: &TypstOptions) -> Result<Self, String> {
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
        let working_directory = if options.command.directory.trim().is_empty() {
            source_dir.clone()
        } else {
            Path::new(&options.command.directory)
                .canonicalize()
                .map_err(|error| {
                    format!(
                        "Could not use Typst working directory {}: {error}",
                        options.command.directory
                    )
                })?
        };
        let mut font_paths = options
            .font_paths
            .iter()
            .filter_map(|path| path.canonicalize().ok())
            .filter(|path| path.starts_with(&project_root))
            .collect::<Vec<_>>();
        font_paths.sort();
        font_paths.dedup();
        Ok(Self {
            command: options.command.clone(),
            source_dir,
            project_root,
            working_directory,
            typst_executable: options.executable.clone(),
            font_paths,
            display_name: request.display_name.clone(),
        })
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
    shadow: PrivateSourceMirror,
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
            .mirrored_source(
                &context.source_dir,
                &context.display_name,
                &request.input.source,
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
        command
            .arg("--root")
            .arg(&context.project_root)
            .arg(shadow_path)
            .arg(&pdf_path)
            .current_dir(&context.source_dir);
        context
            .command
            .apply(&mut command)
            .map_err(|e| e.to_string())?;
        let mut child = command
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
            pending_display_name: request.input.display_name.clone(),
            active_revision: None,
            active_display_name: request.input.display_name.clone(),
            active_started: Instant::now(),
            diagnostics: Vec::new(),
            pending_completion: None,
        })
    }

    fn update(&mut self, request: &CompileRequest) -> Result<(), String> {
        self.shadow.update(&request.input.source).map_err(|error| {
            format!("Could not update the private live-preview document: {error}")
        })?;
        self.pending_revision = request.revision;
        self.pending_display_name
            .clone_from(&request.input.display_name);
        Ok(())
    }

    fn matches_context(&self, context: &WatchContext) -> bool {
        self.context == *context
            && self.shadow.path().file_name() == Some(OsStr::new(&context.display_name))
    }

    fn diagnostic_report(&self, raw: String) -> DiagnosticReport {
        use crate::diagnostics::{DiagnosticSource, normalize_diagnostics};
        let mut report = report(raw, Some(Path::new(&self.active_display_name)));
        let main = self.context.source_dir.join(&self.active_display_name);
        let cwd = &self.context.working_directory;
        for diagnostic in &mut report.diagnostics {
            let DiagnosticSource::File(path) = &diagnostic.source else {
                continue;
            };
            // Typst prints paths relative to its effective working directory,
            // even though watch received an absolute shadow path. A wrapper
            // may instead report root-relative or absolute paths.
            let original = [cwd.join(path), self.context.project_root.join(path)]
                .iter()
                .find_map(|candidate| {
                    crate::private_workspace::original_mirrored_source(
                        &self.context.project_root,
                        candidate,
                    )
                });
            if let Some(original) = original {
                diagnostic.source = if original == main {
                    DiagnosticSource::Main
                } else {
                    DiagnosticSource::File(original)
                };
            }
        }
        normalize_diagnostics(&mut report.diagnostics);
        report
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

pub(super) struct Backend {
    session: Option<WatchSession>,
    logs_tx: Sender<WatchLog>,
    logs_rx: Receiver<WatchLog>,
    next_session_id: u64,
}
impl Backend {
    pub(super) fn new() -> Self {
        let (logs_tx, logs_rx) = mpsc::channel();
        Self {
            session: None,
            logs_tx,
            logs_rx,
            next_session_id: 1,
        }
    }
    pub(super) fn is_running(&self) -> bool {
        self.session.is_some()
    }
    pub(super) fn submit(
        &mut self,
        request: &CompileRequest,
        options: &TypstOptions,
    ) -> Result<(), String> {
        if *TRACE_WATCH {
            eprintln!("tiptoptyp watcher request revision {}", request.revision);
        }
        let context = WatchContext::resolve(&request.input, options)?;
        // macOS suppresses updates below dot-directories. Its fresh watcher
        // initial build is deterministic; other platforms retain watch reuse.
        let reuse = !cfg!(target_os = "macos")
            && self
                .session
                .as_ref()
                .is_some_and(|current| current.matches_context(&context));
        if let (true, Some(current)) = (reuse, self.session.as_mut()) {
            current.update(request)
        } else {
            self.session = None;
            let id = self.next_session_id;
            self.next_session_id = self.next_session_id.wrapping_add(1);
            self.session = Some(WatchSession::start(
                id,
                request,
                context,
                self.logs_tx.clone(),
            )?);
            Ok(())
        }
    }
    pub(super) fn poll(&mut self) -> Vec<EngineResult> {
        let mut results = Vec::new();
        drain_watch_logs(&self.logs_rx, &mut self.session, &mut results);
        finish_settled_completion(&mut self.session, &mut results);
        let exited = self.session.as_mut().and_then(|current| {
            current
                .child
                .try_wait()
                .ok()
                .flatten()
                .map(|status| (current.pending_revision, status))
        });
        if let Some((revision, status)) = exited {
            results.push(EngineResult {
                revision,
                elapsed: Duration::ZERO,
                event: EngineEvent::Failed(DiagnosticReport::error(format!(
                    "`typst watch` stopped unexpectedly with {status}"
                ))),
            });
            self.session = None;
        }
        results
    }
}

fn drain_watch_logs(
    logs: &Receiver<WatchLog>,
    session: &mut Option<WatchSession>,
    results: &mut Vec<EngineResult>,
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
                results.push(EngineResult {
                    revision: current.pending_revision,
                    elapsed: Duration::ZERO,
                    event: EngineEvent::Started,
                });
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
                current.diagnostics.push(log.line);
            }
        }
    }
}

fn finish_settled_completion(session: &mut Option<WatchSession>, results: &mut Vec<EngineResult>) {
    let Some(current) = session.as_mut() else {
        return;
    };
    if !current
        .pending_completion
        .as_ref()
        .is_some_and(|completion| completion.quiet_since.elapsed() >= WATCH_LOG_QUIET_PERIOD)
    {
        return;
    }
    let completion = current.pending_completion.take().unwrap();
    let raw = current.diagnostics.join("\n");
    let diagnostics = current.diagnostic_report(raw);
    let event = if completion.succeeded {
        // Snapshot while this session still owns its output. An update/drop may
        // remove the private workspace before publication or inspection finishes.
        match fs::read(&current.pdf_path) {
            Ok(pdf) => EngineEvent::Pdf {
                synctex: None,
                pdf: pdf.into(),
                diagnostics,
            },
            Err(error) => EngineEvent::Failed(DiagnosticReport::error(format!(
                "Could not snapshot the compiled PDF: {error}"
            ))),
        }
    } else {
        EngineEvent::Failed(if diagnostics.raw.is_empty() {
            DiagnosticReport::error("Typst could not compile the document".to_owned())
        } else {
            diagnostics
        })
    };
    results.push(EngineResult {
        revision: completion.revision,
        elapsed: completion.elapsed,
        event,
    });
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

#[cfg(test)]
mod tests {
    use super::*;
    use tiptoptyp_core::document::TypesettingLanguage;

    #[cfg(unix)]
    #[test]
    fn watch_diagnostics_map_relative_generated_main_and_included_same_basename() {
        use crate::diagnostics::{DiagnosticLocation, DiagnosticSource};
        use std::os::unix::fs::PermissionsExt;
        let root = tempfile::tempdir().unwrap();
        let source_dir = root.path().join("chapters");
        fs::create_dir_all(source_dir.join("other")).unwrap();
        fs::write(source_dir.join("other/main.typ"), "included source").unwrap();
        let source_dir = source_dir.canonicalize().unwrap();
        let script = root.path().join("fake-typst");
        fs::write(&script, concat!(
            "#!/bin/sh\n",
            "root=''; last=''; previous=''; for arg do if [ \"$last\" = --root ]; then root=$arg; fi; previous=$last; last=$arg; done\n",
            "source=$previous; relative=${source#\"$root/\"}\n",
            "printf 'compiling ...\\n%s:4:7: error: main fixture\\n%s:2:5: warning: included fixture\\n' \"$relative\" \"${relative%/*}/other/main.typ\" >&2\n",
            "printf '%s:4:7: error: main fixture\\n%s:2:5: warning: included fixture\\n' \"../$relative\" \"../${relative%/*}/other/main.typ\" >&2\n",
            "printf '%s:4:7: error: main fixture\\n%s:2:5: warning: included fixture\\ncompiled with errors\\n' \"$source\" \"${source%/*}/other/main.typ\" >&2\n",
            "exec sleep 60\n",
        )).unwrap();
        fs::set_permissions(&script, fs::Permissions::from_mode(0o700)).unwrap();
        // The shell's actual cwd is the target, not the spelling of this
        // symlink outside the project. Cwd-relative diagnostics must use it.
        let other_root = tempfile::tempdir().unwrap();
        let cwd_alias = other_root.path().join("cwd");
        std::os::unix::fs::symlink(&source_dir, &cwd_alias).unwrap();
        let options = TypstOptions {
            command: std::sync::Arc::new(crate::tool_command::CommandCustomization {
                directory: cwd_alias.display().to_string(),
                ..Default::default()
            }),
            executable: script,
            font_paths: Vec::new(),
        };
        let request = CompileRequest {
            revision: 7,
            input: CompileInput {
                language: TypesettingLanguage::Typst,
                source: "#unknown".into(),
                source_dir: source_dir.clone(),
                project_root: root.path().into(),
                display_name: "main.typ".into(),
            },
            engine: super::super::EngineConfig::Typst(options.clone()),
        };
        let mut backend = Backend::new();
        backend.submit(&request, &options).unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        let report = loop {
            let ready = backend
                .poll()
                .into_iter()
                .find_map(|result| match result.event {
                    EngineEvent::Failed(report) => Some(report),
                    EngineEvent::Pdf { .. } => panic!("fixture unexpectedly compiled"),
                    EngineEvent::Started => None,
                });
            if let Some(report) = ready {
                break report;
            }
            assert!(
                Instant::now() < deadline,
                "fixture did not publish diagnostics"
            );
            thread::sleep(Duration::from_millis(10));
        };
        assert_eq!(
            report.diagnostics.len(),
            2,
            "absolute, cwd-relative, and root-relative repetitions map to the same source identities"
        );
        assert_eq!(
            report.diagnostics[0].source,
            DiagnosticSource::Main,
            "generated main must map to the editor document"
        );
        assert_eq!(
            report.diagnostics[0].location,
            Some(DiagnosticLocation { line: 4, column: 7 })
        );
        assert_eq!(
            report.diagnostics[1].source,
            DiagnosticSource::File(source_dir.join("other/main.typ")),
            "included files with the same basename remain distinct"
        );
    }

    #[cfg(unix)]
    #[test]
    fn output_survives_session_cleanup_and_late_watcher_messages_are_ignored() {
        use std::os::unix::fs::PermissionsExt;
        let root = tempfile::tempdir().unwrap();
        let script = root.path().join("fake-typst");
        fs::write(&script, concat!(
            "#!/bin/sh\n",
            "for arg do output=$arg; done\n",
            "printf '%s' '%PDF-frozen-bytes' > \"$output\"\n",
            "printf 'compiling ...\\npaper.typ:1:2: warning: fixture warning\\ncompiled successfully\\n' >&2\n",
            "exec sleep 60\n",
        )).unwrap();
        fs::set_permissions(&script, fs::Permissions::from_mode(0o700)).unwrap();
        let options = TypstOptions {
            command: Default::default(),
            executable: script,
            font_paths: Vec::new(),
        };
        let request = CompileRequest {
            revision: 7,
            input: CompileInput {
                language: TypesettingLanguage::Typst,
                source: "= Test".to_owned(),
                source_dir: root.path().to_owned(),
                project_root: root.path().to_owned(),
                display_name: "paper.typ".to_owned(),
            },
            engine: super::super::EngineConfig::Typst(options.clone()),
        };
        let mut backend = Backend::new();
        backend.submit(&request, &options).unwrap();
        let session = backend.session.as_ref().unwrap();
        let private_dir = session.shadow.session_dir().to_owned();
        let pdf_path = session.pdf_path.clone();
        let id = session.id;
        let deadline = Instant::now() + Duration::from_secs(5);
        let pdf = loop {
            let mut ready = None;
            for result in backend.poll() {
                match result.event {
                    EngineEvent::Pdf {
                        pdf, diagnostics, ..
                    } => {
                        assert_eq!(
                            diagnostics.diagnostics[0].source,
                            crate::diagnostics::DiagnosticSource::Main
                        );
                        assert_eq!(diagnostics.diagnostics[0].location.unwrap().column, 2);
                        assert_eq!(diagnostics.diagnostics[0].message, "fixture warning");
                        ready = Some(pdf);
                    }
                    EngineEvent::Failed(report) => panic!("unexpected failure: {report:?}"),
                    EngineEvent::Started => {}
                }
            }
            if let Some(pdf) = ready {
                break pdf;
            }
            assert!(Instant::now() < deadline, "fixture did not compile");
            thread::sleep(Duration::from_millis(10));
        };
        for line in ["compiling ...", "error: stale", "compiled with errors"] {
            backend
                .logs_tx
                .send(WatchLog {
                    session_id: id.wrapping_sub(1),
                    line: line.to_owned(),
                })
                .unwrap();
        }
        assert!(backend.poll().is_empty());
        assert!(
            backend
                .session
                .as_ref()
                .unwrap()
                .pending_completion
                .is_none()
        );
        fs::write(pdf_path, b"%PDF-new-output").unwrap();
        drop(backend);
        assert!(
            !private_dir.exists(),
            "session cleanup must remove its private mirror"
        );
        assert_eq!(pdf.as_ref(), b"%PDF-frozen-bytes");
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
        let request = CompileInput {
            language: TypesettingLanguage::Typst,
            source: String::new(),
            source_dir: source,
            project_root: root.path().to_path_buf(),
            display_name: "main.typ".to_owned(),
        };
        let mut options = TypstOptions {
            command: Default::default(),
            executable: PathBuf::from("/tools/typst-bundled"),
            font_paths: Vec::new(),
        };
        let context = WatchContext::resolve(&request, &options).unwrap();
        assert_eq!(WatchContext::resolve(&request, &options).unwrap(), context);
        options.executable = PathBuf::from("/tools/typst-custom");
        assert_ne!(WatchContext::resolve(&request, &options).unwrap(), context);
    }

    #[test]
    fn watch_context_keeps_only_canonical_project_font_directories() {
        let project = tempfile::tempdir().unwrap();
        let fonts = project.path().join("assets/fonts");
        fs::create_dir_all(&fonts).unwrap();
        let outside = tempfile::tempdir().unwrap();
        let request = CompileInput {
            language: TypesettingLanguage::Typst,
            source: String::new(),
            source_dir: project.path().to_path_buf(),
            project_root: project.path().to_path_buf(),
            display_name: "main.typ".to_owned(),
        };
        let options = TypstOptions {
            command: Default::default(),
            executable: PathBuf::from("typst"),
            font_paths: vec![
                fonts.clone(),
                fonts.clone(),
                outside.path().to_path_buf(),
                project.path().join("missing"),
            ],
        };
        let context = WatchContext::resolve(&request, &options).unwrap();
        assert_eq!(context.font_paths, vec![fonts.canonicalize().unwrap()]);
    }

    #[cfg(unix)]
    #[test]
    fn watch_context_resolves_custom_working_directory_aliases() {
        let project = tempfile::tempdir().unwrap();
        let source = project.path().join("source");
        fs::create_dir(&source).unwrap();
        let outside = tempfile::tempdir().unwrap();
        let alias = outside.path().join("cwd");
        std::os::unix::fs::symlink(&source, &alias).unwrap();
        let request = CompileInput {
            language: TypesettingLanguage::Typst,
            source: String::new(),
            source_dir: source.clone(),
            project_root: project.path().into(),
            display_name: "main.typ".into(),
        };
        let mut options = TypstOptions {
            command: std::sync::Arc::new(crate::tool_command::CommandCustomization {
                directory: alias.display().to_string(),
                ..Default::default()
            }),
            executable: "typst".into(),
            font_paths: Vec::new(),
        };
        let context = WatchContext::resolve(&request, &options).unwrap();
        assert_eq!(context.working_directory, source.canonicalize().unwrap());
        assert_eq!(
            context.command.directory,
            alias.display().to_string(),
            "keep the user's customization unchanged"
        );
        std::sync::Arc::make_mut(&mut options.command).directory =
            outside.path().join("missing").display().to_string();
        assert!(WatchContext::resolve(&request, &options).is_err());
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
        let mut request = CompileInput {
            language: TypesettingLanguage::Typst,
            source: String::new(),
            source_dir: source.canonicalize().unwrap(),
            project_root: project.path().canonicalize().unwrap(),
            display_name: "main.typ".to_owned(),
        };
        let options = TypstOptions {
            command: Default::default(),
            executable: PathBuf::from("/tools/typst"),
            font_paths: Vec::new(),
        };
        let context = WatchContext::resolve(&request, &options).unwrap();
        request.source_dir = alias;
        request.project_root = project.path().to_path_buf();
        assert_eq!(WatchContext::resolve(&request, &options).unwrap(), context);
        request.source_dir = project.path().join("missing");
        assert!(WatchContext::resolve(&request, &options).is_err());
    }
}
