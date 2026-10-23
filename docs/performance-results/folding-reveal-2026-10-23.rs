#![allow(dead_code)]
mod document {
    pub use tiptoptyp_core::document::DocumentKey;
}
mod editor_features {
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub(crate) enum StickyContextKind {
        Heading { level: usize },
        LetBinding,
        Function,
        Block,
    }
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub(crate) struct StickyContextRow {
        pub(crate) kind: StickyContextKind,
        /// One-based source line.
        pub(crate) line: usize,
        /// Character position of the first non-whitespace source character.
        pub(crate) char_index: usize,
        /// One-based source line whose top pushes this context out of the overlay.
        /// A value past the final source line means the context lasts to EOF.
        pub(crate) end_line: usize,
        /// Trimmed, otherwise verbatim original source line.
        pub(crate) text: String,
    }
    #[derive(Debug, Clone)]
    pub(crate) struct ContextRegion {
        pub(crate) range: std::ops::Range<usize>,
        pub(crate) rows: Vec<StickyContextRow>,
    }
}
#[path = "../../src/folding.rs"]
mod folding;
use editor_features::{ContextRegion, StickyContextKind, StickyContextRow};
use std::{hint::black_box, sync::Arc, time::Instant};
use tiptoptyp_core::document::{DocumentKey, WindowSessionId};
fn fixture(n: usize, nested: bool) -> (String, Vec<ContextRegion>) {
    let mut source = String::new();
    let mut regions = Vec::new();
    for i in 0..n {
        let start = source.len();
        let scalar = source.chars().count();
        source.push_str(&format!("= Heading {i}\n"));
        let line = if nested { i + 1 } else { 2 * i + 1 };
        if !nested {
            source.push_str("body Ω\n");
        }
        regions.push(ContextRegion {
            range: start..source.len(),
            rows: vec![StickyContextRow {
                kind: StickyContextKind::Heading {
                    level: if nested { i + 1 } else { 1 },
                },
                line,
                char_index: scalar,
                end_line: if nested { n + 2 } else { line + 2 },
                text: format!("Heading {i}"),
            }],
        });
    }
    if nested {
        source.push_str("body Ω\n");
    }
    source.push_str("visible tail\n");
    (source, regions)
}
fn run(n: usize, nested: bool) {
    let (source, regions) = fixture(n, nested);
    let bytes = source.len();
    let fingerprint = source.bytes().fold(0xcbf29ce484222325u64, |a, b| {
        (a ^ u64::from(b)).wrapping_mul(0x100000001b3)
    });
    let mut folding = folding::Folding::default();
    folding.prepare(
        DocumentKey::new(WindowSessionId::new(1), 0, 0),
        Arc::from(source),
        &regions,
    );
    folding.collapse_all();
    for _ in 0..2 {
        assert!(!black_box(folding.reveal(black_box(0))));
    }
    let loops = 10;
    let mut times = Vec::new();
    for _ in 0..7 {
        let start = Instant::now();
        for _ in 0..loops {
            assert!(!black_box(folding.reveal(black_box(0))));
        }
        times.push(start.elapsed().as_nanos() / loops);
    }
    times.sort();
    println!(
        "fixture={} regions={} bytes={} fnv1a={fingerprint:016x} cursor=0 warmup=2 samples=7 iterations_per_sample={loops} min_ns={} median_ns={} max_ns={}",
        if nested { "nested" } else { "siblings" },
        folding.regions.len(),
        bytes,
        times[0],
        times[3],
        times[6]
    );
}
fn main() {
    println!(
        "optimized actual production folding.rs; cfg(test)=false; rustc 1.98.1; -O -C codegen-units=1; aarch64-apple-darwin"
    );
    run(4_000, false);
    run(1_500, true);
}
