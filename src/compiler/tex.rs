//! Finite TeX builds; the shared compiler owns publication and PDF inspection.
use super::{CompileRequest, EngineEvent, EngineResult};
use crate::{
    diagnostics::{
        Diagnostic, DiagnosticLocation, DiagnosticReport, DiagnosticSeverity, DiagnosticSource,
    },
    private_workspace::{PrivateSourceMirror, PrivateWorkspace},
};
use std::{
    fs::{self, File},
    io::Read,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::Arc,
    time::{Duration, Instant},
};

const MAX_LOG: u64 = 4 * 1024 * 1024;
const MAX_PDF: u64 = 256 * 1024 * 1024;
// First-use package and format downloads can take several minutes. New edits
// still cancel immediately through the worker; this bounds a stalled build.
const BUILD_TIMEOUT: Duration = Duration::from_secs(600);

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TexOptions {
    pub(crate) engine: crate::tex::settings::BuildEngine,
    pub(crate) command: std::sync::Arc<crate::tool_command::CommandCustomization>,
    pub(crate) executable: PathBuf,
    pub(crate) only_cached: bool,
    pub(crate) synctex: bool,
    pub(crate) show_underfull_box_warnings: bool,
    pub(crate) system: Option<Box<crate::tex::tools::SystemBuildOptions>>,
}

struct Session {
    // Stop the process before its mirror and output are removed.
    child: crate::process::OwnedChild,
    mirror: PrivateSourceMirror,
    _cache_lease: Option<std::sync::RwLockReadGuard<'static, ()>>,
    log: PathBuf,
    pdf: PathBuf,
    revision: u64,
    root: PathBuf,
    source: String,
    started: Instant,
    command: Command,
    pass: u8,
    standard: bool,
    latexmk: bool,
    engine: crate::tex::settings::BuildEngine,
    synctex: bool,
    show_underfull_box_warnings: bool,
}

pub(super) struct Backend {
    session: Option<Session>,
    started: bool,
}
impl Backend {
    pub(super) fn new() -> Self {
        Self {
            session: None,
            started: false,
        }
    }
    pub(super) fn is_running(&self) -> bool {
        self.session.is_some()
    }
    pub(super) fn submit(
        &mut self,
        request: &CompileRequest,
        options: &TexOptions,
    ) -> Result<(), String> {
        self.session = None;
        self.started = false;
        self.session = Some(
            Session::start(request, options)
                .map_err(|e| format!("Could not start TeX build: {e}"))?,
        );
        self.started = true;
        Ok(())
    }
    pub(super) fn poll(&mut self) -> Vec<EngineResult> {
        let Some(session) = self.session.as_mut() else {
            return Vec::new();
        };
        if std::mem::take(&mut self.started) {
            return vec![EngineResult {
                revision: session.revision,
                elapsed: Duration::ZERO,
                event: EngineEvent::Started,
            }];
        }
        let status = match session.child.try_wait() {
            Ok(None)
                if session.started.elapsed() < BUILD_TIMEOUT
                    && fs::metadata(&session.log).is_ok_and(|m| m.len() <= MAX_LOG) =>
            {
                return Vec::new();
            }
            Ok(Some(status)) if fs::metadata(&session.log).is_ok_and(|m| m.len() <= MAX_LOG) => {
                Ok(status)
            }
            Ok(Some(_)) => Err("TeX build exceeded the log limit".to_owned()),
            Ok(None) => Err("TeX build exceeded the build time or log limit".to_owned()),
            Err(error) => Err(format!("Could not wait for TeX build: {error}")),
        };
        if status.as_ref().is_ok_and(|status| status.success())
            && session.standard
            && session.pass < 3
        {
            let log = read_limited(&session.log, MAX_LOG).unwrap_or_default();
            if session.pass == 1
                || log.contains("Rerun to get")
                || log.contains("Label(s) may have changed")
            {
                match crate::process::OwnedChild::spawn(&mut session.command) {
                    Ok(child) => {
                        session.child = child;
                        session.pass += 1;
                        return Vec::new();
                    }
                    Err(error) => {
                        let result = EngineResult {
                            revision: session.revision,
                            elapsed: session.started.elapsed(),
                            event: EngineEvent::Failed(DiagnosticReport::error(error.to_string())),
                        };
                        self.session = None;
                        return vec![result];
                    }
                }
            }
        }
        let session = self.session.take().unwrap();
        let console = read_limited(&session.log, MAX_LOG).unwrap_or_default();
        let final_log = session
            .latexmk
            .then(|| read_limited(&session.pdf.with_extension("log"), MAX_LOG).ok())
            .flatten();
        let mut report = diagnostics(
            final_log.as_deref().unwrap_or(&console),
            session.mirror.path(),
            &session.source,
        );
        report.raw = console;
        if let Some(log) = final_log.as_deref() {
            append_raw_log(&mut report.raw, "Final TeX log", log);
        }
        if session.latexmk {
            let bibliography =
                read_limited(&session.pdf.with_extension("blg"), MAX_LOG).unwrap_or_default();
            report
                .diagnostics
                .extend(bibliography_diagnostics(&bibliography));
            append_raw_log(&mut report.raw, "Bibliography log", &bibliography);
        }
        report.diagnostics.retain(|diagnostic| {
            compiler_diagnostic_visible(diagnostic, session.show_underfull_box_warnings)
        });
        for diagnostic in &mut report.diagnostics {
            if diagnostic.provider.as_deref() == Some("Tectonic") {
                diagnostic.provider = Some(session.engine.label().into());
            }
            if let DiagnosticSource::File(path) = &mut diagnostic.source {
                let candidate = if path.is_absolute() {
                    path.clone()
                } else {
                    session.mirror.path().parent().unwrap().join(&*path)
                };
                let candidate = candidate.canonicalize().unwrap_or(candidate);
                if candidate == session.mirror.path() {
                    diagnostic.source = DiagnosticSource::Main;
                } else {
                    *path = candidate
                        .strip_prefix(session.mirror.mirror_root())
                        .map(|relative| session.root.join(relative))
                        .unwrap_or(candidate);
                }
            }
        }
        let event = match status {
            Ok(status) if status.success() => match read_pdf(&session.pdf) {
                Ok(pdf) => EngineEvent::Pdf {
                    synctex: session
                        .synctex
                        .then(|| {
                            [true, false].into_iter().find_map(|compressed| {
                                let path = session.pdf.with_extension(if compressed {
                                    "synctex.gz"
                                } else {
                                    "synctex"
                                });
                                let metadata = fs::metadata(&path).ok()?;
                                if metadata.len() > 64 * 1024 * 1024 {
                                    return None;
                                }
                                Some(Arc::new(crate::synctex::Artifact {
                                    data: fs::read(path).ok()?.into(),
                                    compressed,
                                    root: session.root.clone(),
                                    mirror: session.mirror.mirror_root().to_owned(),
                                }))
                            })
                        })
                        .flatten(),
                    pdf,
                    diagnostics: report,
                },
                Err(error) => EngineEvent::Failed(DiagnosticReport::error(error)),
            },
            status => {
                if !report
                    .diagnostics
                    .iter()
                    .any(|d| d.severity == DiagnosticSeverity::Error)
                {
                    let message =
                        status.map_or_else(|e| e, |s| format!("TeX build exited with {s}"));
                    report
                        .diagnostics
                        .extend(DiagnosticReport::error(message).diagnostics);
                }
                EngineEvent::Failed(report)
            }
        };
        vec![EngineResult {
            revision: session.revision,
            elapsed: session.started.elapsed(),
            event,
        }]
    }
}
impl Session {
    fn start(request: &CompileRequest, options: &TexOptions) -> std::io::Result<Self> {
        let standard_engine = options.engine != crate::tex::settings::BuildEngine::Tectonic;
        if !standard_engine {
            options
                .command
                .validate_for_tool(crate::toolchain::ToolKind::Tectonic)
                .map_err(std::io::Error::other)?;
        }
        let cache_lease = standard_engine
            .then(crate::tex::tools::acquire_build_cache)
            .transpose()?;
        let private = PrivateWorkspace::open(&request.input.project_root)?;
        let mirror = private.mirrored_source(
            &request.input.source_dir,
            &request.input.display_name,
            &request.input.source,
        )?;
        let output = mirror.session_dir().join("output");
        fs::create_dir(&output)?;
        let log = mirror.session_dir().join("tex-output.log");
        let logs = File::create(&log)?;
        let pdf = output
            .join(mirror.path().file_name().unwrap())
            .with_extension("pdf");
        let latexmk = standard_engine
            && options
                .system
                .as_ref()
                .is_some_and(|system| system.driver == crate::tex::settings::BuildDriver::Latexmk);
        let mut command = if latexmk {
            latexmk_command(options, mirror.path(), &output)?
        } else {
            let mut command = Command::new(&options.executable);
            if standard_engine {
                crate::tex::settings::validate_engine_arguments(&options.command.arguments)
                    .map_err(std::io::Error::other)?;
                command
                    .args([
                        "-interaction=nonstopmode",
                        "-halt-on-error",
                        "-file-line-error",
                        "-no-shell-escape",
                    ])
                    .arg(format!("-output-directory={}", output.display()));
                if options.synctex {
                    command.arg("-synctex=1");
                }
            } else {
                command
                    .args(options.synctex.then_some("--synctex"))
                    .arg("--outdir")
                    .arg(&output);
                if options.only_cached {
                    command.arg("--only-cached");
                }
            }
            command
                .arg(mirror.path())
                .current_dir(mirror.path().parent().unwrap())
                .env("NO_COLOR", "1")
                .env("LC_ALL", "C");
            options.command.apply(&mut command)?;
            command
        };
        if standard_engine {
            let paths = options
                .executable
                .parent()
                .into_iter()
                .map(Path::to_path_buf)
                .chain(
                    std::env::var_os("PATH")
                        .into_iter()
                        .flat_map(|path| std::env::split_paths(&path).collect::<Vec<_>>()),
                );
            if let Ok(paths) = std::env::join_paths(paths) {
                // Respect an explicitly customized PATH. Otherwise include the
                // discovered TeX bin for xdvipdfmx, kpsewhich and makeindex.
                if !command.get_envs().any(|(name, _)| name == "PATH") {
                    command.env("PATH", paths);
                }
            }
        }
        command
            .stdin(Stdio::null())
            .stdout(logs.try_clone()?)
            .stderr(logs);
        let child = crate::process::OwnedChild::spawn(&mut command)?;
        Ok(Self {
            child,
            command,
            pass: 1,
            standard: standard_engine && !latexmk,
            latexmk,
            engine: options.engine,
            synctex: options.synctex,
            show_underfull_box_warnings: options.show_underfull_box_warnings,
            mirror,
            _cache_lease: cache_lease,
            log,
            pdf,
            revision: request.revision,
            root: private.project_root().to_owned(),
            source: request.input.source.clone(),
            started: Instant::now(),
        })
    }
}
/// latexmk owns only the finite dependency/rerun graph; the editor retains
/// lifecycle, output locations, source overlays, cancellation and publication.
fn latexmk_command(options: &TexOptions, entry: &Path, output: &Path) -> std::io::Result<Command> {
    use crate::tex::settings::{BuildEngine, SystemTool};
    let invalid = std::io::Error::other;
    let system = options
        .system
        .as_ref()
        .ok_or_else(|| invalid("Missing system build settings"))?;
    let driver = system.latexmk.as_ref().ok_or_else(|| invalid("latexmk was not found. Install latexmk with your TeX distribution, refresh tools, or choose Direct engine in Settings"))?;
    SystemTool {
        executable: driver.program.to_string_lossy().into_owned(),
        arguments: driver.arguments.clone(),
    }
    .validate("latexmk")
    .map_err(std::io::Error::other)?;
    crate::tex::settings::validate_engine_arguments(&options.command.arguments)
        .map_err(std::io::Error::other)?;
    let mut command = Command::new(&driver.program);
    let mode = match options.engine {
        BuildEngine::PdfLatex => ("-pdf", "pdflatex"),
        BuildEngine::XeLatex => ("-pdfxe", "xelatex"),
        BuildEngine::LuaLatex => ("-pdflua", "lualatex"),
        BuildEngine::Tectonic => return Err(invalid("latexmk does not drive Tectonic")),
    };
    let engine = literal_tool_command(&options.executable, &options.command.arguments)?;
    let bibliography = [(&system.bibtex, "bibtex"), (&system.biber, "biber")]
        .into_iter()
        .map(|(tool, name)| {
            SystemTool {
                executable: tool.program.to_string_lossy().into_owned(),
                arguments: tool.arguments.clone(),
            }
            .validate(if name == "biber" { "Biber" } else { "BibTeX" })
            .map_err(std::io::Error::other)?;
            let text = literal_tool_command(&tool.program, &tool.arguments)?;
            Ok(format!(
                "${name} = {};",
                perl_string(&format!("{text} %O %S"))
            ))
        })
        .collect::<std::io::Result<Vec<_>>>()?
        .join(" ");
    command.args(["-norc", "-pvc-", "-pv-", "-view=none", "-f-", mode.0])
        .arg(format!("-outdir={}", output.display()))
        .arg(format!("-auxdir={}", output.display()))
        .arg("-e").arg(format!("$max_repeat = 5; ${} = {}; {bibliography}", mode.1, perl_string(&format!("{engine} -interaction=nonstopmode -halt-on-error -file-line-error -no-shell-escape %O %S"))));
    if options.synctex {
        command.arg("-synctex=1");
    }
    if !system.bibliography_enabled {
        command.arg("-bibtex-");
    }
    command
        .arg(entry)
        .current_dir(entry.parent().unwrap())
        .env("NO_COLOR", "1")
        .env("LC_ALL", "C");
    // These environment and working-directory edits apply to the finite build
    // and its children; engine argv is already in its quoted command above.
    let mut custom = options.command.as_ref().clone();
    custom.arguments = driver.arguments.clone();
    custom.apply(&mut command)?;
    Ok(command)
}

fn literal_tool_command(program: &Path, arguments: &str) -> std::io::Result<String> {
    let invalid = std::io::Error::other;
    let program = program
        .to_str()
        .ok_or_else(|| invalid("latexmk tool paths must be Unicode"))?;
    std::iter::once(program.to_owned())
        .chain(
            shlex::split(arguments)
                .ok_or_else(|| invalid("Tool arguments contain an unmatched quote"))?,
        )
        .map(|token| {
            // latexmk substitutes percent markers before invoking the command.
            // Double literal percent signs before adding its own %O/%S fields.
            shlex::try_quote(&token.replace('%', "%%"))
                .map(|v| v.into_owned())
                .map_err(std::io::Error::other)
        })
        .collect::<std::io::Result<Vec<_>>>()
        .map(|tokens| tokens.join(" "))
}
fn perl_string(text: &str) -> String {
    format!("'{}'", text.replace('\\', "\\\\").replace('\'', "\\'"))
}

fn read_limited(path: &Path, limit: u64) -> std::io::Result<String> {
    let mut text = String::new();
    File::open(path)?.take(limit).read_to_string(&mut text)?;
    Ok(text)
}
fn read_pdf(path: &Path) -> Result<Arc<[u8]>, String> {
    let mut bytes = Vec::new();
    File::open(path)
        .and_then(|f| f.take(MAX_PDF + 1).read_to_end(&mut bytes))
        .map_err(|e| format!("TeX build produced no readable PDF: {e}"))?;
    if bytes.len() as u64 > MAX_PDF || !bytes.starts_with(b"%PDF-") {
        return Err("TeX build produced an invalid or oversized PDF".into());
    }
    Ok(bytes.into())
}

fn append_raw_log(raw: &mut String, label: &str, log: &str) {
    if !log.is_empty() {
        raw.push_str("\n--- ");
        raw.push_str(label);
        raw.push_str(" ---\n");
        raw.push_str(log);
    }
}

fn compiler_diagnostic_visible(diagnostic: &Diagnostic, show_underfull: bool) -> bool {
    show_underfull
        || diagnostic.severity != DiagnosticSeverity::Warning
        || !["Underfull \\hbox", "Underfull \\vbox"]
            .iter()
            .any(|prefix| diagnostic.message.starts_with(prefix))
}

fn bibliography_diagnostics(log: &str) -> Vec<Diagnostic> {
    let biber = log.contains("This is Biber") || log.lines().any(|line| line.contains(" INFO - "));
    log.lines()
        .filter_map(|line| {
            let (severity, message) = if let Some((_, message)) = line.split_once("ERROR - ") {
                (DiagnosticSeverity::Error, message)
            } else if let Some((_, message)) = line.split_once("WARN - ") {
                (DiagnosticSeverity::Warning, message)
            } else {
                (DiagnosticSeverity::Warning, line.strip_prefix("Warning--")?)
            };
            Some(Diagnostic {
                correction: None,
                provider: Some(if biber { "Biber" } else { "BibTeX" }.into()),
                severity,
                source: DiagnosticSource::Global,
                location: None,
                message: message.trim().into(),
                details: Vec::new(),
            })
        })
        .collect()
}

/// Tectonic's status layer emits `warning: file.tex:line: message`. Preserve
/// unfamiliar TeX chatter in the expandable raw output, not as phantom errors.
fn diagnostics(raw: &str, entry: &Path, source: &str) -> DiagnosticReport {
    let mut diagnostics = Vec::new();
    for line in raw.lines() {
        let line = line.trim();
        let (severity, message) = if let Some(m) = line.strip_prefix("error: ") {
            (DiagnosticSeverity::Error, m)
        } else if let Some(m) = line.strip_prefix("warning: ") {
            (DiagnosticSeverity::Warning, m)
        } else if line.contains(".tex:") {
            (DiagnosticSeverity::Error, line)
        } else if line.starts_with("LaTeX Warning:")
            || line.starts_with("Overfull ")
            || line.starts_with("Underfull ")
        {
            (DiagnosticSeverity::Warning, line)
        } else {
            continue;
        };
        let mut location = None;
        let mut origin = DiagnosticSource::Global;
        let mut headline = message;
        // Split from the right-hand numeric component, preserving drive letters.
        for (offset, _) in message.match_indices(':') {
            let rest = &message[offset + 1..];
            let Some((number, text)) = rest.split_once(':') else {
                continue;
            };
            let Ok(line) = number.trim().parse::<usize>() else {
                continue;
            };
            let path = Path::new(message[..offset].trim());
            if path.extension().is_none() {
                continue;
            }
            origin = if path == entry || path == Path::new(entry.file_name().unwrap()) {
                DiagnosticSource::Main
            } else {
                DiagnosticSource::File(path.into())
            };
            location = Some(DiagnosticLocation {
                line: line.max(1),
                column: 1,
            });
            headline = text.trim();
            break;
        }
        diagnostics.push(Diagnostic {
            correction: None,
            provider: Some("Tectonic".into()),
            severity,
            source: origin,
            location,
            message: headline.into(),
            details: Vec::new(),
        });
    }
    // A fatal TeX error often has `l.12` on a later log line.
    if let Some(line) = raw.lines().find_map(|s| {
        s.trim()
            .strip_prefix("l.")
            .and_then(|s| s.split_whitespace().next())
            .and_then(|s| s.parse::<usize>().ok())
    }) && line > 0
        && line <= source.lines().count()
        && let Some(error) = diagnostics
            .iter_mut()
            .find(|d| d.severity == DiagnosticSeverity::Error && d.location.is_none())
    {
        error.source = DiagnosticSource::Main;
        error.location = Some(DiagnosticLocation { line, column: 1 });
    }
    if diagnostics.iter().any(|d| {
        d.severity == DiagnosticSeverity::Error
            && d.location.is_some()
            && !wrapper_error(&d.message)
    }) {
        let mut context = Vec::new();
        diagnostics.retain(|d| {
            if wrapper_error(&d.message) {
                context.push(d.message.clone());
                false
            } else {
                true
            }
        });
        if let Some(error) = diagnostics
            .iter_mut()
            .find(|d| d.severity == DiagnosticSeverity::Error)
        {
            error.details.extend(context);
        }
    }
    crate::diagnostics::normalize_diagnostics(&mut diagnostics);
    DiagnosticReport {
        raw: raw.into(),
        diagnostics,
    }
}

fn wrapper_error(message: &str) -> bool {
    message.starts_with("something bad happened inside ")
        || message == "the XeTeX engine had an unrecoverable error"
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(unix)]
    #[test]
    fn replacing_a_build_cancels_and_cleans_outputs_with_dotted_entry_names() {
        use std::os::unix::fs::PermissionsExt;
        let project = tempfile::tempdir().unwrap();
        let executable = project.path().join("fake-tectonic");
        fs::write(
            &executable,
            r#"#!/bin/sh
previous=''
for argument in "$@"; do
    if [ "$previous" = '--outdir' ]; then output="$argument"; fi
    case "$argument" in *.tex) entry="$argument" ;; esac
    previous="$argument"
done
if /usr/bin/grep -q WAIT "$entry"; then exec /bin/sleep 30; fi
name="${entry##*/}"
printf '%%PDF-1.7\nfixture' > "$output/${name%.tex}.pdf"
printf 'warning: main.draft.tex:1: fixture warning\n'
"#,
        )
        .unwrap();
        fs::set_permissions(&executable, fs::Permissions::from_mode(0o700)).unwrap();
        let options = TexOptions {
            engine: crate::tex::settings::BuildEngine::Tectonic,
            command: crate::tool_command::CommandCustomization::for_tool(
                crate::toolchain::ToolKind::Tectonic,
            )
            .into(),
            executable,
            only_cached: true,
            synctex: true,
            show_underfull_box_warnings: false,
            system: None,
        };
        let mut request = CompileRequest {
            revision: 1,
            input: super::super::CompileInput {
                language: tiptoptyp_core::document::TypesettingLanguage::Tex,
                source: "WAIT".into(),
                source_dir: project.path().into(),
                project_root: project.path().into(),
                display_name: "main.draft.tex".into(),
            },
            engine: super::super::EngineConfig::Tex(options.clone()),
        };
        let mut backend = Backend::new();
        backend.submit(&request, &options).unwrap();
        let old = backend.session.as_mut().unwrap();
        let old_pid = old.child.child_mut().id();
        let old_directory = old.mirror.session_dir().to_owned();
        request.revision = 2;
        request.input.source = "fresh".into();
        backend.submit(&request, &options).unwrap();
        assert!(!old_directory.exists());
        assert_eq!(
            nix::sys::signal::kill(nix::unistd::Pid::from_raw(old_pid as i32), None),
            Err(nix::errno::Errno::ESRCH)
        );
        let deadline = Instant::now() + Duration::from_secs(5);
        let pdf = loop {
            let mut completed = None;
            for result in backend.poll() {
                assert_eq!(result.revision, 2);
                match result.event {
                    EngineEvent::Pdf {
                        pdf, diagnostics, ..
                    } => {
                        assert_eq!(diagnostics.diagnostics.len(), 1);
                        completed = Some(pdf);
                    }
                    EngineEvent::Failed(report) => panic!("{}", report.raw),
                    _ => {}
                }
            }
            if let Some(pdf) = completed {
                break pdf;
            }
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(10));
        };
        drop(backend);
        assert!(pdf.starts_with(b"%PDF-1.7"));
        assert_eq!(
            fs::read_dir(project.path().join(".tiptoptyp"))
                .unwrap()
                .count(),
            0
        );
        assert!(!project.path().join("main.draft.pdf").exists());
    }

    #[cfg(unix)]
    fn executable(root: &Path, name: &str, body: &str) -> PathBuf {
        use std::os::unix::fs::PermissionsExt;
        let path = root.join(name);
        fs::write(&path, format!("#!/bin/sh\nset -eu\n{body}\n")).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).unwrap();
        path
    }
    #[cfg(unix)]
    fn system_options(root: &Path, driver: PathBuf) -> TexOptions {
        use crate::tex::{
            settings::BuildEngine,
            tools::{SystemBuildOptions, SystemProgram},
        };
        TexOptions {
            engine: BuildEngine::PdfLatex,
            command: Default::default(),
            executable: root.join("fake engine"),
            only_cached: false,
            synctex: true,
            show_underfull_box_warnings: false,
            system: Some(Box::new(SystemBuildOptions {
                driver: crate::tex::settings::BuildDriver::Latexmk,
                bibliography_enabled: true,
                latexmk: Some(SystemProgram {
                    program: driver,
                    arguments: "-quiet -bibtex-cond".into(),
                }),
                bibtex: SystemProgram {
                    program: root.join("fake bibtex"),
                    arguments: "-terse".into(),
                },
                biber: SystemProgram {
                    program: root.join("fake biber"),
                    arguments: "--sortlocale en_GB".into(),
                },
            })),
        }
    }
    #[cfg(unix)]
    fn system_request(root: &Path, options: &TexOptions, source: &str) -> CompileRequest {
        CompileRequest {
            revision: 8,
            input: super::super::CompileInput {
                language: tiptoptyp_core::document::TypesettingLanguage::Tex,
                source: source.into(),
                source_dir: root.into(),
                project_root: root.into(),
                display_name: "main.draft.tex".into(),
            },
            engine: super::super::EngineConfig::Tex(options.clone()),
        }
    }
    #[cfg(unix)]
    fn finish(backend: &mut Backend) -> (Arc<[u8]>, DiagnosticReport) {
        let deadline = Instant::now() + Duration::from_secs(60);
        loop {
            for result in backend.poll() {
                match result.event {
                    EngineEvent::Pdf {
                        pdf, diagnostics, ..
                    } => return (pdf, diagnostics),
                    EngineEvent::Failed(report) => {
                        panic!("{}\n{:?}", report.raw, report.diagnostics)
                    }
                    _ => {}
                }
            }
            assert!(Instant::now() < deadline, "build timed out");
            std::thread::sleep(Duration::from_millis(10));
        }
    }
    #[cfg(unix)]
    #[test]
    fn latexmk_build_is_finite_uses_overlay_and_never_publishes_to_project() {
        let project = tempfile::tempdir().unwrap();
        fs::write(project.path().join("main.draft.tex"), "saved source").unwrap();
        fs::write(project.path().join("library.bib"), "bibliography").unwrap();
        fs::write(project.path().join("main.draft.pdf"), "original PDF").unwrap();
        let driver = executable(
            project.path(),
            "fake latexmk",
            r#"
output=''
for argument in "$@"; do
  case "$argument" in
    -pvc) exit 10 ;;
    -outdir=*) output=${argument#-outdir=} ;;
    *.tex) entry=$argument ;;
  esac
done
[ "$(cat "$entry")" = 'unsaved source' ]
[ "$(cat library.bib)" = bibliography ]
printf '%%PDF-1.7\n' > "$output/main.draft.pdf"
printf '%s\n' "$@" > "$output/args"
printf '%s:1: fixture failure should map to original source\n' "$entry"
"#,
        );
        let options = system_options(project.path(), driver);
        let request = system_request(project.path(), &options, "unsaved source");
        let mut backend = Backend::new();
        backend.submit(&request, &options).unwrap();
        let session = backend.session.as_ref().unwrap();
        assert!(
            !session.standard,
            "latexmk reruns are not repeated by the adapter"
        );
        let private = session.mirror.session_dir().to_owned();
        let (pdf, diagnostics) = finish(&mut backend);
        assert!(pdf.starts_with(b"%PDF-"));
        assert_eq!(diagnostics.diagnostics[0].source, DiagnosticSource::Main);
        assert!(!private.exists());
        assert_eq!(
            fs::read_to_string(project.path().join("main.draft.tex")).unwrap(),
            "saved source"
        );
        assert_eq!(
            fs::read_to_string(project.path().join("main.draft.pdf")).unwrap(),
            "original PDF"
        );
        assert!(!project.path().join("main.draft.aux").exists());
    }
    #[cfg(unix)]
    #[test]
    fn latexmk_arguments_preserve_literal_tools_and_forbid_watch_modes() {
        use crate::tex::settings::BuildEngine;
        let root = tempfile::tempdir().unwrap();
        let options = system_options(root.path(), root.path().join("latexmk"));
        for (engine, mode) in [
            (BuildEngine::PdfLatex, "-pdf"),
            (BuildEngine::XeLatex, "-pdfxe"),
            (BuildEngine::LuaLatex, "-pdflua"),
        ] {
            let mut options = options.clone();
            options.engine = engine;
            let command = latexmk_command(
                &options,
                &root.path().join("main.tex"),
                &root.path().join("output"),
            )
            .unwrap();
            let args = command
                .get_args()
                .map(|s| s.to_str().unwrap())
                .collect::<Vec<_>>();
            for required in [mode, "-norc", "-pvc-", "-pv-", "-view=none"] {
                assert!(args.contains(&required), "{args:?}");
            }
            let code = args[args.iter().position(|a| *a == "-e").unwrap() + 1];
            assert!(code.contains("$biber = ") && code.contains("$bibtex = "));
            assert!(code.contains("-no-shell-escape") && code.contains("%O %S"));
            assert!(!args.contains(&"-pvc"));
        }
        let mut options = options;
        options
            .system
            .as_mut()
            .unwrap()
            .latexmk
            .as_mut()
            .unwrap()
            .arguments = "--pvc".into();
        assert!(latexmk_command(&options, &root.path().join("main.tex"), root.path()).is_err());
        options.system.as_mut().unwrap().latexmk = None;
        assert!(
            latexmk_command(&options, &root.path().join("main.tex"), root.path())
                .unwrap_err()
                .to_string()
                .contains("Install latexmk")
        );
        assert_eq!(
            literal_tool_command(Path::new("/tmp/tool"), "'literal %S' '$(touch nope)'").unwrap(),
            "/tmp/tool 'literal %%S' '$(touch nope)'"
        );
    }
    #[cfg(unix)]
    #[test]
    fn replacing_latexmk_build_cancels_entire_group_and_private_workspace() {
        let project = tempfile::tempdir().unwrap();
        let driver = executable(project.path(), "waiting latexmk", "exec /bin/sleep 30");
        let options = system_options(project.path(), driver);
        let mut backend = Backend::new();
        backend
            .submit(&system_request(project.path(), &options, "first"), &options)
            .unwrap();
        let session = backend.session.as_ref().unwrap();
        let private = session.mirror.session_dir().to_owned();
        let pid = backend.session.as_mut().unwrap().child.child_mut().id();
        let mut invalid = options.clone();
        invalid.system.as_mut().unwrap().latexmk = None;
        assert!(
            backend
                .submit(
                    &system_request(project.path(), &invalid, "second"),
                    &invalid
                )
                .is_err()
        );
        assert!(!private.exists());
        assert_eq!(
            nix::sys::signal::kill(nix::unistd::Pid::from_raw(pid as i32), None),
            Err(nix::errno::Errno::ESRCH)
        );
        assert!(!backend.is_running());
    }
    #[cfg(unix)]
    #[test]
    #[ignore = "requires a local TeX distribution with latexmk, BibTeX and Biber"]
    fn installed_latexmk_builds_bibtex_and_biber_without_changing_sources() {
        use crate::tex::{
            settings::{BuildEngine, TexSettings},
            tools::TexTools,
        };
        for engine in [
            BuildEngine::PdfLatex,
            BuildEngine::XeLatex,
            BuildEngine::LuaLatex,
        ] {
            for biber in [false, true] {
                let project = tempfile::tempdir().unwrap();
                let bibliography = "@book{fixture, author={Ada Lovelace}, title={Analytical Engine}, year={1843}, publisher={Example}}\n";
                fs::write(project.path().join("library.bib"), bibliography).unwrap();
                let source = if biber {
                    "\\documentclass{article}\n\\usepackage[backend=biber]{biblatex}\n\\addbibresource{library.bib}\n\\begin{document}\nCitation~\\cite{fixture}.\n\\printbibliography\n\\end{document}\n"
                } else {
                    "\\documentclass{article}\n\\begin{document}\nCitation~\\cite{fixture}.\n\\bibliographystyle{plain}\n\\bibliography{library}\n\\end{document}\n"
                };
                let settings = TexSettings {
                    build_engine: engine,
                    ..Default::default()
                };
                let tools = TexTools::resolve(&settings);
                let options = TexOptions {
                    engine,
                    command: Default::default(),
                    executable: tools.distribution(engine).expect("installed engine").into(),
                    only_cached: true,
                    synctex: true,
                    show_underfull_box_warnings: false,
                    system: tools.build_options(&settings),
                };
                let mut backend = Backend::new();
                backend
                    .submit(&system_request(project.path(), &options, source), &options)
                    .unwrap();
                // latexmk logs every pass, including expected first-pass
                // undefined citations. Inspect the final engine log and bbl
                // before the backend snapshots output and retires its mirror.
                let deadline = Instant::now() + Duration::from_secs(60);
                let final_output = loop {
                    let session = backend.session.as_mut().unwrap();
                    if session.child.try_wait().unwrap().is_some() {
                        break session.pdf.clone();
                    }
                    assert!(
                        Instant::now() < deadline,
                        "{} / biber={biber} timed out",
                        engine.label()
                    );
                    std::thread::sleep(Duration::from_millis(20));
                };
                let final_log = fs::read_to_string(final_output.with_extension("log")).unwrap();
                assert!(
                    !final_log.contains("undefined citations")
                        && !final_log.contains("Citation `fixture'"),
                    "{} / biber={biber}: {final_log}",
                    engine.label()
                );
                let bbl = fs::read_to_string(final_output.with_extension("bbl")).unwrap();
                assert!(
                    bbl.contains("Lovelace"),
                    "{} / biber={biber}: {bbl}",
                    engine.label()
                );
                let (pdf, diagnostics) = finish(&mut backend);
                assert!(pdf.starts_with(b"%PDF-"));
                assert!(
                    diagnostics
                        .raw
                        .contains(if biber { "biber" } else { "bibtex" })
                );
                assert_eq!(
                    fs::read_to_string(project.path().join("library.bib")).unwrap(),
                    bibliography
                );
                assert!(!project.path().join("main.draft.pdf").exists());
                assert!(!project.path().join("main.draft.aux").exists());
                assert!(!project.path().join("main.draft.bbl").exists());
                assert!(
                    fs::read_dir(project.path().join(".tiptoptyp"))
                        .unwrap()
                        .next()
                        .is_none()
                );
            }
        }
    }

    #[test]
    fn bibliography_messages_keep_provider_and_ignore_info() {
        let diagnostics = bibliography_diagnostics(
            "[0] INFO - This is Biber 2.20\n[1] WARN - Missing field year\n[2] ERROR - Cannot find database\n",
        );
        assert_eq!(diagnostics.len(), 2);
        assert_eq!(diagnostics[0].provider.as_deref(), Some("Biber"));
        assert_eq!(diagnostics[1].severity, DiagnosticSeverity::Error);
        let diagnostics =
            bibliography_diagnostics("This is BibTeX\nWarning--empty publisher in fixture\n");
        assert_eq!(diagnostics[0].provider.as_deref(), Some("BibTeX"));
    }
    #[cfg(unix)]
    #[test]
    fn latexmk_diagnostics_discard_resolved_first_pass_citations() {
        let project = tempfile::tempdir().unwrap();
        let driver = executable(
            project.path(),
            "fixture latexmk",
            r#"
for argument in "$@"; do
  case "$argument" in -outdir=*) output=${argument#-outdir=} ;; esac
done
printf '%%PDF-1.7\n' > "$output/main.draft.pdf"
printf 'LaTeX Warning: persistent final warning\n' > "$output/main.draft.log"
printf 'LaTeX Warning: Citation fixture undefined\n'
"#,
        );
        let options = system_options(project.path(), driver);
        let mut backend = Backend::new();
        backend
            .submit(
                &system_request(project.path(), &options, "source"),
                &options,
            )
            .unwrap();
        let (_, report) = finish(&mut backend);
        assert!(report.raw.contains("Citation fixture undefined"));
        assert_eq!(report.diagnostics.len(), 1);
        assert!(
            report.diagnostics[0]
                .message
                .contains("persistent final warning")
        );
    }

    #[cfg(unix)]
    #[test]
    fn underfull_warning_visibility_preserves_real_diagnostics_and_raw_logs_for_every_engine() {
        use crate::tex::settings::BuildEngine;
        for engine in BuildEngine::ALL {
            for show_underfull in [false, true] {
                let project = tempfile::tempdir().unwrap();
                let driver = executable(
                    project.path(),
                    "fixture compiler",
                    r#"
output=''
next_output=no
for argument in "$@"; do
  if [ "$next_output" = yes ]; then output=$argument; next_output=no; fi
  case "$argument" in
    --outdir) next_output=yes ;;
    -outdir=*) output=${argument#-outdir=} ;;
  esac
done
printf '%%PDF-1.7\n' > "$output/main.draft.pdf"
cat > "$output/main.draft.log" <<'LOG'
Underfull \hbox (badness 10000) in paragraph at lines 1--2
Underfull \vbox (badness 10000) while output is active
Overfull \hbox (12.0pt too wide) at line 2
LaTeX Warning: Reference `missing' on page 1 undefined on input line 2.
main.draft.tex:2: Undefined control sequence.
LOG
cat "$output/main.draft.log"
cat > "$output/main.draft.blg" <<'LOG'
[0] INFO - This is Biber 2.20
[1] WARN - Missing bibliography field year
[2] ERROR - Cannot find bibliography database
LOG
"#,
                );
                let mut options = system_options(project.path(), driver.clone());
                options.engine = engine;
                options.show_underfull_box_warnings = show_underfull;
                if engine == BuildEngine::Tectonic {
                    options.executable = driver;
                    options.command = crate::tool_command::CommandCustomization::for_tool(
                        crate::toolchain::ToolKind::Tectonic,
                    )
                    .into();
                    options.system = None;
                }
                let mut backend = Backend::new();
                backend
                    .submit(
                        &system_request(project.path(), &options, "first\nsecond"),
                        &options,
                    )
                    .unwrap();
                let (_, report) = finish(&mut backend);
                assert_eq!(
                    report
                        .diagnostics
                        .iter()
                        .filter(|d| d.message.starts_with("Underfull "))
                        .count(),
                    if show_underfull { 2 } else { 0 },
                    "{engine:?}"
                );
                assert!(
                    report
                        .diagnostics
                        .iter()
                        .any(|d| d.message.starts_with("Overfull ")),
                    "{engine:?}"
                );
                assert!(
                    report
                        .diagnostics
                        .iter()
                        .any(|d| d.message.contains("Reference `missing'")),
                    "{engine:?}"
                );
                assert!(
                    report
                        .diagnostics
                        .iter()
                        .any(|d| d.severity == DiagnosticSeverity::Error
                            && d.message.contains("Undefined control sequence")),
                    "{engine:?}"
                );
                assert!(report.raw.contains("Underfull \\hbox"), "{engine:?}");
                if engine != BuildEngine::Tectonic {
                    assert!(
                        report
                            .diagnostics
                            .iter()
                            .any(|d| d.provider.as_deref() == Some("Biber")
                                && d.message.contains("Missing bibliography field")),
                        "{engine:?}"
                    );
                    assert!(
                        report
                            .diagnostics
                            .iter()
                            .any(|d| d.provider.as_deref() == Some("Biber")
                                && d.severity == DiagnosticSeverity::Error),
                        "{engine:?}"
                    );
                    assert!(
                        report.raw.contains("Final TeX log")
                            && report.raw.contains("Bibliography log"),
                        "{engine:?}"
                    );
                }
            }
        }
    }

    #[test]
    fn underfull_filter_cannot_hide_an_error_or_an_unrelated_warning() {
        for (severity, message, visible) in [
            (
                DiagnosticSeverity::Warning,
                "Underfull \\hbox (badness 10000)",
                false,
            ),
            (
                DiagnosticSeverity::Warning,
                "Underfull \\vbox (badness 10000)",
                false,
            ),
            (
                DiagnosticSeverity::Error,
                "Underfull \\hbox is an actual fixture error",
                true,
            ),
            (
                DiagnosticSeverity::Warning,
                "Overfull \\hbox (10pt too wide)",
                true,
            ),
            (
                DiagnosticSeverity::Warning,
                "LaTeX Warning: Citation missing undefined",
                true,
            ),
            (
                DiagnosticSeverity::Warning,
                "Package warning mentioning Underfull \\hbox",
                true,
            ),
        ] {
            let diagnostic = Diagnostic {
                severity,
                message: message.into(),
                ..DiagnosticReport::error("fixture".into())
                    .diagnostics
                    .remove(0)
            };
            assert_eq!(
                compiler_diagnostic_visible(&diagnostic, false),
                visible,
                "{message}"
            );
            assert!(compiler_diagnostic_visible(&diagnostic, true), "{message}");
        }
    }

    #[test]
    fn parses_status_locations_without_splitting_windows_drive() {
        let report = diagnostics(
            "warning: main.tex:2: Underfull box\nerror: C:\\work\\part.tex:9: Undefined control sequence\nl.1 \\oops\n",
            Path::new("main.tex"),
            "é😀\ntext",
        );
        assert_eq!(report.diagnostics.len(), 2);
        assert_eq!(report.diagnostics[0].source, DiagnosticSource::Main);
        assert_eq!(report.diagnostics[0].location.unwrap().line, 2);
        assert_eq!(report.diagnostics[1].location.unwrap().line, 9);
    }
    #[test]
    #[ignore = "requires a local TeX distribution and synctex"]
    fn installed_engines_build_and_synctex_round_trip_after_private_mirror_is_removed() {
        use crate::tex::settings::BuildEngine;
        for engine in [
            BuildEngine::PdfLatex,
            BuildEngine::XeLatex,
            BuildEngine::LuaLatex,
        ] {
            let executable = crate::tex::tools::distribution_tool(engine.executable())
                .expect("install TeX distribution");
            let project = tempfile::tempdir().unwrap();
            let source = "\\documentclass{article}\n\\begin{document}\n\\section{Introduction}\nA short paragraph for source navigation.\n\\end{document}\n";
            let options = TexOptions {
                engine,
                executable,
                command: Default::default(),
                only_cached: true,
                synctex: true,
                show_underfull_box_warnings: false,
                system: None,
            };
            let request = CompileRequest {
                revision: 1,
                input: super::super::CompileInput {
                    language: tiptoptyp_core::document::TypesettingLanguage::Tex,
                    source: source.into(),
                    source_dir: project.path().into(),
                    project_root: project.path().into(),
                    display_name: "main.tex".into(),
                },
                engine: super::super::EngineConfig::Tex(options.clone()),
            };
            let mut backend = Backend::new();
            backend.submit(&request, &options).unwrap();
            let deadline = Instant::now() + Duration::from_secs(60);
            let (pdf, map) = loop {
                let result = backend
                    .poll()
                    .into_iter()
                    .find_map(|result| match result.event {
                        EngineEvent::Pdf { pdf, synctex, .. } => {
                            Some((pdf, synctex.expect("SyncTeX map")))
                        }
                        EngineEvent::Failed(report) => panic!("{}: {}", engine.label(), report.raw),
                        _ => None,
                    });
                if let Some(result) = result {
                    break result;
                }
                assert!(Instant::now() < deadline, "{} timed out", engine.label());
                std::thread::sleep(Duration::from_millis(20));
            };
            assert!(!map.mirror.exists());
            assert!(pdf.starts_with(b"%PDF-"));
            let target = crate::synctex::query(
                &map,
                &pdf,
                crate::synctex::Query::Source {
                    path: project.path().join("main.tex"),
                    line: 4,
                    column: 1,
                },
            )
            .unwrap();
            let crate::synctex::Destination::Page { page, x, y } = target else {
                panic!("expected PDF destination")
            };
            let target =
                crate::synctex::query(&map, &pdf, crate::synctex::Query::Page { page, x, y })
                    .unwrap();
            let crate::synctex::Destination::Source { path, line, .. } = target else {
                panic!("expected source destination")
            };
            assert_eq!(
                path,
                project.path().canonicalize().unwrap().join("main.tex")
            );
            assert!((3..=4).contains(&line), "{engine:?}: line {line}");
            assert!(!project.path().join("main.pdf").exists());
        }
    }

    #[test]
    #[ignore = "builds arXiv manual fixtures with every installed engine; needs toolchains, packages and minutes"]
    fn arxiv_manual_fixtures_build_with_every_installed_engine() {
        use crate::tex::settings::BuildEngine;
        fn copy_dir(source: &Path, destination: &Path) {
            fs::create_dir_all(destination).unwrap();
            for entry in fs::read_dir(source).unwrap() {
                let entry = entry.unwrap();
                let target = destination.join(entry.file_name());
                if entry.file_type().unwrap().is_dir() {
                    copy_dir(&entry.path(), &target);
                } else {
                    fs::copy(entry.path(), target).unwrap();
                }
            }
        }
        fn entry_point(directory: &Path) -> PathBuf {
            let mut entries = fs::read_dir(directory)
                .unwrap()
                .map(|entry| entry.unwrap().path())
                .filter(|path| {
                    path.extension().is_some_and(|extension| extension == "tex")
                        && fs::read_to_string(path)
                            .is_ok_and(|source| source.contains("\\documentclass"))
                })
                .collect::<Vec<_>>();
            assert_eq!(entries.len(), 1, "one document entry per fixture");
            entries.pop().unwrap()
        }
        // An opt-in fixture run should exercise installed engines without
        // requiring every distribution. Refuse a vacuous pass if none exist.
        let engines = BuildEngine::ALL
            .into_iter()
            .filter_map(|engine| {
                let executable = if engine == BuildEngine::Tectonic {
                    let tool = crate::toolchain::resolve_tool(
                        crate::toolchain::ToolKind::Tectonic,
                        &crate::settings::ToolPreference::default(),
                    );
                    tool.is_available().then_some(tool.program)
                } else {
                    crate::tex::tools::distribution_tool(engine.executable())
                };
                executable.map(|path| (engine, path))
            })
            .collect::<Vec<_>>();
        assert!(!engines.is_empty(), "install at least one TeX build engine");
        let fixtures = Path::new(env!("CARGO_MANIFEST_DIR")).join("manual-tests/arxiv");
        for name in ["editikz", "lax-frobenius"] {
            let project = tempfile::tempdir().unwrap();
            copy_dir(&fixtures.join(name), project.path());
            let entry = entry_point(project.path());
            let source = fs::read_to_string(&entry).unwrap();
            for (engine, executable) in &engines {
                let options = TexOptions {
                    engine: *engine,
                    executable: executable.clone(),
                    command: if *engine == crate::tex::settings::BuildEngine::Tectonic {
                        crate::tool_command::CommandCustomization::for_tool(
                            crate::toolchain::ToolKind::Tectonic,
                        )
                        .into()
                    } else {
                        Default::default()
                    },
                    only_cached: false,
                    synctex: false,
                    show_underfull_box_warnings: false,
                    system: None,
                };
                let request = CompileRequest {
                    revision: 1,
                    input: super::super::CompileInput {
                        language: tiptoptyp_core::document::TypesettingLanguage::Tex,
                        source: source.clone(),
                        source_dir: project.path().into(),
                        project_root: project.path().into(),
                        display_name: entry.file_name().unwrap().to_string_lossy().into_owned(),
                    },
                    engine: super::super::EngineConfig::Tex(options.clone()),
                };
                let mut backend = Backend::new();
                backend.submit(&request, &options).unwrap();
                let deadline = Instant::now() + Duration::from_secs(600);
                let pdf = loop {
                    let mut completed = None;
                    for result in backend.poll() {
                        match result.event {
                            EngineEvent::Pdf { pdf, .. } => completed = Some(pdf),
                            EngineEvent::Failed(report) => {
                                panic!("{name} with {}: {}", engine.label(), report.raw)
                            }
                            _ => {}
                        }
                    }
                    if let Some(pdf) = completed {
                        break pdf;
                    }
                    assert!(
                        Instant::now() < deadline,
                        "{name} with {} timed out",
                        engine.label()
                    );
                    std::thread::sleep(Duration::from_millis(50));
                };
                assert!(
                    pdf.starts_with(b"%PDF-"),
                    "{name} with {} produced no PDF",
                    engine.label()
                );
            }
        }
    }

    #[test]
    #[ignore = "executes pinned Tectonic; first run may download TeX packages"]
    fn real_tectonic_builds_unsaved_source_relative_inputs_errors_and_recovers() {
        let project = tempfile::tempdir().unwrap();
        fs::create_dir(project.path().join("sub")).unwrap();
        fs::write(project.path().join("sub/part.tex"), "Included text.").unwrap();
        fs::write(project.path().join("main.tex"), "disk sentinel").unwrap();
        let source = "\\documentclass{article}\n\\begin{document}\nHello é. \\input{sub/part}\n\\end{document}\n";
        let mut request = CompileRequest {
            revision: 1,
            input: super::super::CompileInput {
                language: tiptoptyp_core::document::TypesettingLanguage::Tex,
                source: source.into(),
                source_dir: project.path().into(),
                project_root: project.path().into(),
                display_name: "main.tex".into(),
            },
            engine: super::super::EngineConfig::Tex(TexOptions {
                engine: crate::tex::settings::BuildEngine::Tectonic,
                command: crate::tool_command::CommandCustomization::for_tool(
                    crate::toolchain::ToolKind::Tectonic,
                )
                .into(),
                executable: crate::toolchain::resolve_tool(
                    crate::toolchain::ToolKind::Tectonic,
                    &crate::settings::ToolPreference::default(),
                )
                .program,
                only_cached: false,
                synctex: true,
                show_underfull_box_warnings: false,
                system: None,
            }),
        };
        let super::super::EngineConfig::Tex(options) = request.engine.clone() else {
            unreachable!()
        };
        let mut backend = Backend::new();
        for valid in [true, false, true] {
            request.revision += 1;
            request.input.source = if valid {
                source.into()
            } else {
                "\\documentclass{article}\n\\begin{document}\n\\undefinedcommand\n\\end{document}\n"
                    .into()
            };
            backend.submit(&request, &options).unwrap();
            let start = Instant::now();
            loop {
                let mut done = false;
                for event in backend.poll() {
                    assert_eq!(event.revision, request.revision);
                    match event.event {
                        EngineEvent::Pdf { pdf, synctex, .. } => {
                            assert!(valid);
                            assert!(pdf.starts_with(b"%PDF-"));
                            let map = synctex.expect("Tectonic must emit a SyncTeX map");
                            assert!(matches!(
                                crate::synctex::query(
                                    &map,
                                    &pdf,
                                    crate::synctex::Query::Source {
                                        path: project.path().join("main.tex"),
                                        line: 3,
                                        column: 1,
                                    }
                                )
                                .unwrap(),
                                crate::synctex::Destination::Page { page: 0, .. }
                            ));
                            done = true;
                        }
                        EngineEvent::Failed(report) => {
                            assert!(!valid, "{}", report.raw);
                            assert!(
                                report
                                    .diagnostics
                                    .iter()
                                    .any(|d| d.severity == DiagnosticSeverity::Error)
                            );
                            eprintln!("Tectonic failure diagnostics: {:?}", report.diagnostics);
                            done = true;
                        }
                        _ => {}
                    }
                }
                if done {
                    break;
                }
                assert!(start.elapsed() < BUILD_TIMEOUT + Duration::from_secs(1));
                std::thread::sleep(Duration::from_millis(15));
            }
        }
        drop(backend);
        assert_eq!(
            fs::read_to_string(project.path().join("main.tex")).unwrap(),
            "disk sentinel"
        );
        for extension in ["pdf", "aux", "log"] {
            assert!(
                !project
                    .path()
                    .join("main")
                    .with_extension(extension)
                    .exists()
            );
        }
        assert_eq!(
            fs::read_dir(project.path().join(".tiptoptyp"))
                .unwrap()
                .count(),
            0
        );
    }
}
