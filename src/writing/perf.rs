//! Opt-in worker measurements. This adds no production instrumentation or I/O.
use super::*;
use std::{hint::black_box, time::Instant};

const PARAGRAPHS: usize = 160;
const SAMPLES: usize = 7;
const WARMUP: usize = 2;

fn fixture(kind: DocumentKind) -> String {
    let mut source = match kind {
        DocumentKind::Typst => "= Writing worker fixture\n\n".to_owned(),
        DocumentKind::Tex => "\\section{Writing worker fixture}\n\n".to_owned(),
        _ => unreachable!(),
    };
    for index in 0..PARAGRAPHS {
        source.push_str(&format!(
            "The estimate in paragraph {index} controls the smooth solution and preserves well-posedness. \
             The proof uses the energy bound\n"
        ));
        source.push_str(match kind {
            DocumentKind::Typst => "$ integral_0^1 norm(u(t))^2 dif t <= C, $\n",
            DocumentKind::Tex => "\\[ \\int_0^1 \\lVert u(t)\\rVert^2 \\,dt \\leq C, \\]\n",
            _ => unreachable!(),
        });
        source.push_str(&format!(
            "and gives a uniform estimate. The bounded operator in paragraph {index} maps the space \
             into itself. "
        ));
        source.push_str(if index == PARAGRAPHS / 2 {
            "This is the the mathematical argument.\n\n"
        } else if index == PARAGRAPHS - 1 {
            "This mathematical argument is misspelld.\n\n"
        } else {
            "This mathematical argument is stable under small changes.\n\n"
        });
    }
    source
}

fn edit(source: &str, sample: usize) -> String {
    let index = PARAGRAPHS / 2;
    let from = format!("The bounded operator in paragraph {index}");
    let replacement = if sample.is_multiple_of(2) {
        format!("The compact operator in paragraph {index}")
    } else {
        format!("The continuous\noperator in paragraph {index}")
    };
    // Keep every measured edit distinct from previously cached revisions.
    let replacement = format!("{replacement} at step {sample}");
    assert_eq!(source.matches(&from).count(), 1);
    source.replacen(&from, &replacement, 1).replacen(
        "the the mathematical argument",
        "the mathematical argument",
        1,
    )
}

fn timed<T>(work: impl FnOnce() -> T) -> (T, f64) {
    let start = Instant::now();
    let result = black_box(work());
    (result, start.elapsed().as_secs_f64() * 1_000.0)
}

fn fingerprint(source: &str) -> String {
    // Stable fixture identity, not a cryptographic provenance claim.
    let hash = source
        .bytes()
        .fold(0xcbf2_9ce4_8422_2325_u64, |hash, byte| {
            (hash ^ u64::from(byte)).wrapping_mul(0x0100_0000_01b3)
        });
    format!("fnv1a64:{hash:016x}")
}

fn median(values: &[f64]) -> f64 {
    let mut sorted = values.to_vec();
    sorted.sort_by(f64::total_cmp);
    sorted[sorted.len() / 2]
}

#[test]
#[ignore = "opt-in matched optimized Harper worker benchmark; run with --release --test-threads=1"]
fn incremental_writing_measurement() {
    assert!(
        !black_box(cfg!(debug_assertions)),
        "run this measurement with --release"
    );
    let language = crate::settings::WritingLanguage::Auto;
    let mut workloads = Vec::new();
    for (name, kind, path) in [
        ("typst", DocumentKind::Typst, Path::new("measurement.typ")),
        ("tex", DocumentKind::Tex, Path::new("measurement.tex")),
    ] {
        let source = fixture(kind);
        let changes: Vec<_> = (0..SAMPLES).map(|sample| edit(&source, sample)).collect();
        let (expected, process_first_uncached_ms) =
            timed(|| check_uncached(&source, kind, path, true, false, language));
        for _ in 0..WARMUP {
            black_box(check_uncached(&source, kind, path, true, false, language));
        }

        assert!(
            expected.len() >= 2,
            "fixture must contain diagnostic work: {name}"
        );
        let mut construction = Vec::new();
        for _ in 0..SAMPLES {
            let (_, elapsed) =
                timed(|| LintGroup::new_curated(Arc::clone(&WRITING_DICTIONARY), Dialect::British));
            construction.push(elapsed);
        }
        let mut baseline = Vec::new();
        let mut baseline_edits = Vec::new();
        let mut checker_cold = Vec::new();
        let mut repeats = Vec::new();
        let mut edits = Vec::new();
        let mut edit_stats = Vec::new();
        let mut checker = Checker::default();
        let (actual, checker_first_ms) =
            timed(|| checker.check(&source, kind, path, true, false, language));
        assert_eq!(actual, expected, "initial cached result: {name}");
        let first_stats = checker.cache_stats();
        for _ in 0..WARMUP {
            assert_eq!(
                checker.check(&source, kind, path, true, false, language),
                expected,
                "warm cache: {name}"
            );
        }

        // Alternate uncached/cold/repeat/edit samples in one optimized test
        // process. Every edit is immediately compared with a whole-document
        // reference, so the speed figure cannot conceal stale diagnostics.
        for changed in &changes {
            let (_, elapsed) = timed(|| check_uncached(&source, kind, path, true, false, language));
            baseline.push(elapsed);

            let mut fresh = Checker::default();
            let (actual, elapsed) =
                timed(|| fresh.check(&source, kind, path, true, false, language));
            assert_eq!(actual, expected, "cold cache: {name}");
            checker_cold.push(elapsed);

            // Prepare the exact unchanged document outside its timed hit.
            checker.check(&source, kind, path, true, false, language);
            let (actual, elapsed) =
                timed(|| checker.check(&source, kind, path, true, false, language));
            assert_eq!(actual, expected, "repeated document: {name}");
            repeats.push(elapsed);

            let (actual, elapsed) =
                timed(|| checker.check(changed, kind, path, true, false, language));
            let stats = checker.cache_stats();
            assert_eq!(stats.1, 1, "novel edit must miss exactly one block: {name}");
            assert!(
                stats.0 >= PARAGRAPHS - 1,
                "novel edit must retain the unchanged paragraphs: {name}: {stats:?}"
            );
            edits.push(elapsed);
            edit_stats.push(stats);
            let (changed_expected, elapsed) =
                timed(|| check_uncached(changed, kind, path, true, false, language));
            baseline_edits.push(elapsed);
            assert!(changed_expected.len() < expected.len());
            assert_eq!(actual, changed_expected, "one-paragraph edit: {name}");
        }
        workloads.push(serde_json::json!({
            "language": name,
            "fixture_bytes": source.len(),
            "fixture_scalars": source.chars().count(),
            "fixture_fingerprint": fingerprint(&source),
            "paragraphs": PARAGRAPHS,
            "grammar": true,
            "unicode_warnings": false,
            "dialect": "british",
            "language_selection": "auto",
            "samples": SAMPLES,
            "warmup_calls": WARMUP,
            "process_first_uncached_ms": process_first_uncached_ms,
            "checker_first_ms": checker_first_ms,
            "checker_first_cache_hits_misses": first_stats,
            "new_curated_construction_ms": construction,
            "uncached_full_ms": baseline,
            "uncached_one_paragraph_edit_ms": baseline_edits,
            "checker_cold_ms": checker_cold,
            "repeat_hit_ms": repeats,
            "one_paragraph_edit_ms": edits,
            "edit_cache_hits_misses": edit_stats,
            "median_ms": {
                "new_curated_construction": median(&construction),
                "uncached_full": median(&baseline),
                "uncached_one_paragraph_edit": median(&baseline_edits),
                "checker_cold": median(&checker_cold),
                "repeat_hit": median(&repeats),
                "one_paragraph_edit": median(&edits),
            },
            "diagnostics": expected.len(),
        }));
    }
    println!(
        "writing-performance-json {}",
        serde_json::json!({
            "schema": 1,
            "profile": "release",
            "os": std::env::consts::OS,
            "architecture": std::env::consts::ARCH,
            "scope": "worker grammar checks; syntax admission, parsing, linting, diagnostic mapping; no GUI, scheduling delay, filesystem-cold startup, or renderer",
            "reference": "full single Harper Document plus new curated LintGroup per request",
            "workloads": workloads,
        })
    );
}
