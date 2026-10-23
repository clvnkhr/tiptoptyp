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

#[test]
#[ignore = "optimized unchanged PDF Find component measurement"]
fn pdf_search_cache_measurement() {
    use std::hint::black_box;
    let source = "É İ mathematical preview token and other text. ".repeat(2048);
    let characters: Vec<_> = source
        .chars()
        .map(|value| Character {
            value,
            rect: Rect::NOTHING,
        })
        .collect();
    let query: Arc<str> = "PREVIEW".into();
    let expected = matching_characters(&characters, &query);
    let mut cache = SearchMatches::default();
    assert_eq!(cache.for_query(&characters, &query), expected);
    const FRAMES: usize = 60;
    const SAMPLES: usize = 15;
    let mut reference_ns = Vec::new();
    let mut cached_ns = Vec::new();
    for sample in 0..SAMPLES + 3 {
        let start = Instant::now();
        for _ in 0..FRAMES {
            black_box(matching_characters(
                black_box(&characters),
                black_box(query.as_ref()),
            ));
        }
        let reference = start.elapsed().as_secs_f64() * 1e9 / FRAMES as f64;
        let start = Instant::now();
        for _ in 0..FRAMES {
            black_box(cache.for_query(black_box(&characters), black_box(&query)));
        }
        let cached = start.elapsed().as_secs_f64() * 1e9 / FRAMES as f64;
        if sample >= 3 {
            reference_ns.push(reference);
            cached_ns.push(cached);
        }
    }
    assert_eq!(cache.rebuilds, 1);
    assert_eq!(cache.for_query(&characters, &query), expected);
    let median = |values: &[f64]| {
        let mut sorted = values.to_vec();
        sorted.sort_by(f64::total_cmp);
        sorted[sorted.len() / 2]
    };
    println!(
        "PDF_SEARCH_CACHE {}",
        serde_json::json!({
            "profile": if cfg!(debug_assertions) { "debug" } else { "release" },
            "fixture_bytes": source.len(), "fixture_characters": characters.len(),
            "query": query.as_ref(), "match_ranges": expected.len(),
            "warmup_samples": 3, "samples": SAMPLES, "frames_per_sample": FRAMES,
            "reference_ns_per_frame": reference_ns, "cached_ns_per_frame": cached_ns,
            "reference_median_ns": median(&reference_ns), "cached_median_ns": median(&cached_ns),
            "scope": "unchanged resident-page search matching only; excludes highlight painting, layout, PDFium, GPU, and input latency"
        })
    );
}
