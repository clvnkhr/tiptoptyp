//! Opt-in row-index preparation probe, excluding layout, widgets, tessellation and GPU work.
use super::{RowIndex, Rows};
use eframe::egui::{self, Color32, FontId, Galley, Pos2, Rect, text::LayoutJob};
use sha2::{Digest, Sha256};
use std::{fmt::Write as _, hint::black_box, sync::Arc, time::Instant};

const WARMUP: usize = 40;
const SAMPLES: usize = 240;
const OPERATIONS: usize = 16;
const ROUNDS: usize = 3;

#[derive(Clone, Copy)]
enum Path {
    Rebuilt,
    Cached,
}

impl Path {
    fn name(self) -> &'static str {
        match self {
            Self::Rebuilt => "rebuild_logical_and_sticky_rows",
            Self::Cached => "reuse_row_index",
        }
    }

    fn rows(self, index: &mut RowIndex, galley: &Arc<Galley>) -> Rows {
        match self {
            Self::Cached => index.rows(galley),
            Self::Rebuilt => {
                let logical = super::super::logical_line_row_ranges(&galley.rows);
                let sticky = super::super::sticky_context_scroll_lines(
                    &logical,
                    |row| galley.rows.get(row).map(|row| row.rect().top()),
                    |row| {
                        galley
                            .rows
                            .get(row)
                            .map(|row| row.char_count_including_newline().0)
                    },
                    |row| galley.rows.get(row).map(|row| row.ends_with_newline),
                )
                .expect("complete galley rows");
                Rows {
                    logical: logical.into(),
                    sticky: sticky.into(),
                }
            }
        }
    }
}

// Keep identical to the saved native Typst fixture in desktop_ui_effects_journeys.py.
fn fixture() -> String {
    let mut source = String::from(
        "#set page(width: 240pt, height: 260pt, margin: 20pt)\n\
         = Effects pacing\nA small live preview with substantial source.\n",
    );
    for index in 0..2000 {
        if index % 5 == 0 {
            writeln!(
                source,
                "#let pacing_sample_{index:04} = (left: {}, right: {}, gain: 1.25) // syntax cache fixture",
                index % 97,
                (index * 17) % 113,
            )
            .unwrap();
        } else {
            writeln!(
                source,
                "// Row {index:04}: amplitude {:02}, alpha beta gamma, coefficient {:03}, stable measurement fixture and deterministic frame pacing content.",
                index % 97,
                (index * 17) % 113,
            )
            .unwrap();
        }
    }
    source.push_str("\nPacing tail ");
    source
}

fn percentile(samples: &[f64], percent: usize) -> f64 {
    let mut sorted = samples.to_vec();
    sorted.sort_by(f64::total_cmp);
    sorted[(sorted.len() - 1) * percent / 100]
}

fn measure(path: Path, galley: &Arc<Galley>, round: usize) -> serde_json::Value {
    let mut index = RowIndex::default();
    let expected = Path::Rebuilt.rows(&mut index, galley);
    let first = path.rows(&mut index, galley);
    assert_eq!(first.logical.as_ref(), expected.logical.as_ref());
    assert_eq!(first.sticky.as_ref(), expected.sticky.as_ref());
    if matches!(path, Path::Cached) {
        let hit = index.rows(galley);
        assert!(Arc::ptr_eq(&first.logical, &hit.logical));
        assert!(Arc::ptr_eq(&first.sticky, &hit.sticky));
    }
    let mut samples = Vec::with_capacity(SAMPLES);
    for sample in 0..WARMUP + SAMPLES {
        let started = Instant::now();
        for _ in 0..OPERATIONS {
            let rows = path.rows(black_box(&mut index), black_box(galley));
            black_box((&rows.logical, &rows.sticky));
            drop(rows);
        }
        let microseconds = started.elapsed().as_secs_f64() * 1_000_000.0 / OPERATIONS as f64;
        if sample >= WARMUP {
            samples.push(microseconds);
        }
    }
    let p50 = percentile(&samples, 50);
    let p95 = percentile(&samples, 95);
    eprintln!(
        "source_row_index round={round} path={} logical_lines={} visual_rows={} warmup={WARMUP} samples={SAMPLES} operations_per_sample={OPERATIONS} p50_us={p50:.3} p95_us={p95:.3}; excludes_layout_widgets_tessellation_gpu=true",
        path.name(),
        expected.logical.len(),
        galley.rows.len(),
    );
    serde_json::json!({
        "round": round,
        "path": path.name(),
        "p50_us": p50,
        "p95_us": p95,
        "mean_us": samples.iter().sum::<f64>() / samples.len() as f64,
        "samples_us_per_operation": samples,
    })
}

#[test]
#[ignore = "opt-in matched optimized source row-index timing; excludes layout, widgets and GPU"]
fn source_row_index_cost_probe() {
    assert!(!black_box(cfg!(debug_assertions)), "run with --release");
    let source = fixture();
    let lines = source.bytes().filter(|&byte| byte == b'\n').count() + 1;
    assert_eq!(lines, 2005);
    let fixture_hash = format!("{:x}", Sha256::digest(source.as_bytes()));
    let context = egui::Context::default();
    context.set_theme(egui::Theme::Dark);
    context.set_pixels_per_point(1.0);
    crate::theme::configure_test_fonts(&context);
    let mut galley = None;
    let output = context.run_ui(
        egui::RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, egui::vec2(800.0, 640.0))),
            ..Default::default()
        },
        |ui| {
            galley = Some(ui.fonts_mut(|fonts| {
                fonts.layout_job(LayoutJob::simple(
                    source.clone(),
                    FontId::monospace(14.0),
                    Color32::WHITE,
                    760.0,
                ))
            }));
        },
    );
    output.drop_without_applying_deltas();
    let galley = galley.expect("laid-out fixture");
    let mut runs = Vec::with_capacity(ROUNDS * 2);
    for round in 0..ROUNDS {
        let paths = if round % 2 == 0 {
            [Path::Rebuilt, Path::Cached]
        } else {
            [Path::Cached, Path::Rebuilt]
        };
        for path in paths {
            runs.push(measure(path, &galley, round));
        }
    }
    let report = serde_json::json!({
        "schema": 1,
        "scope": "source logical/sticky row-index preparation only; no text layout, UI widgets, tessellation, GPU execution or native presentation",
        "profile": "release",
        "os": std::env::consts::OS,
        "arch": std::env::consts::ARCH,
        "fixture": {
            "generator": "desktop_ui_effects_journeys.effects_pacing_fixture(typst=True)",
            "bytes": source.len(),
            "sha256": fixture_hash,
            "logical_lines": lines,
            "visual_rows": galley.rows.len(),
        },
        "viewport": [800, 640],
        "wrap_width": 760,
        "font": "monospace14",
        "theme": "egui-dark",
        "scale": 1,
        "same_galley_for_all_runs": true,
        "warmup_samples": WARMUP,
        "samples_per_round": SAMPLES,
        "operations_per_sample": OPERATIONS,
        "rounds": ROUNDS,
        "runs": runs,
    });
    if let Some(path) = std::env::var_os("TIPTOPTYP_ROW_INDEX_PROBE_JSON") {
        std::fs::write(path, serde_json::to_vec_pretty(&report).unwrap())
            .expect("write optional row-index probe report");
    }
}
