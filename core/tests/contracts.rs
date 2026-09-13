use std::{path::PathBuf, sync::Arc, time::Duration};

use tiptoptyp_core::{
    closing::CloseCoordinator,
    connection::Connection,
    document::{DocumentKey, DocumentKind, DocumentSession, WindowSessionId},
    geometry::{EguiRect, ViewportTransform},
    preview::{ArtifactKey, PreviewContent},
    scheduling::Debounce,
    text::{
        LineIndex, LspPosition, LspRange, LspTextEdit, ScalarColumn, ScalarOffset,
        apply_text_edits, range_to_scalar_range, scalar_position_at,
    },
    workflow::Workflow,
};

fn document(owner: u64) -> DocumentSession<usize> {
    DocumentSession::new(WindowSessionId::new(owner), "saved", DocumentKind::Text)
}

fn edit(line: u32, start: u32, end: u32, new_text: &str) -> LspTextEdit {
    LspTextEdit {
        range: LspRange {
            start: LspPosition::new(line, start),
            end: LspPosition::new(line, end),
        },
        new_text: new_text.to_owned(),
    }
}

#[test]
fn document_notifications_are_coalesced_and_history_is_replayable() {
    let mut document = document(7);
    let owner = document.key().owner;
    assert_eq!(document.key().owner, owner);
    assert!(document.take_edit().is_none());

    document.edit(0, |source| source.push_str(" one"));
    document.edit(4, |source| source.push_str(" two"));
    let changed = document
        .take_edit()
        .expect("two edits share one notification");
    assert_eq!(changed.source(), "saved one two");
    assert_eq!(changed.key(), document.key());
    assert!(document.take_edit().is_none());
    assert!(document.is_dirty());
    assert_eq!(document.history_availability(), (true, false));

    let undone = document.history_step(false, 4).expect("undo snapshot");
    assert_eq!(undone.source.as_ref(), "saved one");
    assert_eq!(document.source(), "saved one");
    assert_eq!(document.take_edit().unwrap().source(), "saved one");
    assert_eq!(document.history_availability(), (true, true));

    let redone = document.history_step(true, 8).expect("redo snapshot");
    assert_eq!(redone.source.as_ref(), "saved one two");
    assert_eq!(document.source(), "saved one two");
    assert_eq!(document.take_edit().unwrap().source(), "saved one two");
    assert!(document.take_edit().is_none());
}

#[test]
fn workflow_rejects_duplicate_exclusive_events_and_clears_abandoned_continuations() {
    let mut workflow = Workflow::<&str, &str, &str>::default();
    workflow.queue("close").unwrap();
    assert_eq!(workflow.queue("duplicate"), Err("duplicate"));
    assert_eq!(workflow.take_action(), Some("close"));

    workflow.choose("save destination").unwrap();
    assert_eq!(
        workflow.choose("second destination"),
        Err("second destination")
    );
    assert_eq!(workflow.take_dialog(), Some("save destination"));
    workflow.continue_after_save("close").unwrap();
    assert_eq!(workflow.continue_after_save("again"), Err("again"));

    workflow.finish_dispatch();
    assert!(!workflow.is_busy());
    assert!(!workflow.has_continuation());
    assert_eq!(workflow.take_action(), None);
}

#[test]
fn close_batch_requires_current_versions_and_quiescent_operations() {
    let first = DocumentKey::new(WindowSessionId::new(1), 2, 3);
    let second = DocumentKey::new(WindowSessionId::new(2), 4, 5);
    let approved_first = DocumentKey::new(first.owner, first.epoch, 4);
    let mut close = CloseCoordinator::default();

    assert!(close.begin(vec![first, second]));
    assert!(!close.begin(vec![first]));
    assert!(!close.accept(first, second));
    assert!(close.accept(first, approved_first));
    assert!(close.accept(second, second));
    assert!(close.ready(&[approved_first, second]));
    assert!(!close.can_exit(&[approved_first, second], true));
    assert!(close.can_exit(&[approved_first, second], false));
    assert!(!close.ready(&[first, second]));

    close.cancel();
    assert!(!close.is_active());
    assert!(close.begin(vec![first]));
    assert_eq!(close.current(), Some(first));
}

#[test]
fn preview_provenance_keeps_displayed_pixels_but_rejects_stale_results() {
    let mut preview = PreviewContent::<u32>::default();
    let first = ArtifactKey {
        revision: 8,
        generation: 1,
    };
    let second = ArtifactKey {
        generation: 2,
        ..first
    };
    preview.accept_artifact(first, Arc::from(&b"first pdf"[..]));
    assert!(preview.accept_raster(first, vec![1, 2]));
    preview.accept_artifact(second, Arc::from(&b"second pdf"[..]));
    assert_eq!(preview.pages(), &[1, 2]);
    assert_eq!(preview.raster_key(), Some(first));
    assert!(!preview.accept_raster(first, vec![99]));
    assert!(preview.fail_raster(second, "decode failed".to_owned()));
    assert_eq!(preview.error(), Some("decode failed"));
    assert_eq!(preview.pdf().unwrap().as_ref(), b"second pdf");
    assert_eq!(preview.pages(), &[1, 2]);
    assert!(preview.accept_raster(second, vec![3]));
    assert_eq!(preview.error(), None);
    assert_eq!(preview.pages(), &[3]);

    preview.invalidate();
    assert!(preview.pdf().is_some());
    assert_eq!(preview.artifact_key(), None);
    preview.clear();
    assert!(preview.pdf().is_none());
    assert!(preview.pages().is_empty());
}

#[test]
fn connection_requires_a_matching_ready_generation_for_every_endpoint() {
    let mut connection = Connection::<u64, &'static str>::default();
    connection.start(10);
    assert!(!connection.connect(10, "stale"));
    assert!(!connection.initialized(9));
    assert!(connection.initialized(10));
    assert!(connection.connect(10, "http://preview"));
    connection.suspend(true);
    assert_eq!(connection.endpoint(), Some(&"http://preview"));
    assert!(!connection.is_ready());
    connection.start(11);
    assert!(!connection.connect(10, "obsolete"));
    assert!(connection.initialized(11));
    assert!(connection.connect(11, "http://preview"));
    connection.suspend(false);
    assert_eq!(connection.endpoint(), None);
}

#[test]
fn text_boundary_contract_handles_empty_final_lines_and_rejects_invalid_edits() {
    let source = "😀\n";
    let applied = apply_text_edits(
        source,
        &[edit(1, 0, 0, "tail")],
        [ScalarOffset::new(0), ScalarOffset::new(2)],
    )
    .unwrap();
    assert_eq!(applied.text, "😀\ntail");
    assert_eq!(applied.mapped_offsets.map(ScalarOffset::get), [0, 6]);

    let split_surrogate = apply_text_edits(
        "😀",
        &[edit(0, 1, 1, "x")],
        [ScalarOffset::new(0), ScalarOffset::new(1)],
    );
    assert!(split_surrogate.is_err());
    assert!(
        apply_text_edits(
            source,
            &[edit(2, 0, 0, "x")],
            [ScalarOffset::new(0), ScalarOffset::new(0)],
        )
        .is_err()
    );

    let forgiving = range_to_scalar_range(
        source,
        &LspRange {
            start: LspPosition::new(9, 0),
            end: LspPosition::new(0, 0),
        },
    );
    assert_eq!(forgiving.start(), forgiving.end());
    assert_eq!(
        scalar_position_at(source, ScalarOffset::new(usize::MAX)),
        (LineIndex::new(1), ScalarColumn::new(0))
    );
}

#[test]
fn geometry_and_debounce_boundaries_are_total_for_invalid_inputs() {
    assert!(EguiRect::new([0.0, 0.0], [10.0, 10.0]).is_some());
    assert!(EguiRect::new([f32::NAN, 0.0], [10.0, 10.0]).is_none());
    assert!(EguiRect::new([0.0, 10.0], [10.0, 0.0]).is_none());
    assert!(ViewportTransform::new([0.0, 0.0], 0.0).is_none());
    let transform = ViewportTransform::new([10.0, 20.0], 2.0).unwrap();
    let rect = transform
        .to_native(EguiRect::new([10.0, 20.0], [15.0, 25.0]).unwrap())
        .unwrap();
    assert_eq!((rect.left(), rect.top()), (10.0, 20.0));
    assert_eq!((rect.width(), rect.height()), (10.0, 10.0));

    let mut debounce = Debounce::default();
    assert_eq!(debounce.remaining(Duration::from_secs(1)), None);
    debounce.schedule(Duration::from_secs(4));
    assert_eq!(
        debounce.remaining(Duration::from_secs(2)),
        Some(Duration::from_secs(2))
    );
    assert_eq!(
        debounce.remaining(Duration::from_secs(5)),
        Some(Duration::ZERO)
    );
    debounce.clear();
    assert!(!debounce.is_pending());
}

#[test]
fn document_replacement_resets_history_without_changing_window_owner() {
    let mut document = document(42);
    let owner = document.key().owner;
    document.edit(0, |source| source.push_str(" unsaved"));
    assert!(document.history_availability().0);
    let old = document.key();

    document.replace_loaded(
        "loaded".to_owned(),
        PathBuf::from("chapter.typ"),
        DocumentKind::Typst,
        Some(99),
    );

    assert_eq!(document.key().owner, owner);
    assert_eq!(document.key().epoch, old.epoch + 1);
    assert_eq!(document.source(), "loaded");
    assert_eq!(document.saved_source(), "loaded");
    assert_eq!(document.disk_fingerprint(), Some(99));
    assert_eq!(document.history_availability(), (false, false));
    assert!(!document.is_dirty());
}
