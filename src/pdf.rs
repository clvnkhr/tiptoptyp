//! Shared PDF pixel data and bounded thumbnail geometry. PDFium owns rendering.

pub const PREVIEW_DPI: f32 = 144.0;
const MAX_RENDER_PIXELS: usize = 32 * 1024 * 1024;

#[derive(Debug)]
pub struct PreviewPage {
    pub size: [usize; 2],
    pub rgba: Vec<u8>,
}

pub(crate) fn thumbnail_size(page: [f32; 2], max_dimension: u32) -> Result<[usize; 2], String> {
    if max_dimension == 0
        || max_dimension > 65535
        || page.iter().any(|value| !value.is_finite() || *value <= 0.0)
    {
        return Err("PDF page dimensions are outside the supported range".into());
    }
    let scale = f64::from(max_dimension) / f64::from(page[0].max(page[1]));
    let size = page
        .map(|value| ((f64::from(value) * scale).ceil() as usize).clamp(1, max_dimension as usize));
    if size[0].saturating_mul(size[1]) > MAX_RENDER_PIXELS {
        return Err("PDF thumbnail exceeds the 32 megapixel request limit".into());
    }
    Ok(size)
}

#[cfg(test)]
pub(crate) fn test_pdf() -> Vec<u8> {
    test_pdf_page("", "1 0 0 rg 0 0 200 100 re f")
}

#[cfg(test)]
pub(crate) fn test_pdf_page(attributes: &str, content: &str) -> Vec<u8> {
    let page = format!(
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 100] {attributes} /Annots [4 0 R] /Contents 5 0 R >>"
    );
    let stream = format!(
        "<< /Length {} >>\nstream\n{content}\nendstream",
        content.len()
    );
    let objects = [
        "<< /Type /Catalog /Pages 2 0 R >>",
        "<< /Type /Pages /Count 1 /Kids [3 0 R] >>",
        &page,
        "<< /Type /Annot /Subtype /Link /Rect [10 20 100 40] /A << /S /URI /URI (https://example.com) >> >>",
        &stream,
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
    fn thumbnail_geometry_preserves_aspect_ratio_and_bounds_allocations() {
        assert_eq!(thumbnail_size([200.0, 100.0], 100).unwrap(), [100, 50]);
        assert_eq!(thumbnail_size([100.0, 200.0], 100).unwrap(), [50, 100]);
        assert_eq!(thumbnail_size([200.0, 100.0], 1).unwrap(), [1, 1]);
        for page in [
            [0.0, 100.0],
            [-1.0, 100.0],
            [f32::NAN, 100.0],
            [100.0, f32::INFINITY],
        ] {
            assert!(thumbnail_size(page, 100).is_err());
        }
        for limit in [0, 20_000, u32::MAX] {
            assert!(thumbnail_size([200.0, 100.0], limit).is_err());
        }
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
            let page = crate::pdfium::thumbnail(&bytes, 720, || false).unwrap();
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
