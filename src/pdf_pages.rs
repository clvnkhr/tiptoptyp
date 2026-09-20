//! Latest-wins PDF page rasterization for the fallback preview.

use std::{
    ops::RangeInclusive,
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc::{self, Receiver, Sender},
    },
    thread,
};

use tiptoptyp_core::preview::ArtifactKey;

use crate::{
    pdf::{PdfRasterMode, PreviewPage, rasterize_pdf_with_program},
    worker::{LatestReceiver, LatestSender, RepaintTarget, latest_channel},
};

/// Keep a useful reading buffer around the visible range without allowing a
/// single scroll request to become an unbounded rasterization job.
const MAX_PAGES_PER_REQUEST: usize = 15;
/// A single visible page therefore requests up to fifteen pages: seven on
/// either side plus the visible page, clamped at document boundaries.
pub(crate) const ADJACENT_PAGE_PREFETCH: usize = 7;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PdfSurface {
    Document,
    Asset,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct RasterPageRequestKey {
    pub(crate) artifact: ArtifactKey,
    pub(crate) first: usize,
    pub(crate) last: usize,
    pub(crate) dpi: u32,
    pub(crate) appearance_revision: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct RasterPageKey {
    pub(crate) artifact: ArtifactKey,
    pub(crate) page: usize,
    pub(crate) dpi: u32,
    pub(crate) appearance_revision: u64,
}

impl RasterPageRequestKey {
    pub(crate) const fn page_key(self, page: usize) -> RasterPageKey {
        RasterPageKey {
            artifact: self.artifact,
            page,
            dpi: self.dpi,
            appearance_revision: self.appearance_revision,
        }
    }
}

struct Request {
    token: u64,
    surface: PdfSurface,
    key: RasterPageRequestKey,
    pdf: Arc<[u8]>,
    project_root: PathBuf,
}

#[derive(Debug)]
pub(crate) struct PdfPageResult {
    pub(crate) surface: PdfSurface,
    pub(crate) key: RasterPageRequestKey,
    pub(crate) output: Result<Vec<(usize, PreviewPage)>, String>,
}

pub(crate) struct PdfPageLoader {
    requests: Option<LatestSender<Request>>,
    results: Receiver<PdfPageResult>,
    worker: Option<thread::JoinHandle<()>>,
    shutdown: Arc<AtomicBool>,
    latest_token: Arc<AtomicU64>,
    next_token: AtomicU64,
}

impl PdfPageLoader {
    pub(crate) fn new(repaint: RepaintTarget) -> Self {
        let (request_tx, request_rx) = latest_channel();
        let (result_tx, result_rx) = mpsc::channel();
        let shutdown = Arc::new(AtomicBool::new(false));
        let latest_token = Arc::new(AtomicU64::new(0));
        let worker_shutdown = shutdown.clone();
        let worker_latest = latest_token.clone();
        let worker = thread::Builder::new()
            .name("tiptoptyp-pdf-pages".to_owned())
            .spawn(move || {
                worker_loop(
                    request_rx,
                    result_tx,
                    repaint,
                    worker_shutdown,
                    worker_latest,
                );
            })
            .ok();
        Self {
            requests: worker.as_ref().map(|_| request_tx),
            results: result_rx,
            worker,
            shutdown,
            latest_token,
            next_token: AtomicU64::new(0),
        }
    }

    pub(crate) fn request(
        &self,
        surface: PdfSurface,
        key: RasterPageRequestKey,
        pdf: Arc<[u8]>,
        project_root: PathBuf,
    ) -> Result<(), String> {
        let page_count = key.last.saturating_sub(key.first).saturating_add(1);
        if page_count > MAX_PAGES_PER_REQUEST {
            return Err(format!(
                "PDF page request contains {page_count} pages; limit is {MAX_PAGES_PER_REQUEST}"
            ));
        }
        let token = self
            .next_token
            .fetch_add(1, Ordering::AcqRel)
            .wrapping_add(1)
            .max(1);
        self.latest_token.store(token, Ordering::Release);
        self.requests
            .as_ref()
            .ok_or_else(|| "The PDF page renderer has stopped".to_owned())?
            .send(Request {
                token,
                surface,
                key,
                pdf,
                project_root,
            })
            .map_err(|_| "The PDF page renderer stopped unexpectedly".to_owned())
    }

    pub(crate) fn cancel(&self) {
        let token = self
            .next_token
            .fetch_add(1, Ordering::AcqRel)
            .wrapping_add(1)
            .max(1);
        self.latest_token.store(token, Ordering::Release);
    }

    pub(crate) fn try_recv(&self) -> Option<PdfPageResult> {
        self.results.try_recv().ok()
    }
}

impl Drop for PdfPageLoader {
    fn drop(&mut self) {
        self.shutdown.store(true, Ordering::Release);
        self.latest_token.fetch_add(1, Ordering::AcqRel);
        self.requests.take();
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

fn worker_loop(
    requests: LatestReceiver<Request>,
    results: Sender<PdfPageResult>,
    repaint: RepaintTarget,
    shutdown: Arc<AtomicBool>,
    latest_token: Arc<AtomicU64>,
) {
    while let Ok(request) = requests.recv() {
        let request = take_latest(request, &requests);
        if shutdown.load(Ordering::Acquire) {
            break;
        }
        let cancelled = || {
            shutdown.load(Ordering::Acquire)
                || latest_token.load(Ordering::Acquire) != request.token
        };
        let output = rasterize_pdf_with_program(
            &request.pdf,
            &request.project_root,
            std::path::Path::new("pdftoppm"),
            PdfRasterMode::PageRange {
                first: request.key.first,
                last: request.key.last,
                dpi: request.key.dpi,
            },
            cancelled,
        )
        .and_then(|pages| index_rendered_pages(request.key.first, request.key.last, pages));
        if latest_token.load(Ordering::Acquire) == request.token
            && results
                .send(PdfPageResult {
                    surface: request.surface,
                    key: request.key,
                    output,
                })
                .is_ok()
        {
            repaint.request_repaint();
        }
    }
}

fn index_rendered_pages(
    first: usize,
    last: usize,
    pages: Vec<PreviewPage>,
) -> Result<Vec<(usize, PreviewPage)>, String> {
    let expected = last.saturating_sub(first).saturating_add(1);
    if pages.len() != expected {
        return Err(format!(
            "PDF renderer returned {} pages for requested range {}-{} ({expected} expected)",
            pages.len(),
            first.saturating_add(1),
            last.saturating_add(1),
        ));
    }
    Ok(pages
        .into_iter()
        .enumerate()
        .map(|(offset, page)| (first + offset, page))
        .collect())
}

fn take_latest(mut request: Request, requests: &LatestReceiver<Request>) -> Request {
    while let Ok(newer) = requests.try_recv() {
        request = newer;
    }
    request
}

pub(crate) fn bounded_prefetch_range(
    visible: RangeInclusive<usize>,
    page_count: usize,
) -> Option<RangeInclusive<usize>> {
    if page_count == 0 {
        return None;
    }
    let visible_first = (*visible.start()).min(page_count - 1);
    let visible_last = (*visible.end()).max(visible_first).min(page_count - 1);
    let mut first = visible_first.saturating_sub(ADJACENT_PAGE_PREFETCH);
    let mut last = visible_last
        .saturating_add(ADJACENT_PAGE_PREFETCH)
        .min(page_count - 1);
    if last - first + 1 > MAX_PAGES_PER_REQUEST {
        let center = visible_first + (visible_last - visible_first) / 2;
        first = center.saturating_sub(MAX_PAGES_PER_REQUEST / 2);
        last = first
            .saturating_add(MAX_PAGES_PER_REQUEST - 1)
            .min(page_count - 1);
        first = last.saturating_add(1).saturating_sub(MAX_PAGES_PER_REQUEST);
    }
    Some(first..=last)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn page() -> PreviewPage {
        PreviewPage {
            size: [1, 1],
            rgba: vec![0, 0, 0, 255],
            links: Vec::new(),
        }
    }

    #[test]
    fn prefetch_is_adjacent_clamped_and_bounded() {
        assert_eq!(bounded_prefetch_range(0..=0, 100), Some(0..=7));
        assert_eq!(bounded_prefetch_range(4..=5, 10), Some(0..=9));
        assert_eq!(bounded_prefetch_range(9..=9, 10), Some(2..=9));
        let range = bounded_prefetch_range(10..=40, 100).unwrap();
        assert_eq!(range.end() - range.start() + 1, MAX_PAGES_PER_REQUEST);
    }

    #[test]
    fn prefetch_keeps_a_fifteen_page_reading_buffer_around_one_page() {
        assert_eq!(bounded_prefetch_range(50..=50, 100), Some(43..=57));
    }

    #[test]
    fn incomplete_render_output_cannot_shift_page_identities() {
        assert!(index_rendered_pages(4, 6, vec![page(), page()]).is_err());
        let indexed = index_rendered_pages(4, 6, vec![page(), page(), page()]).unwrap();
        assert_eq!(
            indexed
                .into_iter()
                .map(|(index, _)| index)
                .collect::<Vec<_>>(),
            vec![4, 5, 6]
        );
    }
}
