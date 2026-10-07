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

use crate::{document::DocumentKind, pdf::PreviewPage};

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
// One retained source avoids repeat image decoding on appearance switches.
// Large photos remain loadable without retaining an additional full raster.
const IMAGE_SOURCE_CACHE_MAX_BYTES: usize = 16 * 1024 * 1024;
pub(crate) const ASSET_THUMBNAIL_MAX_DIMENSION: u32 = 720;

#[derive(Debug)]
pub enum LoadedAsset {
    Image(PreviewPage),
    Pdf { bytes: Vec<u8> },
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
    dark: bool,
}

#[derive(Debug)]
enum AssetCommand {
    Load(AssetRequest),
    ReleaseCache,
}

impl AssetCommand {
    fn into_request(self, image_cache: &mut ImageSourceCache) -> Option<AssetRequest> {
        match self {
            Self::Load(request) => Some(request),
            Self::ReleaseCache => {
                image_cache.entry = None;
                None
            }
        }
    }
}

/// Decodes images and reads opened PDFs away from egui's frame callback.
/// Load images and immutable PDF bytes off the UI thread.
pub struct AssetLoader {
    requests: Option<LatestSender<AssetCommand>>,
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
        dark: bool,
    ) -> Result<(), String> {
        debug_assert!(kind.preview_only());
        self.latest_token.store(token.0, Ordering::Release);
        self.requests
            .as_ref()
            .ok_or_else(|| "The image/PDF loader has stopped".to_owned())?
            .send(AssetCommand::Load(AssetRequest {
                token,
                path,
                kind,
                dark,
            }))
            .map_err(|_| "The image/PDF loader stopped unexpectedly".to_owned())
    }

    pub fn cancel_before(&self, token: AssetToken) {
        self.latest_token.store(token.0, Ordering::Release);
        // Document changes release the old source even when no new asset is
        // opened. Wake the idle worker once; appearance requests do not cancel
        // and keep reusing the same source. A newer load may replace this command.
        if let Some(requests) = &self.requests {
            let _ = requests.send(AssetCommand::ReleaseCache);
        }
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
/// cannot accumulate a queue of image decodes or rendering jobs.
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
        DocumentKind::Typst | DocumentKind::Tex | DocumentKind::Text => {
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
    let page = crate::pdfium::thumbnail(&bytes, max_dimension, cancelled)?;
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

#[derive(Debug, PartialEq, Eq)]
struct ImageSourceKey {
    fingerprint: AssetFingerprint,
    target: PathBuf,
    created: Option<SystemTime>,
    #[cfg(unix)]
    identity: (u64, u64, i64, i64),
}

impl ImageSourceKey {
    fn read(path: &Path) -> Result<Self, String> {
        let metadata = fs::metadata(path)
            .map_err(|error| format!("Could not inspect image {}: {error}", path.display()))?;
        Self::from_metadata(path, &metadata)
    }

    fn from_metadata(path: &Path, metadata: &fs::Metadata) -> Result<Self, String> {
        let target = fs::canonicalize(path)
            .map_err(|error| format!("Could not resolve image {}: {error}", path.display()))?;
        #[cfg(unix)]
        let identity = {
            use std::os::unix::fs::MetadataExt;
            (
                metadata.dev(),
                metadata.ino(),
                metadata.ctime(),
                metadata.ctime_nsec(),
            )
        };
        Ok(Self {
            fingerprint: AssetFingerprint {
                path: path.to_path_buf(),
                length: metadata.len(),
                modified: metadata.modified().ok(),
            },
            target,
            created: metadata.created().ok(),
            #[cfg(unix)]
            identity,
        })
    }
}

#[derive(Default)]
struct ImageSourceCache {
    entry: Option<(ImageSourceKey, Arc<PreviewPage>)>,
}

fn retain_image_source(rgba_bytes: usize) -> bool {
    rgba_bytes <= IMAGE_SOURCE_CACHE_MAX_BYTES
}

impl ImageSourceCache {
    fn source(
        &mut self,
        path: &Path,
        cancelled: &impl Fn() -> bool,
    ) -> Result<Arc<PreviewPage>, String> {
        if cancelled() {
            return Err("Image loading was superseded by another file".into());
        }
        let key = match ImageSourceKey::read(path) {
            Ok(key) => key,
            Err(error) => {
                // A replacement may coalesce a cache-release command. Failed
                // inspection must still release the previous file's pixels.
                self.entry = None;
                return Err(error);
            }
        };
        if let Some((cached, source)) = &self.entry
            && cached == &key
        {
            return Ok(source.clone());
        }
        self.entry = None;
        let (decoded, decoded_key) = load_image(path, cancelled)?;
        let source = Arc::new(decoded);
        if cancelled() {
            return Err("Image loading was superseded by another file".into());
        }
        // A file or symlink replaced during decoding cannot label the pixels
        // with the earlier file's key. Display this completed result, but leave
        // it uncached so the next request observes the current source.
        if retain_image_source(source.rgba.len())
            && decoded_key == key
            && ImageSourceKey::read(path).ok().as_ref() == Some(&decoded_key)
        {
            self.entry = Some((key, source.clone()));
        }
        Ok(source)
    }

    fn load(
        &mut self,
        path: &Path,
        dark: bool,
        cancelled: &impl Fn() -> bool,
    ) -> Result<LoadedAsset, String> {
        let source = self.source(path, cancelled)?;
        Ok(LoadedAsset::Image(image_appearance(source, dark)))
    }
}

fn image_appearance(source: Arc<PreviewPage>, dark: bool) -> PreviewPage {
    let size = source.size;
    let rgba = if dark {
        crate::preview::dark_preview_rgba(&source.rgba)
    } else {
        // Uncached large images have sole ownership: keep their original raster
        // without a second full allocation. Retained sources remain immutable.
        Arc::try_unwrap(source).map_or_else(|source| source.rgba.clone(), |source| source.rgba)
    };
    PreviewPage { size, rgba }
}

fn worker_loop(
    requests: LatestReceiver<AssetCommand>,
    results: Sender<AssetResult>,
    context: crate::worker::RepaintTarget,
    shutdown: Arc<AtomicBool>,
    latest_token: Arc<AtomicU64>,
) {
    let mut image_cache = ImageSourceCache::default();
    while let Ok(command) = requests.recv() {
        let command = take_latest_queued(command, &requests);
        if shutdown.load(Ordering::Acquire) {
            break;
        }
        let Some(request) = command.into_request(&mut image_cache) else {
            continue;
        };

        let cancelled = || {
            shutdown.load(Ordering::Acquire)
                || latest_token.load(Ordering::Acquire) != request.token.0
        };
        if request.kind != DocumentKind::Image {
            image_cache.entry = None;
        }
        let output = match request.kind {
            DocumentKind::Image => image_cache.load(&request.path, request.dark, &cancelled),
            DocumentKind::Pdf => load_pdf(&request.path, cancelled),
            DocumentKind::Typst | DocumentKind::Tex | DocumentKind::Text => {
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
    mut request: AssetCommand,
    requests: &LatestReceiver<AssetCommand>,
) -> AssetCommand {
    while let Ok(newer) = requests.try_recv() {
        request = newer;
    }
    request
}

fn load_image(
    path: &Path,
    cancelled: &impl Fn() -> bool,
) -> Result<(PreviewPage, ImageSourceKey), String> {
    use std::io::Read;
    // Read pixels and identity from the same open file, so changing a symlink
    // between metadata lookup and reading cannot cache another target's pixels.
    let mut file = fs::File::open(path)
        .map_err(|error| format!("Could not read image {}: {error}", path.display()))?;
    let metadata = file
        .metadata()
        .map_err(|error| format!("Could not inspect image {}: {error}", path.display()))?;
    let key = ImageSourceKey::from_metadata(path, &metadata)?;
    let mut encoded = Vec::new();
    file.read_to_end(&mut encoded)
        .map_err(|error| format!("Could not read image {}: {error}", path.display()))?;
    if cancelled() {
        return Err("Image loading was superseded by another file".to_owned());
    }
    let decoded = image::load_from_memory(&encoded)
        .map_err(|error| format!("Could not decode image {}: {error}", path.display()))?
        .into_rgba8();
    let (width, height) = decoded.dimensions();
    Ok((
        PreviewPage {
            size: [width as usize, height as usize],
            rgba: decoded.into_raw(),
        },
        key,
    ))
}

fn load_pdf(path: &Path, cancelled: impl Fn() -> bool) -> Result<LoadedAsset, String> {
    let bytes = fs::read(path)
        .map_err(|error| format!("Could not read PDF {}: {error}", path.display()))?;
    if cancelled() {
        return Err("PDF loading was superseded by another file".into());
    }
    Ok(LoadedAsset::Pdf { bytes })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pdfium_assets_load_bytes_without_poppler_or_workspace_staging() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("fixture.pdf");
        std::fs::write(&path, b"PDFium owns parsing, including password prompts").unwrap();
        let LoadedAsset::Pdf { bytes } = load_pdf(&path, || false).unwrap() else {
            panic!("PDF asset")
        };
        assert_eq!(bytes, b"PDFium owns parsing, including password prompts");
        assert!(!directory.path().join(".tiptoptyp").exists());
        assert!(load_pdf(&path, || true).is_err());
    }

    fn request(token: u64) -> AssetRequest {
        AssetRequest {
            token: AssetToken(token),
            path: PathBuf::from(format!("asset-{token}.pdf")),
            kind: DocumentKind::Pdf,
            dark: false,
        }
    }

    #[test]
    fn asset_cancellation_wakes_worker_to_release_cached_source() {
        let (requests, receiver) = latest_channel();
        let (_results, result_rx) = mpsc::channel();
        let loader = AssetLoader {
            requests: Some(requests),
            results: result_rx,
            disconnected: AtomicBool::new(false),
            worker: None,
            shutdown: Arc::new(AtomicBool::new(false)),
            latest_token: Arc::new(AtomicU64::new(0)),
        };
        loader.cancel_before(AssetToken(1));
        assert_eq!(loader.latest_token.load(Ordering::Acquire), 1);
        assert!(
            matches!(receiver.try_recv(), Ok(AssetCommand::ReleaseCache)),
            "cancellation must wake an idle worker to release the previous image"
        );
    }

    #[test]
    fn image_appearance_reuses_source_and_preserves_alpha_and_original_pixels() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("appearance.png");
        let pixels = vec![0, 0, 0, 255, 255, 255, 255, 128, 32, 64, 96, 0];
        image::RgbaImage::from_raw(3, 1, pixels.clone())
            .unwrap()
            .save(&path)
            .unwrap();
        let mut cache = ImageSourceCache::default();
        let source = cache.source(&path, &|| false).unwrap();
        for dark in [false, true, false, true, false] {
            let LoadedAsset::Image(page) = cache.load(&path, dark, &|| false).unwrap() else {
                panic!("image request returned a PDF")
            };
            assert_eq!(page.size, [3, 1]);
            assert_eq!(
                page.rgba,
                if dark {
                    crate::preview::dark_preview_rgba(&pixels)
                } else {
                    pixels.clone()
                }
            );
            for (pixel, original) in page
                .rgba
                .as_chunks::<4>()
                .0
                .iter()
                .zip(pixels.as_chunks::<4>().0.iter())
            {
                assert_eq!(pixel[3], original[3]);
            }
            assert_eq!(source.rgba, pixels);
            assert!(Arc::ptr_eq(
                &source,
                &cache.source(&path, &|| false).unwrap()
            ));
        }
    }

    #[test]
    fn uncached_large_light_image_moves_pixels_without_copying() {
        let size = [4097, 1024];
        let rgba = vec![128; size[0] * size[1] * 4];
        assert!(!retain_image_source(rgba.len()));
        let original_pixels = rgba.as_ptr();
        let original_capacity = rgba.capacity();
        let page = image_appearance(Arc::new(PreviewPage { size, rgba }), false);
        assert_eq!(page.size, size);
        assert_eq!(page.rgba.as_ptr(), original_pixels);
        assert_eq!(page.rgba.capacity(), original_capacity);
        assert!(page.rgba.iter().all(|value| *value == 128));
    }

    #[test]
    fn image_source_cache_invalidates_changed_files_and_stays_bounded() {
        let directory = tempfile::tempdir().unwrap();
        let first = directory.path().join("first.png");
        let second = directory.path().join("second.png");
        image::RgbaImage::from_pixel(1, 1, image::Rgba([1, 2, 3, 255]))
            .save(&first)
            .unwrap();
        image::RgbaImage::from_pixel(2, 1, image::Rgba([4, 5, 6, 128]))
            .save(&second)
            .unwrap();
        let mut cache = ImageSourceCache::default();
        let original = cache.source(&first, &|| false).unwrap();
        let next = cache.source(&second, &|| false).unwrap();
        assert!(!Arc::ptr_eq(&original, &next));
        assert_eq!(cache.entry.as_ref().unwrap().0.fingerprint.path, second);
        std::fs::copy(&second, &first).unwrap();
        let replaced = cache.source(&first, &|| false).unwrap();
        assert_eq!(replaced.size, [2, 1]);
        assert_eq!(replaced.rgba, next.rgba);
        assert!(retain_image_source(IMAGE_SOURCE_CACHE_MAX_BYTES));
        assert!(!retain_image_source(IMAGE_SOURCE_CACHE_MAX_BYTES + 1));
        assert!(cache.source(&first, &|| true).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn image_source_cache_observes_symlink_retargets_even_with_identical_metadata() {
        use std::os::unix::fs::symlink;
        let directory = tempfile::tempdir().unwrap();
        let first = directory.path().join("first.png");
        let second = directory.path().join("second.png");
        let link = directory.path().join("link.png");
        image::RgbaImage::from_pixel(1, 1, image::Rgba([1, 2, 3, 255]))
            .save(&first)
            .unwrap();
        std::fs::copy(&first, &second).unwrap();
        let modified = fs::metadata(&first).unwrap().modified().unwrap();
        fs::File::options()
            .write(true)
            .open(&second)
            .unwrap()
            .set_times(fs::FileTimes::new().set_modified(modified))
            .unwrap();
        symlink(&first, &link).unwrap();
        let mut cache = ImageSourceCache::default();
        let original = cache.source(&link, &|| false).unwrap();
        let before = AssetFingerprint::read(&link).unwrap();
        fs::remove_file(&link).unwrap();
        symlink(&second, &link).unwrap();
        assert_eq!(AssetFingerprint::read(&link).unwrap(), before);
        let retargeted = cache.source(&link, &|| false).unwrap();
        assert!(
            !Arc::ptr_eq(&original, &retargeted),
            "retargeted symlinks must decode their new target"
        );
        assert_eq!(
            cache.entry.as_ref().unwrap().0.target,
            second.canonicalize().unwrap()
        );
    }

    #[cfg(unix)]
    #[test]
    fn image_source_cache_does_not_retain_a_symlink_target_changed_during_decode() {
        use std::os::unix::fs::symlink;
        let directory = tempfile::tempdir().unwrap();
        let first = directory.path().join("first.png");
        let second = directory.path().join("second.png");
        let link = directory.path().join("link.png");
        image::RgbaImage::from_pixel(1, 1, image::Rgba([1, 2, 3, 255]))
            .save(&first)
            .unwrap();
        image::RgbaImage::from_pixel(1, 1, image::Rgba([4, 5, 6, 128]))
            .save(&second)
            .unwrap();
        symlink(&first, &link).unwrap();
        let calls = std::sync::atomic::AtomicUsize::new(0);
        let mut cache = ImageSourceCache::default();
        let original = cache
            .source(&link, &|| {
                if calls.fetch_add(1, Ordering::Relaxed) == 1 {
                    fs::remove_file(&link).unwrap();
                    symlink(&second, &link).unwrap();
                }
                false
            })
            .unwrap();
        assert_eq!(original.rgba, [1, 2, 3, 255]);
        assert!(
            cache.entry.is_none(),
            "changed-during-decode source must not enter the cache"
        );
        assert_eq!(cache.source(&link, &|| false).unwrap().rgba, [4, 5, 6, 128]);
        assert_eq!(
            cache.entry.as_ref().unwrap().0.target,
            second.canonicalize().unwrap()
        );
    }

    #[cfg(unix)]
    #[test]
    fn image_source_key_observes_atomic_replacement_with_preserved_mtime() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("image.png");
        let replacement = directory.path().join("replacement.png");
        image::RgbaImage::from_pixel(1, 1, image::Rgba([1, 2, 3, 255]))
            .save(&path)
            .unwrap();
        let before = ImageSourceKey::read(&path).unwrap();
        fs::copy(&path, &replacement).unwrap();
        let modified = fs::metadata(&path).unwrap().modified().unwrap();
        fs::File::options()
            .write(true)
            .open(&replacement)
            .unwrap()
            .set_times(fs::FileTimes::new().set_modified(modified))
            .unwrap();
        fs::rename(replacement, &path).unwrap();
        let after = ImageSourceKey::read(&path).unwrap();
        assert_eq!(before.fingerprint, after.fingerprint);
        assert_ne!(
            before, after,
            "new file identity must invalidate retained pixels"
        );
    }

    #[test]
    fn queued_asset_requests_collapse_to_the_newest() {
        let (sender, receiver) = latest_channel();
        sender.send(AssetCommand::Load(request(2))).unwrap();
        sender.send(AssetCommand::Load(request(3))).unwrap();

        let AssetCommand::Load(latest) =
            take_latest_queued(AssetCommand::Load(request(1)), &receiver)
        else {
            panic!("newest asset load was discarded")
        };

        assert_eq!(latest.token.0, 3);
        assert_eq!(latest.path, PathBuf::from("asset-3.pdf"));
    }

    #[test]
    fn cache_release_drops_source_pixels_and_supersedes_pending_loads() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("image.png");
        image::RgbaImage::from_pixel(1, 1, image::Rgba([1, 2, 3, 255]))
            .save(&path)
            .unwrap();
        let mut cache = ImageSourceCache::default();
        let source = cache.source(&path, &|| false).unwrap();
        let pixels = Arc::downgrade(&source);
        drop(source);
        assert!(pixels.upgrade().is_some());
        let (sender, receiver) = latest_channel();
        sender.send(AssetCommand::Load(request(2))).unwrap();
        sender.send(AssetCommand::ReleaseCache).unwrap();
        let latest = take_latest_queued(AssetCommand::Load(request(1)), &receiver);
        assert!(latest.into_request(&mut cache).is_none());
        assert!(cache.entry.is_none());
        assert!(
            pixels.upgrade().is_none(),
            "closed image must release its pixels"
        );
    }

    #[test]
    fn failed_new_image_lookup_releases_the_previous_cached_source() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("image.png");
        image::RgbaImage::from_pixel(1, 1, image::Rgba([1, 2, 3, 255]))
            .save(&path)
            .unwrap();
        let mut cache = ImageSourceCache::default();
        let source = cache.source(&path, &|| false).unwrap();
        let pixels = Arc::downgrade(&source);
        drop(source);
        let (sender, receiver) = latest_channel();
        sender.send(AssetCommand::ReleaseCache).unwrap();
        sender
            .send(AssetCommand::Load(AssetRequest {
                token: AssetToken(1),
                path: directory.path().join("missing.png"),
                kind: DocumentKind::Image,
                dark: false,
            }))
            .unwrap();
        let request = take_latest_queued(AssetCommand::ReleaseCache, &receiver)
            .into_request(&mut cache)
            .unwrap();
        assert!(cache.load(&request.path, request.dark, &|| false).is_err());
        assert!(
            pixels.upgrade().is_none(),
            "a failed replacement must not retain the previous image's pixels"
        );
    }

    #[test]
    fn cache_release_emits_no_asset_result_or_repaint() {
        let (sender, receiver) = latest_channel();
        sender.send(AssetCommand::ReleaseCache).unwrap();
        drop(sender);
        let (results, result_rx) = mpsc::channel();
        let context = eframe::egui::Context::default();
        let repaints = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let observed_repaints = repaints.clone();
        context.set_request_repaint_callback(move |_| {
            observed_repaints.fetch_add(1, Ordering::Relaxed);
        });
        worker_loop(
            receiver,
            results,
            crate::worker::RepaintTarget::current(&context),
            Arc::new(AtomicBool::new(false)),
            Arc::new(AtomicU64::new(0)),
        );
        assert!(matches!(
            result_rx.try_recv(),
            Err(mpsc::TryRecvError::Disconnected)
        ));
        assert_eq!(repaints.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn newer_asset_load_supersedes_a_pending_cache_release() {
        let (sender, receiver) = latest_channel();
        sender.send(AssetCommand::ReleaseCache).unwrap();
        sender.send(AssetCommand::Load(request(3))).unwrap();
        let mut cache = ImageSourceCache::default();
        let latest = take_latest_queued(AssetCommand::Load(request(1)), &receiver);
        let latest = latest.into_request(&mut cache).unwrap();
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
