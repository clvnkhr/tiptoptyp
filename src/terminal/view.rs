use std::{
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
    wheel_remainder: f32,
    preedit: String,
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

    pub(crate) fn show(&mut self, ui: &mut egui::Ui, cwd: &Path) {
        let colors = Colors {
            foreground: ui.visuals().text_color(),
            background: ui.visuals().extreme_bg_color,
        };
        let snapshot = self.session.as_ref().map(Session::snapshot);
        if let Some(snapshot) = &snapshot {
            if self.revision != snapshot.revision
                && !same_cells(self.grid.as_deref(), snapshot.grid.as_deref())
            {
                self.selection = None;
            }
            self.revision = snapshot.revision;
            self.grid = snapshot.grid.clone();
        }
        let mut restart = false;
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            restart = ui
                .button("Restart terminal")
                .on_hover_text("Stop this shell and start a new one in the current workspace")
                .clicked();
            if let Some(snapshot) = &snapshot {
                let status = match &snapshot.status {
                    Status::Starting => Some("Starting shell…"),
                    Status::Exited(message) | Status::Failed(message) => Some(message.as_str()),
                    Status::Running => None,
                };
                if let Some(status) = status {
                    ui.add(egui::Label::new(status).truncate());
                }
            }
            let label = self
                .session
                .as_ref()
                .map_or(self.fixture_cwd.as_deref().unwrap_or(cwd), |session| {
                    session.cwd.as_path()
                });
            ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                ui.add(
                    egui::Label::new(egui::RichText::new(label.display().to_string()).weak())
                        .truncate(),
                );
            });
        });
        if restart {
            self.restart();
        }
        if let Some(error) = self.error.clone() {
            ui.horizontal_wrapped(|ui| {
                ui.colored_label(ui.visuals().error_fg_color, error);
                if ui.small_button("Dismiss").clicked() {
                    self.error = None;
                }
            });
        }
        let font = FontId::monospace(13.0);
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
            paint(ui, outer, rect, cell, &font, grid, self.selection, focused);
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
) {
    ui.painter().rect_filled(outer, 0.0, grid.background);
    let painter = ui.painter().with_clip_rect(rect);
    let selected = selection.map(|(a, b)| a.min(b)..a.max(b));
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
            let is_selected = selected
                .as_ref()
                .is_some_and(|range| range.contains(&(row_index * usize::from(grid.cols) + col)));
            let background = if is_selected {
                ui.visuals().selection.bg_fill
            } else {
                cell.background
            };
            if background != grid.background {
                painter.rect_filled(cell_rect, 0.0, background);
            }
            if cell.style.invisible {
                continue;
            }
            let foreground = if cell.style.faint {
                cell.foreground.gamma_multiply(0.65)
            } else {
                cell.foreground
            };
            if !cell.text.is_empty() {
                let format = egui::TextFormat {
                    font_id: font.clone(),
                    color: foreground,
                    italics: cell.style.italic,
                    ..Default::default()
                };
                let galley = painter.layout_job(egui::text::LayoutJob::single_section(
                    cell.text.clone(),
                    format,
                ));
                painter.with_clip_rect(cell_rect.intersect(rect)).galley(
                    pos,
                    galley.clone(),
                    foreground,
                );
                if cell.style.bold {
                    painter.with_clip_rect(cell_rect.intersect(rect)).galley(
                        pos + Vec2::new(0.4, 0.0),
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
