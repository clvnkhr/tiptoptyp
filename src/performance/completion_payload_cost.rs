//! Isolates the unchanged-frame payload handoff, not egui layout or GPU costs.
use crate::{lsp::protocol, test_allocations::allocated};
use std::{hint::black_box, time::Instant};

fn fixture() -> Vec<protocol::CompletionItem> {
    (0..256)
        .map(|index| protocol::CompletionItem {
            origin: protocol::CompletionOrigin::LanguageServer,
            label: format!("completion_{index}"),
            detail: None,
            documentation: Some("documentation ".repeat(300)),
            insert_text: "alpha".into(),
            insert_text_is_snippet: false,
            text_edit: None,
            additional_text_edits: Vec::new(),
            sort_text: None,
            filter_text: None,
        })
        .collect()
}

#[test]
fn borrowed_popup_payload_has_no_documentation_sized_allocations() {
    let items = fixture();
    let (_, before) = allocated(|| {
        black_box(items.clone());
    });
    let (_, after) = allocated(|| {
        black_box(items.as_slice());
    });
    assert!(before > 1_000_000);
    assert_eq!(after, 0);
    // Enforce that the actual view keeps using the borrowed handoff measured here.
    let renderer = include_str!("../app/completion_popup.rs")
        .split("#[cfg(test)]")
        .next()
        .unwrap();
    let adapter = include_str!("../app/editor_view.rs");
    let adapter = adapter
        .split("fn show_editor_completion_popup")
        .nth(1)
        .unwrap()
        .split("fn search_match_color")
        .next()
        .unwrap();
    assert!(renderer.contains("&'a [CompletionItem]"));
    for source in [renderer, adapter] {
        for owned in [".clone()", ".cloned()", ".to_vec()", ".to_owned()"] {
            assert!(
                !source.contains(owned),
                "popup payload handoff copies via {owned}"
            );
        }
    }
}

#[test]
#[ignore = "optimized local payload-allocation probe, not a UI timing assertion"]
fn completion_payload_cost_probe() {
    let items = fixture();
    println!("case,sample,iterations,total_ns,allocated_bytes");
    for borrowed in [false, true] {
        let operation = || {
            if borrowed {
                black_box(items.as_slice());
            } else {
                black_box(items.clone());
            }
        };
        for _ in 0..5 {
            operation();
        }
        for sample in 0..7 {
            let start = Instant::now();
            let (_, bytes) = allocated(|| {
                for _ in 0..1000 {
                    operation();
                }
            });
            println!(
                "{},{sample},1000,{},{}",
                if borrowed { "borrowed" } else { "cloned" },
                start.elapsed().as_nanos(),
                bytes
            );
        }
    }
}
