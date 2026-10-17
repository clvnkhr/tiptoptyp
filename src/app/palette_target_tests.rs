//! Captured palette targets must survive the popup's temporary native focus.
use super::*;

fn raw(viewport: egui::ViewportId, fullscreen: bool) -> egui::RawInput {
    let mut input = egui::RawInput {
        viewport_id: viewport,
        screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(1000.0, 700.0))),
        ..Default::default()
    };
    let info = input.viewports.entry(viewport).or_default();
    info.fullscreen = Some(fullscreen);
    info.focused = Some(true);
    if viewport != egui::ViewportId::ROOT {
        info.parent = Some(egui::ViewportId::ROOT);
    }
    input
}

fn origin(viewport: egui::ViewportId, preview: Option<preview_controls::Target>) -> Origin {
    Origin {
        viewport,
        focus: None,
        terminal: false,
        preview,
        tex_mode: false,
    }
}

fn palette_input(context: &egui::Context, fullscreen: bool) -> egui::RawInput {
    let mut input = raw(egui::ViewportId::ROOT, fullscreen);
    input
        .viewports
        .get_mut(&egui::ViewportId::ROOT)
        .unwrap()
        .focused = Some(false);
    input.viewports.insert(
        scoped_child_viewport_id(context, "tiptoptyp-popup-overlay"),
        egui::ViewportInfo {
            parent: Some(egui::ViewportId::ROOT),
            focused: Some(true),
            ..Default::default()
        },
    );
    input
}

#[test]
fn palette_zoom_keeps_the_captured_detached_viewer_beside_the_other_main_viewer() {
    for detached_source in [true, false] {
        let directory = tempfile::tempdir().unwrap();
        let context = egui::Context::default();
        let mut app = EditorApp::dormant_for_tests(&context, directory.path().into());
        app.snapshot_scene = None;
        app.document_mut().replace_loaded_unprojected(
            "= Pinned source\n".into(),
            directory.path().join("source.typ"),
            DocumentKind::Typst,
            None,
        );
        app.prepare_asset_tab_fixture(
            &context,
            directory.path().join("asset.pdf"),
            DocumentKind::Pdf,
        );
        app.preview = PreviewController::new(false, PreviewPreference::Pdfium);
        app.view_mode = ViewMode::Split;
        let asset = preview_controls::Target::Asset(app.document().key());
        let target = if detached_source {
            preview_controls::Target::Source
        } else {
            asset
        };
        app.preview_controls.select(target);
        app.preview_control_action(preview_controls::Action::PopOut);
        assert!(app.preview_visible());
        assert_eq!(app.preview_controls.popout_target, target);
        let ids = (app.tabs.active_id(), app.tabs.preview_id());
        let popup = scoped_child_viewport_id(&context, "tiptoptyp-popup-overlay");
        let preview = scoped_child_viewport_id(&context, "preview-window");
        let controls = crate::child_view::child_viewport_id(preview, preview_controls::SALT);
        let output = context.run_ui(palette_input(&context, false), |ui| {
            assert_eq!(focused_input_viewport(ui.ctx()), popup);
            app.execute_palette_action(
                ShortcutAction::PreviewZoomIn,
                origin(controls, Some(target)),
                ui.ctx(),
                None,
            );
        });
        output.drop_without_applying_deltas();
        assert_eq!(
            app.pdfium_preview.controls_snapshot().zoom,
            if detached_source { 1.2 } else { 1.0 }
        );
        assert_eq!(
            app.pdfium_asset.controls_snapshot().zoom,
            if detached_source { 1.0 } else { 1.2 }
        );
        assert_eq!((app.tabs.active_id(), app.tabs.preview_id()), ids);
        assert_eq!(app.document().kind(), DocumentKind::Pdf);
    }
}

#[test]
fn palette_window_management_uses_the_original_windows_state_and_owner() {
    for detached in [false, true] {
        for fullscreen in [false, true] {
            let directory = tempfile::tempdir().unwrap();
            let context = egui::Context::default();
            context.set_embed_viewports(false);
            let mut app = EditorApp::dormant_for_tests(&context, directory.path().into());
            let root = egui::ViewportId::ROOT;
            let preview = scoped_child_viewport_id(&context, "preview-window");
            let controls = crate::child_view::child_viewport_id(preview, preview_controls::SALT);
            let find = scoped_child_viewport_id(&context, find_bar::VIEWPORT_SALT);
            let (from, target) = if detached {
                (controls, preview)
            } else {
                (find, root)
            };
            crate::window_host::set_management_owner(&context, find, Some(root));
            crate::window_host::set_management_owner(&context, controls, Some(preview));
            // The detached viewer and document deliberately disagree. Reading
            // the root's state before targeting the preview would still fail.
            context
                .run_ui(
                    raw(preview, if detached { fullscreen } else { !fullscreen }),
                    |_| {},
                )
                .drop_without_applying_deltas();
            let root_fullscreen = if detached { !fullscreen } else { fullscreen };
            let output = context.run_ui(palette_input(&context, root_fullscreen), |ui| {
                // A real persistent preview is registered by its owner every
                // pass. Otherwise egui retires it and drops targeted commands.
                ui.ctx().show_viewport_deferred(
                    preview,
                    egui::ViewportBuilder::default(),
                    |_, _| {},
                );
                assert_eq!(
                    ui.ctx()
                        .input_for(preview, |input| input.viewport().fullscreen),
                    Some(if detached { fullscreen } else { !fullscreen })
                );
                app.execute_palette_action(
                    ShortcutAction::ToggleFullscreen,
                    origin(from, None),
                    ui.ctx(),
                    None,
                );
            });
            let commands: Vec<_> = output
                .viewport_output
                .iter()
                .flat_map(|(id, viewport)| {
                    viewport
                        .commands
                        .iter()
                        .filter_map(move |command| match command {
                            egui::ViewportCommand::Fullscreen(value) => Some((*id, *value)),
                            _ => None,
                        })
                })
                .collect();
            assert_eq!(commands, vec![(target, !fullscreen)]);
            output.drop_without_applying_deltas();

            let output = context.run_ui(palette_input(&context, root_fullscreen), |ui| {
                // A real persistent preview is registered by its owner every
                // pass. Otherwise egui retires it and drops targeted commands.
                ui.ctx().show_viewport_deferred(
                    preview,
                    egui::ViewportBuilder::default(),
                    |_, _| {},
                );
                assert_eq!(
                    ui.ctx()
                        .input_for(preview, |input| input.viewport().fullscreen),
                    Some(if detached { fullscreen } else { !fullscreen })
                );
                app.execute_palette_action(
                    ShortcutAction::Minimize,
                    origin(from, None),
                    ui.ctx(),
                    None,
                );
            });
            let commands: Vec<_> = output
                .viewport_output
                .iter()
                .flat_map(|(id, viewport)| {
                    viewport
                        .commands
                        .iter()
                        .filter_map(move |command| match command {
                            egui::ViewportCommand::Minimized(value) => Some((*id, *value)),
                            _ => None,
                        })
                })
                .collect();
            assert_eq!(commands, vec![(target, true)]);
            output.drop_without_applying_deltas();
        }
    }
}

#[test]
fn palette_page_commands_keep_the_captured_detached_viewer_beside_the_main_viewer() {
    for detached_source in [true, false] {
        for action in [
            ShortcutAction::PreviewPreviousPage,
            ShortcutAction::PreviewNextPage,
        ] {
            let directory = tempfile::tempdir().unwrap();
            let context = egui::Context::default();
            let mut app = EditorApp::dormant_for_tests(&context, directory.path().into());
            app.snapshot_scene = None;
            app.document_mut().replace_loaded_unprojected(
                "= Pinned source\n".into(),
                directory.path().join("source.typ"),
                DocumentKind::Typst,
                None,
            );
            app.prepare_asset_tab_fixture(
                &context,
                directory.path().join("asset.pdf"),
                DocumentKind::Pdf,
            );
            app.preview = PreviewController::new(false, PreviewPreference::Pdfium);
            app.view_mode = ViewMode::Split;
            let asset = preview_controls::Target::Asset(app.document().key());
            let target = if detached_source {
                preview_controls::Target::Source
            } else {
                asset
            };
            app.preview_controls.select(target);
            app.preview_control_action(preview_controls::Action::PopOut);
            let ids = (app.tabs.active_id(), app.tabs.preview_id());
            let preview = scoped_child_viewport_id(&context, "preview-window");
            let controls = crate::child_view::child_viewport_id(preview, preview_controls::SALT);
            context
                .run_ui(palette_input(&context, false), |ui| {
                    app.execute_palette_action(
                        action,
                        origin(controls, Some(target)),
                        ui.ctx(),
                        None,
                    );
                })
                .drop_without_applying_deltas();
            // Both backends start at page zero with no catalog. Navigation is
            // bounded at zero, but must still reach exactly the chosen viewer.
            assert_eq!(
                app.pdfium_preview.requested_page(),
                detached_source.then_some(0),
                "wrong source viewer for {action:?}"
            );
            assert_eq!(
                app.pdfium_asset.requested_page(),
                (!detached_source).then_some(0),
                "wrong asset viewer for {action:?}"
            );
            assert_eq!((app.tabs.active_id(), app.tabs.preview_id()), ids);
            assert_eq!(app.document().kind(), DocumentKind::Pdf);
        }
    }
}

#[test]
fn palette_page_target_uses_the_main_viewer_without_redirecting_a_stale_capture() {
    for detached_source in [true, false] {
        let directory = tempfile::tempdir().unwrap();
        let context = egui::Context::default();
        let mut app = EditorApp::dormant_for_tests(&context, directory.path().into());
        app.snapshot_scene = None;
        app.document_mut().replace_loaded_unprojected(
            "= Pinned source\n".into(),
            directory.path().join("source.typ"),
            DocumentKind::Typst,
            None,
        );
        app.prepare_asset_tab_fixture(
            &context,
            directory.path().join("asset.pdf"),
            DocumentKind::Pdf,
        );
        app.preview = PreviewController::new(false, PreviewPreference::Pdfium);
        app.view_mode = ViewMode::Split;
        let asset = preview_controls::Target::Asset(app.document().key());
        let source = preview_controls::Target::Source;
        app.preview_controls
            .select(if detached_source { source } else { asset });
        app.preview_control_action(preview_controls::Action::PopOut);
        assert_eq!(
            app.palette_preview_target(origin(egui::ViewportId::ROOT, None)),
            Some(if detached_source { asset } else { source })
        );
        let stale = preview_controls::Target::Asset(DocumentKey {
            revision: app.document().key().revision + 1,
            ..app.document().key()
        });
        assert!(
            app.palette_preview_target(origin(egui::ViewportId::ROOT, Some(stale)))
                .is_none()
        );
    }
}
