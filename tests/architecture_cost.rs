//! Opt-in optimized costs; no timing thresholds in the normal test suite.
#[allow(dead_code)]
#[path = "../src/tinymist/transport.rs"]
mod framing;
#[allow(dead_code)]
#[path = "../src/project_index.rs"]
mod project_index;

use std::{collections::BTreeMap, hint::black_box, io::Cursor, time::Instant};

fn measure(name: &str, iterations: usize, mut operation: impl FnMut()) {
    for _ in 0..5 {
        operation();
    }
    for sample in 0..7 {
        let start = Instant::now();
        for _ in 0..iterations {
            operation();
        }
        println!(
            "{name},{sample},{iterations},{}",
            start.elapsed().as_nanos()
        );
    }
}

#[test]
#[ignore = "optimized local performance probe; not a wall-time CI assertion"]
fn architecture_cost_probe() {
    println!("case,sample,iterations,total_ns");
    for (name, count, missing) in [
        ("index_disk_1", 1, false),
        ("index_disk_64", 64, false),
        ("index_disk_256", 256, false),
        ("index_capped_300", 300, false),
        ("index_missing_64", 64, true),
    ] {
        let fixture = tempfile::tempdir().unwrap();
        let root = fixture.path().canonicalize().unwrap();
        let main = root.join("main.typ");
        let mut source = "= Main\n".to_owned();
        for i in 1..count {
            source.push_str(&format!("#include \"child-{i}.typ\"\n"));
            if !missing {
                std::fs::write(
                    root.join(format!("child-{i}.typ")),
                    format!("= Heading {i}\n#let value_{i} = {i}\n"),
                )
                .unwrap();
            }
        }
        std::fs::write(&main, &source).unwrap();
        let overrides = BTreeMap::new();
        let expected = if missing { 1 } else { count.min(256) };
        assert_eq!(
            project_index::analyze_project(&root, &main, &overrides)
                .outline
                .len(),
            expected
        );
        measure(name, 20, || {
            black_box(project_index::analyze_project(
                black_box(&root),
                black_box(&main),
                black_box(&overrides),
            ));
        });
    }
    for size in [128, 32_768] {
        let payload = serde_json::json!({"jsonrpc":"2.0","method":"textDocument/didChange","params":{"text":"λ".repeat(size / 2)}});
        let mut bytes = Vec::new();
        framing::write_lsp_message(&mut bytes, &payload).unwrap();
        assert_eq!(
            framing::read_lsp_message(&mut Cursor::new(&bytes)).unwrap(),
            Some(payload.clone())
        );
        measure(&format!("lsp_read_{size}"), 1000, || {
            black_box(framing::read_lsp_message(&mut Cursor::new(black_box(&bytes))).unwrap());
        });
        measure(&format!("lsp_write_{size}"), 1000, || {
            let mut output = Vec::new();
            framing::write_lsp_message(&mut output, black_box(&payload)).unwrap();
            black_box(output);
        });
    }
}
