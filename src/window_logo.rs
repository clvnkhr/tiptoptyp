//! A session-local identifying color for each native viewport.
use std::time::Instant;

use eframe::egui::{self, Color32, Pos2, Rect, Sense, Stroke, StrokeKind, Vec2};

use crate::{
    child_view::{
        ChildViewHost, ChildViewSpec, POPUP_BLUR_GRACE, popup_focus_should_close,
        viewport_scoped_id,
    },
    screenshot::CaptureController,
    theme,
};

#[derive(Clone, Default)]
struct LogoState {
    color: Option<Color32>,
    open: bool,
    had_focus: bool,
    blur_started: Option<Instant>,
}

pub(crate) fn toggle(context: &egui::Context) {
    let id = viewport_scoped_id(context, "window-logo-color");
    context.data_mut(|data| {
        let state = data.get_temp_mut_or_default::<LogoState>(id);
        state.open = !state.open;
        state.had_focus = false;
        state.blur_started = None;
    });
    context.request_repaint();
}

pub(crate) fn show(ui: &mut egui::Ui, captures: &CaptureController) {
    let context = ui.ctx().clone();
    let owner_id = context.viewport_id();
    let state_id = viewport_scoped_id(&context, "window-logo-color");
    let mut state =
        context.data_mut(|data| data.get_temp::<LogoState>(state_id).unwrap_or_default());
    let response = logo_button(ui, state.color, state.open);
    if response.clicked() {
        state.open = !state.open;
        state.had_focus = false;
        state.blur_started = None;
    } else if context.input(|input| {
        input.key_pressed(egui::Key::Escape)
            || (input.pointer.any_pressed()
                && input
                    .pointer
                    .interact_pos()
                    .is_some_and(|position| !response.rect.contains(position)))
    }) {
        state.open = false;
    }
    if state.open
        && let Some(owner) = context.input(|input| input.viewport().inner_rect)
    {
        let appearance = context.theme();
        let style = context.style_of(appearance);
        let frame = theme::menu_card_frame(&style);
        let row = style.spacing.interact_size.y.max(theme::TYPE.content);
        let width = 300.0_f32.max(row * 14.0);
        let size = Vec2::new(
            width,
            width + row * 5.0 + style.spacing.item_spacing.y * 6.0,
        ) + frame.total_margin().sum();
        let bounds = picker_bounds(owner, response.rect.left_bottom(), size);
        let spec = ChildViewSpec::dismiss_on_blur(
            "window-logo-picker",
            "Window color",
            bounds.min,
            bounds.size(),
            "logo-color",
        );
        let previous_color = state.color;
        let mut done = false;
        ChildViewHost::show(&context, captures, spec, appearance, &style, |ui, input| {
            let now = Instant::now();
            done |= input.close_requested || input.escape_pressed;
            state.open &= !input.close_requested
                && !input.escape_pressed
                && !popup_focus_should_close(
                    &mut state.had_focus,
                    &mut state.blur_started,
                    input.focused,
                    now,
                );
            if let Some(started) = state.blur_started {
                let elapsed = now.saturating_duration_since(started);
                if elapsed < POPUP_BLUR_GRACE {
                    ui.ctx().request_repaint_after(POPUP_BLUR_GRACE - elapsed);
                }
            }
            egui::CentralPanel::default().frame(frame).show(ui, |ui| {
                egui::ScrollArea::both()
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        done |= picker_contents(ui, &mut state.color, width);
                    });
            });
        });
        if state.color != previous_color {
            context.request_repaint_of(owner_id);
        }
        if done {
            state.open = false;
            crate::window_host::focus(
                &context,
                owner_id,
                crate::window_host::FocusCause::ReturnFromChild,
            );
        }
    }
    if !state.open {
        ChildViewHost::close(&context, "window-logo-picker");
    }
    context.data_mut(|data| data.insert_temp(state_id, state));
}

fn picker_contents(ui: &mut egui::Ui, color: &mut Option<Color32>, width: f32) -> bool {
    ui.set_width(width);
    ui.spacing_mut().slider_width = width;
    ui.label(egui::RichText::new("Window color").strong());
    let mut edited = color.unwrap_or(ui.visuals().panel_fill);
    if egui::color_picker::color_picker_color32(ui, &mut edited, egui::color_picker::Alpha::Opaque)
    {
        *color = Some(edited);
    }
    let mut done = false;
    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
        done =
            crate::app::icons::icon_button(ui, crate::app::icons::UiIcon::Check, "Done").clicked();
        if ui
            .add_enabled(color.is_some(), egui::Button::new("Reset"))
            .clicked()
        {
            *color = None;
        }
    });
    done
}

fn picker_bounds(owner: Rect, anchor: Pos2, desired: Vec2) -> Rect {
    let size = desired.min(owner.size()).max(Vec2::splat(1.0));
    let local = Pos2::new(
        anchor.x.clamp(0.0, (owner.width() - size.x).max(0.0)),
        anchor.y.clamp(0.0, (owner.height() - size.y).max(0.0)),
    );
    Rect::from_min_size(owner.min + local.to_vec2(), size)
}

fn contrast_text(background: Color32) -> Color32 {
    let rgb = egui::Rgba::from(background);
    let luminance = 0.2126 * rgb.r() + 0.7152 * rgb.g() + 0.0722 * rgb.b();
    if luminance > 0.179 {
        Color32::BLACK
    } else {
        Color32::WHITE
    }
}

fn logo_button(ui: &mut egui::Ui, color: Option<Color32>, open: bool) -> egui::Response {
    let mut job = egui::text::LayoutJob::default();
    let foreground = color.map_or(ui.visuals().text_color(), contrast_text);
    for text_color in [
        foreground,
        foreground,
        color.map_or(theme::palette(ui.ctx()).accent, contrast_text),
    ] {
        job.append(
            "t",
            0.0,
            egui::TextFormat {
                font_id: egui::FontId::proportional(theme::TYPE.content),
                color: text_color,
                ..Default::default()
            },
        );
    }
    let galley = ui.fonts_mut(|fonts| fonts.layout_job(job));
    let size = galley.size() + Vec2::new(8.0, 4.0);
    let (rect, response) = ui.allocate_exact_size(size, Sense::click());
    response.widget_info(|| {
        egui::WidgetInfo::labeled(egui::WidgetType::Button, response.enabled(), "Window color")
    });
    let fill = color.unwrap_or_else(|| {
        if response.hovered() || open {
            ui.visuals().widgets.hovered.weak_bg_fill
        } else {
            Color32::TRANSPARENT
        }
    });
    ui.painter().rect_filled(rect, theme::RADIUS.chip, fill);
    if response.has_focus() {
        ui.painter().rect_stroke(
            rect,
            theme::RADIUS.chip,
            Stroke::new(1.0, foreground),
            StrokeKind::Inside,
        );
    }
    ui.painter()
        .galley(rect.center() - galley.size() / 2.0, galley, foreground);
    response.on_hover_cursor(egui::CursorIcon::PointingHand)
}

pub(crate) fn snapshot_fixture(context: &egui::Context) {
    let key = viewport_scoped_id(context, "window-logo-color");
    context.data_mut(|data| {
        data.insert_temp(
            key,
            LogoState {
                color: Some(Color32::from_rgb(68, 150, 130)),
                open: true,
                ..Default::default()
            },
        )
    });
}

pub(crate) fn clear_snapshot(context: &egui::Context) {
    let key = viewport_scoped_id(context, "window-logo-color");
    context.data_mut(|data| data.remove::<LogoState>(key));
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui_kittest::{Harness, kittest::Queryable as _};

    #[test]
    fn logo_is_clickable_and_keeps_its_size_when_colored() {
        let mut harness = Harness::builder().build_ui_state(
            |ui, colored| {
                if logo_button(ui, (*colored).then_some(Color32::YELLOW), false).clicked() {
                    *colored = !*colored;
                }
            },
            false,
        );
        harness.run();
        let size = harness.get_by_label("Window color").rect().size();
        harness.get_by_label("Window color").click();
        harness.run();
        assert!(*harness.state());
        assert_eq!(harness.get_by_label("Window color").rect().size(), size);
        assert_eq!(contrast_text(Color32::YELLOW), Color32::BLACK);
        assert_eq!(contrast_text(Color32::BLACK), Color32::WHITE);
    }

    #[test]
    fn picker_reset_and_done_are_independent_actions() {
        let mut harness = Harness::builder()
            .with_size(Vec2::new(400.0, 600.0))
            .build_ui_state(
                |ui, state: &mut (Option<Color32>, bool)| {
                    state.1 = picker_contents(ui, &mut state.0, 320.0);
                },
                (Some(Color32::GREEN), false),
            );
        harness.run();
        harness.get_by_label("Reset").click();
        harness.run();
        assert_eq!(*harness.state(), (None, false));
        harness.get_by_label("Done").click();
        harness.run_steps(1);
        assert!(harness.state().1);
    }

    #[test]
    fn each_native_window_keeps_its_own_color_and_picker_state() {
        let context = egui::Context::default();
        let other = egui::ViewportId::from_hash_of("second-document");
        for (step, owner) in [egui::ViewportId::ROOT, other, egui::ViewportId::ROOT]
            .into_iter()
            .enumerate()
        {
            let mut input = egui::RawInput {
                viewport_id: owner,
                ..Default::default()
            };
            input.viewports.entry(owner).or_default();
            context
                .run_ui(input, |ui| {
                    let key = viewport_scoped_id(ui.ctx(), "window-logo-color");
                    if owner == egui::ViewportId::ROOT {
                        if step == 0 {
                            snapshot_fixture(ui.ctx());
                        }
                        let state = ui
                            .ctx()
                            .data_mut(|data| data.get_temp::<LogoState>(key))
                            .unwrap();
                        assert!(state.open);
                        assert_eq!(state.color, Some(Color32::from_rgb(68, 150, 130)));
                    } else {
                        assert!(
                            ui.ctx()
                                .data_mut(|data| data.get_temp::<LogoState>(key))
                                .is_none()
                        );
                        clear_snapshot(ui.ctx());
                    }
                })
                .drop_without_applying_deltas();
        }
    }

    #[test]
    fn picker_placement_stays_within_its_owner_at_any_screen_position() {
        for origin in [Pos2::ZERO, Pos2::new(-800.0, 50.0), Pos2::new(200.0, 300.0)] {
            for size in [Vec2::new(400.0, 300.0), Vec2::new(1000.0, 800.0)] {
                let owner = Rect::from_min_size(origin, size);
                let picker = picker_bounds(
                    owner,
                    Pos2::new(size.x - 10.0, 24.0),
                    Vec2::new(360.0, 500.0),
                );
                assert!(owner.contains_rect(picker));
            }
        }
    }
}
