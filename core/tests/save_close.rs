//! The real workflow and document boundary, driven by recorded UI/effect events.
//! Writes are captured as bytes: no dialog, process, filesystem or wall clock.
use tiptoptyp_core::{
    document::{DocumentKind, DocumentSession, WindowSessionId},
    workflow::Workflow,
};

fn document(owner: u64) -> DocumentSession<usize> {
    let mut doc = DocumentSession::new(WindowSessionId::new(owner), "saved", DocumentKind::Text);
    doc.edit(0, |source| source.push_str(" + edit"));
    doc
}
fn choosing_save() -> Workflow<&'static str, &'static str, &'static str> {
    let mut flow = Workflow::default();
    flow.queue("request close").unwrap();
    assert_eq!(flow.take_action(), Some("request close"));
    flow.show_prompt("save changes?").unwrap();
    flow.take_prompt();
    flow.continue_after_save("close").unwrap();
    flow.choose("save destination").unwrap();
    flow
}

#[test]
fn save_close_sequences_cover_cancellation_failure_new_edits_and_durability() {
    for scenario in ["cancel", "write failure", "saved", "new edit", "uncertain"] {
        let mut doc = document(1);
        let mut flow = choosing_save();
        assert!(flow.queue("duplicate close").is_err());
        assert_eq!(flow.take_dialog(), Some("save destination"));
        let mut writes = Vec::new();
        if scenario == "cancel" || scenario == "write failure" {
            flow.cancel_continuation();
        } else {
            let write = doc.prepare_save("note.txt".into(), DocumentKind::Text);
            writes.push(write.source().to_owned());
            if scenario == "new edit" {
                doc.edit(0, |source| source.push_str(" newer"));
            }
            let action = flow
                .complete_save(
                    flow.continuation_token(),
                    doc.record_save(write.committed(10)),
                    doc.is_dirty(),
                    scenario != "uncertain",
                )
                .unwrap();
            assert_eq!(action, (scenario == "saved").then_some("close"));
            assert_eq!(doc.saved_source(), &writes[0]);
            assert_eq!(doc.source() != doc.saved_source(), scenario == "new edit");
        }
        assert_eq!(
            writes.is_empty(),
            scenario == "cancel" || scenario == "write failure"
        );
        assert_eq!(flow.take_continuation(), None);
        flow.finish_dispatch();
        assert!(!flow.is_busy());
    }
}

#[test]
fn receipt_from_another_window_or_replaced_document_cannot_release_close() {
    for other_window in [false, true] {
        let mut doc = document(1);
        let mut flow = choosing_save();
        flow.take_dialog();
        let receipt = doc
            .prepare_save("same-path.txt".into(), DocumentKind::Text)
            .committed(10);
        if other_window {
            doc = document(2);
        } else {
            doc.replace_untitled("replacement");
        }
        let before = doc.snapshot();
        assert!(
            flow.complete_save(
                flow.continuation_token(),
                doc.record_save(receipt),
                doc.is_dirty(),
                true
            )
            .is_err()
        );
        assert_eq!(doc.snapshot().key(), before.key());
        assert_eq!(doc.source(), before.source());
        assert_eq!(flow.take_continuation(), None);
    }
}
