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

fn click_toolbar_command(harness: &mut Harness<'_, EditorApp>, command: AppCommand, label: &str) {
    if matches!(
        command,
        AppCommand::Code
            | AppCommand::Split
            | AppCommand::Preview
            | AppCommand::Explorer
            | AppCommand::Panel
    ) {
        harness.get_by_label("Layout").click();
        harness.run();
        // `run` follows ROOT repaint requests, while a secondary viewport's
        // Area requests its own sizing/placement passes. Finish those two
        // passes before reading and clicking the final row coordinates.
        harness.run_steps(2);
        assert!(matches!(
            harness.state().app_popup,
            Some(AppPopup::Layout { .. })
        ));
        let shortcut = harness
            .state()
            .settings
            .effective_shortcuts()
            .egui(command_spec(command).shortcut_action);
        let label = shortcut.map_or_else(
            || command_spec(command).title.to_owned(),
            |shortcut| {
                format!(
                    "{} {}",
                    command_spec(command).title,
                    harness.ctx.format_shortcut(&shortcut)
                )
            },
        );
        harness.get_by_label(&label).click();
    } else {
        harness.get_by_label(label).click();
    }
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
            let mut configured = false;
            let mut harness = Harness::builder()
                .with_size(Vec2::new(width, 300.0))
                .with_os(egui::os::OperatingSystem::from_target_os())
                .build_ui_state(
                    move |ui, app: &mut EditorApp| {
                        if !configured {
                            crate::theme::configure_test_fonts(ui.ctx());
                            configured = true;
                            return;
                        }
                        app.handle_shortcuts(ui.ctx(), None);
                        app.process_native_menu_commands(ui.ctx(), None);
                        egui::Panel::top("terminal-test-toolbar")
                            .exact_size(METRICS.chrome.toolbar_height)
                            .show(ui, |ui| app.show_toolbar(ui, None));
                        layout_picker::show_test_popup(ui, app);
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
            click_toolbar_command(&mut harness, AppCommand::Panel, "Panel");
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
                    Route::Toolbar | Route::CompactToolbar => {
                        click_toolbar_command(&mut harness, AppCommand::Panel, "Panel")
                    }
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
            (
                AppCommand::Explorer,
                "Explorer",
                "Explorer",
                ViewMode::Split,
            ),
            (
                AppCommand::Settings,
                "Settings",
                "Settings",
                ViewMode::Split,
            ),
            (AppCommand::Code, "Code", "Code", ViewMode::Code),
            (AppCommand::Split, "Split", "Split", ViewMode::Split),
            (AppCommand::Preview, "Preview", "Preview", ViewMode::Preview),
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
                            let fonts = egui::Id::new("command-test-fonts-ready");
                            if !ui
                                .ctx()
                                .data(|d| d.get_temp::<bool>(fonts).unwrap_or(false))
                            {
                                theme::configure_editor_fonts(
                                    ui.ctx(),
                                    Default::default(),
                                    Default::default(),
                                    false,
                                    400,
                                    400,
                                    None,
                                );
                                ui.ctx().data_mut(|d| d.insert_temp(fonts, true));
                                return;
                            }
                            app.handle_shortcuts(ui.ctx(), None);
                            app.process_native_menu_commands(ui.ctx(), None);
                            app.show_toolbar(ui, None);
                            layout_picker::show_test_popup(ui, app);
                            // Commands hand focus to real panel/editor owners;
                            // each must exist in this harness's accessibility tree.
                            if app.bottom_panel.is_visible() {
                                app.show_bottom_panel(ui);
                            } else {
                                app.show_editor(ui);
                            }
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
                    Route::Toolbar => click_toolbar_command(&mut harness, command, label),
                    Route::CompactToolbar => {
                        click_toolbar_command(&mut harness, command, compact_label)
                    }
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
                        app.explorer.panel_visible(),
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
fn problems_keyboard_owner_blocks_source_editing_and_find_commands() {
    for owner in [
        egui::ViewportId::ROOT,
        egui::ViewportId::from_hash_of("problems-second"),
    ] {
        for route in [Route::Shortcut, Route::NativeMenu] {
            let root = tempfile::tempdir().unwrap();
            let context = egui::Context::default();
            let mut app = fixture(&context, root.path(), owner);
            app.bottom_panel.select(PanelTab::Problems);
            let mut configured = false;
            let mut harness = Harness::builder()
                .with_size(Vec2::new(900.0, 500.0))
                .with_os(egui::os::OperatingSystem::from_target_os())
                .build_ui_state(
                    move |ui, app: &mut EditorApp| {
                        if !configured {
                            theme::configure_test_fonts(ui.ctx());
                            configured = true;
                            return;
                        }
                        app.handle_shortcuts(ui.ctx(), None);
                        app.process_native_menu_commands(ui.ctx(), None);
                        app.show_bottom_panel_container(ui);
                        app.show_editor(ui);
                    },
                    app,
                );
            harness.input_mut().viewport_id = owner;
            harness.set_size(Vec2::new(900.0, 500.0));
            harness
                .input_mut()
                .viewports
                .entry(owner)
                .or_default()
                .focused = Some(true);
            harness.run();
            let problems = egui::Id::new((owner, "problems-panel"));
            let search = egui::Id::new((owner, "problems-search"));
            let source = egui::Id::new((owner, "tiptoptyp-source-editor"));
            // Context::read_response reads the current viewport (ROOT outside
            // a pass). Resolve the secondary owner's cached widget explicitly.
            let position = harness.ctx.viewport_for(owner, |viewport| {
                viewport
                    .prev_pass
                    .widgets
                    .get(problems)
                    .unwrap()
                    .interact_rect
                    .center()
            });
            harness.hover_at(position);
            harness.run();
            for pressed in [true, false] {
                harness.event(egui::Event::PointerButton {
                    pos: position,
                    button: egui::PointerButton::Primary,
                    pressed,
                    modifiers: Modifiers::NONE,
                });
                harness.run();
            }
            assert!(harness.ctx.memory(|memory| memory.has_focus(problems)));
            let key = harness.state().document().key();
            let source_selection = egui::text_edit::TextEditState::load(&harness.ctx, source)
                .and_then(|state| state.cursor.char_range());
            for command in [
                AppCommand::Find,
                AppCommand::FindReplace,
                AppCommand::SelectAll,
                AppCommand::Undo,
                AppCommand::Redo,
                AppCommand::Cut,
                AppCommand::Copy,
                AppCommand::Paste,
                AppCommand::ToggleComment,
                AppCommand::Format,
            ] {
                match route {
                    Route::Shortcut => {
                        let action = command_spec(command).shortcut_action;
                        let chord = harness
                            .state()
                            .settings
                            .effective_shortcuts()
                            .egui(action)
                            .unwrap_or_else(|| {
                                let chord = KeyboardShortcut::new(
                                    Modifiers::COMMAND | Modifiers::SHIFT,
                                    egui::Key::F11,
                                );
                                harness.state_mut().settings.shortcut_overrides.assign(
                                    action,
                                    ShortcutChord::from_egui(chord, ShortcutPlatform::current())
                                        .unwrap(),
                                    ShortcutPlatform::current(),
                                );
                                chord
                            });
                        harness.key_press_modifiers(chord.modifiers, chord.logical_key);
                    }
                    Route::NativeMenu => harness.state_mut().enqueue_native_menu_command(command),
                    _ => unreachable!(),
                }
                harness.run();
                assert!(
                    !harness.state().find_bar.visible,
                    "{owner:?} {route:?} {command:?}"
                );
                assert!(
                    !harness.state().preview_controls.open,
                    "{route:?} {command:?}"
                );
                assert_eq!(
                    harness.state().document().key(),
                    key,
                    "{route:?} {command:?}"
                );
                assert_eq!(
                    harness.state().document().source(),
                    "alpha beta",
                    "{route:?} {command:?}"
                );
                assert!(
                    harness.ctx.memory(|memory| memory.has_focus(search)),
                    "{owner:?} {route:?} {command:?}: focused={:?}, search={search:?}, requested={}",
                    harness.ctx.memory(|memory| memory.focused()),
                    harness.state().problems.focus_search
                );
                assert_eq!(
                    egui::text_edit::TextEditState::load(&harness.ctx, source)
                        .and_then(|state| state.cursor.char_range()),
                    source_selection,
                    "{route:?} {command:?}"
                );
                assert!(harness.state().format_request_key.is_none());
                assert!(harness.state().format_when_service_ready.is_none());
            }
            harness.event(egui::Event::Text("must not reach source".into()));
            harness.run();
            assert_eq!(harness.state().document().source(), "alpha beta");
            assert!(
                harness
                    .state()
                    .problems
                    .filter
                    .query
                    .contains("must not reach source")
            );
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
fn explorer_search_edit_commands_keep_fifo_before_following_native_text() {
    for (route, preceding_text) in [
        (Route::NativeMenu, None),
        (Route::Shortcut, None),
        (Route::Shortcut, Some("typed before Cmd+A")),
    ] {
        let root = tempfile::tempdir().unwrap();
        let context = egui::Context::default();
        let mut app = fixture(&context, root.path(), egui::ViewportId::ROOT);
        let mut queries = std::array::from_fn(|_| String::new());
        queries[ExplorerSection::Files.index()] = "old query".into();
        app.explorer.set_queries(queries);
        app.explorer.focus_search();
        let document_key = app.document().key();
        let mut fonts_ready = false;
        let mut harness = Harness::builder()
            .with_size(Vec2::new(700.0, 600.0))
            .with_os(egui::os::OperatingSystem::from_target_os())
            .build_ui_state(
                move |ui, app: &mut EditorApp| {
                    if !fonts_ready {
                        theme::configure_test_fonts(ui.ctx());
                        fonts_ready = true;
                        return;
                    }
                    app.handle_shortcuts(ui.ctx(), None);
                    app.process_native_menu_commands(ui.ctx(), None);
                    app.show_workspace(ui);
                },
                app,
            );
        harness.run();
        harness.get_by_label("Find in Files").click();
        harness.run();
        let command = harness
            .state()
            .settings
            .effective_shortcuts()
            .egui(ShortcutAction::SelectAll)
            .unwrap();
        if matches!(route, Route::NativeMenu) {
            harness
                .state_mut()
                .enqueue_native_menu_command(AppCommand::SelectAll);
            harness
                .state_mut()
                .enqueue_native_menu_command(AppCommand::Copy);
        } else {
            if let Some(text) = preceding_text {
                harness.event(egui::Event::Text(text.into()));
            }
            harness.event(key_event(command));
        }
        harness.event(egui::Event::Text("replacement quer".into()));
        // Native commands are older than this typing, but can be delivered in
        // its paint. Copy must see the old selected query, then typing replaces it.
        harness.step();
        assert_eq!(
            harness.state().explorer.query(ExplorerSection::Files),
            "replacement quer",
            "{route:?}",
        );
        if matches!(route, Route::NativeMenu) {
            assert!(harness.output().platform_output.commands.iter().any(
                |command| matches!(command, egui::OutputCommand::CopyText(text) if text == "old query")
            ), "native menu edits must retain SelectAll → Copy ordering");
        }
        harness.event(egui::Event::Text("y".into()));
        harness.run();
        assert_eq!(
            harness.state().explorer.query(ExplorerSection::Files),
            "replacement query",
            "{route:?}",
        );
        assert_eq!(harness.state().document().source(), "alpha beta");
        assert_eq!(harness.state().document().key(), document_key);
    }
}

#[test]
fn parent_passes_do_not_replay_already_painted_child_edit_chords() {
    let root = tempfile::tempdir().unwrap();
    let context = egui::Context::default();
    let mut app = fixture(&context, root.path(), egui::ViewportId::ROOT);
    let child = scoped_child_viewport_id(&context, preview_controls::SALT);
    app.preview_controls.query = "old query".into();
    let field = egui::Id::new("cross-pass-preview-query");
    let raw = |viewport_id, events| {
        let mut input = egui::RawInput {
            viewport_id,
            events,
            ..Default::default()
        };
        input
            .viewports
            .entry(egui::ViewportId::ROOT)
            .or_default()
            .focused = Some(false);
        let child_info = input.viewports.entry(child).or_default();
        child_info.parent = Some(egui::ViewportId::ROOT);
        child_info.focused = Some(true);
        input
    };
    let paint_child = |app: &mut EditorApp, events| {
        context
            .run_ui(raw(child, events), |ui| {
                let shortcuts = app.settings.effective_shortcuts();
                let _physical = prepare_child_widget_edit_events(ui.ctx(), &shortcuts, false);
                let _queued = crate::child_view::begin_queued_edit_events(
                    ui.ctx(),
                    &mut app.preview_controls.edit_events,
                );
                ui.ctx().memory_mut(|memory| memory.request_focus(field));
                ui.add(egui::TextEdit::singleline(&mut app.preview_controls.query).id(field));
            })
            .drop_without_applying_deltas();
    };
    let parent_pass = |app: &mut EditorApp| {
        context
            .run_ui(raw(egui::ViewportId::ROOT, Vec::new()), |ui| {
                app.handle_shortcuts(ui.ctx(), None);
                app.process_native_menu_commands(ui.ctx(), None);
            })
            .drop_without_applying_deltas();
    };
    paint_child(&mut app, Vec::new());
    paint_child(
        &mut app,
        vec![egui::Event::Key {
            key: egui::Key::A,
            physical_key: Some(egui::Key::A),
            pressed: true,
            repeat: false,
            modifiers: Modifiers::COMMAND,
        }],
    );
    // Native child painting runs before its parent gets another pass. Its
    // InputState still contains the key TextEdit just handled; it is not new
    // parent input and must not be sent back to that child.
    parent_pass(&mut app);
    paint_child(&mut app, vec![egui::Event::Text("preview search f".into())]);
    parent_pass(&mut app);
    paint_child(&mut app, vec![egui::Event::Text("or Pdfium".into())]);
    assert_eq!(app.preview_controls.query, "preview search for Pdfium");
    for _ in 0..3 {
        parent_pass(&mut app);
        paint_child(&mut app, Vec::new());
        assert!(app.preview_controls.edit_events.is_empty());
        assert_eq!(app.preview_controls.query, "preview search for Pdfium");
    }
    // A custom physical chord must be translated locally without its
    // generated canonical Cmd+A becoming the parent's rebound Explorer key.
    for (action, key) in [
        (ShortcutAction::SelectAll, egui::Key::L),
        (ShortcutAction::Explorer, egui::Key::A),
    ] {
        app.settings.shortcut_overrides.assign(
            action,
            ShortcutChord::from_egui(
                KeyboardShortcut::new(Modifiers::COMMAND, key),
                ShortcutPlatform::current(),
            )
            .unwrap(),
            ShortcutPlatform::current(),
        );
    }
    let explorer_visible = app.explorer.panel_visible();
    paint_child(
        &mut app,
        vec![key_event(KeyboardShortcut::new(
            Modifiers::COMMAND,
            egui::Key::L,
        ))],
    );
    parent_pass(&mut app);
    paint_child(&mut app, vec![egui::Event::Text("custom first".into())]);
    parent_pass(&mut app);
    paint_child(&mut app, vec![egui::Event::Text(" second".into())]);
    assert_eq!(app.preview_controls.query, "custom first second");
    assert_eq!(app.explorer.panel_visible(), explorer_visible);

    // A genuine menu Select All retains its standard TextEdit semantics even
    // when Cmd+A now names another action, and is retired after the child paint.
    app.enqueue_native_menu_command(AppCommand::SelectAll);
    parent_pass(&mut app);
    paint_child(&mut app, vec![egui::Event::Text("native first".into())]);
    parent_pass(&mut app);
    paint_child(&mut app, vec![egui::Event::Text(" second".into())]);
    assert_eq!(app.preview_controls.query, "native first second");
    assert_eq!(app.explorer.panel_visible(), explorer_visible);

    // Conversely, the real rebound global key must reach the parent once and
    // must never accidentally select the child's text via TextEdit's builtin.
    paint_child(
        &mut app,
        vec![key_event(KeyboardShortcut::new(
            Modifiers::COMMAND,
            egui::Key::A,
        ))],
    );
    parent_pass(&mut app);
    assert_eq!(app.explorer.panel_visible(), !explorer_visible);
    paint_child(&mut app, vec![egui::Event::Text("!".into())]);
    parent_pass(&mut app);
    assert_eq!(app.preview_controls.query, "native first second!");
    assert_eq!(app.explorer.panel_visible(), !explorer_visible);
    assert_eq!(app.document().source(), "alpha beta");
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
            .build_ui_state(
                |ui, app: &mut EditorApp| {
                    app.show_toolbar(ui, None);
                    layout_picker::show_test_popup(ui, app);
                },
                app,
            );
        harness.run();
        harness.get_by_label("Layout").click();
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
            let label = if matches!(command, AppCommand::Find | AppCommand::Settings) {
                label.to_owned()
            } else {
                harness
                    .state()
                    .settings
                    .effective_shortcuts()
                    .egui(command_spec(command).shortcut_action)
                    .map_or_else(
                        || label.to_owned(),
                        |shortcut| format!("{label} {}", harness.ctx.format_shortcut(&shortcut)),
                    )
            };
            assert_eq!(
                !harness.get_by_label(&label).accesskit_node().is_disabled(),
                harness.state().native_command_enabled(command),
                "{kind:?} {command:?}"
            );
        }
    }
}

#[test]
fn deferred_find_select_all_is_delivered_once_to_query_without_editing_source() {
    use egui_kittest::Harness;
    let directory = tempfile::tempdir().unwrap();
    let context = egui::Context::default();
    let mut app = EditorApp::dormant_for_tests(&context, directory.path().into());
    app.document_mut()
        .replace_unprojected_untitled("keep this source");
    app.find_bar.visible = true;
    app.find_bar.focus = true;
    app.find_bar.query = "old query".into();
    let mut harness = Harness::builder()
        .with_size(egui::vec2(700.0, 100.0))
        .build_ui_state(|ui, app: &mut EditorApp| app.show_find_bar(ui), app);
    harness.run_steps(3);
    let child =
        crate::child_view::child_viewport_id(egui::ViewportId::ROOT, find_bar::VIEWPORT_SALT);
    assert!(harness.state_mut().route_edit_command_to_focused_widget(
        AppCommand::SelectAll,
        &context,
        child
    ));
    harness.run_steps(2);
    harness.event(egui::Event::Text("new".into()));
    harness.run_steps(2);
    assert_eq!(harness.state().find_bar.query, "new");
    assert!(harness.state().find_bar.edit_events.is_empty());
    assert_eq!(harness.state().document().source(), "keep this source");
}

#[test]
fn explorer_drop_inserts_a_relative_path_at_the_caret_and_undoes_once() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().canonicalize().unwrap();
    let context = egui::Context::default();
    let mut app = EditorApp::dormant_for_tests(&context, root.clone());
    app.document_mut().replace_loaded_unprojected(
        "ab".into(),
        root.join("main.typ"),
        DocumentKind::Typst,
        None,
    );
    app.document_mut().set_history_reset(false);
    fs::write(root.join("image.png"), b"fixture").unwrap();
    app.workspace = Some(WorkspaceTree::from_snapshot(
        WorkspaceSnapshot::scan(&root).unwrap(),
    ));
    let mut configured = false;
    let mut caret_set = false;
    let mut harness = Harness::builder()
        .with_size(egui::vec2(900.0, 500.0))
        .build_ui_state(
            move |ui, app: &mut EditorApp| {
                if !configured {
                    theme::configure_editor_fonts(
                        ui.ctx(),
                        Default::default(),
                        Default::default(),
                        false,
                        400,
                        400,
                        None,
                    );
                    configured = true;
                    return;
                }
                if !caret_set {
                    app.store_editor_cursor(ui.ctx(), CCursorRange::one(CCursor::new(1)));
                    caret_set = true;
                }
                egui::Panel::left("explorer")
                    .exact_size(250.0)
                    .show(ui, |ui| app.show_workspace(ui));
                egui::CentralPanel::default().show(ui, |ui| app.show_editor(ui));
            },
            app,
        );
    harness.run_steps(3);
    let start = harness.get_by_label("image.png").rect().center();
    let end = Pos2::new(600.0, 200.0);
    harness.event(egui::Event::PointerMoved(start));
    harness.run_steps(1);
    harness.event(egui::Event::PointerButton {
        pos: start,
        button: egui::PointerButton::Primary,
        pressed: true,
        modifiers: Modifiers::NONE,
    });
    harness.run_steps(1);
    for step in 1..=10 {
        harness.event(egui::Event::PointerMoved(
            start.lerp(end, step as f32 / 10.0),
        ));
        harness.run_steps(1);
    }
    harness.event(egui::Event::PointerButton {
        pos: end,
        button: egui::PointerButton::Primary,
        pressed: false,
        modifiers: Modifiers::NONE,
    });
    harness.run_steps(3);
    assert_eq!(harness.state().document().source(), "aimage.pngb");
    let context = harness.ctx.clone();
    harness.state_mut().undo_editor(&context, false);
    assert_eq!(harness.state().document().source(), "ab");
}

#[test]
fn find_in_live_diff_is_visible_and_owns_typing_without_editing_source() {
    let root = tempfile::tempdir().unwrap();
    let context = egui::Context::default();
    let mut app = fixture(&context, root.path(), egui::ViewportId::ROOT);
    let path = root.path().join("main.typ");
    let source = (0..17)
        .map(|i| format!("alpha line {i}\n"))
        .collect::<String>();
    app.document_mut().replace_loaded_unprojected(
        source.clone(),
        path.clone(),
        DocumentKind::Typst,
        None,
    );
    app.git_editor = crate::git::editor::GitEditorState::document_fixture(
        root.path(),
        &path,
        &source,
        app.document().key(),
    );
    app.git_editor.open_chunk(0, &path);
    app.open_app_popup(AppPopup::GitChunk {
        anchor: Pos2::ZERO,
        chunk: app.git_editor.chunk.clone().unwrap(),
    });
    let mut ready = false;
    let mut harness = Harness::builder()
        .with_size(Vec2::new(900.0, 500.0))
        .build_ui_state(
            move |ui, app: &mut EditorApp| {
                if !ready {
                    theme::configure_editor_fonts(
                        ui.ctx(),
                        Default::default(),
                        Default::default(),
                        false,
                        400,
                        400,
                        None,
                    );
                    ready = true;
                    return;
                }
                app.handle_shortcuts(ui.ctx(), None);
                app.show_editor(ui);
            },
            app,
        );
    harness.run_steps(4);
    let chord = harness
        .state()
        .settings
        .effective_shortcuts()
        .egui(ShortcutAction::Find)
        .unwrap();
    harness.key_press_modifiers(chord.modifiers, chord.logical_key);
    harness.run_steps(4);
    harness.get_by_label("Next match · Enter");
    assert!(find_bar::has_focus(&harness.ctx));
    harness.event(egui::Event::Text("alpha".into()));
    harness.run_steps(4);
    assert_eq!(harness.state().find_bar.query, "alpha");
    assert!(find_bar::has_focus(&harness.ctx));
    assert_eq!(harness.state().document().source(), &source);
    assert!(harness.state().document_diff.is_some());
    harness.key_press_modifiers(chord.modifiers, chord.logical_key);
    harness.run_steps(4);
    assert!(!harness.state().find_bar.visible);
    assert!(harness.state().document_diff.is_some());
    let editor = source_editor_id(&harness.ctx);
    assert!(harness.ctx.memory(|m| m.has_focus(editor)));
}
