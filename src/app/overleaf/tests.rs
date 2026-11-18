//! Controller tests keep the real document/save receipt boundary. They do not
//! replace the browser's network acknowledgement with a local disk write.
use super::*;
use egui_kittest::{Harness, kittest::Queryable as _};

pub(super) const PROJECT: &str = "0123456789abcdef01234567";
pub(super) const EPOCH: u64 = 7;
pub(super) fn entry(id: &str, path: &str, kind: EntryKind) -> Entry {
    Entry {
        id: id.into(),
        path: path.into(),
        kind,
    }
}
pub(super) fn fixture(root: &Path, context: &egui::Context) -> EditorApp {
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
    let mirror = Mirror::open(root).unwrap();
    let previous = crate::overleaf::OfflineStore::load(root, &project, EPOCH).unwrap();
    mirror.sync_tree(&project, model.tree().unwrap()).unwrap();
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
        if let Some(checkpoint) = previous
            .as_ref()
            .and_then(|m| m.document(id))
            .and_then(|d| d.checkpoint)
        {
            model.record_checkpoint(id, checkpoint);
        }
        let receipt = mirror
            .sync_document(
                model.tree().unwrap().entry(id).unwrap(),
                model.document(id).unwrap(),
            )
            .unwrap();
        model.record_checkpoint(id, receipt.checkpoint);
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
        transport: Transport::Online,
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

#[test]
fn git_backup_waits_for_complete_local_copy_but_accepts_saved_offline_drafts() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().canonicalize().unwrap();
    let context = egui::Context::default();
    let mut app = fixture(&root, &context);
    assert!(!app.overleaf_local_copy_pending());
    app.overleaf.connection.as_mut().unwrap().hydration = Hydration::waiting();
    assert!(app.overleaf_local_copy_pending());
    app.overleaf.connection.as_mut().unwrap().hydration = Hydration::default();
    hold_test_save(&mut app, "a");
    assert!(app.overleaf_local_copy_pending());
    clear_test_save(&mut app);
    let tree = app
        .overleaf
        .connection
        .as_ref()
        .unwrap()
        .model
        .tree()
        .unwrap()
        .clone();
    app.overleaf.tree_pending = Some(tree.clone());
    assert!(app.overleaf_local_copy_pending());
    app.overleaf.tree_pending = None;
    let entry = tree.entry("a").unwrap().clone();
    app.overleaf.queued.push_back(MirrorWork {
        checkpoint: true,
        entry: entry.clone(),
        data: MirrorData::Document("Saved locally".into()),
        save: None,
    });
    assert!(app.overleaf_local_copy_pending());
    app.overleaf.queued.clear();
    app.overleaf.offline.checkpoint_waiting.insert("a".into());
    assert!(app.overleaf_local_copy_pending());
    app.overleaf.offline.checkpoint_waiting.clear();
    app.overleaf.downloads.defer(EPOCH, 1, entry).unwrap();
    assert!(app.overleaf_local_copy_pending());
    app.overleaf.downloads.clear();
    let connection = app.overleaf.connection.as_mut().unwrap();
    connection.transport = Transport::Offline;
    connection
        .model
        .set_draft("a", "Saved offline draft".into())
        .unwrap();
    let mut session =
        crate::overleaf::workspace_session::WorkspaceSession::new(&connection.project, [1; 16]);
    session.offline = true;
    app.overleaf.offline.install(session, false);
    app.document_mut().replace_loaded_unprojected(
        "Saved offline draft".into(),
        root.join("main.tex"),
        DocumentKind::Tex,
        None,
    );
    assert!(!app.document().is_dirty());
    assert!(
        app.overleaf
            .connection
            .as_ref()
            .unwrap()
            .model
            .document("a")
            .unwrap()
            .is_dirty()
    );
    assert!(!app.overleaf_local_copy_pending());
    hold_test_save(&mut app, "a");
    app.workspace_root = root.join("another-workspace");
    assert!(
        !app.overleaf_local_copy_pending(),
        "another workspace owns its own backup state"
    );
}

#[test]
fn staging_a_new_overleaf_workspace_opens_login_with_visible_storage_without_a_project_url() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().canonicalize().unwrap();
    let folder = root.join("new-local-workspace");
    let context = egui::Context::default();
    let mut app = EditorApp::dormant_for_tests(&context, root.clone());
    app.document_mut().replace_unprojected_untitled("");
    app.document_mut().edit(CCursorRange::default(), |source| {
        source.push_str("Unsaved local source");
    });
    let key = app.document().key();
    let epoch = app.overleaf.epoch;
    assert!(app.overleaf.browser.is_none());
    assert!(!app.overleaf.browser.creating());

    app.stage_overleaf_workspace(&folder, &context);

    assert!(app.overleaf.visible);
    assert!(app.overleaf.browser.creating());
    assert_eq!(app.overleaf.query, crate::overleaf_browser::LOGIN_URL);
    let draft = app.overleaf.storage_draft.as_ref().unwrap();
    assert!(draft.project_url.is_none());
    assert_eq!(draft.directory, folder.to_string_lossy());
    assert!(draft.keep_local_copy);
    assert!(app.overleaf.storage_controls_visible());
    assert!(app.overleaf.connection.is_none());
    assert!(!app.overleaf.storage_job.is_running());
    assert_eq!(app.overleaf.epoch, epoch);
    assert!(
        !folder.exists(),
        "staging must not prepare the working copy"
    );
    assert_eq!(app.workspace_root, root);
    assert_eq!(app.document().key(), key);
    assert_eq!(app.document().source(), "Unsaved local source");
    assert!(app.document().is_dirty());
    assert!(app.document_workflow.modal().is_none());
    assert!(app.document_workflow.take_action().is_none());
    assert!(app.pending_settings.is_none());
}

#[test]
fn staging_storage_for_another_workspace_preserves_drafts_and_pauses_the_old_account() {
    let directory = tempfile::tempdir().unwrap();
    let base = directory.path().canonicalize().unwrap();
    let project =
        ProjectUrl::parse(&format!("https://www.overleaf.com/project/{PROJECT}")).unwrap();
    let storage = crate::overleaf::StorageLease::prepare(
        &project,
        &crate::overleaf::StorageOptions {
            keep_local_copy: true,
            directory: Some(base.join("current")),
        },
        &base,
    )
    .unwrap();
    let root = storage.root().to_owned();
    let context = egui::Context::default();
    let mut app = fixture(&root, &context);
    app.overleaf.connection.as_mut().unwrap().storage = Some(Arc::clone(&storage));
    app.document_mut().edit(CCursorRange::default(), |source| {
        source.push_str("Unsaved remote draft\n");
    });
    let source = app.document().source().to_owned();
    app.overleaf
        .connection
        .as_mut()
        .unwrap()
        .model
        .set_draft("a", source.clone())
        .unwrap();
    let key = app.document().key();
    let folder = base.join("new-local-workspace");
    // Model the manager's current project browser without creating a native
    // WebView in a deterministic controller test.
    app.overleaf.query = project.url().into();
    app.overleaf.browser.begin();

    app.stage_overleaf_workspace(&folder, &context);

    assert!(app.overleaf.visible);
    assert_eq!(app.overleaf.query, project.url());
    let draft = app.overleaf.storage_draft.as_ref().unwrap();
    assert_eq!(draft.project_url.as_deref(), Some(project.url()));
    assert_eq!(draft.directory, folder.to_string_lossy());
    assert!(draft.keep_local_copy);
    assert!(app.overleaf.storage_changes_pending(Some(&base)));
    let connection = app.overleaf.connection.as_ref().unwrap();
    assert_eq!(connection.project, project);
    assert_eq!(connection.root, root);
    assert!(Arc::ptr_eq(connection.storage.as_ref().unwrap(), &storage));
    assert!(!connection.online());
    assert_eq!(connection.status, "Reconnect required");
    assert!(!connection.hydration.pending());
    assert_eq!(connection.model.epoch(), EPOCH);
    assert_eq!(
        connection.model.document("a").unwrap().draft(),
        Some(source.as_str())
    );
    assert_eq!(app.overleaf.epoch, EPOCH);
    assert!(!app.overleaf.storage_job.is_running());
    assert!(!folder.exists());
    assert_eq!(app.workspace_root, root);
    assert_eq!(app.document().key(), key);
    assert_eq!(app.document().source(), &source);
    assert!(app.document().is_dirty());
    assert!(app.document_workflow.modal().is_none());
    assert!(app.document_workflow.take_action().is_none());
    assert!(app.pending_settings.is_none());
    app.overleaf.reset_storage_draft(Some(&base));
    assert!(app.overleaf.pending_storage.is_none());
    assert!(!app.overleaf.storage_changes_pending(Some(&base)));
    assert!(Arc::ptr_eq(
        app.overleaf
            .connection
            .as_ref()
            .unwrap()
            .storage
            .as_ref()
            .unwrap(),
        &storage,
    ));
}

fn finish_prepared_storage(app: &mut EditorApp, context: &egui::Context) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while app.overleaf.storage_job.is_running() {
        app.poll_overleaf_storage(context);
        assert!(Instant::now() < deadline, "storage worker did not finish");
        std::thread::yield_now();
    }
}

#[test]
fn project_selected_after_workspace_creation_uses_its_folder_before_any_ui_paint() {
    let directory = tempfile::tempdir().unwrap();
    let base = directory.path().canonicalize().unwrap();
    let folder = base.join("new-local-workspace");
    fs::create_dir(&folder).unwrap();
    let context = egui::Context::default();
    let mut app = EditorApp::dormant_for_tests(&context, base.clone());
    let project =
        ProjectUrl::parse(&format!("https://www.overleaf.com/project/{PROJECT}")).unwrap();
    app.stage_overleaf_workspace(&folder, &context);
    assert!(
        app.overleaf
            .storage_draft
            .as_ref()
            .unwrap()
            .project_url
            .is_none()
    );
    app.overleaf.query = project.url().into();
    // Browser discovery and an explicit Connect both enter this controller
    // before show_overleaf can bind a newly selected project to its options.
    app.connect_overleaf(&context).unwrap();
    assert!(app.overleaf.pending_storage.is_some());
    finish_prepared_storage(&mut app, &context);
    let connection = app.overleaf.connection.as_ref().unwrap();
    assert_eq!(connection.project, project);
    assert_eq!(connection.root, project.mirror_root(&folder));
    assert!(connection.storage.as_ref().unwrap().persistent());
    assert!(app.overleaf.pending_storage.is_none());
}

#[test]
fn explicit_managed_copy_reopen_discards_the_next_project_folder_and_keeps_its_exact_root() {
    let directory = tempfile::tempdir().unwrap();
    let base = directory.path().canonicalize().unwrap();
    let project =
        ProjectUrl::parse(&format!("https://www.overleaf.com/project/{PROJECT}")).unwrap();
    let lease = crate::overleaf::StorageLease::prepare(
        &project,
        &crate::overleaf::StorageOptions {
            keep_local_copy: true,
            directory: Some(base.join("existing-copies")),
        },
        &base,
    )
    .unwrap();
    let context = egui::Context::default();
    let mut app = EditorApp::dormant_for_tests(&context, base.clone());
    let next_folder = base.join("next-project");
    app.stage_overleaf_workspace(&next_folder, &context);
    assert!(app.intercept_overleaf_path(lease.root(), &context));
    let deadline = Instant::now() + Duration::from_secs(10);
    while app.overleaf.pending_storage.is_some() {
        app.poll_overleaf_reopen(&context);
        assert!(
            Instant::now() < deadline,
            "descriptor discovery did not finish"
        );
        std::thread::yield_now();
    }
    finish_prepared_storage(&mut app, &context);
    assert_eq!(app.overleaf.connection.as_ref().unwrap().root, lease.root());
    assert!(!next_folder.exists());
}

pub(super) fn park_main(app: &mut EditorApp, root: &Path, context: &egui::Context) -> (u64, u64) {
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
    let operation = begin_test_save(app);
    app.overleaf.save = SavePhase::AwaitingAcknowledgement(RemoteSave {
        operation,
        id: id.into(),
        wire,
        tab,
        key,
        request,
        continuation: None,
        started: Instant::now(),
    });
    wire
}
pub(super) fn apply_work(
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
            origin: MirrorOrigin {
                root: root.to_path_buf(),
                profile: app.overleaf.browser_profile,
            },
            heads: BTreeMap::new(),
            documents: BTreeMap::new(),
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
    assert!(app.overleaf.save.pending().is_none());
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
    assert!(app.document_workflow.save_in_flight());
    assert!(app.overleaf.save.pending().is_none());
    let work = app.overleaf.queued.pop_front().unwrap();
    assert!(work.save.is_some());
    apply_work(&mut app, work, &root, EPOCH, &context);
    assert!(!app.document().is_dirty());
    assert_eq!(app.document().saved_source(), "Saved over Overleaf\n");
    assert_eq!(
        app.document().disk_fingerprint(),
        Some(fingerprint(b"Saved over Overleaf\n"))
    );
    assert!(!app.document_workflow.save_in_flight());
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
    assert!(!app.document_workflow.save_in_flight());
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
    let pending = app.overleaf.save.pending().unwrap();
    assert_eq!(
        (pending.tab, pending.id.as_str(), pending.wire),
        (main, "a", wire)
    );
    assert!(app.overleaf.queued.is_empty());
    assert!(app.document_for_tab(main).unwrap().is_dirty());
    assert!(app.document_for_tab(chapter).unwrap().is_dirty());
}

#[test]
fn browse_dialog_rejects_blank_and_unsafe_typed_addresses_without_connecting() {
    for invalid in [
        "javascript:alert(1)",
        "file:///private/tmp/fixture.tex",
        "https://user:password@www.overleaf.com/login",
    ] {
        let directory = tempfile::tempdir().unwrap();
        let context = egui::Context::default();
        let mut app = EditorApp::dormant_for_tests(&context, directory.path().into());
        app.document_mut()
            .replace_unprojected_untitled("Local document");
        let original = app.document().key();
        // Exercise address validation in the embedded UI without requiring a
        // native WebView. First-open login creation is covered by browsing tests
        // and the real-app connection journey.
        app.overleaf.visible = true;
        let mut harness = Harness::builder()
            .with_size(Vec2::new(1050.0, 760.0))
            .build_ui_state(|ui, app: &mut EditorApp| app.show_overleaf(ui.ctx()), app);
        harness.run();
        harness.get_by_label("Go").click();
        harness.run();
        assert!(harness.state().overleaf.error.is_some());
        assert!(harness.state().overleaf.browser.is_none());
        assert!(harness.state().overleaf.connection.is_none());
        let url_field = egui_kittest::kittest::by()
            .role(egui::accesskit::Role::TextInput)
            .predicate(|node| node.placeholder() == Some(crate::overleaf_browser::LOGIN_URL));
        harness.get(url_field.clone()).click();
        harness.run();
        harness.get(url_field).type_text(invalid);
        harness.run();
        assert_eq!(harness.state().overleaf.query, invalid);
        harness.get_by_label("Go").click();
        harness.run();
        let app = harness.state();
        assert!(app.overleaf.error.is_some(), "{invalid}");
        assert!(!app.overleaf.browser.creating(), "{invalid}");
        assert!(app.overleaf.browser.is_none());
        assert!(app.overleaf.connection.is_none());
        assert!(app.overleaf.events.is_none());
        assert!(!app.overleaf.storage_job.is_running());
        assert_eq!(app.overleaf.epoch, 0);
        assert_eq!(app.document().key(), original);
        assert_eq!(app.document().source(), "Local document");
        assert!(app.overleaf.storage_draft.is_none());
    }
}

#[test]
fn reopened_address_menu_edits_wait_for_the_child_even_after_files_search_focus() {
    let directory = tempfile::tempdir().unwrap();
    let context = egui::Context::default();
    let mut app = EditorApp::dormant_for_tests(&context, directory.path().into());
    app.document_mut()
        .replace_unprojected_untitled("Unchanged local source");
    let original = app.document().key();
    app.overleaf.visible = true;
    app.overleaf.browser.begin();
    app.overleaf.query = "https://www.overleaf.com/project/0123456789abcdef01234567".into();
    let old_url = app.overleaf.query.clone();
    let child = scoped_child_viewport_id(&context, SALT);
    let root_field = egui::Id::new("files-search-before-overleaf");
    let url_field = egui::Id::new("reopened-overleaf-address");
    let raw = |viewport_id, child_focused: bool, events| {
        let mut input = egui::RawInput {
            viewport_id,
            events,
            ..Default::default()
        };
        input
            .viewports
            .entry(egui::ViewportId::ROOT)
            .or_default()
            .focused = Some(!child_focused);
        let child_info = input.viewports.entry(child).or_default();
        child_info.parent = Some(egui::ViewportId::ROOT);
        child_info.focused = Some(child_focused);
        input
    };
    let paint_child = |app: &mut EditorApp, events| {
        context
            .run_ui(raw(child, true, events), |ui| {
                let shortcuts = app.settings.effective_shortcuts();
                let _physical = prepare_child_widget_edit_events(ui.ctx(), &shortcuts, false);
                let _queued = crate::child_view::begin_queued_edit_events(
                    ui.ctx(),
                    &mut app.overleaf.edit_events,
                );
                ui.ctx()
                    .memory_mut(|memory| memory.request_focus(url_field));
                ui.add(egui::TextEdit::singleline(&mut app.overleaf.query).id(url_field));
            })
            .drop_without_applying_deltas();
    };
    let parent_pass = |app: &mut EditorApp, query: &mut String, child_focused| {
        context
            .run_ui(
                raw(egui::ViewportId::ROOT, child_focused, Vec::new()),
                |ui| {
                    ui.ctx()
                        .memory_mut(|memory| memory.request_focus(root_field));
                    app.process_native_menu_commands(ui.ctx(), None);
                    ui.add(egui::TextEdit::singleline(query).id(root_field));
                },
            )
            .drop_without_applying_deltas();
    };
    let mut files_query = "local files".to_owned();
    paint_child(&mut app, Vec::new());
    app.hide_overleaf(&context);
    parent_pass(&mut app, &mut files_query, false);
    app.begin_overleaf(&context);
    paint_child(&mut app, Vec::new());

    app.enqueue_native_menu_command(AppCommand::SelectAll);
    parent_pass(&mut app, &mut files_query, true);
    assert_eq!(app.overleaf.edit_events.len(), 1);
    // Parent paints can precede the independent native child paint. The edit
    // must remain queued rather than enter its obsolete previous InputState.
    parent_pass(&mut app, &mut files_query, true);
    assert_eq!(app.overleaf.edit_events.len(), 1);
    paint_child(&mut app, Vec::new());
    assert!(app.overleaf.edit_events.is_empty());
    let selected = egui::text_edit::TextEditState::load(&context, url_field)
        .unwrap()
        .cursor
        .char_range()
        .unwrap()
        .as_sorted_char_range();
    assert_eq!(selected.start.0..selected.end.0, 0..old_url.chars().count());
    paint_child(
        &mut app,
        vec![egui::Event::Text("http://localhost:".into())],
    );
    parent_pass(&mut app, &mut files_query, true);
    paint_child(&mut app, vec![egui::Event::Text("43210/project".into())]);
    assert_eq!(app.overleaf.query, "http://localhost:43210/project");
    assert_eq!(files_query, "local files");

    // A visible but unfocused connection window must leave root edits local.
    app.enqueue_native_menu_command(AppCommand::SelectAll);
    parent_pass(&mut app, &mut files_query, false);
    assert!(app.overleaf.edit_events.is_empty());
    let selected = egui::text_edit::TextEditState::load(&context, root_field)
        .unwrap()
        .cursor
        .char_range()
        .unwrap()
        .as_sorted_char_range();
    assert_eq!(
        selected.start.0..selected.end.0,
        0..files_query.chars().count()
    );
    assert_eq!(app.overleaf.query, "http://localhost:43210/project");
    assert_eq!(app.document().key(), original);
    assert_eq!(app.document().source(), "Unchanged local source");
}

#[test]
fn browser_navigation_is_disabled_for_clicks_and_enter_while_storage_is_preparing() {
    use egui_kittest::kittest::NodeT as _;
    let directory = tempfile::tempdir().unwrap();
    let context = egui::Context::default();
    let mut app = EditorApp::dormant_for_tests(&context, directory.path().into());
    app.document_mut()
        .replace_unprojected_untitled("Unchanged local source");
    let original = app.document().key();
    app.overleaf.visible = true;
    app.overleaf.query = "http://localhost:43210/login".into();
    app.overleaf.browser_url = "https://pending.invalid/project/0123456789abcdef01234567".into();
    let expected_request = app.overleaf.browser_url.clone();
    let (release, waiting) = std::sync::mpsc::channel();
    app.overleaf
        .storage_job
        .start_and_repaint("Held preparation", &context, move || {
            let _ = waiting.recv();
            Err("Fixture preparation stopped".into())
        })
        .unwrap();
    let mut harness = Harness::builder()
        .with_size(Vec2::new(1050.0, 760.0))
        .build_ui_state(|ui, app: &mut EditorApp| app.show_overleaf(ui.ctx()), app);
    harness.run();
    for label in ["Preparing…", "‹", "›"] {
        assert!(harness.get_by_label(label).accesskit_node().is_disabled());
        harness.get_by_label(label).click();
        harness.run();
        assert_eq!(harness.state().overleaf.browser_url, expected_request);
    }
    let url_field = egui_kittest::kittest::by()
        .role(egui::accesskit::Role::TextInput)
        .value("http://localhost:43210/login");
    harness.get(url_field).click();
    harness.run();
    assert!(harness.state().overleaf.url_editing);
    harness.key_press(egui::Key::Enter);
    harness.run();
    let app = harness.state();
    assert!(
        !app.overleaf.url_editing,
        "Enter reached the address field and completed editing"
    );
    assert_eq!(
        app.overleaf.browser_url, expected_request,
        "Enter must not bypass the disabled navigation button"
    );
    assert!(!app.overleaf.browser.creating());
    assert!(app.overleaf.connection.is_none());
    assert!(app.overleaf.events.is_none());
    assert!(app.overleaf.storage_job.is_running());
    assert_eq!(app.overleaf.epoch, 0);
    assert_eq!(app.document().key(), original);
    assert_eq!(app.document().source(), "Unchanged local source");
    release.send(()).unwrap();
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

/// Complete the actual journal/source worker effects while holding its UI receipt.
pub(super) fn committed_document_reply(app: &EditorApp, root: &Path, work: MirrorWork) -> Mirrored {
    let connection = app.overleaf.connection.as_ref().unwrap();
    let mut document = connection.model.document(&work.entry.id).unwrap().clone();
    let previous = document.checkpoint;
    let mirror = Mirror::open(root).unwrap();
    mirror
        .prepare_unbased_document(&work.entry, &mut document)
        .unwrap();
    let receipt = mirror.sync_document(&work.entry, &document).unwrap();
    document.checkpoint = Some(receipt.checkpoint);
    let heads = BTreeMap::from([(
        work.entry.id.clone(),
        CheckpointAdvance {
            previous,
            written: receipt.checkpoint,
        },
    )]);
    Mirrored {
        epoch: app.overleaf.epoch,
        origin: MirrorOrigin {
            root: root.to_owned(),
            profile: app.overleaf.mirror_profile(),
        },
        hydration_session: connection.hydration.session,
        tree: None,
        error: None,
        documents: BTreeMap::from([(work.entry.id.clone(), document)]),
        heads,
        files: vec![(work, Ok(receipt.path))],
    }
}

#[test]
fn superseded_empty_duplicate_retains_its_committed_head_without_becoming_fresh() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().canonicalize().unwrap();
    let context = egui::Context::default();
    let mut app = fixture(&root, &context);
    let copy = entry("copy", "notes/copy.tex", EntryKind::Doc);
    app.update_overleaf_event(
        BrowserEvent::Project {
            epoch: EPOCH,
            project_id: PROJECT.into(),
            name: "Paper".into(),
            entries: vec![
                entry("a", "main.tex", EntryKind::Doc),
                entry("b", "chapter.tex", EntryKind::Doc),
                copy.clone(),
            ],
            root_doc: Some("a".into()),
        },
        &context,
    )
    .unwrap();
    let tree = app.overleaf.tree_pending.take().unwrap();
    Mirror::open(&root)
        .unwrap()
        .sync_tree(&app.overleaf.connection.as_ref().unwrap().project, &tree)
        .unwrap();
    let hydration = &mut app.overleaf.connection.as_mut().unwrap().hydration;
    hydration.accept_tree(&tree);
    for id in ["a", "b"] {
        hydration.accept_file(tree.entry(id).unwrap());
    }
    let snapshot = |source: &str| BrowserEvent::Document {
        epoch: EPOCH,
        id: copy.id.clone(),
        path: copy.path.clone(),
        source: source.into(),
    };
    app.update_overleaf_event(snapshot(""), &context).unwrap();
    let old_work = app.overleaf.queued.pop_front().unwrap();
    let old_reply = committed_document_reply(&app, &root, old_work);
    let committed = old_reply.heads[&copy.id].written;
    app.update_overleaf_event(snapshot("Copied real source\n"), &context)
        .unwrap();

    // A crash here restores the actually committed empty baseline as cached,
    // never the later browser source paired with the wrong journal.
    let mut restored = crate::overleaf::OfflineStore::load(
        &root,
        &app.overleaf.connection.as_ref().unwrap().project,
        EPOCH + 1,
    )
    .unwrap()
    .unwrap();
    assert_eq!(restored.document(&copy.id).unwrap().source(), "");
    assert!(restored.publish(&copy.id).is_none());
    app.apply_overleaf_mirror(old_reply, &context);
    assert_eq!(
        app.overleaf
            .connection
            .as_ref()
            .unwrap()
            .model
            .document(&copy.id)
            .unwrap()
            .checkpoint,
        Some(committed)
    );
    assert_eq!(
        app.overleaf
            .connection
            .as_ref()
            .unwrap()
            .model
            .document(&copy.id)
            .unwrap()
            .baseline,
        "Copied real source\n"
    );
    app.finish_overleaf_hydration(&context);
    assert!(
        app.overleaf_source_locked(),
        "old source cannot satisfy the current file barrier"
    );
    assert_eq!(app.document().source(), "Main old\n");
    let current_work = app.overleaf.queued.pop_front().unwrap();
    let current_reply = committed_document_reply(&app, &root, current_work);
    app.apply_overleaf_mirror(current_reply, &context);
    app.finish_overleaf_hydration(&context);
    assert!(!app.overleaf_source_locked());
    assert!(app.overleaf.error.is_none(), "{:?}", app.overleaf.error);
    assert_eq!(
        fs::read_to_string(root.join(&copy.path)).unwrap(),
        "Copied real source\n"
    );
}

#[test]
fn same_session_stale_renamed_receipt_cannot_replace_fresh_queued_source() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().canonicalize().unwrap();
    let context = egui::Context::default();
    let mut app = fixture(&root, &context);
    let tab = app.tabs.active_id().unwrap();
    app.overleaf.tab_entities.insert(tab, "a".into());
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
    let mut old_reply = committed_document_reply(&app, &root, old_work);
    old_reply.files[0].0.checkpoint = true;
    let session = old_reply.hydration_session;
    app.update_overleaf_event(listing(Some("renamed.tex")), &context)
        .unwrap();
    app.update_overleaf_event(
        BrowserEvent::Document {
            epoch: EPOCH,
            id: "a".into(),
            path: "renamed.tex".into(),
            source: "Server latest\n".into(),
        },
        &context,
    )
    .unwrap();
    assert_eq!(
        app.overleaf.connection.as_ref().unwrap().hydration.session,
        session
    );
    app.apply_overleaf_mirror(old_reply, &context);
    let current = app
        .overleaf
        .queued
        .iter()
        .find(|work| work.entry.id == "a")
        .unwrap();
    assert_eq!(current.entry.path, "renamed.tex");
    let MirrorData::Document(source) = &current.data else {
        panic!("document work");
    };
    assert_eq!(source, "Server latest\n");
    assert!(
        current.checkpoint,
        "fresh work inherits the outstanding checkpoint intent"
    );
    assert_eq!(app.document().source(), "Main old\n");
    assert!(app.overleaf_source_locked());
    let tree = app.overleaf.tree_pending.take().unwrap();
    Mirror::open(&root)
        .unwrap()
        .sync_tree(&app.overleaf.connection.as_ref().unwrap().project, &tree)
        .unwrap();
    app.overleaf
        .connection
        .as_mut()
        .unwrap()
        .hydration
        .accept_tree(&tree);
    let work = app.overleaf.queued.pop_front().unwrap();
    let reply = committed_document_reply(&app, &root, work);
    app.apply_overleaf_mirror(reply, &context);
    let reply = committed_document_reply(
        &app,
        &root,
        MirrorWork {
            checkpoint: false,
            entry: tree.entry("b").unwrap().clone(),
            data: MirrorData::Document("Chapter old\n".into()),
            save: None,
        },
    );
    app.apply_overleaf_mirror(reply, &context);
    app.finish_overleaf_hydration(&context);
    assert_eq!(app.document().source(), "Server latest\n");
    assert_eq!(
        app.document().path().as_ref(),
        Some(&root.join("renamed.tex"))
    );
    assert!(!app.overleaf_source_locked());
    assert!(app.overleaf.error.is_none(), "{:?}", app.overleaf.error);
}

#[test]
fn fresh_old_path_work_retargets_once_and_finishes_the_renamed_document() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().canonicalize().unwrap();
    let context = egui::Context::default();
    let mut app = fixture(&root, &context);
    let original = entry("copy", "notes/copy.tex", EntryKind::Doc);
    let renamed = entry("copy", "notes/renamed.txt", EntryKind::Doc);
    let listing = |copy: Entry| BrowserEvent::Project {
        epoch: EPOCH,
        project_id: PROJECT.into(),
        name: "Paper".into(),
        root_doc: Some("a".into()),
        entries: vec![
            entry("a", "main.tex", EntryKind::Doc),
            entry("b", "chapter.tex", EntryKind::Doc),
            copy,
        ],
    };
    let snapshot = |entry: &Entry, source: &str| BrowserEvent::Document {
        epoch: EPOCH,
        id: entry.id.clone(),
        path: entry.path.clone(),
        source: source.into(),
    };
    app.update_overleaf_event(listing(original.clone()), &context)
        .unwrap();
    let mirror = Mirror::open(&root).unwrap();
    let tree = app.overleaf.tree_pending.take().unwrap();
    mirror
        .sync_tree(&app.overleaf.connection.as_ref().unwrap().project, &tree)
        .unwrap();
    let hydration = &mut app.overleaf.connection.as_mut().unwrap().hydration;
    hydration.accept_tree(&tree);
    for id in ["a", "b"] {
        hydration.accept_file(tree.entry(id).unwrap());
    }
    app.update_overleaf_event(snapshot(&original, "Server first\n"), &context)
        .unwrap();
    let work = app.overleaf.queued.pop_front().unwrap();
    let old_reply = committed_document_reply(&app, &root, work);
    app.update_overleaf_event(snapshot(&original, "Server latest\n"), &context)
        .unwrap();
    app.update_overleaf_event(listing(renamed.clone()), &context)
        .unwrap();
    app.apply_overleaf_mirror(old_reply, &context);
    let fresh_old_path = app.overleaf.queued.pop_front().unwrap();
    assert_eq!(fresh_old_path.entry, original);
    let MirrorData::Document(source) = &fresh_old_path.data else {
        panic!("document work");
    };
    assert_eq!(source, "Server latest\n");

    let tree = app.overleaf.tree_pending.take().unwrap();
    mirror
        .sync_tree(&app.overleaf.connection.as_ref().unwrap().project, &tree)
        .unwrap();
    app.overleaf
        .connection
        .as_mut()
        .unwrap()
        .hydration
        .accept_tree(&tree);
    let reply = committed_document_reply(&app, &root, fresh_old_path);
    app.apply_overleaf_mirror(reply, &context);
    app.finish_overleaf_hydration(&context);
    assert!(
        app.overleaf_source_locked(),
        "old-path write cannot satisfy the renamed entry"
    );
    let current = app.overleaf.queued.pop_front().unwrap();
    assert_eq!(current.entry, renamed);
    let reply = committed_document_reply(&app, &root, current);
    app.apply_overleaf_mirror(reply, &context);
    app.finish_overleaf_hydration(&context);
    assert!(!app.overleaf_source_locked());
    assert!(app.overleaf.queued.is_empty());
    assert!(app.overleaf.error.is_none(), "{:?}", app.overleaf.error);
    assert_eq!(
        fs::read_to_string(root.join(&renamed.path)).unwrap(),
        "Server latest\n"
    );
    assert!(!root.join(&original.path).exists());
    assert_eq!(app.document().source(), "Main old\n");
}

#[test]
fn stale_checkpoint_receipt_retains_only_its_identity_while_fresh_source_is_deferred() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().canonicalize().unwrap();
    let context = egui::Context::default();
    let mut app = fixture(&root, &context);
    let snapshot = |source: &str| BrowserEvent::Document {
        epoch: EPOCH,
        id: "a".into(),
        path: "main.tex".into(),
        source: source.into(),
    };
    app.update_overleaf_event(snapshot("Server first\n"), &context)
        .unwrap();
    let work = app.overleaf.queued.pop_front().unwrap();
    let mut reply = committed_document_reply(&app, &root, work);
    reply.files[0].0.checkpoint = true;
    app.update_overleaf_event(snapshot("Server latest\n"), &context)
        .unwrap();
    let fresh = app.overleaf.queued.pop_front().unwrap();
    app.overleaf
        .downloads
        .defer(EPOCH, reply.hydration_session, fresh.entry)
        .unwrap();
    app.apply_overleaf_mirror(reply, &context);
    assert!(app.overleaf.queued.is_empty());
    assert!(app.overleaf.offline.checkpoint_waiting.contains("a"));
    assert!(app.overleaf.downloads.has_pending());
    assert_eq!(app.document().source(), "Main old\n");
}

#[test]
fn renamed_or_previous_hydration_receipts_advance_disk_heads_without_installing_source() {
    for rename in [false, true] {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().canonicalize().unwrap();
        let context = egui::Context::default();
        let mut app = fixture(&root, &context);
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
        let work = app.overleaf.queued.pop_front().unwrap();
        let reply = committed_document_reply(&app, &root, work);
        let committed = reply.heads["a"].written;
        if rename {
            app.update_overleaf_event(listing(Some("renamed.tex")), &context)
                .unwrap();
        } else {
            let connection = app.overleaf.connection.as_mut().unwrap();
            let tree = connection.model.tree().unwrap().clone();
            connection.hydration.restart();
            connection.hydration.reset(&tree);
        }
        app.apply_overleaf_mirror(reply, &context);
        let connection = app.overleaf.connection.as_ref().unwrap();
        assert_eq!(
            connection.model.document("a").unwrap().checkpoint,
            Some(committed)
        );
        assert_eq!(app.document().source(), "Main old\n");
        assert!(app.overleaf_source_locked());
        let current = connection.model.tree().unwrap().entry("a").unwrap().clone();
        // The subsequent exact-identity worker still passes the original CAS.
        let _reply = committed_document_reply(
            &app,
            &root,
            MirrorWork {
                checkpoint: false,
                entry: current,
                data: MirrorData::Document("Server first\n".into()),
                save: None,
            },
        );
    }
}

#[test]
fn mirror_head_receipts_are_fenced_to_their_workspace_profile_and_epoch() {
    for wrong in 0..3 {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().canonicalize().unwrap();
        let context = egui::Context::default();
        let mut app = fixture(&root, &context);
        let previous = app
            .overleaf
            .connection
            .as_ref()
            .unwrap()
            .model
            .document("a")
            .unwrap()
            .checkpoint;
        let mut reply = Mirrored {
            epoch: EPOCH,
            origin: MirrorOrigin {
                root: root.clone(),
                profile: app.overleaf.browser_profile,
            },
            hydration_session: 0,
            tree: None,
            error: None,
            files: Vec::new(),
            documents: BTreeMap::new(),
            heads: BTreeMap::from([(
                "a".into(),
                CheckpointAdvance {
                    previous,
                    written: [9; 32],
                },
            )]),
        };
        match wrong {
            0 => reply.epoch += 1,
            1 => reply.origin.root = root.join("another-workspace"),
            _ => reply.origin.profile = Some([5; 16]),
        }
        app.apply_overleaf_mirror(reply, &context);
        assert_eq!(
            app.overleaf
                .connection
                .as_ref()
                .unwrap()
                .model
                .document("a")
                .unwrap()
                .checkpoint,
            previous
        );
    }
}

#[test]
fn checkpoint_batches_keep_the_first_predecessor_and_cannot_roll_back_later_heads() {
    let mut heads = BTreeMap::new();
    CheckpointAdvance::record(&mut heads, "a", Some([1; 32]), [2; 32]);
    CheckpointAdvance::record(&mut heads, "a", Some([2; 32]), [3; 32]);
    CheckpointAdvance::record(&mut heads, "b", None, [4; 32]);
    assert_eq!(heads["a"].previous, Some([1; 32]));
    assert_eq!(heads["a"].written, [3; 32]);
    assert_eq!(heads["b"].previous, None);
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().canonicalize().unwrap();
    let context = egui::Context::default();
    let mut app = fixture(&root, &context);
    let model = &mut app.overleaf.connection.as_mut().unwrap().model;
    model.record_checkpoint("a", [1; 32]);
    assert!(model.advance_checkpoint("a", heads["a"].previous, heads["a"].written));
    assert!(!model.advance_checkpoint("a", Some([1; 32]), [2; 32]));
    assert!(model.advance_checkpoint("a", Some([3; 32]), [5; 32]));
    assert!(!model.advance_checkpoint("a", Some([1; 32]), [3; 32]));
    assert!(!model.advance_checkpoint("missing", None, [6; 32]));
    assert_eq!(model.document("a").unwrap().checkpoint, Some([5; 32]));
    assert_eq!(model.document("a").unwrap().source(), "Main old\n");
}

#[test]
fn a_later_batch_file_failure_cannot_discard_an_earlier_committed_head() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().canonicalize().unwrap();
    let context = egui::Context::default();
    let mut app = fixture(&root, &context);
    app.update_overleaf_event(
        BrowserEvent::Document {
            epoch: EPOCH,
            id: "a".into(),
            path: "main.tex".into(),
            source: "Remote latest\n".into(),
        },
        &context,
    )
    .unwrap();
    let work = app.overleaf.queued.pop_front().unwrap();
    let mut reply = committed_document_reply(&app, &root, work);
    let written = reply.heads["a"].written;
    reply.files.push((
        MirrorWork {
            checkpoint: false,
            entry: entry("b", "chapter.tex", EntryKind::Doc),
            data: MirrorData::Document("Chapter old\n".into()),
            save: None,
        },
        Err("A later file could not be written".into()),
    ));
    app.apply_overleaf_mirror(reply, &context);
    assert_eq!(
        app.overleaf
            .connection
            .as_ref()
            .unwrap()
            .model
            .document("a")
            .unwrap()
            .checkpoint,
        Some(written)
    );
    assert!(app.overleaf.blocked.contains("b"));
    assert!(
        app.overleaf
            .error
            .as_deref()
            .unwrap()
            .contains("A later file")
    );
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
                origin: MirrorOrigin {
                    root: root.to_path_buf(),
                    profile: app.overleaf.browser_profile,
                },
                heads: BTreeMap::new(),
                documents: BTreeMap::new(),
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
            origin: MirrorOrigin {
                root: root.to_path_buf(),
                profile: app.overleaf.browser_profile,
            },
            heads: BTreeMap::new(),
            documents: BTreeMap::new(),
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
    assert!(!app.document_workflow.save_in_flight());
    assert!(app.overleaf.save.pending().is_none());
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
    assert!(app.overleaf.save.pending().is_none());
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
    assert!(!app.document_workflow.save_in_flight());
    assert!(app.overleaf.save.pending().is_none());
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
    assert!(app.overleaf.save.pending().is_none());
    assert!(app.document_workflow.save_in_flight());
    let result = Mirror::open(&root)
        .unwrap()
        .write_document(&work.entry, "Old connection acknowledged\n");
    app.overleaf.query = format!("https://www.overleaf.com/project/{PROJECT}");
    app.connect_overleaf(&context).unwrap();
    assert_eq!(app.overleaf.epoch, EPOCH + 1);
    assert!(app.overleaf.save.pending().is_none());
    assert!(
        !app.document_workflow.save_in_flight(),
        "an old connection's ACK-cache transaction cannot block the new connection"
    );
    assert!(
        app.overleaf
            .connection
            .as_mut()
            .unwrap()
            .model
            .publish("a")
            .is_none(),
        "a new connection must verify remote contents before accepting another publish"
    );
    // The old ACK remains authoritative, but its delayed mirror receipt cannot
    // hydrate the new session. Deliver a fresh listing and snapshot, then its
    // own working-copy receipt, before starting the next save transaction.
    app.update_overleaf_event(
        BrowserEvent::Project {
            epoch: EPOCH + 1,
            project_id: PROJECT.into(),
            name: "Remote paper".into(),
            entries: vec![entry("a", "main.tex", EntryKind::Doc)],
            root_doc: Some("a".into()),
        },
        &context,
    )
    .unwrap();
    app.update_overleaf_event(
        BrowserEvent::Document {
            epoch: EPOCH + 1,
            id: "a".into(),
            path: "main.tex".into(),
            source: "Old connection acknowledged\n".into(),
        },
        &context,
    )
    .unwrap();
    let hydration_session = app.overleaf.connection.as_ref().unwrap().hydration.session;
    let fresh_tree = app.overleaf.tree_pending.take().unwrap();
    let mirror = Mirror::open(&root).unwrap();
    mirror.apply_tree(&fresh_tree).unwrap();
    let fresh_work = app.overleaf.queued.pop_front().unwrap();
    let fresh_result = mirror.write_document(&fresh_work.entry, "Old connection acknowledged\n");
    app.apply_overleaf_mirror(
        Mirrored {
            origin: MirrorOrigin {
                root: root.to_path_buf(),
                profile: app.overleaf.browser_profile,
            },
            heads: BTreeMap::new(),
            documents: BTreeMap::new(),
            hydration_session,
            epoch: EPOCH + 1,
            tree: Some(fresh_tree),
            error: None,
            files: vec![(fresh_work, fresh_result)],
        },
        &context,
    );
    app.finish_overleaf_hydration(&context);
    assert!(!app.overleaf_source_locked());
    assert!(!app.document().is_dirty());
    assert_eq!(
        app.document().saved_source(),
        "Old connection acknowledged\n"
    );
    let next_wire = pending_save(&mut app, tab, "a", "New connection draft\n");
    assert_ne!(next_wire, wire);
    app.apply_overleaf_mirror(
        Mirrored {
            origin: MirrorOrigin {
                root: root.to_path_buf(),
                profile: app.overleaf.browser_profile,
            },
            heads: BTreeMap::new(),
            documents: BTreeMap::new(),
            hydration_session: 0,
            epoch: EPOCH,
            tree: None,
            error: None,
            files: vec![(work, result)],
        },
        &context,
    );
    assert!(app.document_workflow.save_in_flight());
    assert_eq!(app.overleaf.save.pending().unwrap().wire, next_wire);
    assert_eq!(app.document().source(), "New connection draft\n");
    assert_eq!(
        app.document().saved_source(),
        "Old connection acknowledged\n"
    );
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
    assert!(app.overleaf.connection.as_ref().unwrap().online());
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
    app.overleaf.await_project();
    app.overleaf.visible = true;
    app.overleaf.show_browser = true;
    app.update_overleaf_event(listing(Some("main.tex")), &context)
        .unwrap();
    assert_eq!(app.workspace_root, root);
    assert!(!app.overleaf.awaiting_project());
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
fn browser_history_and_address_clear_native_window_controls_at_every_ui_scale() {
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
                        node.placeholder() == Some(crate::overleaf_browser::LOGIN_URL)
                    }),
            )
            .rect();
        let back = harness.get_by_label("‹").rect();
        let forward = harness.get_by_label("›").rect();
        let go = harness.get_by_label("Go").rect();
        let native_inset = (back.left() - window.left()) * zoom;
        assert!(
            native_inset >= 80.0,
            "Browser history overlaps native controls at zoom {zoom}: inset {native_inset}"
        );
        for (previous, next) in [(back, forward), (forward, url), (url, go)] {
            assert!(
                previous.right() <= next.left() && next.right() <= window.right(),
                "Browser controls overlap or clip at zoom {zoom}: {previous:?} then {next:?}"
            );
        }
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
    assert!(!app.local_save.is_running());
    assert!(app.document_workflow.save_in_flight());
    app.tick_autosave(&context);
    assert_eq!(app.active_autosave_deadline(), Some(deadline));
    assert_eq!(app.tabs.active_id(), Some(local));
    assert_eq!(app.document().key(), key);
    assert_eq!(app.document().source(), "Local unsaved\n");
    assert!(app.document().is_dirty());
    assert!(!app.local_save.is_running());
    let pending = app.overleaf.save.pending().unwrap();
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
    assert_eq!(app.overleaf_explorer().unwrap().1, "Remote paper");
    assert_eq!(app.overleaf_explorer().unwrap().2.text, "Unsynced");
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
    assert!(remote.conflict().is_none());
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
    assert!(remote.conflict().is_none());
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

pub(super) fn presence_event(source: &str, epoch: u64) -> BrowserEvent {
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
    app.overleaf.connection.as_mut().unwrap().transport = Transport::Offline;
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
        app.overleaf.connection.as_mut().unwrap().transport = Transport::Offline;
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
    app.overleaf.connection.as_mut().unwrap().transport = Transport::Offline;
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
    app.overleaf.connection.as_mut().unwrap().transport = Transport::Offline;
    let key = app.document().key();
    let source = app.document().source().clone();
    let token = app.formatting.request(key, formatting::Intent::Save);
    let uri = url::Url::from_file_path(app.document().path().as_ref().unwrap())
        .unwrap()
        .to_string();
    app.tex_service.synchronize(
        crate::tex::Snapshot {
            generation: Generation(0),
            key,
            uri: uri.clone(),
            source: Arc::from(source.as_str()),
            root: root.clone(),
            tools: app.tex_tools.clone(),
            settings: crate::tex::settings::TexSettings {
                texlab_enabled: false,
                lint: false,
                formatter: crate::tex::settings::Formatter::Disabled,
                ..Default::default()
            },
        },
        crate::worker::RepaintTarget::test(),
    );
    let generation = app.tex_service.identity().unwrap().generation;
    app.receive_formatted_document(
        &context,
        generation,
        "file:///ignored",
        0,
        token,
        Vec::new(),
    );
    assert_eq!(app.formatting.request_token(), Some(token));
    app.receive_formatted_document(
        &context,
        generation,
        &uri,
        revision_as_i32(key.revision),
        token,
        vec![LspTextEdit {
            range: tiptoptyp_core::text::LspRange {
                start: tiptoptyp_core::text::LspPosition::new(0, 0),
                end: tiptoptyp_core::text::LspPosition::new(0, 4),
            },
            new_text: "Changed".into(),
        }],
    );
    assert!(app.formatting.request_key().is_none());
    assert!(!app.formatting.saves_after_reply());
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
    app.overleaf.connection.as_mut().unwrap().transport = Transport::Offline;
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
        connection.transport = Transport::Offline;
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
                        checkpoint: false,
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
            origin: MirrorOrigin {
                root: root.to_path_buf(),
                profile: app.overleaf.browser_profile,
            },
            heads: BTreeMap::new(),
            documents: BTreeMap::new(),
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
            origin: MirrorOrigin {
                root: root.to_path_buf(),
                profile: app.overleaf.browser_profile,
            },
            heads: BTreeMap::new(),
            documents: BTreeMap::new(),
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
                    checkpoint: false,
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
            origin: MirrorOrigin {
                root: root.to_path_buf(),
                profile: app.overleaf.browser_profile,
            },
            heads: BTreeMap::new(),
            documents: BTreeMap::new(),
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
                assert!(app.overleaf.save.pending().is_none());
                assert!(app.overleaf.save.active());
            }
        }
        app.overleaf.queue(MirrorWork {
            checkpoint: false,
            entry: entry("b", "chapter.tex", EntryKind::Doc),
            data: MirrorData::Document("Chapter old\n".into()),
            save: None,
        });
        let key = app.document().key();
        let source = app.document().source().clone();
        let pending = app.overleaf.save.pending().map(|save| save.wire);
        let save_active = app.overleaf.save.active();
        let save_in_flight = app.document_workflow.save_in_flight();
        let queued: Vec<_> = app
            .overleaf
            .queued
            .iter()
            .map(|work| (work.entry.clone(), work.save.is_some()))
            .collect();
        let connection = app.overleaf.connection.as_mut().unwrap();
        connection.hydration.session = 17;
        let status = connection.status.clone();
        assert!(connection.online());
        assert!(!connection.hydration.pending());
        // There is deliberately no browser. The last case exercises failed
        // dispatch, while the first two must refuse even to attempt refresh.
        app.refresh_overleaf();
        let connection = app.overleaf.connection.as_ref().unwrap();
        assert!(connection.online(), "phase {phase}");
        assert!(!connection.hydration.pending(), "phase {phase}");
        assert_eq!(connection.hydration.session, 17);
        assert_eq!(connection.status, status);
        assert_eq!(app.overleaf.save.pending().map(|save| save.wire), pending);
        assert_eq!(app.overleaf.save.active(), save_active);
        assert_eq!(app.document_workflow.save_in_flight(), save_in_flight);
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
        checkpoint: false,
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
        app.overleaf.connection.as_mut().unwrap().transport = Transport::Offline;
        app.overleaf.await_project();
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
    app.overleaf.connection.as_mut().unwrap().transport = Transport::Offline;
    app.overleaf.await_project();
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
fn applying_storage_uses_pending_preferences_and_persists_same_storage_draft_changes() {
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
        let (completed, completion) = std::sync::mpsc::channel();
        context.set_request_repaint_callback(move |_| {
            let _ = completed.send(());
        });
        app.overleaf
            .storage_job
            .start_and_repaint(
                "Earlier failed Overleaf storage preparation",
                &context,
                || Err("Old storage preparation failed".into()),
            )
            .unwrap();
        completion.recv_timeout(Duration::from_secs(5)).unwrap();
        assert!(matches!(
            app.overleaf.storage_job.poll(),
            LatestJobPoll::Failed(_)
        ));
        app.overleaf.error = Some("Old storage preparation failed".into());
        assert_eq!(
            app.overleaf_connection_activity(),
            crate::activity::Activity::Failed("Old storage preparation failed".into())
        );
        app.apply_overleaf_storage(&context).unwrap();
        assert_eq!(
            app.overleaf.storage_job.activity(),
            crate::activity::Activity::Idle
        );
        assert_eq!(
            app.overleaf_connection_activity(),
            crate::activity::Activity::Running
        );
        assert_eq!(
            app.overleaf_sync_activity(),
            crate::activity::Activity::Running
        );
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
    connection.hydration.finish();
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
            origin: MirrorOrigin {
                root: root.to_path_buf(),
                profile: app.overleaf.browser_profile,
            },
            heads: BTreeMap::new(),
            documents: BTreeMap::new(),
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
            origin: MirrorOrigin {
                root: root.to_path_buf(),
                profile: app.overleaf.browser_profile,
            },
            heads: BTreeMap::new(),
            documents: BTreeMap::new(),
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
        checkpoint: false,
        entry: entry("a", "main.tex", EntryKind::Doc),
        data: MirrorData::Document("Main old\n".into()),
        save: None,
    };
    let old_result = Mirror::open(&root)
        .unwrap()
        .write_document(&old_work.entry, "Main old\n");
    let connection = app.overleaf.connection.as_mut().unwrap();
    connection.transport = Transport::Offline;
    connection.hydration.restart();
    app.update_overleaf_event(listing(Some("renamed.tex")), &context)
        .unwrap();
    assert!(app.overleaf.queued.is_empty());
    app.apply_overleaf_mirror(
        Mirrored {
            origin: MirrorOrigin {
                root: root.to_path_buf(),
                profile: app.overleaf.browser_profile,
            },
            heads: BTreeMap::new(),
            documents: BTreeMap::new(),
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
            origin: MirrorOrigin {
                root: root.to_path_buf(),
                profile: app.overleaf.browser_profile,
            },
            heads: BTreeMap::new(),
            documents: BTreeMap::new(),
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

mod activity_snapshots {
    use super::*;
    use crate::activity::Activity;

    fn status(app: &mut EditorApp, context: &egui::Context, state: &str, message: Option<&str>) {
        app.update_overleaf_event(
            BrowserEvent::Status {
                epoch: EPOCH,
                state: state.into(),
                message: message.map(str::to_owned),
            },
            context,
        )
        .unwrap();
    }

    #[test]
    fn dormant_snapshots_are_read_only_and_remote_save_is_not_local_autosave() {
        let context = egui::Context::default();
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().canonicalize().unwrap();
        let dormant = EditorApp::dormant_for_tests(&context, root.clone());
        for _ in 0..3 {
            assert_eq!(
                dormant.overleaf_connection_activity(),
                Activity::Inactive("Not connected")
            );
            assert_eq!(
                dormant.overleaf_sync_activity(),
                Activity::Inactive("Not connected")
            );
            assert_eq!(
                dormant.overleaf_save_activity(),
                Activity::Inactive("Not connected")
            );
            assert_eq!(
                dormant.overleaf_save_for_active(&dormant.overleaf_save_activity()),
                None
            );
        }
        assert!(!dormant.overleaf.browser.creating());
        assert!(!dormant.overleaf.storage_job.is_running());
        assert!(!dormant.overleaf.io.is_running());
        assert!(dormant.overleaf.queued.is_empty());
        let mut app = fixture(&root, &context);
        app.settings.auto_save = false;
        let key = app.document().key();
        assert_eq!(app.overleaf_connection_activity(), Activity::Idle);
        assert_eq!(app.overleaf_sync_activity(), Activity::Idle);
        assert_eq!(
            app.overleaf_save_for_active(&app.overleaf_save_activity()),
            Some(Activity::Idle)
        );
        assert_eq!(app.document().key(), key);
        assert!(app.overleaf.save.pending().is_none());
    }

    #[test]
    fn browser_state_distinguishes_sign_in_offline_failure_and_advisory() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().canonicalize().unwrap();
        let context = egui::Context::default();
        let mut app = fixture(&root, &context);
        status(&mut app, &context, "loading", None);
        assert_eq!(app.overleaf_connection_activity(), Activity::Running);
        assert_eq!(app.overleaf_sync_activity(), Activity::Running);
        status(&mut app, &context, "signed-out", Some("Please sign in"));
        assert_eq!(
            app.overleaf_connection_activity(),
            Activity::Inactive("Sign in required")
        );
        assert_eq!(
            app.overleaf_sync_activity(),
            Activity::Inactive("Sign in required")
        );
        status(&mut app, &context, "disconnected", None);
        assert_eq!(
            app.overleaf_connection_activity(),
            Activity::Inactive("Disconnected")
        );
        status(&mut app, &context, "error", Some("Permission revoked"));
        assert_eq!(
            app.overleaf_connection_activity(),
            Activity::Failed("Permission revoked".into())
        );
        assert_eq!(
            app.overleaf_sync_activity(),
            Activity::Failed("Permission revoked".into())
        );
        status(
            &mut app,
            &context,
            "connected",
            Some("Refresh to check other files on this older server."),
        );
        assert_eq!(app.overleaf_connection_activity(), Activity::Idle);
        assert_eq!(
            app.overleaf_sync_activity(),
            Activity::Pending("Syncing working copy")
        );
        // A late event from a superseded connection cannot replace its status.
        app.update_overleaf_event(
            BrowserEvent::Status {
                epoch: EPOCH - 1,
                state: "error".into(),
                message: Some("obsolete failure".into()),
            },
            &context,
        )
        .unwrap();
        assert_eq!(app.overleaf_connection_activity(), Activity::Idle);
    }

    #[test]
    fn sync_snapshots_include_hydration_queued_tree_files_and_deferred_downloads() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().canonicalize().unwrap();
        let context = egui::Context::default();
        let mut app = fixture(&root, &context);
        app.overleaf.connection.as_mut().unwrap().hydration = Hydration::waiting();
        assert_eq!(
            app.overleaf_sync_activity(),
            Activity::Pending("Syncing working copy")
        );
        app.overleaf.connection.as_mut().unwrap().hydration = Hydration::default();
        app.overleaf.tree_pending = app
            .overleaf
            .connection
            .as_ref()
            .unwrap()
            .model
            .tree()
            .cloned();
        assert_eq!(
            app.overleaf_sync_activity(),
            Activity::Pending("Working copy queued")
        );
        app.overleaf.tree_pending = None;
        let doc = entry("a", "main.tex", EntryKind::Doc);
        app.overleaf.queue(MirrorWork {
            checkpoint: false,
            entry: doc.clone(),
            data: MirrorData::Document("Main old\n".into()),
            save: None,
        });
        assert_eq!(
            app.overleaf_sync_activity(),
            Activity::Pending("Working copy queued")
        );
        app.overleaf.queued.clear();
        app.overleaf.downloads.defer(EPOCH, 0, doc).unwrap();
        assert_eq!(
            app.overleaf_sync_activity(),
            Activity::Pending("Working copy queued")
        );
        app.overleaf.error = Some("Deferred download failed".into());
        assert_eq!(
            app.overleaf_sync_activity(),
            Activity::Failed("Deferred download failed".into())
        );
        app.overleaf.downloads.clear();
        assert_eq!(
            app.overleaf_connection_activity(),
            Activity::Failed("Deferred download failed".into())
        );
    }

    #[test]
    fn unsaved_native_edits_in_active_and_parked_tabs_are_never_healthy() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().canonicalize().unwrap();
        let context = egui::Context::default();
        let mut app = fixture(&root, &context);
        app.document_mut()
            .edit(CCursorRange::default(), |source| source.push_str("Unsaved"));
        assert!(
            !app.overleaf
                .connection
                .as_ref()
                .unwrap()
                .model
                .has_unsynced()
        );
        assert_eq!(
            app.overleaf_save_for_active(&app.overleaf_save_activity()),
            Some(Activity::Pending("Unsaved changes"))
        );
        let _ = park_main(&mut app, &root, &context);
        assert_eq!(
            app.overleaf_save_activity(),
            Activity::Pending("Unsaved changes")
        );
        status(&mut app, &context, "signed-out", None);
        assert_eq!(
            app.overleaf_save_activity(),
            Activity::Pending("Sign in to save")
        );
        status(&mut app, &context, "disconnected", None);
        assert_eq!(
            app.overleaf_save_activity(),
            Activity::Pending("Offline · unsaved changes")
        );
    }

    #[test]
    fn unrelated_local_edits_do_not_make_remote_save_pending() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().canonicalize().unwrap();
        let local = tempfile::tempdir().unwrap();
        let context = egui::Context::default();
        let mut app = fixture(&root, &context);
        app.document_mut().replace_loaded_unprojected(
            "Local\n".into(),
            local.path().join("local.tex"),
            DocumentKind::Tex,
            Some(fingerprint(b"Local\n")),
        );
        app.document_mut()
            .edit(CCursorRange::default(), |source| source.push_str("Changed"));
        assert_eq!(app.overleaf_save_activity(), Activity::Idle);
        assert_eq!(
            app.overleaf_save_for_active(&app.overleaf_save_activity()),
            None
        );
    }

    #[test]
    fn generated_and_untracked_files_do_not_inherit_remote_save_status() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().canonicalize().unwrap();
        let context = egui::Context::default();
        let mut app = fixture(&root, &context);
        for relative in [".tiptoptyp/generated.tex", "untracked.tex"] {
            app.document_mut().replace_loaded_unprojected(
                "Generated\n".into(),
                root.join(relative),
                DocumentKind::Tex,
                Some(fingerprint(b"Generated\n")),
            );
            assert_eq!(app.overleaf_save_for_active(&Activity::Idle), None);
            app.document_mut().edit(CCursorRange::default(), |source| {
                source.push_str("Local edit")
            });
            assert_eq!(app.overleaf_save_activity(), Activity::Idle);
        }
    }

    #[test]
    fn mirror_work_and_retained_failure_never_report_a_healthy_save() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().canonicalize().unwrap();
        let context = egui::Context::default();
        let mut app = fixture(&root, &context);
        let (release, wait) = std::sync::mpsc::channel();
        let (repaint, repaints) = std::sync::mpsc::channel();
        context.set_request_repaint_callback(move |_| {
            let _ = repaint.send(());
        });
        app.overleaf
            .io
            .start_and_repaint("Activity mirror test", &context, move || {
                wait.recv().unwrap();
                Err("Working copy is not writable".into())
            })
            .unwrap();
        hold_test_save(&mut app, "a");
        assert_eq!(app.overleaf_sync_activity(), Activity::Running);
        assert_eq!(app.overleaf_save_activity(), Activity::Running);
        release.send(()).unwrap();
        repaints.recv_timeout(Duration::from_secs(5)).unwrap();
        assert!(matches!(app.overleaf.io.poll(), LatestJobPoll::Failed(_)));
        for _ in 0..3 {
            assert_eq!(
                app.overleaf_sync_activity(),
                Activity::Failed("Working copy is not writable".into())
            );
            assert_eq!(
                app.overleaf_save_activity(),
                Activity::Failed("Working copy is not writable".into())
            );
        }
        assert!(!app.overleaf.io.is_running());
    }

    #[test]
    fn save_acknowledgement_timeout_conflict_and_mirror_receipt_are_distinct() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().canonicalize().unwrap();
        let context = egui::Context::default();
        let mut app = fixture(&root, &context);
        let tab = app.tabs.active_id().unwrap();
        let request = pending_save(&mut app, tab, "a", "Saved remotely\n");
        assert_eq!(
            app.overleaf_save_activity(),
            Activity::Pending("Awaiting Overleaf confirmation")
        );
        app.overleaf.save.pending_mut().unwrap().started = Instant::now() - SAVE_TIMEOUT;
        assert!(
            matches!(app.overleaf_save_activity(), Activity::Failed(error) if error.contains("not confirmed"))
        );
        app.overleaf.save.pending_mut().unwrap().started = Instant::now();
        app.update_overleaf_event(
            BrowserEvent::Ack {
                epoch: EPOCH,
                id: "a".into(),
                request,
                source: "Saved remotely\n".into(),
            },
            &context,
        )
        .unwrap();
        assert_eq!(
            app.overleaf_save_activity(),
            Activity::Pending("Updating working copy")
        );
        let work = app.overleaf.queued.pop_front().unwrap();
        apply_work(&mut app, work, &root, EPOCH, &context);
        assert_eq!(app.overleaf_save_activity(), Activity::Idle);
        app.overleaf.blocked.insert("a".into());
        assert_eq!(
            app.overleaf_save_activity(),
            Activity::Failed(CONFLICT_ERROR.into())
        );
        app.overleaf.error = Some("Local and Overleaf versions need review".into());
        assert_eq!(
            app.overleaf_save_activity(),
            Activity::Failed("Local and Overleaf versions need review".into())
        );
    }
}

#[test]
fn chooser_switches_preserve_connection_and_pending_native_save() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().canonicalize().unwrap();
    let context = egui::Context::default();
    let mut app = fixture(&root, &context);
    let tab = app.tabs.active_id().unwrap();
    let wire = pending_save(&mut app, tab, "a", "Local pending source\n");
    let key = app.document().key();
    let epoch = app.overleaf.epoch;
    for _ in 0..3 {
        app.switch_workspace_chooser(&context, native_views::WorkspaceChooser::Local);
        assert!(app.workspace_chooser_visible);
        assert!(!app.overleaf.visible);
        app.switch_workspace_chooser(&context, native_views::WorkspaceChooser::Overleaf);
        assert!(!app.workspace_chooser_visible);
        assert!(app.overleaf.visible);
        let pending = app.overleaf.save.pending().unwrap();
        assert_eq!((pending.tab, pending.key, pending.wire), (tab, key, wire));
        assert!(app.document_workflow.save_in_flight());
        assert_eq!(app.document().source(), "Local pending source\n");
        assert_eq!(app.document().key(), key);
        assert_eq!(app.overleaf.epoch, epoch);
        assert!(app.overleaf.connection.as_ref().unwrap().online());
        assert_eq!(app.tabs.active_id(), Some(tab));
        assert_eq!(app.workspace_root, root);
    }
}

#[test]
fn choosing_local_workspace_prompts_for_overleaf_draft_and_cancel_preserves_binding() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().canonicalize().unwrap();
    let local = tempfile::tempdir().unwrap();
    let local_root = local.path().canonicalize().unwrap();
    let context = egui::Context::default();
    let mut app = fixture(&root, &context);
    let tab = app.tabs.active_id().unwrap();
    app.overleaf.tab_entities.insert(tab, "a".into());
    app.document_mut()
        .edit(CCursorRange::default(), |text| text.push_str("Unsaved\n"));
    let key = app.document().key();
    app.queue_open_path(local_root.clone());
    for _ in 0..8 {
        app.execute_pending_document_action(&context, None);
        if app.document_workflow.modal().is_some() {
            break;
        }
    }
    assert_eq!(app.workspace_root, root);
    assert!(matches!(
        app.document_workflow.modal(),
        Some(AppModal::Unsaved { .. })
    ));
    assert_eq!(app.tabs.ids().collect::<Vec<_>>(), [tab]);
    assert_eq!(app.document().source(), "Main old\nUnsaved\n");
    assert!(app.overleaf_tab_matches(tab, "a"));

    let _ = app.document_workflow.take_modal();
    app.document_workflow.clear_modal();
    app.document_workflow.cancel_continuation();
    app.cancel_workspace_switch(&context);
    app.document_workflow.finish_dispatch();
    app.execute_pending_document_action(&context, None);

    assert_eq!(app.workspace_root, root);
    assert_eq!(app.tabs.active_id(), Some(tab));
    assert_eq!(app.document().key(), key);
    assert_eq!(app.document().source(), "Main old\nUnsaved\n");
    assert!(app.document().is_dirty());
    assert!(app.overleaf_tab_matches(tab, "a"));
    assert!(app.overleaf.connection.as_ref().unwrap().online());
    assert!(app.document_workflow.modal().is_none());
    assert!(!app.document_flow_busy());
    assert_eq!(
        fs::read_to_string(root.join("main.tex")).unwrap(),
        "Main old\n"
    );
    assert_eq!(fs::read_dir(local_root).unwrap().count(), 0);
}

#[test]
fn remote_merge_reaches_native_only_after_mirror_receipt_and_undo_is_respected() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().canonicalize().unwrap();
    let context = egui::Context::default();
    let mut app = fixture(&root, &context);
    app.settings.auto_save = false;
    let base = "Opening base.\nMiddle base.\nClosing base.\n";
    let local = base.replace("Opening base.", "Opening local.");
    let remote = base.replace("Closing base.", "Closing remote.");
    let merged = local.replace("Closing base.", "Closing remote.");
    let event = |source: &str| BrowserEvent::Document {
        epoch: EPOCH,
        id: "a".into(),
        path: "main.tex".into(),
        source: source.into(),
    };
    app.update_overleaf_event(event(base), &context).unwrap();
    let work = app.overleaf.queued.pop_front().unwrap();
    apply_work(&mut app, work, &root, EPOCH, &context);
    app.document_mut()
        .edit(CCursorRange::default(), |text| *text = local.clone());
    app.update_overleaf_event(event(&remote), &context).unwrap();
    assert_eq!(app.document().source(), &local);
    assert!(app.overleaf_source_locked());
    app.update_overleaf_event(event(&remote), &context).unwrap();
    assert_eq!(
        app.overleaf
            .connection
            .as_ref()
            .unwrap()
            .model
            .document("a")
            .unwrap()
            .source(),
        merged
    );
    let work = app.overleaf.queued.pop_front().unwrap();
    apply_work(&mut app, work, &root, EPOCH, &context);
    assert_eq!(app.document().source(), &merged);
    assert_eq!(app.document().saved_source(), &remote);
    assert!(app.document().is_dirty());
    assert!(!app.overleaf_source_locked());
    assert_eq!(fs::read_to_string(root.join("main.tex")).unwrap(), remote);
    app.document_mut()
        .history_step(false, CCursorRange::default())
        .unwrap();
    assert_eq!(app.document().source(), &remote);
    assert!(!app.document().is_dirty());
    app.update_overleaf_event(event(&remote), &context).unwrap();
    let model = &app.overleaf.connection.as_ref().unwrap().model;
    assert_eq!(model.document("a").unwrap().source(), remote);
    assert!(
        !model.has_unsynced(),
        "an undone local edit must not be resurrected"
    );
}

#[test]
fn saving_during_native_merge_handoff_cannot_replace_the_merged_model_draft() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().canonicalize().unwrap();
    let context = egui::Context::default();
    let mut app = fixture(&root, &context);
    app.settings.auto_save = false;
    let base = "Opening base.\nMiddle base.\nClosing base.\n";
    let local = base.replace("Opening base.", "Opening local.");
    let remote = base.replace("Closing base.", "Closing remote.");
    let merged = local.replace("Closing base.", "Closing remote.");
    let path = root.join("main.tex");
    app.overleaf
        .connection
        .as_mut()
        .unwrap()
        .model
        .apply(BrowserEvent::Document {
            epoch: EPOCH,
            id: "a".into(),
            path: "main.tex".into(),
            source: base.into(),
        })
        .unwrap();
    app.document_mut().replace_loaded_unprojected(
        base.into(),
        path.clone(),
        DocumentKind::Tex,
        Some(fingerprint(base.as_bytes())),
    );
    app.document_mut()
        .edit(CCursorRange::default(), |text| *text = local.clone());
    app.update_overleaf_event(
        BrowserEvent::Document {
            epoch: EPOCH,
            id: "a".into(),
            path: "main.tex".into(),
            source: remote.clone(),
        },
        &context,
    )
    .unwrap();
    let tab = app.tabs.active_id().unwrap();
    assert!(app.overleaf.rebases.contains_key("a"));
    assert!(!app.submit_overleaf_save(tab, &path, &context));
    let model = &app.overleaf.connection.as_ref().unwrap().model;
    assert_eq!(model.document("a").unwrap().source(), merged);
    assert!(model.in_flight().is_none());
    assert_eq!(app.document().source(), &local);
    assert!(app.overleaf.rebases.contains_key("a"));
}

#[test]
fn local_chooser_discard_closes_overleaf_pages_but_rejected_save_retains_removed_source() {
    for removed in [false, true] {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().canonicalize().unwrap();
        let local = tempfile::tempdir().unwrap();
        let local_root = local.path().canonicalize().unwrap();
        let context = egui::Context::default();
        let mut app = fixture(&root, &context);
        let tab = app.tabs.active_id().unwrap();
        app.overleaf.tab_entities.insert(tab, "a".into());
        app.document_mut()
            .edit(CCursorRange::default(), |text| text.push_str("Unsaved\n"));
        if removed {
            app.update_overleaf_event(
                BrowserEvent::Project {
                    epoch: EPOCH,
                    project_id: PROJECT.into(),
                    name: "Remote paper".into(),
                    entries: vec![entry("b", "chapter.tex", EntryKind::Doc)],
                    root_doc: Some("b".into()),
                },
                &context,
            )
            .unwrap();
            assert!(app.overleaf_entry(&root.join("main.tex")).is_none());
        }
        let dispatch = |app: &mut EditorApp| {
            for _ in 0..8 {
                app.execute_pending_document_action(&context, None);
                if app.document_workflow.modal().is_some() {
                    break;
                }
            }
        };
        app.queue_open_path(local_root.clone());
        dispatch(&mut app);
        assert_eq!(app.workspace_root, root);
        assert!(matches!(
            app.document_workflow.modal(),
            Some(AppModal::Unsaved { .. })
        ));
        assert_eq!(app.tabs.ids().collect::<Vec<_>>(), [tab]);
        assert_eq!(app.document().source(), "Main old\nUnsaved\n");
        assert!(app.document().is_dirty());
        assert!(app.overleaf_tab_matches(tab, "a"));

        if removed {
            let Some(AppModal::Unsaved { pending, .. }) = app.document_workflow.take_modal() else {
                panic!("the removed source still requires a decision");
            };
            app.document_workflow.clear_modal();
            app.document_workflow.queue_action(PendingDocumentAction {
                action: DeferredDocumentAction::SaveThen(Box::new(pending)),
                key: app.document().key(),
                allow_discard: true,
                description: "saving the current document".into(),
            });
            dispatch(&mut app);

            assert_eq!(app.workspace_root, root);
            assert_eq!(app.tabs.ids().collect::<Vec<_>>(), [tab]);
            assert_eq!(app.document().source(), "Main old\nUnsaved\n");
            assert!(app.document().is_dirty());
            assert!(app.overleaf_tab_matches(tab, "a"));
            assert!(app.overleaf.save.pending().is_none());
            assert!(!app.document_workflow.save_in_flight());
            assert!(!app.document_workflow.has_continuation());
            assert!(
                app.overleaf
                    .error
                    .as_deref()
                    .unwrap()
                    .contains("unavailable")
            );

            app.queue_open_path(local_root.clone());
            dispatch(&mut app);
        }

        let Some(AppModal::Unsaved { mut pending, .. }) = app.document_workflow.take_modal() else {
            panic!("only explicit Discard may close the unsaved source");
        };
        app.document_workflow.clear_modal();
        app.document_workflow.cancel_continuation();
        pending.key = app.document().key();
        pending.allow_discard = true;
        app.document_workflow.queue_action(pending);
        dispatch(&mut app);

        assert_eq!(app.workspace_root, local_root);
        assert!(app.tabs.is_empty());
        assert!(app.document().path().is_none());
        assert!(app.document().source().is_empty());
        assert!(!app.overleaf.tab_entities.contains_key(&tab));
        assert!(app.overleaf.connection.as_ref().unwrap().online());
        assert_eq!(
            fs::read_to_string(root.join("main.tex")).unwrap(),
            "Main old\n"
        );
        assert_eq!(fs::read_dir(local_root).unwrap().count(), 0);
    }
}

#[test]
fn replacement_document_cannot_take_ownership_of_removed_source_at_the_same_path() {
    for dirty in [false, true] {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().canonicalize().unwrap();
        let context = egui::Context::default();
        let mut app = fixture(&root, &context);
        let tab = app.tabs.active_id().unwrap();
        app.overleaf.tab_entities.insert(tab, "a".into());
        if dirty {
            app.document_mut().edit(CCursorRange::default(), |text| {
                text.push_str("Original unsaved draft.\n");
            });
        }
        let original = app.document().source().to_owned();
        let key = app.document().key();
        let replacement = "Entirely different server document.\n";
        app.update_overleaf_event(
            BrowserEvent::Project {
                epoch: EPOCH,
                project_id: PROJECT.into(),
                name: "Remote paper".into(),
                entries: vec![entry("replacement", "main.tex", EntryKind::Doc)],
                root_doc: Some("replacement".into()),
            },
            &context,
        )
        .unwrap();
        app.update_overleaf_event(
            BrowserEvent::Document {
                epoch: EPOCH,
                id: "replacement".into(),
                path: "main.tex".into(),
                source: replacement.into(),
            },
            &context,
        )
        .unwrap();
        let tree = app.overleaf.tree_pending.take().unwrap();
        Mirror::open(&root).unwrap().apply_tree(&tree).unwrap();
        app.overleaf
            .connection
            .as_mut()
            .unwrap()
            .hydration
            .accept_tree(&tree);
        while let Some(work) = app.overleaf.queued.pop_front() {
            apply_work(&mut app, work, &root, EPOCH, &context);
        }
        app.finish_overleaf_hydration(&context);
        assert_eq!(
            fs::read_to_string(root.join("main.tex")).unwrap(),
            replacement
        );
        assert_eq!(app.document().source(), &original, "dirty={dirty}");
        assert_eq!(app.document().key(), key, "dirty={dirty}");
        assert_eq!(app.document().is_dirty(), dirty);
        assert!(app.overleaf_tab_matches(tab, "a"));
        assert!(app.overleaf_source_locked(), "the original entity is gone");
        let model = &app.overleaf.connection.as_ref().unwrap().model;
        if dirty {
            assert_eq!(model.document("a").unwrap().source(), original);
            assert!(model.document("a").unwrap().conflict().is_some());
        }
        assert_eq!(model.document("replacement").unwrap().source(), replacement);
        assert!(model.document("replacement").unwrap().draft().is_none());
        assert!(!app.submit_overleaf_save(tab, &root.join("main.tex"), &context));
        let model = &app.overleaf.connection.as_ref().unwrap().model;
        assert!(model.in_flight().is_none());
        assert_eq!(model.document("replacement").unwrap().source(), replacement);
        assert!(app.open_overleaf_path(&root.join("main.tex")));
        assert!(app.overleaf.error.as_deref().unwrap().contains("original"));
        assert_eq!(app.document().source(), &original);
        assert!(app.overleaf_tab_matches(tab, "a"));
        // An open already dispatched before the path was reused cannot adopt
        // the original tab when its asynchronous mirror receipt arrives.
        app.arm_overleaf_navigation();
        app.overleaf.open_after = Some("replacement".into());
        apply_work(
            &mut app,
            MirrorWork {
                checkpoint: false,
                entry: entry("replacement", "main.tex", EntryKind::Doc),
                data: MirrorData::Document(replacement.into()),
                save: None,
            },
            &root,
            EPOCH,
            &context,
        );
        assert_eq!(app.document().source(), &original);
        assert_eq!(app.document().key(), key);
        assert!(app.overleaf_tab_matches(tab, "a"));
        assert!(app.overleaf.open_after.is_none());
    }
}

#[test]
fn invalid_authoritative_event_ends_batch_and_invalidates_pending_hydration() {
    for decode_error in [false, true] {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().canonicalize().unwrap();
        let context = egui::Context::default();
        let mut app = fixture(&root, &context);
        let tab = app.tabs.active_id().unwrap();
        app.overleaf.tab_entities.insert(tab, "a".into());
        app.document_mut().edit(CCursorRange::default(), |text| {
            text.push_str("Retain this unsaved draft.\n");
        });
        let source = app.document().source().clone();
        let key = app.document().key();
        let connection = app.overleaf.connection.as_mut().unwrap();
        let tree = connection.model.tree().unwrap().clone();
        connection.hydration.reset(&tree);
        connection.hydration.accept_tree(&tree);
        for entry in tree.entries() {
            connection.hydration.accept_file(entry);
        }
        let session = connection.hydration.session;
        let inbox = crate::overleaf_inbox::Inbox::new(&context);
        app.overleaf.sender = Some(inbox.sender());
        app.overleaf.events = Some(inbox);
        app.overleaf.tree_pending = Some(tree);
        app.overleaf.queued.push_back(MirrorWork {
            checkpoint: false,
            entry: entry("a", "main.tex", EntryKind::Doc),
            data: MirrorData::Document("Stale pending mirror.\n".into()),
            save: None,
        });
        let invalid = if decode_error {
            Err("Overleaf returned an invalid bridge message.".into())
        } else {
            Ok(BrowserEvent::Project {
                epoch: EPOCH,
                project_id: "abcdef0123456789abcdef01".into(),
                name: "Unrelated project".into(),
                entries: Vec::new(),
                root_doc: None,
            })
        };
        app.apply_overleaf_events(
            [
                invalid,
                Ok(listing(Some("main.tex"))),
                Ok(BrowserEvent::Status {
                    epoch: EPOCH,
                    state: "connected".into(),
                    message: None,
                }),
            ],
            &context,
        );
        app.finish_overleaf_hydration(&context);
        let connection = app.overleaf.connection.as_ref().unwrap();
        assert!(!connection.online());
        assert!(connection.hydration.pending());
        assert_ne!(connection.hydration.session, session);
        assert_eq!(connection.status, "Reconnect required");
        assert!(app.overleaf.events.is_none());
        assert!(app.overleaf.sender.is_none());
        assert!(app.overleaf.tree_pending.is_none());
        assert!(app.overleaf.queued.is_empty());
        assert!(app.overleaf_source_locked());
        assert_eq!(app.document().source(), &source);
        assert_eq!(app.document().key(), key);
        assert!(app.document().is_dirty());
        assert!(
            app.overleaf
                .connection
                .as_mut()
                .unwrap()
                .model
                .publish("a")
                .is_none()
        );
    }
}

fn assert_explorer_status(app: &EditorApp, tone: explorer_view::WorkspaceStatusTone, text: &str) {
    let (_, name, status) = app
        .overleaf_explorer()
        .expect("selected Overleaf workspace");
    assert_eq!(
        name, "Remote paper",
        "state words do not belong in the workspace name"
    );
    assert_eq!(status.tone, tone);
    assert_eq!(status.text, text);
}

#[test]
fn explorer_project_name_is_independent_of_ready_unsynced_saving_and_error_status() {
    use explorer_view::WorkspaceStatusTone as Tone;
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().canonicalize().unwrap();
    let context = egui::Context::default();
    let mut app = fixture(&root, &context);
    app.overleaf.connection.as_mut().unwrap().status = "Server-confirmed files".into();
    assert_explorer_status(&app, Tone::Ready, "Server-confirmed files");
    let tab = app.tabs.active_id().unwrap();
    app.document_mut().edit(CCursorRange::default(), |source| {
        *source = "Main draft\n".into()
    });
    assert_explorer_status(&app, Tone::Warning, "Unsynced");
    pending_save(&mut app, tab, "a", "Main draft\n");
    assert_explorer_status(&app, Tone::Busy, "Saving");
    app.overleaf.error = Some("Overleaf has not confirmed this save".into());
    assert_explorer_status(&app, Tone::Error, "Overleaf has not confirmed this save");
}

#[test]
fn explorer_attention_and_sign_in_take_precedence_over_pending_saves_and_offline_state() {
    use explorer_view::WorkspaceStatusTone as Tone;
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().canonicalize().unwrap();
    let context = egui::Context::default();
    let mut app = fixture(&root, &context);
    hold_test_save(&mut app, "a");
    app.overleaf.blocked.insert("a".into());
    assert_explorer_status(&app, Tone::Error, "Needs attention");
    app.overleaf.error = Some("Review conflicting changes".into());
    assert_explorer_status(&app, Tone::Error, "Review conflicting changes");
    app.overleaf.error = None;
    app.overleaf.blocked.clear();
    app.update_overleaf_event(
        BrowserEvent::Status {
            epoch: EPOCH,
            state: "error".into(),
            message: Some("Permission revoked".into()),
        },
        &context,
    )
    .unwrap();
    assert!(
        app.overleaf.error.is_none(),
        "typed backend failures are independent of native errors"
    );
    assert_explorer_status(&app, Tone::Error, "Permission revoked");
    app.overleaf.browser_status = activity::BrowserStatus::Dormant;
    app.overleaf.authentication.set_required(true);
    app.overleaf.connection.as_mut().unwrap().transport = Transport::Offline;
    assert_explorer_status(&app, Tone::Warning, "Sign in required");
    app.overleaf.authentication.set_required(false);
    app.overleaf.connection.as_mut().unwrap().status = "Ready".into();
    assert_explorer_status(&app, Tone::Offline, "Offline");
    app.document_mut().edit(CCursorRange::default(), |source| {
        source.push_str("Unsent edit")
    });
    assert_explorer_status(&app, Tone::Warning, "Offline · unsynced changes");
}

#[test]
fn explorer_sync_and_save_timeout_use_owned_state_instead_of_status_text() {
    use explorer_view::WorkspaceStatusTone as Tone;
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().canonicalize().unwrap();
    let context = egui::Context::default();
    let mut app = fixture(&root, &context);
    app.overleaf.connection.as_mut().unwrap().status = "Ready".into();
    app.document_mut().edit(CCursorRange::default(), |source| {
        *source = "Main draft\n".into()
    });
    app.overleaf
        .connection
        .as_mut()
        .unwrap()
        .model
        .set_draft("a", "Main draft\n".into())
        .unwrap();
    app.stage_overleaf_rebase("a", "Main old\n").unwrap();
    assert!(!app.overleaf.rebases.is_empty());
    assert!(app.overleaf.blocked.is_empty());
    assert_explorer_status(&app, Tone::Busy, "Syncing files…");
    app.overleaf.rebases.clear();
    app.overleaf.connection.as_mut().unwrap().hydration = Hydration::waiting();
    assert_explorer_status(&app, Tone::Busy, "Syncing files…");
    app.overleaf.connection.as_mut().unwrap().transport = Transport::Offline;
    app.overleaf.browser.begin();
    assert_explorer_status(&app, Tone::Busy, "Connecting to Overleaf…");
    app.overleaf.browser.cancel_creation();
    app.overleaf.connection.as_mut().unwrap().transport = Transport::Online;
    let tab = app.tabs.active_id().unwrap();
    pending_save(&mut app, tab, "a", "Main draft\n");
    app.overleaf.save.pending_mut().unwrap().started = Instant::now() - SAVE_TIMEOUT;
    assert_explorer_status(&app, Tone::Error, "Save not confirmed · reconnect required");
}

#[test]
fn remote_status_does_not_add_an_indicator_to_a_selected_local_workspace() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().canonicalize().unwrap();
    let local = tempfile::tempdir().unwrap();
    let context = egui::Context::default();
    let mut app = fixture(&root, &context);
    hold_test_save(&mut app, "a");
    app.overleaf.authentication.set_required(true);
    app.overleaf.error = Some("Remote error".into());
    app.workspace_root = local.path().canonicalize().unwrap();
    assert!(app.overleaf_explorer().is_none());
}

#[test]
fn browser_creation_and_authentication_have_separate_authoritative_owners() {
    let mut browser = BrowserLifecycle::default();
    assert!(browser.is_none());
    assert!(!browser.creating());
    browser.begin();
    assert!(browser.creating());
    assert!(browser.as_ref().is_none());
    browser = BrowserLifecycle::Failed;
    browser.cancel_creation();
    assert!(matches!(browser, BrowserLifecycle::Failed));
    browser.begin();
    assert!(browser.creating());
    browser.cancel_creation();
    assert!(matches!(browser, BrowserLifecycle::Closed));

    let mut authentication = Authentication::default();
    authentication.set_required(true);
    assert!(authentication.required());
    assert!(!authentication.return_attempted());
    authentication.mark_return_attempted();
    assert!(authentication.required() && authentication.return_attempted());
    authentication.set_required(false);
    assert!(!authentication.required());
    assert!(
        authentication.return_attempted(),
        "browser replacement cannot grant a second automatic return"
    );
    authentication.allow_return();
    assert!(!authentication.return_attempted());
}

#[test]
fn workspace_adoption_receipt_belongs_to_one_browser_epoch() {
    let mut state = State {
        epoch: 7,
        ..Default::default()
    };
    state.await_project();
    assert!(state.awaiting_project());
    state.epoch = 8;
    assert!(!state.awaiting_project());
    assert!(!state.take_adoption());
    state.await_project();
    assert!(state.take_adoption());
    assert!(!state.take_adoption());
}
