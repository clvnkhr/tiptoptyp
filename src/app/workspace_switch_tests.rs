//! Workspace navigation owns the full old tab set, including parked drafts.
use super::*;

struct Fixture {
    _directory: tempfile::TempDir,
    old: PathBuf,
    new: PathBuf,
    first: PathBuf,
    second: PathBuf,
    first_tab: u64,
    second_tab: u64,
    app: EditorApp,
    context: egui::Context,
}

fn fixture() -> Fixture {
    let directory = tempfile::tempdir().unwrap();
    let base = directory.path().canonicalize().unwrap();
    let old = base.join("old-workspace");
    let new = base.join("new-workspace");
    fs::create_dir(&old).unwrap();
    fs::create_dir(&new).unwrap();
    let first = old.join("first.txt");
    let second = old.join("second.txt");
    fs::write(&first, "First saved source\n").unwrap();
    fs::write(&second, "Second saved source\n").unwrap();
    let context = egui::Context::default();
    let mut settings = AppSettings {
        auto_save: false,
        ..AppSettings::default()
    };
    settings.remember_workspace(&old);
    let mut app = EditorApp::new_session(
        &context,
        egui::ViewportId::ROOT,
        Some(first.clone()),
        CaptureController::disabled_for_tests(),
        None,
        None,
        settings,
        EditorWindowHost::Root,
        DocumentLifecycle::Dormant,
    );
    let first_tab = app.tabs.active_id().unwrap();
    app.lifecycle = DocumentLifecycle::Active;
    assert!(app.open_tab_path(second.clone(), &context));
    app.lifecycle = DocumentLifecycle::Dormant;
    let second_tab = app.tabs.active_id().unwrap();
    assert_ne!(first_tab, second_tab);
    assert_eq!(app.workspace_root, old);
    assert_eq!(app.tabs.len(), 2);
    Fixture {
        _directory: directory,
        old,
        new,
        first,
        second,
        first_tab,
        second_tab,
        app,
        context,
    }
}

fn request_workspace(app: &mut EditorApp, root: PathBuf, context: &egui::Context) {
    app.queue_open_path(root);
    for _ in 0..8 {
        app.execute_pending_document_action(context, None);
        if app.document_workflow.modal().is_some() {
            break;
        }
    }
}

fn sources(app: &EditorApp) -> Vec<(u64, Option<PathBuf>, String, String, bool)> {
    app.tabs
        .ids()
        .map(|id| {
            let document = app.document_for_tab(id).unwrap();
            (
                id,
                document.path().clone(),
                document.source().clone(),
                document.saved_source().clone(),
                document.is_dirty(),
            )
        })
        .collect()
}

fn edit_tab(fixture: &mut Fixture, tab: u64, suffix: &str) {
    fixture.app.activate_tab(tab, &fixture.context);
    fixture
        .app
        .document_mut()
        .edit(CCursorRange::default(), |source| {
            source.push_str(suffix);
        });
}

fn dispatch(app: &mut EditorApp, context: &egui::Context) {
    for _ in 0..8 {
        app.execute_pending_document_action(context, None);
        if app.document_workflow.modal().is_some() || app.document_workflow.save_in_flight() {
            break;
        }
    }
}

fn take_unsaved(app: &mut EditorApp) -> PendingDocumentAction {
    let Some(AppModal::Unsaved { pending, .. }) = app.document_workflow.take_modal() else {
        panic!("each unsaved old-workspace page needs its own decision");
    };
    app.document_workflow.clear_modal();
    pending
}

fn discard_prompt(app: &mut EditorApp, context: &egui::Context) {
    let mut pending = take_unsaved(app);
    app.document_workflow.cancel_continuation();
    pending.key = app.document().key();
    pending.allow_discard = true;
    app.document_workflow.queue_action(pending);
    dispatch(app, context);
}

fn cancel_prompt(app: &mut EditorApp, context: &egui::Context) {
    let _ = take_unsaved(app);
    app.document_workflow.cancel_continuation();
    app.cancel_workspace_switch(context);
    app.document_workflow.finish_dispatch();
    dispatch(app, context);
}

fn begin_prompt_save(app: &mut EditorApp, context: &egui::Context) {
    let pending = take_unsaved(app);
    app.document_workflow.queue_action(PendingDocumentAction {
        action: DeferredDocumentAction::SaveThen(Box::new(pending)),
        key: app.document().key(),
        allow_discard: true,
        description: "saving the current document".into(),
    });
    dispatch(app, context);
}

fn save_prompt(app: &mut EditorApp, context: &egui::Context) {
    begin_prompt_save(app, context);
    app.finish_save_for_test(context);
    dispatch(app, context);
}

#[test]
fn changing_workspace_closes_all_clean_pages_from_the_previous_workspace() {
    let mut fixture = fixture();
    request_workspace(&mut fixture.app, fixture.new.clone(), &fixture.context);
    assert_eq!(fixture.app.workspace_root, fixture.new);
    assert!(
        fixture.app.tabs.is_empty(),
        "old-workspace files must not remain as pages in the selected workspace"
    );
    assert!(fixture.app.document().path().is_none());
    assert!(fixture.app.document().source().is_empty());
    assert_eq!(
        fs::read_to_string(fixture.first).unwrap(),
        "First saved source\n"
    );
    assert_eq!(
        fs::read_to_string(fixture.second).unwrap(),
        "Second saved source\n"
    );
    assert_eq!(fs::read_dir(fixture.new).unwrap().count(), 0);
}

#[test]
fn changing_workspace_checks_a_parked_dirty_page_before_switching() {
    let mut fixture = fixture();
    fixture
        .app
        .activate_tab(fixture.first_tab, &fixture.context);
    fixture
        .app
        .document_mut()
        .edit(CCursorRange::default(), |source| {
            source.push_str("Unsaved first-tab edit\n");
        });
    fixture
        .app
        .activate_tab(fixture.second_tab, &fixture.context);
    assert!(!fixture.app.document().is_dirty());
    let before = sources(&fixture.app);

    request_workspace(&mut fixture.app, fixture.new.clone(), &fixture.context);

    assert_eq!(
        fixture.app.workspace_root, fixture.old,
        "switching must wait for the parked draft's Save/Discard/Cancel choice"
    );
    assert!(matches!(
        fixture.app.document_workflow.modal(),
        Some(AppModal::Unsaved { .. })
    ));
    assert_eq!(sources(&fixture.app), before);
    assert_eq!(
        fs::read_to_string(&fixture.first).unwrap(),
        "First saved source\n"
    );
}

#[test]
fn selecting_the_current_workspace_keeps_its_saved_pages_and_parked_draft() {
    let mut fixture = fixture();
    fixture
        .app
        .activate_tab(fixture.first_tab, &fixture.context);
    fixture
        .app
        .document_mut()
        .edit(CCursorRange::default(), |source| {
            source.push_str("Retained draft\n");
        });
    fixture
        .app
        .activate_tab(fixture.second_tab, &fixture.context);
    let before = sources(&fixture.app);
    let active = fixture.app.tabs.active_id();
    let preview = fixture.app.tabs.preview_id();

    request_workspace(&mut fixture.app, fixture.old.clone(), &fixture.context);

    assert_eq!(fixture.app.workspace_root, fixture.old);
    assert_eq!(sources(&fixture.app), before);
    assert_eq!(fixture.app.tabs.active_id(), active);
    assert_eq!(fixture.app.tabs.preview_id(), preview);
    assert!(fixture.app.document_workflow.modal().is_none());
}

#[test]
fn changing_workspace_restores_its_real_remembered_file_without_old_pages() {
    let mut fixture = fixture();
    let destination = fixture.new.join("remembered.txt");
    fs::write(&destination, "Destination source\n").unwrap();
    fixture.app.settings.last_opened_files.insert(
        fixture.new.to_string_lossy().into_owned(),
        destination.to_string_lossy().into_owned(),
    );

    request_workspace(&mut fixture.app, fixture.new.clone(), &fixture.context);

    assert_eq!(fixture.app.workspace_root, fixture.new);
    assert_eq!(fixture.app.tabs.len(), 1);
    assert_eq!(fixture.app.document().path().as_ref(), Some(&destination));
    assert_eq!(fixture.app.document().source(), "Destination source\n");
    assert!(!fixture.app.document().is_dirty());
    assert!(fixture.app.tabs.ids().all(|id| {
        fixture
            .app
            .document_for_tab(id)
            .unwrap()
            .path()
            .as_ref()
            .is_none_or(|path| !path.starts_with(&fixture.old))
    }));
}

#[test]
fn switching_preserves_a_destination_owned_page_and_its_unsaved_source_and_caret() {
    let mut fixture = fixture();
    let destination = fixture.new.join("already-open.txt");
    fs::write(&destination, "Destination saved source\n").unwrap();
    fixture.app.lifecycle = DocumentLifecycle::Active;
    assert!(
        fixture
            .app
            .open_tab_path(destination.clone(), &fixture.context)
    );
    fixture.app.lifecycle = DocumentLifecycle::Dormant;
    let retained = fixture.app.tabs.active_id().unwrap();
    fixture.app.set_tab_workspace(retained, fixture.new.clone());
    fixture
        .app
        .document_mut()
        .edit(CCursorRange::default(), |source| {
            source.push_str("Destination draft stays open\n");
        });
    let source = fixture.app.document().source().clone();
    let cursor = CCursorRange::two(CCursor::new(3), CCursor::new(11));
    fixture.app.store_editor_cursor(&fixture.context, cursor);

    request_workspace(&mut fixture.app, fixture.new.clone(), &fixture.context);

    assert_eq!(fixture.app.workspace_root, fixture.new);
    assert_eq!(fixture.app.tabs.ids().collect::<Vec<_>>(), [retained]);
    assert_eq!(fixture.app.document().path().as_ref(), Some(&destination));
    assert_eq!(fixture.app.document().source(), &source);
    assert!(fixture.app.document().is_dirty());
    assert_eq!(fixture.app.editor_snapshot(&fixture.context).cursor, cursor);
    assert_eq!(
        fixture.app.tab_workspace(retained),
        Some(fixture.new.as_path())
    );
    assert!(fixture.app.document_workflow.modal().is_none());
    assert_eq!(
        fs::read_to_string(destination).unwrap(),
        "Destination saved source\n"
    );
}

#[test]
fn switching_keeps_separately_owned_pages_without_rebinding_their_workspace() {
    let mut fixture = fixture();
    let separate = fixture.old.parent().unwrap().join("separate-workspace");
    fs::create_dir(&separate).unwrap();
    let file = separate.join("separate.txt");
    fs::write(&file, "Separate source\n").unwrap();
    fixture.app.lifecycle = DocumentLifecycle::Active;
    assert!(fixture.app.open_tab_path(file.clone(), &fixture.context));
    fixture.app.lifecycle = DocumentLifecycle::Dormant;
    let retained = fixture.app.tabs.active_id().unwrap();
    fixture.app.set_tab_workspace(retained, separate.clone());
    fixture
        .app
        .document_mut()
        .edit(CCursorRange::default(), |source| {
            source.push_str("Separate draft\n");
        });
    let source = fixture.app.document().source().clone();

    request_workspace(&mut fixture.app, fixture.new.clone(), &fixture.context);

    assert_eq!(fixture.app.workspace_root, fixture.new);
    assert_eq!(fixture.app.tabs.ids().collect::<Vec<_>>(), [retained]);
    assert_eq!(fixture.app.document().path().as_ref(), Some(&file));
    assert_eq!(fixture.app.document().source(), &source);
    assert!(fixture.app.document().is_dirty());
    assert_eq!(
        fixture.app.tab_workspace(retained),
        Some(separate.as_path())
    );
    assert!(fixture.app.document_workflow.modal().is_none());
}

#[test]
fn remembered_destination_does_not_replace_a_separately_owned_clean_untitled_page() {
    let mut fixture = fixture();
    let separate = fixture.old.parent().unwrap().join("separate-workspace");
    fs::create_dir(&separate).unwrap();
    fixture.app.lifecycle = DocumentLifecycle::Active;
    fixture.app.new_tab(&fixture.context);
    fixture.app.lifecycle = DocumentLifecycle::Dormant;
    let retained = fixture.app.tabs.active_id().unwrap();
    fixture.app.set_tab_workspace(retained, separate.clone());
    let destination = fixture.new.join("remembered.txt");
    fs::write(&destination, "Destination source\n").unwrap();
    fixture.app.settings.last_opened_files.insert(
        fixture.new.to_string_lossy().into_owned(),
        destination.to_string_lossy().into_owned(),
    );

    request_workspace(&mut fixture.app, fixture.new.clone(), &fixture.context);

    assert_eq!(fixture.app.workspace_root, fixture.new);
    assert_eq!(fixture.app.tabs.len(), 2);
    let untitled = fixture.app.document_for_tab(retained).unwrap();
    assert!(untitled.path().is_none());
    assert!(untitled.source().is_empty());
    assert!(!untitled.is_dirty());
    assert_eq!(
        fixture.app.tab_workspace(retained),
        Some(separate.as_path())
    );
    assert_eq!(fixture.app.document().path().as_ref(), Some(&destination));
    assert_eq!(fixture.app.document().source(), "Destination source\n");
    assert_ne!(fixture.app.tabs.active_id(), Some(retained));
}

#[test]
fn nested_workspace_changes_close_old_owned_pages_despite_path_containment() {
    for select_parent in [false, true] {
        let mut fixture = fixture();
        let selected = if select_parent {
            fixture.old.parent().unwrap().to_path_buf()
        } else {
            let child = fixture.old.join("nested-workspace");
            fs::create_dir(&child).unwrap();
            let file = child.join("nested.txt");
            fs::write(&file, "Still owned by the old workspace\n").unwrap();
            fixture.app.lifecycle = DocumentLifecycle::Active;
            assert!(fixture.app.open_tab_path(file, &fixture.context));
            fixture.app.lifecycle = DocumentLifecycle::Dormant;
            let tab = fixture.app.tabs.active_id().unwrap();
            assert_eq!(fixture.app.tab_workspace(tab), Some(fixture.old.as_path()));
            child
        };

        request_workspace(&mut fixture.app, selected.clone(), &fixture.context);

        assert_eq!(fixture.app.workspace_root, selected);
        assert!(fixture.app.tabs.is_empty(), "parent={select_parent}");
        assert!(fixture.app.document().source().is_empty());
        assert!(fixture.app.document().path().is_none());
    }
}

#[test]
fn switching_closes_old_owned_untitled_pages_after_approving_their_draft() {
    for dirty in [false, true] {
        let mut fixture = fixture();
        fixture.app.lifecycle = DocumentLifecycle::Active;
        fixture.app.new_tab(&fixture.context);
        fixture.app.lifecycle = DocumentLifecycle::Dormant;
        let untitled = fixture.app.tabs.active_id().unwrap();
        assert!(fixture.app.document().path().is_none());
        assert_eq!(
            fixture.app.tab_workspace(untitled),
            Some(fixture.old.as_path())
        );
        if dirty {
            edit_tab(&mut fixture, untitled, "Untitled draft\n");
        }
        fixture
            .app
            .activate_tab(fixture.second_tab, &fixture.context);

        request_workspace(&mut fixture.app, fixture.new.clone(), &fixture.context);

        if dirty {
            assert_eq!(fixture.app.workspace_root, fixture.old);
            assert_eq!(fixture.app.tabs.active_id(), Some(untitled));
            assert_eq!(fixture.app.document().source(), "Untitled draft\n");
            discard_prompt(&mut fixture.app, &fixture.context);
        }
        assert_eq!(fixture.app.workspace_root, fixture.new);
        assert!(fixture.app.tabs.is_empty(), "dirty={dirty}");
        assert!(fixture.app.document().source().is_empty());
    }
}

#[test]
fn switching_closes_an_old_owned_external_file_without_changing_its_disk_source() {
    let mut fixture = fixture();
    let file = fixture.old.parent().unwrap().join("external.txt");
    fs::write(&file, "External saved source\n").unwrap();
    fixture.app.lifecycle = DocumentLifecycle::Active;
    assert!(fixture.app.open_tab_path(file.clone(), &fixture.context));
    fixture.app.lifecycle = DocumentLifecycle::Dormant;
    let external = fixture.app.tabs.active_id().unwrap();
    assert_eq!(
        fixture.app.tab_workspace(external),
        Some(fixture.old.as_path())
    );
    edit_tab(&mut fixture, external, "External draft\n");

    request_workspace(&mut fixture.app, fixture.new.clone(), &fixture.context);

    assert_eq!(fixture.app.workspace_root, fixture.old);
    assert_eq!(fixture.app.tabs.active_id(), Some(external));
    discard_prompt(&mut fixture.app, &fixture.context);
    assert_eq!(fixture.app.workspace_root, fixture.new);
    assert!(fixture.app.tabs.is_empty());
    assert_eq!(fs::read_to_string(file).unwrap(), "External saved source\n");
}

#[test]
fn cancelling_after_one_discard_keeps_the_whole_old_workspace_and_each_caret() {
    let mut fixture = fixture();
    let first = fixture.first_tab;
    let second = fixture.second_tab;
    edit_tab(&mut fixture, first, "First draft\n");
    let first_cursor = CCursorRange::two(CCursor::new(2), CCursor::new(7));
    fixture
        .app
        .store_editor_cursor(&fixture.context, first_cursor);
    edit_tab(&mut fixture, second, "Second draft\n");
    let second_cursor = CCursorRange::two(CCursor::new(3), CCursor::new(10));
    fixture
        .app
        .store_editor_cursor(&fixture.context, second_cursor);
    let before = sources(&fixture.app);
    request_workspace(&mut fixture.app, fixture.new.clone(), &fixture.context);

    discard_prompt(&mut fixture.app, &fixture.context);
    assert_eq!(fixture.app.workspace_root, fixture.old);
    assert_eq!(
        sources(&fixture.app),
        before,
        "an approval must not close or erase a page yet"
    );
    cancel_prompt(&mut fixture.app, &fixture.context);

    assert_eq!(fixture.app.workspace_root, fixture.old);
    assert_eq!(sources(&fixture.app), before);
    assert!(fixture.app.document_workflow.modal().is_none());
    assert!(!fixture.app.document_flow_busy());
    for (tab, cursor) in [(first, first_cursor), (second, second_cursor)] {
        fixture.app.activate_tab(tab, &fixture.context);
        assert_eq!(fixture.app.editor_snapshot(&fixture.context).cursor, cursor);
        assert_eq!(fixture.app.tab_workspace(tab), Some(fixture.old.as_path()));
    }
    assert_eq!(
        fs::read_to_string(&fixture.first).unwrap(),
        "First saved source\n"
    );
    assert_eq!(
        fs::read_to_string(&fixture.second).unwrap(),
        "Second saved source\n"
    );

    // A canceled approval cannot silently authorize the next request.
    request_workspace(&mut fixture.app, fixture.new.clone(), &fixture.context);
    assert_eq!(fixture.app.workspace_root, fixture.old);
    assert!(matches!(
        fixture.app.document_workflow.modal(),
        Some(AppModal::Unsaved { .. })
    ));
}

#[test]
fn discarding_all_old_workspace_drafts_closes_pages_only_after_the_final_decision() {
    let mut fixture = fixture();
    let first = fixture.first_tab;
    let second = fixture.second_tab;
    edit_tab(&mut fixture, first, "First draft\n");
    edit_tab(&mut fixture, second, "Second draft\n");
    let before = sources(&fixture.app);
    request_workspace(&mut fixture.app, fixture.new.clone(), &fixture.context);

    let mut decided = Vec::new();
    for decision in 0..2 {
        decided.push(fixture.app.document().path().clone().unwrap());
        discard_prompt(&mut fixture.app, &fixture.context);
        if decision == 0 {
            assert_eq!(fixture.app.workspace_root, fixture.old);
            assert_eq!(sources(&fixture.app), before);
        }
    }

    decided.sort();
    assert_eq!(decided, [fixture.first.clone(), fixture.second.clone()]);
    assert_eq!(fixture.app.workspace_root, fixture.new);
    assert!(fixture.app.tabs.is_empty());
    assert_eq!(
        fs::read_to_string(fixture.first).unwrap(),
        "First saved source\n"
    );
    assert_eq!(
        fs::read_to_string(fixture.second).unwrap(),
        "Second saved source\n"
    );
}

#[test]
fn saving_all_old_workspace_drafts_persists_each_file_before_closing_pages() {
    let mut fixture = fixture();
    let first = fixture.first_tab;
    let second = fixture.second_tab;
    edit_tab(&mut fixture, first, "First draft\n");
    edit_tab(&mut fixture, second, "Second draft\n");
    request_workspace(&mut fixture.app, fixture.new.clone(), &fixture.context);

    let mut saved = Vec::new();
    for decision in 0..2 {
        saved.push(fixture.app.document().path().clone().unwrap());
        save_prompt(&mut fixture.app, &fixture.context);
        if decision == 0 {
            assert_eq!(fixture.app.workspace_root, fixture.old);
            assert_eq!(fixture.app.tabs.len(), 2);
        }
    }

    saved.sort();
    assert_eq!(saved, [fixture.first.clone(), fixture.second.clone()]);
    assert_eq!(fixture.app.workspace_root, fixture.new);
    assert!(fixture.app.tabs.is_empty());
    assert_eq!(
        fs::read_to_string(fixture.first).unwrap(),
        "First saved source\nFirst draft\n"
    );
    assert_eq!(
        fs::read_to_string(fixture.second).unwrap(),
        "Second saved source\nSecond draft\n"
    );
}

#[test]
fn save_as_during_workspace_confirmation_keeps_ownership_until_the_final_decision() {
    let mut fixture = fixture();
    let first = fixture.first_tab;
    let second = fixture.second_tab;
    fixture.app.activate_tab(first, &fixture.context);
    fixture.app.reset_untitled_document();
    edit_tab(&mut fixture, first, "Untitled source to save\n");
    edit_tab(&mut fixture, second, "Another unsaved source\n");
    let saved_path = fixture.old.parent().unwrap().join("saved-elsewhere.txt");
    request_workspace(&mut fixture.app, fixture.new.clone(), &fixture.context);
    assert_eq!(fixture.app.tabs.active_id(), Some(first));

    // The native Save As chooser keeps this continuation while the selected
    // destination is submitted through the normal save receipt boundary.
    let pending = take_unsaved(&mut fixture.app);
    fixture.app.document_workflow.continue_after_save(pending);
    assert!(fixture.app.save_to(saved_path.clone(), &fixture.context));
    fixture.app.finish_save_for_test(&fixture.context);
    dispatch(&mut fixture.app, &fixture.context);

    assert_eq!(fixture.app.workspace_root, fixture.old);
    assert_eq!(fixture.app.tabs.ids().collect::<Vec<_>>(), [first, second]);
    assert_eq!(
        fixture.app.tab_workspace(first),
        Some(fixture.old.as_path())
    );
    let saved = fixture.app.document_for_tab(first).unwrap();
    assert_eq!(saved.path().as_ref(), Some(&saved_path));
    assert_eq!(saved.source(), "Untitled source to save\n");
    assert!(!saved.is_dirty());
    assert_eq!(fixture.app.tabs.active_id(), Some(second));
    assert!(matches!(
        fixture.app.document_workflow.modal(),
        Some(AppModal::Unsaved { .. })
    ));

    discard_prompt(&mut fixture.app, &fixture.context);

    assert_eq!(fixture.app.workspace_root, fixture.new);
    assert!(fixture.app.tabs.is_empty());
    assert_eq!(
        fs::read_to_string(saved_path).unwrap(),
        "Untitled source to save\n"
    );
    assert_eq!(
        fs::read_to_string(fixture.second).unwrap(),
        "Second saved source\n"
    );
}

#[test]
fn cancel_after_save_as_keeps_the_saved_file_and_all_old_workspace_pages() {
    let mut fixture = fixture();
    let first = fixture.first_tab;
    let second = fixture.second_tab;
    fixture.app.activate_tab(first, &fixture.context);
    fixture.app.reset_untitled_document();
    edit_tab(&mut fixture, first, "Saved untitled source\n");
    edit_tab(&mut fixture, second, "Retained second-page draft\n");
    let saved_path = fixture.old.parent().unwrap().join("saved-elsewhere.txt");
    request_workspace(&mut fixture.app, fixture.new.clone(), &fixture.context);
    let pending = take_unsaved(&mut fixture.app);
    fixture.app.document_workflow.continue_after_save(pending);
    assert!(fixture.app.save_to(saved_path.clone(), &fixture.context));
    fixture.app.finish_save_for_test(&fixture.context);
    dispatch(&mut fixture.app, &fixture.context);
    assert_eq!(fixture.app.tabs.active_id(), Some(second));

    cancel_prompt(&mut fixture.app, &fixture.context);

    assert_eq!(fixture.app.workspace_root, fixture.old);
    assert_eq!(fixture.app.tabs.ids().collect::<Vec<_>>(), [first, second]);
    assert_eq!(fixture.app.tabs.active_id(), Some(second));
    for tab in [first, second] {
        assert_eq!(fixture.app.tab_workspace(tab), Some(fixture.old.as_path()));
    }
    let saved = fixture.app.document_for_tab(first).unwrap();
    assert_eq!(saved.path().as_ref(), Some(&saved_path));
    assert_eq!(saved.source(), "Saved untitled source\n");
    assert!(!saved.is_dirty());
    let draft = fixture.app.document_for_tab(second).unwrap();
    assert_eq!(
        draft.source(),
        "Second saved source\nRetained second-page draft\n"
    );
    assert!(draft.is_dirty());
    assert_eq!(
        fs::read_to_string(saved_path).unwrap(),
        "Saved untitled source\n"
    );
    assert_eq!(
        fs::read_to_string(fixture.second).unwrap(),
        "Second saved source\n"
    );
    assert!(fixture.app.document_workflow.modal().is_none());
    assert!(!fixture.app.document_flow_busy());
}

#[test]
fn missing_destination_during_confirmation_preserves_all_old_pages() {
    let mut fixture = fixture();
    let first = fixture.first_tab;
    edit_tab(&mut fixture, first, "Retained draft\n");
    fixture
        .app
        .activate_tab(fixture.second_tab, &fixture.context);
    let before = sources(&fixture.app);
    request_workspace(&mut fixture.app, fixture.new.clone(), &fixture.context);
    fs::remove_dir(&fixture.new).unwrap();

    discard_prompt(&mut fixture.app, &fixture.context);

    assert_eq!(fixture.app.workspace_root, fixture.old);
    assert_eq!(sources(&fixture.app), before);
    assert!(!fixture.new.exists());
    assert!(!fixture.app.document_workflow.has_continuation());
}

#[test]
fn failed_save_never_switches_or_closes_a_page() {
    let mut fixture = fixture();
    let first = fixture.first_tab;
    edit_tab(&mut fixture, first, "Retained draft\n");
    fixture
        .app
        .activate_tab(fixture.second_tab, &fixture.context);
    let before = sources(&fixture.app);
    request_workspace(&mut fixture.app, fixture.new.clone(), &fixture.context);
    let relocated = fixture.old.with_file_name("externally-moved-workspace");
    fs::rename(&fixture.old, &relocated).unwrap();

    save_prompt(&mut fixture.app, &fixture.context);

    assert_eq!(fixture.app.workspace_root, fixture.old);
    assert_eq!(sources(&fixture.app), before);
    assert_eq!(
        fs::read_to_string(relocated.join("first.txt")).unwrap(),
        "First saved source\n"
    );
    assert_eq!(fs::read_dir(fixture.new).unwrap().count(), 0);
}

#[test]
fn a_new_edit_during_save_stays_dirty_and_prevents_the_switch() {
    let mut fixture = fixture();
    let first = fixture.first_tab;
    edit_tab(&mut fixture, first, "Saved request draft\n");
    fixture
        .app
        .activate_tab(fixture.second_tab, &fixture.context);
    request_workspace(&mut fixture.app, fixture.new.clone(), &fixture.context);
    let saving_tab = fixture.app.tabs.active_id().unwrap();
    let path = fixture.app.document().path().clone().unwrap();
    let (entered, ready) = std::sync::mpsc::channel();
    let (release, wait) = std::sync::mpsc::channel();
    let locked_path = path.clone();
    let holder = std::thread::spawn(move || {
        crate::resource_lock::with_resource(&locked_path, || {
            entered.send(()).unwrap();
            let _ = wait.recv();
        });
    });
    ready.recv_timeout(Duration::from_secs(5)).unwrap();
    begin_prompt_save(&mut fixture.app, &fixture.context);
    assert!(fixture.app.document_workflow.save_in_flight());
    fixture
        .app
        .document_mut()
        .edit(CCursorRange::default(), |source| {
            source.push_str("Newer edit while saving\n");
        });
    let retained = fixture.app.document().source().clone();
    release.send(()).unwrap();
    holder.join().unwrap();
    fixture.app.finish_save_for_test(&fixture.context);
    dispatch(&mut fixture.app, &fixture.context);

    assert_eq!(fixture.app.workspace_root, fixture.old);
    assert_eq!(fixture.app.tabs.len(), 2);
    let document = fixture.app.document_for_tab(saving_tab).unwrap();
    assert_eq!(document.source(), &retained);
    assert!(document.is_dirty());
    assert!(
        !fs::read_to_string(path)
            .unwrap()
            .contains("Newer edit while saving")
    );
}
