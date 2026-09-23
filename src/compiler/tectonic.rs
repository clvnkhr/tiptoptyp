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
pub(crate) struct TectonicOptions {
    pub(crate) command: std::sync::Arc<crate::tool_command::CommandCustomization>,
    pub(crate) executable: PathBuf,
    pub(crate) only_cached: bool,
}

struct Session {
    // Stop the process before its mirror and output are removed.
    child: crate::process::OwnedChild,
    mirror: PrivateSourceMirror,
    log: PathBuf,
    pdf: PathBuf,
    revision: u64,
    root: PathBuf,
    source: String,
    started: Instant,
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
        options: &TectonicOptions,
    ) -> Result<(), String> {
        self.session = None;
        self.started = false;
        self.session = Some(
            Session::start(request, options)
                .map_err(|e| format!("Could not start Tectonic: {e}"))?,
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
            Ok(Some(status)) => Ok(status),
            Ok(None) => Err("Tectonic exceeded the build time or log limit".to_owned()),
            Err(error) => Err(format!("Could not wait for Tectonic: {error}")),
        };
        let session = self.session.take().unwrap();
        let mut report = diagnostics(
            &read_limited(&session.log, MAX_LOG).unwrap_or_default(),
            session.mirror.path(),
            &session.source,
        );
        for diagnostic in &mut report.diagnostics {
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
                        status.map_or_else(|e| e, |s| format!("Tectonic exited with {s}"));
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
    fn start(request: &CompileRequest, options: &TectonicOptions) -> std::io::Result<Self> {
        let private = PrivateWorkspace::open(&request.input.project_root)?;
        let mirror = private.mirrored_source(
            &request.input.source_dir,
            &request.input.display_name,
            &request.input.source,
        )?;
        let output = mirror.session_dir().join("output");
        fs::create_dir(&output)?;
        let log = mirror.session_dir().join("tectonic-output.log");
        let logs = File::create(&log)?;
        let pdf = output
            .join(mirror.path().file_name().unwrap())
            .with_extension("pdf");
        let mut command = Command::new(&options.executable);
        command
            .args([
                "-X",
                "compile",
                "--untrusted",
                "--keep-logs",
                "--keep-intermediates",
                "--outdir",
            ])
            .arg(&output)
            .arg(mirror.path())
            .current_dir(mirror.path().parent().unwrap())
            .env("NO_COLOR", "1");
        if options.only_cached {
            command.arg("--only-cached");
        }
        options.command.apply(&mut command)?;
        command
            .stdin(Stdio::null())
            .stdout(logs.try_clone()?)
            .stderr(logs);
        let child = crate::process::OwnedChild::spawn(&mut command)?;
        Ok(Self {
            child,
            mirror,
            log,
            pdf,
            revision: request.revision,
            root: private.project_root().to_owned(),
            source: request.input.source.clone(),
            started: Instant::now(),
        })
    }
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
        .map_err(|e| format!("Tectonic produced no readable PDF: {e}"))?;
    if bytes.len() as u64 > MAX_PDF || !bytes.starts_with(b"%PDF-") {
        return Err("Tectonic produced an invalid or oversized PDF".into());
    }
    Ok(bytes.into())
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
        let options = TectonicOptions {
            command: Default::default(),
            executable,
            only_cached: true,
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
            engine: super::super::EngineConfig::Tectonic(options.clone()),
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
            engine: super::super::EngineConfig::Tectonic(TectonicOptions {
                command: Default::default(),
                executable: crate::toolchain::resolve_tool(
                    crate::toolchain::ToolKind::Tectonic,
                    &crate::settings::ToolPreference::default(),
                )
                .program,
                only_cached: false,
            }),
        };
        let super::super::EngineConfig::Tectonic(options) = request.engine.clone() else {
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
                        EngineEvent::Pdf { pdf, .. } => {
                            assert!(valid);
                            assert!(pdf.starts_with(b"%PDF-"));
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
