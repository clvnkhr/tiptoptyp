use super::*;
use std::{
    cell::{Cell, RefCell},
    rc::Rc,
};

#[test]
fn workspace_child_fields_own_native_edits_and_escape_cancels_only_the_form() {
    for parent_field in [false, true] {
        let context = egui::Context::default();
        context.set_embed_viewports(false);
        let events = Rc::new(RefCell::new(Vec::new()));
        let incoming = Rc::clone(&events);
        let time = Cell::new(0.0);
        egui::Context::set_immediate_viewport_renderer(move |context, mut child| {
            let screen = Rect::from_min_size(
                Pos2::ZERO,
                child.builder.inner_size.unwrap_or(Vec2::new(1000.0, 600.0)),
            );
            time.set(time.get() + 1.0);
            let mut input = egui::RawInput {
                viewport_id: child.ids.this,
                time: Some(time.get()),
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
            context
                .run_ui(input, |ui| (child.viewport_ui_cb)(ui))
                .drop_without_applying_deltas();
        });
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().canonicalize().unwrap();
        let mut app = EditorApp::dormant_for_tests(&context, root.clone());
        let source = "unsaved source with its own selection";
        app.document_mut().replace_unprojected_untitled(source);
        let key = app.document().key();
        let cursor = CCursorRange::two(CCursor::new(2), CCursor::new(8));
        app.store_editor_cursor(&context, cursor);
        app.workspace_chooser_visible = true;
        app.begin_workspace_creation();
        app.workspace_creation.draft.as_mut().unwrap().parent = root.to_string_lossy().into_owned();
        let child = scoped_child_viewport_id(&context, SALT);
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
                    // A stale source focus ID must not capture a child's edits.
                    let source_id = source_editor_id(ui.ctx());
                    ui.memory_mut(|memory| memory.request_focus(source_id));
                    app.handle_shortcuts(ui.ctx(), None);
                    app.process_native_menu_commands(ui.ctx(), None);
                    app.show_workspace_chooser(ui.ctx());
                })
                .drop_without_applying_deltas();
        };
        let key_event = |key| egui::Event::Key {
            key,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        };
        frame(&mut app);
        frame(&mut app);
        if parent_field {
            events.borrow_mut().push(key_event(egui::Key::Tab));
            frame(&mut app);
        }
        let replacement = if parent_field {
            root.join("new parent").to_string_lossy().into_owned()
        } else {
            "new workspace".into()
        };
        app.enqueue_native_menu_command(AppCommand::SelectAll);
        frame(&mut app);
        events
            .borrow_mut()
            .push(egui::Event::Text(replacement.clone()));
        frame(&mut app);
        let field = |app: &EditorApp| {
            let draft = app.workspace_creation.draft.as_ref().unwrap();
            if parent_field {
                draft.parent.clone()
            } else {
                draft.name.clone()
            }
        };
        assert_eq!(field(&app), replacement, "parent={parent_field}");
        app.enqueue_native_menu_command(AppCommand::SelectAll);
        frame(&mut app);
        app.enqueue_native_menu_command(AppCommand::Cut);
        frame(&mut app);
        assert_eq!(field(&app), "", "parent={parent_field}: cut");
        app.enqueue_native_menu_command(AppCommand::Undo);
        frame(&mut app);
        assert_eq!(field(&app), replacement, "parent={parent_field}: undo");
        app.enqueue_native_menu_command(AppCommand::SelectAll);
        frame(&mut app);
        app.enqueue_native_menu_command(AppCommand::Paste);
        frame(&mut app);
        // Supply the semantic event AppKit delivers, without an OS clipboard.
        events
            .borrow_mut()
            .push(egui::Event::Paste("pasted workspace".into()));
        frame(&mut app);
        assert_eq!(
            field(&app),
            "pasted workspace",
            "parent={parent_field}: paste"
        );
        if parent_field {
            let shortcut = app
                .settings
                .effective_shortcuts()
                .egui(ShortcutAction::CloseTab)
                .unwrap();
            let mut modifiers = shortcut.modifiers;
            if cfg!(target_os = "macos") {
                modifiers.mac_cmd = true;
            }
            events.borrow_mut().push(egui::Event::Key {
                key: shortcut.logical_key,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers,
            });
        } else {
            events.borrow_mut().push(key_event(egui::Key::Escape));
        }
        frame(&mut app);
        assert!(app.workspace_creation.draft.is_none());
        assert!(app.workspace_chooser_visible);
        assert!(!app.workspace_creation.busy());
        assert!(app.workspace_creation.edit_events.is_empty());
        assert_eq!(app.workspace_root, root);
        assert_eq!(app.document().key(), key);
        assert_eq!(app.document().source(), source);
        assert_eq!(app.editor_snapshot(&context).cursor, cursor);
        assert_eq!(fs::read_dir(&root).unwrap().count(), 0);
    }
}
