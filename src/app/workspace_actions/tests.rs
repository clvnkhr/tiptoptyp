use super::*;
use egui_kittest::{
    Harness,
    kittest::{NodeT as _, Queryable as _},
};

#[test]
fn context_menus_bind_new_file_and_duplicate_to_the_clicked_entry() {
    for is_file in [false, true] {
        for duplicate in [false, true] {
            let path = PathBuf::from(if is_file {
                "/project/chapter/part.tex"
            } else {
                "/project/chapter"
            });
            let expected = path.clone();
            let mut harness = Harness::builder()
                .with_size(Vec2::new(400.0, 600.0))
                .build_ui_state(
                    move |ui, action| {
                        show_workspace_popup_ui(ui, &path, is_file, false, true, action)
                    },
                    None::<AppPopupAction>,
                );
            harness.run();
            harness
                .get_by_label_contains(if duplicate {
                    "Duplicate…"
                } else if is_file {
                    "New file next to this file…"
                } else {
                    "New file in this folder…"
                })
                .click();
            harness.run();
            match harness.state() {
                Some(AppPopupAction::Workspace(WorkspaceMenuAction::Duplicate(path)))
                    if duplicate =>
                {
                    assert_eq!(path, &expected)
                }
                Some(AppPopupAction::Workspace(WorkspaceMenuAction::NewFile {
                    directory,
                    tex,
                })) if !duplicate => {
                    assert_eq!(
                        directory,
                        if is_file {
                            expected.parent().unwrap()
                        } else {
                            &expected
                        }
                    );
                    assert_eq!(*tex, is_file);
                }
                other => panic!("wrong menu target: {other:?}"),
            }
        }
    }
}

#[test]
fn read_only_menu_keeps_file_mutations_disabled() {
    let mut harness = Harness::builder()
        .with_size(Vec2::new(400.0, 600.0))
        .build_ui_state(
            |ui, action| {
                show_workspace_popup_ui(
                    ui,
                    Path::new("/mirror/main.typ"),
                    true,
                    false,
                    false,
                    action,
                )
            },
            None::<AppPopupAction>,
        );
    harness.run();
    for label in [
        "New file next to this file…",
        "Duplicate…",
        "Rename…",
        "Delete…",
    ] {
        assert!(
            harness
                .get_by_label_contains(label)
                .accesskit_node()
                .is_disabled(),
            "{label}"
        );
    }
}

fn finish_worker(app: &mut EditorApp, context: &egui::Context) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while app.file_import.is_running() {
        app.poll_file_import(context);
        assert!(Instant::now() < deadline, "file worker did not finish");
        std::thread::yield_now();
    }
}

#[test]
fn new_file_prompt_does_not_touch_disk_until_submit_and_rejects_invalid_or_existing_names() {
    let root = tempfile::tempdir().unwrap();
    let root = root.path().canonicalize().unwrap();
    let folder = root.join("chapter");
    fs::create_dir(&folder).unwrap();
    let context = egui::Context::default();
    let mut app = EditorApp::dormant_for_tests(&context, root.clone());
    app.begin_workspace_new_file(folder.clone(), true);
    assert_eq!(app.rename_dialog.as_ref().unwrap().name, "untitled.tex");
    assert_eq!(fs::read_dir(&folder).unwrap().count(), 0);
    let dialog = app.rename_dialog.take().unwrap();
    drop(dialog); // Cancel leaves no empty file behind.
    fs::write(folder.join("existing.tex"), "keep me").unwrap();
    for name in ["existing.tex", "../escape.typ", "", ".", "sub/new.typ"] {
        app.begin_workspace_new_file(folder.clone(), true);
        let mut dialog = app.rename_dialog.take().unwrap();
        dialog.name = name.into();
        app.create_workspace_file(dialog, &context);
        finish_worker(&mut app, &context);
        assert!(
            app.rename_dialog.as_ref().unwrap().error.is_some(),
            "{name}"
        );
        assert!(!app.file_import.is_running());
        app.rename_dialog = None;
    }
    assert_eq!(
        fs::read_to_string(folder.join("existing.tex")).unwrap(),
        "keep me"
    );
    app.begin_workspace_new_file(folder.clone(), false);
    let mut dialog = app.rename_dialog.take().unwrap();
    dialog.name = "new.typ".into();
    app.create_workspace_file(dialog, &context);
    finish_worker(&mut app, &context);
    assert_eq!(fs::read(folder.join("new.typ")).unwrap(), b"");
    assert_eq!(
        app.queued_open_requests.pop_front(),
        Some(folder.join("new.typ"))
    );
}

#[test]
fn duplicate_renames_the_copy_and_keeps_original_document_untouched() {
    let root = tempfile::tempdir().unwrap();
    let root = root.path().canonicalize().unwrap();
    let path = root.join("chapter.typ");
    fs::write(&path, "saved original").unwrap();
    let context = egui::Context::default();
    let mut app = EditorApp::dormant_for_tests(&context, root.clone());
    app.document_mut().replace_loaded_unprojected(
        "unsaved original".into(),
        path.clone(),
        DocumentKind::Typst,
        None,
    );
    let key = app.document().key();
    app.duplicate_workspace_entry(path.clone(), &context);
    finish_worker(&mut app, &context);
    let dialog = app.rename_dialog.as_ref().unwrap();
    assert!(!dialog.create);
    assert_eq!(dialog.path, root.join("chapter copy.typ"));
    assert_eq!(dialog.name, "chapter copy.typ");
    assert_eq!(fs::read_to_string(&dialog.path).unwrap(), "saved original");
    assert_eq!(app.document().key(), key);
    assert_eq!(app.document().source(), "unsaved original");
    assert!(app.queued_open_requests.is_empty());
}

#[test]
fn completed_mutation_cannot_steal_another_workspace_or_an_open_dialog() {
    let root = tempfile::tempdir().unwrap();
    let root = root.path().canonicalize().unwrap();
    let other = tempfile::tempdir().unwrap();
    let path = root.join("part.typ");
    fs::write(&path, "source").unwrap();
    let context = egui::Context::default();
    let mut app = EditorApp::dormant_for_tests(&context, root.clone());
    app.duplicate_workspace_entry(path.clone(), &context);
    app.workspace_root = other.path().canonicalize().unwrap();
    finish_worker(&mut app, &context);
    assert!(app.rename_dialog.is_none());
    assert!(app.pending_workspace_dialog.is_none());
    app.workspace_root = root.clone();
    app.duplicate_workspace_entry(path.clone(), &context);
    app.begin_rename(path.clone());
    finish_worker(&mut app, &context);
    assert_eq!(app.rename_dialog.as_ref().unwrap().path, path);
    assert_eq!(
        app.pending_workspace_dialog
            .as_ref()
            .map(|(_, dialog)| &dialog.path),
        Some(&root.join("part copy 2.typ"))
    );
    app.rename_dialog = None;
    app.poll_file_import(&context);
    assert_eq!(
        app.rename_dialog.as_ref().unwrap().path,
        root.join("part copy 2.typ")
    );
}

#[test]
fn renaming_a_duplicate_keeps_collisions_and_invalid_names_in_the_prompt() {
    let root = tempfile::tempdir().unwrap();
    let root = root.path().canonicalize().unwrap();
    let copy = root.join("chapter copy.tex");
    let occupied = root.join("chapter.tex");
    fs::write(&copy, "copy").unwrap();
    fs::write(&occupied, "original").unwrap();
    let context = egui::Context::default();
    let mut app = EditorApp::dormant_for_tests(&context, root.clone());
    for name in ["chapter.tex", ".tiptoptyp", "../outside.tex"] {
        app.commit_rename(copy.clone(), name.into(), &context);
        let dialog = app.rename_dialog.take().unwrap();
        assert_eq!(dialog.path, copy);
        assert_eq!(dialog.name, name);
        assert!(dialog.error.is_some());
        assert_eq!(fs::read_to_string(&copy).unwrap(), "copy");
        assert_eq!(fs::read_to_string(&occupied).unwrap(), "original");
    }
    app.commit_rename(copy.clone(), "new.tex".into(), &context);
    assert!(app.rename_dialog.is_none());
    assert!(!copy.exists());
    assert_eq!(fs::read_to_string(root.join("new.tex")).unwrap(), "copy");
}

#[test]
fn renaming_an_open_file_outside_the_explorer_root_still_works() {
    let workspace = tempfile::tempdir().unwrap();
    let external = tempfile::tempdir().unwrap();
    let old = external.path().canonicalize().unwrap().join("external.tex");
    fs::write(&old, "original").unwrap();
    let context = egui::Context::default();
    let mut app = EditorApp::dormant_for_tests(&context, workspace.path().into());
    app.commit_rename(old.clone(), "renamed.tex".into(), &context);
    assert!(app.rename_dialog.is_none());
    assert!(!old.exists());
    assert_eq!(
        fs::read_to_string(old.with_file_name("renamed.tex")).unwrap(),
        "original"
    );
}

#[test]
fn folder_copy_cannot_be_renamed_out_from_under_an_open_child_document() {
    let root = tempfile::tempdir().unwrap();
    let root = root.path().canonicalize().unwrap();
    let copy = root.join("chapter copy");
    fs::create_dir(&copy).unwrap();
    let path = copy.join("part.tex");
    fs::write(&path, "saved").unwrap();
    let context = egui::Context::default();
    let mut app = EditorApp::dormant_for_tests(&context, root.clone());
    app.document_mut().replace_loaded_unprojected(
        "unsaved".into(),
        path.clone(),
        DocumentKind::Tex,
        None,
    );
    let key = app.document().key();
    app.commit_rename(copy.clone(), "renamed".into(), &context);
    assert!(
        app.rename_dialog
            .as_ref()
            .unwrap()
            .error
            .as_ref()
            .unwrap()
            .contains("Close files")
    );
    assert_eq!(app.document().key(), key);
    assert_eq!(app.document().path().as_ref(), Some(&path));
    assert_eq!(app.document().source(), "unsaved");
    assert_eq!(fs::read_to_string(path).unwrap(), "saved");
    assert!(!root.join("renamed").exists());
}

#[test]
fn native_rename_and_new_file_fields_own_select_all_typing_and_source_selection() {
    use std::{
        cell::{Cell, RefCell},
        rc::Rc,
    };
    for create in [false, true] {
        let context = egui::Context::default();
        context.set_embed_viewports(false);
        let child_events = Rc::new(RefCell::new(Vec::new()));
        let incoming = Rc::clone(&child_events);
        let copied = Rc::new(RefCell::new(Vec::<String>::new()));
        let child_copied = Rc::clone(&copied);
        let child_time = Cell::new(0.0);
        egui::Context::set_immediate_viewport_renderer(move |context, mut child| {
            let screen = Rect::from_min_size(
                Pos2::ZERO,
                child.builder.inner_size.unwrap_or(Vec2::new(1000.0, 600.0)),
            );
            child_time.set(child_time.get() + 1.0);
            let mut input = egui::RawInput {
                viewport_id: child.ids.this,
                time: Some(child_time.get()),
                screen_rect: Some(screen),
                events: std::mem::take(&mut *incoming.borrow_mut()),
                ..Default::default()
            };
            input.viewports.insert(
                child.ids.this,
                egui::ViewportInfo {
                    parent: Some(child.ids.parent),
                    focused: Some(true),
                    inner_rect: Some(screen),
                    ..Default::default()
                },
            );
            let output = context.run_ui(input, |ui| (child.viewport_ui_cb)(ui));
            for command in &output.platform_output.commands {
                if let egui::OutputCommand::CopyText(text) = command {
                    child_copied.borrow_mut().push(text.clone());
                }
            }
            output.drop_without_applying_deltas();
        });
        let root = tempfile::tempdir().unwrap();
        let root = root.path().canonicalize().unwrap();
        let file = root.join("original.typ");
        fs::write(&file, "saved original").unwrap();
        let mut app = EditorApp::dormant_for_tests(&context, root.clone());
        let source = "source must keep its original text and selection";
        app.document_mut().replace_unprojected_untitled(source);
        let key = app.document().key();
        let cursor = CCursorRange::two(CCursor::new(3), CCursor::new(9));
        app.store_editor_cursor(&context, cursor);
        if create {
            app.begin_workspace_new_file(root, false);
        } else {
            app.begin_rename(file.clone());
        }
        let child = scoped_child_viewport_id(&context, "tiptoptyp-rename-overlay");
        let frame = |app: &mut EditorApp| {
            let screen = Rect::from_min_size(Pos2::ZERO, Vec2::new(1000.0, 600.0));
            let mut input = egui::RawInput {
                screen_rect: Some(screen),
                ..Default::default()
            };
            let owner = input.viewports.get_mut(&egui::ViewportId::ROOT).unwrap();
            owner.focused = Some(false);
            owner.inner_rect = Some(screen);
            input.viewports.insert(
                child,
                egui::ViewportInfo {
                    parent: Some(egui::ViewportId::ROOT),
                    focused: Some(true),
                    inner_rect: Some(screen),
                    ..Default::default()
                },
            );
            context
                .run_ui(input, |ui| {
                    // The source retains an old egui focus ID after its native
                    // child takes keyboard ownership, as in the real app.
                    let source_id = source_editor_id(ui.ctx());
                    ui.memory_mut(|memory| memory.request_focus(source_id));
                    app.handle_shortcuts(ui.ctx(), None);
                    app.process_native_menu_commands(ui.ctx(), None);
                    app.show_rename_dialog(ui.ctx());
                })
                .drop_without_applying_deltas();
        };
        frame(&mut app);
        frame(&mut app);
        app.enqueue_native_menu_command(AppCommand::SelectAll);
        frame(&mut app);
        child_events
            .borrow_mut()
            .push(egui::Event::Text("replacement.typ".into()));
        frame(&mut app);
        assert_eq!(
            app.rename_dialog.as_ref().unwrap().name,
            "replacement.typ",
            "create={create}"
        );
        app.enqueue_native_menu_command(AppCommand::SelectAll);
        frame(&mut app);
        app.enqueue_native_menu_command(AppCommand::Copy);
        frame(&mut app);
        assert_eq!(
            copied.borrow().last().map(String::as_str),
            Some("replacement.typ"),
            "create={create}: copy must use the child selection"
        );
        app.enqueue_native_menu_command(AppCommand::Cut);
        frame(&mut app);
        assert_eq!(
            app.rename_dialog.as_ref().unwrap().name,
            "",
            "create={create}: cut"
        );
        app.enqueue_native_menu_command(AppCommand::Undo);
        frame(&mut app);
        assert_eq!(
            app.rename_dialog.as_ref().unwrap().name,
            "replacement.typ",
            "create={create}: undo cut"
        );
        app.enqueue_native_menu_command(AppCommand::SelectAll);
        frame(&mut app);
        app.enqueue_native_menu_command(AppCommand::Paste);
        frame(&mut app);
        // AppKit supplies this semantic event to the requested native child;
        // no OS clipboard or direct text-buffer mutation is used by the test.
        child_events
            .borrow_mut()
            .push(egui::Event::Paste("pasted.typ".into()));
        frame(&mut app);
        assert_eq!(
            app.rename_dialog.as_ref().unwrap().name,
            "pasted.typ",
            "create={create}: paste"
        );
        assert_eq!(app.document().key(), key, "create={create}");
        assert_eq!(app.document().source(), source, "create={create}");
        assert_eq!(
            app.editor_snapshot(&context).cursor,
            cursor,
            "create={create}"
        );
        assert_eq!(fs::read_to_string(file).unwrap(), "saved original");
    }
}
