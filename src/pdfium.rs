//! PDFium owns no UI state. One latest-wins worker per surface retains its parsed
//! document between viewport requests; the binding serializes native calls.
use eframe::egui;
use pdfium_render::prelude::*;
use std::{
    ffi::OsStr,
    path::{Path, PathBuf},
    sync::{
        Arc, Condvar, Mutex, OnceLock,
        atomic::{AtomicU64, Ordering},
    },
};

pub(crate) const MAX_PAGES: usize = 10_000;
const MAX_BYTES: usize = 256 * 1024 * 1024;
const MAX_PIXELS: usize = 24 * 1024 * 1024;

#[derive(Debug, PartialEq, Eq)]
enum LibrarySource {
    Packaged(PathBuf),
    Development(PathBuf),
}

pub(crate) fn library_path() -> Result<PathBuf, String> {
    match library_source()? {
        LibrarySource::Packaged(archive) => crate::bundled_payload::prepare_packaged(&archive)
            .map_err(|error| format!("Could not prepare bundled PDFium: {error}")),
        LibrarySource::Development(path) => Ok(path),
    }
}

/// Normal startup checks the small manifest and resource metadata only. The
/// existing raster/thumbnail worker prepares and binds PDFium on first use.
pub(crate) fn validate_presence() -> Result<(), String> {
    library_source().map(|_| ())
}

fn library_source() -> Result<LibrarySource, String> {
    let name = Pdfium::pdfium_platform_library_name();
    let candidates = if cfg!(feature = "production") {
        Vec::new()
    } else {
        development_library_candidates(
            std::env::current_exe().ok().as_deref(),
            Path::new(env!("CARGO_MANIFEST_DIR")),
            crate::toolchain::target_triple(),
            &name,
        )
    };
    locate_library(
        crate::bundled_payload::packaged_candidates(&name.to_string_lossy()),
        candidates,
    )
}

fn locate_library(
    archives: Vec<PathBuf>,
    development: Vec<PathBuf>,
) -> Result<LibrarySource, String> {
    for archive in archives {
        if archive.exists() {
            crate::bundled_payload::validate_packaged(&archive)
                .map_err(|error| format!("Could not read bundled PDFium: {error}"))?;
            return Ok(LibrarySource::Packaged(archive));
        }
    }
    development.into_iter().find(|p| p.is_file()).map(LibrarySource::Development).ok_or_else(||
        "Bundled PDFium is missing. Source builds: run cargo run --manifest-path xtask/Cargo.toml -- fetch-pdfium. Packaged builds: reinstall the complete app.".into())
}

fn development_library_candidates(
    executable: Option<&Path>,
    repository: &Path,
    target: &str,
    name: &OsStr,
) -> Vec<PathBuf> {
    let mut candidates = Vec::new();
    if let Some(directory) = executable.and_then(Path::parent) {
        candidates.push(directory.join("../Resources/pdfium").join(name));
        candidates.push(directory.join("pdfium").join(name));
        candidates.push(directory.join("../lib/tiptoptyp/pdfium").join(name));
    }
    candidates.push(
        repository
            .join("toolchain/pdfium-targets")
            .join(target)
            .join(name),
    );
    candidates
}

pub(crate) fn validate() -> Result<(), String> {
    engine().map(|_| ())
}

fn engine() -> Result<&'static Pdfium, String> {
    static ENGINE: OnceLock<Result<Pdfium, String>> = OnceLock::new();
    ENGINE
        .get_or_init(|| {
            let bindings = Pdfium::bind_to_library(library_path()?).map_err(|e| e.to_string())?;
            Ok(Pdfium::new(bindings))
        })
        .as_ref()
        .map_err(Clone::clone)
}

/// Runs on the existing asset worker. Uses the same process-wide PDFium engine
/// as document viewing; no subprocess, filesystem workspace, or extra worker.
pub(crate) fn thumbnail(
    bytes: &[u8],
    max_dimension: u32,
    mut cancelled: impl FnMut() -> bool,
) -> Result<crate::pdf::PreviewPage, String> {
    let mut check = || {
        if cancelled() {
            Err("PDF thumbnail was superseded".to_owned())
        } else {
            Ok(())
        }
    };
    check()?;
    if bytes.len() > MAX_BYTES {
        return Err("PDF exceeds the 256 MiB preview limit".into());
    }
    // Reject invalid caps before loading the engine or parsing the document.
    if max_dimension == 0 || max_dimension > 65535 {
        return Err("PDF thumbnail dimensions are outside the supported range".into());
    }
    let document = engine()?
        .load_pdf_from_byte_slice(bytes, None)
        .map_err(|e| e.to_string())?;
    check()?;
    let count = document.pages().len() as usize;
    if count == 0 || count > MAX_PAGES {
        return Err("PDF must have 1–10,000 pages".into());
    }
    let page = document.pages().get(0).map_err(|e| e.to_string())?;
    let size =
        crate::pdf::thumbnail_size([page.width().value, page.height().value], max_dimension)?;
    let config = PdfRenderConfig::new()
        .set_target_size(size[0] as i32, size[1] as i32)
        .set_clear_color(PdfColor::WHITE);
    check()?;
    let result = render_pixels(&page, &config)?;
    check()?;
    Ok(result)
}

fn render_pixels(
    page: &PdfPage<'_>,
    config: &PdfRenderConfig,
) -> Result<crate::pdf::PreviewPage, String> {
    let bitmap = page.render_with_config(config).map_err(|e| e.to_string())?;
    Ok(crate::pdf::PreviewPage {
        size: [bitmap.width() as usize, bitmap.height() as usize],
        rgba: bitmap.as_rgba_bytes(),
    })
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct RequestKey {
    pub revision: u64,
    pub pages: Vec<usize>,
    pub scale: u32, // physical pixels per PDF point, hundredths
    pub palette: (egui::Color32, egui::Color32),
}

/// Search identity is independent of viewport pixels. A changing query must
/// not cancel a render, and scrolling must not restart a document search.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct SearchKey {
    pub revision: u64,
    pub query: Arc<str>,
}

struct SearchRequest {
    id: u64,
    key: SearchKey,
    next_page: usize,
    hits: Vec<usize>,
}

#[derive(Debug, PartialEq, Eq)]
enum SearchProgress {
    Complete,
    Paused,
    Cancelled,
}

impl SearchRequest {
    fn advance(
        &mut self,
        pages: usize,
        mut current: impl FnMut() -> bool,
        mut raster_pending: impl FnMut() -> bool,
        mut matches: impl FnMut(usize) -> bool,
    ) -> SearchProgress {
        while self.next_page < pages {
            if !current() {
                return SearchProgress::Cancelled;
            }
            if raster_pending() {
                return SearchProgress::Paused;
            }
            if matches(self.next_page) {
                self.hits.push(self.next_page);
            }
            self.next_page += 1;
        }
        SearchProgress::Complete
    }
}

pub(crate) struct Request {
    pub key: RequestKey,
    pub bytes: Arc<[u8]>,
}

#[derive(Debug)]
pub(crate) struct Catalog {
    pub sizes: Vec<[f32; 2]>,
    pub outline: Vec<(String, usize)>,
}

#[derive(Debug)]
pub(crate) struct Character {
    pub value: char,
    pub rect: egui::Rect, // normalized display coordinates, including page rotation
}

/// Prepare display pixels on the raster worker. The UI receives a movable,
/// shared image and never clones, recolors or converts a full page on resize.
pub(crate) fn prepared_image(
    size: [usize; 2],
    rgba: Vec<u8>,
    palette: (egui::Color32, egui::Color32),
) -> Arc<egui::ColorImage> {
    if palette == (egui::Color32::WHITE, egui::Color32::BLACK) {
        return Arc::new(egui::ColorImage::from_rgba_unmultiplied(size, &rgba));
    }
    assert_eq!(size[0] * size[1] * 4, rgba.len());
    let (background, foreground) = palette;
    let pixels = rgba
        .as_chunks::<4>()
        .0
        .iter()
        .map(|pixel| {
            let channels: [u8; 3] = std::array::from_fn(|channel| {
                let value = u32::from(pixel[channel]);
                ((u32::from(foreground[channel]) * (255 - value)
                    + u32::from(background[channel]) * value
                    + 127)
                    / 255) as u8
            });
            egui::Color32::from_rgba_unmultiplied(channels[0], channels[1], channels[2], pixel[3])
        })
        .collect();
    Arc::new(egui::ColorImage::new(size, pixels))
}

#[derive(Debug, Clone)]
pub(crate) enum LinkTarget {
    Page(usize),
    Url(String),
}

pub(crate) struct Page {
    pub index: usize,
    pub image: Arc<egui::ColorImage>,
    pub characters: Vec<Character>,
    pub links: Vec<(egui::Rect, LinkTarget)>,
}

pub(crate) struct Batch {
    pub catalog: Arc<Catalog>,
    pub pages: Vec<Page>,
    #[cfg(test)]
    pub parsed_new_document: bool,
}

pub(crate) struct Reply {
    pub key: RequestKey,
    pub result: Result<Batch, String>,
}

#[derive(Default)]
struct Mailbox {
    request: Option<(u64, Request)>,
    reply: Option<Reply>,
    search_request: Option<SearchRequest>,
    search: Option<(SearchKey, Vec<usize>)>,
    stop: bool,
}

pub(crate) struct Worker {
    mailbox: Arc<(Mutex<Mailbox>, Condvar)>,
    serial: Arc<AtomicU64>,
    search_serial: Arc<AtomicU64>,
    document_revision: Arc<AtomicU64>,
}

impl Worker {
    #[cfg(test)]
    pub(crate) fn idle_for_tests() -> Self {
        Self {
            mailbox: Arc::new((Mutex::new(Mailbox::default()), Condvar::new())),
            serial: Arc::new(AtomicU64::new(0)),
            search_serial: Arc::new(AtomicU64::new(0)),
            document_revision: Arc::new(AtomicU64::new(0)),
        }
    }

    pub fn start(repaint: crate::worker::RepaintTarget) -> Self {
        let mailbox = Arc::new((Mutex::new(Mailbox::default()), Condvar::new()));
        let serial = Arc::new(AtomicU64::new(0));
        let search_serial = Arc::new(AtomicU64::new(0));
        let document_revision = Arc::new(AtomicU64::new(0));
        let worker = Self {
            mailbox: mailbox.clone(),
            serial: serial.clone(),
            search_serial: search_serial.clone(),
            document_revision: document_revision.clone(),
        };
        std::thread::spawn(move || {
            let mut retained: Option<(Arc<[u8]>, PdfDocument<'static>, Arc<Catalog>)> = None;
            let mut retained_revision = 0;
            let mut search_cache: Option<(SearchKey, Vec<usize>)> = None;
            loop {
                enum Work {
                    Render(u64, Request),
                    Search(SearchRequest, u64),
                }
                let work = {
                    let (lock, wake) = &*mailbox;
                    let mut state = lock.lock().unwrap();
                    while state.request.is_none() && state.search_request.is_none() && !state.stop {
                        state = wake.wait(state).unwrap();
                    }
                    if state.stop {
                        break;
                    }
                    // Pixels always publish before a potentially long document
                    // search; each queue retains only its latest request.
                    if let Some((id, request)) = state.request.take() {
                        Work::Render(id, request)
                    } else {
                        Work::Search(
                            state.search_request.take().unwrap(),
                            serial.load(Ordering::Acquire),
                        )
                    }
                };
                let (id, request) = match work {
                    Work::Render(id, request) => (id, request),
                    Work::Search(mut request, raster_serial) => {
                        let current = || {
                            search_serial.load(Ordering::Acquire) == request.id
                                && document_revision.load(Ordering::Acquire) == request.key.revision
                        };
                        if !current() {
                            continue;
                        }
                        let hits = if request.key.query.is_empty() {
                            Vec::new()
                        } else if let Some((cached, hits)) = &search_cache
                            && *cached == request.key
                        {
                            hits.clone()
                        } else if retained_revision == request.key.revision
                            && let Some((_, doc, catalog)) = &retained
                        {
                            let query = request.key.query.to_lowercase();
                            let id = request.id;
                            let revision = request.key.revision;
                            match request.advance(
                                catalog.sizes.len(),
                                || {
                                    search_serial.load(Ordering::Acquire) == id
                                        && document_revision.load(Ordering::Acquire) == revision
                                },
                                || serial.load(Ordering::Acquire) != raster_serial,
                                |index| {
                                    doc.pages().get(index as i32).is_ok_and(|page| {
                                        page.text().is_ok_and(|text| {
                                            text.all().to_lowercase().contains(&query)
                                        })
                                    })
                                },
                            ) {
                                SearchProgress::Cancelled => continue,
                                SearchProgress::Paused => {
                                    // Retain progress in the same bounded slot;
                                    // a newer query always owns that slot first.
                                    let mut state = mailbox.0.lock().unwrap();
                                    if state.search_request.is_none()
                                        && !state.stop
                                        && search_serial.load(Ordering::Acquire) == id
                                        && document_revision.load(Ordering::Acquire) == revision
                                    {
                                        state.search_request = Some(request);
                                    }
                                    continue;
                                }
                                SearchProgress::Complete => request.hits,
                            }
                        } else {
                            // A failed document render owns its visible error;
                            // never search the previous document on its behalf.
                            continue;
                        };
                        if search_serial.load(Ordering::Acquire) == request.id
                            && document_revision.load(Ordering::Acquire) == request.key.revision
                        {
                            search_cache = Some((request.key.clone(), hits.clone()));
                            mailbox.0.lock().unwrap().search = Some((request.key, hits));
                            repaint.request_repaint();
                        }
                        continue;
                    }
                };
                let current = || serial.load(Ordering::Acquire) == id;
                let result = (|| {
                    if request.bytes.len() > MAX_BYTES {
                        return Err("PDF exceeds the 256 MiB preview limit".into());
                    }
                    let changed = retained.as_ref().is_none_or(|(bytes, _, _)| {
                        !Arc::ptr_eq(bytes, &request.bytes) && **bytes != *request.bytes
                    });
                    if changed {
                        let doc = engine()?
                            .load_pdf_from_byte_vec(request.bytes.to_vec(), None)
                            .map_err(|e| e.to_string())?;
                        let count = doc.pages().len() as usize;
                        if count == 0 || count > MAX_PAGES {
                            return Err("PDF must have 1–10,000 pages".into());
                        }
                        let mut sizes = Vec::with_capacity(count);
                        for index in 0..count {
                            if !current() {
                                return Err("Superseded".into());
                            }
                            let size = doc
                                .pages()
                                .page_size(index as i32)
                                .map_err(|e| e.to_string())?;
                            let size = [size.width().value, size.height().value];
                            if !size
                                .iter()
                                .all(|v| v.is_finite() && *v > 0.0 && *v < 100_000.0)
                            {
                                return Err("Unsupported PDF page dimensions".into());
                            }
                            sizes.push(size);
                        }
                        let outline = doc
                            .bookmarks()
                            .iter()
                            .take(2000)
                            .filter_map(|b| {
                                Some((
                                    b.title()?,
                                    usize::try_from(b.destination()?.page_index().ok()?).ok()?,
                                ))
                            })
                            .collect();
                        retained = Some((
                            request.bytes.clone(),
                            doc,
                            Arc::new(Catalog { sizes, outline }),
                        ));
                    }
                    retained.as_mut().unwrap().0 = request.bytes.clone();
                    retained_revision = request.key.revision;
                    if changed {
                        search_cache = None;
                    }
                    let (_, doc, catalog) = retained.as_ref().unwrap();
                    let mut pages = Vec::new();
                    let mut pixels = 0;
                    for &index in &request.key.pages {
                        if !current() {
                            return Err("Superseded".into());
                        }
                        let index = index.min(catalog.sizes.len() - 1);
                        if pages.iter().any(|p: &Page| p.index == index) {
                            continue;
                        }
                        let size = raster_size(catalog.sizes[index], request.key.scale)?;
                        pixels += size[0] * size[1];
                        if pixels > MAX_PIXELS {
                            return Err("Visible PDF region exceeds the 24 megapixel budget; zoom in to view fewer pages".into());
                        }
                        let page = doc.pages().get(index as i32).map_err(|e| e.to_string())?;
                        let config =
                            PdfRenderConfig::new().set_target_size(size[0] as i32, size[1] as i32);
                        let crate::pdf::PreviewPage { size, rgba } = render_pixels(&page, &config)?;
                        if !current() {
                            return Err("Superseded".into());
                        }
                        let transform = |rect: PdfRect| -> Option<egui::Rect> {
                            let mut out = egui::Rect::NOTHING;
                            for (x, y) in [
                                (rect.left(), rect.top()),
                                (rect.right(), rect.top()),
                                (rect.left(), rect.bottom()),
                                (rect.right(), rect.bottom()),
                            ] {
                                let (x, y) = page.points_to_pixels(x, y, &config).ok()?;
                                out.extend_with(egui::pos2(
                                    x as f32 / size[0] as f32,
                                    y as f32 / size[1] as f32,
                                ));
                            }
                            Some(out)
                        };
                        let text = page.text().map_err(|e| e.to_string())?;
                        let mut characters = Vec::new();
                        if text.chars().len() > 100_000 {
                            return Err("PDF page exceeds the text geometry limit".into());
                        }
                        for c in text.chars().iter() {
                            if !current() {
                                return Err("Superseded".into());
                            }
                            if let Some(value) = c.unicode_char() {
                                characters.push(Character {
                                    value,
                                    rect: c
                                        .loose_bounds()
                                        .ok()
                                        .and_then(transform)
                                        .unwrap_or(egui::Rect::NOTHING),
                                });
                            }
                        }
                        let links = page
                            .links()
                            .iter()
                            .filter_map(|link| {
                                let rect = transform(link.rect().ok()?)?;
                                let target = if let Some(destination) = link.destination() {
                                    LinkTarget::Page(
                                        usize::try_from(destination.page_index().ok()?).ok()?,
                                    )
                                } else {
                                    let action = link.action()?;
                                    let uri = action.as_uri_action()?.uri().ok()?;
                                    if !safe_url(&uri) {
                                        return None;
                                    }
                                    LinkTarget::Url(uri)
                                };
                                Some((rect, target))
                            })
                            .collect();
                        if !current() {
                            return Err("Superseded".into());
                        }
                        let image = prepared_image(size, rgba, request.key.palette);
                        if !current() {
                            return Err("Superseded".into());
                        }
                        pages.push(Page {
                            index,
                            image,
                            characters,
                            links,
                        });
                    }
                    Ok(Batch {
                        catalog: catalog.clone(),
                        pages,
                        #[cfg(test)]
                        parsed_new_document: changed,
                    })
                })();
                if current() {
                    mailbox.0.lock().unwrap().reply = Some(Reply {
                        key: request.key.clone(),
                        result,
                    });
                    repaint.request_repaint();
                }
            }
        });
        worker
    }

    pub fn request(&self, request: Request) {
        let mut state = self.mailbox.0.lock().unwrap();
        let id = self.serial.fetch_add(1, Ordering::AcqRel) + 1;
        self.document_revision
            .store(request.key.revision, Ordering::Release);
        state.request = Some((id, request));
        state.reply = None;
        self.mailbox.1.notify_one();
    }

    pub fn search(&self, key: SearchKey) {
        let mut state = self.mailbox.0.lock().unwrap();
        let id = self.search_serial.fetch_add(1, Ordering::AcqRel) + 1;
        state.search_request = Some(SearchRequest {
            id,
            key,
            next_page: 0,
            hits: Vec::new(),
        });
        state.search = None;
        self.mailbox.1.notify_one();
    }

    pub fn poll(&self) -> (Option<Reply>, Option<(SearchKey, Vec<usize>)>) {
        let mut state = self.mailbox.0.lock().unwrap();
        (state.reply.take(), state.search.take())
    }
}

impl Drop for Worker {
    fn drop(&mut self) {
        self.serial.fetch_add(1, Ordering::AcqRel);
        self.search_serial.fetch_add(1, Ordering::AcqRel);
        let mut state = self.mailbox.0.lock().unwrap();
        state.stop = true;
        state.request = None;
        state.search_request = None;
        self.mailbox.1.notify_one();
        // Never join a native render on the UI thread. The worker drops its document.
    }
}

fn raster_size(size: [f32; 2], scale: u32) -> Result<[usize; 2], String> {
    if scale == 0 || !size.iter().all(|v| v.is_finite() && *v > 0.0) {
        return Err("Invalid PDF render scale".into());
    }
    let scale = (scale as f32 / 100.0).min((16.0 * 1024.0 * 1024.0 / (size[0] * size[1])).sqrt());
    let result = [
        (size[0] * scale).ceil() as usize,
        (size[1] * scale).ceil() as usize,
    ];
    if result.contains(&0) || result.iter().any(|v| *v > 32767) {
        return Err("PDF bitmap dimensions exceed limits".into());
    }
    Ok(result)
}

pub(crate) fn safe_url(uri: &str) -> bool {
    url::Url::parse(uri).is_ok_and(|u| matches!(u.scheme(), "http" | "https" | "mailto"))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn source_pdfium_candidates_isolate_architectures_and_reuse_package_inputs() {
        let root = Path::new("workspace");
        let executable = root.join("target/debug/tiptoptyp");
        let name = OsStr::new("libpdfium.dylib");
        for target in ["aarch64-apple-darwin", "x86_64-apple-darwin"] {
            let candidates = development_library_candidates(Some(&executable), root, target, name);
            assert_eq!(
                candidates.last().unwrap(),
                &root
                    .join("toolchain/pdfium-targets")
                    .join(target)
                    .join(name),
                "source and package builds must select the same architecture-specific library"
            );
        }
    }

    #[test]
    fn packaged_presence_checks_do_not_decode_or_replace_the_library() {
        let directory = tempfile::tempdir().unwrap();
        let archive = directory.path().join("libpdfium.dylib.zst");
        // Presence is deliberately independent of Zstandard decoding and native
        // loading, both of which belong to the first actual PDF request.
        std::fs::write(&archive, b"not a Zstandard frame").unwrap();
        std::fs::write(
            directory.path().join("manifest.tsv"),
            format!("libpdfium.dylib\t3\t{}\n", "a".repeat(64)),
        )
        .unwrap();
        assert_eq!(
            locate_library(vec![archive.clone()], Vec::new()).unwrap(),
            LibrarySource::Packaged(archive.clone())
        );
        assert_eq!(std::fs::read(&archive).unwrap(), b"not a Zstandard frame");
        assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 2);

        let development = directory.path().join("raw-library");
        std::fs::write(&development, b"development library").unwrap();
        std::fs::remove_file(directory.path().join("manifest.tsv")).unwrap();
        assert!(locate_library(vec![archive.clone()], vec![development.clone()]).is_err());
        std::fs::remove_file(&archive).unwrap();
        assert_eq!(
            locate_library(vec![archive], vec![development.clone()]).unwrap(),
            LibrarySource::Development(development)
        );
        assert!(locate_library(Vec::new(), Vec::new()).is_err());
    }

    #[test]
    fn prepared_pixels_match_the_previous_palette_conversion_including_alpha() {
        let rgba: Vec<_> = [0, 1, 31, 64, 127, 128, 254, 255]
            .into_iter()
            .flat_map(|alpha| {
                (0..=255u8)
                    .flat_map(move |value| [value, 255 - value, value.wrapping_mul(7), alpha])
            })
            .collect();
        let size = [256, 8];
        for palette in [
            (egui::Color32::WHITE, egui::Color32::BLACK),
            (egui::Color32::BLACK, egui::Color32::WHITE),
            (
                egui::Color32::from_rgb(27, 38, 44),
                egui::Color32::from_rgb(224, 214, 182),
            ),
            (
                egui::Color32::from_rgba_unmultiplied(27, 38, 44, 127),
                egui::Color32::from_rgba_unmultiplied(224, 214, 182, 64),
            ),
        ] {
            // Preserve the former two-pass implementation as a test oracle.
            let mut expected = rgba.clone();
            for pixel in expected.as_chunks_mut::<4>().0 {
                for (channel, output) in pixel.iter_mut().take(3).enumerate() {
                    let value = u32::from(*output);
                    *output = ((u32::from(palette.1[channel]) * (255 - value)
                        + u32::from(palette.0[channel]) * value
                        + 127)
                        / 255) as u8;
                }
            }
            let expected = egui::ColorImage::from_rgba_unmultiplied(size, &expected);
            let actual = prepared_image(size, rgba.clone(), palette);
            assert_eq!(*actual, expected);
            assert!(
                actual
                    .pixels
                    .iter()
                    .zip(rgba.as_chunks::<4>().0.iter())
                    .all(|(prepared, original)| prepared.a() == original[3])
            );
        }
    }

    #[test]
    fn palette_changes_are_part_of_render_request_identity() {
        let key = RequestKey {
            revision: 1,
            pages: vec![0],
            scale: 100,
            palette: (egui::Color32::WHITE, egui::Color32::BLACK),
        };
        assert_ne!(
            key,
            RequestKey {
                palette: (egui::Color32::BLACK, egui::Color32::WHITE),
                ..key.clone()
            }
        );
    }

    #[test]
    fn thumbnail_preflight_rejects_cancelled_or_invalid_requests_without_pdfium() {
        assert!(
            thumbnail(b"invalid", 100, || true)
                .unwrap_err()
                .contains("superseded")
        );
        for limit in [0, u32::MAX] {
            assert!(
                thumbnail(b"invalid", limit, || false)
                    .unwrap_err()
                    .contains("dimensions")
            );
        }
    }

    #[test]
    #[ignore = "requires cargo run --manifest-path xtask/Cargo.toml -- fetch-pdfium"]
    fn native_thumbnails_preserve_pixels_rotation_white_background_and_cancellation() {
        let pdf = crate::pdf::test_pdf();
        let image = thumbnail(&pdf, 100, || false).unwrap();
        assert_eq!(image.size, [100, 50]);
        assert_eq!(image.rgba.len(), 100 * 50 * 4);
        assert!(
            image
                .rgba
                .as_chunks::<4>()
                .0
                .iter()
                .all(|pixel| *pixel == [255, 0, 0, 255])
        );
        let rotated = crate::pdf::test_pdf_page("/Rotate 90", "1 0 0 rg 0 0 200 100 re f");
        assert_eq!(thumbnail(&rotated, 100, || false).unwrap().size, [50, 100]);
        let blank = crate::pdf::test_pdf_page("", "");
        assert!(
            thumbnail(&blank, 100, || false)
                .unwrap()
                .rgba
                .iter()
                .all(|byte| *byte == 255)
        );
        assert!(thumbnail(b"not a PDF", 100, || false).is_err());
        assert!(thumbnail(&pdf, 20_000, || false).is_err());
        // Check cancellation at each phase, including after a native render.
        for cancel_at in 1..=4 {
            let mut checks = 0;
            let result = thumbnail(&pdf, 100, || {
                checks += 1;
                checks == cancel_at
            });
            assert!(result.unwrap_err().contains("superseded"));
            assert_eq!(checks, cancel_at);
        }
    }

    #[test]
    fn raster_budget_and_invalid_dimensions() {
        assert!(raster_size([f32::NAN, 20.0], 100).is_err());
        assert!(raster_size([20.0, 20.0], 0).is_err());
        assert_eq!(raster_size([420.0, 550.0], 200).unwrap(), [840, 1100]);
        let size = raster_size([420.0, 550.0], 20000).unwrap();
        assert!(size[0] * size[1] <= 16 * 1024 * 1024 + 10000);
    }
    #[test]
    fn external_links_are_allowlisted() {
        assert!(safe_url("https://example.com"));
        for uri in [
            "javascript:alert(1)",
            "file:///etc/passwd",
            "data:text/html,hello",
        ] {
            assert!(!safe_url(uri));
        }
    }

    #[test]
    #[ignore = "optimized worker probe; set TIPTOPTYP_PDFIUM_PROBE_A and _B to changed fixture PDFs"]
    fn native_fixture_probe() {
        let page_count: usize = std::env::var("TIPTOPTYP_PDFIUM_PROBE_PAGES")
            .unwrap_or_else(|_| "24".into())
            .parse()
            .unwrap();
        let page_index = page_count / 2;
        let a: Arc<[u8]> = std::fs::read(std::env::var("TIPTOPTYP_PDFIUM_PROBE_A").unwrap())
            .unwrap()
            .into();
        let b: Arc<[u8]> = std::fs::read(std::env::var("TIPTOPTYP_PDFIUM_PROBE_B").unwrap())
            .unwrap()
            .into();
        assert_ne!(*a, *b);
        for mode in [
            "fresh_worker_viewport_control",
            "retained_worker_viewport",
            "retained_worker_changed_pdf",
        ] {
            let mut worker = Worker::start(crate::worker::RepaintTarget::test());
            let mut samples = Vec::new();
            let mut opens = 0;
            let mut first = 0.0;
            for iteration in 0..35 {
                let start = std::time::Instant::now();
                if mode == "fresh_worker_viewport_control" {
                    worker = Worker::start(crate::worker::RepaintTarget::test());
                }
                let bytes = if mode == "retained_worker_changed_pdf" && iteration % 2 == 1 {
                    b.clone()
                } else {
                    a.clone()
                };
                worker.request(Request {
                    key: RequestKey {
                        revision: iteration + 1,
                        pages: vec![page_index],
                        scale: 429,
                        palette: (egui::Color32::WHITE, egui::Color32::BLACK),
                    },
                    bytes,
                });
                let reply = loop {
                    if let Some(reply) = worker.poll().0 {
                        break reply;
                    }
                    assert!(start.elapsed().as_secs() < 10);
                    std::thread::sleep(std::time::Duration::from_micros(100));
                };
                let batch = reply.result.unwrap();
                assert_eq!(batch.catalog.sizes.len(), page_count);
                assert_eq!(batch.pages.len(), 1);
                let elapsed = start.elapsed().as_secs_f64() * 1000.0;
                if iteration == 0 {
                    first = elapsed;
                }
                if iteration >= 5 {
                    samples.push(elapsed);
                    opens += usize::from(batch.parsed_new_document);
                }
            }
            let mut sorted = samples.clone();
            sorted.sort_by(f64::total_cmp);
            println!(
                "PDFIUM_PROBE {}",
                serde_json::json!({"mode": mode, "first_ms": first, "samples_ms": samples, "p50_ms": (sorted[14] + sorted[15]) / 2.0, "p95_ms": sorted[28], "document_opens": opens, "warmups": 5, "samples": 30, "profile": if cfg!(debug_assertions) { "debug" } else { "release" }, "pdfium": "chromium/7881", "scale": 4.29, "page_index": page_index, "page_count": page_count})
            );
        }
    }

    #[test]
    fn document_search_yields_to_pixels_and_resumes_without_rescanning_pages() {
        let mut request = SearchRequest {
            id: 1,
            key: SearchKey {
                revision: 1,
                query: "the".into(),
            },
            next_page: 0,
            hits: vec![],
        };
        let visited = std::cell::RefCell::new(Vec::new());
        assert_eq!(
            request.advance(
                6,
                || true,
                || visited.borrow().len() == 2,
                |index| {
                    visited.borrow_mut().push(index);
                    index % 2 == 0
                }
            ),
            SearchProgress::Paused
        );
        assert_eq!(request.next_page, 2);
        assert_eq!(request.hits, [0]);
        assert_eq!(
            request.advance(
                6,
                || true,
                || false,
                |index| {
                    visited.borrow_mut().push(index);
                    index % 2 == 0
                }
            ),
            SearchProgress::Complete
        );
        assert_eq!(*visited.borrow(), [0, 1, 2, 3, 4, 5]);
        assert_eq!(request.hits, [0, 2, 4]);
        assert_eq!(
            request.advance(
                7,
                || false,
                || false,
                |_| panic!("stale queries must not scan another page")
            ),
            SearchProgress::Cancelled
        );
    }

    #[test]
    fn search_and_raster_requests_have_independent_latest_wins_slots() {
        let worker = Worker::idle_for_tests();
        let key = RequestKey {
            revision: 1,
            pages: vec![0],
            scale: 100,
            palette: (egui::Color32::WHITE, egui::Color32::BLACK),
        };
        worker.request(Request {
            key: key.clone(),
            bytes: Arc::from(&b"PDF placeholder"[..]),
        });
        worker.mailbox.0.lock().unwrap().reply = Some(Reply {
            key: key.clone(),
            result: Err("completed render marker".into()),
        });
        for query in ["the", "norm", ""] {
            worker.search(SearchKey {
                revision: 1,
                query: query.into(),
            });
            assert_eq!(
                worker.serial.load(Ordering::Acquire),
                1,
                "query edits must not cancel native rendering"
            );
            let state = worker.mailbox.0.lock().unwrap();
            assert_eq!(state.request.as_ref().unwrap().1.key, key);
            assert_eq!(state.reply.as_ref().unwrap().key, key);
            assert_eq!(
                state.search_request.as_ref().unwrap().key.query.as_ref(),
                query
            );
        }
        let search = SearchKey {
            revision: 1,
            query: "norm".into(),
        };
        worker.mailbox.0.lock().unwrap().search = Some((search.clone(), vec![0]));
        let search_serial = worker.search_serial.load(Ordering::Acquire);
        worker.request(Request {
            key: RequestKey {
                pages: vec![1],
                scale: 200,
                ..key.clone()
            },
            bytes: Arc::from(&b"PDF placeholder"[..]),
        });
        assert_eq!(worker.search_serial.load(Ordering::Acquire), search_serial);
        assert_eq!(
            worker.document_revision.load(Ordering::Acquire),
            search.revision
        );
        assert_eq!(
            worker.poll().1,
            Some((search, vec![0])),
            "new viewport pixels must not discard completed search hits"
        );
        worker.request(Request {
            key: RequestKey { revision: 2, ..key },
            bytes: Arc::from(&b"new PDF placeholder"[..]),
        });
        assert_eq!(
            worker.document_revision.load(Ordering::Acquire),
            2,
            "a new document must cancel searches for its predecessor"
        );
    }

    fn find_fixture_pdf() -> Vec<u8> {
        const PAGES: usize = 12;
        let kids = (0..PAGES)
            .map(|page| format!("{} 0 R", 4 + 2 * page))
            .collect::<Vec<_>>()
            .join(" ");
        let mut objects = vec![
            "<< /Type /Catalog /Pages 2 0 R >>".to_owned(),
            format!("<< /Type /Pages /Count {PAGES} /Kids [{kids}] >>"),
            "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".to_owned(),
        ];
        for page in 0..PAGES {
            let mut content = "BT /F1 9 Tf 24 770 Td 12 TL\n".to_owned();
            for line in 0..60 {
                content.push_str(&format!("(Mathematical preview equation and norm: the estimate remains valid. Page {page} line {line}.) Tj T*\n"));
            }
            content.push_str("ET\n");
            objects.push(format!("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 600 800] /Resources << /Font << /F1 3 0 R >> >> /Contents {} 0 R >>", 5 + 2 * page));
            objects.push(format!(
                "<< /Length {} >>\nstream\n{content}endstream",
                content.len()
            ));
        }
        let mut pdf = "%PDF-1.4\n".to_owned();
        let mut offsets = vec![0];
        for (index, object) in objects.iter().enumerate() {
            offsets.push(pdf.len());
            pdf.push_str(&format!("{} 0 obj\n{object}\nendobj\n", index + 1));
        }
        let xref = pdf.len();
        pdf.push_str(&format!("xref\n0 {}\n0000000000 65535 f \n", offsets.len()));
        for offset in &offsets[1..] {
            pdf.push_str(&format!("{offset:010} 00000 n \n"));
        }
        pdf.push_str(&format!(
            "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF",
            offsets.len()
        ));
        pdf.into_bytes()
    }

    #[test]
    #[ignore = "matched native PDF Find worker measurement; requires packaged PDFium"]
    fn native_find_work_probe() {
        use sha2::{Digest, Sha256};
        const WARMUPS: usize = 4;
        const SAMPLES: usize = 20;
        let bytes: Arc<[u8]> = find_fixture_pdf().into();
        let hash = format!("{:x}", Sha256::digest(&bytes));
        let worker = Worker::start(crate::worker::RepaintTarget::test());
        let initial = RequestKey {
            revision: 1,
            pages: vec![6],
            scale: 200,
            palette: (egui::Color32::WHITE, egui::Color32::BLACK),
        };
        let cold = std::time::Instant::now();
        worker.request(Request {
            key: initial.clone(),
            bytes: bytes.clone(),
        });
        let first = receive(&worker).result.unwrap();
        assert_eq!(first.catalog.sizes.len(), 12);
        assert_eq!(first.pages.len(), 1);
        let cold_ms = cold.elapsed().as_secs_f64() * 1000.0;
        let mut samples = Vec::new();
        for iteration in 0..WARMUPS + SAMPLES {
            let query = ["equation", "norm", "absent-query", ""][iteration % 4];
            let key = SearchKey {
                revision: initial.revision,
                query: query.into(),
            };
            let start = std::time::Instant::now();
            worker.search(key.clone());
            let mut matches = None;
            while matches.is_none() {
                let (reply, search) = worker.poll();
                assert!(reply.is_none(), "Find must not produce replacement pixels");
                if let Some((search_key, hits)) = search {
                    assert_eq!(search_key, key);
                    matches = Some(hits);
                }
                assert!(start.elapsed().as_secs() < 30, "PDF Find worker timed out");
                if matches.is_none() {
                    std::thread::sleep(std::time::Duration::from_micros(100));
                }
            }
            let expected: Vec<_> = if query == "equation" || query == "norm" {
                (0..12).collect()
            } else {
                vec![]
            };
            assert_eq!(matches.as_ref().unwrap(), &expected);
            if iteration >= WARMUPS {
                samples.push(serde_json::json!({ "query": query, "rendered_pages": 0, "rendered_pixels": 0, "render_ms": 0.0, "complete_ms": start.elapsed().as_secs_f64() * 1000.0, "match_pages": matches.unwrap() }));
            }
        }
        println!(
            "PDFIUM_FIND_PROBE {}",
            serde_json::json!({
                "mode": "independent-find", "fixture_sha256": hash, "fixture_bytes": bytes.len(),
                "fixture_pages": 12, "visible_page": 6, "scale": 2.0, "pdfium": "chromium/7881",
                "profile": if cfg!(debug_assertions) { "dev-opt1" } else { "release" },
                "cold_initial_render_ms": cold_ms, "warmup_queries": WARMUPS, "samples": samples,
                "scope": "one retained native PDFium worker, serial changed Find queries and clear; excludes UI/GPU/presentation"
            })
        );
    }

    fn receive(worker: &Worker) -> Reply {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        loop {
            if let Some(reply) = worker.poll().0 {
                return reply;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "PDFium worker did not complete"
            );
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
    }

    #[test]
    #[ignore = "requires cargo run --manifest-path xtask/Cargo.toml -- fetch-pdfium"]
    fn native_worker_reuses_documents_rejects_stale_and_survives_errors() {
        let worker = Worker::start(crate::worker::RepaintTarget::test());
        let bytes: Arc<[u8]> = crate::pdf::test_pdf().into();
        let key = RequestKey {
            revision: 1,
            pages: vec![0],
            scale: 100,
            palette: (egui::Color32::WHITE, egui::Color32::BLACK),
        };
        worker.request(Request {
            key: key.clone(),
            bytes: bytes.clone(),
        });
        let first = receive(&worker).result.unwrap();
        assert!(first.parsed_new_document);
        assert_eq!(first.catalog.sizes, vec![[200.0, 100.0]]);
        assert_eq!(first.pages[0].image.size, [200, 100]);
        assert_eq!(first.pages[0].image.pixels[0], egui::Color32::RED);
        assert_eq!(first.pages[0].links.len(), 1);
        let recolored_key = RequestKey {
            palette: (
                egui::Color32::from_rgb(20, 30, 40),
                egui::Color32::from_rgb(200, 210, 220),
            ),
            ..key.clone()
        };
        worker.request(Request {
            key: recolored_key.clone(),
            bytes: bytes.clone(),
        });
        let recolored = receive(&worker);
        assert_eq!(recolored.key, recolored_key);
        let recolored = recolored.result.unwrap();
        assert!(!recolored.parsed_new_document);
        assert_eq!(
            recolored.pages[0].image.pixels[0],
            egui::Color32::from_rgb(20, 210, 220)
        );
        for scale in 110..=160 {
            worker.request(Request {
                key: RequestKey {
                    scale,
                    ..key.clone()
                },
                bytes: bytes.clone(),
            });
        }
        let reply = receive(&worker);
        assert_eq!(reply.key.scale, 160);
        assert!(!reply.result.unwrap().parsed_new_document);
        worker.request(Request {
            key: RequestKey {
                revision: 2,
                ..key.clone()
            },
            bytes: Arc::from(&b"not a PDF"[..]),
        });
        assert!(receive(&worker).result.is_err());
        worker.request(Request {
            key: key.clone(),
            bytes: bytes.clone(),
        });
        assert!(!receive(&worker).result.unwrap().parsed_new_document);
        let green: Arc<[u8]> = String::from_utf8(bytes.to_vec())
            .unwrap()
            .replace("1 0 0 rg", "0 1 0 rg")
            .into_bytes()
            .into();
        worker.request(Request {
            key: RequestKey { revision: 3, ..key },
            bytes: green,
        });
        let batch = receive(&worker).result.unwrap();
        assert!(batch.parsed_new_document);
        assert_eq!(batch.pages[0].image.pixels[0], egui::Color32::GREEN);
        assert!(
            worker.poll().0.is_none(),
            "idle workers must not regenerate pages"
        );
    }
}
