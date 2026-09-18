use super::*;

#[test]
fn native_drag_is_excluded_on_tab_hover_and_until_the_tab_gesture_ends() {
    let mut tabs = Tabs::default();
    tabs.tab_drag_rects.push(Rect::from_min_max(
        Pos2::new(100.0, 0.0),
        Pos2::new(250.0, 25.0),
    ));
    let tab = Some(Pos2::new(150.0, 12.0));
    let empty_toolbar = Some(Pos2::new(350.0, 12.0));
    assert!(
        tabs.claims_window_drag(tab, false),
        "suppress AppKit before mouse-down"
    );
    assert!(!tabs.claims_window_drag(empty_toolbar, false));
    tabs.tab_drag_active = true;
    assert!(tabs.claims_window_drag(empty_toolbar, true));
    assert!(
        tabs.claims_window_drag(None, true),
        "leaving the viewport must not release a held drag"
    );
    assert!(!tabs.claims_window_drag(empty_toolbar, false));
    assert!(!tabs.claims_window_drag(None, false));
}

#[test]
fn tab_highlight_contains_both_controls_and_vector_icons_share_a_centerline() {
    let context = egui::Context::default();
    let root = tempfile::tempdir().unwrap();
    let app = fixture(&context, root.path());
    context
        .run_ui(Default::default(), |ui| {
            let tab = tab_widget(ui, &app.document, true, true, 110.0);
            let preview = tab
                .preview
                .as_ref()
                .expect("Typst tabs have a preview control");
            for control in [&tab.title, preview, &tab.close] {
                assert!(tab.rect.contains_rect(control.rect));
            }
            let eye = tab_icon_rect(preview.rect, UiIcon::Eye);
            let close = tab_icon_rect(tab.close.rect, UiIcon::Close);
            assert_eq!(eye.center().y, close.center().y);
            let (lid, lashes) = closed_eye_icon_geometry(eye);
            assert!(((lid[0].y + lashes[1][1].y) * 0.5 - close.center().y).abs() < 0.001);
            assert!(preview.rect.contains_rect(eye));
            assert!(tab.close.rect.contains_rect(close));
            assert!(
                tab.rect.width() < 180.0,
                "tab must not absorb the remaining title bar"
            );
            assert!(tab.rect.height() < 40.0);
        })
        .drop_without_applying_deltas();
}

#[test]
fn non_typst_tabs_have_no_preview_eye_and_fixed_widths_are_stable() {
    let context = egui::Context::default();
    let root = tempfile::tempdir().unwrap();
    let mut app = fixture(&context, root.path());
    app.document.replace_loaded_unprojected(
        "notes".into(),
        root.path().join("notes.md"),
        DocumentKind::Text,
        None,
    );
    context
        .run_ui(Default::default(), |ui| {
            let tab = tab_widget(ui, &app.document, true, false, 140.0);
            assert!(tab.preview.is_none());
            assert!(tab.rect.contains_rect(tab.title.rect));
            assert!(tab.rect.contains_rect(tab.close.rect));
            let typst_width = tab_title_width("short.typ", 700.0, true, true) + 40.0;
            let text_width = tab_title_width("notes.md", 700.0, true, false) + 20.0;
            assert_eq!(typst_width, FIXED_TAB_WIDTH);
            assert_eq!(text_width, FIXED_TAB_WIDTH);
        })
        .drop_without_applying_deltas();
}

#[test]
fn reordering_tabs_remaps_active_and_preview_without_changing_tab_identity() {
    let mut tabs = Tabs::new(false);
    tabs.parked = vec![None, None, None, None];
    tabs.ids = vec![10, 20, 30, 40];
    tabs.active = 1;
    tabs.preview = 3;

    assert!(tabs.reorder(1, 3));
    assert_eq!(tabs.ids, vec![10, 30, 40, 20]);
    assert_eq!(tabs.active, 3);
    assert_eq!(tabs.preview, 2);

    assert!(tabs.reorder(2, 0));
    assert_eq!(tabs.ids, vec![40, 10, 30, 20]);
    assert_eq!(tabs.active, 3);
    assert_eq!(tabs.preview, 0);
    assert!(!tabs.reorder(0, 0));
}

#[test]
fn stable_ids_resolve_the_same_documents_across_reorder() {
    let root = tempfile::tempdir().unwrap();
    let context = egui::Context::default();
    let mut app = fixture(&context, root.path());
    let first = app.tabs.active_id().unwrap();
    app.append_tab(&context);
    app.document.replace_loaded_unprojected(
        "second".into(),
        root.path().join("second.typ"),
        DocumentKind::Typst,
        None,
    );
    let second = app.tabs.active_id().unwrap();

    assert!(app.tabs.reorder(0, 1));
    assert_eq!(app.tabs.ids().collect::<Vec<_>>(), vec![second, first]);
    assert_eq!(app.tabs.active_id(), Some(second));
    assert_eq!(app.tabs.preview_id(), Some(first));
    assert_eq!(app.document_for_tab(first).unwrap().source(), "first");
    assert_eq!(app.document_for_tab(second).unwrap().source(), "second");
}

#[test]
fn new_tabs_preserve_the_designated_preview_identity() {
    let root = tempfile::tempdir().unwrap();
    let context = egui::Context::default();
    let mut app = fixture(&context, root.path());
    app.lifecycle = DocumentLifecycle::Active;
    let first = app.tabs.active_id().unwrap();

    app.new_tab(&context);
    assert_eq!(app.tabs.preview_id(), Some(first));
    app.document.replace_loaded_unprojected(
        "second".into(),
        root.path().join("second.typ"),
        DocumentKind::Typst,
        None,
    );
    let second = app.tabs.active_id().unwrap();
    app.select_preview_tab(second, &context);
    app.activate_tab(first, &context);

    app.new_tab(&context);
    assert_eq!(app.tabs.preview_id(), Some(second));
    assert_ne!(
        app.tabs.active_id(),
        Some(second),
        "new tab reused preview ID"
    );
    assert_eq!(
        app.tabs.ids().collect::<Vec<_>>(),
        vec![first, second, 2],
        "tab order or identity changed"
    );
    assert_eq!(app.document_for_tab(second).unwrap().source(), "second");
    assert_eq!(app.preview_document_source().unwrap(), "second");
    assert_ne!(app.tabs.active_id(), app.tabs.preview_id());
}

#[test]
fn vector_tab_controls_remain_named_and_do_not_select_a_different_tab() {
    use egui_kittest::{Harness, kittest::Queryable as _};
    let root = tempfile::tempdir().unwrap();
    let context = egui::Context::default();
    let mut app = fixture(&context, root.path());
    app.append_tab(&context);
    app.tabs.preview = 1;
    let mut harness = Harness::builder()
        .with_size(Vec2::new(700.0, 200.0))
        .build_ui_state(|ui, app: &mut EditorApp| app.show_tabs(ui, None), app);
    harness.run_steps(3);
    let active = harness.state().tabs.active;
    harness.get_by_label("Preview first.typ").click();
    harness.run_steps(3);
    assert_eq!(harness.state().tabs.preview, 0);
    assert_eq!(harness.state().tabs.active, active);
    harness.get_by_label("Close first.typ");
}

#[test]
fn tab_titles_can_be_dragged_to_reorder_tabs() {
    check_tab_drag(false, false);
    check_tab_drag(true, false);
    check_tab_drag(false, true);
    check_tab_drag(true, true);
}

fn check_tab_drag(release_with_move: bool, narrow: bool) {
    use egui_kittest::{Harness, kittest::Queryable as _};
    let root = tempfile::tempdir().unwrap();
    let context = egui::Context::default();
    let mut app = fixture(&context, root.path());
    app.append_tab(&context);
    app.document.replace_loaded_unprojected(
        "second".into(),
        root.path().join("second.typ"),
        DocumentKind::Typst,
        None,
    );
    app.append_tab(&context);
    app.document.replace_loaded_unprojected(
        "third".into(),
        root.path().join("third.typ"),
        DocumentKind::Typst,
        None,
    );
    let mut harness = Harness::builder()
        .with_size(Vec2::new(if narrow { 1000.0 } else { 1800.0 }, 400.0))
        .build_ui_state(
            |ui, app: &mut EditorApp| {
                egui::Panel::top("toolbar")
                    .exact_size(METRICS.chrome.toolbar_height)
                    .show(ui, |ui| app.show_toolbar(ui, None));
            },
            app,
        );
    harness.run_steps(3);

    let first = harness
        .get_by_label(if narrow { "second.typ" } else { "first.typ" })
        .rect();
    let third = harness.get_by_label("third.typ").rect();
    let ids = harness.state().tabs.ids.clone();
    let (start, end) = if narrow {
        (third.center(), first.left_center() + Vec2::new(2.0, 0.0))
    } else {
        (first.center(), third.right_center() + Vec2::new(12.0, 0.0))
    };
    assert!(
        harness
            .state()
            .tabs
            .tab_drag_rects
            .iter()
            .any(|rect| rect.contains(start)),
        "the test must press a visible tab"
    );
    let active_id = harness.state().tabs.active_id();
    let preview_id = ids[harness.state().tabs.preview];
    if release_with_move {
        // Native backends can coalesce movement and a button event into one
        // frame; Harness::event normally gives every event its own frame.
        harness
            .input_mut()
            .events
            .push(egui::Event::PointerMoved(start));
    } else {
        harness.event(egui::Event::PointerMoved(start));
        harness.run_steps(1);
    }
    harness.event(egui::Event::PointerButton {
        pos: start,
        button: egui::PointerButton::Primary,
        pressed: true,
        modifiers: egui::Modifiers::NONE,
    });
    harness.run_steps(1);
    assert!(
        harness.state().tabs.tab_drag_source.is_some(),
        "tab must own the press"
    );
    if release_with_move {
        harness
            .input_mut()
            .events
            .push(egui::Event::PointerMoved(end));
    } else {
        for step in 1..=20 {
            harness.event(egui::Event::PointerMoved(
                start.lerp(end, step as f32 / 20.0),
            ));
            harness.run_steps(2);
            assert!(
                !harness.output().viewport_output.values().any(|viewport| {
                    viewport
                        .commands
                        .iter()
                        .any(|command| matches!(command, egui::ViewportCommand::StartDrag))
                }),
                "a tab gesture must not move the native window"
            );
        }
    }
    harness.event(egui::Event::PointerButton {
        pos: end,
        button: egui::PointerButton::Primary,
        pressed: false,
        modifiers: egui::Modifiers::NONE,
    });
    harness.run_steps(4);

    assert_eq!(
        harness.state().tabs.ids,
        if narrow {
            vec![ids[0], ids[2], ids[1]]
        } else {
            vec![ids[1], ids[2], ids[0]]
        }
    );
    assert_eq!(harness.state().tabs.active_id(), active_id);
    assert_eq!(
        harness.state().tabs.ids[harness.state().tabs.preview],
        preview_id
    );
    assert!(harness.state().tabs.tab_drag_source.is_none());
}

#[test]
fn last_tab_closes_to_an_empty_workspace_and_new_reuses_the_window() {
    let root = tempfile::tempdir().unwrap();
    let context = egui::Context::default();
    let mut app = fixture(&context, root.path());
    let old = app.document.key();
    let output = context.run_ui(Default::default(), |ui| app.finish_close_tab(ui.ctx()));
    assert!(!output.viewport_output.values().any(|v| {
        v.commands
            .iter()
            .any(|c| matches!(c, egui::ViewportCommand::Close))
    }));
    output.drop_without_applying_deltas();
    assert!(app.tabs.is_empty());
    assert_ne!(old, app.document.key());
    assert_eq!(app.workspace_root, root.path().canonicalize().unwrap());
    assert!(!app.is_dirty_for_close());
    assert!(!app.preview_visible());
    assert!(!app.typst_preview_available());
    for _ in 0..20 {
        app.schedule_compile_now();
        app.schedule_project_index();
        app.tick_compile(&context);
        app.sync_preview_visibility();
    }
    assert!(app.compile_deadline.is_none());
    assert!(!app.project_index_deadline.is_pending());
    assert!(app.tinymist_generation.is_none());
    assert!(app.native_command_enabled(AppCommand::New));
    assert!(app.native_command_enabled(AppCommand::Open));
    for command in [
        AppCommand::CloseTab,
        AppCommand::Save,
        AppCommand::Copy,
        AppCommand::Paste,
        AppCommand::Find,
    ] {
        assert!(!app.native_command_enabled(command));
    }
    app.new_tab(&context);
    assert_eq!(app.tabs.len(), 1);
    assert!(app.document.kind().is_typst());
    assert!(app.document.path().is_none());
}

#[test]
fn empty_workspace_open_failures_do_not_create_phantom_tabs() {
    let root = tempfile::tempdir().unwrap();
    let context = egui::Context::default();
    let mut app = fixture(&context, root.path());
    app.empty_workspace(&context);
    assert!(!app.open_tab_path(root.path().join("missing.typ"), &context));
    assert!(app.tabs.is_empty());
    let path = root.path().join("opened.typ");
    fs::write(&path, "= Opened").unwrap();
    assert!(app.open_tab_path(path.clone(), &context));
    assert_eq!(app.tabs.len(), 1);
    assert_eq!(
        app.document.path().as_ref(),
        Some(&path.canonicalize().unwrap())
    );
}

#[test]
fn assets_keep_typst_output_and_reject_late_results_after_closing() {
    use crate::asset::{AssetResult, LoadedAsset};
    let root = tempfile::tempdir().unwrap();
    let context = egui::Context::default();
    let mut app = fixture(&context, root.path());
    app.snapshot_scene = None;
    app.preview.replace_asset(
        ArtifactKey::unversioned(app.document.revision()),
        Some(Arc::from(b"typst output".as_slice())),
        Vec::new(),
    );
    app.append_tab(&context);
    app.document.replace_loaded_unprojected(
        String::new(),
        root.path().join("reference.pdf"),
        DocumentKind::Pdf,
        None,
    );
    app.clear_preview_for_document(true);
    let preview_key = app.preview.content.artifact_key();
    let page = || PreviewPage {
        size: [2, 2],
        rgba: vec![255; 16],
        links: Vec::new(),
    };
    let token = app.asset_token;
    app.accept_asset_result(
        &context,
        AssetResult {
            token,
            output: Ok(LoadedAsset::Pdf {
                bytes: b"asset pdf".to_vec(),
                pages: vec![page(), page()],
            }),
        },
    );
    assert_eq!(
        app.preview.content.pdf().map(|pdf| pdf.as_ref()),
        Some(b"typst output".as_slice())
    );
    assert_eq!(app.preview.content.artifact_key(), preview_key);
    assert_eq!(
        app.asset_preview.content.pdf().map(|pdf| pdf.as_ref()),
        Some(b"asset pdf".as_slice())
    );
    app.apply_editor_location(Some(1), None);
    assert_eq!(app.asset_preview.requested_page, Some(1));
    assert!(app.preview.requested_page.is_none());
    app.empty_workspace(&context);
    app.accept_asset_result(
        &context,
        AssetResult {
            token,
            output: Ok(LoadedAsset::Image(page())),
        },
    );
    assert!(app.asset_preview.content.pages().is_empty());
    assert!(app.preview.content.pages().is_empty());
}

#[test]
fn asset_panes_keep_the_preview_in_every_source_view_mode() {
    use workspace_view::{ContentView, content_view};
    for kind in [DocumentKind::Image, DocumentKind::Pdf] {
        for mode in [ViewMode::Code, ViewMode::Split, ViewMode::Preview] {
            assert_eq!(
                content_view(false, kind, true, mode),
                ContentView::SplitAsset
            );
            assert_eq!(content_view(false, kind, false, mode), ContentView::Asset);
            assert_eq!(content_view(true, kind, true, mode), ContentView::Empty);
        }
    }
}

#[test]
fn empty_workspace_new_button_and_close_tab_workflow_are_reusable() {
    use egui_kittest::{Harness, kittest::Queryable as _};
    let root = tempfile::tempdir().unwrap();
    let context = egui::Context::default();
    let app = fixture(&context, root.path());
    let mut harness = Harness::builder()
        .with_size(Vec2::new(700.0, 400.0))
        .build_ui_state(
            |ui, app: &mut EditorApp| {
                if app.tabs.is_empty() {
                    app.show_empty_workspace(ui, None);
                } else {
                    app.show_tabs(ui, None);
                }
                app.execute_pending_document_action(ui.ctx(), None);
            },
            app,
        );
    harness.run_steps(3);
    harness.get_by_label("Close first.typ").click();
    harness.run_steps(3);
    harness.get_by_label("No open tabs");
    assert!(harness.state().tabs.is_empty());
    harness.get_by_label("New document").click();
    harness.run_steps(3);
    assert_eq!(harness.state().tabs.len(), 1);
    assert!(harness.state().document.kind().is_typst());
}

#[test]
fn switching_requeues_a_revision_bound_raster_but_not_a_ready_interactive_preview() {
    assert!(switch_needs_compile(true, true, false, true));
    assert!(switch_needs_compile(true, true, true, true));
    assert!(switch_needs_compile(false, false, true, false));
    assert!(!switch_needs_compile(true, true, true, false));
    assert!(!switch_needs_compile(true, false, false, false));
}

fn fixture(context: &egui::Context, root: &Path) -> EditorApp {
    let mut app = EditorApp::dormant_for_tests(context, root.into());
    app.document.replace_loaded_unprojected(
        "first".into(),
        root.join("first.typ"),
        DocumentKind::Typst,
        None,
    );
    app.document.set_history_reset(false);
    app
}

#[test]
fn save_completion_follows_a_parked_tab_id_after_reorder_not_the_active_slot() {
    let root = tempfile::tempdir().unwrap();
    let context = egui::Context::default();
    let mut app = fixture(&context, root.path());
    app.snapshot_scene = None;
    let path = root.path().join("first.typ");
    fs::write(&path, "first").unwrap();
    app.document.replace_loaded_unprojected(
        "first".into(),
        path.clone(),
        DocumentKind::Typst,
        Some(fingerprint(b"first")),
    );
    app.document
        .edit(CCursorRange::default(), |source| source.push('!'));
    let saved_tab = app.tabs.active_id().unwrap();
    let (entered, ready) = std::sync::mpsc::channel();
    let (release, wait) = std::sync::mpsc::channel();
    let locked_path = path.clone();
    let holder = std::thread::spawn(move || {
        crate::resource_lock::with_resource(&locked_path, || {
            entered.send(()).unwrap();
            let _ = wait.recv();
        })
    });
    ready.recv_timeout(Duration::from_secs(5)).unwrap();
    assert!(app.save_to(path.clone(), &context));
    app.append_tab(&context);
    app.document.replace_unprojected_untitled("other tab");
    assert!(app.tabs.reorder(0, 1));
    release.send(()).unwrap();
    holder.join().unwrap();
    app.finish_save_for_test(&context);
    assert_eq!(fs::read_to_string(path).unwrap(), "first!");
    assert_eq!(app.document.source(), "other tab");
    assert!(!app.document_for_tab(saved_tab).unwrap().is_dirty());
    assert!(app.manual_format_revision.is_none());
}

#[test]
fn close_window_shortcut_is_not_consumed_as_close_tab() {
    for (action, closes_window) in [
        (ShortcutAction::CloseWindow, true),
        (ShortcutAction::CloseTab, false),
    ] {
        let root = tempfile::tempdir().unwrap();
        let context = egui::Context::default();
        let mut app = fixture(&context, root.path());
        app.append_tab(&context);
        let shortcut = app.settings.effective_shortcuts().egui(action).unwrap();
        let mut output = context.run_ui(
            egui::RawInput {
                events: vec![egui::Event::Key {
                    key: shortcut.logical_key,
                    physical_key: None,
                    pressed: true,
                    repeat: false,
                    modifiers: shortcut.modifiers,
                }],
                ..Default::default()
            },
            |ui| {
                app.handle_shortcuts(ui.ctx(), None);
                app.execute_pending_document_action(ui.ctx(), None);
            },
        );
        output.textures_delta.clear();
        assert_eq!(app.tabs.len(), if closes_window { 2 } else { 1 });
        assert_eq!(
            output.viewport_output.values().any(|viewport| {
                viewport
                    .commands
                    .iter()
                    .any(|command| matches!(command, egui::ViewportCommand::Close))
            }),
            closes_window
        );
    }
}

#[test]
fn close_tab_chord_targets_a_focused_child_window() {
    let context = egui::Context::default();
    let shortcuts = AppSettings::default().effective_shortcuts();
    let shortcut = shortcuts.egui(ShortcutAction::CloseTab).unwrap();
    for child in [false, true] {
        context
            .run_ui(
                egui::RawInput {
                    events: vec![egui::Event::Key {
                        key: shortcut.logical_key,
                        physical_key: None,
                        pressed: true,
                        repeat: false,
                        modifiers: shortcut.modifiers,
                    }],
                    ..Default::default()
                },
                |ui| {
                    assert_eq!(
                        ui.input_mut(|input| consume_window_close(input, &shortcuts, child)),
                        child
                    )
                },
            )
            .drop_without_applying_deltas();
    }
}

#[test]
fn tabs_preserve_unsaved_sources_undo_cursor_and_first_preview() {
    let root = tempfile::tempdir().unwrap();
    let context = egui::Context::default();
    let mut app = fixture(&context, root.path());
    let cursor = CCursorRange::one(CCursor::new(3));
    app.document
        .edit(cursor, |source| source.push_str(" edited"));
    app.store_editor_cursor(&context, cursor);
    let stale = app.document.key();
    let first = app.tabs.active_id().unwrap();
    app.append_tab(&context);
    app.document.replace_loaded_unprojected(
        "second".into(),
        root.path().join("second.typ"),
        DocumentKind::Typst,
        None,
    );
    assert_eq!(app.tabs.len(), 2);
    assert_eq!(app.preview_document_source().unwrap(), "first edited");
    let second = app.tabs.active_id().unwrap();
    app.activate_tab(first, &context);
    assert_eq!(app.document.source(), "first edited");
    assert!(app.is_dirty());
    assert_ne!(app.document.key(), stale);
    assert_eq!(app.editor_snapshot(&context).cursor.primary.index.0, 3);
    app.undo_editor(&context, false);
    assert_eq!(app.document.source(), "first");
    app.activate_tab(second, &context);
    assert_eq!(app.document.source(), "second");
    app.select_preview_tab(second, &context);
    app.activate_tab(first, &context);
    assert_eq!(app.preview_document_source().unwrap(), "second");
    assert!(!app.current_is_preview_document());
}

#[test]
fn closing_checks_dirty_background_tabs_without_discarding_on_cancel() {
    let root = tempfile::tempdir().unwrap();
    let context = egui::Context::default();
    let mut app = fixture(&context, root.path());
    app.document
        .edit(CCursorRange::default(), |source| source.push('!'));
    app.append_tab(&context);
    assert!(!app.is_dirty());
    assert!(app.is_dirty_for_close());
    assert!(app.begin_process_close());
    let process_key = app.document_key();
    app.execute_pending_document_action(&context, None);
    assert_eq!(app.tabs.active, 0);
    assert!(matches!(
        app.document_workflow.modal(),
        Some(AppModal::Unsaved { .. })
    ));
    assert_eq!(app.document_key(), process_key);
    assert_eq!(app.process_close_answer(), None);
    app.document_workflow.clear_modal();
    app.document_workflow.cancel_continuation();
    app.document_workflow.finish_dispatch();
    assert_eq!(app.process_close_answer(), Some(false));
    app.finish_process_close(false);
    assert_eq!(app.tabs.len(), 2);
    assert_eq!(app.document.source(), "first!");
    assert!(app.tabs.approved.is_empty());
}

#[test]
fn closing_preview_tab_reselects_first_survivor_and_preserves_dirty_sibling() {
    let root = tempfile::tempdir().unwrap();
    let context = egui::Context::default();
    let mut app = fixture(&context, root.path());
    let first = app.tabs.active_id().unwrap();
    app.append_tab(&context);
    app.document.replace_unprojected_untitled("second");
    app.document
        .edit(CCursorRange::default(), |source| source.push('!'));
    app.activate_tab(first, &context);
    app.finish_close_tab(&context);
    assert_eq!(app.tabs.len(), 1);
    assert_eq!(app.tabs.preview, 0);
    assert_eq!(app.document.source(), "second!");
    assert!(app.is_dirty());
    assert_eq!(app.preview_document_source().unwrap(), "second!");
}

#[test]
fn tab_buttons_select_preview_and_close_the_named_tab() {
    use egui_kittest::{Harness, kittest::Queryable as _};
    let root = tempfile::tempdir().unwrap();
    let context = egui::Context::default();
    let mut app = fixture(&context, root.path());
    app.append_tab(&context);
    app.document.replace_loaded_unprojected(
        "second".into(),
        root.path().join("second.typ"),
        DocumentKind::Typst,
        None,
    );
    let mut harness = Harness::builder()
        .with_size(Vec2::new(700.0, 70.0))
        .build_ui_state(
            |ui, app: &mut EditorApp| {
                app.show_tabs(ui, None);
                app.execute_pending_document_action(ui.ctx(), None);
            },
            app,
        );
    harness.run_steps(3);
    harness.get_by_label("first.typ").click();
    harness.run_steps(3);
    assert_eq!(harness.state().tabs.active, 0);
    harness.get_by_label("Preview second.typ").click();
    harness.run_steps(3);
    assert_eq!(harness.state().tabs.preview, 1);
    assert_eq!(harness.state().tabs.active, 0);
    harness.get_by_label("Close second.typ").click();
    harness.run_steps(3);
    assert_eq!(harness.state().tabs.len(), 1);
    assert_eq!(harness.state().document.source(), "first");
}

#[test]
fn idle_tab_strip_does_not_start_services_or_mutate_inactive_sources() {
    use egui_kittest::Harness;
    let root = tempfile::tempdir().unwrap();
    let context = egui::Context::default();
    let mut app = fixture(&context, root.path());
    for _ in 0..50 {
        app.append_tab(&context);
    }
    let keys: Vec<_> = app
        .tabs
        .parked
        .iter()
        .flatten()
        .map(|tab| tab.document.key())
        .collect();
    let mut harness = Harness::builder()
        .with_size(Vec2::new(700.0, 70.0))
        .build_ui_state(|ui, app: &mut EditorApp| app.show_tabs(ui, None), app);
    harness.run_steps(10);
    assert!(harness.state().tinymist_generation.is_none());
    assert!(!harness.state().project_index_job.is_running());
    assert!(!harness.state().workspace_scan.is_running());
    assert_eq!(
        harness
            .state()
            .tabs
            .parked
            .iter()
            .flatten()
            .map(|tab| tab.document.key())
            .collect::<Vec<_>>(),
        keys
    );
    assert!(harness.state().tabs.next_autosave.is_none());
}

#[test]
fn parked_autosave_checks_disk_and_never_overwrites_external_edits() {
    let root = tempfile::tempdir().unwrap();
    let context = egui::Context::default();
    let mut app = fixture(&context, root.path());
    app.snapshot_scene = None;
    app.settings.auto_save = true;
    let path = root.path().join("first.typ");
    fs::write(&path, "first").unwrap();
    app.document.replace_loaded_unprojected(
        "first".into(),
        path.clone(),
        DocumentKind::Typst,
        Some(fingerprint(b"first")),
    );
    app.document
        .edit(CCursorRange::default(), |source| source.push('!'));
    app.autosave_deadline = Some(Instant::now());
    app.append_tab(&context);
    app.tick_parked_autosave(&context);
    app.finish_save_for_test(&context);
    assert_eq!(fs::read_to_string(&path).unwrap(), "first!");
    assert!(!app.tabs.parked[0].as_ref().unwrap().document.is_dirty());
    let tab = app.tabs.parked[0].as_mut().unwrap();
    tab.document
        .edit(CCursorRange::default(), |source| source.push('?'));
    tab.autosave = Some(Instant::now());
    app.tabs.refresh_autosave();
    fs::write(&path, "external").unwrap();
    app.tick_parked_autosave(&context);
    app.finish_save_for_test(&context);
    assert_eq!(fs::read_to_string(path).unwrap(), "external");
    assert!(app.tabs.parked[0].as_ref().unwrap().document.is_dirty());
}

#[test]
fn all_dirty_tabs_must_be_approved_and_later_edits_revoke_window_close() {
    let root = tempfile::tempdir().unwrap();
    let context = egui::Context::default();
    let mut app = fixture(&context, root.path());
    app.document
        .edit(CCursorRange::default(), |source| source.push('!'));
    app.append_tab(&context);
    app.document
        .edit(CCursorRange::default(), |source| source.push('!'));
    assert!(app.begin_process_close());
    for _ in 0..2 {
        let Some(AppModal::Unsaved { mut pending, .. }) = app.document_workflow.take_modal() else {
            panic!("each dirty tab needs its own confirmation");
        };
        app.document_workflow.clear_modal();
        pending.allow_discard = true;
        app.document_workflow.queue_action(pending);
        app.execute_pending_document_action(&context, None);
    }
    assert_eq!(app.process_close_answer(), Some(true));
    app.tabs
        .parked
        .iter_mut()
        .flatten()
        .next()
        .unwrap()
        .document
        .edit(CCursorRange::default(), |source| source.push('?'));
    assert!(!app.close_accepted());
}
