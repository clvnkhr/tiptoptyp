//! Shared popup sizing and scroll bounds; no document or native-view ownership.
use super::{METRICS, STATUS_LOG_ROW_HEIGHT};
use crate::{
    native_menu::{CommandMenu, command_specs},
    theme,
};
use eframe::egui::{self, Pos2, Vec2};

pub(super) fn clamp_popup_anchor(anchor: Pos2, popup_size: Vec2, viewport_size: Vec2) -> Pos2 {
    let margin = METRICS.popup.viewport_edge;
    let max_x = (viewport_size.x - popup_size.x - margin).max(margin);
    let max_y = (viewport_size.y - popup_size.y - margin).max(margin);
    Pos2::new(anchor.x.clamp(margin, max_x), anchor.y.clamp(margin, max_y))
}

pub(super) fn app_popup_scroll_id(generation: u64) -> egui::Id {
    egui::Id::new(("app-popup-scroll", generation))
}

pub(super) fn show_popup_contents<R>(
    ui: &mut egui::Ui,
    size: Vec2,
    generation: u64,
    body: impl FnOnce(&mut egui::Ui) -> R,
) -> egui::scroll_area::ScrollAreaOutput<R> {
    // Areas remember their last size. Explicitly update both dimensions so
    // opening a taller menu cannot inherit the previous menu's scroll bounds.
    ui.set_width(size.x);
    ui.set_height(size.y);
    egui::ScrollArea::vertical()
        .id_salt(app_popup_scroll_id(generation))
        .max_height(size.y)
        .show(ui, body)
}

pub(super) fn clamp_popup_above_anchor(
    anchor: Pos2,
    popup_size: Vec2,
    viewport_size: Vec2,
) -> Pos2 {
    clamp_popup_anchor(
        Pos2::new(anchor.x, anchor.y - popup_size.y),
        popup_size,
        viewport_size,
    )
}

pub(super) fn status_log_popup_size(entry_count: usize) -> Vec2 {
    let content_height = 44.0 + entry_count.min(9) as f32 * STATUS_LOG_ROW_HEIGHT;
    Vec2::new(
        METRICS.menu.status_log_size.x,
        content_height.clamp(92.0, METRICS.menu.status_log_size.y),
    )
}

pub(super) fn editor_context_menu_size(
    has_link: bool,
    can_edit_table: bool,
    can_format: bool,
    style: &egui::Style,
) -> Vec2 {
    let extra = usize::from(has_link) + usize::from(can_edit_table) + usize::from(can_format);
    menu_popup_size(METRICS.menu.editor_width, 8 + extra, 1 + extra, style)
}

pub(super) fn command_popup_size(menu: CommandMenu, style: &egui::Style) -> Vec2 {
    let specs = command_specs(menu).collect::<Vec<_>>();
    let separators = specs
        .windows(2)
        .filter(|pair| pair[0].section != pair[1].section)
        .count();
    menu_popup_size(330.0, specs.len(), separators, style)
}

pub(super) fn menu_popup_size(
    width: f32,
    rows: usize,
    separators: usize,
    style: &egui::Style,
) -> Vec2 {
    let spacing = theme::SPACE.small;
    let rows = rows as f32 * (METRICS.menu.row_height + spacing);
    let separators = separators as f32 * (theme::SPACE.control + spacing);
    Vec2::new(
        width,
        rows + separators + theme::menu_card_frame(style).total_margin().sum().y + 2.0,
    )
}

pub(super) fn workspace_context_menu_size(is_file: bool, style: &egui::Style) -> Vec2 {
    // A file has three copy actions where a directory has one.
    menu_popup_size(
        METRICS.menu.workspace_width,
        if is_file { 9 } else { 7 },
        1,
        style,
    )
}
