//! Git actions keep concise native hover help, including unavailable actions.
use super::*;
use crate::git::editor::{document_diff::DocumentDiff, full_view};
use egui_kittest::{Harness, kittest::Queryable as _};

#[test]
fn docked_diff_toolbar_hints_paint_in_the_native_tooltip_viewport() {
    for (label, hint) in [
        ("Close diff editor", "Close diff"),
        ("Revert hunk", "Discard change"),
        ("Unstage", "Unstage change"),
        ("Stage", "Stage change"),
        ("Next", "Next change"),
        ("Previous", "Previous change"),
    ] {
        for ready in [false, true] {
            let directory = tempfile::tempdir().unwrap();
            let context = egui::Context::default();
            let mut app = EditorApp::dormant_for_tests(&context, directory.path().into());
            app.snapshot_scene = None;
            app.settings.toolbar_style = crate::settings::ToolbarStyle::Icons;
            app.document_diff = Some(DocumentDiff::new("old\n".into(), "new\n"));
            let mut harness = Harness::builder()
                .with_size(Vec2::new(900.0, 420.0))
                .build_ui_state(
                    move |ui, state: &mut (EditorApp, String, bool)| {
                        if !state.2 {
                            crate::theme::configure_test_fonts(ui.ctx());
                            ui.ctx().set_embed_viewports(false);
                            state.2 = true;
                            return;
                        }
                        install_hover_runtime_config(ui.ctx(), Duration::ZERO);
                        full_view::show(
                            ui,
                            state.0.document_diff.as_mut().unwrap(),
                            &mut state.1,
                            full_view::ViewConfig {
                                id: egui::Id::new("source"),
                                path: Path::new("main.typ"),
                                settings: &state.0.settings,
                                repository_ready: ready,
                                mitex_dollars: false,
                                jump: None,
                            },
                        );
                        state.0.show_diagnostic_tooltip_window(ui.ctx());
                        state.0.reconcile_child_view_lifecycles(ui.ctx());
                    },
                    (app, "new\n".into(), false),
                );
            // The native tooltip host needs a focused root and its screen
            // rectangle, just as the composed document window supplies them.
            let root = harness
                .input_mut()
                .viewports
                .get_mut(&egui::ViewportId::ROOT)
                .unwrap();
            root.focused = Some(true);
            root.inner_rect = Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(900.0, 420.0)));
            harness.run();
            let target = harness.get_by_label(label).rect();
            harness.hover_at(target.center());
            harness.run_steps(3);
            let overlay_id = native_hover_tooltip_id(&harness.ctx);
            let overlay = harness
                .ctx
                .data(|data| data.get_temp::<HoverTooltipOverlay>(overlay_id))
                .unwrap_or_else(|| panic!("missing {label} hover (enabled={ready})"));
            assert_eq!(overlay.detail.as_ref(), hint);
            assert!(!overlay.git_diff);
            let child = scoped_child_viewport_id(&harness.ctx, "diagnostic-tooltip-overlay");
            let child_output = &harness.output().viewport_output[&child];
            assert!(
                !child_output
                    .commands
                    .iter()
                    .any(|command| matches!(command, egui::ViewportCommand::Close))
            );
            let callback = child_output
                .viewport_ui_cb
                .clone()
                .expect("tooltip window callback");
            let mut input = egui::RawInput {
                viewport_id: child,
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(240.0, 80.0))),
                ..Default::default()
            };
            input.viewports.insert(
                child,
                egui::ViewportInfo {
                    parent: Some(egui::ViewportId::ROOT),
                    inner_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(240.0, 80.0))),
                    ..Default::default()
                },
            );
            let painted = harness.ctx.run_ui(input, |ui| callback(ui));
            assert!(
                painted.shapes.iter().any(|shape| matches!(&shape.shape,
                    egui::Shape::Text(text) if text.galley.job.text.contains(hint)
                )),
                "{label} tooltip was registered without painting its content"
            );
            painted.drop_without_applying_deltas();
        }
    }
}
