use super::*;

fn focused_child_input(context: &egui::Context) -> egui::RawInput {
    let child = scoped_child_viewport_id(context, snippet_window::SALT);
    let mut input = egui::RawInput::default();
    input
        .viewports
        .entry(egui::ViewportId::ROOT)
        .or_default()
        .focused = Some(false);
    input.viewports.insert(
        child,
        egui::ViewportInfo {
            parent: Some(egui::ViewportId::ROOT),
            focused: Some(true),
            ..Default::default()
        },
    );
    input
}

#[test]
fn snippet_window_owns_document_shortcuts_and_native_commands() {
    let directory = tempfile::tempdir().unwrap();
    let context = egui::Context::default();
    context.set_embed_viewports(false);
    let mut app = EditorApp::dormant_for_tests(&context, directory.path().into());
    app.document_mut()
        .replace_unprojected_untitled("Original document");
    let original = app.document().key();
    let tabs = app.tabs.len();
    app.settings_visible = true;
    app.snippets_visible = true;
    let mut input = focused_child_input(&context);
    input.events = vec![egui::Event::Key {
        key: egui::Key::N,
        physical_key: None,
        pressed: true,
        repeat: false,
        modifiers: egui::Modifiers::COMMAND,
    }];
    context
        .run_ui(input, |ui| {
            assert_eq!(
                focused_input_viewport(ui.ctx()),
                scoped_child_viewport_id(ui.ctx(), snippet_window::SALT)
            );
            app.handle_shortcuts(ui.ctx(), None);
            for command in [
                AppCommand::New,
                AppCommand::Explorer,
                AppCommand::Format,
                AppCommand::SelectAll,
                AppCommand::Find,
                AppCommand::Save,
                AppCommand::CloseTab,
            ] {
                app.queued_native_menu_commands.push(command);
            }
            app.process_native_menu_commands(ui.ctx(), None);
            app.show_snippet_editor_window(ui.ctx());
        })
        .drop_without_applying_deltas();
    assert_eq!(app.document().key(), original);
    assert_eq!(app.document().source(), "Original document");
    assert_eq!(app.tabs.len(), tabs);
    assert!(!app.snippets_visible);
    assert!(
        app.settings_visible,
        "closing snippets must not close Settings"
    );
    assert!(!app.close_accepted());
}

#[test]
fn focused_snippet_window_can_save_without_document_tabs() {
    let directory = tempfile::tempdir().unwrap();
    let context = egui::Context::default();
    let mut app = EditorApp::dormant_for_tests(&context, directory.path().into());
    app.lifecycle = DocumentLifecycle::Active;
    app.finish_close_tab(&context);
    assert!(app.tabs.is_empty());
    assert!(!app.native_command_enabled(AppCommand::Save));
    app.snippets_visible = true;
    app.snippet_window.lock().unwrap().focused = true;
    for command in [
        AppCommand::Save,
        AppCommand::CloseTab,
        AppCommand::Find,
        AppCommand::Copy,
    ] {
        assert!(app.native_command_enabled(command), "{command:?}");
    }
    for command in [AppCommand::New, AppCommand::Format, AppCommand::Explorer] {
        assert!(!app.native_command_enabled(command), "{command:?}");
    }
}

#[test]
fn queued_native_save_then_close_publishes_draft_before_hiding_editor() {
    let directory = tempfile::tempdir().unwrap();
    let context = egui::Context::default();
    context.set_embed_viewports(false);
    let mut app = EditorApp::dormant_for_tests(&context, directory.path().into());
    app.document_mut()
        .replace_unprojected_untitled("Original document");
    let original = app.document().key();
    let tabs = app.tabs.len();
    app.settings_visible = true;
    app.snippets_visible = true;
    let mut edited = app.settings.clone();
    edited.snippet_presets[0].prefix = "savedbyqueuedmenu".into();
    app.snippet_window
        .lock()
        .unwrap()
        .stage_unsaved_for_tests(&context, &app.settings, &edited);
    app.enqueue_native_menu_command(AppCommand::Save);
    app.enqueue_native_menu_command(AppCommand::CloseTab);
    context
        .run_ui(focused_child_input(&context), |ui| {
            app.process_native_menu_commands(ui.ctx(), None);
            app.show_snippet_editor_window(ui.ctx());
        })
        .drop_without_applying_deltas();
    assert_eq!(
        app.settings_snapshot().snippet_presets[0].prefix,
        "savedbyqueuedmenu"
    );
    assert!(!app.snippets_visible);
    assert!(app.settings_visible);
    assert_eq!(app.document().key(), original);
    assert_eq!(app.document().source(), "Original document");
    assert_eq!(app.tabs.len(), tabs);
    assert!(!app.snippet_window.lock().unwrap().has_actions());
}
