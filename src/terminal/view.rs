use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::Arc,
};

use eframe::egui::{self, Color32, FontId, Pos2, Rect, Sense, Stroke, StrokeKind, Vec2};
use libghostty_vt::{render::CursorVisualStyle, style::Underline};

use super::{
    engine::{Colors, Engine, Grid, GridSize},
    input,
    session::{Command, Session, Status},
};

pub(crate) fn terminal_id(context: &egui::Context) -> egui::Id {
    crate::child_view::viewport_scoped_id(context, "terminal-grid")
}

#[derive(Default)]
pub(crate) struct TerminalPane {
    session: Option<Session>,
    fixture: Option<Engine>,
    fixture_cwd: Option<PathBuf>,
    size: Option<GridSize>,
    colors: Option<Colors>,
    focus_requested: bool,
    focused: bool,
    selection: Option<(usize, usize)>,
    selection_anchor: usize,
    revision: u64,
    grid: Option<Arc<Grid>>,
    error: Option<String>,
    status: Option<Status>,
    wheel_remainder: f32,
    preedit: String,
    glyphs: GlyphCache,
    emoji: super::emoji::EmojiCache,
}

impl TerminalPane {
    pub(crate) fn request_focus(&mut self) {
        self.focus_requested = true;
    }

    pub(crate) fn restart(&mut self) {
        *self = Self {
            focus_requested: true,
            ..Self::default()
        };
    }

    pub(crate) fn set_visible(&mut self, visible: bool) {
        if let Some(session) = &self.session {
            session.set_visible(visible);
        }
        if !visible && self.focused {
            self.send(Command::Focus(false));
            self.focused = false;
            self.preedit.clear();
        }
    }

    pub(crate) fn prepare_fixture(&mut self, output: &[u8], cwd: &Path) {
        if self.fixture.is_some() {
            return;
        }
        let size = GridSize {
            cols: 100,
            rows: 10,
            cell_width: 8,
            cell_height: 16,
        };
        let colors = Colors {
            foreground: Color32::WHITE,
            background: Color32::BLACK,
        };
        match Engine::new(size, colors) {
            Ok(mut engine) => {
                engine.terminal.vt_write(output);
                self.fixture_cwd = Some(cwd.to_owned());
                self.fixture = Some(engine);
            }
            Err(error) => self.error = Some(error.to_string()),
        }
    }

    pub(crate) fn copy(&self, context: &egui::Context) {
        if let (Some(grid), Some(selection)) = (&self.grid, self.selection) {
            context.copy_text(selected_text(grid, selection));
        }
    }

    pub(crate) fn select_all(&mut self) {
        if let Some(grid) = &self.grid {
            self.selection = Some((0, grid.rows.len() * usize::from(grid.cols)));
        }
    }

    fn send(&mut self, command: Command) {
        if let Some(session) = &self.session
            && let Err(error) = session.send(command)
        {
            self.error = Some(error);
        }
    }

    pub(crate) fn sync_snapshot(&mut self) {
        if let Some(snapshot) = self.session.as_ref().map(Session::snapshot) {
            if self.revision != snapshot.revision
                && !same_cells(self.grid.as_deref(), snapshot.grid.as_deref())
            {
                self.selection = None;
            }
            self.revision = snapshot.revision;
            self.grid = snapshot.grid;
            self.status = Some(snapshot.status);
        }
    }

    pub(crate) fn activity(&mut self) -> crate::activity::Activity {
        use crate::activity::Activity;
        self.sync_snapshot();
        if let Some(error) = &self.error {
            return Activity::Failed(error.clone());
        }
        match self.status.as_ref() {
            Some(Status::Starting) => Activity::Running,
            Some(Status::Failed(error)) => Activity::Failed(error.clone()),
            Some(Status::Exited(_)) => Activity::Inactive("Shell exited"),
            Some(Status::Running) => Activity::Idle,
            None => Activity::Inactive("Not started"),
        }
    }
    pub(crate) fn status_text(&self) -> Option<&str> {
        match self.status.as_ref()? {
            Status::Starting => Some("Starting shell…"),
            Status::Exited(message) | Status::Failed(message) => Some(message),
            Status::Running => None,
        }
    }

    pub(crate) fn starting_directory<'a>(&'a self, workspace: &'a Path) -> &'a Path {
        self.session.as_ref().map_or(
            self.fixture_cwd.as_deref().unwrap_or(workspace),
            |session| session.cwd.as_path(),
        )
    }

    pub(crate) fn show(&mut self, ui: &mut egui::Ui, cwd: &Path) {
        let colors = Colors {
            foreground: ui.visuals().text_color(),
            background: ui.visuals().extreme_bg_color,
        };
        if let Some(error) = self.error.clone() {
            ui.horizontal_wrapped(|ui| {
                ui.colored_label(ui.visuals().error_fg_color, error);
                if crate::app::icons::icon_button(ui, crate::app::icons::UiIcon::Close, "Dismiss")
                    .clicked()
                {
                    self.error = None;
                }
            });
        }
        let font = crate::theme::terminal_font();
        let cell = ui.fonts_mut(|fonts| {
            Vec2::new(
                fonts.glyph_width(&font, 'M'),
                fonts.row_height(&font).ceil(),
            )
        });
        let (_, outer) = ui.allocate_space(ui.available_size().max(Vec2::ZERO));
        let rect = outer.shrink(6.0).intersect(ui.clip_rect());
        let size = grid_size(rect.size(), cell, ui.ctx().pixels_per_point());
        let id = terminal_id(ui.ctx());
        let response = ui.interact(outer, id, Sense::click_and_drag());
        response.widget_info(|| {
            egui::WidgetInfo::labeled(egui::WidgetType::TextEdit, true, "Terminal input")
        });
        if self.focus_requested || response.clicked() || response.drag_started() {
            response.request_focus();
            self.focus_requested = false;
        }
        ui.memory_mut(|memory| {
            memory.set_focus_lock_filter(
                id,
                egui::EventFilter {
                    tab: true,
                    horizontal_arrows: true,
                    vertical_arrows: true,
                    escape: true,
                },
            )
        });
        let focused = response.has_focus() && ui.input(|input| input.focused);
        if self.session.is_none() && self.fixture.is_none() && self.error.is_none() {
            match Session::start(ui.ctx(), cwd, size, colors) {
                Ok(session) => {
                    self.session = Some(session);
                    self.size = Some(size);
                    self.colors = Some(colors);
                }
                Err(error) => self.error = Some(error),
            }
        }
        if self.size != Some(size) {
            if let Some(engine) = &mut self.fixture {
                let _ = engine.resize(size);
            }
            if self
                .session
                .as_ref()
                .is_none_or(|session| session.send(Command::Resize(size)).is_ok())
            {
                self.size = Some(size);
            }
        }
        if self.colors != Some(colors) {
            if let Some(engine) = &mut self.fixture {
                let _ = engine.set_colors(colors);
            }
            if self
                .session
                .as_ref()
                .is_none_or(|session| session.send(Command::Colors(colors)).is_ok())
            {
                self.colors = Some(colors);
            }
        }
        if let Some(engine) = &mut self.fixture {
            match engine.snapshot() {
                Ok(grid) => self.grid = Some(Arc::new(grid)),
                Err(error) => self.error = Some(error.to_string()),
            }
        }
        if self.focused != focused {
            self.send(Command::Focus(focused));
            self.focused = focused;
        }
        if focused {
            ui.output_mut(|output| {
                output.ime = Some(egui::output::IMEOutput {
                    rect,
                    cursor_rect: self
                        .grid
                        .as_ref()
                        .and_then(|grid| grid.cursor.as_ref())
                        .map_or(rect, |cursor| {
                            Rect::from_min_size(
                                rect.min + cell * Vec2::new(cursor.col.into(), cursor.row.into()),
                                cell,
                            )
                            .intersect(rect)
                        }),
                    purpose: egui::IMEPurpose::Terminal,
                    should_interrupt_composition: false,
                })
            });
            let events = ui.input_mut(|input| {
                let mut owned = Vec::new();
                input.events.retain(|event| {
                    if matches!(
                        event,
                        egui::Event::Key { .. }
                            | egui::Event::Text(_)
                            | egui::Event::Paste(_)
                            | egui::Event::Copy
                            | egui::Event::Cut
                            | egui::Event::Ime(_)
                    ) {
                        owned.push(event.clone());
                        false
                    } else {
                        true
                    }
                });
                owned
            });
            for event in &events {
                match event {
                    egui::Event::Copy | egui::Event::Cut => self.copy(ui.ctx()),
                    egui::Event::Paste(text) => self.send(Command::Paste(text.clone())),
                    egui::Event::Ime(egui::ImeEvent::Preedit { text, .. }) => {
                        self.preedit = text.clone()
                    }
                    egui::Event::Ime(egui::ImeEvent::Commit(_)) => self.preedit.clear(),
                    _ => {}
                }
            }
            for key in input::keys(&events) {
                self.send(Command::Key(key));
            }
        }
        if response.hovered() {
            let delta = ui.input_mut(|input| {
                let delta = input.smooth_scroll_delta.y;
                input.smooth_scroll_delta = Vec2::ZERO;
                delta
            });
            self.wheel_remainder -= delta / cell.y;
            let lines = self.wheel_remainder.trunc() as isize;
            if lines != 0 {
                self.wheel_remainder -= lines as f32;
                self.send(Command::Scroll(lines));
                self.selection = None;
            }
        }
        if let Some(grid) = &self.grid {
            if (response.drag_started() || response.clicked())
                && let Some(pos) = response.interact_pointer_pos()
            {
                let index = cell_index(pos, rect, cell, grid);
                self.selection_anchor = index;
                self.selection = Some((index, index));
            }
            if response.dragged()
                && let Some(pos) = response.interact_pointer_pos()
            {
                let index = cell_index(pos, rect, cell, grid);
                self.selection = Some(selection_range(self.selection_anchor, index));
            }
            paint(
                ui,
                outer,
                rect,
                cell,
                &font,
                grid,
                self.selection,
                focused,
                &mut self.glyphs,
                &mut self.emoji,
            );
            if focused
                && !self.preedit.is_empty()
                && let Some(cursor) = &grid.cursor
            {
                let pos = rect.min + cell * Vec2::new(cursor.col.into(), cursor.row.into());
                ui.painter().with_clip_rect(rect).text(
                    pos,
                    egui::Align2::LEFT_TOP,
                    &self.preedit,
                    font,
                    colors.foreground,
                );
            }
        } else {
            ui.painter().rect_filled(outer, 0.0, colors.background);
        }
    }
}

fn grid_size(available: Vec2, cell: Vec2, scale: f32) -> GridSize {
    GridSize {
        cols: (available.x / cell.x.max(1.0)).floor().clamp(2.0, 500.0) as u16,
        rows: (available.y / cell.y.max(1.0)).floor().clamp(1.0, 200.0) as u16,
        cell_width: (cell.x * scale).round().clamp(1.0, 512.0) as u16,
        cell_height: (cell.y * scale).round().clamp(1.0, 512.0) as u16,
    }
}

fn cell_index(pos: Pos2, rect: Rect, cell: Vec2, grid: &Grid) -> usize {
    let col = ((pos.x - rect.min.x) / cell.x)
        .floor()
        .clamp(0.0, f32::from(grid.cols - 1)) as usize;
    let row = ((pos.y - rect.min.y) / cell.y)
        .floor()
        .clamp(0.0, grid.rows.len().saturating_sub(1) as f32) as usize;
    row * usize::from(grid.cols) + col
}

fn selected_text(grid: &Grid, (start, end): (usize, usize)) -> String {
    let range = start.min(end)..start.max(end);
    let mut text = String::new();
    let mut first_row = true;
    for (row_index, row) in grid.rows.iter().enumerate() {
        let row_start = row_index * usize::from(grid.cols);
        if range.end <= row_start || range.start >= row_start + row.len() {
            continue;
        }
        if !first_row {
            text.push('\n');
        }
        first_row = false;
        let mut line = String::new();
        for (col, cell) in row.iter().enumerate() {
            if range.contains(&(row_start + col)) && cell.width != 0 {
                if cell.text.is_empty() {
                    line.push(' ');
                } else {
                    line.push_str(&cell.text);
                }
            }
        }
        text.push_str(line.trim_end());
    }
    text
}

fn selection_range(anchor: usize, current: usize) -> (usize, usize) {
    (anchor.min(current), anchor.max(current).saturating_add(1))
}

fn same_cells(previous: Option<&Grid>, current: Option<&Grid>) -> bool {
    match (previous, current) {
        (Some(previous), Some(current)) => {
            previous.cols == current.cols
                && previous.scroll_offset == current.scroll_offset
                && previous.rows.len() == current.rows.len()
                && previous
                    .rows
                    .iter()
                    .zip(&current.rows)
                    .all(|(a, b)| Arc::ptr_eq(a, b))
        }
        (None, None) => true,
        _ => false,
    }
}

const GLYPH_CACHE_CAPACITY: usize = 256;

#[derive(Default)]
struct GlyphCache {
    entries: HashMap<(usize, [u32; 2], bool, bool), FittedGlyph>,
}

struct FittedGlyph {
    // Retain the source so its pointer cannot be reused as another cache key.
    _source: Arc<egui::Galley>,
    galley: Arc<egui::Galley>,
    offset: Vec2,
}

impl GlyphCache {
    fn fit(
        &mut self,
        source: Arc<egui::Galley>,
        size: Vec2,
        bold: bool,
    ) -> (Arc<egui::Galley>, Vec2) {
        self.fit_kind(source, size, bold, false)
    }

    fn fit_kind(
        &mut self,
        source: Arc<egui::Galley>,
        size: Vec2,
        bold: bool,
        icon: bool,
    ) -> (Arc<egui::Galley>, Vec2) {
        let (scale, offset) = if icon {
            icon_placement(source.mesh_bounds, size, bold)
        } else {
            glyph_placement(source.mesh_bounds, size, bold)
        };
        if scale == 1.0 && offset == Vec2::ZERO {
            return (source, offset);
        }
        let key = (
            Arc::as_ptr(&source) as usize,
            [size.x.to_bits(), size.y.to_bits()],
            bold,
            icon,
        );
        if !self.entries.contains_key(&key) {
            if self.entries.len() == GLYPH_CACHE_CAPACITY {
                self.entries.clear();
            }
            let mut shape =
                egui::epaint::TextShape::new(Pos2::ZERO, source.clone(), Color32::WHITE);
            if scale != 1.0 {
                shape.transform(egui::emath::TSTransform::from_scaling(scale));
            }
            self.entries.insert(
                key,
                FittedGlyph {
                    _source: source,
                    galley: shape.galley,
                    offset,
                },
            );
        }
        let fitted = &self.entries[&key];
        (fitted.galley.clone(), fitted.offset)
    }
}

/// Font ink may exceed its advance (emoji, private-use icons and combining
/// marks). Fit that ink into Ghostty's allocation; never invent column widths.
fn glyph_placement(bounds: Rect, size: Vec2, bold: bool) -> (f32, Vec2) {
    if !bounds.is_finite() || !bounds.is_positive() {
        return (1.0, Vec2::ZERO);
    }
    let available = Vec2::new(
        (size.x - if bold { 0.4 } else { 0.0 }).max(0.1),
        size.y.max(0.1),
    );
    let scale = (available.x / bounds.width())
        .min(available.y / bounds.height())
        .min(1.0);
    let scaled = bounds * scale;
    let offset = Vec2::new(
        0.0_f32.clamp(
            -scaled.left(),
            (available.x - scaled.right()).max(-scaled.left()),
        ),
        0.0_f32.clamp(
            -scaled.top(),
            (available.y - scaled.bottom()).max(-scaled.top()),
        ),
    );
    (scale, offset)
}

fn terminal_icon(text: &str) -> bool {
    let mut chars = text.chars();
    chars.next().is_some_and(|c| {
        matches!(c as u32, 0xe000..=0xf8ff | 0xf0000..=0xffffd | 0x100000..=0x10fffd)
            && !matches!(c as u32, 0xe0b0..=0xe0d4)
    }) && chars.next().is_none()
}

// Symbols-only fonts have different ascenders and em sizes from the text face.
// Normalize icon ink to the text's cap-height vicinity, preserving aspect ratio.
fn icon_placement(bounds: Rect, size: Vec2, bold: bool) -> (f32, Vec2) {
    if !bounds.is_finite() || !bounds.is_positive() {
        return (1.0, Vec2::ZERO);
    }
    let width = (size.x - if bold { 0.4 } else { 0.0 }).max(0.1);
    let scale = (width / bounds.width()).min(size.y * 0.75 / bounds.height());
    let scaled = bounds * scale;
    (
        scale,
        Vec2::new(
            (width - scaled.width()) * 0.5 - scaled.left(),
            (size.y - scaled.height()) * 0.5 - scaled.top(),
        ),
    )
}

fn icon_columns(cell: &super::engine::Cell, next: Option<&super::engine::Cell>) -> f32 {
    if cell.width == 1
        && next.is_some_and(|next| {
            next.width == 1 && next.text == " " && next.background == cell.background
        })
    {
        2.0
    } else {
        f32::from(cell.width.max(1))
    }
}

#[allow(clippy::too_many_arguments)]
fn paint(
    ui: &egui::Ui,
    outer: Rect,
    rect: Rect,
    cell_size: Vec2,
    font: &FontId,
    grid: &Grid,
    selection: Option<(usize, usize)>,
    focused: bool,
    glyphs: &mut GlyphCache,
    emoji: &mut super::emoji::EmojiCache,
) {
    ui.painter().rect_filled(outer, 0.0, grid.background);
    let painter = ui.painter().with_clip_rect(rect);
    let selected = selection.map(|(a, b)| a.min(b)..a.max(b));
    // Paint backgrounds before glyphs so a symbol may use the following blank
    // cell without its ink being erased by selection/background painting.
    for (row_index, row) in grid.rows.iter().enumerate() {
        for (col, cell) in row.iter().enumerate() {
            if cell.width == 0 {
                continue;
            }
            let selected = selected
                .as_ref()
                .is_some_and(|range| range.contains(&(row_index * usize::from(grid.cols) + col)));
            let color = if selected {
                ui.visuals().selection.bg_fill
            } else {
                cell.background
            };
            if color != grid.background {
                painter.rect_filled(
                    Rect::from_min_size(
                        rect.min + cell_size * Vec2::new(col as f32, row_index as f32),
                        Vec2::new(cell_size.x * f32::from(cell.width), cell_size.y),
                    ),
                    0.0,
                    color,
                );
            }
        }
    }
    for (row_index, row) in grid.rows.iter().enumerate() {
        for (col, cell) in row.iter().enumerate() {
            if cell.width == 0 {
                continue;
            }
            let pos = rect.min + cell_size * Vec2::new(col as f32, row_index as f32);
            let cell_rect = Rect::from_min_size(
                pos,
                Vec2::new(cell_size.x * f32::from(cell.width.max(1)), cell_size.y),
            );
            if !cell_rect.intersects(rect) {
                continue;
            }
            if cell.style.invisible {
                continue;
            }
            let foreground = if cell.style.faint {
                cell.foreground.gamma_multiply(0.65)
            } else {
                cell.foreground
            };
            if let Some(texture) = emoji.glyph(
                ui.ctx(),
                &cell.text,
                cell.width,
                font.size * ui.ctx().pixels_per_point(),
            ) {
                let size = texture.size_vec2();
                let scale = (cell_rect.width() / size.x).min(cell_rect.height() / size.y);
                let target = Rect::from_center_size(cell_rect.center(), size * scale);
                let tint = if cell.style.faint {
                    Color32::WHITE.gamma_multiply(0.65)
                } else {
                    Color32::WHITE
                };
                painter.with_clip_rect(cell_rect.intersect(rect)).image(
                    texture.id(),
                    target,
                    Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)),
                    tint,
                );
            } else if !cell.text.is_empty() {
                let icon = terminal_icon(&cell.text);
                let ink_rect = if icon {
                    Rect::from_min_size(
                        pos,
                        Vec2::new(
                            cell_size.x * icon_columns(cell, row.get(col + 1)),
                            cell_size.y,
                        ),
                    )
                } else {
                    cell_rect
                };
                let format = egui::TextFormat {
                    font_id: if icon {
                        crate::theme::terminal_icon_font(font.size * 1.5)
                    } else {
                        font.clone()
                    },
                    color: foreground,
                    italics: cell.style.italic,
                    ..Default::default()
                };
                let galley = painter.layout_job(egui::text::LayoutJob::single_section(
                    cell.text.clone(),
                    format,
                ));
                let (galley, offset) = if icon {
                    glyphs.fit_kind(galley, ink_rect.size(), cell.style.bold, true)
                } else {
                    glyphs.fit(galley, ink_rect.size(), cell.style.bold)
                };
                painter.with_clip_rect(ink_rect.intersect(rect)).galley(
                    pos + offset,
                    galley.clone(),
                    foreground,
                );
                if cell.style.bold {
                    painter.with_clip_rect(ink_rect.intersect(rect)).galley(
                        pos + offset + Vec2::new(0.4, 0.0),
                        galley,
                        foreground,
                    );
                }
            }
            if cell.style.underline != Underline::None {
                painter.line_segment(
                    [
                        cell_rect.left_bottom() - Vec2::new(0.0, 1.0),
                        cell_rect.right_bottom() - Vec2::new(0.0, 1.0),
                    ],
                    Stroke::new(1.0, foreground),
                );
            }
            if cell.style.strikethrough {
                painter.line_segment(
                    [cell_rect.left_center(), cell_rect.right_center()],
                    Stroke::new(1.0, foreground),
                );
            }
        }
    }
    if let Some(cursor) = &grid.cursor {
        let cursor_rect = Rect::from_min_size(
            rect.min + cell_size * Vec2::new(cursor.col.into(), cursor.row.into()),
            cell_size,
        );
        if focused {
            match cursor.style {
                CursorVisualStyle::Bar => {
                    painter.rect_filled(
                        Rect::from_min_size(cursor_rect.min, Vec2::new(2.0, cell_size.y)),
                        0.0,
                        cursor.color,
                    );
                }
                CursorVisualStyle::Underline => {
                    painter.line_segment(
                        [cursor_rect.left_bottom(), cursor_rect.right_bottom()],
                        Stroke::new(2.0, cursor.color),
                    );
                }
                _ => {
                    painter.rect_filled(cursor_rect, 0.0, cursor.color.gamma_multiply(0.35));
                }
            }
        } else {
            painter.rect_stroke(
                cursor_rect,
                0.0,
                Stroke::new(1.0, cursor.color),
                StrokeKind::Inside,
            );
        }
    }
    if grid.scroll_total > grid.rows.len() {
        let track = Rect::from_min_max(
            Pos2::new(outer.right() - 4.0, rect.top()),
            Pos2::new(outer.right() - 1.0, rect.bottom()),
        );
        let total = grid.scroll_total as f32;
        let top = track.top() + track.height() * grid.scroll_offset as f32 / total;
        let height = (track.height() * grid.rows.len() as f32 / total)
            .max(3.0)
            .min(track.bottom() - top);
        ui.painter().rect_filled(
            Rect::from_min_size(
                Pos2::new(track.left(), top),
                Vec2::new(track.width(), height),
            ),
            1.0,
            ui.visuals().weak_text_color(),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn icon_ink_is_normalized_without_overwriting_adjacent_text() {
        for bold in [false, true] {
            for width in [8.0, 16.0] {
                let bounds = Rect::from_min_max(Pos2::new(2.0, 4.0), Pos2::new(7.0, 10.0));
                let size = Vec2::new(width, 16.0);
                let (scale, offset) = icon_placement(bounds, size, bold);
                let ink = (bounds * scale).translate(offset);
                assert!(
                    Rect::from_min_size(Pos2::ZERO, size)
                        .expand(0.001)
                        .contains_rect(ink)
                );
                assert!(ink.height() > bounds.height());
                if width == 16.0 {
                    assert!((ink.height() - 12.0).abs() < 0.01);
                }
            }
        }
        assert!(terminal_icon("\u{f03d3}"));
        assert!(terminal_icon("\u{f418}"));
        assert!(!terminal_icon("\u{e0b0}"));
        assert!(!terminal_icon("abc"));
        let cell = super::super::engine::Cell {
            text: "\u{f03d3}".into(),
            width: 1,
            foreground: Color32::WHITE,
            background: Color32::BLACK,
            style: Default::default(),
        };
        let mut next = cell.clone();
        next.text = " ".into();
        assert_eq!(icon_columns(&cell, Some(&next)), 2.0);
        next.text = "x".into();
        assert_eq!(icon_columns(&cell, Some(&next)), 1.0);
        assert_eq!(icon_columns(&cell, None), 1.0);
        next.text = " ".into();
        next.background = Color32::RED;
        assert_eq!(icon_columns(&cell, Some(&next)), 1.0);
    }

    #[test]
    #[ignore = "local font probe: requires installed FiraCode and Symbols Nerd Font Mono"]
    fn terminal_font_probe_installed_nerd_icons_have_ink_without_changing_ascii() {
        for name in [
            "FiraCodeNerdFontMono-Regular.ttf",
            "SymbolsNerdFontMono-Regular.ttf",
        ] {
            let path = PathBuf::from(std::env::var_os("HOME").unwrap())
                .join("Library/Fonts")
                .join(name);
            assert!(
                path.is_file(),
                "install the probe font first: {}",
                path.display()
            );
            let catalog = crate::font_catalog::FontCatalog::single_font_fixture(&path);
            let family = catalog
                .terminal_symbols()
                .expect("Nerd Font recognized from real OpenType family metadata");
            let context = egui::Context::default();
            crate::theme::configure_editor_fonts(
                &context,
                Default::default(),
                Default::default(),
                false,
                400,
                400,
                Some(family),
            );
            context
                .run_ui(Default::default(), |ui| {
                    ui.fonts_mut(|fonts| {
                        let font = crate::theme::terminal_font();
                        let width = fonts.glyph_width(&font, 'M');
                        for c in ' '..='~' {
                            assert!((fonts.glyph_width(&font, c) - width).abs() < 0.01);
                        }
                        let missing =
                            fonts.layout_no_wrap("\u{0378}".into(), font.clone(), Color32::WHITE);
                        for c in ['\u{f418}', '\u{e7a8}', '\u{f03d3}', '\u{f03d7}'] {
                            let galley = fonts.layout_no_wrap(
                                c.into(),
                                crate::theme::terminal_icon_font(font.size * 1.5),
                                Color32::WHITE,
                            );
                            let (scale, offset) = icon_placement(
                                galley.mesh_bounds,
                                Vec2::new(width * 2.0, 16.0),
                                true,
                            );
                            eprintln!(
                                "{name} U+{:X} original={:?} fitted={:?}",
                                c as u32,
                                galley.mesh_bounds,
                                (galley.mesh_bounds * scale).translate(offset)
                            );
                            let uv = galley.rows[0].glyphs[0].uv_rect;
                            assert_ne!(uv, missing.rows[0].glyphs[0].uv_rect, "missing {c}");
                            let atlas = fonts.image();
                            assert!(
                                (uv.min[1]..uv.max[1]).any(|y| (uv.min[0]..uv.max[0])
                                    .any(|x| atlas[(x as usize, y as usize)].a() != 0)),
                                "invisible {c}"
                            );
                        }
                    })
                })
                .drop_without_applying_deltas();
        }
    }

    #[test]
    #[ignore = "opt-in release microbenchmark; reports overhead, no wall-time assertion"]
    fn terminal_font_probe_cached_fit_cost() {
        use std::{hint::black_box, time::Instant};
        let context = egui::Context::default();
        let mut galleys = Vec::new();
        context
            .run_ui(Default::default(), |ui| {
                for c in '!'..='~' {
                    galleys.push(ui.painter().layout_no_wrap(
                        c.into(),
                        FontId::monospace(13.0),
                        Color32::WHITE,
                    ));
                }
                galleys.push(ui.painter().layout_no_wrap(
                    "😀".into(),
                    FontId::monospace(13.0),
                    Color32::WHITE,
                ));
            })
            .drop_without_applying_deltas();
        let size = Vec2::new(7.82666, 16.0);
        let mut cache = GlyphCache::default();
        let cold = Instant::now();
        for galley in &galleys {
            black_box(cache.fit(galley.clone(), size, false));
        }
        let cold_ns = cold.elapsed().as_nanos();
        const ROUNDS: usize = 10_000;
        let baseline = Instant::now();
        for _ in 0..ROUNDS {
            for galley in &galleys {
                black_box((galley.clone(), Vec2::ZERO));
            }
        }
        let baseline_ns = baseline.elapsed().as_nanos();
        let after = Instant::now();
        for _ in 0..ROUNDS {
            for galley in &galleys {
                black_box(cache.fit(galley.clone(), size, false));
            }
        }
        let after_ns = after.elapsed().as_nanos();
        eprintln!(
            "terminal glyph probe: profile={} os={} arch={} font=Hack/NotoEmoji 13pt cell=7.82666x16 ppp=1 color=white fixture={} glyphs rounds={ROUNDS} cold_ns={cold_ns} baseline_ns={baseline_ns} cached_fit_ns={after_ns} entries={}",
            if cfg!(debug_assertions) {
                "debug"
            } else {
                "release"
            },
            std::env::consts::OS,
            std::env::consts::ARCH,
            galleys.len(),
            cache.entries.len()
        );
    }

    #[test]
    fn glyph_ink_fits_ghostty_cells_without_changing_column_allocation() {
        for bounds in [
            Rect::from_min_max(Pos2::new(-2.0, -1.0), Pos2::new(17.0, 18.0)),
            Rect::from_min_max(Pos2::new(0.0, 3.0), Pos2::new(8.0, 15.0)),
        ] {
            for columns in [1.0, 2.0] {
                for bold in [false, true] {
                    let cell = Vec2::new(7.82666 * columns, 16.0);
                    let (scale, offset) = glyph_placement(bounds, cell, bold);
                    let ink = (bounds * scale).translate(offset);
                    let available = Rect::from_min_size(Pos2::ZERO, cell).expand(0.001);
                    assert!(available.contains_rect(ink));
                    assert!(available.contains_rect(
                        ink.translate(Vec2::new(if bold { 0.4 } else { 0.0 }, 0.0))
                    ));
                    assert!(scale > 0.0 && scale <= 1.0);
                }
            }
        }
        assert_eq!(
            glyph_placement(Rect::NOTHING, Vec2::ZERO, false),
            (1.0, Vec2::ZERO)
        );
    }

    #[test]
    fn glyph_cache_reuses_transforms_and_bounds_retained_work() {
        let context = egui::Context::default();
        context
            .run_ui(Default::default(), |ui| {
                let source = ui.painter().layout_no_wrap(
                    "😀".into(),
                    FontId::monospace(13.0),
                    Color32::WHITE,
                );
                let original = source.mesh_bounds;
                let mut cache = GlyphCache::default();
                let size = Vec2::new(7.0, 16.0);
                let first = cache.fit(source.clone(), size, false).0;
                for _ in 0..100 {
                    assert!(Arc::ptr_eq(
                        &first,
                        &cache.fit(source.clone(), size, false).0
                    ));
                }
                assert_eq!(cache.entries.len(), 1);
                let icon = cache.fit_kind(source.clone(), size, false, true).0;
                assert!(!Arc::ptr_eq(&first, &icon));
                assert!(Arc::ptr_eq(
                    &icon,
                    &cache.fit_kind(source.clone(), size, false, true).0
                ));
                let bold = cache.fit(source.clone(), size, true).0;
                assert!(!Arc::ptr_eq(&first, &bold));
                let recolored =
                    ui.painter()
                        .layout_no_wrap("😀".into(), FontId::monospace(13.0), Color32::RED);
                assert!(!Arc::ptr_eq(&first, &cache.fit(recolored, size, false).0));
                for index in 0..GLYPH_CACHE_CAPACITY * 2 {
                    cache.fit(
                        source.clone(),
                        Vec2::new(1.0 + index as f32 / 100.0, 16.0),
                        false,
                    );
                    assert!(cache.entries.len() <= GLYPH_CACHE_CAPACITY);
                }
                assert_eq!(
                    source.mesh_bounds, original,
                    "shared font layout must remain immutable"
                );
            })
            .drop_without_applying_deltas();
    }

    #[test]
    fn terminal_ascii_keeps_one_cell_after_application_font_configuration() {
        let context = egui::Context::default();
        crate::theme::configure_editor_fonts(
            &context,
            Default::default(),
            Default::default(),
            false,
            400,
            400,
            None,
        );
        for scale in [1.0, 1.25, 1.5, 2.0] {
            let mut input = egui::RawInput::default();
            input
                .viewports
                .get_mut(&egui::ViewportId::ROOT)
                .unwrap()
                .native_pixels_per_point = Some(scale);
            context
                .run_ui(input, |ui| {
                    let font = crate::theme::terminal_font();
                    ui.fonts_mut(|fonts| {
                        let width = fonts.glyph_width(&font, 'M');
                        let missing = fonts.layout_no_wrap("\u{0378}".into(), font.clone(), Color32::WHITE);
                        for text in ["😀", "🚀", "e\u{301}"] {
                            let galley = fonts.layout_no_wrap(text.into(), font.clone(), Color32::WHITE);
                            let uv = galley.rows[0].glyphs[0].uv_rect;
                            assert_ne!(uv, missing.rows[0].glyphs[0].uv_rect, "missing {text}");
                            let atlas = fonts.image();
                            assert!((uv.min[1]..uv.max[1]).any(|y| (uv.min[0]..uv.max[0]).any(|x| atlas[(x as usize, y as usize)].a() != 0)), "{text} has no visible ink at {scale}x");
                        }
                        for character in ' '..='~' {
                            let advance = fonts.glyph_width(&font, character);
                            assert!(
                                (advance - width).abs() < 0.01,
                                "{character:?} advances {advance}, but terminal cells are {width} wide at scale {scale}"
                            );
                        }
                    });
                })
                .drop_without_applying_deltas();
        }
    }

    #[test]
    fn grid_geometry_rounds_down_and_bounds_work() {
        let size = grid_size(Vec2::new(809.0, 161.0), Vec2::new(8.0, 16.0), 2.0);
        assert_eq!(
            (size.cols, size.rows, size.cell_width, size.cell_height),
            (101, 10, 16, 32)
        );
        let tiny = grid_size(Vec2::ZERO, Vec2::new(8.0, 16.0), 1.0);
        assert_eq!((tiny.cols, tiny.rows), (2, 1));
        let huge = grid_size(Vec2::splat(1e8), Vec2::splat(1.0), 1.0);
        assert_eq!((huge.cols, huge.rows), (500, 200));
    }

    #[test]
    fn copy_preserves_unicode_and_omits_wide_spacers() {
        let mut engine = Engine::new(
            GridSize {
                cols: 10,
                rows: 3,
                cell_width: 8,
                cell_height: 16,
            },
            Colors {
                foreground: Color32::WHITE,
                background: Color32::BLACK,
            },
        )
        .unwrap();
        engine.terminal.vt_write("界e\u{301}\r\nnext".as_bytes());
        let grid = engine.snapshot().unwrap();
        assert_eq!(selected_text(&grid, (0, 14)), "界e\u{301}\nnext");
        assert_eq!(selected_text(&grid, (14, 0)), "界e\u{301}\nnext");
        engine.terminal.vt_write(b"\x1b[2J\x1b[H\r\n\r\nlast");
        let blank_lines = engine.snapshot().unwrap();
        assert_eq!(selected_text(&blank_lines, (0, 24)), "\n\nlast");
        let unchanged = engine.snapshot().unwrap();
        assert!(same_cells(Some(&blank_lines), Some(&unchanged)));
        assert!(!same_cells(Some(&grid), Some(&blank_lines)));
    }

    #[test]
    fn selection_includes_both_drag_endpoints_in_either_direction() {
        assert_eq!(selection_range(3, 7), (3, 8));
        assert_eq!(selection_range(7, 3), (3, 8));
    }

    #[test]
    fn unstarted_pane_has_no_shell_or_grid_work() {
        let mut pane = TerminalPane::default();
        pane.set_visible(false);
        assert!(pane.session.is_none());
        assert!(pane.grid.is_none());
    }
}
