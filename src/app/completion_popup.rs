//! Read-only completion popup. The owner applies actions; font rows paint only their names.
use super::icons::{UiIcon, paint_ui_icon};
use crate::{
    child_view::viewport_scoped_id, font_catalog::FontCatalog, lsp::protocol::CompletionOrigin,
    theme, tinymist::CompletionItem,
};
use eframe::egui::{self, Pos2, Rect, RichText, Vec2};
use std::borrow::Cow;
const COMPLETION_POPUP_WIDTH: f32 = 300.0;
const COMPLETION_POPUP_MAX_HEIGHT: f32 = 180.0;
const COMPLETION_ROW_HEIGHT: f32 = 20.0;

fn paint_origin(ui: &egui::Ui, rect: Rect, origin: CompletionOrigin) {
    let color = ui.visuals().weak_text_color();
    match origin {
        CompletionOrigin::BuiltIn => {
            ui.painter().text(
                rect.center(),
                egui::Align2::CENTER_CENTER,
                "<>",
                egui::FontId::monospace(10.0),
                color,
            );
        }
        CompletionOrigin::Font => {}
        CompletionOrigin::LanguageServer | CompletionOrigin::Snippet => {
            paint_ui_icon(
                ui.painter(),
                rect,
                if origin == CompletionOrigin::Snippet {
                    UiIcon::Document
                } else {
                    UiIcon::Link
                },
                color,
            );
        }
    }
}

#[derive(Clone, Copy)]
pub(super) struct CompletionPopupInput<'a> {
    pub items: &'a [CompletionItem],
    pub source: &'a str,
    pub source_cursor: usize,
    pub selected: usize,
    pub anchor: Rect,
    pub fonts: &'a FontCatalog,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum CompletionAction {
    Accept(usize),
    Dismiss,
}

#[derive(Default)]
pub(super) struct CompletionPopupOutput {
    pub action: Option<CompletionAction>,
    pub font_names_pending: bool,
}

pub(super) fn show(
    context: &egui::Context,
    viewport: Rect,
    input: CompletionPopupInput<'_>,
) -> CompletionPopupOutput {
    let CompletionPopupInput {
        items,
        selected,
        anchor,
        fonts,
        ..
    } = input;
    if items.is_empty() || !viewport.is_positive() {
        return CompletionPopupOutput::default();
    }
    let selected = selected.min(items.len() - 1);
    let popup_width = COMPLETION_POPUP_WIDTH.min((viewport.width() - 8.0).max(1.0));
    let frame = theme::popup_card_frame(&context.style_of(context.theme())).inner_margin(4);
    let margin = frame.total_margin().sum();
    let inner_width = (popup_width - margin.x).max(1.0);
    let list_height = (items.len() as f32 * (COMPLETION_ROW_HEIGHT + 2.0) - 2.0)
        .clamp(COMPLETION_ROW_HEIGHT, COMPLETION_POPUP_MAX_HEIGHT);
    let desired_size = Vec2::new(popup_width, list_height + margin.y);
    let position = completion_popup_position(anchor, desired_size, viewport);
    let mut clicked = None;
    let mut font_names_pending = false;
    let selection_id = viewport_scoped_id(context, "completion-last-selection");
    let frame_nr = context.cumulative_frame_nr();
    let scroll_selection = context.data_mut(|data| {
        let previous = data.get_temp::<(usize, u64)>(selection_id);
        data.insert_temp(selection_id, (selected, frame_nr));
        previous.is_none_or(|(index, painted)| index != selected || frame_nr > painted + 1)
    });

    let popup = egui::Area::new(viewport_scoped_id(context, "editor-completion-popup"))
        .order(egui::Order::Foreground)
        .fixed_pos(position)
        .constrain_to(viewport)
        .show(context, |ui| {
            frame.show(ui, |ui| {
                ui.spacing_mut().item_spacing.y = 2.0;
                ui.spacing_mut().interact_size.y = COMPLETION_ROW_HEIGHT;
                ui.spacing_mut().button_padding.y = 0.0;
                ui.set_min_width(inner_width);
                ui.set_max_width(inner_width);
                ui.set_height(list_height);
                egui::ScrollArea::vertical()
                    .id_salt("editor-completion-items")
                    .max_height(list_height)
                    .min_scrolled_height(0.0)
                    .auto_shrink([false, false])
                    .show_rows(ui, COMPLETION_ROW_HEIGHT, items.len(), |ui, visible| {
                        if scroll_selection {
                            let top = ui.max_rect().top()
                                + (selected as f32 - visible.start as f32)
                                    * (COMPLETION_ROW_HEIGHT + ui.spacing().item_spacing.y);
                            ui.scroll_to_rect(
                                Rect::from_min_size(
                                    Pos2::new(ui.max_rect().left(), top),
                                    Vec2::new(inner_width, COMPLETION_ROW_HEIGHT),
                                ),
                                None,
                            );
                        }
                        for index in visible {
                            let item = &items[index];
                            let font = item.origin == CompletionOrigin::Font;
                            let label = if font {
                                Cow::Borrowed(item.label.as_str())
                            } else {
                                Cow::Owned(crate::completion::display_label(
                                    item,
                                    input.source,
                                    input.source_cursor,
                                ))
                            };
                            let accessible_label = if font {
                                Cow::Borrowed(label.as_ref())
                            } else {
                                Cow::Owned(format!("{label} · {}", item.origin.label()))
                            };
                            let response = ui.add_sized(
                                [ui.available_width(), COMPLETION_ROW_HEIGHT],
                                egui::Button::selectable(
                                    index == selected,
                                    RichText::new(if font { "" } else { &label })
                                        .monospace()
                                        .size(theme::TYPE.supporting),
                                )
                                .right_text(
                                    RichText::new(if font { "" } else { "    " })
                                        .monospace()
                                        .size(theme::TYPE.supporting),
                                )
                                .truncate(),
                            );
                            let icon_rect = Rect::from_center_size(
                                Pos2::new(response.rect.right() - 11.0, response.rect.center().y),
                                Vec2::splat(12.0),
                            );
                            if font {
                                let rect = response
                                    .rect
                                    .shrink2(Vec2::new(ui.spacing().button_padding.x, 0.0));
                                if let Some(family) = fonts
                                    .families()
                                    .iter()
                                    .find(|family| family.name == label.as_ref())
                                {
                                    font_names_pending |= !crate::font_preview::paint_name(
                                        ui,
                                        rect,
                                        index,
                                        family,
                                        theme::TYPE.supporting,
                                    );
                                } else {
                                    ui.painter().with_clip_rect(rect).text(
                                        rect.left_center(),
                                        egui::Align2::LEFT_CENTER,
                                        &label,
                                        egui::FontId::proportional(theme::TYPE.supporting),
                                        ui.visuals().text_color(),
                                    );
                                }
                            } else {
                                paint_origin(ui, icon_rect, item.origin);
                            }
                            response.widget_info(|| {
                                egui::WidgetInfo::labeled(
                                    egui::WidgetType::Button,
                                    true,
                                    accessible_label.as_ref(),
                                )
                            });
                            #[cfg(all(feature = "desktop-ui-tests", target_os = "macos"))]
                            if ui.is_rect_visible(response.rect) {
                                crate::desktop_test::observe(
                                    &format!("completion.item.{index}"),
                                    &response,
                                );
                            }
                            if response.clicked() {
                                clicked = Some(index);
                            }
                        }
                    });
            });
        });

    if let Some(index) = clicked {
        return CompletionPopupOutput {
            action: Some(CompletionAction::Accept(index)),
            font_names_pending,
        };
    }
    let clicked_outside = context.input(|input| {
        input.pointer.any_pressed()
            && input
                .pointer
                .interact_pos()
                .is_some_and(|pointer| !popup.response.rect.contains(pointer))
    });
    CompletionPopupOutput {
        action: clicked_outside.then_some(CompletionAction::Dismiss),
        font_names_pending,
    }
}

pub(super) fn completion_popup_position(anchor: Rect, desired_size: Vec2, viewport: Rect) -> Pos2 {
    let edge = 4.0;
    let gap = theme::SPACE.tight;
    let min_x = viewport.left() + edge;
    let max_x = (viewport.right() - edge - desired_size.x).max(min_x);
    let x = anchor.left().clamp(min_x, max_x);
    let below = anchor.bottom() + gap;
    let above = anchor.top() - gap - desired_size.y;
    let preferred_y =
        if below + desired_size.y <= viewport.bottom() - edge || above < viewport.top() + edge {
            below
        } else {
            above
        };
    let min_y = viewport.top() + edge;
    let max_y = (viewport.bottom() - edge - desired_size.y).max(min_y);
    Pos2::new(x, preferred_y.clamp(min_y, max_y))
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui_kittest::{Harness, kittest::Queryable as _};

    #[test]
    fn semantic_selection_acceptance_and_outside_dismissal_emit_actions() {
        let items: Vec<_> = [
            ("alpha", CompletionOrigin::LanguageServer),
            ("beta", CompletionOrigin::Snippet),
            ("gamma", CompletionOrigin::BuiltIn),
            ("delta", CompletionOrigin::Font),
        ]
        .into_iter()
        .map(|(label, origin)| CompletionItem {
            origin,
            label: label.into(),
            insert_text: label.into(),
            detail: None,
            documentation: Some("Explanatory documentation must stay hidden".into()),
            insert_text_is_snippet: false,
            text_edit: None,
            additional_text_edits: Vec::new(),
            sort_text: None,
            filter_text: None,
        })
        .collect();
        let mut harness = Harness::builder()
            .with_size(Vec2::new(700.0, 500.0))
            .build_ui_state(
                move |ui, state: &mut (usize, Vec<CompletionAction>)| {
                    if ui.ctx().cumulative_frame_nr() == 0 {
                        theme::configure_test_fonts(ui.ctx());
                        theme::configure_styles(ui.ctx());
                        return;
                    }
                    if let Some(action) = show(
                        ui.ctx(),
                        ui.max_rect(),
                        CompletionPopupInput {
                            items: &items,
                            source: "",
                            source_cursor: 0,
                            selected: state.0,
                            anchor: Rect::from_min_size(Pos2::new(30.0, 30.0), Vec2::splat(10.0)),
                            fonts: &FontCatalog::default(),
                        },
                    )
                    .action
                    {
                        state.1.push(action);
                    }
                    egui::Area::new(egui::Id::new("outside"))
                        .fixed_pos(Pos2::new(550.0, 400.0))
                        .show(ui.ctx(), |ui| {
                            let _ = ui.button("Outside");
                        });
                },
                (0, Vec::new()),
            );
        harness.run_steps(8);
        let name_rect = harness.get_by_label("delta").rect();
        assert!(
            (name_rect.height() - COMPLETION_ROW_HEIGHT).abs() < 0.01,
            "font row height {} exceeds the virtual list pitch",
            name_rect.height()
        );
        assert!(harness.state().1.is_empty());
        for label in [
            "alpha · Language server",
            "beta · Custom snippet",
            "gamma · Built-in completion",
            "delta",
        ] {
            assert!(
                (harness.get_by_label(label).rect().height() - COMPLETION_ROW_HEIGHT).abs() < 0.01
            );
        }
        harness.get_by_label("beta · Custom snippet").hover();
        harness.run_steps(8);
        assert_eq!(harness.state().0, 0, "hover never moves keyboard selection");
        assert!(harness.state().1.is_empty());
        assert!(
            harness
                .query_by_label("Explanatory documentation must stay hidden")
                .is_none()
        );
        harness.get_by_label("delta").hover();
        harness.run_steps(30);
        assert_eq!(
            harness.get_by_label("delta").rect(),
            name_rect,
            "hover must not resize the font row"
        );
        assert!(harness.query_by_label("Installed font").is_none());
        assert!(harness.query_by_label("Loading font preview…").is_none());
        harness.get_by_label("beta · Custom snippet").click();
        harness.run_steps(8);
        assert!(harness.state().1.contains(&CompletionAction::Accept(1)));
        harness.get_by_label("Outside").click();
        harness.run_steps(8);
        assert!(harness.state().1.contains(&CompletionAction::Dismiss));
    }
    #[test]
    fn virtual_rows_follow_keyboard_selection_in_both_directions() {
        let items: Vec<_> = (0..200)
            .map(|index| CompletionItem {
                origin: CompletionOrigin::Font,
                label: format!("Family {index}"),
                insert_text: String::new(),
                detail: Some("Must not show font details".into()),
                documentation: Some("Must not show font documentation".into()),
                insert_text_is_snippet: false,
                text_edit: None,
                additional_text_edits: Vec::new(),
                sort_text: None,
                filter_text: None,
            })
            .collect();
        let mut harness = Harness::builder()
            .with_size(Vec2::new(700.0, 500.0))
            .build_ui_state(
                move |ui, selected: &mut usize| {
                    if ui.ctx().cumulative_frame_nr() == 0 {
                        theme::configure_test_fonts(ui.ctx());
                        theme::configure_styles(ui.ctx());
                        return;
                    }
                    show(
                        ui.ctx(),
                        ui.max_rect(),
                        CompletionPopupInput {
                            items: &items,
                            source: "",
                            source_cursor: 0,
                            selected: *selected,
                            anchor: Rect::from_min_size(Pos2::new(30.0, 30.0), Vec2::splat(10.0)),
                            fonts: &FontCatalog::default(),
                        },
                    );
                },
                0,
            );
        harness.run_steps(8);
        harness.get_by_label("Family 0");
        assert!(harness.query_by_label("Family 199").is_none());
        *harness.state_mut() = 199;
        harness.run_steps(30);
        harness.get_by_label("Family 199");
        assert!(harness.query_by_label("Family 0").is_none());
        *harness.state_mut() = 0;
        harness.run_steps(30);
        harness.get_by_label("Family 0");
        assert!(
            harness
                .query_by_label("Must not show font details")
                .is_none()
        );
        assert!(
            harness
                .query_by_label("Must not show font documentation")
                .is_none()
        );
    }
}
