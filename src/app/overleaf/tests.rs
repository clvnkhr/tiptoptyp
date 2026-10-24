//! Controller tests keep the real document/save receipt boundary. They do not
//! replace the browser's network acknowledgement with a local disk write.
use super::*;
use egui_kittest::{Harness, kittest::Queryable as _};

const PROJECT: &str = "0123456789abcdef01234567";
const EPOCH: u64 = 7;
fn entry(id: &str, path: &str, kind: EntryKind) -> Entry {
    Entry {
        id: id.into(),
        path: path.into(),
        kind,
    }
}
fn fixture(root: &Path, context: &egui::Context) -> EditorApp {
    let mut app = EditorApp::dormant_for_tests(context, root.into());
    let project =
        ProjectUrl::parse(&format!("https://www.overleaf.com/project/{PROJECT}")).unwrap();
    let mut model = Model::for_project(EPOCH, &project);
    model
        .apply(BrowserEvent::Project {
            epoch: EPOCH,
            project_id: PROJECT.into(),
            name: "Remote paper".into(),
            entries: vec![
                entry("a", "main.tex", EntryKind::Doc),
                entry("b", "chapter.tex", EntryKind::Doc),
            ],
            root_doc: Some("a".into()),
        })
        .unwrap();
    Mirror::open(root)
        .unwrap()
        .apply_tree(model.tree().unwrap())
        .unwrap();
    for (id, path, source) in [
        ("a", "main.tex", "Main old\n"),
        ("b", "chapter.tex", "Chapter old\n"),
    ] {
        fs::write(root.join(path), source).unwrap();
        model
            .apply(BrowserEvent::Document {
                epoch: EPOCH,
                id: id.into(),
                path: path.into(),
                source: source.into(),
            })
            .unwrap();
    }
    app.overleaf.epoch = EPOCH;
    app.overleaf.workspace = Some(WorkspaceTree::from_snapshot(remote_snapshot(
        root,
        model.tree().unwrap(),
    )));
    app.overleaf.connection = Some(Connection {
        project,
        root: root.into(),
        model,
        status: "Connected".into(),
        online: true,
        storage: None,
        hydration: Hydration::default(),
    });
    app.document_mut().replace_loaded_unprojected(
        "Main old\n".into(),
        root.join("main.tex"),
        DocumentKind::Tex,
        Some(fingerprint(b"Main old\n")),
    );
    app
}
fn park_main(app: &mut EditorApp, root: &Path, context: &egui::Context) -> (u64, u64) {
    let main = app.tabs.active_id().unwrap();
    app.lifecycle = DocumentLifecycle::Active;
    app.new_tab(context);
    app.lifecycle = DocumentLifecycle::Dormant;
    app.document_mut().replace_loaded_unprojected(
        "Chapter old\n".into(),
        root.join("chapter.tex"),
        DocumentKind::Tex,
        Some(fingerprint(b"Chapter old\n")),
    );
    let chapter = app.tabs.active_id().unwrap();
    assert_ne!(main, chapter);
    (main, chapter)
}
fn pending_save(app: &mut EditorApp, tab: u64, id: &str, source: &str) -> u64 {
    let document = app.tabs.document_mut(tab).unwrap();
    document.edit(CCursorRange::default(), |text| *text = source.into());
    let key = document.key();
    let path = document.path().clone().unwrap();
    let request = document.prepare_save(path, document.kind()).unwrap();
    let model = &mut app.overleaf.connection.as_mut().unwrap().model;
    model.set_draft(id, request.source().into()).unwrap();
    let Some(BrowserCommand::Edit { request: wire, .. }) = model.publish(id) else {
        panic!("expected edit");
    };
    app.overleaf.pending = Some(RemoteSave {
        id: id.into(),
        wire,
        tab,
        key,
        request,
        continuation: None,
        started: Instant::now(),
    });
    app.document_workflow.save_in_flight = true;
    wire
}
fn apply_work(
    app: &mut EditorApp,
    work: MirrorWork,
    root: &Path,
    epoch: u64,
    context: &egui::Context,
) {
    // This helper dispatches new work now. Capture its generation before any
    // filesystem effects, exactly as the production mirror worker does.
    let hydration_session = app.overleaf.connection.as_ref().unwrap().hydration.session;
    let mirror = Mirror::open(root).unwrap();
    let result = match &work.data {
        MirrorData::Document(source) => mirror.write_document(&work.entry, source),
        MirrorData::File(base64) => mirror.write_file(&work.entry, base64),
    };
    app.apply_overleaf_mirror(
        Mirrored {
            hydration_session,
            epoch,
            tree: None,
            error: None,
            files: vec![(work, result)],
        },
        context,
    );
}

#[test]
fn explorer_snapshot_comes_only_from_server_listing_with_implicit_folders() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().canonicalize().unwrap();
    fs::create_dir(root.join(".tiptoptyp")).unwrap();
    fs::write(root.join(".tiptoptyp/private.tex"), b"generated").unwrap();
    fs::write(root.join("unlisted.tex"), b"not on Overleaf").unwrap();
    let tree = Tree::new(
        "Paper".into(),
        Some("a".into()),
        vec![
            entry("z", "z.tex", EntryKind::Doc),
            entry("a", "chapters/main.tex", EntryKind::Doc),
            entry("image", "figures/a.png", EntryKind::File),
        ],
    )
    .unwrap();
    let snapshot = remote_snapshot(&root, &tree);
    assert_eq!(snapshot.root, root);
    assert_eq!(
        snapshot
            .nodes
            .iter()
            .map(|node| node.display_name().into_owned())
            .collect::<Vec<_>>(),
        ["chapters", "figures", "z.tex"]
    );
    assert!(snapshot.find("chapters").unwrap().is_directory());
    assert_eq!(
        snapshot.find("chapters/main.tex").unwrap().path,
        root.join("chapters/main.tex")
    );
    assert!(snapshot.find("figures/a.png").unwrap().is_file());
    assert!(snapshot.find(".tiptoptyp/private.tex").is_none());
    assert!(snapshot.find("unlisted.tex").is_none());
    assert!(
        !root.join("chapters/main.tex").exists(),
        "listing construction must not hydrate on the UI thread"
    );
}

#[test]
fn clean_parked_document_refreshes_without_switching_active_editor() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().canonicalize().unwrap();
    let context = egui::Context::default();
    let mut app = fixture(&root, &context);
    let (main, chapter) = park_main(&mut app, &root, &context);
    let chapter_key = app.document().key();
    app.update_overleaf_event(
        BrowserEvent::Document {
            epoch: EPOCH,
            id: "a".into(),
            path: "main.tex".into(),
            source: "Remote main new\n".into(),
        },
        &context,
    )
    .unwrap();
    let work = app.overleaf.queued.pop_front().unwrap();
    assert_eq!(
        app.document_for_tab(main).unwrap().source(),
        "Main old\n",
        "browser event alone cannot imply backing mirror completion"
    );
    apply_work(&mut app, work, &root, EPOCH, &context);
    assert_eq!(
        app.document_for_tab(main).unwrap().source(),
        "Remote main new\n"
    );
    assert!(!app.document_for_tab(main).unwrap().is_dirty());
    assert_eq!(app.tabs.active_id(), Some(chapter));
    assert_eq!(app.document().key(), chapter_key);
    assert_eq!(app.document().source(), "Chapter old\n");
}

#[test]
fn dirty_parked_document_retains_local_edit_and_blocks_remote_overwrite() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().canonicalize().unwrap();
    let context = egui::Context::default();
    let mut app = fixture(&root, &context);
    let (main, chapter) = park_main(&mut app, &root, &context);
    app.tabs
        .document_mut(main)
        .unwrap()
        .edit(CCursorRange::default(), |text| {
            *text = "Local unsaved\n".into()
        });
    let local_key = app.document_for_tab(main).unwrap().key();
    app.update_overleaf_event(
        BrowserEvent::Document {
            epoch: EPOCH,
            id: "a".into(),
            path: "main.tex".into(),
            source: "Server changed\n".into(),
        },
        &context,
    )
    .unwrap();
    assert!(app.overleaf.blocked.contains("a"));
    let work = app.overleaf.queued.pop_front().unwrap();
    apply_work(&mut app, work, &root, EPOCH, &context);
    assert_eq!(
        fs::read_to_string(root.join("main.tex")).unwrap(),
        "Server changed\n"
    );
    let document = app.document_for_tab(main).unwrap();
    assert_eq!(document.source(), "Local unsaved\n");
    assert_eq!(document.key(), local_key);
    assert!(document.is_dirty());
    assert_eq!(app.tabs.active_id(), Some(chapter));
    assert!(!app.submit_overleaf_save(main, &root.join("main.tex"), &context));
    assert!(app.overleaf.pending.is_none());
}

#[test]
fn save_receipt_requires_remote_acknowledgement_and_successful_mirror_completion() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().canonicalize().unwrap();
    let context = egui::Context::default();
    let mut app = fixture(&root, &context);
    let tab = app.tabs.active_id().unwrap();
    let wire = pending_save(&mut app, tab, "a", "Saved over Overleaf\n");
    fs::write(root.join("main.tex"), b"Saved over Overleaf\n").unwrap();
    assert!(
        app.document().is_dirty(),
        "local cache writes are not server acknowledgements"
    );
    assert_eq!(app.document().saved_source(), "Main old\n");
    app.update_overleaf_event(
        BrowserEvent::Ack {
            epoch: EPOCH,
            id: "a".into(),
            request: wire,
            source: "Saved over Overleaf\n".into(),
        },
        &context,
    )
    .unwrap();
    assert!(
        app.document().is_dirty(),
        "the acknowledged snapshot still needs its mirror receipt"
    );
    assert!(app.document_workflow.save_in_flight);
    assert!(app.overleaf.pending.is_none());
    let work = app.overleaf.queued.pop_front().unwrap();
    assert!(work.save.is_some());
    apply_work(&mut app, work, &root, EPOCH, &context);
    assert!(!app.document().is_dirty());
    assert_eq!(app.document().saved_source(), "Saved over Overleaf\n");
    assert_eq!(
        app.document().disk_fingerprint(),
        Some(fingerprint(b"Saved over Overleaf\n"))
    );
    assert!(!app.document_workflow.save_in_flight);
}

#[test]
fn acknowledgement_of_older_edit_does_not_mark_newer_typing_saved() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().canonicalize().unwrap();
    let context = egui::Context::default();
    let mut app = fixture(&root, &context);
    app.settings.auto_save = true;
    app.settings.auto_save_delay_ms = 321;
    let tab = app.tabs.active_id().unwrap();
    let wire = pending_save(&mut app, tab, "a", "First edit\n");
    app.document_mut().edit(CCursorRange::default(), |text| {
        *text = "Newer typing\n".into()
    });
    app.update_overleaf_event(
        BrowserEvent::Ack {
            epoch: EPOCH,
            id: "a".into(),
            request: wire,
            source: "First edit\n".into(),
        },
        &context,
    )
    .unwrap();
    let work = app.overleaf.queued.pop_front().unwrap();
    app.set_active_autosave_deadline(None);
    let before_receipt = Instant::now();
    apply_work(&mut app, work, &root, EPOCH, &context);
    let delay = Duration::from_millis(321);
    let deadline = app
        .active_autosave_deadline()
        .expect("newer typing must be scheduled for another save after the older ACK receipt");
    assert!(deadline >= before_receipt + delay);
    assert!(deadline <= Instant::now() + delay);
    assert!(!app.document_workflow.save_in_flight);
    assert_eq!(app.document().source(), "Newer typing\n");
    assert_eq!(app.document().saved_source(), "First edit\n");
    assert!(app.document().is_dirty());
    assert!(
        app.overleaf
            .connection
            .as_ref()
            .unwrap()
            .model
            .has_unsynced()
    );
}

#[test]
fn obsolete_connection_mirror_receipts_cannot_clean_or_replace_current_document() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().canonicalize().unwrap();
    let context = egui::Context::default();
    let mut app = fixture(&root, &context);
    let tab = app.tabs.active_id().unwrap();
    let wire = pending_save(&mut app, tab, "a", "Old connection edit\n");
    app.update_overleaf_event(
        BrowserEvent::Ack {
            epoch: EPOCH,
            id: "a".into(),
            request: wire,
            source: "Old connection edit\n".into(),
        },
        &context,
    )
    .unwrap();
    let work = app.overleaf.queued.pop_front().unwrap();
    app.overleaf.epoch += 1;
    app.overleaf
        .connection
        .as_mut()
        .unwrap()
        .model
        .reconnect(EPOCH + 1);
    app.document_mut().edit(CCursorRange::default(), |text| {
        *text = "Current unsaved\n".into()
    });
    let key = app.document().key();
    apply_work(&mut app, work, &root, EPOCH, &context);
    assert_eq!(app.document().source(), "Current unsaved\n");
    assert_eq!(app.document().key(), key);
    assert_eq!(app.document().saved_source(), "Main old\n");
    assert!(app.document().is_dirty());
}

#[test]
fn another_tabs_save_and_wrong_owner_ack_cannot_consume_the_pending_request() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().canonicalize().unwrap();
    let context = egui::Context::default();
    let mut app = fixture(&root, &context);
    let (main, chapter) = park_main(&mut app, &root, &context);
    let wire = pending_save(&mut app, main, "a", "Main draft\n");
    app.document_mut().edit(CCursorRange::default(), |text| {
        *text = "Chapter draft\n".into()
    });
    assert!(!app.submit_overleaf_save(chapter, &root.join("chapter.tex"), &context));
    app.update_overleaf_event(
        BrowserEvent::Ack {
            epoch: EPOCH,
            id: "b".into(),
            request: wire,
            source: "Chapter draft\n".into(),
        },
        &context,
    )
    .unwrap();
    let pending = app.overleaf.pending.as_ref().unwrap();
    assert_eq!(
        (pending.tab, pending.id.as_str(), pending.wire),
        (main, "a", wire)
    );
    assert!(app.overleaf.queued.is_empty());
    assert!(app.document_for_tab(main).unwrap().is_dirty());
    assert!(app.document_for_tab(chapter).unwrap().is_dirty());
}

#[test]
fn connect_dialog_rejects_blank_and_typed_invalid_urls_without_a_browser() {
    let directory = tempfile::tempdir().unwrap();
    let context = egui::Context::default();
    let mut app = EditorApp::dormant_for_tests(&context, directory.path().into());
    app.document_mut()
        .replace_unprojected_untitled("Local document");
    let original = app.document().key();
    app.overleaf.visible = true;
    let mut harness = Harness::builder()
        .with_size(Vec2::new(1050.0, 760.0))
        .build_ui_state(|ui, app: &mut EditorApp| app.show_overleaf(ui.ctx()), app);
    harness.run();
    harness.get_by_label("Connect").click();
    harness.run();
    assert!(harness.state().overleaf.error.is_some());
    assert!(harness.state().overleaf.browser.is_none());
    assert!(harness.state().overleaf.connection.is_none());
    let invalid = "http://www.overleaf.com/project/0123456789abcdef01234567";
    let url_field = egui_kittest::kittest::by()
        .role(egui::accesskit::Role::TextInput)
        .predicate(|node| node.placeholder() == Some("https://www.overleaf.com/project/…"));
    harness.get(url_field.clone()).click();
    harness.run();
    harness.get(url_field).type_text(invalid);
    harness.run();
    assert_eq!(harness.state().overleaf.query, invalid);
    harness.get_by_label("Connect").click();
    harness.run();
    let app = harness.state();
    assert!(app.overleaf.error.as_ref().unwrap().contains("HTTPS"));
    assert!(!app.overleaf.create_browser);
    assert!(app.overleaf.browser.is_none());
    assert!(app.overleaf.connection.is_none());
    assert_eq!(app.overleaf.epoch, 0);
    assert_eq!(app.document().key(), original);
    assert_eq!(app.document().source(), "Local document");
    assert_eq!(app.overleaf.storage_draft.as_ref().unwrap().directory, "");
}

#[test]
fn opening_an_already_downloaded_document_uses_fresh_unchanged_server_snapshot() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().canonicalize().unwrap();
    let context = egui::Context::default();
    let mut app = fixture(&root, &context);
    // Background project hydration has already installed chapter's baseline.
    // The explicit Open response is therefore byte-identical to that baseline.
    app.overleaf.open_after = Some("b".into());
    app.update_overleaf_event(
        BrowserEvent::Document {
            epoch: EPOCH,
            id: "b".into(),
            path: "chapter.tex".into(),
            source: "Chapter old\n".into(),
        },
        &context,
    )
    .unwrap();
    if let Some(position) = app
        .overleaf
        .queued
        .iter()
        .position(|work| work.entry.id == "b")
    {
        let work = app.overleaf.queued.remove(position).unwrap();
        apply_work(&mut app, work, &root, EPOCH, &context);
    }
    assert_eq!(
        app.document().path().as_deref(),
        Some(root.join("chapter.tex").as_path())
    );
    assert_eq!(app.document().source(), "Chapter old\n");
    assert!(app.overleaf.open_after.is_none());
}

#[test]
fn older_same_connection_mirror_reply_cannot_install_a_superseded_server_snapshot() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().canonicalize().unwrap();
    let context = egui::Context::default();
    let mut app = fixture(&root, &context);
    let (main, chapter) = park_main(&mut app, &root, &context);
    app.update_overleaf_event(
        BrowserEvent::Document {
            epoch: EPOCH,
            id: "a".into(),
            path: "main.tex".into(),
            source: "Server first\n".into(),
        },
        &context,
    )
    .unwrap();
    let old_work = app.overleaf.queued.pop_front().unwrap();
    app.update_overleaf_event(
        BrowserEvent::Document {
            epoch: EPOCH,
            id: "a".into(),
            path: "main.tex".into(),
            source: "Server latest\n".into(),
        },
        &context,
    )
    .unwrap();
    apply_work(&mut app, old_work, &root, EPOCH, &context);
    assert_ne!(
        app.document_for_tab(main).unwrap().source(),
        "Server first\n",
        "a superseded in-flight cache job must not flash obsolete source into a clean editor"
    );
    let latest_work = app.overleaf.queued.pop_front().unwrap();
    apply_work(&mut app, latest_work, &root, EPOCH, &context);
    assert_eq!(
        app.document_for_tab(main).unwrap().source(),
        "Server latest\n"
    );
    assert_eq!(app.tabs.active_id(), Some(chapter));
}

fn listing(main_path: Option<&str>) -> BrowserEvent {
    let mut entries = vec![entry("b", "chapter.tex", EntryKind::Doc)];
    if let Some(path) = main_path {
        entries.push(entry("a", path, EntryKind::Doc));
    }
    BrowserEvent::Project {
        epoch: EPOCH,
        project_id: PROJECT.into(),
        name: "Remote paper".into(),
        entries,
        root_doc: Some(if main_path.is_some() { "a" } else { "b" }.into()),
    }
}

/// Deterministically complete the same bounded mirror batches as the I/O
/// worker, including jobs requeued when a newer tree supersedes their path.
fn apply_pending_cache(app: &mut EditorApp, root: &Path, context: &egui::Context) {
    for _ in 0..16 {
        if app.overleaf.tree_pending.is_none() && app.overleaf.queued.is_empty() {
            return;
        }
        let hydration_session = app.overleaf.connection.as_ref().unwrap().hydration.session;
        let tree = app.overleaf.tree_pending.take();
        let mirror = Mirror::open(root).unwrap();
        if let Some(tree) = &tree {
            mirror.apply_tree(tree).unwrap();
        }
        let files = app
            .overleaf
            .queued
            .drain(..app.overleaf.queued.len().min(32))
            .map(|work| {
                let result = match &work.data {
                    MirrorData::Document(source) => mirror.write_document(&work.entry, source),
                    MirrorData::File(source) => mirror.write_file(&work.entry, source),
                };
                (work, result)
            })
            .collect();
        app.apply_overleaf_mirror(
            Mirrored {
                hydration_session,
                epoch: EPOCH,
                tree,
                error: None,
                files,
            },
            context,
        );
    }
    panic!("mirror jobs did not settle after a finite rename transition");
}

#[test]
fn remote_rename_retargets_clean_and_dirty_parked_tabs_by_entity_id() {
    for dirty in [false, true] {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().canonicalize().unwrap();
        let context = egui::Context::default();
        let mut app = fixture(&root, &context);
        let (main, chapter) = park_main(&mut app, &root, &context);
        let chapter_key = app.document().key();
        if dirty {
            app.tabs
                .document_mut(main)
                .unwrap()
                .edit(CCursorRange::default(), |text| {
                    *text = "Local unsaved before rename\n".into();
                });
        }
        let owner = app.document_for_tab(main).unwrap().key().owner;
        app.update_overleaf_event(listing(Some("sections/renamed.tex")), &context)
            .unwrap();
        // No source publication is required: changing a filename need not
        // change the byte-identical authoritative document baseline.
        apply_pending_cache(&mut app, &root, &context);
        let document = app.document_for_tab(main).unwrap();
        assert_eq!(
            document.path().as_deref(),
            Some(root.join("sections/renamed.tex").as_path())
        );
        assert_eq!(document.key().owner, owner);
        assert_eq!(
            document.source(),
            if dirty {
                "Local unsaved before rename\n"
            } else {
                "Main old\n"
            }
        );
        assert_eq!(document.saved_source(), "Main old\n");
        assert_eq!(document.is_dirty(), dirty);
        assert_eq!(
            app.overleaf.tab_entities.get(&main).map(String::as_str),
            Some("a")
        );
        assert_eq!(app.tabs.active_id(), Some(chapter));
        assert_eq!(app.document().key(), chapter_key);
        assert!(!root.join("main.tex").exists());
        assert_eq!(
            fs::read_to_string(root.join("sections/renamed.tex")).unwrap(),
            "Main old\n"
        );
    }
}

#[test]
fn rename_during_ack_cache_completion_cannot_restore_an_obsolete_path() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().canonicalize().unwrap();
    let context = egui::Context::default();
    let mut app = fixture(&root, &context);
    let tab = app.tabs.active_id().unwrap();
    let wire = pending_save(&mut app, tab, "a", "Acknowledged edit\n");
    app.update_overleaf_event(
        BrowserEvent::Ack {
            epoch: EPOCH,
            id: "a".into(),
            request: wire,
            source: "Acknowledged edit\n".into(),
        },
        &context,
    )
    .unwrap();
    let work = app.overleaf.queued.pop_front().unwrap();
    assert!(work.save.is_some());
    // The old cache write has finished, but its UI receipt is delayed until
    // after the server has renamed the same entity.
    let result = Mirror::open(&root)
        .unwrap()
        .write_document(&work.entry, "Acknowledged edit\n");
    app.update_overleaf_event(listing(Some("renamed.tex")), &context)
        .unwrap();
    app.apply_overleaf_mirror(
        Mirrored {
            hydration_session: 0,
            epoch: EPOCH,
            tree: None,
            error: None,
            files: vec![(work, result)],
        },
        &context,
    );
    apply_pending_cache(&mut app, &root, &context);
    let document = app.document_for_tab(tab).unwrap();
    assert_eq!(
        document.path().as_deref(),
        Some(root.join("renamed.tex").as_path())
    );
    assert_eq!(document.source(), "Acknowledged edit\n");
    assert_eq!(document.saved_source(), "Acknowledged edit\n");
    assert!(!document.is_dirty());
    assert!(!app.document_workflow.save_in_flight);
    assert!(app.overleaf.pending.is_none());
    assert!(!root.join("main.tex").exists());
    assert_eq!(
        fs::read_to_string(root.join("renamed.tex")).unwrap(),
        "Acknowledged edit\n"
    );
}

#[test]
fn deleted_remote_document_retains_dirty_parked_source_and_rejects_save() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().canonicalize().unwrap();
    let context = egui::Context::default();
    let mut app = fixture(&root, &context);
    let (main, chapter) = park_main(&mut app, &root, &context);
    app.tabs
        .document_mut(main)
        .unwrap()
        .edit(CCursorRange::default(), |text| {
            *text = "Retain this local draft\n".into();
        });
    let key = app.document_for_tab(main).unwrap().key();
    app.update_overleaf_event(listing(None), &context).unwrap();
    apply_pending_cache(&mut app, &root, &context);
    let document = app.document_for_tab(main).unwrap();
    assert_eq!(document.source(), "Retain this local draft\n");
    assert_eq!(document.saved_source(), "Main old\n");
    assert_eq!(document.key(), key);
    assert!(document.is_dirty());
    assert_eq!(app.tabs.active_id(), Some(chapter));
    assert!(!app.submit_overleaf_save(main, &root.join("main.tex"), &context));
    assert!(app.overleaf.pending.is_none());
    assert!(app.overleaf.error.is_some());
    assert!(
        app.overleaf
            .connection
            .as_ref()
            .unwrap()
            .model
            .has_unsynced()
    );
    assert!(!root.join("main.tex").exists());
}

#[test]
fn acknowledgement_after_remote_deletion_releases_save_without_cleaning_the_draft() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().canonicalize().unwrap();
    let context = egui::Context::default();
    let mut app = fixture(&root, &context);
    let tab = app.tabs.active_id().unwrap();
    let wire = pending_save(&mut app, tab, "a", "Retain deleted in-flight edit\n");
    app.update_overleaf_event(listing(None), &context).unwrap();
    app.update_overleaf_event(
        BrowserEvent::Ack {
            epoch: EPOCH,
            id: "a".into(),
            request: wire,
            source: "Retain deleted in-flight edit\n".into(),
        },
        &context,
    )
    .unwrap();
    apply_pending_cache(&mut app, &root, &context);
    assert_eq!(app.document().source(), "Retain deleted in-flight edit\n");
    assert_eq!(app.document().saved_source(), "Main old\n");
    assert!(app.document().is_dirty());
    assert!(!app.document_workflow.save_in_flight);
    assert!(app.overleaf.pending.is_none());
    assert!(app.overleaf.error.is_some());
}

#[test]
fn save_as_local_copy_breaks_remote_entity_binding_and_is_not_retargeted() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().canonicalize().unwrap();
    let local_directory = tempfile::tempdir().unwrap();
    let local_path = local_directory
        .path()
        .canonicalize()
        .unwrap()
        .join("independent.tex");
    let context = egui::Context::default();
    let mut app = fixture(&root, &context);
    let tab = app.tabs.active_id().unwrap();
    app.overleaf.tab_entities.insert(tab, "a".into());
    let request = app
        .document()
        .prepare_save(local_path.clone(), DocumentKind::Tex)
        .unwrap();
    fs::write(&local_path, request.source()).unwrap();
    app.document_mut()
        .record_save(request.committed(fingerprint(b"Main old\n")))
        .unwrap();
    app.document_mut().edit(CCursorRange::default(), |text| {
        *text = "Independent local changes\n".into();
    });
    let key = app.document().key();
    app.update_overleaf_event(
        BrowserEvent::Document {
            epoch: EPOCH,
            id: "a".into(),
            path: "main.tex".into(),
            source: "Remote replacement\n".into(),
        },
        &context,
    )
    .unwrap();
    apply_pending_cache(&mut app, &root, &context);
    let local = app.document_for_tab(tab).unwrap();
    assert_eq!(local.path().as_deref(), Some(local_path.as_path()));
    assert_eq!(local.source(), "Independent local changes\n");
    assert_eq!(local.key(), key);
    assert!(local.is_dirty());
    assert!(!app.overleaf.tab_entities.contains_key(&tab));
    assert_eq!(fs::read_to_string(local_path).unwrap(), "Main old\n");
    let remote = app
        .overleaf
        .connection
        .as_ref()
        .unwrap()
        .model
        .document("a")
        .unwrap();
    assert_eq!(remote.source(), "Remote replacement\n");
    assert!(!remote.is_dirty());
}

#[test]
fn reconnect_after_ack_before_cache_receipt_resets_only_the_old_save_transaction() {
    let directory = tempfile::tempdir().unwrap();
    let base = directory.path().canonicalize().unwrap();
    let project =
        ProjectUrl::parse(&format!("https://www.overleaf.com/project/{PROJECT}")).unwrap();
    let storage = crate::overleaf::StorageLease::prepare(
        &project,
        &crate::overleaf::StorageOptions {
            keep_local_copy: true,
            directory: Some(base.clone()),
        },
        &base,
    )
    .unwrap();
    let root = storage.root().to_owned();
    let context = egui::Context::default();
    let mut app = fixture(&root, &context);
    app.overleaf.connection.as_mut().unwrap().storage = Some(storage);
    app.settings
        .set_overleaf_storage_preference(
            project.url(),
            crate::settings::OverleafStoragePreference {
                keep_local_copy: true,
                directory: Some(base.to_string_lossy().into_owned()),
            },
        )
        .unwrap();
    let tab = app.tabs.active_id().unwrap();
    let wire = pending_save(&mut app, tab, "a", "Old connection acknowledged\n");
    app.update_overleaf_event(
        BrowserEvent::Ack {
            epoch: EPOCH,
            id: "a".into(),
            request: wire,
            source: "Old connection acknowledged\n".into(),
        },
        &context,
    )
    .unwrap();
    let work = app.overleaf.queued.pop_front().unwrap();
    assert!(work.save.is_some());
    assert!(app.overleaf.pending.is_none());
    assert!(app.document_workflow.save_in_flight);
    let result = Mirror::open(&root)
        .unwrap()
        .write_document(&work.entry, "Old connection acknowledged\n");
    app.overleaf.query = format!("https://www.overleaf.com/project/{PROJECT}");
    app.connect_overleaf(&context).unwrap();
    assert_eq!(app.overleaf.epoch, EPOCH + 1);
    assert!(app.overleaf.pending.is_none());
    assert!(
        !app.document_workflow.save_in_flight,
        "an old connection's ACK-cache transaction cannot block the new connection"
    );
    let next_wire = pending_save(&mut app, tab, "a", "New connection draft\n");
    app.apply_overleaf_mirror(
        Mirrored {
            hydration_session: 0,
            epoch: EPOCH,
            tree: None,
            error: None,
            files: vec![(work, result)],
        },
        &context,
    );
    assert!(app.document_workflow.save_in_flight);
    assert_eq!(app.overleaf.pending.as_ref().unwrap().wire, next_wire);
    assert_eq!(app.document().source(), "New connection draft\n");
    assert_eq!(app.document().saved_source(), "Main old\n");
    assert!(app.document().is_dirty());
}

#[test]
fn overleaf_explorer_does_not_replace_a_selected_local_workspace() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().canonicalize().unwrap();
    let local_directory = tempfile::tempdir().unwrap();
    let local_root = local_directory.path().canonicalize().unwrap();
    let context = egui::Context::default();
    let mut app = fixture(&root, &context);
    assert!(app.overleaf_explorer().is_some());
    app.workspace_root = local_root.clone();
    assert!(app.overleaf_explorer().is_none());
    app.update_overleaf_event(listing(Some("main.tex")), &context)
        .unwrap();
    assert_eq!(app.workspace_root, local_root);
    assert!(app.overleaf_explorer().is_none());
    assert!(app.overleaf.connection.as_ref().unwrap().online);
    assert!(app.overleaf.workspace.is_some());
    app.workspace_root = root;
    assert!(app.overleaf_explorer().is_some());
}

#[test]
fn initial_project_selects_its_workspace_once_without_later_focus_stealing() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().canonicalize().unwrap();
    let local_directory = tempfile::tempdir().unwrap();
    let local_root = local_directory.path().canonicalize().unwrap();
    let context = egui::Context::default();
    let mut app = fixture(&root, &context);
    app.workspace_root = local_root.clone();
    app.overleaf.awaiting_project = true;
    app.overleaf.visible = true;
    app.overleaf.show_browser = true;
    app.update_overleaf_event(listing(Some("main.tex")), &context)
        .unwrap();
    assert_eq!(app.workspace_root, root);
    assert!(!app.overleaf.awaiting_project);
    assert!(!app.overleaf.visible);
    assert!(!app.overleaf.show_browser);
    assert!(app.overleaf_explorer().is_some());
    app.workspace_root = local_root.clone();
    app.overleaf.visible = true;
    app.update_overleaf_event(listing(Some("main.tex")), &context)
        .unwrap();
    assert_eq!(app.workspace_root, local_root);
    assert!(
        app.overleaf.visible,
        "a later listing must not hide an explicitly opened manager"
    );
    assert!(app.overleaf_explorer().is_none());
}

#[test]
fn selecting_a_local_workspace_preserves_remote_tab_roots_and_local_copies() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().canonicalize().unwrap();
    let local_directory = tempfile::tempdir().unwrap();
    let local_root = local_directory.path().canonicalize().unwrap();
    let context = egui::Context::default();
    let mut app = fixture(&root, &context);
    let (main, chapter) = park_main(&mut app, &root, &context);
    app.lifecycle = DocumentLifecycle::Active;
    app.new_tab(&context);
    app.lifecycle = DocumentLifecycle::Dormant;
    let copy = app.tabs.active_id().unwrap();
    app.document_mut().replace_loaded_unprojected(
        "Main old\n".into(),
        root.join("main.tex"),
        DocumentKind::Tex,
        Some(fingerprint(b"Main old\n")),
    );
    let local_path = local_root.join("independent.tex");
    let request = app
        .document()
        .prepare_save(local_path.clone(), DocumentKind::Tex)
        .unwrap();
    fs::write(&local_path, request.source()).unwrap();
    app.document_mut()
        .record_save(request.committed(fingerprint(b"Main old\n")))
        .unwrap();
    app.document_mut().edit(CCursorRange::default(), |text| {
        *text = "Independent local draft\n".into();
    });
    // A Save As copy can still have a stale ID until the next browser event.
    app.overleaf.tab_entities.insert(copy, "a".into());
    let key = app.document().key();
    app.workspace_root = local_root.clone();
    app.tabs.set_workspace(&local_root);
    app.restore_overleaf_tab_workspaces();
    assert_eq!(app.tab_workspace(main), Some(root.as_path()));
    assert_eq!(app.tab_workspace(chapter), Some(root.as_path()));
    assert_eq!(app.tab_workspace(copy), Some(local_root.as_path()));
    assert_eq!(app.workspace_root, local_root);
    assert_eq!(app.tabs.active_id(), Some(copy));
    assert_eq!(app.document().path().as_deref(), Some(local_path.as_path()));
    assert_eq!(app.document().source(), "Independent local draft\n");
    assert_eq!(app.document().key(), key);
    assert!(app.document().is_dirty());
    assert_eq!(app.document_for_tab(main).unwrap().source(), "Main old\n");
    assert_eq!(
        app.document_for_tab(chapter).unwrap().source(),
        "Chapter old\n"
    );
}

#[test]
fn mirror_roots_are_never_remembered_as_local_workspaces_without_a_connection() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().canonicalize().unwrap();
    let context = egui::Context::default();
    let mut app = EditorApp::dormant_for_tests(&context, root.clone());
    let previous = vec![root.to_string_lossy().into_owned()];
    app.settings.recent_workspaces = previous.clone();
    app.pending_settings = Some(app.settings.clone());
    let project =
        ProjectUrl::parse(&format!("https://www.overleaf.com/project/{PROJECT}")).unwrap();
    let mirror = project.mirror_root(&cache_base().unwrap());
    assert!(app.overleaf.connection.is_none());
    assert!(is_mirror_path(&mirror));
    app.remember_workspace(&mirror);
    app.remember_workspace(&mirror.join("chapters"));
    assert_eq!(app.settings.recent_workspaces, previous);
    assert_eq!(
        app.pending_settings.as_ref().unwrap().recent_workspaces,
        previous
    );
    let other_directory = tempfile::tempdir().unwrap();
    let other_root = other_directory.path().canonicalize().unwrap();
    app.remember_workspace(&other_root);
    let expected = vec![
        other_root.to_string_lossy().into_owned(),
        root.to_string_lossy().into_owned(),
    ];
    assert_eq!(app.settings.recent_workspaces, expected);
    assert_eq!(
        app.pending_settings.as_ref().unwrap().recent_workspaces,
        expected
    );
}

#[cfg(target_os = "macos")]
#[test]
fn connect_url_field_clears_native_window_controls_at_every_ui_scale() {
    for zoom in [0.75, 1.0, 1.5] {
        let directory = tempfile::tempdir().unwrap();
        let context = egui::Context::default();
        let mut app = EditorApp::dormant_for_tests(&context, directory.path().into());
        app.overleaf.visible = true;
        let mut harness = Harness::builder()
            .with_size(Vec2::new(1800.0, 1200.0))
            .build_ui_state(|ui, app: &mut EditorApp| app.show_overleaf(ui.ctx()), app);
        harness.ctx.set_zoom_factor(zoom);
        harness.run();
        let child = scoped_child_viewport_id(&harness.ctx, SALT);
        let window = harness
            .ctx
            .memory(|memory| memory.area_rect(egui::Id::new(child)))
            .expect("the embedded connection window must exist");
        let url = harness
            .get(
                egui_kittest::kittest::by()
                    .role(egui::accesskit::Role::TextInput)
                    .predicate(|node| {
                        node.placeholder() == Some("https://www.overleaf.com/project/…")
                    }),
            )
            .rect();
        let native_inset = (url.left() - window.left()) * zoom;
        assert!(
            native_inset >= 80.0,
            "URL field overlaps native controls at zoom {zoom}: inset {native_inset}"
        );
        assert!(
            url.right() <= window.right(),
            "URL field clips at zoom {zoom}"
        );
        assert!(harness.state().overleaf.browser.is_none());
    }
}

#[test]
fn remote_save_preserves_another_active_local_documents_expired_autosave_deadline() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().canonicalize().unwrap();
    let local_directory = tempfile::tempdir().unwrap();
    let local_root = local_directory.path().canonicalize().unwrap();
    let local_path = local_root.join("local.tex");
    let context = egui::Context::default();
    let mut app = fixture(&root, &context);
    app.settings.auto_save = true;
    let (remote, local) = park_main(&mut app, &root, &context);
    app.document_mut().replace_loaded_unprojected(
        "Local saved\n".into(),
        local_path.clone(),
        DocumentKind::Tex,
        Some(fingerprint(b"Local saved\n")),
    );
    app.document_mut().edit(CCursorRange::default(), |text| {
        *text = "Local unsaved\n".into();
    });
    let key = app.document().key();
    let deadline = Instant::now() - Duration::from_secs(1);
    app.set_active_autosave_deadline(Some(deadline));
    let wire = pending_save(&mut app, remote, "a", "Remote save still pending\n");
    assert!(!app.save_job.is_running());
    assert!(app.document_workflow.save_in_flight);
    app.tick_autosave(&context);
    assert_eq!(app.active_autosave_deadline(), Some(deadline));
    assert_eq!(app.tabs.active_id(), Some(local));
    assert_eq!(app.document().key(), key);
    assert_eq!(app.document().source(), "Local unsaved\n");
    assert!(app.document().is_dirty());
    assert!(!app.save_job.is_running());
    let pending = app.overleaf.pending.as_ref().unwrap();
    assert_eq!((pending.tab, pending.wire), (remote, wire));
    assert!(
        !local_path.exists(),
        "the remote transaction still owns the save slot"
    );
}

#[test]
fn exporting_the_last_conflicted_draft_releases_remote_state_before_reopening() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().canonicalize().unwrap();
    let local_directory = tempfile::tempdir().unwrap();
    let local_path = local_directory
        .path()
        .canonicalize()
        .unwrap()
        .join("retained.tex");
    let context = egui::Context::default();
    let mut app = fixture(&root, &context);
    let local = app.tabs.active_id().unwrap();
    app.document_mut().edit(CCursorRange::default(), |text| {
        *text = "Retain my local draft\n".into();
    });
    assert!(
        !app.overleaf
            .connection
            .as_ref()
            .unwrap()
            .model
            .has_unsynced()
    );
    assert!(app.overleaf_explorer().unwrap().1.ends_with("Unsynced"));
    let wire = pending_save(&mut app, local, "a", "Retain my local draft\n");
    app.update_overleaf_event(
        BrowserEvent::Conflict {
            epoch: EPOCH,
            id: "a".into(),
            request: wire,
            source: "Latest server version\n".into(),
            message: "A remote edit won the race.".into(),
        },
        &context,
    )
    .unwrap();
    assert!(app.overleaf.blocked.contains("a"));
    assert_eq!(app.overleaf.error.as_deref(), Some(CONFLICT_ERROR));
    assert!(
        app.overleaf
            .connection
            .as_ref()
            .unwrap()
            .model
            .has_unsynced()
    );
    assert!(app.overleaf.tab_entities.contains_key(&local));
    let request = app
        .document()
        .prepare_save(local_path.clone(), DocumentKind::Tex)
        .unwrap();
    fs::write(&local_path, request.source()).unwrap();
    app.document_mut()
        .record_save(request.committed(fingerprint(b"Retain my local draft\n")))
        .unwrap();
    app.document_mut().edit(CCursorRange::default(), |text| {
        *text = "Independent local changes after export\n".into();
    });
    let local_key = app.document().key();
    app.prune_overleaf_bindings();
    assert!(!app.overleaf.tab_entities.contains_key(&local));
    assert!(app.overleaf.blocked.is_empty());
    assert!(app.overleaf.error.is_none());
    let model = &app.overleaf.connection.as_ref().unwrap().model;
    assert!(!model.has_unsynced());
    let remote = model.document("a").unwrap();
    assert_eq!(remote.source(), "Latest server version\n");
    assert!(!remote.is_dirty());
    assert!(remote.conflict.is_none());
    // Open's fresh browser reply is byte-identical to the current server baseline.
    app.overleaf.open_after = Some("a".into());
    app.update_overleaf_event(
        BrowserEvent::Document {
            epoch: EPOCH,
            id: "a".into(),
            path: "main.tex".into(),
            source: "Latest server version\n".into(),
        },
        &context,
    )
    .unwrap();
    let work = app.overleaf.queued.pop_front().unwrap();
    // A real owner is active; dormant fixtures intentionally replace their
    // sole synthetic document instead of opening a tab or starting services.
    app.lifecycle = DocumentLifecycle::Active;
    apply_work(&mut app, work, &root, EPOCH, &context);
    app.lifecycle = DocumentLifecycle::Dormant;
    assert_ne!(app.tabs.active_id(), Some(local));
    assert_eq!(
        app.document().path().as_deref(),
        Some(root.join("main.tex").as_path())
    );
    assert_eq!(app.document().source(), "Latest server version\n");
    assert!(!app.document().is_dirty());
    let remote = app
        .overleaf
        .connection
        .as_ref()
        .unwrap()
        .model
        .document("a")
        .unwrap();
    assert!(!remote.is_dirty());
    assert!(remote.conflict.is_none());
    assert!(app.overleaf.blocked.is_empty());
    assert!(app.overleaf.error.is_none());
    let exported = app.document_for_tab(local).unwrap();
    assert_eq!(exported.path().as_deref(), Some(local_path.as_path()));
    assert_eq!(exported.key(), local_key);
    assert_eq!(
        exported.source(),
        "Independent local changes after export\n"
    );
    assert!(exported.is_dirty());
    assert_eq!(
        fs::read_to_string(local_path).unwrap(),
        "Retain my local draft\n"
    );
}

fn presence_event(source: &str, epoch: u64) -> BrowserEvent {
    serde_json::from_value(serde_json::json!({"event":"presence","epoch":epoch,"own_id":"own","self_user_id":"account",
        "peers":[{"id":"remote","user_id":"another","name":"Collaborator","doc_id":"a","row":0,"column":1}],
        "documents":[{"id":"a","source_fingerprint":SourceIndex::new(source).fingerprint,"source_utf16":source.encode_utf16().count(),"cursors":[{"id":"remote","anchor":1,"head":1}]}]})).unwrap()
}
#[test]
fn presence_overlay_reuses_revision_cache_and_rejects_local_divergence() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().canonicalize().unwrap();
    let context = egui::Context::default();
    let mut app = fixture(&root, &context);
    app.update_overleaf_event(presence_event("Main old\n", EPOCH), &context)
        .unwrap();
    let before = app.overleaf_presence_for_active();
    assert_eq!(before.len(), 1);
    assert_eq!(before[0].head, 1);
    assert!(Arc::ptr_eq(&before, &app.overleaf_presence_for_active()));
    app.document_mut().edit(CCursorRange::default(), |source| {
        source.insert_str(0, "local ")
    });
    assert!(app.overleaf_presence_for_active().is_empty());
    assert_ne!(
        app.overleaf.presence_cache.as_ref().unwrap().key,
        app.document().key()
    );
    assert_eq!(app.overleaf_collaborators().len(), 1);
    assert!(!app.overleaf.io.is_running());
    assert!(app.overleaf.queued.is_empty());
}
#[test]
fn newer_presence_waits_for_matching_native_source_instead_of_painting_stale_ranges() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().canonicalize().unwrap();
    let context = egui::Context::default();
    let mut app = fixture(&root, &context);
    app.update_overleaf_event(presence_event("A newer source\n", EPOCH), &context)
        .unwrap();
    assert!(app.overleaf_presence_for_active().is_empty());
    assert_eq!(app.overleaf_collaborators().len(), 1);
    app.document_mut().replace_loaded_unprojected(
        "A newer source\n".into(),
        root.join("main.tex"),
        DocumentKind::Tex,
        None,
    );
    assert_eq!(app.overleaf_presence_for_active().len(), 1);
    app.overleaf.connection.as_mut().unwrap().online = false;
    assert!(app.overleaf_presence_for_active().is_empty());
    assert!(app.overleaf_collaborators().is_empty());
}

#[test]
fn presence_uses_projected_editor_coordinates_not_hidden_mitex_imports() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().canonicalize().unwrap();
    let context = egui::Context::default();
    let mut app = fixture(&root, &context);
    let canonical = "#import \"@preview/mitex:0.2.7\": mi\n文 #mi(`\\alpha+😀`) end\n";
    let path = root.join("main.typ");
    app.document_mut().replace_loaded_unprojected(
        canonical.into(),
        path,
        DocumentKind::Typst,
        None,
    );
    app.document_mut()
        .enable(tiptoptyp::mitex_projection::Config::default())
        .unwrap();
    let offset = canonical[..canonical.find("end").unwrap()]
        .encode_utf16()
        .count();
    let event=serde_json::from_value(serde_json::json!({"event":"presence","epoch":EPOCH,"own_id":"own","self_user_id":"account",
        "peers":[{"id":"remote","user_id":"another","name":"Collaborator","doc_id":"a","row":1,"column":offset}],
        "documents":[{"id":"a","source_fingerprint":SourceIndex::new(canonical).fingerprint,"source_utf16":canonical.encode_utf16().count(),"cursors":[{"id":"remote","anchor":offset,"head":offset}]}]})).unwrap();
    let connection = app.overleaf.connection.as_mut().unwrap();
    connection
        .model
        .apply(BrowserEvent::Project {
            epoch: EPOCH,
            project_id: PROJECT.into(),
            name: "Paper".into(),
            entries: vec![entry("a", "main.typ", EntryKind::Doc)],
            root_doc: Some("a".into()),
        })
        .unwrap();
    app.update_overleaf_event(event, &context).unwrap();
    let cursors = app.overleaf_presence_for_active();
    assert_eq!(cursors.len(), 1);
    let displayed = app.document().source();
    let expected = displayed[..displayed.find("end").unwrap()].chars().count();
    assert_eq!(cursors[0].head, expected);
    assert!(expected < offset);
}

#[test]
fn working_alone_does_not_reindex_the_document_for_each_native_keystroke() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().canonicalize().unwrap();
    let context = egui::Context::default();
    let mut app = fixture(&root, &context);
    assert!(app.overleaf_presence_for_active().is_empty());
    let indexed = app.overleaf.presence_cache.as_ref().unwrap().key;
    for _ in 0..3 {
        app.document_mut()
            .edit(CCursorRange::default(), |source| source.push('x'));
        assert!(app.overleaf_presence_for_active().is_empty());
        assert_eq!(app.overleaf.presence_cache.as_ref().unwrap().key, indexed);
        assert_ne!(app.document().key(), indexed);
    }
}

#[test]
fn syncing_remote_source_rejects_typing_paste_and_find_replacement_but_keeps_selection() {
    for diff in [false, true] {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().canonicalize().unwrap();
        let mut harness = editor_particles::tests::editor(false);
        let context = harness.ctx.clone();
        let mut app = fixture(&root, &context);
        app.snapshot_scene = None;
        app.settings.sticky_context_rows = false;
        app.overleaf.connection.as_mut().unwrap().online = false;
        if diff {
            app.document_diff = Some(crate::git::editor::document_diff::DocumentDiff::new(
                "Main before\n".into(),
                app.document().source(),
            ));
        }
        app.document_mut().set_history_reset(false);
        app.pending_editor_selection = Some(EditorSelection::Focus(0..4));
        *harness.state_mut() = app;
        harness.run_steps(3);
        assert!(harness.state().overleaf_source_locked());
        let key = harness.state().document().key();
        let source = harness.state().document().source().clone();
        harness.event(egui::Event::Text("must not replace selection".into()));
        harness.run_steps(1);
        harness.event(egui::Event::Paste("must not paste".into()));
        harness.run_steps(1);
        assert_eq!(harness.state().document().source(), &source);
        assert_eq!(harness.state().document().key(), key);
        assert_eq!(harness.state().document_diff.is_some(), diff);
        harness.state_mut().store_editor_cursor(
            &context,
            CCursorRange {
                primary: CCursor::new(4),
                secondary: CCursor::new(0),
                ..Default::default()
            },
        );
        let editor = source_editor_id(&context);
        context.memory_mut(|memory| memory.request_focus(editor));
        harness.key_press_modifiers(Modifiers::SHIFT, egui::Key::ArrowRight);
        harness.run_steps(1);
        assert_eq!(
            harness
                .state_mut()
                .editor_snapshot(&context)
                .cursor
                .as_sorted_char_range(),
            egui::text::CharIndex(0)..egui::text::CharIndex(5),
        );
        harness.event(egui::Event::Copy);
        harness.run_steps(1);
        assert!(
            harness
                .output()
                .platform_output
                .commands
                .iter()
                .any(|command| {
                    matches!(command, egui::OutputCommand::CopyText(text) if text == "Main ")
                })
        );
        let app = harness.state_mut();
        app.find_bar.query = "Main".into();
        app.find_bar.replacement = "Changed".into();
        app.apply_find_actions(&context, false, false, true, false);
        app.apply_find_actions(&context, false, false, false, true);
        assert_eq!(app.document().source(), &source);
        assert_eq!(app.document().key(), key);
    }
}

#[test]
fn syncing_remote_source_blocks_commands_and_generated_insertions_without_disabling_widget_edits() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().canonicalize().unwrap();
    let context = egui::Context::default();
    let mut app = fixture(&root, &context);
    app.overleaf.connection.as_mut().unwrap().online = false;
    assert!(app.overleaf_source_locked());
    let key = app.document().key();
    let source = app.document().source().clone();
    for spec in crate::native_menu::COMMAND_SPECS {
        if !source_mutating_command(spec.command) {
            continue;
        }
        if settings_window::is_widget_edit_command(spec.command) {
            assert!(app.native_command_enabled(spec.command));
        } else {
            assert!(!app.native_command_enabled(spec.command));
        }
        app.execute_app_command(spec.command, &context, None);
        assert_eq!(app.document().key(), key, "{:?}", spec.command);
        assert_eq!(app.document().source(), &source, "{:?}", spec.command);
    }
    app.insert_editor_text(&context, "file dropped from Explorer");
    app.insert_generated_symbol(
        &context,
        &crate::handwriting::Insertion {
            text: "\\alpha".into(),
            package: None,
        },
    );
    assert_eq!(app.document().key(), key);
    assert_eq!(app.document().source(), &source);
    assert!(app.table_editor.is_none());
    assert!(app.citation.is_none());
}

#[test]
fn formatter_and_current_diagnostic_replies_cannot_edit_while_remote_source_is_syncing() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().canonicalize().unwrap();
    let context = egui::Context::default();
    let mut app = fixture(&root, &context);
    app.overleaf.connection.as_mut().unwrap().online = false;
    let key = app.document().key();
    let source = app.document().source().clone();
    app.format_request_key = Some(key);
    app.manual_format_revision = Some(key.revision);
    app.receive_formatted_document(&context, Generation(1), "file:///ignored", 0, None);
    assert!(app.format_request_key.is_none());
    assert!(app.manual_format_revision.is_none());
    let diagnostic = Diagnostic {
        correction: Some(crate::diagnostics::DiagnosticCorrection {
            range: 0..4,
            original: "Main".into(),
            rule: None,
            category: None,
            replacements: vec!["Changed".into()],
            document: Some(key),
        }),
        provider: Some("Harper".into()),
        severity: DiagnosticSeverity::Help,
        source: DiagnosticSource::Main,
        location: Some(crate::diagnostics::DiagnosticLocation { line: 1, column: 1 }),
        message: "Correction".into(),
        details: Vec::new(),
    };
    assert!(
        app.apply_diagnostic_correction(&diagnostic, 0, &context)
            .is_err()
    );
    assert_eq!(app.document().key(), key);
    assert_eq!(app.document().source(), &source);
}

#[test]
fn local_document_remains_editable_while_a_parked_overleaf_project_is_syncing() {
    let directory = tempfile::tempdir().unwrap();
    let base = directory.path().canonicalize().unwrap();
    let root = base.join("remote");
    fs::create_dir(&root).unwrap();
    let context = egui::Context::default();
    let mut app = fixture(&root, &context);
    app.overleaf.connection.as_mut().unwrap().online = false;
    app.document_mut().replace_loaded_unprojected(
        "Local\n".into(),
        base.join("local.tex"),
        DocumentKind::Tex,
        None,
    );
    assert!(!app.overleaf_source_locked());
    app.store_editor_cursor(&context, CCursorRange::one(CCursor::new(0)));
    app.insert_editor_text(&context, "Allowed ");
    assert_eq!(app.document().source(), "Allowed Local\n");
}

#[test]
fn reconnect_rename_waits_for_fresh_remote_bytes_before_relocating_an_open_tab() {
    for fresh in ["Main old\n", "Fresh remote revision\n"] {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().canonicalize().unwrap();
        let context = egui::Context::default();
        let mut app = fixture(&root, &context);
        let tab = app.tabs.active_id().unwrap();
        app.overleaf.tab_entities.insert(tab, "a".into());
        let connection = app.overleaf.connection.as_mut().unwrap();
        connection.online = false;
        connection.hydration.restart();
        app.update_overleaf_event(listing(Some("renamed.tex")), &context)
            .unwrap();
        assert!(
            app.overleaf.queued.is_empty(),
            "the retained model baseline is not a fresh server snapshot"
        );
        assert_eq!(app.document().path().as_ref(), Some(&root.join("main.tex")));
        assert!(!root.join("renamed.tex").exists());
        assert!(app.overleaf_source_locked());
        app.update_overleaf_event(
            BrowserEvent::Document {
                epoch: EPOCH,
                id: "a".into(),
                path: "renamed.tex".into(),
                source: fresh.into(),
            },
            &context,
        )
        .unwrap();
        let work = app
            .overleaf
            .queued
            .front()
            .expect("fresh bytes must be materialized, including unchanged contents");
        assert_eq!(work.entry.id, "a");
        assert_eq!(work.entry.path, "renamed.tex");
        assert!(matches!(&work.data, MirrorData::Document(source) if source == fresh));
        assert_eq!(app.document().path().as_ref(), Some(&root.join("main.tex")));
    }
}

#[test]
fn matching_old_session_mirror_receipts_cannot_complete_a_new_hydration_barrier() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().canonicalize().unwrap();
    let context = egui::Context::default();
    let mut app = fixture(&root, &context);
    let tree = app
        .overleaf
        .connection
        .as_ref()
        .unwrap()
        .model
        .tree()
        .unwrap()
        .clone();
    let hydration = &mut app.overleaf.connection.as_mut().unwrap().hydration;
    let old_session = hydration.session;
    hydration.restart();
    hydration.reset(&tree);
    let current_session = hydration.session;
    let make_files = || {
        tree.entries()
            .iter()
            .map(|entry| {
                let source = if entry.id == "a" {
                    "Main old\n"
                } else {
                    "Chapter old\n"
                };
                let result = Mirror::open(&root).unwrap().write_document(entry, source);
                (
                    MirrorWork {
                        entry: entry.clone(),
                        data: MirrorData::Document(source.into()),
                        save: None,
                    },
                    result,
                )
            })
            .collect::<Vec<_>>()
    };
    app.apply_overleaf_mirror(
        Mirrored {
            epoch: EPOCH,
            hydration_session: old_session,
            tree: Some(tree.clone()),
            error: None,
            files: make_files(),
        },
        &context,
    );
    app.finish_overleaf_hydration(&context);
    assert!(
        app.overleaf_source_locked(),
        "old matching tree and file receipts must not retire the new session"
    );
    app.apply_overleaf_mirror(
        Mirrored {
            epoch: EPOCH,
            hydration_session: current_session,
            tree: Some(tree),
            error: None,
            files: Vec::new(),
        },
        &context,
    );
    app.finish_overleaf_hydration(&context);
    assert!(
        app.overleaf_source_locked(),
        "the new tree receipt alone cannot reuse old-session files"
    );
    let current_files = app
        .overleaf
        .connection
        .as_ref()
        .unwrap()
        .model
        .tree()
        .unwrap()
        .entries()
        .iter()
        .map(|entry| {
            let source = if entry.id == "a" {
                "Main old\n"
            } else {
                "Chapter old\n"
            };
            let result = Mirror::open(&root).unwrap().write_document(entry, source);
            (
                MirrorWork {
                    entry: entry.clone(),
                    data: MirrorData::Document(source.into()),
                    save: None,
                },
                result,
            )
        })
        .collect();
    app.apply_overleaf_mirror(
        Mirrored {
            epoch: EPOCH,
            hydration_session: current_session,
            tree: None,
            error: None,
            files: current_files,
        },
        &context,
    );
    app.finish_overleaf_hydration(&context);
    assert!(
        !app.overleaf_source_locked(),
        "fresh successful receipts must actually release editing"
    );
}

#[test]
fn rejected_or_unsent_refresh_preserves_pending_saves_passive_work_and_sync_readiness() {
    for phase in 0..3 {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().canonicalize().unwrap();
        let context = egui::Context::default();
        let mut app = fixture(&root, &context);
        let tab = app.tabs.active_id().unwrap();
        if phase < 2 {
            let wire = pending_save(&mut app, tab, "a", "Retained local save\n");
            if phase == 1 {
                app.update_overleaf_event(
                    BrowserEvent::Ack {
                        epoch: EPOCH,
                        id: "a".into(),
                        request: wire,
                        source: "Retained local save\n".into(),
                    },
                    &context,
                )
                .unwrap();
                assert!(app.overleaf.pending.is_none());
                assert!(app.overleaf.save_active);
            }
        }
        app.overleaf.queue(MirrorWork {
            entry: entry("b", "chapter.tex", EntryKind::Doc),
            data: MirrorData::Document("Chapter old\n".into()),
            save: None,
        });
        let key = app.document().key();
        let source = app.document().source().clone();
        let pending = app.overleaf.pending.as_ref().map(|save| save.wire);
        let save_active = app.overleaf.save_active;
        let save_in_flight = app.document_workflow.save_in_flight;
        let queued: Vec<_> = app
            .overleaf
            .queued
            .iter()
            .map(|work| (work.entry.clone(), work.save.is_some()))
            .collect();
        let connection = app.overleaf.connection.as_mut().unwrap();
        connection.hydration.session = 17;
        let status = connection.status.clone();
        assert!(connection.online);
        assert!(!connection.hydration.pending);
        // There is deliberately no browser. The last case exercises failed
        // dispatch, while the first two must refuse even to attempt refresh.
        app.refresh_overleaf();
        let connection = app.overleaf.connection.as_ref().unwrap();
        assert!(connection.online, "phase {phase}");
        assert!(!connection.hydration.pending, "phase {phase}");
        assert_eq!(connection.hydration.session, 17);
        assert_eq!(connection.status, status);
        assert_eq!(app.overleaf.pending.as_ref().map(|save| save.wire), pending);
        assert_eq!(app.overleaf.save_active, save_active);
        assert_eq!(app.document_workflow.save_in_flight, save_in_flight);
        assert_eq!(
            app.overleaf
                .queued
                .iter()
                .map(|work| (work.entry.clone(), work.save.is_some()))
                .collect::<Vec<_>>(),
            queued
        );
        assert_eq!(app.document().key(), key);
        assert_eq!(app.document().source(), &source);
        assert!(app.overleaf.error.is_some());
    }
}

#[test]
fn passive_mirror_queue_retains_large_project_listings_up_to_the_declared_entry_limit() {
    let work = |index| MirrorWork {
        entry: entry(
            &format!("doc-{index}"),
            &format!("part-{index}.tex"),
            EntryKind::Doc,
        ),
        data: MirrorData::Document("x".into()),
        save: None,
    };
    let mut state = State::default();
    for index in 0..300 {
        state.queue(work(index));
    }
    assert_eq!(
        state.queued.len(),
        300,
        "a project with more than 256 tiny files must not silently lose downloads"
    );
    assert!(state.error.is_none());
    // Fill the remaining fixture slots directly so testing the upper bound
    // doesn't turn into a quadratic 10,000-enqueue performance workload.
    state
        .queued
        .extend((300..crate::overleaf::MAX_ENTRIES - 1).map(work));
    state.queue(work(crate::overleaf::MAX_ENTRIES - 1));
    assert_eq!(state.queued.len(), crate::overleaf::MAX_ENTRIES);
    assert!(state.error.is_none());
    assert!(
        state
            .queued
            .iter()
            .enumerate()
            .all(|(index, work)| work.entry.id == format!("doc-{index}"))
    );
    state.queue(work(crate::overleaf::MAX_ENTRIES));
    assert_eq!(state.queued.len(), crate::overleaf::MAX_ENTRIES);
    assert!(
        state.error.is_some(),
        "the declared project bound remains enforced"
    );
}

#[test]
fn delayed_project_and_file_publications_cannot_replace_a_newer_local_editor() {
    for edit_before_project in [false, true] {
        let directory = tempfile::tempdir().unwrap();
        let base = directory.path().canonicalize().unwrap();
        let remote = base.join("remote");
        let local = base.join("local");
        fs::create_dir(&remote).unwrap();
        fs::create_dir(&local).unwrap();
        let context = egui::Context::default();
        let mut app = fixture(&remote, &context);
        app.workspace_root = local.clone();
        let path = local.join("draft.tex");
        app.document_mut().replace_loaded_unprojected(
            "Local source\n".into(),
            path.clone(),
            DocumentKind::Tex,
            None,
        );
        app.overleaf.connection.as_mut().unwrap().online = false;
        app.overleaf.awaiting_project = true;
        app.arm_overleaf_navigation();
        if edit_before_project {
            app.document_mut().edit(CCursorRange::default(), |source| {
                source.push_str("new typing")
            });
        }
        app.update_overleaf_event(listing(Some("main.tex")), &context)
            .unwrap();
        if edit_before_project {
            assert_eq!(app.workspace_root, local);
        } else {
            assert_eq!(
                app.workspace_root, remote,
                "the authorized Project update advances its owner's expected workspace"
            );
            app.document_mut().edit(CCursorRange::default(), |source| {
                source.push_str("new typing")
            });
        }
        let key = app.document().key();
        let source = app.document().source().clone();
        app.update_overleaf_event(
            BrowserEvent::Document {
                epoch: EPOCH,
                id: "a".into(),
                path: "main.tex".into(),
                source: "Fresh remote source\n".into(),
            },
            &context,
        )
        .unwrap();
        let work = app.overleaf.queued.pop_front().unwrap();
        apply_work(&mut app, work, &remote, EPOCH, &context);
        assert_eq!(app.document().path().as_ref(), Some(&path));
        assert_eq!(app.document().key(), key);
        assert_eq!(app.document().source(), &source);
        assert!(app.overleaf.open_after.is_none());
        assert!(
            app.overleaf.connection.is_some(),
            "background sync remains connected independently of navigation"
        );
        assert_eq!(
            fs::read_to_string(remote.join("main.tex")).unwrap(),
            "Fresh remote source\n"
        );
    }
}

#[test]
fn authorized_project_workspace_change_does_not_cancel_its_delayed_document_open() {
    let directory = tempfile::tempdir().unwrap();
    let base = directory.path().canonicalize().unwrap();
    let remote = base.join("remote");
    let local = base.join("local");
    fs::create_dir(&remote).unwrap();
    fs::create_dir(&local).unwrap();
    let context = egui::Context::default();
    let mut app = fixture(&remote, &context);
    app.workspace_root = local.clone();
    app.document_mut().replace_loaded_unprojected(
        "Local\n".into(),
        local.join("draft.tex"),
        DocumentKind::Tex,
        None,
    );
    app.overleaf.connection.as_mut().unwrap().online = false;
    app.overleaf.awaiting_project = true;
    app.arm_overleaf_navigation();
    app.overleaf.reopen_relative = Some((
        format!("https://www.overleaf.com/project/{PROJECT}"),
        EPOCH,
        PathBuf::from("main.tex"),
    ));
    app.update_overleaf_event(listing(Some("main.tex")), &context)
        .unwrap();
    assert!(app.overleaf_navigation_current());
    assert!(
        app.overleaf.reopen_relative.is_none(),
        "a relative reopen request is consumed by one project listing"
    );
    app.update_overleaf_event(
        BrowserEvent::Document {
            epoch: EPOCH,
            id: "a".into(),
            path: "main.tex".into(),
            source: "Fresh remote\n".into(),
        },
        &context,
    )
    .unwrap();
    let work = app.overleaf.queued.pop_front().unwrap();
    apply_work(&mut app, work, &remote, EPOCH, &context);
    assert_eq!(
        app.document().path().as_ref(),
        Some(&remote.join("main.tex"))
    );
    assert_eq!(app.document().source(), "Fresh remote\n");
    assert_eq!(app.workspace_root, remote);
    assert!(app.overleaf.navigation.is_none());
}

#[test]
fn reconnect_uses_pending_storage_preferences_and_persists_same_storage_draft_changes() {
    for use_draft in [false, true] {
        let directory = tempfile::tempdir().unwrap();
        let base = directory.path().canonicalize().unwrap();
        let project =
            ProjectUrl::parse(&format!("https://www.overleaf.com/project/{PROJECT}")).unwrap();
        let lease = crate::overleaf::StorageLease::prepare(
            &project,
            &crate::overleaf::StorageOptions {
                keep_local_copy: false,
                directory: None,
            },
            &base,
        )
        .unwrap();
        let context = egui::Context::default();
        let mut app = fixture(lease.root(), &context);
        app.overleaf.connection.as_mut().unwrap().storage = Some(lease);
        let preference = crate::settings::OverleafStoragePreference {
            keep_local_copy: false,
            directory: Some(base.join("chosen-folder").to_string_lossy().into_owned()),
        };
        if use_draft {
            app.settings
                .set_overleaf_storage_preference(
                    project.url(),
                    crate::settings::OverleafStoragePreference {
                        keep_local_copy: false,
                        directory: None,
                    },
                )
                .unwrap();
            app.overleaf.storage_draft =
                Some(storage_ui::StorageDraft::new(project.url(), &preference));
        } else {
            let mut pending = app.settings.clone();
            pending
                .set_overleaf_storage_preference(project.url(), preference.clone())
                .unwrap();
            app.pending_settings = Some(pending);
        }
        app.overleaf.query = project.url().into();
        app.overleaf.reopen_relative = Some((
            "https://www.overleaf.com/project/ffffffffffffffffffffffff".into(),
            EPOCH,
            PathBuf::from("chapter.tex"),
        ));
        app.connect_overleaf(&context).unwrap();
        assert!(
            app.overleaf.reopen_relative.is_none(),
            "manual Connect clears another project's relative-file request"
        );
        assert_eq!(app.overleaf.epoch, EPOCH + 1);
        assert_eq!(
            app.pending_settings
                .as_ref()
                .unwrap()
                .overleaf_storage_preference(project.url())
                .unwrap(),
            preference
        );
        assert!(
            !app.overleaf.storage_job.is_running(),
            "reusing the same temporary lease doesn't prepare another copy"
        );
    }
}

#[test]
fn transport_disconnect_and_reconnect_requires_fresh_documents_and_assets_again() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().canonicalize().unwrap();
    let context = egui::Context::default();
    let mut app = fixture(&root, &context);
    let entries = vec![
        entry("a", "main.tex", EntryKind::Doc),
        entry("b", "chapter.tex", EntryKind::Doc),
        entry("image", "image.png", EntryKind::File),
    ];
    let project_event = || BrowserEvent::Project {
        epoch: EPOCH,
        project_id: PROJECT.into(),
        name: "Remote paper".into(),
        entries: entries.clone(),
        root_doc: Some("a".into()),
    };
    let connection = app.overleaf.connection.as_mut().unwrap();
    connection.model.apply(project_event()).unwrap();
    let tree = connection.model.tree().unwrap().clone();
    connection.hydration.reset(&tree);
    connection.hydration.accept_tree(&tree);
    for entry in tree.entries() {
        connection.hydration.accept_file(entry);
    }
    connection.hydration.pending = false;
    let prior_session = connection.hydration.session;
    app.update_overleaf_event(
        BrowserEvent::Status {
            epoch: EPOCH,
            state: "disconnected".into(),
            message: None,
        },
        &context,
    )
    .unwrap();
    assert!(app.overleaf_source_locked());
    assert_ne!(
        app.overleaf.connection.as_ref().unwrap().hydration.session,
        prior_session
    );
    app.update_overleaf_event(
        BrowserEvent::Status {
            epoch: EPOCH,
            state: "connected".into(),
            message: None,
        },
        &context,
    )
    .unwrap();
    app.update_overleaf_event(project_event(), &context)
        .unwrap();
    let session = app.overleaf.connection.as_ref().unwrap().hydration.session;
    for (id, path, source) in [
        ("a", "main.tex", "Main old\n"),
        ("b", "chapter.tex", "Chapter old\n"),
    ] {
        app.update_overleaf_event(
            BrowserEvent::Document {
                epoch: EPOCH,
                id: id.into(),
                path: path.into(),
                source: source.into(),
            },
            &context,
        )
        .unwrap();
    }
    let tree = app.overleaf.tree_pending.take().unwrap();
    let mirror = Mirror::open(&root).unwrap();
    mirror.apply_tree(&tree).unwrap();
    let files = app
        .overleaf
        .queued
        .drain(..)
        .map(|work| {
            let MirrorData::Document(source) = &work.data else {
                panic!("only documents arrived");
            };
            let result = mirror.write_document(&work.entry, source);
            (work, result)
        })
        .collect();
    app.apply_overleaf_mirror(
        Mirrored {
            epoch: EPOCH,
            hydration_session: session,
            tree: Some(tree),
            error: None,
            files,
        },
        &context,
    );
    app.finish_overleaf_hydration(&context);
    assert!(
        app.overleaf_source_locked(),
        "old asset completion cannot substitute for this session's download"
    );
    app.update_overleaf_event(
        BrowserEvent::File {
            epoch: EPOCH,
            id: "image".into(),
            path: "image.png".into(),
            base64: "YQ==".into(),
        },
        &context,
    )
    .unwrap();
    let work = app.overleaf.queued.pop_front().unwrap();
    let MirrorData::File(base64) = &work.data else {
        panic!("asset bytes");
    };
    let result = mirror.write_file(&work.entry, base64);
    app.apply_overleaf_mirror(
        Mirrored {
            epoch: EPOCH,
            hydration_session: session,
            tree: None,
            error: None,
            files: vec![(work, result)],
        },
        &context,
    );
    app.finish_overleaf_hydration(&context);
    assert!(!app.overleaf_source_locked());
}

#[test]
fn stale_passive_receipt_cannot_requeue_an_old_baseline_under_a_fresh_remote_name() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().canonicalize().unwrap();
    let context = egui::Context::default();
    let mut app = fixture(&root, &context);
    let tab = app.tabs.active_id().unwrap();
    app.overleaf.tab_entities.insert(tab, "a".into());
    let key = app.document().key();
    let old_session = app.overleaf.connection.as_ref().unwrap().hydration.session;
    let old_work = MirrorWork {
        entry: entry("a", "main.tex", EntryKind::Doc),
        data: MirrorData::Document("Main old\n".into()),
        save: None,
    };
    let old_result = Mirror::open(&root)
        .unwrap()
        .write_document(&old_work.entry, "Main old\n");
    let connection = app.overleaf.connection.as_mut().unwrap();
    connection.online = false;
    connection.hydration.restart();
    app.update_overleaf_event(listing(Some("renamed.tex")), &context)
        .unwrap();
    assert!(app.overleaf.queued.is_empty());
    app.apply_overleaf_mirror(
        Mirrored {
            epoch: EPOCH,
            hydration_session: old_session,
            tree: None,
            error: None,
            files: vec![(old_work, old_result)],
        },
        &context,
    );
    assert!(
        app.overleaf.queued.is_empty(),
        "retargeting an obsolete receipt must not republish its cached baseline as current work"
    );
    assert_eq!(app.document().key(), key);
    assert_eq!(app.document().path().as_ref(), Some(&root.join("main.tex")));
    assert_eq!(app.document().source(), "Main old\n");
    assert!(!root.join("renamed.tex").exists());
    app.finish_overleaf_hydration(&context);
    assert!(app.overleaf_source_locked());
    for (id, path, source) in [
        ("a", "renamed.tex", "Fresh remote version\n"),
        ("b", "chapter.tex", "Chapter old\n"),
    ] {
        app.update_overleaf_event(
            BrowserEvent::Document {
                epoch: EPOCH,
                id: id.into(),
                path: path.into(),
                source: source.into(),
            },
            &context,
        )
        .unwrap();
    }
    let tree = app.overleaf.tree_pending.take().unwrap();
    let session = app.overleaf.connection.as_ref().unwrap().hydration.session;
    let mirror = Mirror::open(&root).unwrap();
    mirror.apply_tree(&tree).unwrap();
    let files = app
        .overleaf
        .queued
        .drain(..)
        .map(|work| {
            let MirrorData::Document(source) = &work.data else {
                panic!("document refresh");
            };
            let result = mirror.write_document(&work.entry, source);
            (work, result)
        })
        .collect();
    app.apply_overleaf_mirror(
        Mirrored {
            epoch: EPOCH,
            hydration_session: session,
            tree: Some(tree),
            error: None,
            files,
        },
        &context,
    );
    app.finish_overleaf_hydration(&context);
    assert_eq!(
        app.document().path().as_ref(),
        Some(&root.join("renamed.tex"))
    );
    assert_eq!(app.document().source(), "Fresh remote version\n");
    assert!(
        !app.overleaf_source_locked(),
        "fresh current-session receipts still complete the normal rename"
    );
}
