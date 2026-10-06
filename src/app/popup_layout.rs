//! Shared popup sizing and scroll bounds; no document or native-view ownership.
use super::{METRICS, STATUS_LOG_ROW_HEIGHT};
use crate::{
    native_menu::{CommandMenu, command_specs},
    shortcuts::{ShortcutAction, ShortcutBindings},
    theme,
};
use eframe::egui::{self, Pos2, Rect, Vec2};

/// Minimum breathing room between a row caption and its right-aligned text.
const MENU_ROW_GAP: f32 = 16.0;
/// Bounds for measured menu content widths: never a sliver, never overflowing.
const MENU_MIN_CONTENT_WIDTH: f32 = 180.0;
const MENU_MAX_CONTENT_WIDTH: f32 = 400.0;

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

/// One rendered editor menu row for measurement: caption plus right-aligned
/// text, exactly as `menu_row` paints them.
pub(super) struct EditorMenuRow {
    pub label: &'static str,
    pub right: String,
}

/// The row list behind `editor_menu_rows`, without separators. Shared by
/// window sizing and rendering so the popup always stretches to fit its
/// actual content.
pub(super) fn editor_menu_row_specs(
    has_link: bool,
    has_table: bool,
    math: Option<&crate::math_conversion::Target>,
    can_format: bool,
    shortcuts: &ShortcutBindings,
    format: &dyn Fn(egui::KeyboardShortcut) -> String,
) -> Vec<EditorMenuRow> {
    let shortcut = |action: ShortcutAction| shortcuts.egui(action).map(format).unwrap_or_default();
    let mut rows = Vec::new();
    if has_link {
        rows.push(EditorMenuRow {
            label: "Open Link in Browser",
            right: String::new(),
        });
    }
    if let Some(math) = math {
        // Typst offers the single other form directly; TeX keeps the
        // choice flyout behind a parent row.
        if let Some((label, _)) = math.single_alternative() {
            rows.push(EditorMenuRow {
                label,
                right: String::new(),
            });
        } else {
            rows.push(EditorMenuRow {
                label: "Convert to…",
                right: "›".to_owned(),
            });
        }
    }
    if has_table {
        rows.push(EditorMenuRow {
            label: "Edit Table…",
            right: String::new(),
        });
    }
    for (label, action) in [
        ("Undo", ShortcutAction::Undo),
        ("Redo", ShortcutAction::Redo),
        ("Cut", ShortcutAction::Cut),
        ("Copy", ShortcutAction::Copy),
        ("Paste", ShortcutAction::Paste),
        ("Select All", ShortcutAction::SelectAll),
        ("Toggle Comment", ShortcutAction::ToggleComment),
        ("Reveal in Preview", ShortcutAction::SyncPreview),
    ] {
        rows.push(EditorMenuRow {
            label,
            right: shortcut(action),
        });
    }
    if can_format {
        rows.push(EditorMenuRow {
            label: "Format Document",
            right: shortcut(ShortcutAction::Format),
        });
    }
    rows
}

/// Content width one row needs: padding, caption, gap, and right text.
pub(super) fn menu_row_content_width(
    context: &egui::Context,
    style: &egui::Style,
    label: &str,
    right: &str,
) -> f32 {
    let font = egui::TextStyle::Button.resolve(style);
    let pad = style.spacing.button_padding.x;
    let (caption, trailing) = context.fonts_mut(|fonts| {
        let plain = egui::Color32::WHITE;
        (
            fonts
                .layout_no_wrap(label.to_owned(), font.clone(), plain)
                .size()
                .x,
            if right.is_empty() {
                0.0
            } else {
                fonts.layout_no_wrap(right.to_owned(), font, plain).size().x
            },
        )
    });
    let gap = if right.is_empty() { 0.0 } else { MENU_ROW_GAP };
    pad + caption + gap + trailing + pad
}

/// Measured content widths for the editor menu: the parent column stretches
/// to its widest row, and the flyout card stretches to its widest choice.
/// Neither includes card chrome; callers add frame margins.
pub(super) struct EditorMenuWidths {
    pub parent: f32,
    pub submenu_choices: f32,
}

pub(super) fn editor_menu_widths(
    context: &egui::Context,
    style: &egui::Style,
    rows: &[EditorMenuRow],
    choices: &[&str],
) -> EditorMenuWidths {
    let parent = rows
        .iter()
        .map(|row| menu_row_content_width(context, style, row.label, &row.right))
        .fold(0.0, f32::max)
        .ceil()
        .clamp(MENU_MIN_CONTENT_WIDTH, MENU_MAX_CONTENT_WIDTH);
    let submenu_choices = if choices.is_empty() {
        0.0
    } else {
        choices
            .iter()
            .map(|choice| menu_row_content_width(context, style, choice, ""))
            .fold(0.0, f32::max)
            .ceil()
    };
    EditorMenuWidths {
        parent,
        submenu_choices,
    }
}

/// Separators rendered by `editor_menu_rows`: one after the link and table
/// blocks, one after Redo, one before Format. Shared by sizing and rendering.
pub(super) fn editor_menu_separators(has_link: bool, has_table: bool, can_format: bool) -> usize {
    1 + usize::from(has_link) + usize::from(has_table) + usize::from(can_format)
}

/// Exact content height of a row/separator stack: fixed row and separator
/// heights, with gaps between consecutive blocks only, never trailing.
pub(super) fn editor_menu_content_height(rows: usize, separators: usize) -> f32 {
    rows as f32 * METRICS.menu.row_height
        + separators as f32 * theme::SPACE.control
        + rows.saturating_add(separators).saturating_sub(1) as f32 * theme::SPACE.small
}

/// Everything both window sizing and rendering need, measured once from the
/// same row list so the popup stretches to fit its actual content.
pub(super) struct EditorMenuGeometry {
    pub submenu_content: f32,
    /// Parent card column: rows plus the nested card's own chrome. The nested
    /// card drops its outer margin, so this is the full card chrome minus the
    /// window-level outer margin on each side.
    pub column_size: Vec2,
    pub rows_height: f32,
}

pub(super) fn editor_menu_geometry(
    context: &egui::Context,
    style: &egui::Style,
    rows: &[EditorMenuRow],
    choices: &[&str],
    separators: usize,
) -> EditorMenuGeometry {
    let widths = editor_menu_widths(context, style, rows, choices);
    let margins = theme::menu_card_frame(style).total_margin().sum();
    let outer = METRICS.popup.menu_outer_margin as f32;
    let nested = Vec2::new(margins.x - 2.0 * outer, margins.y - 2.0 * outer);
    let rows_height = editor_menu_content_height(rows.len(), separators);
    EditorMenuGeometry {
        submenu_content: widths.submenu_choices,
        column_size: Vec2::new(widths.parent + nested.x, rows_height + nested.y),
        rows_height,
    }
}

/// Parent card column: inset by the outer margin on top/left/bottom so its
/// border aligns with a plain menu card; the right side stays open for the
/// attached flyout.
pub(super) fn editor_parent_column(content: Rect, geometry: &EditorMenuGeometry) -> Rect {
    let outer = METRICS.popup.menu_outer_margin as f32;
    Rect::from_min_size(content.min + egui::vec2(outer, outer), geometry.column_size)
}

pub(super) fn editor_context_menu_size(
    style: &egui::Style,
    geometry: &EditorMenuGeometry,
    has_flyout: bool,
) -> Vec2 {
    let frame = theme::menu_card_frame(style);
    let margins = frame.total_margin().sum();
    let outer = METRICS.popup.menu_outer_margin as f32;
    let width = if has_flyout {
        let flyout = geometry.submenu_content + margins.x;
        geometry.column_size.x - frame.stroke.width + flyout + outer
    } else {
        geometry.column_size.x + 2.0 * outer
    };
    Vec2::new(width, geometry.rows_height + margins.y)
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
