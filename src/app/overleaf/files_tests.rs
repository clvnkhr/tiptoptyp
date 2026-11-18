use super::*;
use egui_kittest::{
    Harness,
    kittest::{NodeT as _, Queryable as _},
};

#[test]
fn preview_source_types_follow_the_editor_document_language_policy() {
    for extension in crate::document::TEX_SOURCE_EXTENSIONS {
        let path = format!("/mirror/source.{}", extension.to_ascii_uppercase());
        assert!(preview_source_path(Path::new(&path)));
    }
    assert!(preview_source_path(Path::new("/mirror/main.TYP")));
    for path in [
        "/mirror/references.bib",
        "/mirror/figure.pdf",
        "/mirror/source.txt",
    ] {
        assert!(!preview_source_path(Path::new(path)));
    }
}

fn tree() -> Tree {
    Tree::new(
        "Paper".into(),
        Some("a".into()),
        vec![
            Entry {
                id: "a".into(),
                path: "main.tex".into(),
                kind: EntryKind::Doc,
            },
            Entry {
                id: "b".into(),
                path: "sections".into(),
                kind: EntryKind::Folder,
            },
            Entry {
                id: "c".into(),
                path: "sections/intro.tex".into(),
                kind: EntryKind::Doc,
            },
            Entry {
                id: "d".into(),
                path: "main copy.tex".into(),
                kind: EntryKind::Doc,
            },
            Entry {
                id: "e".into(),
                path: "figure.png".into(),
                kind: EntryKind::File,
            },
        ],
    )
    .unwrap()
}
fn dialog(action: FileAction) -> Dialog {
    Dialog {
        epoch: 7,
        action,
        id: Some("a".into()),
        path: "main.tex".into(),
        name: "main copy 2.tex".into(),
        parent: String::new(),
        folders: vec![String::new(), "sections".into()],
        focus: true,
        error: None,
        edit_events: vec![],
    }
}
#[test]
fn file_names_preserve_extension_and_do_not_suggest_existing_siblings() {
    let tree = tree();
    assert_eq!(
        available_name(&tree, "", "main.tex", true),
        "main copy 2.tex"
    );
    assert_eq!(
        available_name(&tree, "sections", "intro.tex", true),
        "intro copy.tex"
    );
    assert_eq!(
        available_name(&tree, "", "untitled.tex", false),
        "untitled.tex"
    );
    for name in [
        "../escape.tex",
        "chapter/new.tex",
        ".git",
        ".tiptoptyp",
        "bad*name.tex",
        " trailing.tex ",
        "constructor",
        "",
        "a\u{7f}.tex",
    ] {
        assert!(validate_name(name).is_err(), "{name}");
    }
    assert!(validate_name("章节.tex").is_ok());
}
#[test]
fn file_action_validation_rejects_stale_identity_collisions_and_cyclic_moves() {
    let tree = tree();
    assert!(validate_dialog(&dialog(FileAction::Duplicate), &tree).is_ok());
    let mut rename = dialog(FileAction::Rename);
    rename.name = "MAIN COPY.TEX".into();
    assert!(
        validate_dialog(&rename, &tree)
            .unwrap_err()
            .contains("already exists")
    );
    rename.name = "main.tex".into();
    assert!(validate_dialog(&rename, &tree).is_ok());
    rename.path = "stale.tex".into();
    assert!(
        validate_dialog(&rename, &tree)
            .unwrap_err()
            .contains("changed")
    );
    let mut movement = dialog(FileAction::Move);
    movement.id = Some("b".into());
    movement.path = "sections".into();
    movement.parent = "sections".into();
    assert!(
        validate_dialog(&movement, &tree)
            .unwrap_err()
            .contains("inside itself")
    );
}
#[test]
fn overleaf_menu_routes_new_and_duplicate_without_enabling_unsafe_binary_copies() {
    for (kind, mutable) in [
        (EntryKind::Doc, true),
        (EntryKind::File, true),
        (EntryKind::Folder, true),
        (EntryKind::Doc, false),
    ] {
        let mut harness = Harness::builder()
            .with_size(Vec2::new(420.0, 600.0))
            .build_ui_state(
                move |ui, action| {
                    file_menu(
                        ui,
                        Path::new("/mirror/sections/file.tex"),
                        Path::new("/mirror/sections"),
                        Some(kind),
                        mutable,
                        action,
                    );
                },
                None::<AppPopupAction>,
            );
        harness.run();
        assert_eq!(
            !harness
                .get_by_label_contains("Duplicate…")
                .accesskit_node()
                .is_disabled(),
            mutable && kind == EntryKind::Doc
        );
        for label in ["New file…", "New folder…", "Rename…", "Move…", "Delete…"] {
            assert_eq!(
                !harness
                    .get_by_label_contains(label)
                    .accesskit_node()
                    .is_disabled(),
                mutable,
                "{label}"
            );
        }
        if mutable {
            harness.get_by_label_contains("New file…").click();
            harness.run();
            assert!(
                matches!(harness.state(), Some(AppPopupAction::Workspace(WorkspaceMenuAction::Overleaf(Action::NewFile(path)))) if path == Path::new("/mirror/sections"))
            );
        }
    }
}
#[test]
fn overleaf_file_form_keeps_typing_and_pending_controls_owned_by_its_dialog() {
    let mut harness = Harness::builder()
        .with_size(Vec2::new(450.0, 300.0))
        .build_ui_state(
            |ui, state: &mut (Dialog, bool, Option<Choice>)| {
                if let Some(choice) = file_form(ui, &mut state.0, state.1) {
                    state.2 = Some(choice);
                }
            },
            (dialog(FileAction::Duplicate), false, None),
        );
    harness.run();
    harness.get_by_label("Overleaf file name").type_text("new");
    harness.run();
    assert!(harness.state().0.name.contains("new"));
    harness.state_mut().1 = true;
    // The in-flight spinner intentionally repaints until the server replies.
    harness.run_steps(2);
    assert!(
        harness
            .get_by_label("Duplicate")
            .accesskit_node()
            .is_disabled()
    );
    assert!(
        harness
            .get_by_label("Cancel")
            .accesskit_node()
            .is_disabled()
    );
    assert!(
        harness
            .get_by_label("Overleaf file name")
            .accesskit_node()
            .is_disabled()
    );
    harness.state_mut().1 = false;
    harness.run();
    harness.get_by_label("Cancel").click();
    harness.run();
    assert!(harness.state().2 == Some(Choice::Cancel));
}
#[test]
fn managed_create_rename_and_duplicate_never_start_local_filesystem_work() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().canonicalize().unwrap();
    let context = egui::Context::default();
    for action in [
        Action::NewFile(root.clone()),
        Action::NewFolder(root.clone()),
        Action::Duplicate(root.join("main.tex")),
        Action::Rename(root.join("main.tex")),
        Action::Delete(root.join("main.tex")),
    ] {
        let mut app = super::super::tests::fixture(&root, &context);
        app.overleaf.files.grant(7, true);
        let original = fs::read(root.join("main.tex")).unwrap();
        app.begin_overleaf_file_action(action);
        assert!(app.overleaf_file_dialog_open());
        app.submit_overleaf_file_action(); // No browser in this fixture: fail safely.
        assert!(app.overleaf.files.dialog().unwrap().error.is_some());
        assert!(!app.file_import.is_running());
        assert_eq!(fs::read(root.join("main.tex")).unwrap(), original);
        assert!(!root.join("untitled.tex").exists());
        assert!(!root.join("New folder").exists());
        assert!(!root.join("main copy.tex").exists());
    }
}
#[test]
fn file_permissions_and_receipts_are_epoch_and_request_scoped() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().canonicalize().unwrap();
    let context = egui::Context::default();
    let mut app = super::super::tests::fixture(&root, &context);
    assert!(!app.overleaf_files_writable());
    app.update_overleaf_event(
        BrowserEvent::FileAccess {
            epoch: 6,
            writable: true,
        },
        &context,
    )
    .unwrap();
    assert!(!app.overleaf_files_writable());
    app.update_overleaf_event(
        BrowserEvent::FileAccess {
            epoch: 7,
            writable: true,
        },
        &context,
    )
    .unwrap();
    assert!(app.overleaf_files_writable());
    app.overleaf.files.operation = Operation::Editing(dialog(FileAction::Delete));
    app.overleaf.files.submit(Pending {
        epoch: 7,
        request: 10,
        action: FileAction::Delete,
        started: Instant::now(),
    });
    app.update_overleaf_event(
        BrowserEvent::FileAction {
            epoch: 7,
            request: 9,
            id: None,
            path: None,
            error: None,
        },
        &context,
    )
    .unwrap();
    assert!(app.overleaf.files.pending().is_some());
    app.update_overleaf_event(
        BrowserEvent::FileAction {
            epoch: 7,
            request: 10,
            id: None,
            path: None,
            error: Some("Permission denied".into()),
        },
        &context,
    )
    .unwrap();
    assert!(app.overleaf.files.pending().is_none());
    assert_eq!(
        app.overleaf.files.dialog().unwrap().error.as_deref(),
        Some("Permission denied")
    );
    assert!(root.join("main.tex").exists());
    app.overleaf.epoch = 8;
    assert!(!app.overleaf_files_writable());
}

#[test]
fn interrupted_file_operations_release_the_dialog_without_retrying_or_changing_mirrors() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().canonicalize().unwrap();
    let context = egui::Context::default();
    let mut app = super::super::tests::fixture(&root, &context);
    app.overleaf.files.operation = Operation::Editing(dialog(FileAction::Delete));
    app.overleaf.files.submit(Pending {
        epoch: 7,
        request: 10,
        action: FileAction::Delete,
        started: Instant::now(),
    });
    app.overleaf.connection.as_mut().unwrap().transport = Transport::Offline;
    app.show_overleaf_file_dialog(&context);
    assert!(!app.overleaf_file_operation_pending());
    assert!(app.overleaf.files.dialog().unwrap().error.is_some());
    assert_eq!(
        fs::read_to_string(root.join("main.tex")).unwrap(),
        "Main old\n"
    );
}

#[test]
fn native_edit_and_source_commands_belong_to_the_file_dialog() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().canonicalize().unwrap();
    let context = egui::Context::default();
    let mut app = super::super::tests::fixture(&root, &context);
    app.overleaf.files.operation = Operation::Editing(dialog(FileAction::Rename));
    let source = app.document().source().to_owned();
    let key = app.document().key();
    assert!(app.route_overleaf_file_command(AppCommand::SelectAll, &context));
    assert!(matches!(
        app.overleaf.files.dialog().unwrap().edit_events.as_slice(),
        [egui::Event::Key {
            key: egui::Key::A,
            ..
        }]
    ));
    assert!(app.route_overleaf_file_command(AppCommand::ToggleComment, &context));
    assert!(app.route_overleaf_file_command(AppCommand::New, &context));
    assert_eq!(app.document().source(), &source);
    assert_eq!(app.document().key(), key);
    assert!(app.route_overleaf_file_command(AppCommand::CloseTab, &context));
    assert!(!app.overleaf_file_dialog_open());
    assert_eq!(app.document().key(), key);
}

#[test]
fn file_dialog_viewport_is_owned_by_its_document_window() {
    let owner = egui::ViewportId::ROOT;
    let child = crate::child_view::child_viewport_id(owner, "tiptoptyp-overleaf-file-overlay");
    assert!(owned_input_viewports(owner).contains(&child));
    let other = egui::ViewportId::from_hash_of("another document");
    assert!(!owned_input_viewports(other).contains(&child));
}

#[test]
fn uncertain_file_operation_timeout_requires_a_fresh_connection_before_resubmission() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().canonicalize().unwrap();
    let context = egui::Context::default();
    let mut app = super::super::tests::fixture(&root, &context);
    app.overleaf.files.grant(7, true);
    app.overleaf.files.operation = Operation::Editing(dialog(FileAction::Delete));
    app.overleaf.files.submit(Pending {
        epoch: 7,
        request: 10,
        action: FileAction::Delete,
        started: Instant::now().checked_sub(Duration::from_secs(51)).unwrap(),
    });
    app.show_overleaf_file_dialog(&context);
    assert!(matches!(
        app.overleaf.files.operation,
        Operation::Uncertain { .. }
    ));
    assert!(!app.overleaf_file_operation_pending());
    assert!(!app.overleaf_files_writable());
    app.submit_overleaf_file_action();
    assert!(!app.overleaf_file_operation_pending());
    assert!(
        app.overleaf
            .files
            .dialog()
            .unwrap()
            .error
            .as_ref()
            .unwrap()
            .contains("connection changed")
    );
    assert_eq!(
        fs::read_to_string(root.join("main.tex")).unwrap(),
        "Main old\n"
    );
}
