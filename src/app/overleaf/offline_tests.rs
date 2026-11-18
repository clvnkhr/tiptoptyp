//! Real checkpoint workers must not turn a local save into a cloud receipt.
use super::tests::{PROJECT, committed_document_reply, fixture, park_main};
use super::*;
use crate::overleaf::{
    OfflineStore, StorageLease, StorageOptions, workspace_session::WorkspaceSession,
};

struct PersistentFixture {
    _directory: tempfile::TempDir,
    context: egui::Context,
    app: EditorApp,
    lease: Arc<StorageLease>,
    base: PathBuf,
}

fn persistent_fixture() -> PersistentFixture {
    let directory = tempfile::tempdir().unwrap();
    let parent = directory.path().canonicalize().unwrap();
    let base = parent.join("workspace");
    let project =
        ProjectUrl::parse(&format!("https://www.overleaf.com/project/{PROJECT}")).unwrap();
    let lease = StorageLease::prepare(
        &project,
        &StorageOptions {
            keep_local_copy: true,
            directory: Some(base.clone()),
        },
        &parent,
    )
    .unwrap();
    let context = egui::Context::default();
    let mut app = fixture(lease.root(), &context);
    app.settings.auto_save = false;
    app.overleaf.connection.as_mut().unwrap().storage = Some(Arc::clone(&lease));
    let session = WorkspaceSession::new(&project, [0x12; 16]);
    session.write(lease.root(), &project).unwrap();
    app.overleaf.browser_profile = Some(session.profile_id);
    app.overleaf.offline.install(session, false);
    let connection = app.overleaf.connection.as_ref().unwrap();
    let mirror = Mirror::open(lease.root()).unwrap();
    mirror
        .sync_tree(&project, connection.model.tree().unwrap())
        .unwrap();
    let main = app.tabs.active_id().unwrap();
    app.overleaf.tab_entities.insert(main, "a".into());
    PersistentFixture {
        _directory: directory,
        context,
        app,
        lease,
        base,
    }
}

fn settle(app: &mut EditorApp, context: &egui::Context) {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        app.poll_overleaf(context);
        if !app.overleaf.io.is_running()
            && app.overleaf.queued.is_empty()
            && !app.overleaf.save.active()
            && app.overleaf.offline.saved()
        {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "checkpoint worker did not settle: {:?}",
            app.overleaf.error
        );
        std::thread::yield_now();
    }
    assert!(app.overleaf.error.is_none(), "{:?}", app.overleaf.error);
}

fn go_offline(fixture: &mut PersistentFixture) {
    fixture.app.overleaf_work_offline(&fixture.context).unwrap();
    settle(&mut fixture.app, &fixture.context);
}

fn save_main(fixture: &mut PersistentFixture, source: &str) -> u64 {
    let tab = fixture.app.tabs.active_id().unwrap();
    fixture
        .app
        .document_mut()
        .edit(CCursorRange::default(), |text| *text = source.into());
    assert!(fixture.app.submit_overleaf_save(
        tab,
        &fixture.lease.root().join("main.tex"),
        &fixture.context,
    ));
    assert!(fixture.app.document_workflow.save_in_flight());
    assert!(fixture.app.overleaf.save.pending().is_none());
    settle(&mut fixture.app, &fixture.context);
    tab
}

fn reopen(fixture: &PersistentFixture) -> EditorApp {
    let mut app = EditorApp::dormant_for_tests(&fixture.context, fixture.lease.root().into());
    app.empty_workspace(&fixture.context);
    app.settings.auto_save = false;
    let project = &fixture.app.overleaf.connection.as_ref().unwrap().project;
    app.settings
        .set_overleaf_storage_preference(
            project.url(),
            crate::settings::OverleafStoragePreference {
                keep_local_copy: true,
                directory: Some(fixture.base.to_string_lossy().into_owned()),
            },
        )
        .unwrap();
    app.open_overleaf_project_url(project.url(), None, &fixture.context)
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    while app.overleaf.storage_job.is_running() {
        app.poll_overleaf_storage(&fixture.context);
        assert!(Instant::now() < deadline, "restore worker did not settle");
        std::thread::yield_now();
    }
    settle(&mut app, &fixture.context);
    app
}

fn seed_stored_workspace(
    base: &Path,
    project: &ProjectUrl,
    profile: [u8; 16],
    source: &str,
) -> Arc<StorageLease> {
    let lease = StorageLease::prepare(
        project,
        &StorageOptions {
            keep_local_copy: true,
            directory: Some(base.into()),
        },
        base.parent().unwrap(),
    )
    .unwrap();
    let entry = tests::entry("a", "main.tex", EntryKind::Doc);
    let tree = Tree::new(
        "Saved workspace".into(),
        Some("a".into()),
        vec![entry.clone()],
    )
    .unwrap();
    let mirror = Mirror::open(lease.root()).unwrap();
    mirror.sync_tree(project, &tree).unwrap();
    mirror
        .sync_document(&entry, &crate::overleaf::Document::new(source.into()))
        .unwrap();
    let mut session = WorkspaceSession::new(project, profile);
    session.offline = true;
    session.has_page_state = true;
    session.open_documents.push("a".into());
    session.active_document = Some("a".into());
    session.preview_document = Some("a".into());
    session.write(lease.root(), project).unwrap();
    crate::overleaf::workspace_session::bind_workspace(base, lease.root(), project).unwrap();
    lease
}

fn finish_stored_open(app: &mut EditorApp, context: &egui::Context) {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        app.execute_pending_document_action(context, None);
        app.poll_overleaf(context);
        app.advance_workspace_switch(context);
        if !app.overleaf.storage_job.is_running() && !app.workspace_switch.active() {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "workspace restore did not finish: {:?}",
            app.overleaf.error
        );
        std::thread::yield_now();
    }
    settle(app, context);
}

#[test]
fn offline_save_is_locally_clean_but_retains_an_unpublished_remote_draft() {
    let mut fixture = persistent_fixture();
    go_offline(&mut fixture);
    save_main(&mut fixture, "Main saved offline\n");

    assert!(!fixture.app.document().is_dirty());
    assert_eq!(
        fixture.app.document().saved_source(),
        "Main saved offline\n"
    );
    assert!(!fixture.app.document_workflow.save_in_flight());
    assert!(fixture.app.overleaf.save.pending().is_none());
    assert!(fixture.app.overleaf.browser.is_none());
    let connection = fixture.app.overleaf.connection.as_ref().unwrap();
    assert!(!connection.online());
    assert!(connection.model.in_flight().is_none());
    let document = connection.model.document("a").unwrap();
    assert_eq!(document.baseline, "Main old\n");
    assert_eq!(document.draft(), Some("Main saved offline\n"));
    assert!(connection.model.has_unsynced());
    assert_eq!(
        fs::read_to_string(fixture.lease.root().join("main.tex")).unwrap(),
        "Main saved offline\n"
    );
    let mut restored = OfflineStore::load(
        fixture.lease.root(),
        &connection.project,
        connection.model.epoch() + 1,
    )
    .unwrap()
    .unwrap();
    assert_eq!(restored.document("a").unwrap().baseline, "Main old\n");
    assert_eq!(
        restored.document("a").unwrap().source(),
        "Main saved offline\n"
    );
    assert!(
        restored.publish("a").is_none(),
        "disk is not server verification"
    );
}

#[test]
fn work_offline_waits_for_an_accepted_remote_update_to_reach_disk_and_native_source() {
    let mut fixture = persistent_fixture();
    let epoch = fixture.app.overleaf.epoch;
    fixture
        .app
        .update_overleaf_event(
            BrowserEvent::Document {
                epoch,
                id: "a".into(),
                path: "main.tex".into(),
                source: "Fresh remote source\n".into(),
            },
            &fixture.context,
        )
        .unwrap();
    assert!(!fixture.app.overleaf.queued.is_empty());
    assert_eq!(fixture.app.document().source(), "Main old\n");
    assert!(fixture.app.overleaf_work_offline(&fixture.context).is_err());
    assert_eq!(fixture.app.overleaf.epoch, epoch);
    assert!(!fixture.app.overleaf.offline.active());
    settle(&mut fixture.app, &fixture.context);
    assert_eq!(fixture.app.document().source(), "Fresh remote source\n");
    go_offline(&mut fixture);
    assert_eq!(
        fixture
            .app
            .overleaf
            .connection
            .as_ref()
            .unwrap()
            .model
            .document("a")
            .unwrap()
            .baseline,
        "Fresh remote source\n"
    );
    assert_eq!(
        fs::read_to_string(fixture.lease.root().join("main.tex")).unwrap(),
        "Fresh remote source\n"
    );
}

#[test]
fn first_adoption_reviews_existing_local_text_and_use_remote_does_not_resurrect_it_on_close() {
    let mut fixture = persistent_fixture();
    let root = fixture.lease.root().to_owned();
    fs::remove_dir_all(root.join(".tiptoptyp/overleaf-sync")).unwrap();
    let project = fixture
        .app
        .overleaf
        .connection
        .as_ref()
        .unwrap()
        .project
        .clone();
    fixture.app.overleaf.connection.as_mut().unwrap().model =
        Model::for_project(tests::EPOCH, &project);
    fixture
        .app
        .update_overleaf_event(
            BrowserEvent::Project {
                epoch: tests::EPOCH,
                project_id: PROJECT.into(),
                name: "Existing copy".into(),
                root_doc: Some("a".into()),
                entries: vec![
                    tests::entry("a", "main.tex", EntryKind::Doc),
                    tests::entry("b", "chapter.tex", EntryKind::Doc),
                ],
            },
            &fixture.context,
        )
        .unwrap();
    for (id, path, source) in [
        ("a", "main.tex", "Fresh Overleaf version\n"),
        ("b", "chapter.tex", "Chapter old\n"),
    ] {
        fixture
            .app
            .update_overleaf_event(
                BrowserEvent::Document {
                    epoch: tests::EPOCH,
                    id: id.into(),
                    path: path.into(),
                    source: source.into(),
                },
                &fixture.context,
            )
            .unwrap();
    }
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        fixture.app.poll_overleaf(&fixture.context);
        if !fixture.app.overleaf.io.is_running()
            && fixture.app.overleaf.queued.is_empty()
            && fixture.app.overleaf.tree_pending.is_none()
            && !fixture
                .app
                .overleaf
                .connection
                .as_ref()
                .unwrap()
                .hydration
                .pending()
        {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "existing-copy review did not settle"
        );
        std::thread::yield_now();
    }
    let model = &fixture.app.overleaf.connection.as_ref().unwrap().model;
    assert!(!model.document("a").unwrap().conflict().unwrap().base_known);
    let review = model.conflict_review("a").unwrap();
    assert_eq!(review.local, "Main old\n");
    assert_eq!(review.remote, "Fresh Overleaf version\n");
    assert_eq!(
        fs::read_to_string(root.join("main.tex")).unwrap(),
        review.local
    );
    assert!(model.in_flight().is_none());
    fixture
        .app
        .apply_overleaf_review(&fixture.context, &review, review.remote.clone())
        .unwrap();
    settle(&mut fixture.app, &fixture.context);
    assert!(!fixture.app.overleaf.offline.saved_sources.contains_key("a"));
    let main = fixture.app.tabs.active_id().unwrap();
    fixture.app.close_workspace_tabs(&[main], &fixture.context);
    fixture.app.prune_overleaf_bindings();
    settle(&mut fixture.app, &fixture.context);
    assert_eq!(
        fs::read_to_string(root.join("main.tex")).unwrap(),
        review.remote
    );
    assert!(
        !fixture
            .app
            .overleaf
            .connection
            .as_ref()
            .unwrap()
            .model
            .has_unsynced()
    );
}

#[test]
fn discarding_a_stale_window_preserves_the_other_windows_saved_file_and_reloads_it() {
    let mut fixture = persistent_fixture();
    go_offline(&mut fixture);
    let project = fixture
        .app
        .overleaf
        .connection
        .as_ref()
        .unwrap()
        .project
        .clone();
    let mut other = OfflineStore::load(fixture.lease.root(), &project, tests::EPOCH + 100)
        .unwrap()
        .unwrap();
    other
        .set_draft("a", "Saved in another window\n".into())
        .unwrap();
    Mirror::open(fixture.lease.root())
        .unwrap()
        .sync_document(
            other.tree().unwrap().entry("a").unwrap(),
            other.document("a").unwrap(),
        )
        .unwrap();
    fixture
        .app
        .document_mut()
        .edit(CCursorRange::default(), |source| {
            *source = "This window's unsaved edit\n".into()
        });
    let main = fixture.app.tabs.active_id().unwrap();
    assert!(fixture.app.submit_overleaf_save(
        main,
        &fixture.lease.root().join("main.tex"),
        &fixture.context
    ));
    let deadline = Instant::now() + Duration::from_secs(10);
    while fixture.app.overleaf.save.active()
        || fixture.app.overleaf.io.is_running()
        || !fixture.app.overleaf.queued.is_empty()
    {
        fixture.app.poll_overleaf(&fixture.context);
        assert!(Instant::now() < deadline);
        std::thread::yield_now();
    }
    assert!(fixture.app.overleaf.offline.disk_untrusted.contains("a"));
    fixture.app.close_workspace_tabs(&[main], &fixture.context);
    fixture.app.prune_overleaf_bindings();
    assert!(fixture.app.overleaf.queued.is_empty());
    assert_eq!(
        fs::read_to_string(fixture.lease.root().join("main.tex")).unwrap(),
        "Saved in another window\n"
    );
    fixture
        .app
        .open_stored_overleaf_project(
            &project,
            fixture.lease.root(),
            Some(PathBuf::from("main.tex")),
            &fixture.context,
        )
        .unwrap();
    finish_stored_open(&mut fixture.app, &fixture.context);
    assert_eq!(fixture.app.document().source(), "Saved in another window\n");
    assert!(fixture.app.overleaf.offline.disk_untrusted.is_empty());
    assert!(!fixture.app.document().is_dirty());
}

#[test]
fn selecting_another_storage_folder_allocates_a_login_profile_instead_of_sharing_the_active_account()
 {
    let mut fixture = persistent_fixture();
    go_offline(&mut fixture);
    let old_profile = fixture.app.overleaf.browser_profile.unwrap();
    let old_project = fixture
        .app
        .overleaf
        .connection
        .as_ref()
        .unwrap()
        .project
        .clone();
    let main = fixture.app.tabs.active_id().unwrap();
    fixture.app.close_workspace_tabs(&[main], &fixture.context);
    fixture.app.prune_overleaf_bindings();
    settle(&mut fixture.app, &fixture.context);
    fixture.app.workspace_root = fixture.base.parent().unwrap().into();
    let other =
        ProjectUrl::parse("https://www.overleaf.com/project/1123456789abcdef01234567").unwrap();
    let base = fixture
        .base
        .parent()
        .unwrap()
        .join("another-storage-choice");
    let root = other.mirror_root(&base);
    fixture
        .app
        .open_stored_overleaf_project(&other, &root, None, &fixture.context)
        .unwrap();
    finish_stored_open(&mut fixture.app, &fixture.context);
    assert_ne!(fixture.app.overleaf.browser_profile, Some(old_profile));
    assert_eq!(
        WorkspaceSession::load(fixture.lease.root(), &old_project)
            .unwrap()
            .unwrap()
            .profile_id,
        old_profile
    );
    assert_eq!(
        WorkspaceSession::load(&root, &other)
            .unwrap()
            .unwrap()
            .profile_id,
        fixture.app.overleaf.browser_profile.unwrap()
    );
}

#[test]
fn closing_a_locally_saved_page_preserves_its_cloud_draft_and_discards_only_newer_typing() {
    for discard_newer_typing in [false, true] {
        let mut fixture = persistent_fixture();
        go_offline(&mut fixture);
        let main = save_main(&mut fixture, "Saved offline source\n");
        if discard_newer_typing {
            fixture
                .app
                .document_mut()
                .edit(CCursorRange::default(), |source| {
                    source.push_str("Unsaved typing\n")
                });
            let source = fixture.app.document().source().to_owned();
            fixture
                .app
                .overleaf
                .connection
                .as_mut()
                .unwrap()
                .model
                .set_draft("a", source)
                .unwrap();
        }
        // Closing here models an already approved close/discard decision.
        fixture.app.close_workspace_tabs(&[main], &fixture.context);
        fixture.app.prune_overleaf_bindings();
        settle(&mut fixture.app, &fixture.context);

        assert!(fixture.app.tabs.is_empty());
        let model = &fixture.app.overleaf.connection.as_ref().unwrap().model;
        assert!(model.has_unsynced());
        assert_eq!(
            model.document("a").unwrap().source(),
            "Saved offline source\n"
        );
        assert_eq!(model.document("a").unwrap().baseline, "Main old\n");
        assert_eq!(
            fs::read_to_string(fixture.lease.root().join("main.tex")).unwrap(),
            "Saved offline source\n"
        );
        assert_eq!(
            fixture.app.overleaf.offline.saved_sources["a"],
            "Saved offline source\n"
        );
    }
}

#[test]
fn reopening_an_offline_workspace_restores_page_order_active_page_preview_and_profile() {
    let mut fixture = persistent_fixture();
    let (main, chapter) = park_main(&mut fixture.app, fixture.lease.root(), &fixture.context);
    fixture.app.overleaf.tab_entities.insert(main, "a".into());
    fixture
        .app
        .overleaf
        .tab_entities
        .insert(chapter, "b".into());
    fixture.app.select_preview_tab(chapter, &fixture.context);
    fixture.app.activate_tab(main, &fixture.context);
    go_offline(&mut fixture);
    let project = &fixture.app.overleaf.connection.as_ref().unwrap().project;
    let saved = WorkspaceSession::load(fixture.lease.root(), project)
        .unwrap()
        .unwrap();
    assert_eq!(saved.open_documents, ["a", "b"]);
    assert_eq!(saved.active_document.as_deref(), Some("a"));
    assert_eq!(saved.preview_document.as_deref(), Some("b"));
    assert!(saved.offline);

    let restored = reopen(&fixture);
    let entities: Vec<_> = restored
        .tabs
        .ids()
        .map(|tab| restored.overleaf_entity_for_tab(tab).unwrap())
        .collect();
    assert_eq!(entities, ["a", "b"]);
    assert_eq!(
        restored.overleaf_entity_for_tab(restored.tabs.active_id().unwrap()),
        Some("a")
    );
    assert_eq!(
        restored.overleaf_entity_for_tab(restored.tabs.preview_id().unwrap()),
        Some("b")
    );
    assert_eq!(restored.overleaf.browser_profile, Some(saved.profile_id));
    assert!(restored.overleaf.offline.active());
    assert!(restored.overleaf.offline.restored);
    assert!(restored.overleaf.browser.is_none());
    assert!(!restored.overleaf.browser.creating());
    assert!(!restored.overleaf_source_locked());
}

#[test]
fn reopening_a_saved_workspace_with_all_pages_closed_keeps_it_empty() {
    let mut fixture = persistent_fixture();
    go_offline(&mut fixture);
    let closing: Vec<_> = fixture.app.tabs.ids().collect();
    fixture.app.close_workspace_tabs(&closing, &fixture.context);
    fixture.app.prune_overleaf_bindings();
    settle(&mut fixture.app, &fixture.context);
    let project = &fixture.app.overleaf.connection.as_ref().unwrap().project;
    let saved = WorkspaceSession::load(fixture.lease.root(), project)
        .unwrap()
        .unwrap();
    assert!(saved.open_documents.is_empty());
    assert!(saved.active_document.is_none());
    assert!(saved.preview_document.is_none());

    let restored = reopen(&fixture);
    assert!(
        restored.tabs.is_empty(),
        "closed pages must not reopen the main file"
    );
    assert!(restored.overleaf.offline.active());
    assert!(restored.overleaf.offline.restored);
}

#[test]
fn installing_a_different_workspace_drops_its_predecessors_saved_source_and_mode() {
    let first = ProjectUrl::parse(&format!("https://www.overleaf.com/project/{PROJECT}")).unwrap();
    let second =
        ProjectUrl::parse("https://www.overleaf.com/project/1123456789abcdef01234567").unwrap();
    let mut state = offline::State::default();
    let mut session = WorkspaceSession::new(&first, [0x12; 16]);
    session.offline = true;
    session.open_documents.push("shared-fixture-id".into());
    state.install(session, true);
    state
        .saved_sources
        .insert("shared-fixture-id".into(), "First project source".into());
    state.install(WorkspaceSession::new(&second, [0x34; 16]), false);

    assert!(state.saved_sources.is_empty());
    assert!(!state.active());
    assert!(!state.restored);
    assert!(!state.restoring_pages());
    assert!(state.saved());
    let session = state.session.as_ref().unwrap();
    assert_eq!(session.profile_id, [0x34; 16]);
    assert!(session.open_documents.is_empty());
    assert!(session.active_document.is_none());
}

#[test]
fn cached_open_cannot_relabel_an_old_native_draft_as_the_replacement_at_its_path() {
    let mut fixture = persistent_fixture();
    go_offline(&mut fixture);
    let main = save_main(&mut fixture, "Original document's saved local work\n");
    let key = fixture.app.document().key();
    let epoch = fixture.app.overleaf.epoch;
    let model = &mut fixture.app.overleaf.connection.as_mut().unwrap().model;
    model
        .apply(BrowserEvent::Project {
            epoch,
            project_id: PROJECT.into(),
            name: "Replaced document".into(),
            entries: vec![
                tests::entry("replacement", "main.tex", EntryKind::Doc),
                tests::entry("b", "chapter.tex", EntryKind::Doc),
            ],
            root_doc: Some("replacement".into()),
        })
        .unwrap();
    model
        .apply(BrowserEvent::Document {
            epoch,
            id: "replacement".into(),
            path: "main.tex".into(),
            source: "Different remote document\n".into(),
        })
        .unwrap();

    assert!(
        fixture
            .app
            .open_cached_overleaf_path(&fixture.lease.root().join("main.tex"), &fixture.context,)
    );
    assert_eq!(
        fixture
            .app
            .overleaf
            .tab_entities
            .get(&main)
            .map(String::as_str),
        Some("a")
    );
    assert_eq!(fixture.app.document().key(), key);
    assert_eq!(
        fixture.app.document().source(),
        "Original document's saved local work\n"
    );
    assert!(fixture.app.overleaf.error.is_some());
    let model = &fixture.app.overleaf.connection.as_ref().unwrap().model;
    assert_eq!(
        model.document("replacement").unwrap().source(),
        "Different remote document\n"
    );
    assert_eq!(
        model.document("a").unwrap().source(),
        "Original document's saved local work\n"
    );
}

#[test]
fn discarding_online_typing_after_a_remote_update_does_not_promote_its_recovery_checkpoint() {
    let mut fixture = persistent_fixture();
    let tab = fixture.app.tabs.active_id().unwrap();
    fixture
        .app
        .document_mut()
        .edit(CCursorRange::default(), |source| {
            *source = "Main old\nLocal unsaved paragraph\n".into();
        });
    fixture
        .app
        .update_overleaf_event(
            BrowserEvent::Document {
                epoch: fixture.app.overleaf.epoch,
                id: "a".into(),
                path: "main.tex".into(),
                source: "Main remote\n".into(),
            },
            &fixture.context,
        )
        .unwrap();
    settle(&mut fixture.app, &fixture.context);
    assert!(fixture.app.document().is_dirty());
    assert!(
        fixture
            .app
            .document()
            .source()
            .contains("Local unsaved paragraph")
    );
    assert_eq!(fixture.app.document().saved_source(), "Main remote\n");

    // A recovery checkpoint must not turn a prior unsaved edit into a Save.
    fixture.app.close_workspace_tabs(&[tab], &fixture.context);
    fixture.app.prune_overleaf_bindings();
    let model = &fixture.app.overleaf.connection.as_ref().unwrap().model;
    assert_eq!(model.document("a").unwrap().source(), "Main remote\n");
    assert!(!model.document("a").unwrap().is_dirty());
}

#[test]
fn a_rejected_offline_disk_write_never_cleans_the_native_draft_or_changes_external_bytes() {
    let mut fixture = persistent_fixture();
    go_offline(&mut fixture);
    let path = fixture.lease.root().join("main.tex");
    fs::write(&path, "New external source\n").unwrap();
    fixture
        .app
        .document_mut()
        .edit(CCursorRange::default(), |source| {
            *source = "Native offline draft\n".into()
        });
    let tab = fixture.app.tabs.active_id().unwrap();
    assert!(
        fixture
            .app
            .submit_overleaf_save(tab, &path, &fixture.context)
    );
    let deadline = Instant::now() + Duration::from_secs(10);
    while fixture.app.overleaf.save.active() || fixture.app.overleaf.io.is_running() {
        fixture.app.poll_overleaf(&fixture.context);
        assert!(Instant::now() < deadline, "rejected save did not finish");
        std::thread::yield_now();
    }
    assert!(
        fixture
            .app
            .overleaf
            .error
            .as_deref()
            .is_some_and(|error| error.contains("preserved"))
    );
    assert!(fixture.app.document().is_dirty());
    assert_eq!(fixture.app.document().source(), "Native offline draft\n");
    assert_eq!(fixture.app.document().saved_source(), "Main old\n");
    assert!(!fixture.app.document_workflow.save_in_flight());
    assert!(fixture.app.overleaf.save.pending().is_none());
    assert!(!fixture.app.overleaf.offline.saved_sources.contains_key("a"));
    assert_eq!(fs::read_to_string(&path).unwrap(), "New external source\n");
}

#[test]
fn reopening_the_same_project_from_another_workspace_uses_its_root_and_account_profile() {
    let mut fixture = persistent_fixture();
    go_offline(&mut fixture);
    let project = fixture
        .app
        .overleaf
        .connection
        .as_ref()
        .unwrap()
        .project
        .clone();
    let other = seed_stored_workspace(
        &fixture.base.parent().unwrap().join("other-workspace"),
        &project,
        [0x34; 16],
        "Same project in a separately saved workspace\n",
    );
    let closing: Vec<_> = fixture.app.tabs.ids().collect();
    fixture.app.close_workspace_tabs(&closing, &fixture.context);
    fixture.app.prune_overleaf_bindings();
    settle(&mut fixture.app, &fixture.context);
    assert!(fixture.app.tabs.is_empty());
    assert_eq!(fixture.app.overleaf.browser_profile, Some([0x12; 16]));
    assert_eq!(
        fixture.app.overleaf.connection.as_ref().unwrap().root,
        fixture.lease.root()
    );

    fixture
        .app
        .open_stored_overleaf_project(&project, other.root(), None, &fixture.context)
        .unwrap();
    finish_stored_open(&mut fixture.app, &fixture.context);

    let connection = fixture.app.overleaf.connection.as_ref().unwrap();
    assert_eq!(connection.project, project);
    assert_eq!(connection.root, other.root());
    assert_eq!(fixture.app.workspace_root, other.root());
    assert_eq!(fixture.app.overleaf.browser_profile, Some([0x34; 16]));
    assert_eq!(
        fixture
            .app
            .overleaf
            .offline
            .session
            .as_ref()
            .unwrap()
            .profile_id,
        [0x34; 16]
    );
    assert_eq!(
        fixture.app.document().path().as_deref(),
        Some(other.root().join("main.tex").as_path())
    );
    assert_eq!(
        fixture.app.document().source(),
        "Same project in a separately saved workspace\n"
    );
    assert!(fixture.app.overleaf.offline.saved_sources.is_empty());
    assert!(fixture.app.overleaf.offline.active());
    assert_eq!(
        WorkspaceSession::load(fixture.lease.root(), &project)
            .unwrap()
            .unwrap()
            .profile_id,
        [0x12; 16]
    );
}

#[test]
fn saved_offline_work_can_leave_for_another_project_and_return_without_publishing() {
    let mut fixture = persistent_fixture();
    go_offline(&mut fixture);
    save_main(&mut fixture, "First workspace's pending offline changes\n");
    let first = fixture
        .app
        .overleaf
        .connection
        .as_ref()
        .unwrap()
        .project
        .clone();
    let second =
        ProjectUrl::parse("https://www.overleaf.com/project/1123456789abcdef01234567").unwrap();
    let other = seed_stored_workspace(
        &fixture
            .base
            .parent()
            .unwrap()
            .join("second-project-workspace"),
        &second,
        [0x34; 16],
        "Second project's independent source\n",
    );

    fixture
        .app
        .open_stored_overleaf_project(&second, other.root(), None, &fixture.context)
        .unwrap();
    finish_stored_open(&mut fixture.app, &fixture.context);

    assert_eq!(fixture.app.workspace_root, other.root());
    assert_eq!(
        fixture.app.overleaf.connection.as_ref().unwrap().project,
        second
    );
    assert_eq!(fixture.app.overleaf.browser_profile, Some([0x34; 16]));
    assert_eq!(
        fixture.app.document().source(),
        "Second project's independent source\n"
    );
    assert!(fixture.app.overleaf.save.pending().is_none());
    assert!(fixture.app.overleaf.browser.is_none());
    assert!(fixture.app.tabs.ids().all(|tab| {
        fixture
            .app
            .document_for_tab(tab)
            .unwrap()
            .path()
            .as_ref()
            .is_none_or(|path| !path.starts_with(fixture.lease.root()))
    }));
    let mut saved =
        OfflineStore::load(fixture.lease.root(), &first, fixture.app.overleaf.epoch + 1)
            .unwrap()
            .unwrap();
    assert_eq!(saved.document("a").unwrap().baseline, "Main old\n");
    assert_eq!(
        saved.document("a").unwrap().source(),
        "First workspace's pending offline changes\n"
    );
    assert!(saved.has_unsynced());
    assert!(saved.publish("a").is_none());

    fixture
        .app
        .open_stored_overleaf_project(&first, fixture.lease.root(), None, &fixture.context)
        .unwrap();
    finish_stored_open(&mut fixture.app, &fixture.context);

    assert_eq!(fixture.app.workspace_root, fixture.lease.root());
    assert_eq!(fixture.app.overleaf.browser_profile, Some([0x12; 16]));
    assert_eq!(
        fixture.app.document().source(),
        "First workspace's pending offline changes\n"
    );
    assert!(!fixture.app.document().is_dirty());
    assert!(fixture.app.overleaf.offline.active());
    assert!(
        fixture
            .app
            .overleaf
            .connection
            .as_ref()
            .unwrap()
            .model
            .has_unsynced()
    );
    assert!(fixture.app.overleaf.save.pending().is_none());
    assert!(fixture.app.overleaf.browser.is_none());
}

#[test]
fn staging_then_reset_and_reconnect_retains_the_current_workspaces_profile() {
    let mut fixture = persistent_fixture();
    let project = fixture
        .app
        .overleaf
        .connection
        .as_ref()
        .unwrap()
        .project
        .clone();
    fixture.app.overleaf.query = project.url().into();
    let original_key = fixture.app.document().key();
    let proposed = fixture.base.parent().unwrap().join("cancelled-workspace");
    fixture
        .app
        .stage_overleaf_workspace(&proposed, &fixture.context);
    assert_ne!(fixture.app.overleaf.browser_profile, Some([0x12; 16]));
    assert!(fixture.app.overleaf.pending_storage.is_some());

    fixture
        .app
        .overleaf
        .reset_storage_draft(fixture.base.parent());
    assert!(fixture.app.overleaf.pending_storage.is_none());
    fixture.app.connect_overleaf(&fixture.context).unwrap();
    settle(&mut fixture.app, &fixture.context);

    assert_eq!(fixture.app.overleaf.browser_profile, Some([0x12; 16]));
    assert_eq!(
        fixture
            .app
            .overleaf
            .offline
            .session
            .as_ref()
            .unwrap()
            .profile_id,
        [0x12; 16]
    );
    let connection = fixture.app.overleaf.connection.as_ref().unwrap();
    assert_eq!(connection.project, project);
    assert_eq!(connection.root, fixture.lease.root());
    assert!(Arc::ptr_eq(
        connection.storage.as_ref().unwrap(),
        &fixture.lease
    ));
    assert_eq!(fixture.app.document().key(), original_key);
    assert_eq!(fixture.app.document().source(), "Main old\n");
    assert!(!proposed.exists());
    assert!(!fixture.app.overleaf.storage_job.is_running());
}

#[test]
fn staging_a_workspace_keeps_owned_passive_commit_heads_without_installing_old_source() {
    let mut fixture = persistent_fixture();
    let root = fixture.lease.root().to_owned();
    let project = fixture
        .app
        .overleaf
        .connection
        .as_ref()
        .unwrap()
        .project
        .clone();
    fixture.app.overleaf.query = project.url().into();
    fixture
        .app
        .update_overleaf_event(
            BrowserEvent::Document {
                epoch: tests::EPOCH,
                id: "a".into(),
                path: "main.tex".into(),
                source: "Remote source committed by the held worker\n".into(),
            },
            &fixture.context,
        )
        .unwrap();
    let work = fixture.app.overleaf.queued.pop_front().unwrap();
    let mut reply = committed_document_reply(&fixture.app, &root, work);
    let written = reply.heads["a"].written;
    let original_profile = reply.origin.profile;
    reply.error = Some("A superseded browser's error".into());
    fixture
        .app
        .document_mut()
        .edit(CCursorRange::default(), |source| {
            source.push_str("Native unsaved edits retained during staging\n");
        });
    let native_source = fixture.app.document().source().to_owned();
    let key = fixture.app.document().key();
    fixture
        .app
        .overleaf
        .connection
        .as_mut()
        .unwrap()
        .model
        .set_draft("a", native_source.clone())
        .unwrap();
    let proposed = fixture.base.parent().unwrap().join("staged-new-workspace");
    fixture
        .app
        .stage_overleaf_workspace(&proposed, &fixture.context);
    assert_ne!(fixture.app.overleaf.browser_profile, original_profile);
    fixture.app.apply_overleaf_mirror(reply, &fixture.context);
    assert_eq!(fixture.app.workspace_root, root);
    assert_eq!(fixture.app.document().key(), key);
    assert_eq!(fixture.app.document().source(), &native_source);
    assert!(fixture.app.document().is_dirty());
    assert!(
        fixture.app.overleaf.error.is_none(),
        "a staged account cannot install an old browser error"
    );
    let connection = fixture.app.overleaf.connection.as_ref().unwrap();
    assert_eq!(connection.root, root);
    assert_eq!(
        connection.model.document("a").unwrap().draft(),
        Some(native_source.as_str())
    );
    assert!(!connection.online());
    let document = connection.model.document("a").unwrap();
    let entry = connection.model.tree().unwrap().entry("a").unwrap();
    let followup = Mirror::open(&root).unwrap().sync_document(entry, document);
    assert!(
        followup.is_ok(),
        "staging discarded the completed physical head {:?}; subsequent CAS failed: {followup:?}",
        document.checkpoint
    );
    assert_eq!(document.checkpoint, Some(written));
    assert_eq!(
        fixture
            .app
            .overleaf
            .offline
            .session
            .as_ref()
            .unwrap()
            .profile_id,
        [0x12; 16]
    );
    assert!(!proposed.exists());
}

#[test]
fn staged_workspace_heads_reject_unrelated_and_unbound_browser_profiles_roots_and_epochs() {
    for wrong in 0..4 {
        let mut fixture = persistent_fixture();
        let root = fixture.lease.root().to_owned();
        let previous = fixture
            .app
            .overleaf
            .connection
            .as_ref()
            .unwrap()
            .model
            .document("a")
            .unwrap()
            .checkpoint;
        let proposed = fixture.base.parent().unwrap().join("staged-workspace");
        fixture
            .app
            .stage_overleaf_workspace(&proposed, &fixture.context);
        let mut reply = Mirrored {
            epoch: fixture.app.overleaf.epoch,
            origin: MirrorOrigin {
                root: root.clone(),
                profile: Some([0x12; 16]),
            },
            hydration_session: 0,
            tree: None,
            error: Some("A rejected receipt cannot install its error".into()),
            files: Vec::new(),
            documents: BTreeMap::new(),
            heads: BTreeMap::from([(
                "a".into(),
                CheckpointAdvance {
                    previous,
                    written: [0x56; 32],
                },
            )]),
        };
        match wrong {
            0 => reply.origin.profile = Some([0x34; 16]),
            1 => reply.origin.profile = fixture.app.overleaf.browser_profile,
            2 => reply.origin.root = root.join("another-workspace"),
            _ => reply.epoch += 1,
        }
        fixture.app.apply_overleaf_mirror(reply, &fixture.context);
        assert_eq!(
            fixture
                .app
                .overleaf
                .connection
                .as_ref()
                .unwrap()
                .model
                .document("a")
                .unwrap()
                .checkpoint,
            previous,
            "wrong domain {wrong}"
        );
        assert!(fixture.app.overleaf.error.is_none());
    }
}

#[test]
fn a_staged_owned_old_receipt_cannot_roll_back_a_later_committed_head() {
    let mut fixture = persistent_fixture();
    let root = fixture.lease.root().to_owned();
    let work = MirrorWork {
        checkpoint: false,
        entry: tests::entry("a", "main.tex", EntryKind::Doc),
        data: MirrorData::Document("Main old\n".into()),
        save: None,
    };
    let old = committed_document_reply(&fixture.app, &root, work);
    let first = old.heads["a"].written;
    fixture
        .app
        .overleaf
        .connection
        .as_mut()
        .unwrap()
        .model
        .record_checkpoint("a", first);
    fixture
        .app
        .update_overleaf_event(
            BrowserEvent::Document {
                epoch: tests::EPOCH,
                id: "a".into(),
                path: "main.tex".into(),
                source: "A later committed remote baseline\n".into(),
            },
            &fixture.context,
        )
        .unwrap();
    let later_work = fixture.app.overleaf.queued.pop_front().unwrap();
    let latest = committed_document_reply(&fixture.app, &root, later_work);
    let written = latest.heads["a"].written;
    assert_ne!(first, written);
    fixture.app.apply_overleaf_mirror(latest, &fixture.context);
    let key = fixture.app.document().key();
    let source = fixture.app.document().source().to_owned();
    let proposed = fixture.base.parent().unwrap().join("staged-workspace");
    fixture
        .app
        .stage_overleaf_workspace(&proposed, &fixture.context);
    fixture.app.apply_overleaf_mirror(old, &fixture.context);
    assert_eq!(
        fixture
            .app
            .overleaf
            .connection
            .as_ref()
            .unwrap()
            .model
            .document("a")
            .unwrap()
            .checkpoint,
        Some(written)
    );
    assert_eq!(fixture.app.document().key(), key);
    assert_eq!(fixture.app.document().source(), &source);
}

#[test]
fn passive_work_queued_before_staging_keeps_its_bound_workspace_profile() {
    let mut fixture = persistent_fixture();
    fixture
        .app
        .update_overleaf_event(
            BrowserEvent::Document {
                epoch: tests::EPOCH,
                id: "a".into(),
                path: "main.tex".into(),
                source: "An old workspace's queued remote source\n".into(),
            },
            &fixture.context,
        )
        .unwrap();
    let key = fixture.app.document().key();
    let source = fixture.app.document().source().to_owned();
    let proposed = fixture.base.parent().unwrap().join("staged-workspace");
    fixture
        .app
        .stage_overleaf_workspace(&proposed, &fixture.context);
    assert_ne!(fixture.app.overleaf.browser_profile, Some([0x12; 16]));
    fixture.app.poll_overleaf(&fixture.context);
    let deadline = Instant::now() + Duration::from_secs(10);
    let reply = loop {
        match fixture.app.overleaf.io.poll() {
            LatestJobPoll::Ready(reply) => break reply,
            LatestJobPoll::Failed(error) => panic!("queued mirror failed: {error}"),
            _ => {}
        }
        assert!(Instant::now() < deadline, "queued mirror did not finish");
        std::thread::yield_now();
    };
    assert_eq!(reply.origin.profile, Some([0x12; 16]));
    let written = reply.heads["a"].written;
    fixture.app.apply_overleaf_mirror(reply, &fixture.context);
    assert_eq!(
        fixture
            .app
            .overleaf
            .connection
            .as_ref()
            .unwrap()
            .model
            .document("a")
            .unwrap()
            .checkpoint,
        Some(written)
    );
    assert_eq!(fixture.app.document().key(), key);
    assert_eq!(fixture.app.document().source(), &source);
    assert!(
        !fixture
            .app
            .overleaf
            .offline
            .recovery_sources
            .contains_key("a")
    );
    settle(&mut fixture.app, &fixture.context);
}

#[test]
fn offline_saves_after_staging_complete_only_their_owned_receipts() {
    for newer_edit in [false, true] {
        let mut fixture = persistent_fixture();
        go_offline(&mut fixture);
        let root = fixture.lease.root().to_owned();
        let proposed = fixture.base.parent().unwrap().join("staged-workspace");
        fixture
            .app
            .stage_overleaf_workspace(&proposed, &fixture.context);
        let staged_profile = fixture.app.overleaf.browser_profile;
        assert!(fixture.app.overleaf.offline.active());
        assert!(!fixture.app.overleaf_source_locked());
        let tab = fixture.app.tabs.active_id().unwrap();
        fixture
            .app
            .document_mut()
            .edit(CCursorRange::default(), |source| {
                *source = "Saved in the retained offline workspace\n".into()
            });
        assert!(
            fixture
                .app
                .submit_overleaf_local_save(tab, &fixture.context)
        );
        let operation = fixture.app.document_workflow.current_save().unwrap().id;
        let work = fixture.app.overleaf.queued.pop_front().unwrap();
        let mut reply = committed_document_reply(&fixture.app, &root, work);
        assert_eq!(reply.origin.profile, Some([0x12; 16]));
        reply.error = Some("An old browser batch error".into());
        reply.tree = Some(
            fixture
                .app
                .overleaf
                .connection
                .as_ref()
                .unwrap()
                .model
                .tree()
                .unwrap()
                .clone(),
        );
        reply.files.push((
            MirrorWork {
                checkpoint: false,
                entry: tests::entry("b", "chapter.tex", EntryKind::Doc),
                data: MirrorData::Document("Chapter old\n".into()),
                save: None,
            },
            Err("A passive member cannot install an error".into()),
        ));
        fixture.app.overleaf.open_after = Some("a".into());
        if newer_edit {
            fixture
                .app
                .document_mut()
                .edit(CCursorRange::default(), |source| {
                    source.push_str("Newer unsaved typing\n")
                });
            let source = fixture.app.document().source().to_owned();
            fixture
                .app
                .overleaf
                .connection
                .as_mut()
                .unwrap()
                .model
                .set_draft("a", source)
                .unwrap();
        }
        let source = fixture.app.document().source().to_owned();
        let key = fixture.app.document().key();
        fixture.app.apply_overleaf_mirror(reply, &fixture.context);
        assert!(
            !fixture.app.overleaf.save.active(),
            "the bound offline save operation must retire after staging"
        );
        assert!(!fixture.app.document_workflow.save_in_flight());
        assert!(!fixture.app.document_workflow.end_save(operation));
        assert_eq!(fixture.app.document().source(), &source);
        assert_eq!(fixture.app.document().key(), key);
        assert_eq!(fixture.app.document().is_dirty(), newer_edit);
        assert_eq!(fixture.app.overleaf.browser_profile, staged_profile);
        assert_eq!(fixture.app.workspace_root, root);
        assert_eq!(fixture.app.overleaf.open_after.as_deref(), Some("a"));
        assert!(fixture.app.overleaf.error.is_none());
        assert!(!fixture.app.overleaf.blocked.contains("b"));
        assert!(!fixture.app.overleaf.offline.disk_untrusted.contains("b"));
        assert!(
            fixture
                .app
                .overleaf
                .connection
                .as_ref()
                .unwrap()
                .model
                .has_unsynced()
        );
        assert_eq!(
            fs::read_to_string(root.join("main.tex")).unwrap(),
            "Saved in the retained offline workspace\n"
        );
    }
}

#[test]
fn a_failed_offline_save_after_staging_retires_only_its_ticket_and_keeps_dirty_source() {
    let mut fixture = persistent_fixture();
    go_offline(&mut fixture);
    let root = fixture.lease.root().to_owned();
    let proposed = fixture.base.parent().unwrap().join("staged-workspace");
    fixture
        .app
        .stage_overleaf_workspace(&proposed, &fixture.context);
    let tab = fixture.app.tabs.active_id().unwrap();
    fixture
        .app
        .document_mut()
        .edit(CCursorRange::default(), |source| {
            source.push_str("Offline save must report its failure\n")
        });
    let source = fixture.app.document().source().to_owned();
    assert!(
        fixture
            .app
            .submit_overleaf_local_save(tab, &fixture.context)
    );
    let work = fixture.app.overleaf.queued.pop_front().unwrap();
    let journal = root
        .join(".tiptoptyp/overleaf-sync/documents")
        .join(format!(
            "{}.json",
            hex::encode(<sha2::Sha256 as sha2::Digest>::digest(b"a"))
        ));
    fs::remove_file(&journal).unwrap();
    fs::create_dir(&journal).unwrap();
    let connection = fixture.app.overleaf.connection.as_ref().unwrap();
    let error = Mirror::open(&root)
        .unwrap()
        .sync_document(&work.entry, connection.model.document("a").unwrap())
        .unwrap_err();
    let reply = Mirrored {
        epoch: fixture.app.overleaf.epoch,
        origin: MirrorOrigin {
            root: root.clone(),
            profile: Some([0x12; 16]),
        },
        hydration_session: connection.hydration.session,
        tree: None,
        error: None,
        files: vec![(work, Err(error))],
        documents: BTreeMap::new(),
        heads: BTreeMap::new(),
    };
    fixture.app.apply_overleaf_mirror(reply, &fixture.context);
    assert!(!fixture.app.overleaf.save.active());
    assert!(!fixture.app.document_workflow.save_in_flight());
    assert!(fixture.app.document().is_dirty());
    assert_eq!(fixture.app.document().source(), &source);
    assert!(
        fixture
            .app
            .overleaf
            .error
            .as_deref()
            .is_some_and(|error| error.contains("could not be cached"))
    );
    assert_eq!(
        fs::read_to_string(root.join("main.tex")).unwrap(),
        "Main old\n"
    );
}

#[test]
fn a_stale_staged_offline_save_cannot_release_a_newer_operation() {
    let mut fixture = persistent_fixture();
    go_offline(&mut fixture);
    let root = fixture.lease.root().to_owned();
    let proposed = fixture.base.parent().unwrap().join("staged-workspace");
    fixture
        .app
        .stage_overleaf_workspace(&proposed, &fixture.context);
    let tab = fixture.app.tabs.active_id().unwrap();
    fixture
        .app
        .document_mut()
        .edit(CCursorRange::default(), |source| {
            *source = "First saved snapshot\n".into()
        });
    assert!(
        fixture
            .app
            .submit_overleaf_local_save(tab, &fixture.context)
    );
    let first_operation = fixture.app.document_workflow.current_save().unwrap().id;
    let work = fixture.app.overleaf.queued.pop_front().unwrap();
    let old = committed_document_reply(&fixture.app, &root, work);
    assert!(fixture.app.finish_overleaf_save(first_operation));
    fixture
        .app
        .document_mut()
        .edit(CCursorRange::default(), |source| {
            *source = "Second saved snapshot\n".into()
        });
    assert!(
        fixture
            .app
            .submit_overleaf_local_save(tab, &fixture.context)
    );
    let second_operation = fixture.app.document_workflow.current_save().unwrap().id;
    assert_ne!(first_operation, second_operation);
    fixture.app.apply_overleaf_mirror(old, &fixture.context);
    assert_eq!(
        fixture.app.document_workflow.current_save().unwrap().id,
        second_operation
    );
    assert_eq!(
        fixture.app.overleaf.save.operation(),
        Some(second_operation)
    );
    assert_eq!(fixture.app.document().source(), "Second saved snapshot\n");
    assert!(fixture.app.document().is_dirty());
    let work = fixture.app.overleaf.queued.pop_front().unwrap();
    let current = committed_document_reply(&fixture.app, &root, work);
    fixture.app.apply_overleaf_mirror(current, &fixture.context);
    assert!(!fixture.app.document_workflow.save_in_flight());
    assert!(!fixture.app.overleaf.save.active());
    assert!(!fixture.app.document().is_dirty());
}

#[test]
fn repeated_idle_session_polls_do_not_start_a_worker_or_replace_its_checkpoint_file() {
    use std::os::unix::fs::MetadataExt as _;
    let mut fixture = persistent_fixture();
    go_offline(&mut fixture);
    let path = fixture
        .lease
        .root()
        .join(crate::overleaf::workspace_session::SESSION_PATH);
    let bytes = fs::read(&path).unwrap();
    let before = fs::metadata(&path).unwrap();
    let identity = (
        before.dev(),
        before.ino(),
        before.mtime(),
        before.mtime_nsec(),
    );

    for _ in 0..128 {
        fixture.app.poll_overleaf_session(&fixture.context);
        assert!(
            fixture.app.overleaf.offline.saved(),
            "idle polling admitted a metadata worker"
        );
        let after = fs::metadata(&path).unwrap();
        assert_eq!(
            (after.dev(), after.ino(), after.mtime(), after.mtime_nsec()),
            identity
        );
    }
    assert_eq!(fs::read(&path).unwrap(), bytes);
}

#[test]
fn a_failed_session_checkpoint_stays_unsaved_and_does_not_retry_on_idle_polls() {
    let mut fixture = persistent_fixture();
    settle(&mut fixture.app, &fixture.context);
    let path = fixture
        .lease
        .root()
        .join(crate::overleaf::workspace_session::SESSION_PATH);
    let saved = fs::read(&path).unwrap();
    fs::remove_file(&path).unwrap();
    fs::create_dir(&path).unwrap();

    fixture.app.overleaf_work_offline(&fixture.context).unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    while fixture.app.wait_for_overleaf_session(&fixture.context) {
        assert!(!fixture.app.overleaf.offline.saved());
        assert!(
            Instant::now() < deadline,
            "failed checkpoint did not finish"
        );
        std::thread::yield_now();
    }
    assert!(fixture.app.overleaf.offline.active());
    assert!(!fixture.app.overleaf.offline.saved());
    let error = fixture.app.overleaf.error.clone().unwrap();
    assert!(error.contains("Could not remember the Overleaf workspace"));
    for _ in 0..32 {
        assert!(
            !fixture.app.wait_for_overleaf_session(&fixture.context),
            "an unchanged failure must not spawn another worker"
        );
        assert!(
            !fixture.app.overleaf.offline.saved(),
            "failure must never be reported as saved"
        );
        assert_eq!(fixture.app.overleaf.error.as_ref(), Some(&error));
    }
    assert!(path.is_dir());
    fs::remove_dir(&path).unwrap();
    fs::write(&path, saved).unwrap();
}

#[test]
fn an_online_restored_empty_workspace_stays_empty_after_fresh_project_hydration() {
    let mut fixture = persistent_fixture();
    go_offline(&mut fixture);
    let closing: Vec<_> = fixture.app.tabs.ids().collect();
    fixture.app.close_workspace_tabs(&closing, &fixture.context);
    fixture.app.prune_overleaf_bindings();
    settle(&mut fixture.app, &fixture.context);
    let project = &fixture.app.overleaf.connection.as_ref().unwrap().project;
    let mut session = WorkspaceSession::load(fixture.lease.root(), project)
        .unwrap()
        .unwrap();
    assert!(session.has_page_state);
    assert!(session.open_documents.is_empty());
    session.offline = false;
    session.write(fixture.lease.root(), project).unwrap();

    let mut restored = reopen(&fixture);
    assert!(restored.overleaf.offline.restored);
    assert!(!restored.overleaf.offline.active());
    assert!(restored.overleaf.awaiting_project());
    assert!(restored.tabs.is_empty());
    let epoch = restored.overleaf.epoch;
    restored
        .update_overleaf_event(
            BrowserEvent::Project {
                epoch,
                project_id: PROJECT.into(),
                name: "Refreshed saved workspace".into(),
                entries: vec![
                    tests::entry("a", "main.tex", EntryKind::Doc),
                    tests::entry("b", "chapter.tex", EntryKind::Doc),
                ],
                root_doc: Some("a".into()),
            },
            &fixture.context,
        )
        .unwrap();
    assert!(restored.overleaf.open_after.is_none());
    for (id, path, source) in [
        ("a", "main.tex", "Main refreshed remotely\n"),
        ("b", "chapter.tex", "Chapter refreshed remotely\n"),
    ] {
        restored
            .update_overleaf_event(
                BrowserEvent::Document {
                    epoch,
                    id: id.into(),
                    path: path.into(),
                    source: source.into(),
                },
                &fixture.context,
            )
            .unwrap();
    }
    settle(&mut restored, &fixture.context);

    let connection = restored.overleaf.connection.as_ref().unwrap();
    assert!(connection.online());
    assert!(!connection.hydration.pending());
    assert!(restored.tabs.is_empty());
    assert!(restored.tabs.active_id().is_none());
    assert!(restored.tabs.preview_id().is_none());
    assert!(restored.overleaf.open_after.is_none());
    assert_eq!(
        fs::read_to_string(fixture.lease.root().join("main.tex")).unwrap(),
        "Main refreshed remotely\n"
    );
    let session = WorkspaceSession::load(fixture.lease.root(), project)
        .unwrap()
        .unwrap();
    assert!(session.has_page_state);
    assert!(session.open_documents.is_empty());
}

#[test]
fn discarded_typing_is_checkpointed_before_departure_and_stays_discarded_on_reopen() {
    let mut fixture = persistent_fixture();
    let base = "Opening.\nMiddle.\nClosing.\n";
    fixture
        .app
        .update_overleaf_event(
            BrowserEvent::Document {
                epoch: fixture.app.overleaf.epoch,
                id: "a".into(),
                path: "main.tex".into(),
                source: base.into(),
            },
            &fixture.context,
        )
        .unwrap();
    settle(&mut fixture.app, &fixture.context);
    go_offline(&mut fixture);
    let main = save_main(&mut fixture, "Opening saved.\nMiddle.\nClosing.\n");
    fixture
        .app
        .document_mut()
        .edit(CCursorRange::default(), |source| {
            *source = "Opening saved.\nMiddle unsaved.\nClosing.\n".into();
        });
    // Reconnecting receives an independent remote edit and checkpoints the
    // merged unsaved typing for recovery without making it a user Save.
    fixture.app.resume_overleaf(&fixture.context).unwrap();
    let epoch = fixture.app.overleaf.epoch;
    fixture
        .app
        .update_overleaf_event(
            BrowserEvent::Project {
                epoch,
                project_id: PROJECT.into(),
                name: "Recovery checkpoint".into(),
                entries: vec![
                    tests::entry("a", "main.tex", EntryKind::Doc),
                    tests::entry("b", "chapter.tex", EntryKind::Doc),
                ],
                root_doc: Some("a".into()),
            },
            &fixture.context,
        )
        .unwrap();
    for (id, path, source) in [
        ("a", "main.tex", "Opening.\nMiddle.\nClosing remote.\n"),
        ("b", "chapter.tex", "Chapter old\n"),
    ] {
        fixture
            .app
            .update_overleaf_event(
                BrowserEvent::Document {
                    epoch,
                    id: id.into(),
                    path: path.into(),
                    source: source.into(),
                },
                &fixture.context,
            )
            .unwrap();
    }
    settle(&mut fixture.app, &fixture.context);
    assert!(fixture.app.document().is_dirty());
    assert_eq!(
        fixture.app.document().source(),
        "Opening saved.\nMiddle unsaved.\nClosing remote.\n"
    );
    assert_eq!(
        fs::read_to_string(fixture.lease.root().join("main.tex")).unwrap(),
        fixture.app.document().source().as_str()
    );
    go_offline(&mut fixture);
    let first = fixture
        .app
        .overleaf
        .connection
        .as_ref()
        .unwrap()
        .project
        .clone();
    let second =
        ProjectUrl::parse("https://www.overleaf.com/project/1123456789abcdef01234567").unwrap();
    let other = seed_stored_workspace(
        &fixture.base.parent().unwrap().join("after-discard"),
        &second,
        [0x34; 16],
        "Other project's saved source\n",
    );
    fixture
        .app
        .open_stored_overleaf_project(&second, other.root(), None, &fixture.context)
        .unwrap();
    let Some(AppModal::Unsaved { mut pending, .. }) = fixture.app.document_workflow.take_modal()
    else {
        panic!("the newer unsaved typing requires a departure decision");
    };
    assert!(matches!(
        pending.action,
        DeferredDocumentAction::ApproveWorkspaceSwitch
    ));
    fixture.app.document_workflow.clear_modal();
    fixture.app.document_workflow.cancel_continuation();
    pending.key = fixture.app.document().key();
    pending.allow_discard = true;
    fixture.app.document_workflow.queue_action(pending);
    fixture
        .app
        .execute_pending_document_action(&fixture.context, None);

    assert_eq!(fixture.app.workspace_root, fixture.lease.root());
    assert!(fixture.app.tabs.index_of(main).is_some());
    assert!(fixture.app.workspace_switch.active());
    assert!(
        fixture.app.overleaf.io.is_running()
            || fixture
                .app
                .overleaf
                .queued
                .iter()
                .any(|work| work.checkpoint),
        "departure must wait for the rollback checkpoint"
    );
    finish_stored_open(&mut fixture.app, &fixture.context);
    assert_eq!(fixture.app.workspace_root, other.root());
    let expected = "Opening saved.\nMiddle.\nClosing remote.\n";
    assert_eq!(
        fs::read_to_string(fixture.lease.root().join("main.tex")).unwrap(),
        expected
    );

    fixture
        .app
        .open_stored_overleaf_project(&first, fixture.lease.root(), None, &fixture.context)
        .unwrap();
    finish_stored_open(&mut fixture.app, &fixture.context);
    assert_eq!(fixture.app.workspace_root, fixture.lease.root());
    assert_eq!(fixture.app.document().source(), expected);
    assert!(!fixture.app.document().is_dirty());
    assert!(fixture.app.overleaf.offline.active());
    assert!(fixture.app.overleaf.save.pending().is_none());
    assert!(fixture.app.overleaf.browser.is_none());
    assert_eq!(fixture.app.overleaf.offline.saved_sources["a"], expected);
}

#[test]
fn restoring_many_pages_is_bounded_per_frame_and_finishes_with_saved_order_and_selection() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().canonicalize().unwrap();
    let context = egui::Context::default();
    let project =
        ProjectUrl::parse(&format!("https://www.overleaf.com/project/{PROJECT}")).unwrap();
    let entries: Vec<_> = (0..11)
        .map(|index| {
            tests::entry(
                &format!("doc{index:02}"),
                &format!("chapter{index:02}.tex"),
                EntryKind::Doc,
            )
        })
        .collect();
    let mut model = Model::for_project(tests::EPOCH, &project);
    model
        .apply(BrowserEvent::Project {
            epoch: tests::EPOCH,
            project_id: PROJECT.into(),
            name: "Many saved pages".into(),
            entries: entries.clone(),
            root_doc: Some(entries[0].id.clone()),
        })
        .unwrap();
    for (index, entry) in entries.iter().enumerate() {
        let source = format!("Saved chapter {index}.\n");
        fs::write(root.join(&entry.path), &source).unwrap();
        model
            .apply(BrowserEvent::Document {
                epoch: tests::EPOCH,
                id: entry.id.clone(),
                path: entry.path.clone(),
                source,
            })
            .unwrap();
    }
    let order = [7, 1, 10, 0, 5, 9, 3, 8, 4, 6, 2];
    let expected: Vec<_> = order
        .iter()
        .map(|&index| entries[index].id.clone())
        .collect();
    let mut session = WorkspaceSession::new(&project, [0x12; 16]);
    session.offline = true;
    session.has_page_state = true;
    session.open_documents = expected.clone();
    session.active_document = Some(entries[9].id.clone());
    // The designated preview is deliberately in the final batch.
    session.preview_document = Some(entries[2].id.clone());
    session.write(&root, &project).unwrap();
    let mut app = EditorApp::dormant_for_tests(&context, root.clone());
    app.empty_workspace(&context);
    app.overleaf.epoch = tests::EPOCH;
    app.overleaf.connection = Some(Connection {
        project,
        root,
        model,
        status: "Offline".into(),
        transport: Transport::Offline,
        storage: None,
        hydration: Hydration::default(),
    });
    app.overleaf.offline.install(session, true);

    for expected_count in [4, 8, 11] {
        let before_frame = app.tabs.len();
        let output = context.run_ui(egui::RawInput::default(), |ui| {
            let context = ui.ctx();
            let before_call = app.tabs.len();
            app.restore_overleaf_pages(context);
            let after_call = app.tabs.len();
            assert!(after_call - before_call <= 4);
            app.restore_overleaf_pages(context);
            assert_eq!(
                app.tabs.len(),
                after_call,
                "another call in the same frame must not admit another batch"
            );
        });
        output.drop_without_applying_deltas();
        assert!(app.tabs.len() - before_frame <= 4);
        assert_eq!(app.tabs.len(), expected_count);
        assert_eq!(app.overleaf.offline.restoring_pages(), expected_count < 11);
    }

    let actual: Vec<_> = app
        .tabs
        .ids()
        .map(|tab| app.overleaf_entity_for_tab(tab).unwrap().to_owned())
        .collect();
    assert_eq!(actual, expected);
    assert_eq!(
        app.overleaf_entity_for_tab(app.tabs.active_id().unwrap()),
        Some("doc09")
    );
    assert_eq!(
        app.overleaf_entity_for_tab(app.tabs.preview_id().unwrap()),
        Some("doc02")
    );
}
