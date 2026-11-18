//! Collaborator navigation keeps source ownership, local input and revision guards.
use super::{
    tests::{EPOCH, entry, fixture},
    *,
};
use crate::app::overleaf_presence::BadgeAction;

fn remote_cursor(
    app: &mut EditorApp,
    context: &egui::Context,
    id: &str,
    source: &str,
    head: usize,
) {
    let event = serde_json::from_value(serde_json::json!({
        "event": "presence", "epoch": EPOCH, "own_id": "own", "self_user_id": "account",
        "peers": [{"id": "remote", "user_id": "other", "name": "Collaborator", "doc_id": id, "row": 0, "column": head}],
        "documents": [{"id": id, "source_fingerprint": SourceIndex::new(source).fingerprint,
            "source_utf16": source.encode_utf16().count(), "cursors": [{"id": "remote", "anchor": head, "head": head}]}]
    })).unwrap();
    app.update_overleaf_event(event, context).unwrap();
}

#[test]
fn collaborator_seek_reveals_once_and_takes_source_focus() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().canonicalize().unwrap();
    let context = egui::Context::default();
    let mut app = fixture(&root, &context);
    remote_cursor(&mut app, &context, "a", "Main old\n", 4);
    app.overleaf_presence_action(BadgeAction::Seek("remote".into()), &context);
    app.advance_overleaf_cursor(&context);
    assert_eq!(
        app.pending_editor_selection,
        Some(EditorSelection::Focus(4..4))
    );
    assert_eq!(app.overleaf.cursor_navigation.following_peer(), None);
    app.pending_editor_selection = None;
    app.advance_overleaf_cursor(&context);
    assert!(
        app.pending_editor_selection.is_none(),
        "a seek must not follow later updates"
    );
    assert_eq!(app.document().source(), "Main old\n");
    assert!(!app.document().is_dirty());
}

#[test]
fn collaborator_follow_preserves_local_selection_and_keyboard_owner() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().canonicalize().unwrap();
    let context = egui::Context::default();
    let mut app = fixture(&root, &context);
    let local = CCursorRange::two(CCursor::new(2), CCursor::new(6));
    app.store_editor_cursor(&context, local);
    let focus = egui::Id::new("find-field-still-owns-keyboard");
    context.memory_mut(|memory| memory.request_focus(focus));
    remote_cursor(&mut app, &context, "a", "Main old\n", 1);
    app.overleaf_presence_action(BadgeAction::Follow("remote".into()), &context);
    app.advance_overleaf_cursor(&context);
    assert_eq!(
        app.overleaf.cursor_navigation.following_peer(),
        Some("remote")
    );
    assert_eq!(app.overleaf.cursor_navigation.take_reveal(), Some(1));
    assert!(app.pending_editor_selection.is_none());
    assert_eq!(app.editor_snapshot(&context).cursor, local);
    assert_eq!(context.memory(|memory| memory.focused()), Some(focus));

    remote_cursor(&mut app, &context, "a", "Main old\n", 7);
    app.advance_overleaf_cursor(&context);
    assert_eq!(app.overleaf.cursor_navigation.take_reveal(), Some(7));
    assert!(app.pending_editor_selection.is_none());
    assert_eq!(app.editor_snapshot(&context).cursor, local);
    assert_eq!(context.memory(|memory| memory.focused()), Some(focus));
    assert!(!app.document().is_dirty());
    assert!(
        app.overleaf.last_cursor.is_none(),
        "viewing a collaborator must not publish their position as ours"
    );
}

#[test]
fn unchanged_collaborator_positions_do_not_repeat_viewport_requests() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().canonicalize().unwrap();
    let context = egui::Context::default();
    let mut app = fixture(&root, &context);
    remote_cursor(&mut app, &context, "a", "Main old\n", 4);
    app.overleaf_presence_action(BadgeAction::Follow("remote".into()), &context);
    app.advance_overleaf_cursor(&context);
    assert_eq!(app.overleaf.cursor_navigation.take_reveal(), Some(4));
    for _ in 0..8 {
        remote_cursor(&mut app, &context, "a", "Main old\n", 4);
        app.advance_overleaf_cursor(&context);
        assert!(app.overleaf.cursor_navigation.reveal().is_none());
    }
    assert!(app.overleaf.queued.is_empty());
    assert!(!app.overleaf.io.is_running());
}

#[test]
fn following_waits_for_matching_remote_source_then_resumes() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().canonicalize().unwrap();
    let context = egui::Context::default();
    let mut app = fixture(&root, &context);
    let source = "Remote source ahead of the native mirror\n";
    remote_cursor(&mut app, &context, "a", source, 12);
    app.overleaf_presence_action(BadgeAction::Follow("remote".into()), &context);
    app.advance_overleaf_cursor(&context);
    assert!(app.overleaf.cursor_navigation.reveal().is_none());
    assert!(app.pending_editor_selection.is_none());
    assert_eq!(
        app.overleaf.cursor_navigation.following_peer(),
        Some("remote")
    );
    assert_eq!(app.document().source(), "Main old\n");
    app.document_mut().replace_loaded_unprojected(
        source.into(),
        root.join("main.tex"),
        DocumentKind::Tex,
        None,
    );
    app.advance_overleaf_cursor(&context);
    assert_eq!(app.overleaf.cursor_navigation.take_reveal(), Some(12));
    assert_eq!(
        app.overleaf.cursor_navigation.following_peer(),
        Some("remote")
    );
}

#[test]
fn following_stops_on_peer_departure_or_disconnect() {
    for disconnected in [false, true] {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().canonicalize().unwrap();
        let context = egui::Context::default();
        let mut app = fixture(&root, &context);
        remote_cursor(&mut app, &context, "a", "Main old\n", 4);
        app.overleaf_presence_action(BadgeAction::Follow("remote".into()), &context);
        app.advance_overleaf_cursor(&context);
        if disconnected {
            app.overleaf.connection.as_mut().unwrap().transport = Transport::Offline;
        } else {
            app.overleaf
                .connection
                .as_mut()
                .unwrap()
                .model
                .clear_presence();
        }
        app.advance_overleaf_cursor(&context);
        assert_eq!(app.overleaf.cursor_navigation.following_peer(), None);
        assert!(app.overleaf.cursor_navigation.reveal().is_none());
        assert_eq!(app.document().source(), "Main old\n");
        assert!(!app.document().is_dirty());
    }
}

#[test]
fn following_stops_before_local_edit_can_be_replaced_or_revealed() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().canonicalize().unwrap();
    let context = egui::Context::default();
    let mut app = fixture(&root, &context);
    remote_cursor(&mut app, &context, "a", "Main old\n", 4);
    app.overleaf_presence_action(BadgeAction::Follow("remote".into()), &context);
    app.advance_overleaf_cursor(&context);
    app.document_mut().edit(CCursorRange::default(), |source| {
        source.insert_str(0, "my edit ")
    });
    app.advance_overleaf_cursor(&context);
    assert_eq!(app.overleaf.cursor_navigation.following_peer(), None);
    assert!(app.overleaf.cursor_navigation.reveal().is_none());
    assert!(app.pending_editor_selection.is_none());
    assert_eq!(app.document().source(), "my edit Main old\n");
    assert!(app.document().is_dirty());
}

#[test]
fn unknown_collaborator_actions_leave_document_and_selection_untouched() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().canonicalize().unwrap();
    let context = egui::Context::default();
    let mut app = fixture(&root, &context);
    let local = CCursorRange::two(CCursor::new(2), CCursor::new(6));
    app.store_editor_cursor(&context, local);
    for action in [
        BadgeAction::Seek("absent".into()),
        BadgeAction::Follow("absent".into()),
    ] {
        app.overleaf_presence_action(action, &context);
        app.advance_overleaf_cursor(&context);
        assert_eq!(app.overleaf.cursor_navigation.following_peer(), None);
        assert!(app.overleaf.cursor_navigation.reveal().is_none());
        assert!(app.pending_editor_selection.is_none());
        assert_eq!(app.editor_snapshot(&context).cursor, local);
        assert_eq!(app.document().source(), "Main old\n");
        assert!(app.overleaf.error.is_none());
    }
}

#[test]
fn collaborator_follow_maps_utf16_carets_into_projected_editor_source() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().canonicalize().unwrap();
    let context = egui::Context::default();
    let mut app = fixture(&root, &context);
    let canonical = "#import \"@preview/mitex:0.2.7\": mi\n文 #mi(`\\alpha+😀`) end\n";
    app.overleaf
        .connection
        .as_mut()
        .unwrap()
        .model
        .apply(BrowserEvent::Project {
            epoch: EPOCH,
            project_id: tests::PROJECT.into(),
            name: "Paper".into(),
            entries: vec![entry("a", "main.typ", EntryKind::Doc)],
            root_doc: Some("a".into()),
        })
        .unwrap();
    app.document_mut().replace_loaded_unprojected(
        canonical.into(),
        root.join("main.typ"),
        DocumentKind::Typst,
        None,
    );
    app.document_mut()
        .enable(tiptoptyp::mitex_projection::Config::default())
        .unwrap();
    let head = canonical[..canonical.find("end").unwrap()]
        .encode_utf16()
        .count();
    remote_cursor(&mut app, &context, "a", canonical, head);
    app.overleaf_presence_action(BadgeAction::Follow("remote".into()), &context);
    app.advance_overleaf_cursor(&context);
    let source = app.document().source();
    let expected = source[..source.find("end").unwrap()].chars().count();
    assert_ne!(expected, head);
    assert_eq!(app.overleaf.cursor_navigation.take_reveal(), Some(expected));
    assert!(app.pending_editor_selection.is_none());
}

#[test]
fn pending_collaborator_document_open_is_deduplicated_and_resumes_after_receipt() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().canonicalize().unwrap();
    let context = egui::Context::default();
    let mut app = fixture(&root, &context);
    remote_cursor(&mut app, &context, "a", "Main old\n", 4);
    app.overleaf_presence_action(BadgeAction::Follow("remote".into()), &context);
    app.overleaf.cursor_navigation.clear_reveal();
    remote_cursor(&mut app, &context, "b", "Chapter old\n", 3);
    // The browser transport is native-only. Seed its admitted Open receipt;
    // subsequent advancement must wait rather than dispatch another request.
    app.arm_overleaf_navigation();
    app.overleaf.open_after = Some("b".into());
    app.overleaf
        .cursor_navigation
        .target
        .as_mut()
        .unwrap()
        .start_open("b".into());
    let owner = app.tabs.active_id();
    let key = app.document().key();
    for _ in 0..8 {
        app.advance_overleaf_cursor(&context);
        assert_eq!(app.overleaf.open_after.as_deref(), Some("b"));
        assert!(
            app.overleaf.error.is_none(),
            "a second Open would fail without a browser"
        );
        assert!(app.overleaf.cursor_navigation.reveal().is_none());
        assert_eq!(
            app.overleaf.cursor_navigation.following_peer(),
            Some("remote")
        );
        assert_eq!(app.tabs.active_id(), owner);
        assert_eq!(app.document().key(), key);
        assert_eq!(app.document().source(), "Main old\n");
    }
    tests::apply_work(
        &mut app,
        MirrorWork {
            checkpoint: false,
            entry: entry("b", "chapter.tex", EntryKind::Doc),
            data: MirrorData::Document("Chapter old\n".into()),
            save: None,
        },
        &root,
        EPOCH,
        &context,
    );
    assert_eq!(
        app.tabs
            .active_id()
            .and_then(|tab| app.overleaf_entry_for_tab(tab))
            .map(|entry| entry.id.as_str()),
        Some("b"),
        "the real mirror receipt must bind the opened server entity"
    );
    app.advance_overleaf_cursor(&context);
    assert_eq!(app.overleaf.cursor_navigation.take_reveal(), Some(3));
    assert_eq!(
        app.overleaf.cursor_navigation.following_peer(),
        Some("remote")
    );
    assert!(
        app.overleaf
            .cursor_navigation
            .target
            .as_ref()
            .unwrap()
            .opening()
            .is_none()
    );
}

#[test]
fn cancelling_a_pending_collaborator_open_preserves_newer_manual_navigation() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().canonicalize().unwrap();
    let context = egui::Context::default();
    let mut app = fixture(&root, &context);
    remote_cursor(&mut app, &context, "a", "Main old\n", 4);
    app.overleaf_presence_action(BadgeAction::Follow("remote".into()), &context);
    remote_cursor(&mut app, &context, "b", "Chapter old\n", 3);
    app.overleaf
        .cursor_navigation
        .target
        .as_mut()
        .unwrap()
        .start_open("b".into());
    app.arm_overleaf_navigation();
    app.overleaf.open_after = Some("a".into());
    app.advance_overleaf_cursor(&context);
    assert_eq!(app.overleaf.cursor_navigation.following_peer(), None);
    assert!(app.overleaf.cursor_navigation.reveal().is_none());
    assert_eq!(app.overleaf.open_after.as_deref(), Some("a"));
    assert!(app.overleaf.navigation.is_some());
    assert_eq!(app.document().source(), "Main old\n");
}

#[test]
fn leaving_the_remote_workspace_clears_follow_and_rejects_its_late_open_receipt() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().canonicalize().unwrap();
    let local = tempfile::tempdir().unwrap();
    let local_root = local.path().canonicalize().unwrap();
    let context = egui::Context::default();
    let mut app = fixture(&root, &context);
    remote_cursor(&mut app, &context, "a", "Main old\n", 4);
    app.overleaf_presence_action(BadgeAction::Follow("remote".into()), &context);
    remote_cursor(&mut app, &context, "b", "Chapter old\n", 3);
    // Admit a browser Open before its mirror receipt arrives.
    app.arm_overleaf_navigation();
    app.overleaf.open_after = Some("b".into());
    app.overleaf
        .cursor_navigation
        .target
        .as_mut()
        .unwrap()
        .start_open("b".into());
    app.overleaf.reopen_relative = Some((
        app.overleaf
            .connection
            .as_ref()
            .unwrap()
            .project
            .url()
            .into(),
        EPOCH,
        PathBuf::from("chapter.tex"),
    ));
    app.overleaf.await_project();

    app.queue_open_path(local_root.clone());
    for _ in 0..8 {
        app.execute_pending_document_action(&context, None);
    }

    assert_eq!(app.workspace_root, local_root);
    assert!(app.tabs.is_empty());
    assert!(app.overleaf.cursor_navigation.target.is_none());
    assert!(app.overleaf.cursor_navigation.reveal().is_none());
    assert!(app.overleaf.open_after.is_none());
    assert!(app.overleaf.navigation.is_none());
    assert!(app.overleaf.reopen_relative.is_none());
    assert!(!app.overleaf.awaiting_project());

    tests::apply_work(
        &mut app,
        MirrorWork {
            checkpoint: false,
            entry: entry("b", "chapter.tex", EntryKind::Doc),
            data: MirrorData::Document("Chapter old\n".into()),
            save: None,
        },
        &root,
        EPOCH,
        &context,
    );
    for _ in 0..3 {
        app.advance_overleaf_cursor(&context);
        assert_eq!(app.workspace_root, local_root);
        assert!(app.tabs.is_empty());
        assert!(app.document().path().is_none());
        assert!(app.document().source().is_empty());
        assert!(app.overleaf.open_after.is_none());
        assert!(app.overleaf.error.is_none());
    }
}

#[test]
fn departure_for_a_new_connection_keeps_its_relative_destination_and_clears_old_navigation() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().canonicalize().unwrap();
    let context = egui::Context::default();
    let mut app = fixture(&root, &context);
    remote_cursor(&mut app, &context, "a", "Main old\n", 4);
    app.overleaf_presence_action(BadgeAction::Follow("remote".into()), &context);
    app.arm_overleaf_navigation();
    app.overleaf.open_after = Some("b".into());
    app.overleaf
        .cursor_navigation
        .target
        .as_mut()
        .unwrap()
        .start_open("b".into());
    let requested = (
        "https://www.overleaf.com/project/abcdef0123456789abcdef01".into(),
        EPOCH,
        PathBuf::from("chapters/requested.tex"),
    );
    app.overleaf.reopen_relative = Some(requested.clone());
    app.overleaf.await_project();

    app.depart_overleaf_workspace(&root, true);

    assert!(app.overleaf.cursor_navigation.target.is_none());
    assert!(app.overleaf.cursor_navigation.reveal().is_none());
    assert!(app.overleaf.open_after.is_none());
    assert!(app.overleaf.navigation.is_none());
    assert!(!app.overleaf.awaiting_project());
    assert_eq!(app.overleaf.reopen_relative, Some(requested));
    assert_eq!(app.workspace_root, root);
    assert_eq!(app.document().source(), "Main old\n");
}

#[test]
fn bystander_presence_changes_do_not_repeat_followed_caret_reveal() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().canonicalize().unwrap();
    let context = egui::Context::default();
    let mut app = fixture(&root, &context);
    remote_cursor(&mut app, &context, "a", "Main old\n", 4);
    app.overleaf_presence_action(BadgeAction::Follow("remote".into()), &context);
    assert_eq!(app.overleaf.cursor_navigation.take_reveal(), Some(4));
    let generation = app
        .overleaf
        .connection
        .as_ref()
        .unwrap()
        .model
        .presence()
        .generation();
    let event = serde_json::from_value(serde_json::json!({
        "event": "presence", "epoch": EPOCH, "own_id": "own", "self_user_id": "account",
        "peers": [
            {"id": "remote", "user_id": "other", "name": "Collaborator", "doc_id": "a", "row": 0, "column": 4},
            {"id": "bystander", "user_id": "third", "name": "Another collaborator", "doc_id": "b", "row": 0, "column": 1}
        ],
        "documents": [{"id": "a", "source_fingerprint": SourceIndex::new("Main old\n").fingerprint,
            "source_utf16": "Main old\n".encode_utf16().count(), "cursors": [{"id": "remote", "anchor": 4, "head": 4}]}]
    })).unwrap();
    app.update_overleaf_event(event, &context).unwrap();
    assert_ne!(
        app.overleaf
            .connection
            .as_ref()
            .unwrap()
            .model
            .presence()
            .generation(),
        generation
    );
    app.advance_overleaf_cursor(&context);
    assert!(app.overleaf.cursor_navigation.reveal().is_none());
    assert_eq!(
        app.overleaf.cursor_navigation.following_peer(),
        Some("remote")
    );
}

#[test]
fn source_keyboard_navigation_stops_following_without_consuming_the_key() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().canonicalize().unwrap();
    let context = egui::Context::default();
    let mut app = fixture(&root, &context);
    remote_cursor(&mut app, &context, "a", "Main old\n", 4);
    app.overleaf_presence_action(BadgeAction::Follow("remote".into()), &context);
    app.overleaf.cursor_navigation.take_reveal();
    let key = egui::Event::Key {
        key: egui::Key::ArrowLeft,
        physical_key: None,
        pressed: true,
        repeat: false,
        modifiers: egui::Modifiers::NONE,
    };
    context
        .run_ui(
            egui::RawInput {
                events: vec![key.clone()],
                ..Default::default()
            },
            |ui| {
                let editor_id = source_editor_id(ui.ctx());
                ui.memory_mut(|memory| memory.request_focus(editor_id));
                app.advance_overleaf_cursor(ui.ctx());
                assert!(
                    ui.input(|input| input.events.contains(&key)),
                    "the editor must still receive the user's key"
                );
            },
        )
        .drop_without_applying_deltas();
    assert_eq!(app.overleaf.cursor_navigation.following_peer(), None);
    assert!(app.overleaf.cursor_navigation.reveal().is_none());
}

#[test]
fn typing_into_another_control_does_not_cancel_collaborator_following() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().canonicalize().unwrap();
    let context = egui::Context::default();
    let mut app = fixture(&root, &context);
    remote_cursor(&mut app, &context, "a", "Main old\n", 4);
    app.overleaf_presence_action(BadgeAction::Follow("remote".into()), &context);
    app.overleaf.cursor_navigation.take_reveal();
    let focus = egui::Id::new("another-control");
    let text = egui::Event::Text("find a word".into());
    context
        .run_ui(
            egui::RawInput {
                events: vec![text.clone()],
                ..Default::default()
            },
            |ui| {
                ui.memory_mut(|memory| memory.request_focus(focus));
                app.advance_overleaf_cursor(ui.ctx());
                assert!(ui.input(|input| input.events.contains(&text)));
                assert_eq!(ui.memory(|memory| memory.focused()), Some(focus));
            },
        )
        .drop_without_applying_deltas();
    assert_eq!(
        app.overleaf.cursor_navigation.following_peer(),
        Some("remote")
    );
    assert!(app.overleaf.cursor_navigation.reveal().is_none());
    assert_eq!(app.document().source(), "Main old\n");
}

#[test]
fn same_frame_tab_switch_cannot_consume_previous_documents_follow_reveal() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().canonicalize().unwrap();
    let context = egui::Context::default();
    let mut app = fixture(&root, &context);
    remote_cursor(&mut app, &context, "a", "Main old\n", 4);
    app.overleaf_presence_action(BadgeAction::Follow("remote".into()), &context);
    assert_eq!(app.overleaf.cursor_navigation.reveal(), Some(4));
    let (main, chapter) = tests::park_main(&mut app, &root, &context);
    assert_ne!(main, chapter);
    // Toolbar tab switching happens after advance, before the editor paints.
    // Do not call advance again: the body must reject the previous tab's request.
    context
        .run_ui(Default::default(), |ui| {
            assert!(app.take_overleaf_reveal(ui).is_none());
        })
        .drop_without_applying_deltas();
    assert_eq!(app.overleaf.cursor_navigation.following_peer(), None);
    assert!(app.overleaf.cursor_navigation.reveal().is_none());
    assert_eq!(app.document().source(), "Chapter old\n");
    assert_eq!(app.document_for_tab(main).unwrap().source(), "Main old\n");
}

#[test]
fn source_click_scroll_and_escape_cancel_following_without_consuming_input() {
    let position = egui::pos2(30.0, 30.0);
    for event in [
        egui::Event::PointerButton {
            pos: position,
            button: egui::PointerButton::Primary,
            pressed: true,
            modifiers: egui::Modifiers::NONE,
        },
        egui::Event::MouseWheel {
            unit: egui::MouseWheelUnit::Point,
            delta: egui::vec2(0.0, -40.0),
            phase: egui::TouchPhase::Move,
            modifiers: egui::Modifiers::NONE,
        },
        egui::Event::Key {
            key: egui::Key::Escape,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        },
    ] {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().canonicalize().unwrap();
        let context = egui::Context::default();
        let mut app = fixture(&root, &context);
        remote_cursor(&mut app, &context, "a", "Main old\n", 4);
        app.overleaf_presence_action(BadgeAction::Follow("remote".into()), &context);
        context
            .run_ui(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(800.0, 600.0),
                    )),
                    events: vec![egui::Event::PointerMoved(position), event.clone()],
                    ..Default::default()
                },
                |ui| {
                    assert!(ui.available_rect_before_wrap().contains(position));
                    app.advance_overleaf_cursor(ui.ctx());
                    assert!(app.take_overleaf_reveal(ui).is_none());
                    assert!(
                        ui.input(|input| input.events.contains(&event)),
                        "normal interaction must receive {event:?}"
                    );
                },
            )
            .drop_without_applying_deltas();
        assert_eq!(
            app.overleaf.cursor_navigation.following_peer(),
            None,
            "{event:?}"
        );
        assert!(app.overleaf.cursor_navigation.reveal().is_none());
        assert_eq!(app.document().source(), "Main old\n");
    }
}

#[test]
fn queued_reveal_cannot_survive_a_source_revision_change_before_paint() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().canonicalize().unwrap();
    let context = egui::Context::default();
    let mut app = fixture(&root, &context);
    remote_cursor(&mut app, &context, "a", "Main old\n", 4);
    app.overleaf_presence_action(BadgeAction::Follow("remote".into()), &context);
    assert_eq!(app.overleaf.cursor_navigation.reveal(), Some(4));
    let source = "A refreshed server document\n";
    app.document_mut().replace_loaded_unprojected(
        source.into(),
        root.join("main.tex"),
        DocumentKind::Tex,
        None,
    );
    context
        .run_ui(Default::default(), |ui| {
            assert!(app.take_overleaf_reveal(ui).is_none());
        })
        .drop_without_applying_deltas();
    assert!(app.overleaf.cursor_navigation.reveal().is_none());
    assert_eq!(
        app.overleaf.cursor_navigation.following_peer(),
        Some("remote")
    );
    remote_cursor(&mut app, &context, "a", source, 9);
    app.advance_overleaf_cursor(&context);
    assert_eq!(app.overleaf.cursor_navigation.take_reveal(), Some(9));
}

#[test]
fn newer_presence_invalidates_an_unpainted_old_reveal_until_mirror_catches_up() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().canonicalize().unwrap();
    let context = egui::Context::default();
    let mut app = fixture(&root, &context);
    remote_cursor(&mut app, &context, "a", "Main old\n", 4);
    app.overleaf_presence_action(BadgeAction::Follow("remote".into()), &context);
    assert_eq!(app.overleaf.cursor_navigation.reveal(), Some(4));
    let source = "A refreshed server document\n";
    remote_cursor(&mut app, &context, "a", source, 9);
    app.advance_overleaf_cursor(&context);
    assert!(app.overleaf.cursor_navigation.reveal().is_none());
    assert_eq!(
        app.overleaf.cursor_navigation.following_peer(),
        Some("remote")
    );
    assert_eq!(app.document().source(), "Main old\n");
    app.document_mut().replace_loaded_unprojected(
        source.into(),
        root.join("main.tex"),
        DocumentKind::Tex,
        None,
    );
    app.advance_overleaf_cursor(&context);
    assert_eq!(app.overleaf.cursor_navigation.take_reveal(), Some(9));
}

#[test]
fn collaborator_returning_or_leaving_source_cancels_their_obsolete_pending_open() {
    for returns_to_active in [true, false] {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().canonicalize().unwrap();
        let context = egui::Context::default();
        let mut app = fixture(&root, &context);
        remote_cursor(&mut app, &context, "a", "Main old\n", 4);
        app.overleaf_presence_action(BadgeAction::Follow("remote".into()), &context);
        remote_cursor(&mut app, &context, "b", "Chapter old\n", 3);
        // The native browser has admitted Open B, but its fresh mirror has not
        // arrived. The collaborator changes location before that receipt.
        app.arm_overleaf_navigation();
        app.overleaf.open_after = Some("b".into());
        app.overleaf
            .cursor_navigation
            .target
            .as_mut()
            .unwrap()
            .start_open("b".into());
        if returns_to_active {
            remote_cursor(&mut app, &context, "a", "Main old\n", 5);
        } else {
            let event = serde_json::from_value(serde_json::json!({
                "event": "presence", "epoch": EPOCH, "own_id": "own", "self_user_id": "account",
                "peers": [{"id": "remote", "user_id": "other", "name": "Collaborator", "doc_id": null, "row": null, "column": null}],
                "documents": []
            })).unwrap();
            app.update_overleaf_event(event, &context).unwrap();
        }
        app.advance_overleaf_cursor(&context);
        assert!(
            app.overleaf.open_after.is_none(),
            "the obsolete document must not open later"
        );
        assert!(app.overleaf.navigation.is_none());
        assert!(
            app.overleaf
                .cursor_navigation
                .target
                .as_ref()
                .unwrap()
                .opening()
                .is_none()
        );
        assert_eq!(
            app.overleaf.cursor_navigation.following_peer(),
            Some("remote")
        );
        assert_eq!(
            app.overleaf.cursor_navigation.reveal(),
            returns_to_active.then_some(5)
        );
        assert_eq!(app.document().source(), "Main old\n");
        assert!(!app.document().is_dirty());
    }
}

#[test]
fn table_editor_ignores_new_collaborator_seek_and_follow_before_dispatch() {
    for following in [false, true] {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().canonicalize().unwrap();
        let context = egui::Context::default();
        let mut app = fixture(&root, &context);
        let local = CCursorRange::two(CCursor::new(2), CCursor::new(6));
        app.store_editor_cursor(&context, local);
        remote_cursor(&mut app, &context, "b", "Chapter old\n", 3);
        app.begin_new_tex_table(&context);
        assert!(
            app.table_editor.is_some(),
            "a real table draft owns the source"
        );
        assert!(app.document_flow_busy());
        let key = app.document().key();
        let tab = app.tabs.active_id();
        let preview = app.tabs.preview_id();
        let action = if following {
            BadgeAction::Follow("remote".into())
        } else {
            BadgeAction::Seek("remote".into())
        };
        app.overleaf_presence_action(action, &context);
        app.advance_overleaf_cursor(&context);
        assert!(app.overleaf.cursor_navigation.target.is_none());
        assert!(app.overleaf.cursor_navigation.reveal().is_none());
        assert!(app.overleaf.open_after.is_none());
        assert!(app.overleaf.navigation.is_none());
        assert!(
            app.overleaf.error.is_none(),
            "without a browser, an attempted Open would report a dispatch error"
        );
        assert!(app.pending_editor_selection.is_none());
        assert_eq!(app.tabs.active_id(), tab);
        assert_eq!(app.tabs.preview_id(), preview);
        assert_eq!(app.document().key(), key);
        assert_eq!(app.document().source(), "Main old\n");
        assert_eq!(app.editor_snapshot(&context).cursor, local);
        assert!(app.table_editor.is_some());
    }
}

#[test]
fn opening_a_table_editor_cancels_follow_before_a_cross_document_request() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().canonicalize().unwrap();
    let context = egui::Context::default();
    let mut app = fixture(&root, &context);
    remote_cursor(&mut app, &context, "a", "Main old\n", 4);
    app.overleaf_presence_action(BadgeAction::Follow("remote".into()), &context);
    assert_eq!(app.overleaf_following_peer(), Some("remote"));
    assert_eq!(app.overleaf.cursor_navigation.reveal(), Some(4));
    app.begin_new_tex_table(&context);
    assert!(app.table_editor.is_some());
    let key = app.document().key();
    remote_cursor(&mut app, &context, "b", "Chapter old\n", 3);
    app.advance_overleaf_cursor(&context);
    assert!(app.overleaf.cursor_navigation.target.is_none());
    assert!(app.overleaf.cursor_navigation.reveal().is_none());
    assert!(app.overleaf.open_after.is_none());
    assert!(app.overleaf.navigation.is_none());
    assert!(
        app.overleaf.error.is_none(),
        "no browser Open may be dispatched"
    );
    assert!(app.pending_editor_selection.is_none());
    assert_eq!(app.document().key(), key);
    assert_eq!(app.document().source(), "Main old\n");
    assert!(app.table_editor.is_some());
}

#[test]
fn late_collaborator_open_receipt_cannot_finish_or_retry_through_a_table_editor() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().canonicalize().unwrap();
    let context = egui::Context::default();
    let mut app = fixture(&root, &context);
    let (main, chapter) = tests::park_main(&mut app, &root, &context);
    app.activate_tab(main, &context);
    assert_eq!(app.tabs.active_id(), Some(main));
    let local = CCursorRange::two(CCursor::new(2), CCursor::new(6));
    app.store_editor_cursor(&context, local);
    remote_cursor(&mut app, &context, "a", "Main old\n", 4);
    app.overleaf_presence_action(BadgeAction::Follow("remote".into()), &context);
    remote_cursor(&mut app, &context, "b", "Chapter old\n", 3);
    // Admit the browser Open before a table draft takes ownership. Its fresh
    // mirror can arrive in the next frame, before advance checks cancellation.
    app.arm_overleaf_navigation();
    app.overleaf.open_after = Some("b".into());
    app.overleaf
        .cursor_navigation
        .target
        .as_mut()
        .unwrap()
        .start_open("b".into());
    app.begin_new_tex_table(&context);
    assert!(app.table_editor.is_some());
    let key = app.document().key();
    let preview = app.tabs.preview_id();
    let tabs = app.tabs.len();
    tests::apply_work(
        &mut app,
        MirrorWork {
            checkpoint: false,
            entry: entry("b", "chapter.tex", EntryKind::Doc),
            data: MirrorData::Document("Chapter old\n".into()),
            save: None,
        },
        &root,
        EPOCH,
        &context,
    );
    assert_eq!(app.tabs.active_id(), Some(main));
    assert_ne!(app.tabs.active_id(), Some(chapter));
    assert_eq!(app.tabs.preview_id(), preview);
    assert_eq!(app.tabs.len(), tabs);
    assert_eq!(app.document().key(), key);
    assert_eq!(app.document().source(), "Main old\n");
    assert_eq!(app.editor_snapshot(&context).cursor, local);
    assert!(app.table_editor.is_some());
    assert!(app.overleaf.cursor_navigation.target.is_none());
    assert!(app.overleaf.cursor_navigation.reveal().is_none());
    assert!(app.overleaf.open_after.is_none());
    assert!(app.overleaf.navigation.is_none());
    assert!(app.overleaf.error.is_none());
    // Closing the draft cannot revive the abandoned follow or loop Open B.
    app.table_editor = None;
    for _ in 0..3 {
        app.advance_overleaf_cursor(&context);
        assert!(app.overleaf.open_after.is_none());
        assert!(app.overleaf.error.is_none());
        assert_eq!(app.tabs.active_id(), Some(main));
    }
}

#[test]
fn keyboard_navigation_without_a_widget_owner_stops_following_without_consuming_the_key() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().canonicalize().unwrap();
    let context = egui::Context::default();
    let mut app = fixture(&root, &context);
    remote_cursor(&mut app, &context, "a", "Main old\n", 4);
    app.overleaf_presence_action(BadgeAction::Follow("remote".into()), &context);
    app.overleaf.cursor_navigation.take_reveal();
    assert_eq!(app.overleaf_following_peer(), Some("remote"));
    assert!(context.memory(|memory| memory.focused()).is_none());
    let key = egui::Event::Key {
        key: egui::Key::ArrowLeft,
        physical_key: None,
        pressed: true,
        repeat: false,
        modifiers: egui::Modifiers::NONE,
    };
    context
        .run_ui(
            egui::RawInput {
                events: vec![key.clone()],
                ..Default::default()
            },
            |ui| {
                assert!(ui.memory(|memory| memory.focused()).is_none());
                app.advance_overleaf_cursor(ui.ctx());
                assert!(
                    ui.input(|input| input.events.contains(&key)),
                    "stopping follow must leave unclaimed navigation available to the editor"
                );
                assert!(ui.memory(|memory| memory.focused()).is_none());
            },
        )
        .drop_without_applying_deltas();
    assert!(app.overleaf.cursor_navigation.target.is_none());
    assert!(app.overleaf.cursor_navigation.reveal().is_none());
    assert!(app.pending_editor_selection.is_none());
    assert_eq!(app.document().source(), "Main old\n");
    assert!(!app.document().is_dirty());
}

#[test]
fn global_command_keys_preserve_follow_with_source_or_unclaimed_keyboard_focus() {
    for editor_focused in [false, true] {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().canonicalize().unwrap();
        let context = egui::Context::default();
        let mut app = fixture(&root, &context);
        let local = CCursorRange::two(CCursor::new(2), CCursor::new(6));
        app.store_editor_cursor(&context, local);
        remote_cursor(&mut app, &context, "a", "Main old\n", 4);
        app.overleaf_presence_action(BadgeAction::Follow("remote".into()), &context);
        app.overleaf.cursor_navigation.take_reveal();
        let editor_id = source_editor_id(&context);
        let expected_focus = editor_focused.then_some(editor_id);
        for (key, modifiers) in [
            (egui::Key::F12, Modifiers::COMMAND | Modifiers::SHIFT),
            (egui::Key::S, Modifiers::COMMAND),
            (egui::Key::C, Modifiers::COMMAND),
            // Primary+R is the shipped Compile shortcut. Function keys can
            // also be assigned global actions without becoming source input.
            (egui::Key::R, Modifiers::COMMAND),
            (egui::Key::F5, Modifiers::NONE),
        ] {
            let event = egui::Event::Key {
                key,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers,
            };
            context
                .run_ui(
                    egui::RawInput {
                        events: vec![event.clone()],
                        ..Default::default()
                    },
                    |ui| {
                        if editor_focused {
                            ui.memory_mut(|memory| memory.request_focus(editor_id));
                        }
                        assert_eq!(ui.memory(|memory| memory.focused()), expected_focus);
                        app.advance_overleaf_cursor(ui.ctx());
                        assert!(
                            ui.input(|input| input.events.contains(&event)),
                            "global {key:?} must remain available to its command handler"
                        );
                        assert_eq!(ui.memory(|memory| memory.focused()), expected_focus);
                    },
                )
                .drop_without_applying_deltas();
            assert_eq!(
                app.overleaf_following_peer(),
                Some("remote"),
                "global {key:?} must preserve follow with editor focus={editor_focused}"
            );
            assert!(app.overleaf.cursor_navigation.reveal().is_none());
            assert!(app.pending_editor_selection.is_none());
            assert_eq!(app.editor_snapshot(&context).cursor, local);
            assert_eq!(app.document().source(), "Main old\n");
            assert!(!app.document().is_dirty());
        }
    }
}

fn start_follow_test_save(app: &mut EditorApp, tab: u64, id: &str) -> (u64, String) {
    let document = app.document_for_tab(tab).unwrap();
    let key = document.key();
    let request = document
        .prepare_save(document.path().clone().unwrap(), document.kind())
        .unwrap();
    let source = request.source().to_owned();
    let model = &mut app.overleaf.connection.as_mut().unwrap().model;
    model.set_draft(id, source.clone()).unwrap();
    let Some(BrowserCommand::Edit { request: wire, .. }) = model.publish(id) else {
        panic!("the save must own a real model publication");
    };
    let operation = begin_test_save(app);
    app.overleaf.save = SavePhase::AwaitingAcknowledgement(RemoteSave {
        operation,
        id: id.into(),
        wire,
        tab,
        key,
        request,
        continuation: None,
        started: Instant::now(),
    });

    (wire, source)
}

fn finish_follow_test_save(
    app: &mut EditorApp,
    root: &Path,
    context: &egui::Context,
    id: &str,
    wire: u64,
    source: String,
) {
    app.update_overleaf_event(
        BrowserEvent::Ack {
            epoch: EPOCH,
            id: id.into(),
            request: wire,
            source,
        },
        context,
    )
    .unwrap();
    let index = app
        .overleaf
        .queued
        .iter()
        .position(|work| work.entry.id == id && work.save.is_some())
        .expect("the matching acknowledgment must retain the native save receipt");
    let work = app.overleaf.queued.remove(index).unwrap();
    tests::apply_work(app, work, root, EPOCH, context);
    assert!(!app.document_workflow.save_in_flight());
}

#[test]
fn ordinary_explicit_or_parked_save_suspends_follow_until_its_real_receipt() {
    for parked in [false, true] {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().canonicalize().unwrap();
        let context = egui::Context::default();
        let mut app = fixture(&root, &context);
        let (save_tab, save_id) = if parked {
            let (main, chapter) = tests::park_main(&mut app, &root, &context);
            app.activate_tab(main, &context);
            assert_eq!(app.tabs.active_id(), Some(main));
            app.tabs
                .document_mut(chapter)
                .unwrap()
                .edit(CCursorRange::default(), |source| {
                    source.push_str("Parked draft\n")
                });
            (chapter, "b")
        } else {
            (app.tabs.active_id().unwrap(), "a")
        };
        let owner = app.tabs.active_id();
        let key = app.document().key();
        let local = CCursorRange::two(CCursor::new(2), CCursor::new(6));
        app.store_editor_cursor(&context, local);
        remote_cursor(&mut app, &context, "a", "Main old\n", 4);
        app.overleaf_presence_action(BadgeAction::Follow("remote".into()), &context);
        assert_eq!(app.overleaf.cursor_navigation.take_reveal(), Some(4));
        let (wire, source) = start_follow_test_save(&mut app, save_tab, save_id);
        assert!(app.document_flow_busy());
        assert!(!app.document_transaction_busy());
        remote_cursor(&mut app, &context, "a", "Main old\n", 7);
        for _ in 0..8 {
            app.advance_overleaf_cursor(&context);
            context
                .run_ui(Default::default(), |ui| {
                    assert!(app.take_overleaf_reveal(ui).is_none());
                })
                .drop_without_applying_deltas();
            assert_eq!(app.overleaf_following_peer(), Some("remote"));
            assert!(app.overleaf.open_after.is_none());
            assert!(app.overleaf.error.is_none());
            assert!(app.overleaf.cursor_navigation.reveal().is_none());
            assert_eq!(app.tabs.active_id(), owner);
            assert_eq!(app.document().key(), key);
            assert_eq!(app.editor_snapshot(&context).cursor, local);
        }
        finish_follow_test_save(&mut app, &root, &context, save_id, wire, source);
        app.advance_overleaf_cursor(&context);
        assert_eq!(app.overleaf_following_peer(), Some("remote"));
        assert_eq!(app.overleaf.cursor_navigation.take_reveal(), Some(7));
        app.advance_overleaf_cursor(&context);
        assert!(app.overleaf.cursor_navigation.reveal().is_none());
        assert_eq!(app.tabs.active_id(), owner);
        assert_eq!(app.document().source(), "Main old\n");
        assert_eq!(app.editor_snapshot(&context).cursor, local);
        assert!(!app.document().is_dirty());
    }
}

#[test]
fn cursor_open_arriving_during_save_is_deferred_without_losing_follow_or_reusing_its_receipt() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().canonicalize().unwrap();
    let context = egui::Context::default();
    let mut app = fixture(&root, &context);
    let (main, chapter) = tests::park_main(&mut app, &root, &context);
    app.activate_tab(main, &context);
    assert_eq!(app.tabs.active_id(), Some(main));
    remote_cursor(&mut app, &context, "a", "Main old\n", 4);
    app.overleaf_presence_action(BadgeAction::Follow("remote".into()), &context);
    remote_cursor(&mut app, &context, "b", "Chapter old\n", 3);
    app.arm_overleaf_navigation();
    app.overleaf.open_after = Some("b".into());
    app.overleaf
        .cursor_navigation
        .target
        .as_mut()
        .unwrap()
        .start_open("b".into());
    let (wire, source) = start_follow_test_save(&mut app, main, "a");
    let key = app.document().key();
    let preview = app.tabs.preview_id();
    tests::apply_work(
        &mut app,
        MirrorWork {
            checkpoint: false,
            entry: entry("b", "chapter.tex", EntryKind::Doc),
            data: MirrorData::Document("Chapter old\n".into()),
            save: None,
        },
        &root,
        EPOCH,
        &context,
    );
    assert_eq!(app.overleaf_following_peer(), Some("remote"));
    assert!(app.overleaf.open_after.is_none());
    assert!(app.overleaf.navigation.is_none());
    assert!(
        app.overleaf
            .cursor_navigation
            .target
            .as_ref()
            .unwrap()
            .opening()
            .is_none()
    );
    for _ in 0..8 {
        app.advance_overleaf_cursor(&context);
        assert_eq!(app.overleaf_following_peer(), Some("remote"));
        assert!(app.overleaf.open_after.is_none());
        assert!(
            app.overleaf.error.is_none(),
            "saving cannot send another Open"
        );
        assert_eq!(app.tabs.active_id(), Some(main));
        assert_eq!(app.tabs.preview_id(), preview);
        assert_eq!(app.document().key(), key);
    }
    finish_follow_test_save(&mut app, &root, &context, "a", wire, source);
    assert_eq!(app.overleaf_following_peer(), Some("remote"));
    assert_eq!(app.tabs.active_id(), Some(main));
    assert_ne!(app.tabs.active_id(), Some(chapter));
    // A retry must use the browser freshness path, never activate from the
    // already-consumed receipt. This fixture deliberately has no browser;
    // admission therefore fails once, retaining the original document.
    app.advance_overleaf_cursor(&context);
    let error = app
        .overleaf
        .error
        .clone()
        .expect("a fresh browser Open was attempted");
    assert!(app.overleaf.cursor_navigation.target.is_none());
    for _ in 0..3 {
        app.advance_overleaf_cursor(&context);
        assert_eq!(app.overleaf.error.as_deref(), Some(error.as_str()));
        assert!(app.overleaf.open_after.is_none());
        assert_eq!(app.tabs.active_id(), Some(main));
    }
    assert_eq!(app.tabs.preview_id(), preview);
    assert_eq!(app.document().source(), "Main old\n");
    assert!(!app.document().is_dirty());
}
