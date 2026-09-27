//! Read-only completion popup. The owner applies actions and supplies optional footer painting.
use crate::{child_view::viewport_scoped_id, theme, tinymist::CompletionItem};
use eframe::egui::{self, Pos2, Rect, RichText, Vec2};
const COMPLETION_POPUP_WIDTH: f32 = 300.0;
const COMPLETION_POPUP_MAX_HEIGHT: f32 = 180.0;
const COMPLETION_ROW_HEIGHT: f32 = 20.0;

#[derive(Clone, Copy)]
pub(super) struct CompletionPopupInput<'a> {
    pub items: &'a [CompletionItem],
    pub source: &'a str,
    pub source_cursor: usize,
    pub selected: usize,
    pub anchor: Rect,
    pub has_footer: bool,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum CompletionAction {
    Accept(usize),
    Dismiss,
}

pub(super) fn show(
    context: &egui::Context,
    viewport: Rect,
    input: CompletionPopupInput<'_>,
    mut footer: impl FnMut(&mut egui::Ui),
) -> Option<CompletionAction> {
    let CompletionPopupInput {
        items,
        selected,
        anchor,
        has_footer,
        ..
    } = input;
    if items.is_empty() || !viewport.is_positive() {
        return None;
    }
    let selected = selected.min(items.len() - 1);
    let popup_width = COMPLETION_POPUP_WIDTH.min((viewport.width() - 8.0).max(1.0));
    let frame = theme::popup_card_frame(&context.style_of(context.theme())).inner_margin(4);
    let margin = frame.total_margin().sum();
    let inner_width = (popup_width - margin.x).max(1.0);
    let list_height = (items.len() as f32 * (COMPLETION_ROW_HEIGHT + 2.0) - 2.0)
        .clamp(COMPLETION_ROW_HEIGHT, COMPLETION_POPUP_MAX_HEIGHT);
    let footer_height = f32::from(has_footer) * 36.0;
    let desired_size = Vec2::new(popup_width, list_height + footer_height + margin.y);
    let position = completion_popup_position(anchor, desired_size, viewport);
    let mut clicked = None;
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
                ui.set_min_width(inner_width);
                ui.set_max_width(inner_width);
                ui.set_height(list_height + footer_height);
                egui::ScrollArea::vertical()
                    .id_salt("editor-completion-items")
                    .max_height(list_height)
                    .min_scrolled_height(0.0)
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        for (index, item) in items.iter().enumerate() {
                            let label = crate::completion::display_label(
                                item,
                                input.source,
                                input.source_cursor,
                            );
                            let mut response = ui.add_sized(
                                [ui.available_width(), COMPLETION_ROW_HEIGHT],
                                egui::Button::selectable(
                                    index == selected,
                                    RichText::new(label)
                                        .monospace()
                                        .size(theme::TYPE.supporting),
                                )
                                .right_text("")
                                .truncate(),
                            );
                            if let Some(documentation) = item.documentation.as_deref() {
                                response = response.on_hover_text(documentation);
                            }
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
                            if index == selected && scroll_selection {
                                response.scroll_to_me(None);
                            }
                        }
                    });
                if has_footer {
                    footer(ui);
                }
            });
        });

    if let Some(index) = clicked {
        return Some(CompletionAction::Accept(index));
    }
    let clicked_outside = context.input(|input| {
        input.pointer.any_pressed()
            && input
                .pointer
                .interact_pos()
                .is_some_and(|pointer| !popup.response.rect.contains(pointer))
    });
    if clicked_outside {
        return Some(CompletionAction::Dismiss);
    }
    None
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
        let items: Vec<_> = ["alpha", "beta"]
            .into_iter()
            .map(|label| CompletionItem {
                label: label.into(),
                insert_text: label.into(),
                detail: None,
                documentation: None,
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
                    if let Some(action) = show(
                        ui.ctx(),
                        ui.max_rect(),
                        CompletionPopupInput {
                            items: &items,
                            source: "",
                            source_cursor: 0,
                            selected: state.0,
                            anchor: Rect::from_min_size(Pos2::new(30.0, 30.0), Vec2::splat(10.0)),
                            has_footer: false,
                        },
                        |_| panic!("absent footer must not run"),
                    ) {
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
        assert!(harness.state().1.is_empty());
        harness.get_by_label("beta ").hover();
        harness.run_steps(8);
        assert_eq!(harness.state().0, 0, "hover never moves keyboard selection");
        assert!(harness.state().1.is_empty());
        harness.get_by_label("beta ").click();
        harness.run_steps(8);
        assert!(harness.state().1.contains(&CompletionAction::Accept(1)));
        harness.get_by_label("Outside").click();
        harness.run_steps(8);
        assert!(harness.state().1.contains(&CompletionAction::Dismiss));
    }
}
