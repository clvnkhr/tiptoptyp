use super::*;
use egui_kittest::{
    Harness,
    kittest::{NodeT as _, Queryable as _},
};

fn draft(parent: &Path) -> Draft {
    Draft {
        name: "Research notes".into(),
        parent: parent.to_string_lossy().into_owned(),
        use_overleaf: false,
        error: None,
        focus: false,
    }
}

#[test]
fn new_workspace_request_validates_every_option_before_creating_a_folder() {
    let mut draft = draft(Path::new("/projects"));
    assert_eq!(
        draft.request().unwrap(),
        (PathBuf::from("/projects"), "Research notes".into())
    );
    draft.parent = "/projects/parent with trailing space ".into();
    assert_eq!(draft.request().unwrap().0, PathBuf::from(&draft.parent));
    draft.parent = "/projects".into();
    for name in ["", " ", "../escape", "nested/folder", ".tiptoptyp"] {
        draft.name = name.into();
        assert!(draft.request().is_err(), "{name}");
    }
    draft.name = " Paper ".into();
    draft.parent = "relative/folder".into();
    assert!(draft.request().is_err());
    draft.parent = "/projects".into();
    draft.use_overleaf = true;
    let (parent, name) = draft.request().unwrap();
    assert_eq!(parent, Path::new("/projects"));
    assert_eq!(name, "Paper");
}

#[test]
fn new_workspace_checkbox_keeps_the_form_unchanged_and_disables_mutations_while_busy() {
    let mut harness = Harness::builder()
        .with_size(Vec2::new(520.0, 500.0))
        .build_ui_state(
            |ui, state: &mut (Draft, bool, Action)| {
                let action = show(ui, &mut state.0, state.1);
                state.2.create |= action.create;
                state.2.cancel |= action.cancel;
                state.2.choose_parent |= action.choose_parent;
            },
            (draft(Path::new("/projects")), false, Action::default()),
        );
    harness.run();
    assert!(harness.query_by_label("Overleaf project URL").is_none());
    let submit_rect = harness.get_by_label("Create workspace").rect();
    harness.get_by_label("Use for an Overleaf project").click();
    harness.run();
    assert!(harness.state().0.use_overleaf);
    assert!(harness.query_by_label("Overleaf project URL").is_none());
    assert_eq!(harness.get_by_label("Create workspace").rect(), submit_rect);
    harness.get_by_label("Create workspace").click();
    harness.run();
    assert!(harness.state().2.create);
    harness.state_mut().1 = true;
    harness.run();
    for label in [
        "Name",
        "Parent folder",
        "Use for an Overleaf project",
        "Create workspace",
        "Cancel",
        "Choose…",
    ] {
        assert!(
            harness.get_by_label(label).accesskit_node().is_disabled(),
            "{label}"
        );
    }
}

fn finish(app: &mut EditorApp, context: &egui::Context) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while app.workspace_creation.busy() {
        app.poll_workspace_creation(context);
        assert!(Instant::now() < deadline);
        std::thread::yield_now();
    }
}

#[test]
fn new_workspace_submit_never_overwrites_and_leaves_creation_errors_editable() {
    let parent = tempfile::tempdir().unwrap();
    let parent = parent.path().canonicalize().unwrap();
    let context = egui::Context::default();
    let mut app = EditorApp::dormant_for_tests(&context, parent.clone());
    app.workspace_creation.draft = Some(draft(&parent));
    assert!(!parent.join("Research notes").exists());
    app.workspace_creation.draft = None; // Cancel has no filesystem effect.
    assert!(!parent.join("Research notes").exists());
    fs::create_dir(parent.join("Research notes")).unwrap();
    fs::write(parent.join("Research notes/keep.txt"), "keep").unwrap();
    app.workspace_creation.draft = Some(draft(&parent));
    app.submit_workspace_creation(&context);
    finish(&mut app, &context);
    assert!(
        app.workspace_creation
            .draft
            .as_ref()
            .unwrap()
            .error
            .as_ref()
            .unwrap()
            .contains("already exists")
    );
    assert_eq!(
        fs::read_to_string(parent.join("Research notes/keep.txt")).unwrap(),
        "keep"
    );
    app.workspace_creation.draft.as_mut().unwrap().name = "Fresh workspace".into();
    app.submit_workspace_creation(&context);
    finish(&mut app, &context);
    assert!(app.workspace_creation.draft.is_none());
    assert!(parent.join("Fresh workspace").is_dir());
    assert!(app.document_workflow.take_action().is_some());
}

#[test]
fn new_local_workspace_requires_unsaved_confirmation_before_switching() {
    let parent = tempfile::tempdir().unwrap();
    let parent = parent.path().canonicalize().unwrap();
    let context = egui::Context::default();
    let mut app = EditorApp::dormant_for_tests(&context, parent.clone());
    app.document_mut().replace_unprojected_untitled("");
    app.document_mut().edit(CCursorRange::default(), |source| {
        source.push_str("Unsaved local notes");
    });
    assert!(app.document().is_dirty());
    let key = app.document().key();
    app.workspace_creation.draft = Some(draft(&parent));
    app.submit_workspace_creation(&context);
    finish(&mut app, &context);
    // Creation queues the selected folder; dispatching it checks every page
    // owned by the departing workspace before changing the active root.
    app.execute_pending_document_action(&context, None);

    let created = parent.join("Research notes");
    assert!(created.is_dir());
    assert!(app.workspace_creation.draft.is_none());
    assert_eq!(app.workspace_root, parent);
    assert_eq!(app.document().key(), key);
    assert_eq!(app.document().source(), "Unsaved local notes");
    assert!(app.document().is_dirty());
    let Some(AppModal::Unsaved { pending, .. }) = app.document_workflow.modal() else {
        panic!("opening the created workspace must ask about the unsaved source");
    };
    assert_eq!(pending.key, key);
    assert!(!pending.allow_discard);
    assert!(matches!(
        &pending.action,
        DeferredDocumentAction::ApproveWorkspaceSwitch
    ));
    assert!(app.document_workflow.take_action().is_none());
}

#[test]
fn new_workspace_overleaf_option_stages_storage_without_connecting_or_replacing_source() {
    let parent = tempfile::tempdir().unwrap();
    let parent = parent.path().canonicalize().unwrap();
    let context = egui::Context::default();
    let mut app = EditorApp::dormant_for_tests(&context, parent.clone());
    app.document_mut()
        .replace_unprojected_untitled("unsaved source");
    let key = app.document().key();
    let mut request = draft(&parent);
    request.use_overleaf = true;
    app.workspace_creation.draft = Some(request);
    app.submit_workspace_creation(&context);
    finish(&mut app, &context);
    assert!(app.workspace_creation.draft.is_none());
    assert!(app.overleaf.visible);
    assert_eq!(app.workspace_root, parent);
    assert_eq!(app.document().source(), "unsaved source");
    assert_eq!(app.document().key(), key);
    assert_eq!(
        fs::read_dir(parent.join("Research notes")).unwrap().count(),
        0
    );
    assert!(app.document_workflow.take_action().is_none());
    assert!(
        app.settings
            .existing_recent_workspaces()
            .contains(&parent.join("Research notes"))
    );
}

#[test]
fn created_workspace_waits_for_a_save_admitted_after_the_form_opened() {
    for domain in [
        crate::workflow::SaveDomain::Local,
        crate::workflow::SaveDomain::Overleaf,
        crate::workflow::SaveDomain::Offline,
    ] {
        for use_overleaf in [false, true] {
            let directory = tempfile::tempdir().unwrap();
            let parent = directory.path().canonicalize().unwrap();
            let context = egui::Context::default();
            let mut app = EditorApp::dormant_for_tests(&context, parent.clone());
            app.settings.auto_save = false;
            app.document_mut()
                .replace_unprojected_untitled("Unsaved original source");
            let key = app.document().key();
            app.begin_workspace_creation();
            let request = app.workspace_creation.draft.as_mut().unwrap();
            request.name = "Created while saving".into();
            request.parent = parent.to_string_lossy().into_owned();
            request.use_overleaf = use_overleaf;

            // Only the actual creation worker can request this context's next
            // repaint; it publishes its ready result before that notification.
            for _ in 0..3 {
                context
                    .run_ui(Default::default(), |_| {})
                    .drop_without_applying_deltas();
            }
            let (ready, completed) = std::sync::mpsc::channel();
            context.set_request_repaint_callback(move |_| {
                let _ = ready.send(());
            });
            app.submit_workspace_creation(&context);
            let operation = app.document_workflow.begin_save(key, domain).unwrap();
            completed.recv_timeout(Duration::from_secs(10)).unwrap();
            let created = parent.join("Created while saving");
            assert!(created.is_dir());
            for _ in 0..2 {
                app.poll_workspace_creation(&context);
                assert!(
                    app.workspace_creation.busy(),
                    "{domain:?}: ready result must remain owned while saving"
                );
                assert!(app.workspace_creation.draft.is_some());
                assert!(!app.overleaf.visible);
                assert!(app.overleaf.browser.as_ref().is_none());
                assert_eq!(app.workspace_root, parent);
                assert_eq!(app.document().key(), key);
                assert_eq!(app.document().source(), "Unsaved original source");
                assert!(app.document_workflow.take_action().is_none());
            }
            let mut unrelated = crate::workflow::DocumentWorkflow::default();
            let unrelated_key = DocumentKey {
                owner: if key.owner == tiptoptyp_core::document::WindowSessionId::new(0) {
                    tiptoptyp_core::document::WindowSessionId::new(1)
                } else {
                    tiptoptyp_core::document::WindowSessionId::new(0)
                },
                ..key
            };
            let stale = unrelated.begin_save(unrelated_key, domain).unwrap();
            assert!(!app.document_workflow.end_save(stale));
            app.poll_workspace_creation(&context);
            assert!(app.workspace_creation.busy());

            // Save backends retire this same ticket on success, failure or
            // cancellation. The ready creation does not depend on that outcome.
            assert!(app.document_workflow.end_save(operation));
            finish(&mut app, &context);
            assert!(app.workspace_creation.draft.is_none());
            assert_eq!(app.workspace_root, parent);
            assert_eq!(app.document().key(), key);
            assert_eq!(app.document().source(), "Unsaved original source");
            if use_overleaf {
                assert!(app.overleaf.visible);
                assert!(app.document_workflow.take_action().is_none());
            } else {
                assert!(!app.overleaf.visible);
                assert!(app.overleaf.browser.as_ref().is_none());
                assert!(app.document_workflow.take_action().is_some());
            }
        }
    }
}
