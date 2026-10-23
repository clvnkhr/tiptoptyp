//! Compact grouped service indicators with scroll-away filters.
use super::*;

fn controls_contents(ui: &mut egui::Ui, state: &mut State, rows: &[Indicator]) -> bool {
    let before = state.filter.clone();
    let mut refocus = false;
    state.popup_open = false;
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing.x = 4.0;
        let width = (ui.available_width() - 344.0).max(110.0);
        let search = ui.add(
            egui::TextEdit::singleline(&mut state.filter.query)
                .id(search_id(ui.ctx()))
                .hint_text("Search activity")
                .desired_width(width),
        );
        search.widget_info(|| {
            let mut info =
                egui::WidgetInfo::labeled(egui::WidgetType::TextEdit, true, "Search activity");
            info.current_text_value = Some(state.filter.query.clone());
            info
        });
        if std::mem::take(&mut state.focus_search) {
            search.request_focus();
        }
        #[cfg(all(feature = "desktop-ui-tests", target_os = "macos"))]
        crate::desktop_test::observe("activity.search", &search);
        let group = egui::ComboBox::from_id_salt("activity-group-filter")
            .width(116.0)
            .selected_text(state.filter.group.map(Group::label).unwrap_or("All groups"))
            .show_ui(ui, |ui| {
                let all = ui.selectable_value(&mut state.filter.group, None, "All groups");
                refocus |= all.clicked();
                #[cfg(all(feature = "desktop-ui-tests", target_os = "macos"))]
                crate::desktop_test::observe("activity.group-option.All", &all);
                for group in Group::ALL {
                    let count = rows.iter().filter(|row| row.group == group).count();
                    let option = ui.selectable_value(
                        &mut state.filter.group,
                        Some(group),
                        format!("{} ({count})", group.label()),
                    );
                    option.widget_info(|| {
                        egui::WidgetInfo::labeled(
                            egui::WidgetType::SelectableLabel,
                            true,
                            group.label(),
                        )
                    });
                    refocus |= option.clicked();
                    #[cfg(all(feature = "desktop-ui-tests", target_os = "macos"))]
                    crate::desktop_test::observe(
                        &format!("activity.group-option.{group:?}"),
                        &option,
                    );
                }
            });
        state.popup_open |= group.inner.is_some();
        group
            .response
            .widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::ComboBox, true, "Group"));
        #[cfg(all(feature = "desktop-ui-tests", target_os = "macos"))]
        crate::desktop_test::observe("activity.group-filter", &group.response);
        let status = egui::ComboBox::from_id_salt("activity-state-filter")
            .width(92.0)
            .selected_text(
                state
                    .filter
                    .status
                    .map(Status::label)
                    .unwrap_or("All states"),
            )
            .show_ui(ui, |ui| {
                let all = ui.selectable_value(&mut state.filter.status, None, "All states");
                refocus |= all.clicked();
                #[cfg(all(feature = "desktop-ui-tests", target_os = "macos"))]
                crate::desktop_test::observe("activity.state-option.All", &all);
                for status in Status::ALL {
                    let count = rows.iter().filter(|row| status.matches(&row.state)).count();
                    let option = ui.selectable_value(
                        &mut state.filter.status,
                        Some(status),
                        format!("{} ({count})", status.label()),
                    );
                    option.widget_info(|| {
                        egui::WidgetInfo::labeled(
                            egui::WidgetType::SelectableLabel,
                            true,
                            status.label(),
                        )
                    });
                    refocus |= option.clicked();
                    #[cfg(all(feature = "desktop-ui-tests", target_os = "macos"))]
                    crate::desktop_test::observe(
                        &format!("activity.state-option.{status:?}"),
                        &option,
                    );
                }
            });
        state.popup_open |= status.inner.is_some();
        status
            .response
            .widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::ComboBox, true, "State"));
        #[cfg(all(feature = "desktop-ui-tests", target_os = "macos"))]
        crate::desktop_test::observe("activity.state-filter", &status.response);
        let grouped = icons::quiet_square_icon_button(
            ui,
            UiIcon::Menu,
            "Group related activity",
            20.0,
            state.grouped,
        );
        #[cfg(all(feature = "desktop-ui-tests", target_os = "macos"))]
        crate::desktop_test::observe("activity.grouping", &grouped);
        if grouped.clicked() {
            state.grouped = !state.grouped;
            refocus = true;
        }
        let reset = ui
            .add_enabled_ui(state.filter.is_active(), |ui| {
                icons::square_icon_button(ui, UiIcon::Revert, "Reset activity filters", 20.0)
            })
            .inner;
        #[cfg(all(feature = "desktop-ui-tests", target_os = "macos"))]
        crate::desktop_test::observe("activity.reset", &reset);
        if reset.clicked() {
            state.filter = Filter::default();
            refocus = true;
        }
        let count = state.filter.filter_indices(rows).len();
        ui.label(
            RichText::new(format!("{count}/{}", rows.len()))
                .size(theme::TYPE.supporting)
                .weak(),
        );
        if refocus {
            search.request_focus();
        }
    });
    ui.separator();
    before != state.filter || refocus
}

fn controls(ui: &mut egui::Ui, state: &mut State, rows: &[Indicator]) -> bool {
    let start = ui.cursor().top();
    let width = ui.available_width().max(1.0);
    let mut child = ui.new_child(
        egui::UiBuilder::new()
            .id_salt("activity-filter-controls")
            .max_rect(Rect::from_min_size(ui.cursor().min, Vec2::new(width, 0.0)))
            .layout(Layout::top_down(Align::Min)),
    );
    let changed = controls_contents(&mut child, state, rows);
    let (rect, _) =
        ui.allocate_exact_size(Vec2::new(width, child.min_rect().height()), Sense::hover());
    state.controls_rect = rect;
    state.controls_visible = rect.intersect(ui.clip_rect()).is_positive();
    state.controls_height = (ui.cursor().top() - start).max(0.0);
    changed
}

fn chip(ui: &mut egui::Ui, row: &Indicator) {
    let palette = theme::palette(ui.ctx());
    let color = match row.state {
        Activity::Idle => palette.success,
        Activity::Running | Activity::Pending(_) => palette.warning,
        Activity::Failed(_) => palette.error,
        Activity::Inactive(_) => ui.visuals().weak_text_color(),
    };
    let response = ui.add(
        egui::Label::new(
            RichText::new(format!("● {}", row.name))
                .color(color)
                .size(theme::TYPE.supporting),
        )
        .wrap_mode(egui::TextWrapMode::Extend),
    );
    // Preserve existing per-service detail without adding new explanatory UI.
    let detail = match &row.state {
        Activity::Failed(error) => format!("{}\n{}\n{error}", row.role, row.state.label()),
        _ => format!("{}\n{}", row.role, row.state.label()),
    };
    native_hover_text(response, detail);
}

fn list(ui: &mut egui::Ui, state: &mut State, rows: &[Indicator]) {
    let indices = state.filter.filter_indices(rows);
    if indices.is_empty() {
        ui.label(RichText::new("No matching activity").weak());
        return;
    }
    if !state.grouped {
        ui.horizontal_wrapped(|ui| {
            for index in indices {
                chip(ui, &rows[index]);
            }
        });
        return;
    }
    for group in Group::ALL {
        let count = indices
            .iter()
            .filter(|&&index| rows[index].group == group)
            .count();
        if count == 0 {
            continue;
        }
        let open = !state.collapsed.contains(&group);
        ui.horizontal_top(|ui| {
            let label = format!("{} ({count})", group.label());
            let text = RichText::new(format!("{} {label}", if open { "▾" } else { "▸" }))
                .strong()
                .size(theme::TYPE.supporting);
            let header = ui.add_sized(
                Vec2::new(112.0, ui.text_style_height(&egui::TextStyle::Small)),
                egui::Button::new(text).frame(false).small(),
            );
            header
                .widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, &label));
            #[cfg(all(feature = "desktop-ui-tests", target_os = "macos"))]
            crate::desktop_test::observe(&format!("activity.group.{group:?}"), &header);
            if header.clicked() {
                header.request_focus();
                if open {
                    state.collapsed.insert(group);
                } else {
                    state.collapsed.remove(&group);
                }
            }
            if open {
                ui.horizontal_wrapped(|ui| {
                    for &index in &indices {
                        if rows[index].group == group {
                            chip(ui, &rows[index]);
                        }
                    }
                });
            }
        });
    }
}

pub(super) fn show_indicators(ui: &mut egui::Ui, state: &mut State, rows: &[Indicator]) {
    ui.set_min_width(ui.available_width());
    ui.spacing_mut().item_spacing = Vec2::new(5.0, 1.0);
    let search = search_id(ui.ctx());
    let sticky = state.filter.is_active()
        || state.focus_search
        || state.popup_open
        || ui.ctx().memory(|m| m.has_focus(search));
    let height = if state.controls_height > 0.0 {
        state.controls_height
    } else {
        ui.spacing().interact_size.y + 8.0
    };
    let body_height = ui.available_height();
    let scroll_id = ui.make_persistent_id(egui::IdSalt::new("activity-scroll"));
    let saved = egui::scroll_area::State::load(ui.ctx(), scroll_id);
    let offset = initial_offset(
        state.scroll_layout,
        saved.map(|s| s.offset.y),
        sticky,
        height,
    );
    state.sticky_filters = sticky;
    if sticky {
        controls(ui, state, rows);
    }
    let mut scroll = egui::ScrollArea::vertical()
        .id_salt("activity-scroll")
        .min_scrolled_height(0.0)
        .auto_shrink([false, false]);
    if let Some(offset) = offset {
        scroll = scroll.vertical_scroll_offset(offset);
    }
    let mut output = scroll.show(ui, |ui| {
        ui.set_min_width(ui.available_width());
        if !sticky {
            controls(ui, state, rows);
            ui.set_min_height(body_height + state.controls_height);
        }
        list(ui, state, rows);
    });
    debug_assert_eq!(output.id, scroll_id);
    let measured = state.controls_height;
    if !sticky
        && (measured - height).abs() > 0.5
        && (saved.is_none() || output.state.offset.y >= height - 0.5)
    {
        output.state.offset.y = (output.state.offset.y + measured - height).max(0.0);
        output.state.store(ui.ctx(), scroll_id);
        ui.ctx().request_discard("Activity filter toolbar height");
    }
    state.scroll_layout = Some(ScrollLayout {
        sticky,
        height: measured,
    });
    state.scroll_offset = output.state.offset.y;
    state.scroll_rect = output.inner_rect;
    #[cfg(all(feature = "desktop-ui-tests", target_os = "macos"))]
    {
        state.inventory = rows.to_vec();
        crate::desktop_test::observe_scroll(
            "activity",
            ui.ctx(),
            output.inner_rect,
            output.content_size.y,
            output.state.offset.y,
            sticky,
        );
    }
}
