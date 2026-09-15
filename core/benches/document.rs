//! Repeatable headless scaling probes, not machine-dependent pass/fail limits.
use std::{hint::black_box, time::Instant};
use tiptoptyp_core::document::{DocumentKind, DocumentSession, WindowSessionId};

fn measure(name: &str, bytes: usize, iterations: usize, mut operation: impl FnMut()) {
    for _ in 0..32 {
        operation();
    }
    let started = Instant::now();
    for _ in 0..iterations {
        operation();
    }
    println!(
        "{name},{bytes},{iterations},{:.1}",
        started.elapsed().as_nanos() as f64 / iterations as f64
    );
}

fn main() {
    println!("workload,source_bytes,iterations,ns_per_operation");
    for bytes in [16 * 1024, 256 * 1024, 1024 * 1024] {
        let source = "// Typst αβγ source\n".repeat(bytes / "// Typst αβγ source\n".len());
        let mut document =
            DocumentSession::new(WindowSessionId::new(1), source, DocumentKind::Typst);
        let bytes = document.source().len();
        measure("snapshot", bytes, 100_000, || {
            black_box(document.snapshot());
        });
        let key = document.key();
        measure("no_op_edit", bytes, 1_000, || {
            document.edit((), |text| {
                black_box(text.len());
            });
        });
        assert_eq!(
            document.key(),
            key,
            "no-op edits must not invalidate consumers"
        );
        measure("edit_and_undo", bytes, 200, || {
            document.edit((), |text| text.push('x'));
            black_box(document.history_step(false, ()).expect("edit is undoable"));
            black_box(document.take_edit());
        });
        assert_eq!(document.source().len(), bytes);
    }
}
