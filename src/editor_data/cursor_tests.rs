//! Cached source coordinates must match scalar-based editor positions.
use super::*;
use tiptoptyp_core::document::WindowSessionId;

fn revision(value: u64) -> DocumentKey {
    DocumentKey::new(WindowSessionId::new(1), 0, value)
}

fn assert_positions(source: &str) {
    let mut data = EditorDerivedData::default();
    data.prepare_source(&DocumentSnapshot::fixture(revision(1), source));
    let mut line = 1;
    let mut column = 1;
    for (at, character) in source.chars().enumerate() {
        assert_eq!(
            data.line_column_at_char(at),
            (line, column),
            "{source:?} at {at}"
        );
        assert_eq!(data.line_index_at_char(at), line - 1, "{source:?} at {at}");
        if character == '\n' {
            line += 1;
            column = 1;
        } else {
            column += 1;
        }
    }
    for at in [
        source.chars().count(),
        source.chars().count() + 1,
        usize::MAX,
    ] {
        assert_eq!(
            data.line_column_at_char(at),
            (line, column),
            "{source:?} at {at}"
        );
        assert_eq!(data.line_index_at_char(at), line - 1, "{source:?} at {at}");
    }
}

#[test]
fn cached_coordinates_match_scalar_positions_newlines_and_out_of_range_carets() {
    for source in [
        "",
        "\n",
        "\r\n",
        "first\nsecond",
        "\nα🦀中\r\n\ne\u{301}\n",
        "plain\rtail",
    ] {
        assert_positions(source);
    }
    // Exhaust all four-character combinations of distinct scalar widths,
    // line endings and a combining mark; no external generator or timing bound.
    let alphabet = ['a', '\n', '\r', '中', '🦀', '\u{301}'];
    for mut case in 0..alphabet.len().pow(4) {
        let mut source = String::new();
        for _ in 0..4 {
            source.push(alphabet[case % alphabet.len()]);
            case /= alphabet.len();
        }
        assert_positions(&source);
    }
}

#[test]
fn cursor_queries_reuse_the_revision_index_and_refresh_for_replaced_documents() {
    let mut data = EditorDerivedData::default();
    assert_eq!(data.line_column_at_char(usize::MAX), (1, 1));
    let source = "α\n中🦀\n";
    let snapshot = DocumentSnapshot::fixture(revision(3), source);
    data.prepare_source(&snapshot);
    let starts = data.line_starts.as_ptr();
    let bytes = data.char_starts.as_ptr();
    for _ in 0..120 {
        data.prepare_source(&snapshot);
        assert_eq!(data.line_column_at_char(4), (2, 3));
        assert_eq!(data.line_column_at_char(usize::MAX), (3, 1));
        assert_eq!(data.line_starts.as_ptr(), starts);
        assert_eq!(data.char_starts.as_ptr(), bytes);
    }
    assert_eq!(data.rebuild_counts(), (1, 0));
    assert_eq!(
        data.syntax_rebuilds, 0,
        "coordinate lookup does not parse syntax"
    );

    let replacements = [
        (revision(4), "β\r\nnew", (2, 4)),
        (
            DocumentKey::new(WindowSessionId::new(1), 1, 4),
            "\n\n",
            (3, 1),
        ),
        (DocumentKey::new(WindowSessionId::new(2), 1, 4), "", (1, 1)),
    ];
    for (key, text, expected) in replacements {
        data.prepare_source(&DocumentSnapshot::fixture(key, text));
        assert_eq!(data.line_column_at_char(usize::MAX), expected);
        assert_eq!(data.line_index_at_char(usize::MAX), expected.0 - 1);
    }
    assert_eq!(data.rebuild_counts(), (4, 0));
}

fn reference_repaint_queries(source: &str, at: usize) -> ((usize, usize), usize) {
    // Previous production path: status clamps by counting the complete source,
    // then status and current-line painting independently scan to the caret.
    let at = at.min(source.chars().count());
    let mut line = 1;
    let mut column = 1;
    for character in source.chars().take(at) {
        if character == '\n' {
            line += 1;
            column = 1;
        } else {
            column += 1;
        }
    }
    let highlight_line = source
        .chars()
        .take(at)
        .filter(|&character| character == '\n')
        .count();
    ((line, column), highlight_line)
}

fn percentile(samples: &[f64], percent: usize) -> f64 {
    let mut sorted = samples.to_vec();
    sorted.sort_by(f64::total_cmp);
    sorted[(sorted.len() - 1) * percent / 100]
}

#[test]
#[ignore = "opt-in matched optimized cursor lookup measurement; no timing assertion"]
fn cursor_coordinate_repaint_cost_probe() {
    use sha2::{Digest, Sha256};
    use std::{fmt::Write as _, hint::black_box, time::Instant};

    assert!(!black_box(cfg!(debug_assertions)), "run with --release");
    const WARMUP: usize = 16;
    const SAMPLES: usize = 80;
    const OPERATIONS: usize = 16;
    const ROUNDS: usize = 3;
    let mut fixtures = Vec::new();
    for lines in [1000, 20_000] {
        let mut source = String::new();
        for line in 0..lines {
            writeln!(source, "Row {line:05}: αβγ 🦀 e\u{301} 中 ordinary prose and mathematics at the document tail.").unwrap();
        }
        let at = source.chars().count();
        let mut data = EditorDerivedData::default();
        data.prepare_source(&DocumentSnapshot::fixture(revision(1), &source));
        let expected = reference_repaint_queries(&source, at);
        assert_eq!(
            (data.line_column_at_char(at), data.line_index_at_char(at)),
            expected
        );
        let mut runs = Vec::new();
        for round in 0..ROUNDS {
            for indexed in if round % 2 == 0 {
                [false, true]
            } else {
                [true, false]
            } {
                let mut samples = Vec::with_capacity(SAMPLES);
                for sample in 0..WARMUP + SAMPLES {
                    let started = Instant::now();
                    for _ in 0..OPERATIONS {
                        let result = if indexed {
                            let cached = black_box(&data);
                            let cursor = black_box(at);
                            (
                                cached.line_column_at_char(cursor),
                                cached.line_index_at_char(cursor),
                            )
                        } else {
                            reference_repaint_queries(black_box(&source), black_box(at))
                        };
                        black_box(result);
                    }
                    let us = started.elapsed().as_secs_f64() * 1_000_000.0 / OPERATIONS as f64;
                    if sample >= WARMUP {
                        samples.push(us);
                    }
                }
                let p50 = percentile(&samples, 50);
                let p95 = percentile(&samples, 95);
                let path = if indexed {
                    "indexed"
                } else {
                    "reference_prefix_scans"
                };
                eprintln!(
                    "cursor_coordinates path={path} source_lines={lines} source_bytes={} cursor=EOF round={round} p50_us={p50:.3} p95_us={p95:.3}; excludes_source_index_build_layout_widgets_gpu=true",
                    source.len()
                );
                runs.push(serde_json::json!({
                    "round": round,
                    "path": path,
                    "p50_us": p50,
                    "p95_us": p95,
                    "samples_us_per_repaint": samples,
                }));
            }
        }
        assert_eq!(data.rebuild_counts(), (1, 0));
        fixtures.push(serde_json::json!({
            "source_lines": lines,
            "logical_lines": data.source_metrics().line_count,
            "bytes": source.len(),
            "sha256": hex::encode(Sha256::digest(source.as_bytes())),
            "scalar_count": at,
            "cursor": "EOF",
            "runs": runs,
        }));
    }
    let report = serde_json::json!({
        "schema": 1,
        "scope": "status cursor coordinates and current-line index queries on unchanged source only; excludes source-index construction, layout, widgets, tessellation and GPU",
        "profile": "release",
        "os": std::env::consts::OS,
        "arch": std::env::consts::ARCH,
        "fixture_generator": "cursor_tests::cursor_coordinate_repaint_cost_probe, Unicode source rows",
        "viewport_theme": "not applicable: pure source-coordinate queries",
        "warmup_samples": WARMUP,
        "samples_per_round": SAMPLES,
        "operations_per_sample": OPERATIONS,
        "rounds": ROUNDS,
        "fixtures": fixtures,
    });
    if let Some(path) = std::env::var_os("TIPTOPTYP_CURSOR_PROBE_JSON") {
        std::fs::write(path, serde_json::to_vec_pretty(&report).unwrap()).unwrap();
    }
}
