//! Divider grips and ownership: no extra widgets, spacing, or changed resize hit areas.
use eframe::egui::{self, Pos2, Rect, Stroke, StrokeKind};

/// Only actual divider drags count; text selections and other held buttons do
/// not defer preview work. Require the current button state on the release frame.
pub(super) fn main_panel_resizing(context: &egui::Context) -> bool {
    context.input(|input| input.pointer.primary_down())
        && [
            super::explorer_panel_id(context),
            super::editor_panel_id(context),
            crate::child_view::viewport_scoped_id(context, "bottom-panel"),
        ]
        .into_iter()
        .any(|id| {
            context
                .read_response(id.with("__resize"))
                .is_some_and(|response| response.dragged())
        })
}

/// Reserve the real panel resize handles for pointer interaction without
/// shrinking painted preview pages, their fit-width geometry, or focus bounds.
pub(super) fn preview_interaction_bounds(ui: &egui::Ui, rect: Rect) -> Rect {
    let ids = [
        super::explorer_panel_id(ui.ctx()),
        super::editor_panel_id(ui.ctx()),
        crate::child_view::viewport_scoped_id(ui.ctx(), "bottom-panel"),
    ]
    .map(|id| id.with("__resize"));
    // Only this pass counts: hidden, maximized and detached panels must not
    // reserve a handle retained in the previous pass's observation state.
    let handles = ui.ctx().viewport(|viewport| {
        ids.map(|id| {
            viewport
                .this_pass
                .widgets
                .get(id)
                .filter(|handle| {
                    handle.enabled
                        && handle.sense.senses_drag()
                        && handle.interact_rect.is_positive()
                        && handle.layer_id == ui.layer_id()
                })
                .map(|handle| handle.interact_rect)
        })
    });
    clip_resize_handles(rect, handles)
}

fn clip_resize_handles(mut rect: Rect, handles: [Option<Rect>; 3]) -> Rect {
    for (index, handle) in handles.into_iter().enumerate() {
        let Some(handle) = handle else { continue };
        if !rect.intersects(handle) {
            continue;
        }
        if index == 2 {
            // Bottom panel's handle meets the lower edge of the preview.
            if handle.top() <= rect.bottom() && rect.bottom() <= handle.bottom() {
                rect.max.y = handle.top().next_down().max(rect.top());
            }
        } else if handle.left() <= rect.left() && rect.left() <= handle.right() {
            // Rect hit tests include their edges: exclude the handle's full
            // hit area, not just its visible center line.
            rect.min.x = handle.right().next_up().min(rect.right());
        }
    }
    rect
}

/// The stroke a resizable egui panel paints its separator with while the user
/// hovers or drags it; `None` while idle. Panels register their resize handle
/// under `<panel id>.with("__resize")`, so this mirrors the separator's own
/// active/hovered stroke choice exactly.
pub(super) fn panel_divider(ui: &egui::Ui, panel_id: egui::Id) -> Option<Stroke> {
    let response = ui.ctx().read_response(panel_id.with("__resize"))?;
    if response.dragged() {
        Some(ui.style().visuals.widgets.active.fg_stroke)
    } else if response.hovered() {
        Some(ui.style().visuals.widgets.hovered.fg_stroke)
    } else {
        None
    }
}

pub(super) fn segments(center: Pos2, horizontal: bool) -> [(Pos2, Pos2); 2] {
    [-2.0, 2.0].map(|offset| {
        if horizontal {
            (
                center + egui::vec2(-9.0, offset),
                center + egui::vec2(9.0, offset),
            )
        } else {
            (
                center + egui::vec2(offset, -9.0),
                center + egui::vec2(offset, 9.0),
            )
        }
    })
}
/// egui reserves a separator-width outer margin, then draws its rule halfway
/// through that margin. Use the rule's center, rather than the panel's edge.
pub(super) fn panel_center(ui: &egui::Ui, rect: Rect, horizontal: bool) -> Pos2 {
    let stroke = ui.visuals().widgets.noninteractive.bg_stroke.width;
    let inset = stroke.round() - 0.5 * stroke;
    if horizontal {
        egui::pos2(rect.center().x, rect.top() + inset)
    } else {
        egui::pos2(rect.right() - inset, rect.center().y)
    }
}
fn grip_bounds(center: Pos2, horizontal: bool, span: egui::emath::Rangef) -> Rect {
    let length = (span.max - span.min).clamp(0.0, 24.0);
    Rect::from_center_size(
        center,
        if horizontal {
            egui::vec2(length, 8.0)
        } else {
            egui::vec2(8.0, length)
        },
    )
}
pub(super) fn paint(
    ui: &egui::Ui,
    center: Pos2,
    horizontal: bool,
    span: egui::emath::Rangef,
    highlight: Option<Stroke>,
) {
    let bounds = grip_bounds(center, horizontal, span);
    if !bounds.is_positive() {
        return;
    }
    // Native/content panels may leave only a few pixels along this divider.
    // Keep the transverse mask straddling the rule, but never paint past the
    // allocated divider span or outside the containing panel's clip.
    let mut clip = ui.clip_rect();
    if horizontal {
        clip.min.x = clip.min.x.max(bounds.left());
        clip.max.x = clip.max.x.min(bounds.right());
    } else {
        clip.min.y = clip.min.y.max(bounds.top());
        clip.max.y = clip.max.y.min(bounds.bottom());
    }
    if !clip.is_positive() {
        return;
    }
    let painter = ui
        .ctx()
        .layer_painter(egui::LayerId::new(
            egui::Order::Middle,
            egui::Id::new("divider-grips"),
        ))
        .with_clip_rect(clip);
    painter.rect_filled(bounds, 0.0, ui.visuals().panel_fill);
    // The bars stay dim; only the outline carries the highlight.
    let bars = Stroke::new(1.5, ui.visuals().widgets.noninteractive.bg_stroke.color);
    let length = if horizontal {
        bounds.width()
    } else {
        bounds.height()
    };
    let inset = 3.0_f32.min(length / 4.0);
    for (mut a, mut b) in segments(center, horizontal) {
        if horizontal {
            a.x = bounds.left() + inset;
            b.x = bounds.right() - inset;
        } else {
            a.y = bounds.top() + inset;
            b.y = bounds.bottom() - inset;
        }
        painter.line_segment([a, b], bars);
    }
    if let Some(stroke) = highlight {
        // A highlighted divider must not lose its handle behind the grip's
        // masking fill; outline the handle in the divider's own color.
        painter.rect_stroke(
            bounds,
            2.0,
            Stroke::new(1.5, stroke.color),
            StrokeKind::Inside,
        );
    }
}
/// Native child views must leave their document window's neighboring grips
/// exposed. Detached previews have no such neighbors, even when those panels
/// remain visible in the document window.
pub(super) fn native_preview_grips(
    split_pane: bool,
    explorer_visible: bool,
    bottom_visible: bool,
    popout: bool,
) -> (bool, bool) {
    let main = !popout;
    (
        main && (split_pane || explorer_visible),
        main && bottom_visible,
    )
}

/// A panel's contents may demand more height than the remaining main region.
pub(super) fn divider_rect(panel: Rect, allocated: Rect) -> Rect {
    panel.intersect(allocated)
}

pub(super) fn editor_maximum(
    available: Rect,
    preferred: f32,
    previous: Option<Rect>,
    independent: bool,
) -> f32 {
    if independent
        && let Some(previous) = previous
        && previous.right() < available.right()
    {
        // Keep an existing small preview instead of enlarging its preferred
        // reserve when Explorer disappears. A real viewport shrink still
        // falls back to normal limits if retaining the edge consumes the
        // entire preview, or if the old edge lies outside the viewport.
        preferred
            .max(previous.right() - available.left())
            .min(available.width().max(0.0))
    } else {
        preferred
    }
}

pub(super) fn editor_width(width: f32, explorer_delta: f32, minimum: f32, maximum: f32) -> f32 {
    (width - explorer_delta).clamp(minimum, maximum.max(minimum))
}

/// Reanchor a retained split pane after its neighbors changed while it was
/// visible or hidden. The pane's own last position is the reference, so a
/// Code-only/Preview-only/popout frame cannot erase the required displacement.
pub(super) fn compensated_editor_rect(
    previous: Rect,
    available: Rect,
    minimum: f32,
    maximum: f32,
) -> Option<Rect> {
    let displacement = available.left() - previous.left();
    if displacement.abs() <= 0.01 {
        return None;
    }
    let width = editor_width(previous.width(), displacement, minimum, maximum);
    let mut adjusted = previous;
    adjusted.min.x = available.left();
    adjusted.max.x = available.left() + width;
    Some(adjusted)
}

#[cfg(all(feature = "desktop-ui-tests", target_os = "macos"))]
pub(super) fn observe(
    name: &str,
    response: &egui::Response,
    panel_id: egui::Id,
    rect: Rect,
    horizontal: bool,
    top: bool,
) {
    let Some(mut target) = response.ctx.read_response(panel_id.with("__resize")) else {
        return;
    };
    target.rect = if horizontal && top {
        // The source widget may own the inclusive shared boundary. Approach
        // inside the real resize widget, clear of source and panel content.
        target.interact_rect.intersect(rect)
    } else if horizontal {
        Rect::from_center_size(
            egui::pos2(
                rect.center().x,
                if top { rect.top() } else { rect.bottom() },
            ),
            egui::vec2(rect.width(), 6.0),
        )
    } else {
        Rect::from_center_size(
            egui::pos2(rect.right(), rect.center().y),
            egui::vec2(6.0, rect.height()),
        )
    };
    if target.rect.is_positive() {
        target.interact_rect = target.rect;
        crate::desktop_test::observe(name, &target);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn grips_fit_their_divider_span_even_when_panels_are_nearly_maximized() {
        for horizontal in [false, true] {
            for length in [0.0, 1.0, 5.0, 10.0, 24.0, 100.0] {
                let span = egui::emath::Rangef::new(20.0, 20.0 + length);
                let center = if horizontal {
                    egui::pos2(20.0 + length / 2.0, 50.0)
                } else {
                    egui::pos2(50.0, 20.0 + length / 2.0)
                };
                let bounds = grip_bounds(center, horizontal, span);
                let actual = if horizontal {
                    bounds.x_range()
                } else {
                    bounds.y_range()
                };
                assert!(
                    actual.min >= span.min && actual.max <= span.max,
                    "grip {actual:?} protrudes beyond divider {span:?}"
                );
                assert_eq!(bounds.center(), center);
            }
        }
    }

    #[test]
    fn short_grips_clip_bars_and_highlights_to_the_containing_panel() {
        let context = egui::Context::default();
        for horizontal in [false, true] {
            let mut output = context.run_ui(egui::RawInput::default(), |ui| {
                let enclosing = Rect::from_min_max(egui::pos2(40.0, 40.0), egui::pos2(60.0, 60.0));
                ui.set_clip_rect(enclosing);
                paint(
                    ui,
                    egui::pos2(50.0, 50.0),
                    horizontal,
                    egui::emath::Rangef::new(45.0, 55.0),
                    Some(Stroke::new(1.0, egui::Color32::RED)),
                );
            });
            output.textures_delta.clear();
            assert_eq!(output.shapes.len(), 4);
            for shape in output.shapes {
                let span = if horizontal {
                    shape.clip_rect.x_range()
                } else {
                    shape.clip_rect.y_range()
                };
                assert_eq!(span, egui::emath::Rangef::new(45.0, 55.0));
                match shape.shape {
                    egui::Shape::Rect(rect) => assert!(shape.clip_rect.contains_rect(rect.rect)),
                    egui::Shape::LineSegment { points, .. } => {
                        assert!(
                            points
                                .into_iter()
                                .all(|point| shape.clip_rect.contains(point))
                        );
                    }
                    _ => panic!("unexpected grip shape"),
                }
            }
        }
    }

    #[test]
    fn divider_geometry_uses_visible_allocation_not_oversized_child_contents() {
        let allocated = Rect::from_min_max(egui::pos2(0.0, 40.0), egui::pos2(1200.0, 52.0));
        let oversized = Rect::from_min_max(egui::pos2(0.0, 40.0), egui::pos2(300.0, 104.0));
        let divider = divider_rect(oversized, allocated);
        assert!(allocated.contains_rect(divider));
        assert_eq!(divider.center().y, 46.0);
        assert_eq!(divider.height(), 12.0);
    }

    #[test]
    fn independent_divider_retains_small_preview_space_until_actual_viewport_shrink() {
        let previous = Rect::from_min_max(egui::pos2(300.0, 40.0), egui::pos2(1170.0, 400.0));
        let closed = Rect::from_min_max(egui::pos2(0.0, 40.0), egui::pos2(1200.0, 400.0));
        assert_eq!(editor_maximum(closed, 1104.0, Some(previous), true), 1170.0);
        let reopened = Rect::from_min_max(egui::pos2(300.0, 40.0), egui::pos2(1200.0, 400.0));
        assert_eq!(editor_maximum(reopened, 804.0, Some(previous), true), 870.0);
        assert_eq!(
            editor_maximum(reopened, 804.0, Some(previous), false),
            804.0
        );
        assert_eq!(editor_maximum(reopened, 804.0, None, true), 804.0);
        let shrunk = Rect::from_min_max(egui::pos2(300.0, 40.0), egui::pos2(1100.0, 400.0));
        assert_eq!(editor_maximum(shrunk, 704.0, Some(previous), true), 704.0);
        let before_parent = Rect::from_min_max(egui::pos2(0.0, 40.0), egui::pos2(200.0, 400.0));
        assert_eq!(
            editor_maximum(reopened, 804.0, Some(before_parent), true),
            804.0
        );
    }

    #[test]
    fn independent_divider_reserves_preview_at_exact_edge_viewport_shrink() {
        let previous = Rect::from_min_max(egui::pos2(300.0, 40.0), egui::pos2(900.0, 400.0));
        let available = Rect::from_min_max(egui::pos2(300.0, 40.0), egui::pos2(900.0, 400.0));
        let layout = crate::theme::split_pane_layout(available.width());
        assert_eq!(
            editor_maximum(available, layout.editor_maximum, Some(previous), true),
            layout.editor_maximum,
            "retaining the edge at the new viewport edge hides the entire preview"
        );
    }

    #[test]
    fn independent_egui_panels_keep_a_forty_pixel_preview_through_close_reopen_and_idle() {
        let context = egui::Context::default();
        let explorer_id = egui::Id::new("narrow-preview-explorer");
        let editor_id = egui::Id::new("narrow-preview-editor");
        let screen = Rect::from_min_size(Pos2::ZERO, egui::vec2(1200.0, 300.0));

        // Reuse the same Context so Panel persists its actual size between
        // frames. The last two frames exercise an idle closed layout too.
        for explorer_width in [1120.0, 0.0, 1120.0, 1120.0, 0.0, 0.0] {
            let mut observed = (Rect::NOTHING, Rect::NOTHING);
            context
                .run_ui(
                    egui::RawInput {
                        screen_rect: Some(screen),
                        ..Default::default()
                    },
                    |ui| {
                        if explorer_width > 0.0 {
                            egui::Panel::left(explorer_id)
                                .frame(egui::Frame::NONE)
                                .show_separator_line(false)
                                .exact_size(explorer_width)
                                .show(ui, |ui| {
                                    ui.label("E");
                                });
                        }

                        let available = ui.available_rect_before_wrap();
                        let layout = crate::theme::split_pane_layout(available.width());
                        let previous = egui::PanelState::load(&context, editor_id);
                        let maximum = editor_maximum(
                            available,
                            layout.editor_maximum,
                            previous.map(|state| state.outer_rect),
                            true,
                        );
                        if let Some(mut state) = previous
                            && let Some(adjusted) = compensated_editor_rect(
                                state.outer_rect,
                                available,
                                layout.editor_minimum,
                                maximum,
                            )
                        {
                            state.outer_rect = adjusted;
                            context.data_mut(|data| data.insert_persisted(editor_id, state));
                        }

                        observed.0 = egui::Panel::left(editor_id)
                            .frame(egui::Frame::NONE)
                            .show_separator_line(false)
                            .resizable(true)
                            .default_size(layout.editor_width)
                            .min_size(layout.editor_minimum)
                            .max_size(maximum)
                            .show(ui, |ui| {
                                // Fill the persisted slot without a large
                                // child that can expand the panel itself.
                                ui.set_min_size(ui.available_size());
                                ui.label("x");
                            })
                            .response
                            .rect;
                        observed.1 = egui::CentralPanel::default()
                            .frame(egui::Frame::NONE)
                            .show(ui, |ui| {
                                ui.label("P");
                            })
                            .response
                            .rect;
                    },
                )
                .drop_without_applying_deltas();

            assert_eq!(observed.0.left(), explorer_width);
            assert_eq!(
                observed.0.right(),
                1160.0,
                "Explorer width {explorer_width}"
            );
            assert_eq!(observed.1.left(), observed.0.right());
            assert_eq!(observed.1.right(), screen.right());
            assert_eq!(observed.1.width(), 40.0, "Explorer width {explorer_width}");
            let persisted = egui::PanelState::load(&context, editor_id).unwrap();
            assert_eq!(persisted.outer_rect, observed.0);
        }
    }

    #[test]
    fn independent_preview_edge_survives_explorer_changes_while_split_is_hidden() {
        let context = egui::Context::default();
        let explorer_id = egui::Id::new("hidden-split-explorer");
        let editor_id = egui::Id::new("hidden-split-editor");
        // A hidden split pane can correspond to Code, Preview, or a popout.
        // Closing, reopening and resizing Explorer while hidden must preserve
        // the edge when Split returns. A real viewport shrink may move it.
        for (explorer_width, split, viewport_width, expected_edge) in [
            (300.0, true, 1200.0, 900.0),
            (0.0, false, 1200.0, 900.0),
            (0.0, true, 1200.0, 900.0),
            (400.0, false, 1200.0, 900.0),
            (400.0, true, 1200.0, 900.0),
            (470.0, false, 1200.0, 900.0),
            (470.0, true, 1200.0, 900.0),
            (470.0, false, 880.0, 900.0),
            (470.0, true, 880.0, 784.0),
            (470.0, true, 1200.0, 784.0),
            (0.0, false, 1200.0, 784.0),
            (0.0, true, 1200.0, 784.0),
            (400.0, true, 1200.0, 784.0),
        ] {
            let screen = Rect::from_min_size(Pos2::ZERO, egui::vec2(viewport_width, 300.0));
            let mut observed_edge = None;
            context
                .run_ui(
                    egui::RawInput {
                        screen_rect: Some(screen),
                        ..Default::default()
                    },
                    |ui| {
                        if explorer_width > 0.0 {
                            egui::Panel::left(explorer_id)
                                .frame(egui::Frame::NONE)
                                .show_separator_line(false)
                                .exact_size(explorer_width)
                                .show(ui, |ui| {
                                    ui.label("Explorer");
                                });
                        }
                        let available = ui.available_rect_before_wrap();
                        let layout = crate::theme::split_pane_layout(available.width());
                        let previous = split
                            .then(|| egui::PanelState::load(&context, editor_id))
                            .flatten();
                        let maximum = editor_maximum(
                            available,
                            layout.editor_maximum,
                            previous.map(|state| state.outer_rect),
                            split,
                        );
                        if let Some(mut state) = previous
                            && let Some(adjusted) = compensated_editor_rect(
                                state.outer_rect,
                                available,
                                layout.editor_minimum,
                                maximum,
                            )
                        {
                            state.outer_rect = adjusted;
                            context.data_mut(|data| data.insert_persisted(editor_id, state));
                        }
                        if split {
                            observed_edge = Some(
                                egui::Panel::left(editor_id)
                                    .frame(egui::Frame::NONE)
                                    .show_separator_line(false)
                                    .default_size(600.0)
                                    .min_size(layout.editor_minimum)
                                    .max_size(maximum)
                                    .show(ui, |ui| {
                                        ui.set_min_size(ui.available_size());
                                    })
                                    .response
                                    .rect
                                    .right(),
                            );
                        }
                        egui::CentralPanel::default()
                            .frame(egui::Frame::NONE)
                            .show(ui, |ui| {
                                ui.label("Document");
                            });
                    },
                )
                .drop_without_applying_deltas();
            if let Some(edge) = observed_edge {
                assert_eq!(edge, expected_edge, "Explorer width {explorer_width}");
                assert!(edge < viewport_width, "preview has no remaining width");
            }
        }
    }

    #[test]
    fn independent_divider_preserves_preview_edge_until_a_minimum_is_hit() {
        assert_eq!(
            editor_width(600.0, 70.0, 100.0, 800.0) + 370.0,
            600.0 + 300.0
        );
        assert_eq!(editor_width(120.0, 70.0, 100.0, 800.0), 100.0);
        assert_eq!(editor_width(600.0, -70.0, 100.0, 800.0), 670.0);
    }
    #[test]
    fn independent_divider_stays_fixed_through_resize_hide_and_reopen() {
        let mut explorer = 300.0;
        let mut editor = 600.0;
        for next in [370.0, 0.0, 370.0, 300.0, 0.0, 300.0] {
            let edge = explorer + editor;
            editor = editor_width(editor, next - explorer, 100.0, 1300.0 - next);
            explorer = next;
            assert_eq!(explorer + editor, edge);
        }
    }

    #[test]
    fn grips_are_centered_rotated_versions_of_the_same_geometry() {
        let a = segments(Pos2::ZERO, true);
        let b = segments(Pos2::ZERO, false);
        for ((p, q), (r, s)) in a.into_iter().zip(b) {
            assert_eq!(egui::vec2(p.x, p.y), egui::vec2(r.y, r.x));
            assert_eq!(egui::vec2(q.x, q.y), egui::vec2(s.y, s.x));
        }
    }
    #[test]
    fn panel_grips_straddle_the_separator_not_the_outer_edge() {
        let context = egui::Context::default();
        let mut output = context.run_ui(egui::RawInput::default(), |ui| {
            ui.visuals_mut().widgets.noninteractive.bg_stroke.width = 1.0;
            let rect = Rect::from_min_max(egui::pos2(0.0, 30.0), egui::pos2(200.0, 300.0));
            for (horizontal, expected) in [
                (true, egui::pos2(100.0, 30.5)),
                (false, egui::pos2(199.5, 165.0)),
            ] {
                let center = panel_center(ui, rect, horizontal);
                assert_eq!(center, expected);
                let bars = segments(center, horizontal);
                let midpoint = bars[0]
                    .0
                    .lerp(bars[0].1, 0.5)
                    .lerp(bars[1].0.lerp(bars[1].1, 0.5), 0.5);
                assert_eq!(midpoint, expected);
            }
        });
        output.textures_delta.clear();
    }

    #[test]
    fn highlighted_grips_outline_the_handle_bounds_and_leave_their_bars_dim() {
        let context = egui::Context::default();
        let highlight = Stroke::new(2.0, egui::Color32::RED);
        for highlight in [None, Some(highlight)] {
            let mut output = context.run_ui(egui::RawInput::default(), move |ui| {
                let dim = ui.visuals().widgets.noninteractive.bg_stroke.color;
                ui.ctx()
                    .data_mut(|data| data.insert_temp(egui::Id::new("grip-test-dim"), dim));
                paint(
                    ui,
                    Pos2::new(50.0, 50.0),
                    true,
                    ui.max_rect().x_range(),
                    highlight,
                );
            });
            output.textures_delta.clear();
            let dim: egui::Color32 = context
                .data(|data| data.get_temp(egui::Id::new("grip-test-dim")))
                .unwrap();
            let mut bars = 0;
            let mut outlines = 0;
            for shape in &output.shapes {
                match &shape.shape {
                    egui::Shape::LineSegment { stroke, .. } => {
                        bars += 1;
                        assert_eq!(stroke.color, dim, "bars never carry the highlight");
                    }
                    egui::Shape::Rect(rect) => {
                        if let Some(stroke) = highlight
                            && rect.stroke.color == stroke.color
                        {
                            outlines += 1;
                            assert_eq!(rect.rect.size(), egui::vec2(24.0, 8.0));
                        }
                    }
                    _ => {}
                }
            }
            assert_eq!(bars, 2);
            assert_eq!(
                outlines,
                usize::from(highlight.is_some()),
                "only a highlighted divider outlines its handle"
            );
        }
    }

    #[test]
    fn preview_content_cannot_steal_the_shared_resize_boundary() {
        use egui_kittest::Harness;
        for (salt, horizontal) in [
            ("filesystem", false),
            ("editor", false),
            ("bottom-panel", true),
        ] {
            let mut harness = Harness::builder()
                .with_size(egui::vec2(900.0, 500.0))
                .build_ui_state(
                    move |ui, state: &mut (bool, Rect, Rect, bool, usize)| {
                        let id = crate::child_view::viewport_scoped_id(ui.ctx(), salt);
                        if state.0 {
                            let panel = if horizontal {
                                egui::Panel::bottom(id)
                            } else {
                                egui::Panel::left(id)
                            };
                            state.1 = panel
                                .resizable(true)
                                .default_size(180.0)
                                .min_size(50.0)
                                .show(ui, |ui| {
                                    ui.label("Neighboring panel");
                                    ui.take_available_space();
                                })
                                .response
                                .rect;
                        }
                        egui::CentralPanel::default()
                            .frame(egui::Frame::NONE)
                            .show(ui, |ui| {
                                let rect = ui.available_rect_before_wrap();
                                state.2 = rect;
                                // Fit-width PDF pages fill the central pane and
                                // deliberately retain their full paint rectangle.
                                ui.painter().rect_filled(rect, 0.0, egui::Color32::WHITE);
                                let clip = ui.clip_rect();
                                ui.set_clip_rect(preview_interaction_bounds(ui, clip));
                                let page = ui.interact(
                                    rect,
                                    egui::Id::new("preview-test-page"),
                                    egui::Sense::click_and_drag(),
                                );
                                ui.set_clip_rect(clip);
                                assert_eq!(page.rect, rect, "page focus/paint bounds stay intact");
                                if page.clicked() || page.drag_started() {
                                    page.request_focus();
                                }
                                if page.clicked() {
                                    state.4 += 1;
                                }
                                state.3 = page.has_focus();
                            });
                        let before = ui.ctx().viewport(|viewport| {
                            (
                                viewport
                                    .this_pass
                                    .widgets
                                    .layers()
                                    .map(|(_, widgets)| widgets.len())
                                    .sum::<usize>(),
                                viewport.this_pass.widgets.get(id.with("__resize")).copied(),
                            )
                        });
                        let _ = preview_interaction_bounds(ui, ui.clip_rect());
                        let after = ui.ctx().viewport(|viewport| {
                            (
                                viewport
                                    .this_pass
                                    .widgets
                                    .layers()
                                    .map(|(_, widgets)| widgets.len())
                                    .sum::<usize>(),
                                viewport.this_pass.widgets.get(id.with("__resize")).copied(),
                            )
                        });
                        assert_eq!(
                            before, after,
                            "reserving interaction bounds must retain every existing widget"
                        );
                    },
                    (true, Rect::NOTHING, Rect::NOTHING, false, 0),
                );
            harness.run_steps(3);
            let original = harness.state().1;
            let page = harness.state().2;
            let start = if horizontal {
                egui::pos2(original.center().x, original.top())
            } else {
                egui::pos2(original.right(), original.center().y)
            };
            let delta = if horizontal {
                egui::vec2(0.0, -20.0)
            } else {
                egui::vec2(20.0, 0.0)
            };
            harness.hover_at(start);
            harness.run_steps(2);
            harness.drag_at(start);
            harness.run_steps(2);
            harness.hover_at(start + delta);
            harness.run_steps(3);
            harness.drop_at(start + delta);
            harness.run_steps(3);
            let actual = harness.state().1;
            let change = if horizontal {
                actual.height() - original.height()
            } else {
                actual.width() - original.width()
            };
            assert!(
                (change - 20.0).abs() < 0.1,
                "{salt}: resize was stolen by preview: {original:?} -> {actual:?}"
            );
            assert!(!harness.state().3, "{salt}: divider drag focused preview");
            let painted = harness.state().2;
            if horizontal {
                assert_eq!(painted.left(), page.left());
                assert_eq!(painted.right(), page.right());
                assert!((painted.height() - page.height() + 20.0).abs() < 0.1);
            } else {
                assert_eq!(painted.top(), page.top());
                assert_eq!(painted.bottom(), page.bottom());
                assert!((painted.width() - page.width() + 20.0).abs() < 0.1);
            }
            // The interior remains selectable, and old handles cannot block
            // a page after the corresponding panel becomes hidden.
            harness.hover_at(painted.center());
            for pressed in [true, false] {
                harness.event(egui::Event::PointerButton {
                    pos: painted.center(),
                    button: egui::PointerButton::Primary,
                    pressed,
                    modifiers: egui::Modifiers::NONE,
                });
                harness.run_steps(1);
            }
            harness.run_steps(2);
            assert!(harness.state().3);
            assert_eq!(harness.state().4, 1, "page interior received its click");
            harness.state_mut().0 = false;
            harness.run_steps(3);
            harness.hover_at(start);
            for pressed in [true, false] {
                harness.event(egui::Event::PointerButton {
                    pos: start,
                    button: egui::PointerButton::Primary,
                    pressed,
                    modifiers: egui::Modifiers::NONE,
                });
                harness.run_steps(1);
            }
            harness.run_steps(2);
            assert!(harness.state().3);
            assert_eq!(
                harness.state().4,
                2,
                "hidden handle did not steal the page click"
            );
        }
    }

    #[test]
    fn only_enabled_current_resize_handles_reserve_preview_interaction() {
        for (enabled, resizable) in [(true, true), (false, true), (true, false)] {
            let context = egui::Context::default();
            let mut output = context.run_ui(egui::RawInput::default(), |ui| {
                if !enabled {
                    ui.disable();
                }
                let id = super::super::explorer_panel_id(ui.ctx());
                egui::Panel::left(id)
                    .resizable(resizable)
                    .default_size(180.0)
                    .show(ui, |ui| {
                        ui.take_available_space();
                    });
                let rect = ui.available_rect_before_wrap();
                let hit = preview_interaction_bounds(ui, rect);
                if enabled && resizable {
                    assert!(hit.left() > rect.left(), "actual handle must be excluded");
                    let handle = ui.ctx().viewport(|viewport| {
                        viewport
                            .this_pass
                            .widgets
                            .get(id.with("__resize"))
                            .copied()
                            .unwrap()
                    });
                    assert!(!hit.contains(handle.interact_rect.right_center()));
                } else {
                    assert_eq!(hit, rect, "disabled/fixed panels cannot reserve a gutter");
                }
            });
            output.textures_delta.clear();
        }
    }

    #[test]
    fn reserving_resize_handles_preserves_tab_focus_order() {
        use egui_kittest::Harness;
        let sequence = |reserve: bool, shift: bool| {
            let mut harness = Harness::builder()
                .with_size(egui::vec2(900.0, 500.0))
                .build_ui_state(
                    move |ui, state: &mut (String, String, String, Option<egui::Id>)| {
                        let id = super::super::explorer_panel_id(ui.ctx());
                        egui::Panel::left(id)
                            .resizable(true)
                            .default_size(180.0)
                            .show(ui, |ui| {
                                ui.add(
                                    egui::TextEdit::singleline(&mut state.0)
                                        .id(egui::Id::new("side-field")),
                                );
                                ui.take_available_space();
                            });
                        egui::CentralPanel::default().show(ui, |ui| {
                            ui.add(
                                egui::TextEdit::singleline(&mut state.1)
                                    .id(egui::Id::new("first-field")),
                            );
                            ui.add(
                                egui::TextEdit::singleline(&mut state.2)
                                    .id(egui::Id::new("last-field")),
                            );
                        });
                        if reserve {
                            let _ = preview_interaction_bounds(ui, ui.clip_rect());
                        }
                        state.3 = ui.memory(|memory| memory.focused());
                    },
                    (String::new(), String::new(), String::new(), None),
                );
            harness.run_steps(3);
            let mut sequence = Vec::new();
            for _ in 0..12 {
                harness.key_press_modifiers(
                    egui::Modifiers {
                        shift,
                        ..egui::Modifiers::NONE
                    },
                    egui::Key::Tab,
                );
                harness.run_steps(3);
                sequence.push(harness.state().3);
            }
            sequence
        };
        for shift in [false, true] {
            assert_eq!(
                sequence(true, shift),
                sequence(false, shift),
                "shift={shift}"
            );
        }
    }

    #[test]
    fn panel_resize_states_reach_the_grip_through_hover_and_drag() {
        use egui_kittest::Harness;
        let mut harness = Harness::builder()
            .with_size(egui::vec2(400.0, 300.0))
            .build_ui_state(
                |ui, state: &mut (Option<Stroke>, egui::Color32, egui::Color32, f32)| {
                    let shown = egui::Panel::left("grip-chain")
                        .resizable(true)
                        .default_size(150.0)
                        .show(ui, |ui| {
                            ui.label("side");
                        });
                    state.0 = panel_divider(ui, egui::Id::new("grip-chain"));
                    state.1 = ui.visuals().widgets.hovered.fg_stroke.color;
                    state.2 = ui.visuals().widgets.active.fg_stroke.color;
                    state.3 = shown.response.rect.right();
                },
                (
                    None,
                    egui::Color32::TRANSPARENT,
                    egui::Color32::TRANSPARENT,
                    0.0,
                ),
            );
        harness.run_steps(2);
        assert!(harness.state().0.is_none());
        let edge = harness.state().3;
        harness.hover_at(Pos2::new(edge, 150.0));
        harness.run_steps(2);
        assert_eq!(
            harness.state().0.map(|stroke| stroke.color),
            Some(harness.state().1),
            "hovering the divider reports the hovered stroke"
        );
        harness.drag_at(Pos2::new(edge - 2.0, 150.0));
        harness.run_steps(1);
        harness.hover_at(Pos2::new(edge + 12.0, 150.0));
        harness.run_steps(2);
        assert_eq!(
            harness.state().0.map(|stroke| stroke.color),
            Some(harness.state().2),
            "dragging the divider reports the active stroke"
        );
        harness.drop_at(Pos2::new(edge + 2.0, 150.0));
        harness.hover_at(Pos2::new(10.0, 10.0));
        harness.run_steps(3);
        assert!(harness.state().0.is_none());
    }
}
