use super::*;
const ID: &str = "0123456789abcdef01234567";
fn entry(id: &str, path: &str, kind: EntryKind) -> Entry {
    Entry {
        id: id.into(),
        path: path.into(),
        kind,
    }
}
fn project(epoch: u64) -> BrowserEvent {
    BrowserEvent::Project {
        epoch,
        project_id: ID.into(),
        name: "Paper".into(),
        entries: vec![
            entry("a", "main.tex", EntryKind::Doc),
            entry("b", "chapter.tex", EntryKind::Doc),
            entry("asset", "images/a.png", EntryKind::File),
        ],
        root_doc: Some("a".into()),
    }
}
fn snapshot(epoch: u64, id: &str, source: &str) -> BrowserEvent {
    BrowserEvent::Document {
        epoch,
        id: id.into(),
        path: if id == "a" { "main.tex" } else { "chapter.tex" }.into(),
        source: source.into(),
    }
}
fn model() -> Model {
    let mut model = Model::new(7);
    assert_eq!(model.apply(project(7)).unwrap(), ModelChange::Project);
    assert!(matches!(
        model.apply(snapshot(7, "a", "old")).unwrap(),
        ModelChange::Document { .. }
    ));
    model
}
fn edit(model: &mut Model, source: &str) -> u64 {
    model.set_draft("a", source.into()).unwrap();
    let BrowserCommand::Edit { request, .. } = model.publish("a").unwrap() else {
        panic!("expected edit")
    };
    request
}
#[test]
fn project_url_is_exact_and_separates_self_hosted_origins() {
    let url = ProjectUrl::parse(&format!(" https://www.overleaf.com/project/{ID}/ ")).unwrap();
    assert_eq!(url.url(), format!("https://www.overleaf.com/project/{ID}"));
    assert_eq!(url.origin(), "https://www.overleaf.com");
    assert_eq!(url.id(), ID);
    let local = ProjectUrl::parse(&format!("http://localhost:4321/project/{ID}")).unwrap();
    assert_ne!(
        url.mirror_root(Path::new("/cache")),
        local.mirror_root(Path::new("/cache"))
    );
    assert!(ProjectUrl::parse(&format!("https://latex.example.org/project/{ID}")).is_ok());
    assert!(ProjectUrl::parse(&format!("http://[::1]:4321/project/{ID}")).is_ok());
    for invalid in [
        format!("http://overleaf.com/project/{ID}"),
        format!("https://user:secret@overleaf.com/project/{ID}"),
        format!("https://overleaf.com/project/{ID}?token=secret"),
        format!("https://overleaf.com/project/{ID}#x"),
        format!("https://overleaf.com/project/{ID}/edit"),
        "https://overleaf.com/project/no".into(),
        format!("file:///project/{ID}"),
        format!("https://overleaf.com/read/{ID}"),
    ] {
        assert!(ProjectUrl::parse(&invalid).is_err(), "{invalid}");
    }
}
#[test]
fn tree_rejects_escaping_paths_and_unrepresentable_collisions() {
    for path in [
        "",
        "/main.tex",
        "../main.tex",
        "a/../main.tex",
        "a/./b",
        "a//b",
        "a/",
        "C:/main.tex",
        "a\\b",
        ".git/config",
        "a/.GIT/config",
        ".tiptoptyp/cache",
        "a\0b",
        "a\nb",
    ] {
        assert!(validate_path(path).is_err(), "{path:?}");
    }
    for entries in [
        vec![
            entry("a", "main.tex", EntryKind::Doc),
            entry("b", "MAIN.tex", EntryKind::Doc),
        ],
        vec![
            entry("a", "caf\u{e9}.tex", EntryKind::Doc),
            entry("b", "cafe\u{301}.tex", EntryKind::Doc),
        ],
        vec![
            entry("a", "caf\u{e9}", EntryKind::Doc),
            entry("b", "cafe\u{301}/main.tex", EntryKind::Doc),
        ],
        vec![
            entry("a", "one", EntryKind::Doc),
            entry("a", "two", EntryKind::Doc),
        ],
        vec![
            entry("a", "one", EntryKind::Doc),
            entry("b", "one/two", EntryKind::Doc),
        ],
    ] {
        assert!(Tree::new("x".into(), None, entries).is_err());
    }
    assert!(Tree::new("x".into(), Some("missing".into()), vec![]).is_err());
    let entries = vec![
        entry("f", "images", EntryKind::Folder),
        entry("a", "images/図.png", EntryKind::File),
    ];
    let tree = Tree::new("x".into(), None, entries).unwrap();
    assert_eq!(tree.id_for_path("images/図.png"), Some("a"));
    assert_eq!(tree.entry("a").unwrap().path, "images/図.png");
    assert_eq!(tree.entries().len(), 2);
}
#[test]
fn wire_contract_is_tagged_and_round_trips_without_losing_source() {
    let command = BrowserCommand::Edit {
        epoch: 7,
        id: "a".into(),
        base: "α\n".into(),
        source: "β\r\n".into(),
        request: 9,
    };
    let json = serde_json::to_value(&command).unwrap();
    assert_eq!(json["command"], "edit");
    assert_eq!(json["source"], "β\r\n");
    assert_eq!(
        serde_json::from_value::<BrowserCommand>(json).unwrap(),
        command
    );
    let event: BrowserEvent = serde_json::from_str(r#"{"event":"project","epoch":7,"project_id":"0123456789abcdef01234567","name":"Paper","entries":[{"id":"a","path":"main.tex","kind":"doc"}]}"#).unwrap();
    assert_eq!(event.epoch(), 7);
    assert!(serde_json::from_str::<BrowserEvent>(r#"{"event":"project","epoch":7,"project_id":"x","name":"Paper","entries":[{"id":"a","path":"main.tex","kind":"symlink"}]}"#).is_err());
}
#[test]
fn serialized_publish_keeps_old_receipt_and_coalesces_newer_drafts() {
    let mut state = model();
    let request = edit(&mut state, "first");
    assert_eq!(state.in_flight().unwrap().base, "old");
    assert_eq!(state.document("a").unwrap().baseline, "old");
    state.set_draft("a", "second".into()).unwrap();
    state.set_draft("a", "third".into()).unwrap();
    assert!(state.publish("a").is_none());
    assert_eq!(state.in_flight().unwrap().source, "first");
    assert_eq!(
        state
            .apply(BrowserEvent::Ack {
                epoch: 7,
                id: "a".into(),
                request,
                source: "first".into()
            })
            .unwrap(),
        ModelChange::Acknowledged {
            id: "a".into(),
            request,
            source: "first".into(),
            dirty: true
        }
    );
    assert_eq!(state.document("a").unwrap().source(), "third");
    let BrowserCommand::Edit {
        base,
        source,
        request: next,
        ..
    } = state.publish("a").unwrap()
    else {
        panic!()
    };
    assert_eq!((base.as_str(), source.as_str()), ("first", "third"));
    state
        .apply(BrowserEvent::Ack {
            epoch: 7,
            id: "a".into(),
            request: next,
            source,
        })
        .unwrap();
    assert!(!state.has_unsynced());
    assert!(state.in_flight().is_none());
}
#[test]
fn undo_to_old_baseline_is_retained_while_publish_is_in_flight() {
    let mut state = model();
    let request = edit(&mut state, "first");
    state.set_draft("a", "old".into()).unwrap();
    state
        .apply(BrowserEvent::Ack {
            epoch: 7,
            id: "a".into(),
            request,
            source: "first".into(),
        })
        .unwrap();
    assert!(
        matches!(state.publish("a"), Some(BrowserCommand::Edit { base, source, .. }) if base == "first" && source == "old")
    );
}
#[test]
fn local_mirror_and_optimistic_browser_snapshot_do_not_acknowledge_a_save() {
    let mut state = model();
    edit(&mut state, "new");
    assert_eq!(
        state.apply(snapshot(7, "a", "new")).unwrap(),
        ModelChange::Ignored
    );
    assert_eq!(state.document("a").unwrap().baseline, "old");
    assert!(state.has_unsynced());
    assert!(state.in_flight().is_some());
}
#[test]
fn mismatched_request_owner_and_epoch_cannot_complete_a_publish() {
    let mut state = model();
    let request = edit(&mut state, "new");
    for (epoch, id, request) in [(6, "a", request), (7, "b", request), (7, "a", request + 1)] {
        assert_eq!(
            state
                .apply(BrowserEvent::Ack {
                    epoch,
                    id: id.into(),
                    request,
                    source: "new".into()
                })
                .unwrap(),
            ModelChange::Ignored
        );
        assert_eq!(state.document("a").unwrap().baseline, "old");
    }
    state.reconnect(8);
    assert_eq!(state.epoch(), 8);
    assert!(state.in_flight().is_none());
    assert_eq!(state.document("a").unwrap().source(), "new");
    assert_eq!(
        state
            .apply(BrowserEvent::Ack {
                epoch: 7,
                id: "a".into(),
                request,
                source: "new".into()
            })
            .unwrap(),
        ModelChange::Ignored
    );
}
#[test]
fn concurrent_remote_changes_preserve_draft_and_stop_automatic_overwrite() {
    let mut state = model();
    state.set_draft("a", "local".into()).unwrap();
    assert!(matches!(
        state.apply(snapshot(7, "a", "remote")).unwrap(),
        ModelChange::Document { conflict: true, .. }
    ));
    let document = state.document("a").unwrap();
    assert_eq!(document.baseline, "remote");
    assert_eq!(document.source(), "local");
    assert_eq!(document.conflict.as_ref().unwrap().remote, "remote");
    assert!(state.publish("a").is_none());
    assert!(state.resolve_server("a"));
    assert_eq!(state.document("a").unwrap().source(), "remote");
    assert!(!state.has_unsynced());
}
#[test]
fn clean_remote_changes_replace_baseline_but_wrong_identity_is_rejected() {
    let mut state = model();
    state.apply(snapshot(7, "a", "remote")).unwrap();
    assert_eq!(state.document("a").unwrap().source(), "remote");
    let mut wrong = snapshot(7, "a", "bad");
    if let BrowserEvent::Document { path, .. } = &mut wrong {
        *path = "chapter.tex".into();
    }
    assert!(state.apply(wrong).is_err());
    assert_eq!(state.document("a").unwrap().source(), "remote");
    let url = ProjectUrl::parse(&format!("https://overleaf.com/project/{ID}")).unwrap();
    let mut state = Model::for_project(7, &url);
    let mut wrong = project(7);
    if let BrowserEvent::Project { project_id, .. } = &mut wrong {
        *project_id = "aaaaaaaaaaaaaaaaaaaaaaaa".into();
    }
    assert!(state.apply(wrong).is_err());
    assert!(state.tree().is_none());
}
#[test]
fn conflicts_and_unexpected_ack_bytes_never_discard_newer_drafts() {
    for conflict in [true, false] {
        let mut state = model();
        let request = edit(&mut state, "first");
        state.set_draft("a", "newer local".into()).unwrap();
        let event = if conflict {
            BrowserEvent::Conflict {
                epoch: 7,
                id: "a".into(),
                request,
                source: "server".into(),
                message: "changed".into(),
            }
        } else {
            BrowserEvent::Ack {
                epoch: 7,
                id: "a".into(),
                request,
                source: "server".into(),
            }
        };
        assert_eq!(
            state.apply(event).unwrap(),
            ModelChange::Conflict { id: "a".into() }
        );
        let document = state.document("a").unwrap();
        assert_eq!(document.baseline, "server");
        assert_eq!(document.source(), "newer local");
        assert!(document.conflict.is_some());
        assert!(state.publish("a").is_none());
    }
}
#[test]
fn deleted_document_draft_is_retained_without_recreating_remote_file() {
    let mut state = model();
    state.set_draft("a", "local".into()).unwrap();
    state
        .apply(BrowserEvent::Project {
            epoch: 7,
            project_id: ID.into(),
            name: "Paper".into(),
            entries: vec![],
            root_doc: None,
        })
        .unwrap();
    assert_eq!(state.document("a").unwrap().source(), "local");
    assert!(state.document("a").unwrap().conflict.is_some());
    assert!(state.publish("a").is_none());
}
#[test]
fn browser_payload_caps_and_asset_identity_are_checked() {
    let mut state = model();
    assert!(
        state
            .set_draft("a", "x".repeat(MAX_SOURCE_BYTES + 1))
            .is_err()
    );
    assert!(
        state
            .apply(BrowserEvent::File {
                epoch: 7,
                id: "asset".into(),
                path: "main.tex".into(),
                base64: "AA==".into()
            })
            .is_err()
    );
    assert!(
        state
            .apply(BrowserEvent::Document {
                epoch: 7,
                id: "a".into(),
                path: "main.tex".into(),
                source: "x".repeat(MAX_SOURCE_BYTES + 1)
            })
            .is_err()
    );
    let entries = (0..10_001)
        .map(|i| entry(&i.to_string(), &format!("{i}.tex"), EntryKind::Doc))
        .collect();
    assert!(Tree::new("large".into(), None, entries).is_err());
}
#[cfg(unix)]
#[test]
fn mirror_keeps_assets_exact_and_server_manifest_retires_deleted_files() {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path().canonicalize().unwrap().join("mirror");
    let mirror = Mirror::open(&root).unwrap();
    assert_eq!(mirror.root(), root);
    let doc = entry("doc", "sub/main.tex", EntryKind::Doc);
    let asset = entry("asset", "images/a.png", EntryKind::File);
    let tree = Tree::new("Paper".into(), None, vec![doc.clone(), asset.clone()]).unwrap();
    mirror.apply_tree(&tree).unwrap();
    mirror
        .write_document(&doc, "α\r\n\\begin{document}\n")
        .unwrap();
    mirror.write_file(&asset, "AAECA/8=").unwrap();
    assert_eq!(
        std::fs::read(root.join(&doc.path)).unwrap(),
        "α\r\n\\begin{document}\n".as_bytes()
    );
    assert_eq!(
        std::fs::read(root.join(&asset.path)).unwrap(),
        [0, 1, 2, 3, 255]
    );
    std::fs::write(root.join("generated.pdf"), b"build output").unwrap();
    let reopened = Mirror::open(&root).unwrap();
    reopened
        .apply_tree(&Tree::new("Paper".into(), None, vec![doc]).unwrap())
        .unwrap();
    assert!(!root.join(&asset.path).exists());
    assert!(root.join("generated.pdf").exists());
}
#[cfg(unix)]
#[test]
fn mirror_rejects_symlinks_in_roots_parents_targets_and_metadata() {
    use std::os::unix::fs::symlink;
    let temporary = tempfile::tempdir().unwrap();
    let base = temporary.path().canonicalize().unwrap();
    let outside = base.join("outside");
    std::fs::create_dir(&outside).unwrap();
    symlink(&outside, base.join("alias")).unwrap();
    assert!(Mirror::open(&base.join("alias/project")).is_err());
    let root = base.join("mirror");
    let mirror = Mirror::open(&root).unwrap();
    symlink(&outside, root.join("escape")).unwrap();
    assert!(
        mirror
            .write_document(&entry("a", "escape/file.tex", EntryKind::Doc), "bad")
            .is_err()
    );
    std::fs::write(outside.join("original.tex"), b"preserve").unwrap();
    symlink(outside.join("original.tex"), root.join("main.tex")).unwrap();
    assert!(
        mirror
            .write_document(&entry("a", "main.tex", EntryKind::Doc), "bad")
            .is_err()
    );
    symlink(&outside, root.join(".tiptoptyp")).unwrap();
    assert!(
        mirror
            .apply_tree(&Tree::new("x".into(), None, vec![]).unwrap())
            .is_err()
    );
    assert_eq!(
        std::fs::read(outside.join("original.tex")).unwrap(),
        b"preserve"
    );
    assert!(!outside.join("file.tex").exists());
}
#[cfg(unix)]
#[test]
fn malformed_asset_data_never_touches_existing_mirror_file() {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path().canonicalize().unwrap();
    let mirror = Mirror::open(&root).unwrap();
    let asset = entry("a", "a.bin", EntryKind::File);
    mirror.write_file(&asset, "AA==").unwrap();
    for invalid in [
        "A", "====", "AA=A", "AB==", "AAB=", "AA==AAAA", "AA?=", "AA==\n",
    ] {
        assert!(mirror.write_file(&asset, invalid).is_err(), "{invalid}");
        assert_eq!(std::fs::read(root.join("a.bin")).unwrap(), [0]);
    }
}

#[test]
fn publishing_requested_document_cannot_consume_another_tabs_save_request() {
    let mut state = model();
    state.apply(snapshot(7, "b", "chapter old")).unwrap();
    state.set_draft("a", "main draft".into()).unwrap();
    state.set_draft("b", "chapter draft".into()).unwrap();
    let Some(BrowserCommand::Edit {
        id, base, source, ..
    }) = state.publish("b")
    else {
        panic!("expected requested chapter publish");
    };
    assert_eq!(
        (id.as_str(), base.as_str(), source.as_str()),
        ("b", "chapter old", "chapter draft")
    );
    assert!(state.publish("a").is_none());
    assert_eq!(state.document("a").unwrap().source(), "main draft");
}
#[test]
fn remote_changes_cannot_erase_undo_to_baseline_while_a_publish_is_in_flight() {
    let mut state = model();
    edit(&mut state, "first");
    state.set_draft("a", "old".into()).unwrap();
    assert!(matches!(
        state.apply(snapshot(7, "a", "server")).unwrap(),
        ModelChange::Document { conflict: true, .. }
    ));
    assert_eq!(state.document("a").unwrap().source(), "old");
    assert_eq!(state.document("a").unwrap().baseline, "server");
}

#[test]
fn explicit_unchanged_save_gets_a_fresh_receipt_without_background_noop_uploads() {
    let mut state = model();
    assert!(state.in_flight().is_none());
    assert!(!state.has_unsynced());
    let Some(BrowserCommand::Edit {
        id,
        base,
        source,
        request,
        epoch,
    }) = state.publish("a")
    else {
        panic!("explicit unchanged save must reach the browser acknowledgement boundary");
    };
    assert_eq!(
        (id.as_str(), base.as_str(), source.as_str(), epoch),
        ("a", "old", "old", 7)
    );
    assert!(state.in_flight().is_some());
    assert!(state.has_unsynced());
    assert!(state.publish("a").is_none());
    assert_eq!(
        state
            .apply(BrowserEvent::Ack {
                epoch,
                id: id.clone(),
                request,
                source: source.clone()
            })
            .unwrap(),
        ModelChange::Acknowledged {
            id,
            request,
            source,
            dirty: false
        }
    );
    assert!(!state.has_unsynced());
    assert!(state.in_flight().is_none());
}

#[test]
fn releasing_last_native_draft_removes_phantom_conflicts_on_reopen() {
    let mut state = model();
    state
        .set_draft("a", "discarded local draft".into())
        .unwrap();
    state.apply(snapshot(7, "a", "server revision")).unwrap();
    assert!(state.document("a").unwrap().conflict.is_some());
    assert!(state.release_native_draft("a"));
    assert!(!state.has_unsynced());
    let document = state.document("a").unwrap();
    assert_eq!(document.baseline, "server revision");
    assert!(document.draft.is_none() && document.conflict.is_none());
    // Reopening uses current Overleaf contents, rather than resurrecting the
    // draft the user closed/discarded or exported to a separate local file.
    assert!(matches!(
        state
            .apply(snapshot(7, "a", "server after reopening"))
            .unwrap(),
        ModelChange::Document {
            conflict: false,
            ..
        }
    ));
    assert_eq!(
        state.document("a").unwrap().source(),
        "server after reopening"
    );
    assert!(!state.has_unsynced());
}

#[test]
fn last_native_binding_cannot_release_a_document_still_being_published() {
    let mut state = model();
    let request = edit(&mut state, "publishing draft");
    assert!(!state.release_native_draft("a"));
    assert_eq!(state.in_flight().unwrap().request, request);
    assert_eq!(state.document("a").unwrap().source(), "publishing draft");
    state
        .apply(BrowserEvent::Ack {
            epoch: 7,
            id: "a".into(),
            request,
            source: "publishing draft".into(),
        })
        .unwrap();
    assert!(state.release_native_draft("a"));
    assert_eq!(state.document("a").unwrap().baseline, "publishing draft");
}

#[test]
fn releasing_a_deleted_native_draft_drops_its_retained_contents() {
    let mut state = model();
    state
        .set_draft("a", "retain until explicit discard".into())
        .unwrap();
    state
        .apply(BrowserEvent::Project {
            epoch: 7,
            project_id: ID.into(),
            name: "Paper".into(),
            entries: Vec::new(),
            root_doc: None,
        })
        .unwrap();
    assert!(state.document("a").is_some());
    assert!(state.release_native_draft("a"));
    assert!(state.document("a").is_none());
    assert!(!state.has_unsynced());
    assert!(state.release_native_draft("a"));
}

#[test]
fn repeated_server_deletions_retire_clean_downloads_without_unbounded_history() {
    let mut state = Model::new(7);
    let mut previous: Vec<String> = Vec::new();
    for index in 0..64 {
        let id = format!("doc{index}");
        let path = format!("part{index}.tex");
        state
            .apply(BrowserEvent::Project {
                epoch: 7,
                project_id: ID.into(),
                name: "Paper".into(),
                entries: vec![entry(&id, &path, EntryKind::Doc)],
                root_doc: Some(id.clone()),
            })
            .unwrap();
        for removed in &previous {
            assert!(state.document(removed).is_none());
        }
        state
            .apply(BrowserEvent::Document {
                epoch: 7,
                id: id.clone(),
                path,
                source: "clean server contents".into(),
            })
            .unwrap();
        previous.push(id);
    }
    state
        .apply(BrowserEvent::Project {
            epoch: 7,
            project_id: ID.into(),
            name: "Empty".into(),
            entries: Vec::new(),
            root_doc: None,
        })
        .unwrap();
    for removed in previous {
        assert!(state.document(&removed).is_none());
    }
    assert!(!state.has_unsynced());
}

#[test]
fn deletion_keeps_even_a_clean_snapshot_while_its_explicit_receipt_is_pending() {
    let mut state = model();
    let Some(BrowserCommand::Edit { request, .. }) = state.publish("a") else {
        panic!("expected explicit no-op request");
    };
    state
        .apply(BrowserEvent::Project {
            epoch: 7,
            project_id: ID.into(),
            name: "Empty".into(),
            entries: Vec::new(),
            root_doc: None,
        })
        .unwrap();
    assert!(state.document("a").is_some());
    assert!(!state.release_native_draft("a"));
    state
        .apply(BrowserEvent::Ack {
            epoch: 7,
            id: "a".into(),
            request,
            source: "old".into(),
        })
        .unwrap();
    assert!(state.release_native_draft("a"));
    assert!(state.document("a").is_none());
}
