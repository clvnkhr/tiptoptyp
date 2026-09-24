//! Bounded comparison of the allocation primitives removed in the 901c audit.
//! rustc -O scripts/probes/editor-allocation.rs -o /tmp/editor-allocation
//! /tmp/editor-allocation
//! This excludes parsing, layout, GUI presentation, and cache invalidation.
use std::{borrow::Cow, hint::black_box, time::Instant};

fn measure(mut operation: impl FnMut(), iterations: usize) -> u128 {
    for _ in 0..100 {
        operation();
    }
    let start = Instant::now();
    for _ in 0..iterations {
        operation();
    }
    start.elapsed().as_nanos() / iterations as u128
}

fn main() {
    let source = "#let value = 42\n".repeat(70_000);
    let cached = source.clone();
    let matches = (0..10_000)
        .map(|i| (i * 4..i * 4 + 3, i * 4..i * 4 + 3))
        .collect::<Vec<_>>();
    for sample in 0..5 {
        let copied_source = measure(
            || {
                let parse_source = black_box(source.as_str()).to_owned();
                black_box(black_box(cached.as_str()) == parse_source);
            },
            2_000,
        );
        let borrowed_source = measure(
            || {
                let parse_source = Cow::Borrowed(black_box(source.as_str()));
                black_box(black_box(cached.as_str()) == parse_source.as_ref());
            },
            2_000,
        );
        let cloned_matches = measure(
            || {
                let cloned = black_box(&matches).clone();
                black_box(&cloned);
            },
            2_000,
        );
        // An unchanged replacement restores the moved buffer into the cache.
        let mut retained = matches.clone();
        let moved_matches = measure(
            || {
                let moved = std::mem::take(black_box(&mut retained));
                black_box(&moved);
                retained = moved;
            },
            2_000,
        );
        println!("sample={sample} bytes={} matches={} source_copy_ns={copied_source} source_borrow_ns={borrowed_source} matches_clone_ns={cloned_matches} matches_move_ns={moved_matches}", source.len(), matches.len());
    }
}
