//! Opt-in CPU publication measurement; no desktop or native raster timing.
use super::*;

#[test]
#[ignore = "optimized PDFium UI publication probe"]
fn pdfium_publication_probe() {
    let context = egui::Context::default();
    let catalog = Arc::new(Catalog {
        sizes: vec![[595.0, 842.0]],
        outline: vec![],
    });
    let mut cases = Vec::new();
    for size in [[1200, 1697], [2000, 2828]] {
        for palette in [
            (Color32::WHITE, Color32::BLACK),
            (
                Color32::from_rgb(32, 45, 60),
                Color32::from_rgb(190, 200, 220),
            ),
        ] {
            let mut view = PdfiumView::default();
            let mut samples = Vec::new();
            for iteration in 0..18 {
                let mut rgba = Vec::with_capacity(size[0] * size[1] * 4);
                for pixel in 0..size[0] * size[1] {
                    rgba.extend_from_slice(if pixel % 2 == 0 {
                        &[0, 64, 128, 255]
                    } else {
                        &[255, 128, 64, 255]
                    });
                }
                let mut batch = Some(Batch {
                    catalog: catalog.clone(),
                    pages: vec![crate::pdfium::Page {
                        index: 0,
                        image: crate::pdfium::prepared_image(size, rgba, palette),
                        characters: vec![],
                        links: vec![],
                    }],
                    parsed_new_document: false,
                });
                let key = RequestKey {
                    revision: 1,
                    pages: vec![0],
                    scale: 100 + iteration as u32,
                    query: String::new(),
                    palette,
                };
                let mut elapsed = 0.0;
                context
                    .run_ui(
                        egui::RawInput {
                            screen_rect: Some(Rect::from_min_size(
                                Pos2::ZERO,
                                egui::vec2(1400.0, 900.0),
                            )),
                            ..Default::default()
                        },
                        |ui| {
                            let started = Instant::now();
                            view.accept(batch.take().unwrap(), &key, ui);
                            elapsed = started.elapsed().as_secs_f64() * 1000.0;
                        },
                    )
                    .drop_without_applying_deltas();
                std::hint::black_box(&view);
                if iteration >= 3 {
                    samples.push(elapsed);
                }
            }
            let mut sorted = samples.clone();
            sorted.sort_by(f64::total_cmp);
            cases.push(serde_json::json!({"pixels":size,"palette":[palette.0.to_array(),palette.1.to_array()],"samples_ms":samples,"p50_ms":sorted[7],"p95_ms":sorted[14]}));
        }
    }
    println!(
        "PDFIUM_PUBLICATION {}",
        serde_json::json!({"profile":if cfg!(debug_assertions){"debug"}else{"release"},"viewport_points":[1400,900],"warmups":3,"samples":15,"cases":cases,"scope":"CPU UI admission including texture queue, excludes raster, GPU upload and presentation"})
    );
}
