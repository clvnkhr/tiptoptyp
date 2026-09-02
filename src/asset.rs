use std::{
    fs,
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc::{self, Receiver, RecvTimeoutError, Sender},
    },
    thread,
    time::Duration,
};

use crate::{
    compiler::{PreviewPage, rasterize_pdf},
    document::DocumentKind,
};
use eframe::egui;

const POLL_INTERVAL: Duration = Duration::from_millis(20);

#[derive(Debug)]
pub enum LoadedAsset {
    Image(PreviewPage),
    Pdf {
        bytes: Vec<u8>,
        pages: Vec<PreviewPage>,
    },
}

#[derive(Debug)]
pub struct AssetResult {
    pub token: u64,
    pub output: Result<LoadedAsset, String>,
}

#[derive(Debug)]
struct AssetRequest {
    token: u64,
    path: PathBuf,
    kind: DocumentKind,
}

/// Decodes images and rasterizes directly opened PDFs away from egui's frame
/// callback. In particular, a long PDF cannot freeze resizing or leave the UI
/// in a modal-looking state while Poppler is working.
pub struct AssetLoader {
    requests: Option<Sender<AssetRequest>>,
    results: Receiver<AssetResult>,
    worker: Option<thread::JoinHandle<()>>,
    shutdown: Arc<AtomicBool>,
    latest_token: Arc<AtomicU64>,
}

impl AssetLoader {
    pub fn new(context: egui::Context) -> Self {
        let (request_tx, request_rx) = mpsc::channel();
        let (result_tx, result_rx) = mpsc::channel();
        let shutdown = Arc::new(AtomicBool::new(false));
        let latest_token = Arc::new(AtomicU64::new(0));
        let worker_shutdown = shutdown.clone();
        let worker_latest = latest_token.clone();
        let worker = thread::Builder::new()
            .name("tiptoptyp-asset-loader".to_owned())
            .spawn(move || {
                worker_loop(
                    request_rx,
                    result_tx,
                    context,
                    worker_shutdown,
                    worker_latest,
                );
            })
            .expect("failed to start asset loader");
        Self {
            requests: Some(request_tx),
            results: result_rx,
            worker: Some(worker),
            shutdown,
            latest_token,
        }
    }

    pub fn request(&self, token: u64, path: PathBuf, kind: DocumentKind) -> Result<(), String> {
        debug_assert!(kind.preview_only());
        self.latest_token.store(token, Ordering::Release);
        self.requests
            .as_ref()
            .ok_or_else(|| "The image/PDF loader has stopped".to_owned())?
            .send(AssetRequest { token, path, kind })
            .map_err(|_| "The image/PDF loader stopped unexpectedly".to_owned())
    }

    pub fn cancel_before(&self, token: u64) {
        self.latest_token.store(token, Ordering::Release);
    }

    pub fn try_recv(&self) -> Option<AssetResult> {
        self.results.try_recv().ok()
    }
}

impl Drop for AssetLoader {
    fn drop(&mut self) {
        self.shutdown.store(true, Ordering::Release);
        self.requests.take();
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

fn worker_loop(
    requests: Receiver<AssetRequest>,
    results: Sender<AssetResult>,
    context: egui::Context,
    shutdown: Arc<AtomicBool>,
    latest_token: Arc<AtomicU64>,
) {
    while !shutdown.load(Ordering::Acquire) {
        let mut request = match requests.recv_timeout(POLL_INTERVAL) {
            Ok(request) => request,
            Err(RecvTimeoutError::Timeout) => continue,
            Err(RecvTimeoutError::Disconnected) => break,
        };
        while let Ok(newer) = requests.try_recv() {
            request = newer;
        }

        let cancelled = || {
            shutdown.load(Ordering::Acquire)
                || latest_token.load(Ordering::Acquire) != request.token
        };
        let output = match request.kind {
            DocumentKind::Image => load_image(&request.path, &cancelled),
            DocumentKind::Pdf => load_pdf(&request.path, cancelled),
            DocumentKind::Typst | DocumentKind::Text => {
                Err("Only binary preview assets use the asset loader".to_owned())
            }
        };
        if latest_token.load(Ordering::Acquire) == request.token
            && results
                .send(AssetResult {
                    token: request.token,
                    output,
                })
                .is_ok()
        {
            context.request_repaint();
        }
    }
}

fn load_image(path: &PathBuf, cancelled: &impl Fn() -> bool) -> Result<LoadedAsset, String> {
    let encoded = fs::read(path)
        .map_err(|error| format!("Could not read image {}: {error}", path.display()))?;
    if cancelled() {
        return Err("Image loading was superseded by another file".to_owned());
    }
    let decoded = image::load_from_memory(&encoded)
        .map_err(|error| format!("Could not decode image {}: {error}", path.display()))?
        .into_rgba8();
    let (width, height) = decoded.dimensions();
    Ok(LoadedAsset::Image(PreviewPage {
        size: [width as usize, height as usize],
        rgba: decoded.into_raw(),
        links: Vec::new(),
    }))
}

fn load_pdf(path: &PathBuf, mut cancelled: impl FnMut() -> bool) -> Result<LoadedAsset, String> {
    let bytes = fs::read(path)
        .map_err(|error| format!("Could not read PDF {}: {error}", path.display()))?;
    if cancelled() {
        return Err("PDF loading was superseded by another file".to_owned());
    }
    let pages = rasterize_pdf(&bytes, &mut cancelled)?;
    Ok(LoadedAsset::Pdf { bytes, pages })
}
