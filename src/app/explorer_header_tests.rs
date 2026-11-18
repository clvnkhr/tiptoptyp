use super::*;
use egui_kittest::{Harness, kittest::Queryable as _};

struct HeaderState {
    status: Option<WorkspaceStatus<'static>>,
    dark: bool,
    name: Rect,
    refresh: Rect,
}

fn header(
    width: f32,
    status: Option<WorkspaceStatus<'static>>,
    dark: bool,
) -> Harness<'static, HeaderState> {
    let mut configured = false;
    Harness::builder()
        .with_size(Vec2::new(width, 100.0))
        .with_step_dt(0.1)
        .build_ui_state(
            move |ui, state: &mut HeaderState| {
                if !configured {
                    theme::configure_test_fonts(ui.ctx());
                    configured = true;
                }
                ui.ctx().set_theme(if state.dark { egui::Theme::Dark } else { egui::Theme::Light });
                theme::panel_header(ui, "workspace-status-test", |ui| {
                    state.name = workspace_header_label(
                        ui,
                        "Overleaf · A very long mathematical project name with several chapters and appendices",
                        state.status,
                    ).rect;
                    state.refresh = icon_button(ui, UiIcon::Refresh, "Refresh filesystem").rect;
                });
            },
            HeaderState { status, dark, name: Rect::ZERO, refresh: Rect::ZERO },
        )
}

fn status(text: &'static str, tone: WorkspaceStatusTone) -> Option<WorkspaceStatus<'static>> {
    Some(WorkspaceStatus { text, tone })
}

#[test]
fn status_column_is_thin_right_aligned_stable_and_absent_for_local_workspaces() {
    for width in [120.0, 240.0, 520.0] {
        let mut harness = header(width, status("Ready", WorkspaceStatusTone::Ready), false);
        harness.run();
        let name = harness.state().name;
        let refresh = harness.state().refresh;
        let dot = harness.get_by_label("Ready").rect();
        assert_eq!(dot.width(), WORKSPACE_STATUS_WIDTH);
        assert_eq!(dot.height(), METRICS.explorer.header_row_height);
        assert!((dot.center().y - name.center().y).abs() < 0.01);
        assert!((dot.left() - name.right() - theme::SPACE.small).abs() < 0.01);
        assert!((refresh.left() - dot.right() - theme::SPACE.small).abs() < 0.01);
        assert!(refresh.right() <= width);
        assert!(!harness.output().shapes.iter().any(|shape| {
            matches!(&shape.shape, egui::Shape::Text(text) if text.galley.text().contains("Ready"))
        }), "status belongs to hover text, never to the painted name");

        harness.state_mut().status = status(
            "A much longer saving status with additional server confirmation details",
            WorkspaceStatusTone::Busy,
        );
        harness.run();
        let long_dot = harness
            .get_by_label("A much longer saving status with additional server confirmation details")
            .rect();
        assert_eq!(long_dot, dot);
        assert_eq!(harness.state().name, name);
        assert_eq!(harness.state().refresh, refresh);

        harness.state_mut().status = None;
        harness.run();
        assert!(harness.query_by_label("Ready").is_none());
        assert!(!harness.output().shapes.iter().any(|shape| {
            matches!(&shape.shape, egui::Shape::Circle(circle) if circle.radius == WORKSPACE_STATUS_RADIUS)
        }));
        assert_eq!(harness.state().refresh, refresh);
        assert!(
            (harness.state().name.width()
                - name.width()
                - WORKSPACE_STATUS_WIDTH
                - theme::SPACE.small)
                .abs()
                < 0.01,
            "local name reclaims both the indicator lane and its gap"
        );
    }
}

#[test]
fn indicator_uses_the_current_theme_palette_for_every_status() {
    for dark in [false, true] {
        let mut harness = header(300.0, status("Status", WorkspaceStatusTone::Ready), dark);
        for tone in [
            WorkspaceStatusTone::Ready,
            WorkspaceStatusTone::Busy,
            WorkspaceStatusTone::Warning,
            WorkspaceStatusTone::Error,
            WorkspaceStatusTone::Offline,
        ] {
            harness.state_mut().status = status("Status", tone);
            harness.run();
            let rect = harness.get_by_label("Status").rect();
            let palette = theme::palette(&harness.ctx);
            let expected = match tone {
                WorkspaceStatusTone::Ready => palette.success,
                WorkspaceStatusTone::Busy => palette.accent,
                WorkspaceStatusTone::Warning => palette.warning,
                WorkspaceStatusTone::Error => palette.error,
                WorkspaceStatusTone::Offline => palette.neutral,
            };
            assert!(harness.output().shapes.iter().any(|shape| {
                matches!(&shape.shape, egui::Shape::Circle(circle)
                    if circle.center == rect.center()
                    && circle.radius == WORKSPACE_STATUS_RADIUS && circle.fill == expected)
            }));
        }
    }
}

#[test]
fn hovering_indicator_shows_current_status_without_changing_header_geometry() {
    let mut harness = header(300.0, status("Saving", WorkspaceStatusTone::Busy), true);
    harness.run();
    let name = harness.state().name;
    let refresh = harness.state().refresh;
    let dot = harness.get_by_label("Saving").rect();
    harness.hover_at(dot.center());
    harness.run_steps(12);
    let overlay_id = super::super::tooltips::native_hover_tooltip_id(&harness.ctx);
    let overlay = harness
        .ctx
        .data(|data| data.get_temp::<super::super::tooltips::HoverTooltipOverlay>(overlay_id))
        .unwrap();
    assert_eq!(&*overlay.detail, "Saving");
    assert_eq!(overlay.origin, dot);
    assert_eq!(harness.state().name, name);
    assert_eq!(harness.state().refresh, refresh);

    harness.state_mut().status = status("Unsynced", WorkspaceStatusTone::Warning);
    harness.run_steps(12);
    let overlay = harness
        .ctx
        .data(|data| data.get_temp::<super::super::tooltips::HoverTooltipOverlay>(overlay_id))
        .unwrap();
    assert_eq!(&*overlay.detail, "Unsynced");
    assert_eq!(overlay.origin, dot);
    assert_eq!(harness.state().name, name);
    assert_eq!(harness.state().refresh, refresh);
}
