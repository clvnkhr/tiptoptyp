//! Palette integration checks use real command routing and text-edit state.
use super::*;

fn fixture(context: &egui::Context, root: &Path) -> EditorApp {
    let mut app = EditorApp::dormant_for_tests(context, root.into());
    app.document_mut()
        .replace_unprojected_untitled("alpha beta gamma");
    app.document_mut().set_history_reset(false);
    app.pending_editor_selection = Some(EditorSelection::Focus(2..7));
    for _ in 0..2 {
        context
            .run_ui(input(None, Vec::new()), |ui| app.show_editor(ui))
            .drop_without_applying_deltas();
    }
    assert_eq!(selection(context), 2..7);
    app
}

fn input(child: Option<&'static str>, events: Vec<egui::Event>) -> egui::RawInput {
    let mut input = egui::RawInput {
        screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(900.0, 500.0))),
        events,
        ..Default::default()
    };
    input
        .viewports
        .entry(egui::ViewportId::ROOT)
        .or_default()
        .focused = Some(child.is_none());
    if let Some(salt) = child {
        let child = crate::child_view::child_viewport_id(egui::ViewportId::ROOT, salt);
        input.viewports.insert(
            child,
            egui::ViewportInfo {
                parent: Some(egui::ViewportId::ROOT),
                focused: Some(true),
                ..Default::default()
            },
        );
    }
    input
}

fn key(chord: KeyboardShortcut) -> egui::Event {
    egui::Event::Key {
        key: chord.logical_key,
        physical_key: None,
        pressed: true,
        repeat: false,
        modifiers: chord.modifiers,
    }
}

fn selection(context: &egui::Context) -> std::ops::Range<usize> {
    let range = egui::text_edit::TextEditState::load(context, source_editor_id(context))
        .expect("the real source editor has stored its state")
        .cursor
        .char_range()
        .unwrap()
        .as_sorted_char_range();
    range.start.0..range.end.0
}

fn route(context: &egui::Context, app: &mut EditorApp, raw: egui::RawInput) {
    context
        .run_ui(raw, |ui| {
            app.handle_shortcuts(ui.ctx(), None);
            app.process_native_menu_commands(ui.ctx(), None);
            app.show_editor(ui);
        })
        .drop_without_applying_deltas();
}

/// Exercise the native host's query body without creating a desktop window.
/// Queued native edits are delivered and retired around its actual TextEdit.
fn query_frame(
    context: &egui::Context,
    app: &mut EditorApp,
    events: Vec<egui::Event>,
) -> Vec<egui::OutputCommand> {
    let popup_events = events.clone();
    let output = context.run_ui(input(Some("tiptoptyp-popup-overlay"), events), |ui| {
        // The owner router reads the focused child's InputState. The query
        // body is rendered deterministically in this viewport, so move its
        // physical events through that child state first and paint only the
        // events the actual parent router leaves for the field.
        let popup = scoped_child_viewport_id(ui.ctx(), "tiptoptyp-popup-overlay");
        ui.ctx()
            .input_mut_for(popup, |input| input.events.clone_from(&popup_events));
        app.handle_shortcuts(ui.ctx(), None);
        let remaining = ui.ctx().input_for(popup, |input| input.events.clone());
        ui.ctx().input_mut(|input| input.events = remaining);
        app.process_native_menu_commands(ui.ctx(), None);
        if !app.command_palette_visible() {
            app.show_editor(ui);
            return;
        }
        let entries = app.command_palette_entries(ui.ctx());
        let action = {
            let _edits = prepare_child_widget_edit_events(
                ui.ctx(),
                &app.settings.effective_shortcuts(),
                false,
            );
            // Native menu edits are trusted semantic commands: they are
            // delivered after normalization of physical keyboard events.
            let _queued = crate::child_view::begin_queued_edit_events(
                ui.ctx(),
                &mut app.command_palette_edit_events,
            );
            app.command_palette.show(ui, &entries)
        };
        if action == Some(command_palette::Action::Dismiss) {
            app.dismiss_command_palette(ui.ctx(), true);
            app.show_editor(ui);
        }
    });
    let commands = output.platform_output.commands.clone();
    output.drop_without_applying_deltas();
    commands
}

#[test]
fn palette_shortcut_and_rebound_shortcut_toggle_without_mutating_the_document() {
    for custom in [false, true] {
        let directory = tempfile::tempdir().unwrap();
        let context = egui::Context::default();
        let mut app = fixture(&context, directory.path());
        let original = app.document().key();
        let source = app.document().source().clone();
        let tabs = app.tabs.len();
        let default = app
            .settings
            .effective_shortcuts()
            .egui(ShortcutAction::CommandPalette)
            .unwrap();
        if custom {
            app.settings.shortcut_overrides.set(
                ShortcutAction::CommandPalette,
                Some(crate::shortcuts::ShortcutChord::parse("Primary+Shift+K").unwrap()),
            );
            route(&context, &mut app, input(None, vec![key(default)]));
            assert!(
                !app.command_palette_visible(),
                "the displaced default is inactive"
            );
        }
        let chord = app
            .settings
            .effective_shortcuts()
            .egui(ShortcutAction::CommandPalette)
            .unwrap();
        route(&context, &mut app, input(None, vec![key(chord)]));
        assert!(app.command_palette_visible());
        assert!(app.command_palette.query.is_empty());
        assert_eq!(selection(&context), 2..7);
        query_frame(&context, &mut app, Vec::new());
        query_frame(&context, &mut app, vec![key(chord)]);
        assert!(!app.command_palette_visible());
        assert_eq!(app.document().key(), original);
        assert_eq!(app.document().source(), &source);
        assert_eq!(app.tabs.len(), tabs);
        assert_eq!(selection(&context), 2..7);
        assert!(context.memory(|memory| memory.has_focus(source_editor_id(&context))));
    }
}

#[test]
fn focused_settings_never_opens_a_palette_for_the_underlying_document() {
    let directory = tempfile::tempdir().unwrap();
    let context = egui::Context::default();
    let mut app = fixture(&context, directory.path());
    let original = app.document().key();
    app.settings_visible = true;
    let chord = app
        .settings
        .effective_shortcuts()
        .egui(ShortcutAction::CommandPalette)
        .unwrap();
    route(
        &context,
        &mut app,
        input(Some("tiptoptyp-settings"), vec![key(chord)]),
    );
    assert!(!app.command_palette_visible());
    app.enqueue_native_menu_command(AppCommand::CommandPalette);
    route(
        &context,
        &mut app,
        input(Some("tiptoptyp-settings"), Vec::new()),
    );
    assert!(!app.command_palette_visible());
    assert!(app.settings_visible);
    assert_eq!(app.document().key(), original);
    assert_eq!(app.document().source(), "alpha beta gamma");
    assert_eq!(selection(&context), 2..7);
}

#[test]
fn native_edits_act_on_the_palette_query_while_save_is_ignored_and_close_only_dismisses() {
    let directory = tempfile::tempdir().unwrap();
    let context = egui::Context::default();
    let mut app = fixture(&context, directory.path());
    let original = app.document().key();
    let tabs = app.tabs.len();
    context
        .run_ui(input(None, Vec::new()), |ui| {
            app.toggle_command_palette(ui.ctx());
        })
        .drop_without_applying_deltas();
    app.command_palette.query = "query text".into();
    query_frame(&context, &mut app, Vec::new());
    app.enqueue_native_menu_command(AppCommand::SelectAll);
    app.enqueue_native_menu_command(AppCommand::Copy);
    let commands = query_frame(&context, &mut app, Vec::new());
    assert!(commands.iter().any(
        |command| matches!(command, egui::OutputCommand::CopyText(text) if text == "query text")
    ));
    assert!(app.command_palette_edit_events.is_empty());
    assert_eq!(selection(&context), 2..7);
    app.enqueue_native_menu_command(AppCommand::Cut);
    query_frame(&context, &mut app, Vec::new());
    assert!(app.command_palette.query.is_empty());
    query_frame(&context, &mut app, vec![egui::Event::Text("Find".into())]);
    assert_eq!(app.command_palette.query, "Find");
    app.enqueue_native_menu_command(AppCommand::Save);
    query_frame(&context, &mut app, Vec::new());
    assert!(app.command_palette_visible());
    assert!(!app.document_workflow.has_dialog());
    assert!(app.document().path().is_none());
    app.enqueue_native_menu_command(AppCommand::CloseTab);
    query_frame(&context, &mut app, Vec::new());
    assert!(!app.command_palette_visible());
    assert!(app.command_palette_edit_events.is_empty());
    assert_eq!(
        app.tabs.len(),
        tabs,
        "Close Tab dismisses the palette alone"
    );
    assert_eq!(app.document().key(), original);
    assert_eq!(app.document().source(), "alpha beta gamma");
    assert_eq!(selection(&context), 2..7);
    assert!(context.memory(|memory| memory.has_focus(source_editor_id(&context))));
}

#[test]
fn cancelling_restores_the_original_source_selection_and_selected_commands_restore_its_target() {
    for execute in [false, true] {
        let directory = tempfile::tempdir().unwrap();
        let context = egui::Context::default();
        let mut app = fixture(&context, directory.path());
        context
            .run_ui(input(None, Vec::new()), |ui| {
                app.toggle_command_palette(ui.ctx());
            })
            .drop_without_applying_deltas();
        app.command_palette.query = "Select all".into();
        query_frame(&context, &mut app, Vec::new());
        assert!(!context.memory(|memory| memory.has_focus(source_editor_id(&context))));
        if execute {
            context
                .run_ui(input(None, Vec::new()), |ui| {
                    let origin = app.command_palette_origin.unwrap();
                    app.close_app_popup();
                    app.pending_app_popup_action = Some(AppPopupAction::Palette {
                        action: ShortcutAction::SelectAll,
                        origin,
                    });
                    app.execute_pending_app_popup_action(ui.ctx(), None);
                    app.show_editor(ui);
                })
                .drop_without_applying_deltas();
            assert_eq!(selection(&context), 0.."alpha beta gamma".chars().count());
        } else {
            query_frame(
                &context,
                &mut app,
                vec![key(KeyboardShortcut::new(
                    Modifiers::NONE,
                    egui::Key::Escape,
                ))],
            );
            assert_eq!(selection(&context), 2..7);
        }
        assert!(!app.command_palette_visible());
        assert!(context.memory(|memory| memory.has_focus(source_editor_id(&context))));
        assert_eq!(app.document().source(), "alpha beta gamma");
    }
}

#[test]
fn terminal_origin_find_opens_terminal_search_and_keeps_source_unchanged() {
    let directory = tempfile::tempdir().unwrap();
    let context = egui::Context::default();
    let mut app = fixture(&context, directory.path());
    app.terminal
        .prepare_fixture(b"terminal output", directory.path());
    app.bottom_panel.select(PanelTab::Terminal);
    let terminal = crate::terminal::terminal_id(&context);
    context.memory_mut(|memory| memory.request_focus(terminal));
    context
        .run_ui(input(None, Vec::new()), |ui| {
            egui::Panel::bottom("palette-test-terminal")
                .exact_size(180.0)
                .show(ui, |ui| app.show_bottom_panel(ui));
            app.show_editor(ui);
        })
        .drop_without_applying_deltas();
    assert!(app.terminal_focused(&context, egui::ViewportId::ROOT));
    // Ordinary TextEdit focus loss collapses its selection to its caret.
    // Establish that normal terminal-focus baseline before opening a palette.
    let source_selection = selection(&context);
    let chord = app
        .settings
        .effective_shortcuts()
        .egui(ShortcutAction::CommandPalette)
        .unwrap();
    route(&context, &mut app, input(None, vec![key(chord)]));
    assert!(
        app.command_palette_visible(),
        "the shell must not consume Cmd+Shift+P"
    );
    assert!(app.command_palette_origin.unwrap().terminal);
    query_frame(&context, &mut app, Vec::new());
    context
        .run_ui(input(None, Vec::new()), |ui| {
            let origin = app.command_palette_origin.unwrap();
            app.close_app_popup();
            app.pending_app_popup_action = Some(AppPopupAction::Palette {
                action: ShortcutAction::Find,
                origin,
            });
            app.execute_pending_app_popup_action(ui.ctx(), None);
        })
        .drop_without_applying_deltas();
    assert!(app.terminal.find_open());
    assert!(!app.find_bar.visible);
    assert_eq!(app.document().source(), "alpha beta gamma");
    assert_eq!(selection(&context), source_selection);
}

#[test]
fn empty_workspace_palette_allows_new_but_disables_source_and_save_actions() {
    let directory = tempfile::tempdir().unwrap();
    let context = egui::Context::default();
    let mut app = EditorApp::dormant_for_tests(&context, directory.path().into());
    app.tabs = tabs::Tabs::default();
    context
        .run_ui(input(None, Vec::new()), |ui| {
            app.toggle_command_palette(ui.ctx());
            let entries = app.command_palette_entries(ui.ctx());
            for (action, enabled) in [
                (ShortcutAction::New, true),
                (ShortcutAction::Open, true),
                (ShortcutAction::Format, false),
                (ShortcutAction::Save, false),
                (ShortcutAction::SelectAll, false),
            ] {
                let entry = entries.iter().find(|entry| entry.action == action).unwrap();
                assert_eq!(entry.enabled, enabled, "{action:?}");
            }
        })
        .drop_without_applying_deltas();
    assert!(app.command_palette_visible());
    assert!(app.tabs.is_empty());
}

#[test]
fn more_specific_commands_are_not_stolen_by_the_palette_chord() {
    let directory = tempfile::tempdir().unwrap();
    let context = egui::Context::default();
    let mut app = fixture(&context, directory.path());
    let bindings = app.settings.effective_shortcuts();
    let chord = bindings.egui(ShortcutAction::Packages).unwrap();
    context
        .run_ui(input(None, vec![key(chord)]), |ui| {
            assert!(!app.handle_palette_shortcuts(ui.ctx(), egui::ViewportId::ROOT, &bindings));
            assert!(!app.command_palette_visible());
            assert_eq!(ui.input(|input| input.events.len()), 1);
        })
        .drop_without_applying_deltas();
}

#[test]
fn preview_origin_cannot_run_source_clipboard_or_find_replacement_commands() {
    let directory = tempfile::tempdir().unwrap();
    let context = egui::Context::default();
    let mut app = fixture(&context, directory.path());
    let mut origin = app.palette_origin(&context);
    origin.preview = Some(preview_controls::Target::Source);
    origin.focus = None;
    for action in [
        ShortcutAction::Cut,
        ShortcutAction::Copy,
        ShortcutAction::Paste,
        ShortcutAction::SelectAll,
        ShortcutAction::FindReplace,
        ShortcutAction::FindNext,
        ShortcutAction::FindPrevious,
    ] {
        assert!(
            !app.palette_action_enabled(action, origin, &context),
            "{action:?}"
        );
        app.execute_palette_action(action, origin, &context, None);
        assert_eq!(app.document().source(), "alpha beta gamma");
        assert_eq!(selection(&context), 2..7);
        assert!(app.pending_editor_selection.is_none());
        assert!(app.pending_widget_paste.is_none());
    }
}

#[test]
fn detached_preview_controls_can_open_a_palette_without_losing_their_target() {
    let directory = tempfile::tempdir().unwrap();
    let context = egui::Context::default();
    let mut app = fixture(&context, directory.path());
    app.preview_controls.popout = Some(ViewMode::Split);
    app.preview_controls.popout_target = preview_controls::Target::Source;
    let window = scoped_child_viewport_id(&context, "preview-window");
    let controls = crate::child_view::child_viewport_id(window, preview_controls::SALT);
    let mut raw = input(None, Vec::new());
    raw.viewports
        .get_mut(&egui::ViewportId::ROOT)
        .unwrap()
        .focused = Some(false);
    raw.viewports.insert(
        window,
        egui::ViewportInfo {
            parent: Some(egui::ViewportId::ROOT),
            focused: Some(false),
            ..Default::default()
        },
    );
    raw.viewports.insert(
        controls,
        egui::ViewportInfo {
            parent: Some(window),
            focused: Some(true),
            ..Default::default()
        },
    );
    context
        .run_ui(raw, |ui| {
            assert_eq!(focused_input_viewport(ui.ctx()), controls);
            assert!(app.palette_admitted(ui.ctx()));
            app.toggle_command_palette(ui.ctx());
            assert!(app.command_palette_visible());
            let origin = app.command_palette_origin.unwrap();
            assert_eq!(origin.viewport, controls);
            assert_eq!(origin.preview, Some(preview_controls::Target::Source));
            assert!(!origin.terminal);
            assert!(app.palette_action_enabled(ShortcutAction::Find, origin, ui.ctx()));
        })
        .drop_without_applying_deltas();
    assert_eq!(app.document().source(), "alpha beta gamma");
    assert_eq!(selection(&context), 2..7);
}

#[test]
fn palette_comfy_preview_is_a_global_view_command_from_text_fields() {
    for find_child in [false, true] {
        let directory = tempfile::tempdir().unwrap();
        let context = egui::Context::default();
        let mut app = fixture(&context, directory.path());
        app.settings.comfy_background = false;
        app.settings.comfy_text = false;
        let source = app.document().source().clone();
        let mut query = String::from("preserved query");
        context
            .run_ui(input(None, Vec::new()), |ui| {
                let field = ui.add(egui::TextEdit::singleline(&mut query));
                field.request_focus();
                let origin = Origin {
                    viewport: if find_child {
                        scoped_child_viewport_id(ui.ctx(), find_bar::VIEWPORT_SALT)
                    } else {
                        ui.ctx().viewport_id()
                    },
                    focus: Some(field.id),
                    terminal: false,
                    preview: None,
                    tex_mode: false,
                };
                assert!(app.palette_action_enabled(ShortcutAction::ComfyPreview, origin, ui.ctx()));
                app.execute_palette_action(ShortcutAction::ComfyPreview, origin, ui.ctx(), None);
            })
            .drop_without_applying_deltas();
        let settings = app.pending_settings.as_ref().unwrap_or(&app.settings);
        assert!(
            settings.comfy_background && settings.comfy_text,
            "enabled Comfy command was swallowed by a text field (find_child={find_child})"
        );
        assert_eq!(query, "preserved query");
        assert_eq!(app.document().source(), &source);
    }
}

#[test]
fn palette_catalog_exposes_preview_page_navigation_and_fit_width() {
    let directory = tempfile::tempdir().unwrap();
    let context = egui::Context::default();
    let app = fixture(&context, directory.path());
    let entries = app.command_palette_entries(&context);
    for action in [
        ShortcutAction::PreviewPreviousPage,
        ShortcutAction::PreviewNextPage,
        ShortcutAction::PreviewFitWidth,
    ] {
        assert_eq!(
            entries
                .iter()
                .filter(|entry| entry.action == action)
                .count(),
            1,
            "preview command missing or duplicated: {action:?}"
        );
    }
}

#[test]
fn palette_page_actions_use_current_page_bounds_and_an_idempotent_fit() {
    for (page, count, previous, next) in [
        (5, 10, 4, 6),
        (0, 10, 0, 1),
        (9, 10, 8, 9),
        (0, 0, 0, 0),
        (0, 1, 0, 0),
        (usize::MAX, usize::MAX, usize::MAX - 1, usize::MAX - 1),
    ] {
        let snapshot = preview_controls::Snapshot {
            page,
            count,
            ..Default::default()
        };
        for (command, expected) in [
            (
                ShortcutAction::PreviewPreviousPage,
                serde_json::json!({"action": "page", "value": previous}),
            ),
            (
                ShortcutAction::PreviewNextPage,
                serde_json::json!({"action": "page", "value": next}),
            ),
            (
                ShortcutAction::PreviewFitWidth,
                serde_json::json!({"action": "fit"}),
            ),
        ] {
            assert_eq!(
                serde_json::to_value(preview_page_action(command, &snapshot).unwrap()).unwrap(),
                expected,
                "{command:?}: page={page}, count={count}"
            );
        }
    }
    let directory = tempfile::tempdir().unwrap();
    let context = egui::Context::default();
    let app = fixture(&context, directory.path());
    let entries = app.command_palette_entries(&context);
    assert_eq!(
        entries
            .iter()
            .find(|entry| entry.action == ShortcutAction::PreviewFitWidth)
            .unwrap()
            .label,
        "Fit preview to width"
    );
    assert!(preview_page_action(ShortcutAction::New, &Default::default()).is_none());
}

#[test]
fn palette_find_origin_preserves_its_host_beyond_the_normal_blur_grace() {
    let directory = tempfile::tempdir().unwrap();
    let context = egui::Context::default();
    let mut app = fixture(&context, directory.path());
    let source = app.document().source().clone();
    app.find_bar.visible = true;
    app.find_bar.replace_visible = true;
    app.find_bar.query = "alpha".into();
    app.find_bar.replacement = "preserved replacement".into();
    app.find_bar.child_focused = true;
    context
        .run_ui(input(Some(find_bar::VIEWPORT_SALT), Vec::new()), |ui| {
            app.toggle_command_palette(ui.ctx());
            assert_eq!(
                app.command_palette_origin.unwrap().viewport,
                scoped_child_viewport_id(ui.ctx(), find_bar::VIEWPORT_SALT)
            );
        })
        .drop_without_applying_deltas();
    assert!(app.command_palette_visible());
    // Without the captured-origin presentation policy, Find's normal grace
    // would hide its dormant native host before cancellation could focus it.
    app.find_bar.focus = false;
    app.find_bar.child_focused = false;
    let now = Instant::now();
    assert!(app.find_bar.present(false, now));
    let after_grace = now + Duration::from_millis(200);
    assert!(!app.find_bar.present(false, after_grace));
    context
        .run_ui(input(Some("tiptoptyp-popup-overlay"), Vec::new()), |ui| {
            // Use the same ownership policy as show_find_overlay. Keep the
            // native host available without requesting its query field focus:
            // a Replace field must retain its original keyboard/IME target.
            let held = app.palette_holds_find(ui.ctx());
            assert!(held);
            assert!(app.find_bar.present(held, after_grace));
            assert!(!app.find_bar.focus);
        })
        .drop_without_applying_deltas();
    let mut inactive = input(None, Vec::new());
    inactive
        .viewports
        .get_mut(&egui::ViewportId::ROOT)
        .unwrap()
        .focused = Some(false);
    context
        .run_ui(inactive, |ui| {
            assert!(
                !app.palette_holds_find(ui.ctx()),
                "an inactive palette must not keep Find visible over another app"
            );
        })
        .drop_without_applying_deltas();
    context
        .run_ui(input(Some("tiptoptyp-popup-overlay"), Vec::new()), |ui| {
            app.dismiss_command_palette(ui.ctx(), true);
            assert!(!app.palette_holds_find(ui.ctx()));
        })
        .drop_without_applying_deltas();
    assert!(!app.command_palette_visible());
    assert!(app.find_bar.present(false, after_grace));
    assert!(
        !app.find_bar.focus,
        "cancellation must not force the Find query over Replace"
    );
    assert!(app.find_bar.visible && app.find_bar.replace_visible);
    assert_eq!(app.find_bar.query, "alpha");
    assert_eq!(app.find_bar.replacement, "preserved replacement");
    assert_eq!(app.document().source(), &source);
    context
        .run_ui(input(None, Vec::new()), |ui| {
            app.toggle_command_palette(ui.ctx());
            assert!(
                !app.palette_holds_find(ui.ctx()),
                "a source-origin palette must not retain an unrelated Find host"
            );
            assert!(!app.find_bar.present(
                app.palette_holds_find(ui.ctx()),
                after_grace + Duration::from_millis(200)
            ));
        })
        .drop_without_applying_deltas();
}

#[test]
fn palette_ime_commit_enter_keeps_query_ready_for_more_typing() {
    let directory = tempfile::tempdir().unwrap();
    let context = egui::Context::default();
    let mut app = fixture(&context, directory.path());
    let source = app.document().source().clone();
    context
        .run_ui(input(None, Vec::new()), |ui| {
            app.toggle_command_palette(ui.ctx())
        })
        .drop_without_applying_deltas();
    query_frame(&context, &mut app, Vec::new());
    query_frame(
        &context,
        &mut app,
        vec![egui::Event::Ime(egui::ImeEvent::Preedit {
            text: "保存".into(),
            active_range_chars: None,
        })],
    );
    query_frame(
        &context,
        &mut app,
        vec![
            egui::Event::Ime(egui::ImeEvent::Commit("Save".into())),
            key(KeyboardShortcut::new(Modifiers::NONE, egui::Key::Enter)),
        ],
    );
    assert!(
        app.command_palette_visible(),
        "IME confirmation must not execute a command"
    );
    assert_eq!(app.command_palette.query, "Save");
    query_frame(&context, &mut app, vec![egui::Event::Text(" as".into())]);
    assert_eq!(
        app.command_palette.query, "Save as",
        "IME confirmation surrendered the palette query's keyboard focus"
    );
    assert_eq!(app.document().source(), &source);
}

#[test]
fn latex_diff_palette_search_and_enter_work_without_a_tex_document() {
    for empty in [false, true] {
        let directory = tempfile::tempdir().unwrap();
        let context = egui::Context::default();
        let mut app = EditorApp::dormant_for_tests(&context, directory.path().into());
        if empty {
            app.tabs = tabs::Tabs::default();
        } else {
            app.document_mut().replace_loaded_unprojected(
                "= Unchanged Typst source".into(),
                directory.path().join("paper.typ"),
                DocumentKind::Typst,
                None,
            );
        }
        context
            .run_ui(input(None, Vec::new()), |ui| {
                app.toggle_command_palette(ui.ctx())
            })
            .drop_without_applying_deltas();
        for query in ["TeX", "Create LaTeX Diff", "latexdiff"] {
            app.command_palette.query = query.into();
            context
                .run_ui(input(None, Vec::new()), |ui| {
                    let entries = app.command_palette_entries(ui.ctx());
                    let entry = entries
                        .iter()
                        .find(|entry| entry.action == ShortcutAction::LatexDiff)
                        .unwrap();
                    assert_eq!(entry.label, "Create LaTeX Diff…");
                    assert_eq!(entry.category, "File");
                    assert!(entry.enabled, "standalone diff disabled (empty={empty})");
                    let _ = app.command_palette.show(ui, &entries);
                    assert!(
                        app.command_palette
                            .enabled_actions()
                            .any(|action| action == ShortcutAction::LatexDiff),
                        "query={query}"
                    );
                })
                .drop_without_applying_deltas();
        }
        let enter = key(KeyboardShortcut::new(Modifiers::NONE, egui::Key::Enter));
        context
            .run_ui(input(None, vec![enter]), |ui| {
                let entries = app.command_palette_entries(ui.ctx());
                assert_eq!(
                    app.command_palette.show(ui, &entries),
                    Some(command_palette::Action::Execute(ShortcutAction::LatexDiff))
                );
                let origin = app.command_palette_origin.unwrap();
                app.close_app_popup();
                app.execute_palette_action(ShortcutAction::LatexDiff, origin, ui.ctx(), None);
            })
            .drop_without_applying_deltas();
        assert!(
            app.latexdiff.is_some(),
            "palette did not open the wizard (empty={empty})"
        );
        if empty {
            assert!(app.tabs.is_empty());
        } else {
            assert_eq!(app.document().source(), "= Unchanged Typst source");
        }
    }
}

#[test]
fn documentation_and_citation_import_are_searchable_without_open_tabs() {
    for (action, query) in [
        (ShortcutAction::TexDocumentation, "texdoc"),
        (ShortcutAction::ImportCitation, "import citation"),
    ] {
        let directory = tempfile::tempdir().unwrap();
        let context = egui::Context::default();
        let mut app = EditorApp::dormant_for_tests(&context, directory.path().into());
        app.tabs = tabs::Tabs::default();
        context
            .run_ui(input(None, Vec::new()), |ui| {
                app.toggle_command_palette(ui.ctx())
            })
            .drop_without_applying_deltas();
        app.command_palette.query = query.into();
        context
            .run_ui(input(None, Vec::new()), |ui| {
                let entries = app.command_palette_entries(ui.ctx());
                let entry = entries
                    .iter()
                    .find(|entry| entry.action == action)
                    .expect("command in palette");
                assert!(entry.enabled);
                let _ = app.command_palette.show(ui, &entries);
                assert!(
                    app.command_palette
                        .enabled_actions()
                        .any(|candidate| candidate == action)
                );
            })
            .drop_without_applying_deltas();
        context
            .run_ui(
                input(
                    None,
                    vec![key(KeyboardShortcut::new(
                        Modifiers::NONE,
                        egui::Key::Enter,
                    ))],
                ),
                |ui| {
                    let entries = app.command_palette_entries(ui.ctx());
                    assert_eq!(
                        app.command_palette.show(ui, &entries),
                        Some(command_palette::Action::Execute(action))
                    );
                    let origin = app.command_palette_origin.unwrap();
                    app.close_app_popup();
                    app.execute_palette_action(action, origin, ui.ctx(), None);
                },
            )
            .drop_without_applying_deltas();
        assert!(app.tool_dialog_open());
        assert_eq!(
            app.tex_documentation.is_some(),
            action == ShortcutAction::TexDocumentation
        );
        assert_eq!(
            app.citation.is_some(),
            action == ShortcutAction::ImportCitation
        );
        assert!(app.tabs.is_empty());
    }
}

#[test]
fn palette_tool_handoff_restores_source_without_reactivating_owner() {
    for action in [
        ShortcutAction::LatexDiff,
        ShortcutAction::TexDocumentation,
        ShortcutAction::ImportCitation,
    ] {
        let directory = tempfile::tempdir().unwrap();
        let context = egui::Context::default();
        let mut app = fixture(&context, directory.path());
        let source = app.document().source().clone();
        context
            .run_ui(input(None, Vec::new()), |ui| {
                app.toggle_command_palette(ui.ctx())
            })
            .drop_without_applying_deltas();
        let origin = app.command_palette_origin.unwrap();
        query_frame(&context, &mut app, Vec::new());
        assert_ne!(context.memory(|memory| memory.focused()), origin.focus);
        let output = context.run_ui(input(Some("tiptoptyp-popup-overlay"), Vec::new()), |ui| {
            app.close_app_popup();
            app.execute_palette_action(action, origin, ui.ctx(), None);
            assert_eq!(ui.ctx().memory(|memory| memory.focused()), origin.focus);
        });
        assert!(app.tool_dialog_open(), "{action:?}");
        assert!(
            !output
                .viewport_output
                .values()
                .any(|viewport| viewport.commands.contains(&egui::ViewportCommand::Focus)),
            "{action:?} must let the new modal activate without a competing native owner Focus"
        );
        output.drop_without_applying_deltas();
        assert_eq!(selection(&context), 2..7, "{action:?}");
        assert_eq!(app.document().source(), &source);
    }
}

#[test]
fn palette_cancel_and_ordinary_commands_still_reactivate_origin() {
    for cancel in [false, true] {
        let directory = tempfile::tempdir().unwrap();
        let context = egui::Context::default();
        let mut app = fixture(&context, directory.path());
        context
            .run_ui(input(None, Vec::new()), |ui| {
                app.toggle_command_palette(ui.ctx())
            })
            .drop_without_applying_deltas();
        let origin = app.command_palette_origin.unwrap();
        query_frame(&context, &mut app, Vec::new());
        let output = context.run_ui(input(Some("tiptoptyp-popup-overlay"), Vec::new()), |ui| {
            if cancel {
                app.dismiss_command_palette(ui.ctx(), true);
            } else {
                app.close_app_popup();
                app.execute_palette_action(
                    ShortcutAction::ToggleLineNumbers,
                    origin,
                    ui.ctx(),
                    None,
                );
            }
            assert_eq!(ui.ctx().memory(|memory| memory.focused()), origin.focus);
        });
        let native_focus: Vec<_> = output
            .viewport_output
            .iter()
            .filter_map(|(id, viewport)| {
                viewport
                    .commands
                    .contains(&egui::ViewportCommand::Focus)
                    .then_some(*id)
            })
            .collect();
        assert_eq!(
            native_focus,
            vec![egui::ViewportId::ROOT],
            "cancel={cancel}"
        );
        output.drop_without_applying_deltas();
        assert_eq!(selection(&context), 2..7);
    }
}
