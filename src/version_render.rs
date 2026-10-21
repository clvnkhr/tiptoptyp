//! Isolated PDF rendering of a version-history snapshot. The shared compiler
//! preserves configured arguments, fonts, relative imports and TeX passes;
//! this owner never submits to the editor's preview compiler.
use crate::{
    compiler::{CompileEvent, CompileInput, CompileRequest, Compiler, EngineConfig},
    private_workspace::PrivateWorkspace,
    worker::{OperationSummary, RepaintTarget},
};
use std::{
    fs,
    path::PathBuf,
    sync::atomic::{AtomicBool, Ordering},
    time::{Duration, Instant},
};

const TIMEOUT: Duration = Duration::from_secs(600);
const POLL_INTERVAL: Duration = Duration::from_millis(20);
const MAX_PDF: usize = 256 * 1024 * 1024;

#[derive(Debug, Clone)]
pub(crate) struct Request {
    pub(crate) original: PathBuf,
    pub(crate) source: String,
    pub(crate) project_root: PathBuf,
    pub(crate) engine: EngineConfig,
}

/// Keep this owner while its PDF is open: the directory is deleted on drop.
#[derive(Debug)]
pub(crate) struct RenderedVersion {
    pub(crate) path: PathBuf,
    pub(crate) pdf: std::sync::Arc<[u8]>,
    _directory: tempfile::TempDir,
}
impl OperationSummary for RenderedVersion {
    fn completion_summary(&self) -> String {
        "Version PDF rendered".into()
    }
}

/// Runs on the history worker, including process shutdown and all filesystem IO.
/// Only the selected file's bytes are historical; imported files and assets are
/// resolved from the current project, as with ordinary editor snapshot builds.
pub(crate) fn render(request: Request, cancel: &AtomicBool) -> Result<RenderedVersion, String> {
    render_with_timeout(request, cancel, TIMEOUT)
}

fn render_with_timeout(
    request: Request,
    cancel: &AtomicBool,
    timeout: Duration,
) -> Result<RenderedVersion, String> {
    if cancel.load(Ordering::Acquire) {
        return Err("Version PDF rendering cancelled".into());
    }
    let original = request
        .original
        .canonicalize()
        .map_err(|error| format!("Cannot locate the original document: {error}"))?;
    if !original.is_file() {
        return Err("Version PDF rendering needs a document file".into());
    }
    let language = crate::document::detect_document(&original, request.source.as_bytes())?
        .typesetting_language()
        .ok_or("Only Typst and TeX versions can be rendered to PDF")?;
    let display_name = original
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or("The original document needs a UTF-8 filename")?
        .to_owned();
    let source_dir = original
        .parent()
        .ok_or("The document needs a parent directory")?
        .to_owned();
    let private = PrivateWorkspace::open(&request.project_root)
        .map_err(|error| format!("Cannot prepare the version PDF: {error}"))?;
    // A private context receives compiler wakeups without repainting or altering
    // the real editor. ExclusiveJob publishes the completed result to its owner.
    let compiler = Compiler::new(RepaintTarget::current(&eframe::egui::Context::default()));
    compiler.request(CompileRequest {
        revision: 0,
        input: CompileInput {
            language,
            source: request.source,
            source_dir,
            project_root: private.project_root().to_owned(),
            display_name,
        },
        engine: request.engine,
    })?;
    let started = Instant::now();
    let pdf = loop {
        if cancel.load(Ordering::Acquire) {
            return Err("Version PDF rendering cancelled".into());
        }
        if started.elapsed() >= timeout {
            return Err("Version PDF rendering timed out after 10 minutes".into());
        }
        if let Some(result) = compiler.try_recv() {
            match result.event {
                CompileEvent::Started => {}
                CompileEvent::Failed(report) => {
                    let explanation = if report.raw.trim().is_empty() {
                        report
                            .diagnostics
                            .iter()
                            .map(|item| item.full_message())
                            .collect::<Vec<_>>()
                            .join("\n")
                    } else {
                        report.raw
                    };
                    let explanation: String = explanation.chars().take(16 * 1024).collect();
                    return Err(format!("Could not render this version:\n{explanation}"));
                }
                CompileEvent::Artifact(artifact) => break artifact.pdf,
            }
        }
        std::thread::sleep(POLL_INTERVAL);
    };
    // Stop the watch process and release its source mirror before exposing the
    // artifact; future project edits cannot change these immutable PDF bytes.
    drop(compiler);
    if cancel.load(Ordering::Acquire) {
        return Err("Version PDF rendering cancelled".into());
    }
    if !pdf.starts_with(b"%PDF-") || pdf.len() > MAX_PDF {
        return Err("The compiler produced an invalid or oversized version PDF".into());
    }
    let directory = private
        .temp_dir("version-pdf-")
        .map_err(|error| format!("Cannot create the version PDF directory: {error}"))?;
    let mut path = directory.path().join(original.file_name().unwrap());
    path.set_extension("pdf");
    fs::write(&path, &pdf).map_err(|error| format!("Cannot save the version PDF: {error}"))?;
    Ok(RenderedVersion {
        path,
        pdf,
        _directory: directory,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        compiler::{TexOptions, TypstOptions},
        tool_command::CommandCustomization,
    };
    use std::sync::Arc;

    #[cfg(unix)]
    fn script(root: &std::path::Path, body: &str) -> PathBuf {
        use std::os::unix::fs::PermissionsExt;
        let path = root.join("compiler");
        fs::write(&path, format!("#!/bin/sh\nset -eu\n{body}\n")).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).unwrap();
        path
    }

    fn request(root: &std::path::Path, extension: &str, executable: PathBuf) -> Request {
        let source_dir = root.join("chapters");
        fs::create_dir_all(&source_dir).unwrap();
        let original = source_dir.join(format!("paper.draft.{extension}"));
        fs::write(&original, "current text").unwrap();
        fs::write(source_dir.join("asset.txt"), "current asset").unwrap();
        Request {
            original,
            source: "historical text".into(),
            project_root: root.into(),
            engine: EngineConfig::Typst(TypstOptions {
                command: CommandCustomization::for_tool(crate::toolchain::ToolKind::Typst).into(),
                executable,
                font_paths: Vec::new(),
            }),
        }
    }

    #[test]
    fn already_cancelled_render_does_not_create_private_storage() {
        let root = tempfile::tempdir().unwrap();
        let request = request(root.path(), "typ", "must-not-run".into());
        let error = render(request, &AtomicBool::new(true)).unwrap_err();
        assert!(error.contains("cancelled"));
        assert!(!root.path().join(".tiptoptyp").exists());
    }

    #[cfg(unix)]
    #[test]
    fn typst_renders_exact_snapshot_with_configured_environment_and_relative_assets() {
        let root = tempfile::tempdir().unwrap();
        let program = script(
            root.path(),
            r#"
previous=''; source=''; output=''
for arg do
    source=$previous; output=$arg; previous=$arg
done
[ "$VERSION_TEST" = 'configured value' ]
[ "$(pwd -P)" = "$EXPECTED_CWD" ]
[ "$(cat "$source")" = 'historical text' ]
[ "$(cat "${source%/*}/asset.txt")" = 'current asset' ]
printf 'compiling ...\n' >&2
printf '%%PDF-1.7\n' > "$output"
cat "$source" >> "$output"
printf '\ncompiled successfully\n' >&2
exec sleep 60
"#,
        );
        let mut request = request(root.path(), "typ", program);
        let EngineConfig::Typst(options) = &mut request.engine else {
            unreachable!()
        };
        let mut command = (*options.command).clone();
        command.directory = root
            .path()
            .canonicalize()
            .unwrap()
            .to_string_lossy()
            .into_owned();
        command.environment = serde_json::json!({"VERSION_TEST":"configured value", "EXPECTED_CWD":command.directory}).to_string();
        options.command = command.into();
        let original = request.original.clone();
        let adjacent_pdf = original.with_extension("pdf");
        fs::write(&adjacent_pdf, "existing PDF").unwrap();
        let result = render(request, &AtomicBool::new(false)).unwrap();
        assert_eq!(
            fs::read(&result.path).unwrap(),
            b"%PDF-1.7\nhistorical text"
        );
        assert_eq!(fs::read_to_string(original).unwrap(), "current text");
        assert_eq!(fs::read_to_string(adjacent_pdf).unwrap(), "existing PDF");
        let private = root.path().join(".tiptoptyp");
        let entries: Vec<_> = fs::read_dir(private)
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect();
        assert_eq!(
            entries.len(),
            1,
            "Compiler mirrors are removed before publication"
        );
        let path = result.path.clone();
        assert!(path.is_file());
        drop(result);
        assert!(!path.exists(), "The viewer owns the PDF's lifetime");
    }

    #[cfg(unix)]
    #[test]
    fn tex_uses_the_selected_engine_and_never_overwrites_the_original_or_its_pdf() {
        let root = tempfile::tempdir().unwrap();
        let program = script(
            root.path(),
            r#"
previous=''; output=''; source=''
for arg do
    if [ "$previous" = --outdir ]; then output=$arg; fi
    case "$arg" in *.tex) source=$arg ;; esac
    previous=$arg
done
[ "$VERSION_TEST" = configured ]
[ "$(cat "$source")" = 'historical text' ]
[ "$(cat asset.txt)" = 'current asset' ]
name=${source##*/}
printf '%%PDF-1.7\n' > "$output/${name%.tex}.pdf"
cat "$source" >> "$output/${name%.tex}.pdf"
"#,
        );
        let mut request = request(root.path(), "tex", program.clone());
        let mut command = CommandCustomization::for_tool(crate::toolchain::ToolKind::Tectonic);
        command.environment = r#"{"VERSION_TEST":"configured"}"#.into();
        request.engine = EngineConfig::Tex(TexOptions {
            engine: crate::tex::settings::BuildEngine::Tectonic,
            command: command.into(),
            executable: program,
            only_cached: true,
            synctex: true,
            system: None,
        });
        let original = request.original.clone();
        let adjacent_pdf = original.with_extension("pdf");
        fs::write(&adjacent_pdf, "existing PDF").unwrap();
        let result = render(request, &AtomicBool::new(false)).unwrap();
        assert_eq!(
            fs::read(&result.path).unwrap(),
            b"%PDF-1.7\nhistorical text"
        );
        assert_eq!(fs::read_to_string(original).unwrap(), "current text");
        assert_eq!(fs::read_to_string(adjacent_pdf).unwrap(), "existing PDF");
    }

    #[cfg(unix)]
    #[test]
    fn cancelled_inflight_build_reaps_the_process_and_cleans_its_mirror() {
        let root = tempfile::tempdir().unwrap();
        let program = script(
            root.path(),
            "printf ready > \"$VERSION_STARTED\"\nexec sleep 60",
        );
        let mut request = request(root.path(), "typ", program);
        let marker = root.path().join("started");
        let EngineConfig::Typst(options) = &mut request.engine else {
            unreachable!()
        };
        let mut command = (*options.command).clone();
        command.environment = serde_json::json!({"VERSION_STARTED":marker}).to_string();
        options.command = command.into();
        let cancelled = Arc::new(AtomicBool::new(false));
        let worker_cancelled = cancelled.clone();
        let worker = std::thread::spawn(move || render(request, &worker_cancelled));
        let deadline = Instant::now() + Duration::from_secs(5);
        while !marker.exists() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
        cancelled.store(true, Ordering::Release);
        let error = worker.join().unwrap().unwrap_err();
        assert!(
            marker.exists(),
            "The process must start before cancellation"
        );
        assert!(error.contains("cancelled"));
        assert_eq!(
            fs::read_dir(root.path().join(".tiptoptyp"))
                .unwrap()
                .count(),
            0
        );
    }

    #[cfg(unix)]
    #[test]
    fn compiler_failures_preserve_the_error_and_remove_the_snapshot() {
        let root = tempfile::tempdir().unwrap();
        let program = script(
            root.path(),
            "printf 'compiling ...\\nerror: missing package fixture\\ncompiled with errors\\n' >&2\nexec sleep 60",
        );
        let request = request(root.path(), "typ", program);
        let error = render(request, &AtomicBool::new(false)).unwrap_err();
        assert!(error.contains("missing package fixture"), "{error}");
        assert_eq!(
            fs::read_dir(root.path().join(".tiptoptyp"))
                .unwrap()
                .count(),
            0
        );
    }
    #[cfg(unix)]
    #[test]
    fn successful_compiler_message_cannot_publish_a_non_pdf_artifact() {
        let root = tempfile::tempdir().unwrap();
        let program = script(
            root.path(),
            r#"
for output do :; done
printf not-a-pdf > "$output"
printf 'compiling ...\ncompiled successfully\n' >&2
exec sleep 60
"#,
        );
        let request = request(root.path(), "typ", program);
        let error = render(request, &AtomicBool::new(false)).unwrap_err();
        assert!(
            error.contains("invalid or oversized version PDF"),
            "{error}"
        );
        assert_eq!(
            fs::read_dir(root.path().join(".tiptoptyp"))
                .unwrap()
                .count(),
            0
        );
    }
}
