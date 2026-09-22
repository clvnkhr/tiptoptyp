//! Shared build service. Engine adapters own processes and translate output;
//! this owner schedules requests and publishes canonical PDF artifacts.
mod typst;

use crate::{
    diagnostics::DiagnosticReport,
    language_support::BuildEngineKind,
    pdf::{PdfDocumentCatalog, inspect_pdf_with_program},
    worker::{LatestReceiver, LatestSender, latest_channel},
};
use std::{
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc::{self, Receiver, RecvTimeoutError, Sender},
    },
    thread,
    time::Duration,
};
use tiptoptyp_core::document::TypesettingLanguage;

const WORKER_POLL_INTERVAL: Duration = Duration::from_millis(15);
pub(crate) use typst::TypstOptions;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum EngineConfig {
    Typst(TypstOptions),
}
impl EngineConfig {
    pub(crate) fn kind(&self) -> BuildEngineKind {
        match self {
            Self::Typst(_) => BuildEngineKind::Typst,
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) struct CompileInput {
    pub(crate) language: TypesettingLanguage,
    pub(crate) source: String,
    pub(crate) source_dir: PathBuf,
    pub(crate) project_root: PathBuf,
    pub(crate) display_name: String,
}

#[derive(Debug, Clone)]
pub(crate) struct CompileRequest {
    pub(crate) revision: u64,
    /// Export/PDF.js builds publish canonical bytes without PDF inspection.
    pub(crate) rasterize: bool,
    pub(crate) input: CompileInput,
    pub(crate) engine: EngineConfig,
}
impl CompileRequest {
    fn validate(&self) -> Result<(), String> {
        if self.input.language == self.engine.kind().language() {
            Ok(())
        } else {
            Err(format!(
                "The {:?} engine cannot compile {:?} source",
                self.engine.kind(),
                self.input.language
            ))
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) struct CompileArtifact {
    pub(crate) key: ArtifactKey,
    pub(crate) pdf: Arc<[u8]>,
    pub(crate) diagnostics: DiagnosticReport,
}

pub(crate) use tiptoptyp_core::preview::ArtifactKey;

#[derive(Debug)]
pub(crate) enum CompileEvent {
    Started,
    Failed(DiagnosticReport),
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
pub(crate) struct CompileResult {
    pub(crate) revision: u64,
    pub(crate) elapsed: Duration,
    pub(crate) event: CompileEvent,
}

enum CompilerCommand {
    Request(CompileRequest),
    Pause,
}

pub(crate) struct Compiler {
    requests: Option<LatestSender<CompilerCommand>>,
    results: Receiver<CompileResult>,
    disconnected: AtomicBool,
    worker: Option<thread::JoinHandle<()>>,
    shutdown: Arc<AtomicBool>,
    latest_revision: Arc<AtomicU64>,
}

impl Compiler {
    pub(crate) fn new(context: crate::worker::RepaintTarget) -> Self {
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

    pub(crate) fn request(&self, request: CompileRequest) -> Result<(), String> {
        self.latest_revision
            .store(request.revision, Ordering::Release);
        self.requests
            .as_ref()
            .ok_or_else(|| "The preview worker has stopped".to_owned())?
            .send(CompilerCommand::Request(request))
            .map_err(|_| "The preview worker stopped unexpectedly".to_owned())
    }

    pub(crate) fn pause(&self, revision: u64) -> Result<(), String> {
        // Interrupt PDF inspection before asking the worker to retire its backend.
        self.latest_revision
            .store(revision.wrapping_add(1), Ordering::Release);
        self.requests
            .as_ref()
            .ok_or_else(|| "The preview worker has stopped".to_owned())?
            .send(CompilerCommand::Pause)
            .map_err(|_| "The preview worker stopped unexpectedly".to_owned())
    }

    pub(crate) fn try_recv(&self) -> Option<CompileResult> {
        Some(
            crate::worker::poll_service(&self.results, &self.disconnected)?.unwrap_or_else(|()| {
                CompileResult {
                    revision: self.latest_revision.load(Ordering::Acquire),
                    elapsed: Duration::ZERO,
                    event: CompileEvent::Failed(DiagnosticReport::error(
                        "The preview worker stopped unexpectedly".to_owned(),
                    )),
                }
            }),
        )
    }
}

impl Drop for Compiler {
    fn drop(&mut self) {
        // Closing the only request sender wakes the worker. Joining guarantees
        // the backend drops and reaps its process before the app exits.
        self.shutdown.store(true, Ordering::Release);
        self.requests.take();
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

/// Internal adapter output; no compiler log syntax crosses this boundary.
struct EngineResult {
    revision: u64,
    elapsed: Duration,
    event: EngineEvent,
}
enum EngineEvent {
    Started,
    Failed(DiagnosticReport),
    Pdf {
        pdf: Arc<[u8]>,
        project_root: PathBuf,
        diagnostics: DiagnosticReport,
        rasterize: bool,
    },
}

enum Backend {
    Typst(typst::Backend),
}
impl Backend {
    fn kind(&self) -> BuildEngineKind {
        match self {
            Self::Typst(_) => BuildEngineKind::Typst,
        }
    }
    fn for_config(config: &EngineConfig) -> Self {
        match config {
            EngineConfig::Typst(_) => Self::Typst(typst::Backend::new()),
        }
    }
    fn submit(&mut self, request: &CompileRequest) -> Result<(), String> {
        match (self, &request.engine) {
            (Self::Typst(backend), EngineConfig::Typst(options)) => {
                backend.submit(request, options)
            }
        }
    }
    fn poll(&mut self) -> Vec<EngineResult> {
        match self {
            Self::Typst(backend) => backend.poll(),
        }
    }
    fn is_running(&self) -> bool {
        match self {
            Self::Typst(backend) => backend.is_running(),
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
    let mut next_artifact_generation = 1_u64;
    let mut backend: Option<Backend> = None;
    loop {
        let mut poll = |backend: &mut Option<Backend>| {
            if let Some(backend) = backend {
                for result in backend.poll() {
                    publish_engine_result(
                        result,
                        &results,
                        &context,
                        &shutdown,
                        &latest_revision,
                        &mut next_artifact_generation,
                    );
                }
            }
        };
        poll(&mut backend);
        // Paused/idle workers sleep on the queue; only live processes need polling.
        let command = if backend.as_ref().is_some_and(Backend::is_running) {
            requests.recv_timeout(WORKER_POLL_INTERVAL)
        } else {
            requests.recv().map_err(|_| RecvTimeoutError::Disconnected)
        };
        match command {
            Ok(CompilerCommand::Pause) => backend = None,
            Ok(CompilerCommand::Request(request)) => {
                poll(&mut backend);
                let submitted = request.validate().and_then(|()| {
                    if backend
                        .as_ref()
                        .is_some_and(|backend| backend.kind() != request.engine.kind())
                    {
                        backend = None;
                    }
                    backend
                        .get_or_insert_with(|| Backend::for_config(&request.engine))
                        .submit(&request)
                });
                if let Err(error) = submitted {
                    // No old process may continue publishing after a rejected build.
                    backend = None;
                    send_result(
                        &results,
                        &context,
                        CompileResult {
                            revision: request.revision,
                            elapsed: Duration::ZERO,
                            event: CompileEvent::Failed(DiagnosticReport::error(error)),
                        },
                    );
                }
            }
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => break,
        }
    }
}

fn publish_engine_result(
    result: EngineResult,
    results: &Sender<CompileResult>,
    context: &crate::worker::RepaintTarget,
    shutdown: &AtomicBool,
    latest_revision: &AtomicU64,
    next_artifact_generation: &mut u64,
) {
    let event = match result.event {
        EngineEvent::Started => CompileEvent::Started,
        EngineEvent::Failed(report) => CompileEvent::Failed(report),
        EngineEvent::Pdf {
            pdf,
            project_root,
            diagnostics,
            rasterize,
        } => {
            let key = ArtifactKey {
                revision: result.revision,
                generation: *next_artifact_generation,
            };
            *next_artifact_generation = (*next_artifact_generation).wrapping_add(1).max(1);
            publish_compiled_artifact(
                pdf,
                &project_root,
                diagnostics,
                key,
                result.elapsed,
                rasterize,
                shutdown,
                latest_revision,
                Path::new("pdfinfo"),
                results,
                context,
            );
            return;
        }
    };
    send_result(
        results,
        context,
        CompileResult {
            revision: result.revision,
            elapsed: result.elapsed,
            event,
        },
    );
}

#[allow(clippy::too_many_arguments)]
fn publish_compiled_artifact(
    pdf: Arc<[u8]>,
    project_root: &Path,
    diagnostics: DiagnosticReport,
    key: ArtifactKey,
    elapsed: Duration,
    rasterize: bool,
    shutdown: &AtomicBool,
    latest_revision: &AtomicU64,
    inspector: &Path,
    results: &Sender<CompileResult>,
    context: &crate::worker::RepaintTarget,
) {
    // The backend already snapshotted its output. Export and optional page
    // inspection share these bytes even if a later build replaces the file.
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
        ArtifactKey, CompileEvent, CompileInput, CompileRequest, CompileResult, Compiler,
        CompilerCommand, EngineConfig, TypstOptions, publish_compiled_artifact, worker_loop,
    };
    use crate::{diagnostics::DiagnosticReport, worker::latest_channel};
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
    use tiptoptyp_core::document::TypesettingLanguage;

    #[test]
    fn mismatched_engine_is_rejected_before_touching_the_project_or_executable() {
        let project = tempfile::tempdir().unwrap();
        let (tx, rx) = latest_channel();
        let (results, received) = mpsc::channel();
        tx.send(CompilerCommand::Request(CompileRequest {
            revision: 19,
            rasterize: false,
            input: CompileInput {
                language: TypesettingLanguage::Tex,
                source: "\\documentclass{article}".to_owned(),
                source_dir: project.path().join("missing"),
                project_root: project.path().to_owned(),
                display_name: "paper.tex".to_owned(),
            },
            engine: EngineConfig::Typst(TypstOptions {
                executable: "must-not-run".into(),
                font_paths: Vec::new(),
            }),
        }))
        .unwrap();
        drop(tx);
        worker_loop(
            rx,
            results,
            crate::worker::RepaintTarget::test(),
            Arc::new(AtomicBool::new(false)),
            Arc::new(AtomicU64::new(19)),
        );
        let result = received.recv().unwrap();
        assert_eq!(result.revision, 19);
        let CompileEvent::Failed(report) = result.event else {
            panic!("expected mismatch failure")
        };
        assert_eq!(report.raw, "The Typst engine cannot compile Tex source");
        assert_eq!(
            report.diagnostics[0].source,
            crate::diagnostics::DiagnosticSource::Global
        );
        assert!(received.try_recv().is_err());
        assert_eq!(fs::read_dir(project.path()).unwrap().count(), 0);
    }

    #[test]
    fn engine_pdf_events_share_exact_bytes_and_get_distinct_generations_without_inspection() {
        use super::{EngineEvent, EngineResult, publish_engine_result};
        let (tx, rx) = mpsc::channel();
        let pdf: Arc<[u8]> = Arc::from(b"%PDF-engine-output\0".as_slice());
        let mut generation = 12;
        for _ in 0..2 {
            publish_engine_result(
                EngineResult {
                    revision: 7,
                    elapsed: Duration::from_millis(4),
                    event: EngineEvent::Pdf {
                        pdf: pdf.clone(),
                        project_root: PathBuf::from("missing-no-inspection-allowed"),
                        diagnostics: DiagnosticReport::default(),
                        rasterize: false,
                    },
                },
                &tx,
                &crate::worker::RepaintTarget::test(),
                &AtomicBool::new(false),
                &AtomicU64::new(7),
                &mut generation,
            );
        }
        let results: Vec<_> = rx.try_iter().collect();
        assert_eq!(
            results.len(),
            2,
            "PDF.js/export must not invoke optional metadata tools"
        );
        for (result, expected) in results.iter().zip([12, 13]) {
            let CompileEvent::Artifact(artifact) = &result.event else {
                panic!("expected canonical artifact")
            };
            assert!(
                Arc::ptr_eq(&pdf, &artifact.pdf),
                "publication must not copy or reconstruct the PDF"
            );
            assert_eq!(
                artifact.key,
                ArtifactKey {
                    revision: 7,
                    generation: expected
                }
            );
        }
        assert_eq!(generation, 14);
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
                        input: CompileInput {
                            language: TypesettingLanguage::Typst,

                            source: String::new(),
                            // Resolve fails deterministically before starting a process.
                            source_dir: project.path().join("missing"),
                            project_root: project.path().to_path_buf(),
                            display_name: "main.typ".to_owned(),
                        },

                        engine: EngineConfig::Typst(TypstOptions {
                            executable: PathBuf::from("unused-typst"),
                            font_paths: Vec::new(),
                        }),
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
            Arc::from(fs::read(&pdf_path).unwrap()),
            project.path(),
            DiagnosticReport {
                raw: "warning diagnostic".to_owned(),
                diagnostics: Vec::new(),
            },
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
        assert_eq!(artifact.diagnostics.raw, "warning diagnostic");
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
                Arc::from(fs::read(&pdf_path).unwrap()),
                project.path(),
                DiagnosticReport::default(),
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
            input: CompileInput {
                language: TypesettingLanguage::Typst,

                source: source.to_owned(),
                source_dir: root.path().to_path_buf(),
                project_root: root.path().to_path_buf(),
                display_name: "integration.typ".to_owned(),
            },

            engine: EngineConfig::Typst(TypstOptions {
                executable: typst_executable.clone(),
                font_paths: Vec::new(),
            }),
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
        assert!(error.raw.contains("expected expression"), "{error:?}");

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
