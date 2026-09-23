//! In-process PDF inspection, links and rasterization. No host utilities or
//! temporary PDFs are needed; callers retain scheduling and artifact identity.
use hayro::{
    RenderCache, RenderSettings,
    hayro_syntax::{
        Pdf,
        object::{Array, Dict, Name, Object, String as PdfString},
    },
};
use std::path::Path;

pub const PREVIEW_DPI: f32 = 144.0;
const MAX_PDF_PAGES: usize = 10_000;
const MAX_RENDER_PIXELS: usize = 32 * 1024 * 1024;

#[derive(Debug)]
pub struct PreviewPage {
    pub size: [usize; 2],
    pub rgba: Vec<u8>,
    pub links: Vec<PreviewLink>,
}
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct PdfPageMetadata {
    pub(crate) size: [usize; 2],
    pub(crate) links: Vec<PreviewLink>,
}
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct PdfDocumentCatalog {
    pub(crate) pages: Vec<PdfPageMetadata>,
}
#[derive(Debug, Clone, PartialEq)]
pub struct PreviewLink {
    pub rect: [f32; 4],
    pub target: String,
}

fn load(pdf: &[u8]) -> Result<Pdf, String> {
    let pdf = Pdf::new(pdf.to_vec()).map_err(|error| format!("Could not read PDF: {error:?}"))?;
    if pdf.pages().is_empty() || pdf.pages().len() > MAX_PDF_PAGES {
        return Err("PDF must contain between 1 and 10,000 pages".into());
    }
    Ok(pdf)
}
fn check_cancelled(cancelled: &mut impl FnMut() -> bool) -> Result<(), String> {
    if cancelled() {
        Err("PDF work was superseded by a newer artifact".into())
    } else {
        Ok(())
    }
}
fn dimensions(
    page: &hayro::hayro_syntax::page::Page<'_>,
    scale: f32,
) -> Result<[usize; 2], String> {
    let (w, h) = page.render_dimensions();
    let (w, h) = (w * scale, h * scale);
    if !w.is_finite() || !h.is_finite() || w <= 0.0 || h <= 0.0 || w > 65535.0 || h > 65535.0 {
        return Err("PDF page dimensions are outside the supported range".into());
    }
    Ok([w.ceil() as usize, h.ceil() as usize])
}
pub(crate) fn inspect_pdf(
    pdf: &[u8],
    _project_root: &Path,
    mut cancelled: impl FnMut() -> bool,
) -> Result<PdfDocumentCatalog, String> {
    check_cancelled(&mut cancelled)?;
    let pdf = load(pdf)?;
    let mut pages = Vec::with_capacity(pdf.pages().len());
    for page in pdf.pages().iter() {
        check_cancelled(&mut cancelled)?;
        pages.push(PdfPageMetadata {
            size: dimensions(page, PREVIEW_DPI / 72.0)?,
            links: page_links(&pdf, page),
        });
    }
    Ok(PdfDocumentCatalog { pages })
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PdfRasterMode {
    FirstPage { max_dimension: u32 },
    PageRange { first: usize, last: usize, dpi: u32 },
}
pub(crate) fn rasterize_pdf_first_page(
    pdf: &[u8],
    project_root: &Path,
    max_dimension: u32,
    cancelled: impl FnMut() -> bool,
) -> Result<PreviewPage, String> {
    rasterize_pdf(
        pdf,
        project_root,
        PdfRasterMode::FirstPage { max_dimension },
        cancelled,
    )?
    .pop()
    .ok_or_else(|| "PDF has no pages".into())
}
pub(crate) fn rasterize_pdf(
    pdf: &[u8],
    _project_root: &Path,
    mode: PdfRasterMode,
    mut cancelled: impl FnMut() -> bool,
) -> Result<Vec<PreviewPage>, String> {
    check_cancelled(&mut cancelled)?;
    let pdf = load(pdf)?;
    let (first, last) = match mode {
        PdfRasterMode::FirstPage { .. } => (0, 0),
        PdfRasterMode::PageRange { first, last, .. } => (first, last),
    };
    if first > last || last >= pdf.pages().len() {
        return Err("Invalid PDF page range".into());
    }
    let cache = RenderCache::new();
    let mut output = Vec::new();
    let mut total_pixels = 0usize;
    for page in &pdf.pages()[first..=last] {
        check_cancelled(&mut cancelled)?;
        let (w, h) = page.render_dimensions();
        let scale = match mode {
            PdfRasterMode::FirstPage { max_dimension } => max_dimension as f32 / w.max(h),
            PdfRasterMode::PageRange { dpi, .. } => dpi as f32 / 72.0,
        };
        let mut size = dimensions(page, scale)?;
        if let PdfRasterMode::FirstPage { max_dimension } = mode {
            for dimension in &mut size {
                *dimension = (*dimension).min(max_dimension as usize);
            }
        }
        total_pixels = total_pixels.saturating_add(size[0].saturating_mul(size[1]));
        if total_pixels > MAX_RENDER_PIXELS {
            return Err("PDF render exceeds the 32 megapixel request limit".into());
        }
        let settings = RenderSettings {
            x_scale: scale,
            y_scale: scale,
            width: Some(size[0] as u16),
            height: Some(size[1] as u16),
            bg_color: hayro::vello_cpu::color::palette::css::WHITE,
        };
        let pixels = hayro::render(page, &cache, &Default::default(), &settings);
        check_cancelled(&mut cancelled)?;
        output.push(PreviewPage {
            size,
            rgba: pixels.data_as_u8_slice().to_vec(),
            links: Vec::new(),
        });
    }
    Ok(output)
}
fn destination(pdf: &Pdf, object: Object<'_>, depth: usize) -> Option<String> {
    if depth > 16 {
        return None;
    }
    match object {
        Object::Array(array) => {
            let page = array.iter::<Dict<'_>>().next()?;
            let index = pdf
                .pages()
                .iter()
                .position(|candidate| candidate.raw() == &page)?;
            Some(format!("#page={}", index + 1))
        }
        Object::Dict(dict) => destination(pdf, dict.get::<Object<'_>>(b"D")?, depth + 1),
        Object::Name(name) => named_destination(pdf, name.as_ref(), depth + 1),
        Object::String(name) => named_destination(pdf, name.as_bytes(), depth + 1),
        _ => None,
    }
}
fn named_destination(pdf: &Pdf, name: &[u8], depth: usize) -> Option<String> {
    let root = pdf.xref().get::<Dict<'_>>(pdf.xref().root_id())?;
    if let Some(value) = root
        .get::<Dict<'_>>(b"Dests")
        .and_then(|d| d.get::<Object<'_>>(name))
    {
        return destination(pdf, value, depth + 1);
    }
    let names = root.get::<Dict<'_>>(b"Names")?.get::<Dict<'_>>(b"Dests")?;
    fn find(pdf: &Pdf, node: Dict<'_>, name: &[u8], depth: usize) -> Option<String> {
        if depth > 16 {
            return None;
        }
        if let Some(names) = node.get::<Array<'_>>(b"Names") {
            let mut entries = names.iter::<Object<'_>>();
            while let (Some(key), Some(value)) = (entries.next(), entries.next()) {
                if matches!(key,Object::String(key) if key.as_bytes() == name) {
                    return destination(pdf, value, depth + 1);
                }
            }
        }
        node.get::<Array<'_>>(b"Kids")?
            .iter::<Dict<'_>>()
            .find_map(|kid| find(pdf, kid, name, depth + 1))
    }
    find(pdf, names, name, depth)
}
fn page_links(pdf: &Pdf, page: &hayro::hayro_syntax::page::Page<'_>) -> Vec<PreviewLink> {
    let Some(annotations) = page.raw().get::<Array<'_>>(b"Annots") else {
        return Vec::new();
    };
    let (w, h) = page.render_dimensions();
    let [a, b, c, d, e, f] = page.initial_transform(true).as_coeffs();
    annotations
        .iter::<Dict<'_>>()
        .take(10_000)
        .filter_map(|annotation| {
            if annotation.get::<Name<'_>>(b"Subtype")?.as_ref() != b"Link" {
                return None;
            }
            let rect: Vec<f32> = annotation
                .get::<Array<'_>>(b"Rect")?
                .iter::<f32>()
                .take(4)
                .collect();
            let [x0, y0, x1, y1] = *rect.as_slice() else {
                return None;
            };
            let map = |x: f32, y: f32| {
                [
                    ((a * x as f64 + c * y as f64 + e) / w as f64) as f32,
                    ((b * x as f64 + d * y as f64 + f) / h as f64) as f32,
                ]
            };
            let p = map(x0, y0);
            let q = map(x1, y1);
            let rect = [
                p[0].min(q[0]).clamp(0.0, 1.0),
                p[1].min(q[1]).clamp(0.0, 1.0),
                p[0].max(q[0]).clamp(0.0, 1.0),
                p[1].max(q[1]).clamp(0.0, 1.0),
            ];
            if !rect.iter().all(|v| v.is_finite()) {
                return None;
            }
            let target = if let Some(action) = annotation.get::<Dict<'_>>(b"A") {
                match action.get::<Name<'_>>(b"S")?.as_ref() {
                    b"URI" => {
                        String::from_utf8_lossy(action.get::<PdfString<'_>>(b"URI")?.as_bytes())
                            .into_owned()
                    }
                    b"GoTo" => destination(pdf, action.get::<Object<'_>>(b"D")?, 0)?,
                    b"GoToR" => {
                        String::from_utf8_lossy(action.get::<PdfString<'_>>(b"F")?.as_bytes())
                            .into_owned()
                    }
                    _ => return None,
                }
            } else {
                destination(pdf, annotation.get::<Object<'_>>(b"Dest")?, 0)?
            };
            Some(PreviewLink { rect, target })
        })
        .collect()
}

#[cfg(test)]
pub(crate) fn test_pdf() -> Vec<u8> {
    let objects = [
        "<< /Type /Catalog /Pages 2 0 R >>",
        "<< /Type /Pages /Count 1 /Kids [3 0 R] >>",
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 100] /Annots [4 0 R] /Contents 5 0 R >>",
        "<< /Type /Annot /Subtype /Link /Rect [10 20 100 40] /A << /S /URI /URI (https://example.com) >> >>",
        "<< /Length 27 >>\nstream\n1 0 0 rg 0 0 200 100 re f\nendstream",
    ];
    let mut pdf = String::from("%PDF-1.4\n");
    let mut offsets = vec![0];
    for (i, object) in objects.iter().enumerate() {
        offsets.push(pdf.len());
        pdf.push_str(&format!("{} 0 obj\n{object}\nendobj\n", i + 1));
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
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn inspection_and_thumbnail_need_no_host_utilities_or_writable_workspace() {
        let pdf = test_pdf();
        let root = Path::new("/nonexistent/read-only");
        let catalog = inspect_pdf(&pdf, root, || false).unwrap();
        assert_eq!(catalog.pages[0].size, [400, 200]);
        assert_eq!(catalog.pages[0].links[0].target, "https://example.com");
        assert_eq!(catalog.pages[0].links[0].rect, [0.05, 0.6, 0.5, 0.8]);
        let image = rasterize_pdf_first_page(&pdf, root, 100, || false).unwrap();
        assert_eq!(image.size, [100, 50]);
        assert_eq!(image.rgba.len(), 100 * 50 * 4);
        assert_eq!(&image.rgba[..4], &[255, 0, 0, 255]);
    }
    #[test]
    fn cancelled_invalid_and_oversized_requests_fail() {
        let pdf = test_pdf();
        let root = Path::new(".");
        assert!(inspect_pdf(&pdf, root, || true).is_err());
        assert!(inspect_pdf(b"not a PDF", root, || false).is_err());
        assert!(rasterize_pdf_first_page(&pdf, root, 0, || false).is_err());
        assert!(rasterize_pdf_first_page(&pdf, root, 20_000, || false).is_err());
        assert!(
            rasterize_pdf(
                &pdf,
                root,
                PdfRasterMode::PageRange {
                    first: 0,
                    last: 1,
                    dpi: 72
                },
                || false
            )
            .is_err()
        );
    }
}

#[cfg(test)]
mod probes {
    #[test]
    #[ignore = "opt-in optimized thumbnail timing and PNG inspection"]
    fn thumbnail_probe() {
        let path = std::env::var("TIPTOPTYP_PDF_PROBE").expect("set TIPTOPTYP_PDF_PROBE");
        let bytes = std::fs::read(path).unwrap();
        for run in 0..4 {
            let started = std::time::Instant::now();
            let page = super::rasterize_pdf_first_page(
                &bytes,
                std::path::Path::new("/nonexistent"),
                720,
                || false,
            )
            .unwrap();
            eprintln!(
                "thumbnail run={run} elapsed_us={} size={:?}",
                started.elapsed().as_micros(),
                page.size
            );
            if run == 3
                && let Ok(output) = std::env::var("TIPTOPTYP_PDF_PROBE_PNG")
            {
                image::save_buffer(
                    output,
                    &page.rgba,
                    page.size[0] as u32,
                    page.size[1] as u32,
                    image::ColorType::Rgba8,
                )
                .unwrap();
            }
        }
    }
}
