//! Production startup must never manufacture a document to fill an empty window.
use super::*;

fn session(
    context: &egui::Context,
    path: Option<PathBuf>,
    settings: AppSettings,
    scene: Option<UiSnapshotScene>,
) -> EditorApp {
    EditorApp::new_session(
        context,
        egui::ViewportId::ROOT,
        path,
        CaptureController::disabled_for_tests(),
        None,
        scene,
        settings,
        EditorWindowHost::Root,
        DocumentLifecycle::Dormant,
    )
}

fn recent_settings(root: &Path) -> AppSettings {
    let mut settings = AppSettings::default();
    settings.remember_workspace(root);
    settings
}

fn assert_no_open_documents(app: &EditorApp) {
    assert_eq!(app.tabs.len(), 0);
    assert!(app.tabs.active_id().is_none());
    assert!(app.tabs.preview_id().is_none());
    assert!(app.document().source().is_empty());
    assert!(app.document().saved_source().is_empty());
    assert!(app.document().path().is_none());
    assert!(!app.document().is_dirty());
    assert!(app.compile_deadline.is_none());
    assert!(app.tinymist_sync.generation().is_none());
}

#[test]
fn first_launch_without_a_file_has_no_open_document_even_with_tex_auto_enable() {
    for mitex_auto_enable in [false, true] {
        let context = egui::Context::default();
        let settings = AppSettings {
            mitex_auto_enable,
            ..AppSettings::default()
        };
        assert!(settings.recent_workspaces.is_empty());
        assert!(settings.last_opened_files.is_empty());
        let app = session(&context, None, settings, None);
        assert_no_open_documents(&app);
    }
}

#[test]
fn restored_workspace_with_no_usable_last_file_has_no_open_document() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().canonicalize().unwrap();
    let unsupported = root.join("unsupported.fixture");
    fs::write(
        &unsupported,
        "An existing file with an unsupported extension",
    )
    .unwrap();
    for last_file in [None, Some(root.join("missing.typ")), Some(unsupported)] {
        let context = egui::Context::default();
        let mut settings = recent_settings(&root);
        if let Some(path) = last_file {
            settings.last_opened_files.insert(
                root.to_string_lossy().into_owned(),
                path.to_string_lossy().into_owned(),
            );
        }
        let app = session(&context, None, settings, None);
        assert_eq!(app.workspace_root, root);
        assert_no_open_documents(&app);
    }
}

#[test]
fn missing_recent_workspace_does_not_create_a_fallback_document() {
    let directory = tempfile::tempdir().unwrap();
    let context = egui::Context::default();
    let settings = AppSettings {
        recent_workspaces: vec![directory.path().join("missing").display().to_string()],
        ..AppSettings::default()
    };
    let app = session(&context, None, settings, None);
    assert_no_open_documents(&app);
}

#[test]
fn missing_explicit_launch_path_reports_the_error_without_a_fallback_document() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().canonicalize().unwrap();
    let context = egui::Context::default();
    let app = session(
        &context,
        Some(root.join("missing.typ")),
        recent_settings(&root),
        None,
    );
    assert_no_open_documents(&app);
    assert!(app.notice.as_ref().is_some_and(|notice| {
        notice.kind == NoticeKind::Error && notice.message.contains("does not exist")
    }));
}

#[test]
fn invalid_utf8_launch_opens_the_import_choice_without_a_fallback_document() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().canonicalize().unwrap();
    let path = root.join("legacy.typ");
    fs::write(&path, [b'c', b'a', b'f', 0xe9]).unwrap();
    let context = egui::Context::default();
    let app = session(&context, Some(path), recent_settings(&root), None);
    assert_no_open_documents(&app);
    assert!(app.encoding_import.is_some());
}

#[test]
fn explicit_empty_directory_launch_does_not_create_a_document_or_backing_file() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().canonicalize().unwrap();
    let context = egui::Context::default();
    let app = session(&context, Some(root.clone()), AppSettings::default(), None);
    assert_eq!(app.workspace_root, root);
    assert_no_open_documents(&app);
    assert_eq!(fs::read_dir(&root).unwrap().count(), 0);
}

#[test]
fn stored_overleaf_root_and_file_launches_stay_empty_while_waiting_for_server_content() {
    let directory = tempfile::tempdir().unwrap();
    let base = directory.path().canonicalize().unwrap();
    let project = crate::overleaf::ProjectUrl::parse(
        "https://www.overleaf.com/project/0123456789abcdef01234567",
    )
    .unwrap();
    let lease = crate::overleaf::StorageLease::prepare(
        &project,
        &crate::overleaf::StorageOptions {
            keep_local_copy: true,
            directory: Some(base.clone()),
        },
        &base,
    )
    .unwrap();
    let root = lease.root().to_owned();
    let file = root.join("main.typ");
    fs::write(&file, "= Offline content must wait for the server").unwrap();
    let mut settings = AppSettings::default();
    settings.remember_overleaf_project(project.url()).unwrap();
    settings
        .set_overleaf_storage_preference(
            project.url(),
            crate::settings::OverleafStoragePreference {
                keep_local_copy: true,
                directory: Some(base.to_string_lossy().into_owned()),
            },
        )
        .unwrap();
    for path in [root, file] {
        let context = egui::Context::default();
        let app = session(&context, Some(path.clone()), settings.clone(), None);
        assert!(app.is_overleaf_path(&path));
        assert_no_open_documents(&app);
    }
}

#[test]
fn new_secondary_window_has_an_empty_workspace_until_a_document_is_requested() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().canonicalize().unwrap();
    let context = egui::Context::default();
    let viewport = egui::ViewportId::from_hash_of("empty-new-window");
    let app = EditorApp::new_secondary(
        &context,
        viewport,
        EditorWindowRequest::New {
            workspace_root: root.clone(),
        },
        AppSettings::default(),
        CaptureController::disabled_for_tests(),
    );
    assert_eq!(app.workspace_root, root);
    assert_no_open_documents(&app);
    assert_eq!(fs::read_dir(&root).unwrap().count(), 0);
}

#[test]
fn reused_dormant_new_window_does_not_restore_its_closed_document_or_create_a_blank_tab() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().canonicalize().unwrap();
    let path = root.join("previous.typ");
    fs::write(&path, "= Closed source").unwrap();
    let context = egui::Context::default();
    let mut app = session(&context, Some(path), recent_settings(&root), None);
    assert_eq!(app.tabs.len(), 1);
    app.lifecycle = DocumentLifecycle::Active;
    app.finish_window_close();
    let next = root.join("next-workspace");
    fs::create_dir(&next).unwrap();
    app.reuse_dormant_window(EditorWindowRequest::New {
        workspace_root: next.clone(),
    });
    assert!(app.needs_visible_window());
    assert_eq!(app.workspace_root, next);
    assert_no_open_documents(&app);
    assert_eq!(fs::read_dir(&next).unwrap().count(), 0);
}

#[test]
fn saved_file_launch_and_restore_still_open_the_real_document() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().canonicalize().unwrap();
    let path = root.join("real.typ");
    let source = "= A saved user document\n";
    fs::write(&path, source).unwrap();
    for explicit in [false, true] {
        let context = egui::Context::default();
        let mut settings = recent_settings(&root);
        settings.last_opened_files.insert(
            root.to_string_lossy().into_owned(),
            path.to_string_lossy().into_owned(),
        );
        let app = session(&context, explicit.then(|| path.clone()), settings, None);
        assert_eq!(app.tabs.len(), 1);
        assert_eq!(app.document().path().as_ref(), Some(&path));
        assert_eq!(app.document().source(), source);
        assert_eq!(app.document().saved_source(), source);
        assert!(!app.document().is_dirty());
    }
}

#[test]
fn explicit_new_document_from_an_empty_workspace_creates_one_blank_tab() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().canonicalize().unwrap();
    let context = egui::Context::default();
    let mut app = session(&context, Some(root), AppSettings::default(), None);
    assert_no_open_documents(&app);
    app.execute_app_command(AppCommand::New, &context, None);
    app.execute_pending_document_action(&context, None);
    assert_eq!(app.tabs.len(), 1);
    assert!(app.document().source().is_empty());
    assert!(app.document().path().is_none());
    assert!(!app.document().is_dirty());
}

#[test]
fn selecting_an_empty_workspace_drops_a_single_clean_blank_tab() {
    let directory = tempfile::tempdir().unwrap();
    let base = directory.path().canonicalize().unwrap();
    let previous = base.join("previous");
    let selected = base.join("selected");
    fs::create_dir(&previous).unwrap();
    fs::create_dir(&selected).unwrap();
    let context = egui::Context::default();
    let mut app = session(&context, Some(previous), AppSettings::default(), None);
    app.new_tab(&context);
    assert_eq!(app.tabs.len(), 1);
    assert!(app.document().source().is_empty());
    assert!(app.document().path().is_none());
    assert!(!app.document().is_dirty());

    app.finish_open_folder_selection(selected.clone(), &context);

    assert_eq!(app.workspace_root, selected);
    assert_no_open_documents(&app);
    assert_eq!(fs::read_dir(&selected).unwrap().count(), 0);
}

#[test]
fn selecting_an_empty_workspace_checks_parked_source_and_cancel_keeps_old_workspace() {
    let directory = tempfile::tempdir().unwrap();
    let base = directory.path().canonicalize().unwrap();
    let previous = base.join("previous");
    let selected = base.join("selected");
    fs::create_dir(&previous).unwrap();
    fs::create_dir(&selected).unwrap();
    let first = previous.join("first.txt");
    let second = previous.join("second.txt");
    fs::write(&first, "First saved source\n").unwrap();
    fs::write(&second, "Second saved source\n").unwrap();
    let context = egui::Context::default();
    let mut app = session(
        &context,
        Some(first.clone()),
        recent_settings(&previous),
        None,
    );
    let parked = app.tabs.active_id().unwrap();
    app.document_mut().edit(CCursorRange::default(), |source| {
        source.push_str("Unsaved first-tab edit\n");
    });
    app.lifecycle = DocumentLifecycle::Active;
    assert!(app.open_tab_path(second.clone(), &context));
    app.lifecycle = DocumentLifecycle::Dormant;
    assert_eq!(app.tabs.len(), 2);
    assert_ne!(app.tabs.active_id(), Some(parked));
    assert!(app.document_for_tab(parked).unwrap().is_dirty());
    assert!(!app.document().is_dirty());
    let before = app
        .tabs
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
        .collect::<Vec<_>>();
    let active = app.tabs.active_id();

    app.queue_open_path(selected.clone());
    for _ in 0..8 {
        app.execute_pending_document_action(&context, None);
        if app.document_workflow.modal().is_some() {
            break;
        }
    }

    assert_eq!(app.workspace_root, previous);
    assert!(matches!(
        app.document_workflow.modal(),
        Some(AppModal::Unsaved { .. })
    ));
    assert_eq!(app.tabs.len(), 2);
    assert!(app.document_for_tab(parked).unwrap().is_dirty());

    let _ = app.document_workflow.take_modal();
    app.document_workflow.clear_modal();
    app.document_workflow.cancel_continuation();
    app.cancel_workspace_switch(&context);
    app.document_workflow.finish_dispatch();
    app.execute_pending_document_action(&context, None);

    assert_eq!(app.workspace_root, previous);
    assert_eq!(app.tabs.len(), 2);
    assert_eq!(app.tabs.active_id(), active);
    assert!(app.document_workflow.modal().is_none());
    for (id, path, source, saved_source, dirty) in before {
        let document = app.document_for_tab(id).unwrap();
        assert_eq!(document.path(), &path);
        assert_eq!(document.source(), &source);
        assert_eq!(document.saved_source(), &saved_source);
        assert_eq!(document.is_dirty(), dirty);
    }
    assert_eq!(fs::read(&first).unwrap(), b"First saved source\n");
    assert_eq!(fs::read(&second).unwrap(), b"Second saved source\n");
    assert_eq!(fs::read_dir(&selected).unwrap().count(), 0);
}

#[test]
fn explorer_can_select_a_real_preview_source_from_an_empty_workspace() {
    for (name, source) in [
        ("main.typ", "= A real Typst source\n"),
        (
            "main.tex",
            "\\documentclass{article}\n\\begin{document}Real source\\end{document}\n",
        ),
    ] {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().canonicalize().unwrap();
        let path = root.join(name);
        fs::write(&path, source).unwrap();
        let context = egui::Context::default();
        let mut app = session(&context, Some(root), AppSettings::default(), None);
        assert_no_open_documents(&app);

        app.toggle_file_for_preview(path.clone(), &context);

        assert_eq!(app.tabs.len(), 1);
        assert_eq!(app.tabs.preview_id(), app.tabs.active_id());
        assert!(app.tabs.active_id().is_some());
        assert_eq!(app.document().path().as_ref(), Some(&path));
        assert_eq!(app.document().source(), source);
        assert!(!app.document().is_dirty());
        assert_eq!(fs::read_to_string(&path).unwrap(), source);
    }
}

#[test]
fn empty_transitions_advance_document_keys_and_refresh_cached_source_on_reopen() {
    for close_window in [false, true] {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().canonicalize().unwrap();
        let first = root.join("first.typ");
        let second = root.join("second.typ");
        let first_source = "= First\n";
        let second_source = "= Second document with a different length\nAnother line\n";
        fs::write(&first, first_source).unwrap();
        fs::write(&second, second_source).unwrap();
        let context = egui::Context::default();
        let mut app = session(&context, Some(first), recent_settings(&root), None);
        app.prepare_editor_source_data();
        assert_eq!(app.editor_data.source_snapshot().as_ref(), first_source);
        let previous = app.document().key();

        if close_window {
            app.lifecycle = DocumentLifecycle::Active;
            app.finish_window_close();
        } else {
            app.finish_close_tab(&context);
        }
        assert_no_open_documents(&app);
        let empty = app.document().key();
        assert_eq!(empty.owner, previous.owner);
        assert!(empty.epoch > previous.epoch);
        assert!(empty.revision > previous.revision);

        assert!(app.open_tab_path(second.clone(), &context));
        let reopened = app.document().key();
        assert_eq!(reopened.owner, previous.owner);
        assert!(reopened.epoch > empty.epoch);
        assert!(reopened.revision > empty.revision);
        app.prepare_editor_source_data();
        assert_eq!(app.document().path().as_ref(), Some(&second));
        assert_eq!(app.editor_data.source_snapshot().as_ref(), second_source);
        assert_eq!(
            app.editor_data.source_metrics().char_count,
            second_source.chars().count()
        );
        assert_eq!(app.editor_data.source_metrics().line_count, 3);
        assert!(!app.document().is_dirty());
    }
}

#[test]
fn explicitly_selected_qa_scene_may_install_its_demo_fixture() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().canonicalize().unwrap();
    let context = egui::Context::default();
    let app = session(
        &context,
        Some(root),
        AppSettings::default(),
        Some(UiSnapshotScene::Main),
    );
    assert_eq!(app.tabs.len(), 1);
    assert_eq!(app.document().source(), demo::SOURCE);
    assert!(app.document().path().is_none());
}
