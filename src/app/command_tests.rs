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
    app.find_bar.visible = false;
    app.bottom_panel = BottomPanel::default();
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
fn find_shortcuts_refocus_resume_and_only_close_when_focused() {
    for replace in [false, true] {
        let root = tempfile::tempdir().unwrap();
        let context = egui::Context::default();
        let mut app = fixture(&context, root.path(), egui::ViewportId::ROOT);
        app.document_mut()
            .replace_unprojected_untitled("alpha alpha alpha");
        app.find_bar.query = "alpha".into();
        let mut fonts_configured = false;
        let mut harness = Harness::builder()
            .with_size(Vec2::new(900.0, 500.0))
            .build_ui_state(
                move |ui, app: &mut EditorApp| {
                    if !fonts_configured {
                        theme::configure_editor_fonts(
                            ui.ctx(),
                            Default::default(),
                            Default::default(),
                            false,
                            400,
                            400,
                            None,
                        );
                        fonts_configured = true;
                        return;
                    }
                    app.handle_shortcuts(ui.ctx(), None);
                    app.show_editor(ui);
                },
                app,
            );
        harness.run();
        let action = if replace {
            ShortcutAction::FindReplace
        } else {
            ShortcutAction::Find
        };
        let chord = harness
            .state()
            .settings
            .effective_shortcuts()
            .egui(action)
            .unwrap();
        harness.key_press_modifiers(chord.modifiers, chord.logical_key);
        harness.run();
        assert!(harness.state().find_bar.visible);
        assert_eq!(harness.state().find_bar.replace_visible, replace);
        harness.get_by_label("Next match · Enter").click();
        harness.run();
        harness.get_by_label("Next match · Enter").click();
        harness.run();
        harness.get_by_label("2/3");
        // Restore source keyboard focus without changing the search session.
        let context = harness.ctx.clone();
        let editor = source_editor_id(&context);
        context.memory_mut(|memory| memory.request_focus(editor));
        harness.step();
        harness.key_press_modifiers(chord.modifiers, chord.logical_key);
        harness.run();
        assert!(harness.state().find_bar.visible, "refocus must not close");
        assert!(find_bar::has_focus(&context));
        assert_eq!(harness.state().find_bar.search.selected_ordinal(), Some(2));
        harness.get_by_label("2/3");
        assert_eq!(
            harness
                .state()
                .find_bar
                .search
                .selected()
                .unwrap()
                .char_range,
            6..11
        );
        assert_eq!(
            harness
                .state_mut()
                .editor_snapshot(&context)
                .cursor
                .primary
                .index
                .0,
            11
        );
        harness.key_press_modifiers(chord.modifiers, chord.logical_key);
        harness.run();
        assert!(!harness.state().find_bar.visible);
        harness.key_press_modifiers(chord.modifiers, chord.logical_key);
        harness.run();
        harness.get_by_label("2/3");
        harness.get_by_label("Next match · Enter").click();
        harness.run();
        harness.get_by_label("3/3");
        harness.get_by_label("Next match · Enter").click();
        harness.run();
        harness.get_by_label("1/3");
        harness.key_press(egui::Key::Escape);
        harness.run();
        assert!(!harness.state().find_bar.visible);
        assert_eq!(harness.state().find_bar.search.selected_ordinal(), Some(1));
    }
}

#[test]
fn panel_routes_close_and_reopen_a_focused_terminal_without_mutating_source() {
    for owner in [
        egui::ViewportId::ROOT,
        egui::ViewportId::from_hash_of("terminal-second"),
    ] {
        for route in [
            Route::Toolbar,
            Route::CompactToolbar,
            Route::NativeMenu,
            Route::Shortcut,
        ] {
            let root = tempfile::tempdir().unwrap();
            let context = egui::Context::default();
            let mut app = fixture(&context, root.path(), owner);
            app.terminal.prepare_fixture(b"test", Path::new("/project"));
            let width = if matches!(route, Route::CompactToolbar) {
                600.0
            } else {
                1800.0
            };
            let mut harness = Harness::builder()
                .with_size(Vec2::new(width, 300.0))
                .with_os(egui::os::OperatingSystem::from_target_os())
                .build_ui_state(
                    |ui, app: &mut EditorApp| {
                        app.handle_shortcuts(ui.ctx(), None);
                        app.process_native_menu_commands(ui.ctx(), None);
                        egui::Panel::top("terminal-test-toolbar")
                            .exact_size(METRICS.chrome.toolbar_height)
                            .show(ui, |ui| app.show_toolbar(ui, None));
                        if app.bottom_panel.is_visible() {
                            app.show_bottom_panel(ui);
                        }
                    },
                    app,
                );
            harness.input_mut().viewport_id = owner;
            harness.set_size(Vec2::new(width, 300.0));
            harness
                .input_mut()
                .viewports
                .entry(owner)
                .or_default()
                .focused = Some(true);
            harness.run();
            harness.get_by_label("Panel").click();
            harness.run();
            assert_eq!(
                harness.state().bottom_panel.selected(),
                Some(PanelTab::Problems)
            );
            harness.get_by_label("Terminal").click();
            harness.run();
            harness.get_by_label("Terminal input").click();
            harness.run();
            // Outside an egui pass the Context reports ROOT, even when its
            // last input pass belonged to a secondary document viewport.
            let id = egui::Id::new((owner, "terminal-grid"));
            assert!(
                harness.ctx.memory(|memory| memory.has_focus(id)),
                "{owner:?} {route:?}"
            );
            for visible in [false, true] {
                match route {
                    Route::Toolbar | Route::CompactToolbar => harness.get_by_label("Panel").click(),
                    Route::NativeMenu => harness
                        .state_mut()
                        .enqueue_native_menu_command(AppCommand::Panel),
                    Route::Shortcut => {
                        let shortcut = harness
                            .state()
                            .settings
                            .effective_shortcuts()
                            .egui(ShortcutAction::Panel)
                            .unwrap();
                        harness.key_press_modifiers(shortcut.modifiers, shortcut.logical_key);
                    }
                }
                harness.run();
                assert_eq!(
                    harness.state().bottom_panel.is_visible(),
                    visible,
                    "{route:?}"
                );
                assert_eq!(
                    harness.ctx.memory(|memory| memory.has_focus(id)),
                    visible,
                    "{route:?}"
                );
                assert_eq!(harness.state().document().source(), "alpha beta");
                if visible {
                    assert_eq!(
                        harness.state().bottom_panel.selected(),
                        Some(PanelTab::Terminal)
                    );
                }
            }
        }
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
            (AppCommand::Panel, "Panel", "Panel", ViewMode::Split),
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
                        app.find_bar.visible,
                        command == AppCommand::Find,
                        "{route:?} {command:?}"
                    );
                    assert_eq!(
                        app.bottom_panel.selected() == Some(PanelTab::Problems),
                        command == AppCommand::Panel
                    );
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
                assert!(!app.find_bar.visible);
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
            app.find_bar.query = "beta".into();
            app.find_bar.visible = find;
            app.find_bar.focus = find;
            if !find {
                app.pending_editor_selection = Some(EditorSelection::Focus(0..0));
            }
            let paint = |app: &mut EditorApp, ui: &mut egui::Ui| {
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
    app.find_bar.visible = true;
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
        app.find_bar.visible,
        "completion dismiss owns Escape before Find"
    );

    app.shortcut_editor.capture = Some(ShortcutAction::Find);
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
    assert!(app.shortcut_editor.capture.is_none());
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
            (AppCommand::Panel, "Panel"),
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
