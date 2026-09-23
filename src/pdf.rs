//! Bounded PDF thumbnails for hover previews. PDFium owns document viewing.
use hayro::{RenderCache, RenderSettings, hayro_syntax::Pdf};
use std::path::Path;

pub const PREVIEW_DPI: f32 = 144.0;
const MAX_PDF_PAGES: usize = 10_000;
const MAX_RENDER_PIXELS: usize = 32 * 1024 * 1024;

#[derive(Debug)]
pub struct PreviewPage {
    pub size: [usize; 2],
    pub rgba: Vec<u8>,
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
pub(crate) fn rasterize_pdf_first_page(
    pdf: &[u8],
    _project_root: &Path,
    max_dimension: u32,
    mut cancelled: impl FnMut() -> bool,
) -> Result<PreviewPage, String> {
    check_cancelled(&mut cancelled)?;
    let pdf = load(pdf)?;
    let page = &pdf.pages()[0];
    let (width, height) = page.render_dimensions();
    let scale = max_dimension as f32 / width.max(height);
    let mut size = dimensions(page, scale)?;
    for dimension in &mut size {
        *dimension = (*dimension).min(max_dimension as usize);
    }
    if size[0].saturating_mul(size[1]) > MAX_RENDER_PIXELS {
        return Err("PDF thumbnail exceeds the 32 megapixel request limit".into());
    }
    let settings = RenderSettings {
        x_scale: scale,
        y_scale: scale,
        width: Some(size[0] as u16),
        height: Some(size[1] as u16),
        bg_color: hayro::vello_cpu::color::palette::css::WHITE,
    };
    let pixels = hayro::render(page, &RenderCache::new(), &Default::default(), &settings);
    check_cancelled(&mut cancelled)?;
    Ok(PreviewPage {
        size,
        rgba: pixels.data_as_u8_slice().to_vec(),
    })
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
    fn thumbnail_needs_no_host_utilities_or_writable_workspace() {
        let pdf = test_pdf();
        let root = Path::new("/nonexistent/read-only");
        let image = rasterize_pdf_first_page(&pdf, root, 100, || false).unwrap();
        assert_eq!(image.size, [100, 50]);
        assert_eq!(image.rgba.len(), 100 * 50 * 4);
        assert_eq!(&image.rgba[..4], &[255, 0, 0, 255]);
    }
    #[test]
    fn cancelled_invalid_and_oversized_requests_fail() {
        let pdf = test_pdf();
        let root = Path::new(".");
        assert!(rasterize_pdf_first_page(&pdf, root, 100, || true).is_err());
        assert!(rasterize_pdf_first_page(b"not a PDF", root, 100, || false).is_err());
        assert!(rasterize_pdf_first_page(&pdf, root, 0, || false).is_err());
        assert!(rasterize_pdf_first_page(&pdf, root, 20_000, || false).is_err());
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
