//! PDFium owns no UI state. One latest-wins worker per surface retains its parsed
//! document between viewport requests; the binding serializes native calls.
use eframe::egui;
use pdfium_render::prelude::*;
use std::{
    path::PathBuf,
    sync::{
        Arc, Condvar, Mutex, OnceLock,
        atomic::{AtomicU64, Ordering},
    },
};

pub(crate) const MAX_PAGES: usize = 10_000;
const MAX_BYTES: usize = 256 * 1024 * 1024;
const MAX_PIXELS: usize = 24 * 1024 * 1024;

pub(crate) fn library_path() -> Result<PathBuf, String> {
    let name = Pdfium::pdfium_platform_library_name();
    let mut candidates = Vec::new();
    if let Ok(exe) = std::env::current_exe()
        && let Some(dir) = exe.parent()
    {
        candidates.push(dir.join("../Resources/pdfium").join(&name));
        candidates.push(dir.join("pdfium").join(&name));
        candidates.push(dir.join("../lib/tiptoptyp/pdfium").join(&name));
    }
    if !cfg!(feature = "production") {
        candidates.push(
            PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("toolchain/pdfium-bundle")
                .join(name),
        );
    }
    candidates.into_iter().find(|p| p.is_file()).ok_or_else(||
        "Bundled PDFium is missing. Source builds: run cargo run --manifest-path xtask/Cargo.toml -- fetch-pdfium. Packaged builds: reinstall the complete app.".into())
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

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct RequestKey {
    pub revision: u64,
    pub pages: Vec<usize>,
    pub scale: u32, // physical pixels per PDF point, hundredths
    pub query: String,
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

#[derive(Debug, Clone)]
pub(crate) enum LinkTarget {
    Page(usize),
    Url(String),
}

pub(crate) struct Page {
    pub index: usize,
    pub size: [usize; 2],
    pub rgba: Vec<u8>,
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
    search: Option<(RequestKey, Vec<usize>)>,
    stop: bool,
}

pub(crate) struct Worker {
    mailbox: Arc<(Mutex<Mailbox>, Condvar)>,
    serial: Arc<AtomicU64>,
}

impl Worker {
    pub fn start(repaint: crate::worker::RepaintTarget) -> Self {
        let mailbox = Arc::new((Mutex::new(Mailbox::default()), Condvar::new()));
        let serial = Arc::new(AtomicU64::new(0));
        let worker = Self {
            mailbox: mailbox.clone(),
            serial: serial.clone(),
        };
        std::thread::spawn(move || {
            let mut retained: Option<(Arc<[u8]>, PdfDocument<'static>, Arc<Catalog>)> = None;
            let mut search_cache: Option<(u64, String, Vec<usize>)> = None;
            loop {
                let (id, request) = {
                    let (lock, wake) = &*mailbox;
                    let mut state = lock.lock().unwrap();
                    while state.request.is_none() && !state.stop {
                        state = wake.wait(state).unwrap();
                    }
                    if state.stop {
                        break;
                    }
                    state.request.take().unwrap()
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
                        let bitmap = page
                            .render_with_config(&config)
                            .map_err(|e| e.to_string())?;
                        let size = [bitmap.width() as usize, bitmap.height() as usize];
                        let rgba = bitmap.as_rgba_bytes();
                        drop(bitmap);
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
                        pages.push(Page {
                            index,
                            size,
                            rgba,
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
                let succeeded = result.is_ok();
                if current() {
                    mailbox.0.lock().unwrap().reply = Some(Reply {
                        key: request.key.clone(),
                        result,
                    });
                    repaint.request_repaint();
                }
                // Search is deliberately after publication, never a visible-render gate.
                if succeeded
                    && !request.key.query.is_empty()
                    && let Some((_, doc, catalog)) = &retained
                {
                    if let Some((revision, query, hits)) = &search_cache
                        && *revision == request.key.revision
                        && *query == request.key.query
                    {
                        if current() {
                            mailbox.0.lock().unwrap().search = Some((request.key, hits.clone()));
                            repaint.request_repaint();
                        }
                        continue;
                    }
                    let query = request.key.query.to_lowercase();
                    let mut hits = Vec::new();
                    for index in 0..catalog.sizes.len() {
                        if !current() {
                            break;
                        }
                        if let Ok(page) = doc.pages().get(index as i32)
                            && let Ok(text) = page.text()
                            && text.all().to_lowercase().contains(&query)
                        {
                            hits.push(index);
                        }
                    }
                    if current() {
                        search_cache = Some((
                            request.key.revision,
                            request.key.query.clone(),
                            hits.clone(),
                        ));
                        mailbox.0.lock().unwrap().search = Some((request.key, hits));
                        repaint.request_repaint();
                    }
                }
            }
        });
        worker
    }

    pub fn request(&self, request: Request) {
        let id = self.serial.fetch_add(1, Ordering::AcqRel) + 1;
        let mut state = self.mailbox.0.lock().unwrap();
        state.request = Some((id, request));
        state.reply = None;
        state.search = None;
        self.mailbox.1.notify_one();
    }

    pub fn poll(&self) -> (Option<Reply>, Option<(RequestKey, Vec<usize>)>) {
        let mut state = self.mailbox.0.lock().unwrap();
        (state.reply.take(), state.search.take())
    }
}

impl Drop for Worker {
    fn drop(&mut self) {
        self.serial.fetch_add(1, Ordering::AcqRel);
        let mut state = self.mailbox.0.lock().unwrap();
        state.stop = true;
        state.request = None;
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
                        query: String::new(),
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
            query: String::new(),
        };
        worker.request(Request {
            key: key.clone(),
            bytes: bytes.clone(),
        });
        let first = receive(&worker).result.unwrap();
        assert!(first.parsed_new_document);
        assert_eq!(first.catalog.sizes, vec![[200.0, 100.0]]);
        assert_eq!(first.pages[0].size, [200, 100]);
        assert_eq!(&first.pages[0].rgba[..4], &[255, 0, 0, 255]);
        assert_eq!(first.pages[0].links.len(), 1);
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
        assert_eq!(&batch.pages[0].rgba[..4], &[0, 255, 0, 255]);
        assert!(
            worker.poll().0.is_none(),
            "idle workers must not regenerate pages"
        );
    }
}
