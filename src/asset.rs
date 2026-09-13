use crate::worker::{LatestReceiver, LatestSender, latest_channel};
use std::{
    collections::VecDeque,
    fs,
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc::{self, Receiver, Sender},
    },
    thread,
    time::SystemTime,
};

use crate::{
    compiler::{PreviewPage, rasterize_pdf, rasterize_pdf_first_page},
    document::DocumentKind,
    private_workspace::project_root_for_path,
};

macro_rules! request_token {
    ($name:ident) => {
        #[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
        pub(crate) struct $name(u64);
        impl $name {
            pub(crate) fn advance(&mut self) {
                self.0 = self.0.wrapping_add(1).max(1);
            }
        }
        impl std::fmt::Display for $name {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                self.0.fmt(f)
            }
        }
    };
}
request_token!(AssetToken);
request_token!(ThumbnailToken);

const THUMBNAIL_CACHE_CAPACITY: usize = 8;
pub(crate) const ASSET_THUMBNAIL_MAX_DIMENSION: u32 = 720;

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
    pub token: AssetToken,
    pub output: Result<LoadedAsset, String>,
}

#[derive(Debug)]
struct AssetRequest {
    token: AssetToken,
    path: PathBuf,
    kind: DocumentKind,
}

/// Decodes images and rasterizes directly opened PDFs away from egui's frame
/// callback. In particular, a long PDF cannot freeze resizing or leave the UI
/// in a modal-looking state while Poppler is working.
pub struct AssetLoader {
    requests: Option<LatestSender<AssetRequest>>,
    results: Receiver<AssetResult>,
    disconnected: AtomicBool,
    worker: Option<thread::JoinHandle<()>>,
    shutdown: Arc<AtomicBool>,
    latest_token: Arc<AtomicU64>,
}

impl AssetLoader {
    pub fn new(context: crate::worker::RepaintTarget) -> Self {
        let (request_tx, request_rx) = latest_channel();
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
            .ok();
        Self {
            requests: worker.as_ref().map(|_| request_tx),
            results: result_rx,
            disconnected: AtomicBool::new(false),
            worker,
            shutdown,
            latest_token,
        }
    }

    pub fn request(
        &self,
        token: AssetToken,
        path: PathBuf,
        kind: DocumentKind,
    ) -> Result<(), String> {
        debug_assert!(kind.preview_only());
        self.latest_token.store(token.0, Ordering::Release);
        self.requests
            .as_ref()
            .ok_or_else(|| "The image/PDF loader has stopped".to_owned())?
            .send(AssetRequest { token, path, kind })
            .map_err(|_| "The image/PDF loader stopped unexpectedly".to_owned())
    }

    pub fn cancel_before(&self, token: AssetToken) {
        self.latest_token.store(token.0, Ordering::Release);
    }

    pub fn try_recv(&self) -> Option<AssetResult> {
        Some(
            crate::worker::poll_service(&self.results, &self.disconnected)?.unwrap_or_else(|()| {
                AssetResult {
                    token: AssetToken(self.latest_token.load(Ordering::Acquire)),
                    output: Err("The image/PDF loader stopped unexpectedly".to_owned()),
                }
            }),
        )
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

#[derive(Clone, Debug)]
pub(crate) struct AssetThumbnail {
    pub(crate) size: [usize; 2],
    pub(crate) rgba: Arc<[u8]>,
}

#[derive(Debug)]
pub(crate) struct AssetThumbnailResult {
    pub(crate) token: ThumbnailToken,
    pub(crate) path: PathBuf,
    pub(crate) kind: DocumentKind,
    pub(crate) output: Result<AssetThumbnail, String>,
}

#[derive(Debug)]
struct AssetThumbnailRequest {
    token: ThumbnailToken,
    path: PathBuf,
    kind: DocumentKind,
}

/// Loads small, single-page hover previews independently from the selected
/// document loader. Work is serial and latest-wins so rapidly crossing files
/// cannot accumulate a queue of image decodes or Poppler children.
pub(crate) struct AssetThumbnailLoader {
    requests: Option<LatestSender<AssetThumbnailRequest>>,
    results: Receiver<AssetThumbnailResult>,
    disconnected: AtomicBool,
    worker: Option<thread::JoinHandle<()>>,
    shutdown: Arc<AtomicBool>,
    latest_token: Arc<AtomicU64>,
}

impl AssetThumbnailLoader {
    pub(crate) fn new(context: crate::worker::RepaintTarget) -> Self {
        let (request_tx, request_rx) = latest_channel();
        let (result_tx, result_rx) = mpsc::channel();
        let shutdown = Arc::new(AtomicBool::new(false));
        let latest_token = Arc::new(AtomicU64::new(0));
        let worker_shutdown = shutdown.clone();
        let worker_latest = latest_token.clone();
        let worker = thread::Builder::new()
            .name("tiptoptyp-asset-thumbnail".to_owned())
            .spawn(move || {
                thumbnail_worker_loop(
                    request_rx,
                    result_tx,
                    context,
                    worker_shutdown,
                    worker_latest,
                    ASSET_THUMBNAIL_MAX_DIMENSION,
                );
            })
            .ok();
        Self {
            requests: worker.as_ref().map(|_| request_tx),
            results: result_rx,
            disconnected: AtomicBool::new(false),
            worker,
            shutdown,
            latest_token,
        }
    }

    pub(crate) fn request(
        &self,
        token: ThumbnailToken,
        path: PathBuf,
        kind: DocumentKind,
    ) -> Result<(), String> {
        debug_assert!(kind.preview_only());
        self.latest_token.store(token.0, Ordering::Release);
        self.requests
            .as_ref()
            .ok_or_else(|| "The asset thumbnail loader has stopped".to_owned())?
            .send(AssetThumbnailRequest { token, path, kind })
            .map_err(|_| "The asset thumbnail loader stopped unexpectedly".to_owned())
    }

    pub(crate) fn cancel_before(&self, token: ThumbnailToken) {
        self.latest_token.store(token.0, Ordering::Release);
    }

    pub(crate) fn try_recv(&self) -> Option<Result<AssetThumbnailResult, String>> {
        crate::worker::poll_service(&self.results, &self.disconnected).map(|result| {
            result.map_err(|()| "The thumbnail loader stopped unexpectedly".to_owned())
        })
    }
}

impl Drop for AssetThumbnailLoader {
    fn drop(&mut self) {
        self.shutdown.store(true, Ordering::Release);
        self.requests.take();
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct AssetFingerprint {
    path: PathBuf,
    length: u64,
    modified: Option<SystemTime>,
}

impl AssetFingerprint {
    fn read(path: &Path) -> Result<Self, String> {
        let metadata = fs::metadata(path)
            .map_err(|error| format!("Could not inspect asset {}: {error}", path.display()))?;
        Ok(Self {
            path: path.to_path_buf(),
            length: metadata.len(),
            modified: metadata.modified().ok(),
        })
    }
}

#[derive(Default)]
struct ThumbnailCache {
    entries: VecDeque<(AssetFingerprint, AssetThumbnail)>,
}

impl ThumbnailCache {
    fn get(&mut self, key: &AssetFingerprint) -> Option<AssetThumbnail> {
        let index = self.entries.iter().position(|(cached, _)| cached == key)?;
        let entry = self.entries.remove(index)?;
        let thumbnail = entry.1.clone();
        self.entries.push_back(entry);
        Some(thumbnail)
    }

    fn insert(&mut self, key: AssetFingerprint, thumbnail: AssetThumbnail) {
        self.entries.retain(|(cached, _)| cached.path != key.path);
        self.entries.push_back((key, thumbnail));
        while self.entries.len() > THUMBNAIL_CACHE_CAPACITY {
            self.entries.pop_front();
        }
    }
}

fn thumbnail_worker_loop(
    requests: LatestReceiver<AssetThumbnailRequest>,
    results: Sender<AssetThumbnailResult>,
    context: crate::worker::RepaintTarget,
    shutdown: Arc<AtomicBool>,
    latest_token: Arc<AtomicU64>,
    max_dimension: u32,
) {
    let mut cache = ThumbnailCache::default();
    while let Ok(request) = requests.recv() {
        let request = take_latest_thumbnail_queued(request, &requests);
        if shutdown.load(Ordering::Acquire) {
            break;
        }

        let cancelled = || {
            shutdown.load(Ordering::Acquire)
                || latest_token.load(Ordering::Acquire) != request.token.0
        };
        let output = AssetFingerprint::read(&request.path).and_then(|fingerprint| {
            if let Some(thumbnail) = cache.get(&fingerprint) {
                return Ok(thumbnail);
            }
            let thumbnail = load_thumbnail(&request.path, request.kind, max_dimension, &cancelled)?;
            cache.insert(fingerprint, thumbnail.clone());
            Ok(thumbnail)
        });

        if latest_token.load(Ordering::Acquire) == request.token.0
            && results
                .send(AssetThumbnailResult {
                    token: request.token,
                    path: request.path,
                    kind: request.kind,
                    output,
                })
                .is_ok()
        {
            context.request_repaint();
        }
    }
}

fn take_latest_thumbnail_queued(
    mut request: AssetThumbnailRequest,
    requests: &LatestReceiver<AssetThumbnailRequest>,
) -> AssetThumbnailRequest {
    while let Ok(newer) = requests.try_recv() {
        request = newer;
    }
    request
}

fn load_thumbnail(
    path: &Path,
    kind: DocumentKind,
    max_dimension: u32,
    cancelled: &impl Fn() -> bool,
) -> Result<AssetThumbnail, String> {
    match kind {
        DocumentKind::Image => load_image_thumbnail(path, max_dimension, cancelled),
        DocumentKind::Pdf => load_pdf_thumbnail(path, max_dimension, cancelled),
        DocumentKind::Typst | DocumentKind::Text => {
            Err("Only image and PDF files have hover thumbnails".to_owned())
        }
    }
}

fn load_image_thumbnail(
    path: &Path,
    max_dimension: u32,
    cancelled: &impl Fn() -> bool,
) -> Result<AssetThumbnail, String> {
    let encoded = fs::read(path)
        .map_err(|error| format!("Could not read image {}: {error}", path.display()))?;
    if cancelled() {
        return Err("Image thumbnail loading was superseded".to_owned());
    }
    let decoded = image::load_from_memory(&encoded)
        .map_err(|error| format!("Could not decode image {}: {error}", path.display()))?;
    if cancelled() {
        return Err("Image thumbnail loading was superseded".to_owned());
    }
    let (width, height) = thumbnail_dimensions(decoded.width(), decoded.height(), max_dimension);
    let decoded = if (width, height) == (decoded.width(), decoded.height()) {
        decoded.into_rgba8()
    } else {
        decoded
            .resize_exact(width, height, image::imageops::FilterType::Triangle)
            .into_rgba8()
    };
    if cancelled() {
        return Err("Image thumbnail loading was superseded".to_owned());
    }
    Ok(AssetThumbnail {
        size: [width as usize, height as usize],
        rgba: decoded.into_raw().into(),
    })
}

fn load_pdf_thumbnail(
    path: &Path,
    max_dimension: u32,
    cancelled: &impl Fn() -> bool,
) -> Result<AssetThumbnail, String> {
    let bytes = fs::read(path)
        .map_err(|error| format!("Could not read PDF {}: {error}", path.display()))?;
    if cancelled() {
        return Err("PDF thumbnail loading was superseded".to_owned());
    }
    let project_root = project_root_for_path(path).map_err(|error| {
        format!(
            "Could not locate private workspace storage for {}: {error}",
            path.display()
        )
    })?;
    let page = rasterize_pdf_first_page(&bytes, &project_root, max_dimension, cancelled)?;
    thumbnail_from_preview_page(page, max_dimension)
}

fn thumbnail_from_preview_page(
    page: PreviewPage,
    max_dimension: u32,
) -> Result<AssetThumbnail, String> {
    let width = u32::try_from(page.size[0])
        .map_err(|_| "The preview page is too wide for an image thumbnail".to_owned())?;
    let height = u32::try_from(page.size[1])
        .map_err(|_| "The preview page is too tall for an image thumbnail".to_owned())?;
    let expected = page.size[0]
        .checked_mul(page.size[1])
        .and_then(|pixels| pixels.checked_mul(4));
    if expected != Some(page.rgba.len()) {
        return Err("The preview page contained invalid RGBA pixel data".to_owned());
    }
    let (bounded_width, bounded_height) = thumbnail_dimensions(width, height, max_dimension);
    if (bounded_width, bounded_height) == (width, height) {
        return Ok(AssetThumbnail {
            size: page.size,
            rgba: page.rgba.into(),
        });
    }
    let image = image::RgbaImage::from_raw(width, height, page.rgba)
        .ok_or_else(|| "The preview page contained invalid RGBA pixel data".to_owned())?;
    let image = image::imageops::resize(
        &image,
        bounded_width,
        bounded_height,
        image::imageops::FilterType::Triangle,
    );
    Ok(AssetThumbnail {
        size: [bounded_width as usize, bounded_height as usize],
        rgba: image.into_raw().into(),
    })
}

fn thumbnail_dimensions(width: u32, height: u32, max_dimension: u32) -> (u32, u32) {
    let max_dimension = max_dimension.max(1);
    let longest = width.max(height);
    if longest == 0 {
        return (1, 1);
    }
    if longest <= max_dimension {
        return (width.max(1), height.max(1));
    }
    let scale = f64::from(max_dimension) / f64::from(longest);
    (
        (f64::from(width) * scale).round().max(1.0) as u32,
        (f64::from(height) * scale).round().max(1.0) as u32,
    )
}

fn worker_loop(
    requests: LatestReceiver<AssetRequest>,
    results: Sender<AssetResult>,
    context: crate::worker::RepaintTarget,
    shutdown: Arc<AtomicBool>,
    latest_token: Arc<AtomicU64>,
) {
    while let Ok(request) = requests.recv() {
        let request = take_latest_queued(request, &requests);
        if shutdown.load(Ordering::Acquire) {
            break;
        }

        let cancelled = || {
            shutdown.load(Ordering::Acquire)
                || latest_token.load(Ordering::Acquire) != request.token.0
        };
        let output = match request.kind {
            DocumentKind::Image => load_image(&request.path, &cancelled),
            DocumentKind::Pdf => load_pdf(&request.path, cancelled),
            DocumentKind::Typst | DocumentKind::Text => {
                Err("Only binary preview assets use the asset loader".to_owned())
            }
        };
        if latest_token.load(Ordering::Acquire) == request.token.0
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

fn take_latest_queued(
    mut request: AssetRequest,
    requests: &LatestReceiver<AssetRequest>,
) -> AssetRequest {
    while let Ok(newer) = requests.try_recv() {
        request = newer;
    }
    request
}

fn load_image(path: &Path, cancelled: &impl Fn() -> bool) -> Result<LoadedAsset, String> {
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

fn load_pdf(path: &Path, mut cancelled: impl FnMut() -> bool) -> Result<LoadedAsset, String> {
    let bytes = fs::read(path)
        .map_err(|error| format!("Could not read PDF {}: {error}", path.display()))?;
    if cancelled() {
        return Err("PDF loading was superseded by another file".to_owned());
    }
    let project_root = project_root_for_path(path).map_err(|error| {
        format!(
            "Could not locate private workspace storage for {}: {error}",
            path.display()
        )
    })?;
    let pages = rasterize_pdf(&bytes, &project_root, &mut cancelled)?;
    Ok(LoadedAsset::Pdf { bytes, pages })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request(token: u64) -> AssetRequest {
        AssetRequest {
            token: AssetToken(token),
            path: PathBuf::from(format!("asset-{token}.pdf")),
            kind: DocumentKind::Pdf,
        }
    }

    #[test]
    fn queued_asset_requests_collapse_to_the_newest() {
        let (sender, receiver) = latest_channel();
        sender.send(request(2)).unwrap();
        sender.send(request(3)).unwrap();

        let latest = take_latest_queued(request(1), &receiver);

        assert_eq!(latest.token.0, 3);
        assert_eq!(latest.path, PathBuf::from("asset-3.pdf"));
    }

    fn thumbnail_request(token: u64) -> AssetThumbnailRequest {
        AssetThumbnailRequest {
            token: ThumbnailToken(token),
            path: PathBuf::from(format!("thumbnail-{token}.png")),
            kind: DocumentKind::Image,
        }
    }

    #[test]
    fn queued_thumbnail_requests_collapse_to_the_newest() {
        let (sender, receiver) = latest_channel();
        sender.send(thumbnail_request(2)).unwrap();
        sender.send(thumbnail_request(3)).unwrap();

        let latest = take_latest_thumbnail_queued(thumbnail_request(1), &receiver);

        assert_eq!(latest.token.0, 3);
        assert_eq!(latest.path, PathBuf::from("thumbnail-3.png"));
    }

    #[test]
    fn thumbnail_dimensions_preserve_aspect_ratio_without_upscaling() {
        assert_eq!(thumbnail_dimensions(1600, 800, 400), (400, 200));
        assert_eq!(thumbnail_dimensions(800, 1600, 400), (200, 400));
        assert_eq!(thumbnail_dimensions(120, 60, 400), (120, 60));
        assert_eq!(thumbnail_dimensions(0, 0, 400), (1, 1));
    }

    #[test]
    fn image_thumbnails_are_bounded_before_entering_the_cache() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("wide.png");
        image::RgbaImage::from_pixel(120, 60, image::Rgba([12, 34, 56, 255]))
            .save(&path)
            .unwrap();

        let thumbnail = load_image_thumbnail(&path, 40, &|| false).unwrap();

        assert_eq!(thumbnail.size, [40, 20]);
        assert_eq!(thumbnail.rgba.len(), 40 * 20 * 4);
    }

    #[test]
    fn rasterizer_output_is_defensively_bounded_before_caching() {
        let page = PreviewPage {
            size: [120, 60],
            rgba: vec![255; 120 * 60 * 4],
            links: Vec::new(),
        };

        let thumbnail = thumbnail_from_preview_page(page, 40).unwrap();

        assert_eq!(thumbnail.size, [40, 20]);
        assert_eq!(thumbnail.rgba.len(), 40 * 20 * 4);
    }

    #[test]
    fn thumbnail_cache_is_lru_bounded_and_replaces_changed_paths() {
        let mut cache = ThumbnailCache::default();
        let thumbnail = AssetThumbnail {
            size: [1, 1],
            rgba: Arc::from([0, 0, 0, 0]),
        };
        for index in 0..=THUMBNAIL_CACHE_CAPACITY {
            cache.insert(
                AssetFingerprint {
                    path: PathBuf::from(format!("asset-{index}.png")),
                    length: index as u64,
                    modified: None,
                },
                thumbnail.clone(),
            );
        }
        assert_eq!(cache.entries.len(), THUMBNAIL_CACHE_CAPACITY);
        assert!(
            cache
                .entries
                .iter()
                .all(|(entry, _)| entry.path != Path::new("asset-0.png"))
        );

        cache.insert(
            AssetFingerprint {
                path: PathBuf::from("asset-1.png"),
                length: 999,
                modified: None,
            },
            thumbnail,
        );
        assert_eq!(cache.entries.len(), THUMBNAIL_CACHE_CAPACITY);
        assert_eq!(
            cache
                .entries
                .iter()
                .filter(|(entry, _)| entry.path == Path::new("asset-1.png"))
                .count(),
            1
        );
    }
}
