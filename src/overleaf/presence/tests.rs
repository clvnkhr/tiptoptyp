use super::*;
use crate::overleaf::{BrowserEvent, Entry, Model, ModelChange};
fn tree() -> Tree {
    Tree::new(
        "Paper".into(),
        None,
        vec![Entry {
            id: "doc".into(),
            path: "main.tex".into(),
            kind: EntryKind::Doc,
        }],
    )
    .unwrap()
}
fn snapshot(source: &str) -> PresenceSnapshot {
    PresenceSnapshot {
        own_id: Some("own".into()),
        self_user_id: Some("account".into()),
        peers: vec![
            PresencePeer {
                id: "own".into(),
                user_id: Some("account".into()),
                name: "Me".into(),
                doc_id: Some("doc".into()),
                row: Some(0),
                column: Some(1),
            },
            PresencePeer {
                id: "other".into(),
                user_id: Some("account".into()),
                name: "Other window".into(),
                doc_id: Some("doc".into()),
                row: Some(0),
                column: Some(1),
            },
        ],
        documents: vec![PresenceDocument {
            id: "doc".into(),
            source_fingerprint: SourceIndex::new(source).fingerprint,
            source_utf16: source.encode_utf16().count(),
            cursors: vec![
                PresenceCursor {
                    id: "own".into(),
                    anchor: 1,
                    head: 1,
                },
                PresenceCursor {
                    id: "other".into(),
                    anchor: 1,
                    head: 1,
                },
            ],
        }],
    }
}
#[test]
fn own_session_is_excluded_but_another_client_of_same_account_is_retained() {
    let mut presence = Presence::default();
    assert!(presence.replace(snapshot("hello"), Some(&tree())));
    let peers = presence.collaborators();
    assert_eq!(peers.len(), 1);
    assert_eq!(peers[0].id, "other");
    assert_eq!(peers[0].color, [38, 157, 217]);
    let cursors = presence.cursors("doc", &SourceIndex::new("hello"), "hello");
    assert_eq!(cursors.len(), 1);
    assert_eq!(cursors[0].id, "other");
}
#[test]
fn site_colors_match_anonymous_same_account_and_md5_hue_rules() {
    assert_eq!(site_color(None, Some("account")), [98, 217, 38]);
    assert_eq!(site_color(Some("account"), Some("account")), [38, 157, 217]);
    // MD5('hello') starts 5d41402a => hue 234, HSL(234,70%,50%).
    assert_eq!(site_color(Some("hello"), None), [38, 56, 217]);
    // MD5('0') selects 196: the site moves its own-blue band by 140 degrees.
    assert_eq!(site_color(Some("0"), None), [217, 38, 110]);
    assert_ne!(
        site_color(Some("different"), None),
        site_color(Some("hello"), None)
    );
}
#[test]
fn replacement_departure_and_identical_updates_reuse_shared_allocations() {
    let mut presence = Presence::default();
    let snapshot = snapshot("hello");
    assert!(presence.replace(snapshot.clone(), Some(&tree())));
    let before = presence.collaborators();
    let generation = presence.generation();
    assert!(!presence.replace(snapshot, Some(&tree())));
    assert_eq!(presence.generation(), generation);
    assert!(Arc::ptr_eq(&before, &presence.collaborators()));
    presence.clear();
    assert!(presence.collaborators().is_empty());
    assert!(
        presence
            .cursors("doc", &SourceIndex::new("hello"), "hello")
            .is_empty()
    );
}
#[test]
fn source_fingerprint_gate_suppresses_stale_and_cross_document_positions() {
    let mut presence = Presence::default();
    presence.replace(snapshot("hello"), Some(&tree()));
    assert!(
        presence
            .cursors("doc", &SourceIndex::new("jello"), "jello")
            .is_empty()
    );
    assert!(
        presence
            .cursors("another", &SourceIndex::new("hello"), "hello")
            .is_empty()
    );
}
#[test]
fn utf16_conversion_rejects_split_surrogates_and_handles_crlf_multiline_endpoints() {
    let source = "a😀\r\nβ";
    let index = SourceIndex::new(source);
    for (position, scalar) in [
        (0, Some(0)),
        (1, Some(1)),
        (2, None),
        (3, Some(2)),
        (4, Some(3)),
        (5, Some(4)),
        (6, Some(5)),
        (7, None),
    ] {
        assert_eq!(
            index.scalar_at_utf16(source, position),
            scalar,
            "offset {position}"
        );
    }
    assert_eq!(index.utf16_at_position(0, 3), Some(3));
    assert_eq!(index.utf16_at_position(0, 4), None);
    assert_eq!(index.utf16_at_position(0, 5), None);
    assert_eq!(index.utf16_at_position(1, 1), Some(6));
    assert_eq!(index.utf16_at_position(2, 0), None);
    let mut snapshot = snapshot(source);
    snapshot.documents[0].cursors[1].head = 2;
    let mut presence = Presence::default();
    presence.replace(snapshot, Some(&tree()));
    assert!(presence.cursors("doc", &index, source).is_empty());
}
#[test]
fn hostile_presence_is_bounded_and_cannot_overwrite_a_good_snapshot() {
    let mut presence = Presence::default();
    presence.replace(snapshot("hello"), Some(&tree()));
    let generation = presence.generation();
    let mut invalid = snapshot("hello");
    invalid.peers = vec![invalid.peers[0].clone(); MAX_PEERS + 1];
    assert!(!presence.replace(invalid, Some(&tree())));
    let mut invalid = snapshot("hello");
    invalid.documents[0].cursors[0].head = 100;
    assert!(!presence.replace(invalid, Some(&tree())));
    let mut invalid = snapshot("hello");
    invalid.documents[0].id = "missing".into();
    assert!(!presence.replace(invalid, Some(&tree())));
    assert_eq!(presence.generation(), generation);
    assert_eq!(presence.collaborators().len(), 1);
}
#[test]
fn tree_deletion_clears_positions_without_losing_online_badges() {
    let mut presence = Presence::default();
    presence.replace(snapshot("hello"), Some(&tree()));
    presence.retain_tree(&Tree::new("Paper".into(), None, vec![]).unwrap());
    assert_eq!(presence.collaborators().len(), 1);
    assert_eq!(presence.collaborators()[0].doc_id, None);
    assert!(
        presence
            .cursors("doc", &SourceIndex::new("hello"), "hello")
            .is_empty()
    );
}
#[test]
fn model_epoch_and_disconnect_invalidate_presence_without_affecting_documents() {
    let mut model = Model::new(7);
    model
        .apply(BrowserEvent::Project {
            epoch: 7,
            project_id: "0123456789abcdef01234567".into(),
            name: "Paper".into(),
            entries: tree().entries,
            root_doc: None,
        })
        .unwrap();
    assert_eq!(
        model
            .apply(BrowserEvent::Presence {
                epoch: 6,
                snapshot: snapshot("hello")
            })
            .unwrap(),
        ModelChange::Ignored
    );
    assert!(model.presence().collaborators().is_empty());
    assert_eq!(
        model
            .apply(BrowserEvent::Presence {
                epoch: 7,
                snapshot: snapshot("hello")
            })
            .unwrap(),
        ModelChange::Presence
    );
    model.reconnect(8);
    assert!(model.presence().collaborators().is_empty());
    model
        .apply(BrowserEvent::Presence {
            epoch: 8,
            snapshot: snapshot("hello"),
        })
        .unwrap();
    model
        .apply(BrowserEvent::Status {
            epoch: 8,
            state: "disconnected".into(),
            message: None,
        })
        .unwrap();
    assert!(model.presence().collaborators().is_empty());
}
