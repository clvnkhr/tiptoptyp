use super::*;
use egui_kittest::{
    Harness,
    kittest::{NodeT as _, Queryable as _},
};

#[derive(Clone, Copy, Debug)]
enum Route {
    Shortcut,
    NativeMenu,
    Toolbar,
    CompactToolbar,
}

fn fixture(context: &egui::Context, root: &Path, owner: egui::ViewportId) -> EditorApp {
    let mut app = EditorApp::dormant_window_for_tests(context, root.into(), owner);
    app.settings.titlebar_menus = false;
    app.document_mut()
        .replace_unprojected_untitled("alpha beta");
    app.document_mut().set_history_reset(false);
    app.find_visible = false;
    app.problems_visible = false;
    app.explorer.open();
    app.view_mode = ViewMode::Split;
    app
}

fn key_event(shortcut: KeyboardShortcut) -> egui::Event {
    egui::Event::Key {
        key: shortcut.logical_key,
        physical_key: None,
        pressed: true,
        repeat: false,
        modifiers: shortcut.modifiers,
    }
}

#[test]
fn command_toolbar_menu_and_shortcut_effects_agree_once_per_owner() {
    for owner in [
        egui::ViewportId::ROOT,
        egui::ViewportId::from_hash_of("commands-second"),
    ] {
        for (command, label, compact_label, view) in [
            (AppCommand::Find, "Find", "Find", ViewMode::Split),
            (AppCommand::Problems, "Problems", "!", ViewMode::Split),
            (AppCommand::Explorer, "Explorer", "Files", ViewMode::Split),
            (AppCommand::Settings, "Settings", "Set", ViewMode::Split),
            (AppCommand::Code, "Code", "C", ViewMode::Code),
            (AppCommand::Split, "Split", "S", ViewMode::Split),
            (AppCommand::Preview, "Preview", "P", ViewMode::Preview),
        ] {
            for route in [
                Route::Shortcut,
                Route::NativeMenu,
                Route::Toolbar,
                Route::CompactToolbar,
            ] {
                let root = tempfile::tempdir().unwrap();
                let context = egui::Context::default();
                let app = fixture(&context, root.path(), owner);
                let key = app.document().key();
                let width = if matches!(route, Route::CompactToolbar) {
                    600.0
                } else {
                    1800.0
                };
                let mut harness = Harness::builder()
                    .with_size(Vec2::new(width, 160.0))
                    .with_os(egui::os::OperatingSystem::from_target_os())
                    .build_ui_state(
                        |ui, app: &mut EditorApp| {
                            app.handle_shortcuts(ui.ctx(), None);
                            app.process_native_menu_commands(ui.ctx(), None);
                            app.show_toolbar(ui, None);
                        },
                        app,
                    );
                harness.input_mut().viewport_id = owner;
                harness.set_size(Vec2::new(width, 160.0));
                harness
                    .input_mut()
                    .viewports
                    .entry(owner)
                    .or_default()
                    .focused = Some(true);
                harness.run();
                assert!(harness.state().native_command_enabled(command));
                match route {
                    Route::Toolbar => harness.get_by_label(label).click(),
                    Route::CompactToolbar => harness.get_by_label(compact_label).click(),
                    Route::NativeMenu => harness.state_mut().enqueue_native_menu_command(command),
                    Route::Shortcut => {
                        let chord = harness
                            .state()
                            .settings
                            .effective_shortcuts()
                            .egui(command_spec(command).shortcut_action)
                            .unwrap();
                        harness.key_press_modifiers(chord.modifiers, chord.logical_key);
                    }
                }
                harness.run();
                // Include later idle passes: a replayed toggle would undo its first effect.
                for _ in 0..2 {
                    harness.step();
                    let app = harness.state();
                    assert_eq!(
                        app.find_visible,
                        command == AppCommand::Find,
                        "{route:?} {command:?}"
                    );
                    assert_eq!(app.problems_visible, command == AppCommand::Problems);
                    assert_eq!(
                        app.explorer.contents_visible(),
                        command != AppCommand::Explorer,
                        "{owner:?} {route:?} {command:?}"
                    );
                    assert_eq!(app.settings_open_requested, command == AppCommand::Settings);
                    assert_eq!(app.view_mode, view);
                    assert_eq!(app.document().key(), key);
                    assert!(app.compile_deadline.is_none());
                    assert!(app.tinymist_sync.generation.is_none());
                }
                assert_eq!(
                    harness.state_mut().take_settings_open_request(),
                    command == AppCommand::Settings
                );
                assert!(!harness.state_mut().take_settings_open_request());
            }
        }
    }
}

#[test]
fn command_admission_covers_empty_workspace_busy_file_flow_and_document_kind() {
    for owner in [
        egui::ViewportId::ROOT,
        egui::ViewportId::from_hash_of("admission-second"),
    ] {
        // Menu availability and file-flow admission are separate existing gates.
        for (command, empty, busy, kind, enabled, new_windows) in [
            (
                AppCommand::NewWindow,
                false,
                false,
                DocumentKind::Typst,
                true,
                1,
            ),
            (
                AppCommand::NewWindow,
                false,
                true,
                DocumentKind::Typst,
                true,
                0,
            ),
            (
                AppCommand::NewWindow,
                true,
                false,
                DocumentKind::Typst,
                true,
                1,
            ),
            (AppCommand::Save, false, true, DocumentKind::Typst, true, 0),
            (AppCommand::New, false, true, DocumentKind::Typst, true, 0),
            (
                AppCommand::CloseTab,
                false,
                true,
                DocumentKind::Typst,
                true,
                0,
            ),
            (AppCommand::Find, true, false, DocumentKind::Typst, false, 0),
            (
                AppCommand::CloseTab,
                true,
                false,
                DocumentKind::Typst,
                false,
                0,
            ),
            (AppCommand::Code, false, false, DocumentKind::Text, false, 0),
            (
                AppCommand::Format,
                false,
                false,
                DocumentKind::Text,
                false,
                0,
            ),
        ] {
            for route in [Route::Shortcut, Route::NativeMenu] {
                let root = tempfile::tempdir().unwrap();
                let context = egui::Context::default();
                let mut app = fixture(&context, root.path(), owner);
                if empty {
                    app.empty_workspace(&context);
                } else if kind != DocumentKind::Typst {
                    app.document_mut().replace_loaded_unprojected(
                        "plain".into(),
                        root.path().join("plain.txt"),
                        kind,
                        None,
                    );
                }
                if busy {
                    app.document_workflow.start_dialog(PendingDialog::new(
                        DocumentDialogRequest {
                            target: DocumentDialogTarget::OpenFolder,
                            key: app.document().key(),
                        },
                        std::future::pending(),
                    ));
                }
                assert_eq!(app.native_command_enabled(command), enabled);
                let key = app.document().key();
                let mut raw = egui::RawInput {
                    viewport_id: owner,
                    ..Default::default()
                };
                raw.viewports.entry(owner).or_default().focused = Some(true);
                if matches!(route, Route::Shortcut) {
                    raw.events.push(key_event(
                        app.settings
                            .effective_shortcuts()
                            .egui(command_spec(command).shortcut_action)
                            .unwrap(),
                    ));
                } else {
                    app.enqueue_native_menu_command(command);
                }
                for _ in 0..2 {
                    context
                        .run_ui(raw.clone(), |ui| {
                            app.handle_shortcuts(ui.ctx(), None);
                            app.process_native_menu_commands(ui.ctx(), None);
                        })
                        .drop_without_applying_deltas();
                    raw.events.clear();
                }
                assert_eq!(
                    app.pending_window_requests.len(),
                    new_windows,
                    "{route:?} {command:?}"
                );
                assert_eq!(app.document().key(), key);
                assert_eq!(app.tabs.is_empty(), empty);
                assert_eq!(app.document_workflow.has_dialog(), busy);
                assert!(!app.find_visible);
                assert_eq!(app.view_mode, ViewMode::Split);
                assert!(app.compile_deadline.is_none());
            }
        }
    }
}

#[test]
fn command_select_all_obeys_source_or_find_focus() {
    for find in [false, true] {
        for route in [Route::Shortcut, Route::NativeMenu] {
            let root = tempfile::tempdir().unwrap();
            let context = egui::Context::default();
            let mut app = fixture(&context, root.path(), egui::ViewportId::ROOT);
            app.find_query = "beta".into();
            app.find_visible = find;
            app.focus_find = find;
            if !find {
                app.pending_editor_selection = Some(EditorSelection::Focus(0..0));
            }
            let paint = |app: &mut EditorApp, ui: &mut egui::Ui| {
                if find {
                    app.show_find_bar(ui);
                }
                app.show_editor(ui);
            };
            for _ in 0..2 {
                context
                    .run_ui(Default::default(), |ui| paint(&mut app, ui))
                    .drop_without_applying_deltas();
            }
            let focused = context.memory(|m| m.focused()).unwrap();
            assert_eq!(focused == source_editor_id(&context), !find);
            let key = app.document().key();
            let mut raw = egui::RawInput::default();
            if matches!(route, Route::Shortcut) {
                raw.events.push(key_event(
                    app.settings
                        .effective_shortcuts()
                        .egui(ShortcutAction::SelectAll)
                        .unwrap(),
                ));
            } else {
                app.enqueue_native_menu_command(AppCommand::SelectAll);
            }
            context
                .run_ui(raw, |ui| {
                    app.handle_shortcuts(ui.ctx(), None);
                    app.process_native_menu_commands(ui.ctx(), None);
                    paint(&mut app, ui);
                })
                .drop_without_applying_deltas();
            let selection = egui::text_edit::TextEditState::load(&context, focused)
                .unwrap()
                .cursor
                .char_range()
                .unwrap()
                .as_sorted_char_range();
            assert_eq!(
                selection.start.0..selection.end.0,
                0..if find { 4 } else { 10 },
                "{route:?}"
            );
            assert_eq!(app.document().key(), key);
        }
    }
}

#[test]
fn command_completion_and_capture_consume_keys_before_global_actions() {
    let root = tempfile::tempdir().unwrap();
    let context = egui::Context::default();
    let mut app = fixture(&context, root.path(), egui::ViewportId::ROOT);
    app.find_visible = true;
    app.document_mut()
        .replace_unprojected_untitled("#mi(`\\alp`)");
    app.request_editor_completion(9, Rect::ZERO, true);
    assert!(!app.editor_completion.as_ref().unwrap().items.is_empty());
    let raw = egui::RawInput {
        events: vec![key_event(KeyboardShortcut::new(
            Modifiers::NONE,
            egui::Key::Escape,
        ))],
        ..Default::default()
    };
    context
        .run_ui(raw, |ui| app.handle_shortcuts(ui.ctx(), None))
        .drop_without_applying_deltas();
    assert!(app.editor_completion.is_none());
    assert!(
        app.find_visible,
        "completion dismiss owns Escape before Find"
    );

    app.shortcut_capture = Some(ShortcutAction::Find);
    let raw = egui::RawInput {
        events: vec![key_event(
            app.settings
                .effective_shortcuts()
                .egui(ShortcutAction::Explorer)
                .unwrap(),
        )],
        ..Default::default()
    };
    context
        .run_ui(raw, |ui| app.handle_shortcuts(ui.ctx(), None))
        .drop_without_applying_deltas();
    assert!(app.shortcut_capture.is_none());
    assert!(
        app.explorer.panel_visible(),
        "captured chord must not execute Explorer"
    );
    assert!(app.pending_settings.is_some());
}

#[test]
fn command_titlebar_menus_switch_then_toggle_closed() {
    let root = tempfile::tempdir().unwrap();
    let context = egui::Context::default();
    let mut app = fixture(&context, root.path(), egui::ViewportId::ROOT);
    app.settings.titlebar_menus = true;
    let mut harness = Harness::builder()
        .with_size(Vec2::new(1800.0, 160.0))
        .build_ui_state(|ui, app: &mut EditorApp| app.show_toolbar(ui, None), app);
    harness.run();
    for label in ["File", "Edit", "View"] {
        harness.get_by_label(label).click();
        harness.run();
        assert!(matches!(
            (label, &harness.state().app_popup),
            ("File", Some(AppPopup::File { .. }))
                | ("Edit", Some(AppPopup::Edit { .. }))
                | ("View", Some(AppPopup::View { .. }))
        ));
    }
    harness.get_by_label("View").click();
    harness.run();
    assert!(harness.state().app_popup.is_none());
}

#[test]
fn command_toolbar_availability_matches_document_and_empty_workspace_admission() {
    for kind in [
        None,
        Some(DocumentKind::Typst),
        Some(DocumentKind::Text),
        Some(DocumentKind::Pdf),
        Some(DocumentKind::Image),
    ] {
        let root = tempfile::tempdir().unwrap();
        let context = egui::Context::default();
        let mut app = fixture(&context, root.path(), egui::ViewportId::ROOT);
        if let Some(kind) = kind {
            app.document_mut().replace_loaded_unprojected(
                "text".into(),
                root.path().join("file"),
                kind,
                None,
            );
        } else {
            app.empty_workspace(&context);
        }
        let mut harness = Harness::builder()
            .with_size(Vec2::new(1800.0, 160.0))
            .build_ui_state(|ui, app: &mut EditorApp| app.show_toolbar(ui, None), app);
        harness.run();
        for (command, label) in [
            (AppCommand::Find, "Find"),
            (AppCommand::Problems, "Problems"),
            (AppCommand::Explorer, "Explorer"),
            (AppCommand::Settings, "Settings"),
            (AppCommand::Code, "Code"),
            (AppCommand::Split, "Split"),
            (AppCommand::Preview, "Preview"),
        ] {
            assert_eq!(
                !harness.get_by_label(label).accesskit_node().is_disabled(),
                harness.state().native_command_enabled(command),
                "{kind:?} {command:?}"
            );
        }
    }
}
