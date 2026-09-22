use std::sync::Arc;

use eframe::egui::Color32;
use libghostty_vt::{
    RenderState, Terminal, TerminalOptions, key,
    render::{CellIterator, CursorVisualStyle, Dirty, RowIterator},
    screen::CellWide,
    style::{PaletteIndex, RgbColor, Style},
    terminal::{Mode, ScrollViewport},
};

use super::input::KeyInput;

// The pinned C implementation measures this in bytes (the wrapper docs say
// lines). Ghostty rounds it to whole pages and always retains the active grid.
pub(super) const SCROLLBACK_BYTES: usize = 8 * 1024 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct GridSize {
    pub cols: u16,
    pub rows: u16,
    pub cell_width: u16,
    pub cell_height: u16,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Colors {
    pub foreground: Color32,
    pub background: Color32,
}

#[derive(Clone, Debug)]
pub(super) struct Cell {
    pub text: String,
    pub width: u8,
    pub foreground: Color32,
    pub background: Color32,
    pub style: Style,
}

#[derive(Clone, Debug)]
pub(super) struct Cursor {
    pub col: u16,
    pub row: u16,
    pub style: CursorVisualStyle,
    pub color: Color32,
}

#[derive(Clone, Debug)]
pub(super) struct Grid {
    pub rows: Vec<Arc<Vec<Cell>>>,
    pub cols: u16,
    pub cursor: Option<Cursor>,
    pub background: Color32,
    pub scroll_offset: usize,
    pub scroll_total: usize,
}

/// All handles stay on their creating thread. Only `Grid` crosses to the UI.
pub(super) struct Engine {
    pub terminal: Terminal<'static, 'static>,
    render: RenderState<'static>,
    rows: RowIterator<'static>,
    cells: CellIterator<'static>,
    keys: key::Encoder<'static>,
    key_event: key::Event<'static>,
    cached_rows: Vec<Arc<Vec<Cell>>>,
}

impl Engine {
    pub fn new(size: GridSize, colors: Colors) -> Result<Self, libghostty_vt::Error> {
        let mut engine = Self {
            terminal: Terminal::new(TerminalOptions {
                cols: size.cols,
                rows: size.rows,
                max_scrollback: SCROLLBACK_BYTES,
            })?,
            render: RenderState::new()?,
            rows: RowIterator::new()?,
            cells: CellIterator::new()?,
            keys: key::Encoder::new()?,
            key_event: key::Event::new()?,
            cached_rows: Vec::new(),
        };
        // Bound unterminated application-control strings too.
        engine.terminal.set_apc_max_bytes(Some(1024 * 1024))?;
        engine.set_colors(colors)?;
        engine.resize(size)?;
        Ok(engine)
    }

    pub fn set_colors(&mut self, colors: Colors) -> Result<(), libghostty_vt::Error> {
        self.terminal
            .set_default_fg_color(Some(rgb(colors.foreground)))?
            .set_default_bg_color(Some(rgb(colors.background)))?
            .set_default_cursor_color(Some(rgb(colors.foreground)))?;
        // ANSI's first sixteen colors need a light palette too: Ghostty's
        // default pale greens/yellows have poor contrast on a light workspace.
        let background = colors.background;
        let light = u32::from(background.r()) * 299
            + u32::from(background.g()) * 587
            + u32::from(background.b()) * 114
            > 128_000;
        let ansi = if light {
            [
                0x263238, 0xb91c1c, 0x267326, 0x8a6100, 0x1d4ed8, 0x8f2da8, 0x007582, 0x6b7280,
                0x52525b, 0xd02020, 0x2e7d32, 0xa16207, 0x2563eb, 0xa21caf, 0x0e7490, 0x111827,
            ]
        } else {
            [
                0x282a36, 0xff5555, 0x50fa7b, 0xf1fa8c, 0x8be9fd, 0xff79c6, 0x8be9fd, 0xd8dee9,
                0x6272a4, 0xff6e6e, 0x69ff94, 0xffffa5, 0x9aedfe, 0xff92df, 0xa4ffff, 0xffffff,
            ]
        };
        let mut palette = self.terminal.default_color_palette()?;
        for (index, value) in ansi.into_iter().enumerate() {
            palette.set(
                PaletteIndex(index as u8),
                RgbColor {
                    r: (value >> 16) as u8,
                    g: (value >> 8) as u8,
                    b: value as u8,
                },
            );
        }
        self.terminal.set_default_color_palette(Some(palette))?;
        Ok(())
    }

    pub fn resize(&mut self, size: GridSize) -> Result<(), libghostty_vt::Error> {
        self.terminal.resize(
            size.cols,
            size.rows,
            size.cell_width.into(),
            size.cell_height.into(),
        )
    }

    pub fn key(&mut self, input: KeyInput) -> Result<Vec<u8>, libghostty_vt::Error> {
        self.key_event
            .set_key(input.key)
            .set_mods(input.mods)
            .set_action(input.action)
            .set_unshifted_codepoint(input.unshifted)
            .set_utf8(input.text);
        let mut bytes = Vec::new();
        self.keys
            .set_options_from_terminal(&self.terminal)
            .encode_to_vec(&self.key_event, &mut bytes)?;
        if !bytes.is_empty() {
            self.terminal.scroll_viewport(ScrollViewport::Bottom);
        }
        Ok(bytes)
    }

    pub fn paste(&mut self, text: &str) -> Result<Vec<u8>, libghostty_vt::Error> {
        self.terminal.scroll_viewport(ScrollViewport::Bottom);
        let bracketed = self.terminal.mode(Mode::BRACKETED_PASTE)?;
        Ok(paste_bytes(text, bracketed))
    }

    pub fn snapshot(&mut self) -> Result<Grid, libghostty_vt::Error> {
        let snapshot = self.render.update(&self.terminal)?;
        let colors = snapshot.colors()?;
        let full = snapshot.dirty()? == Dirty::Full
            || self.cached_rows.len() != usize::from(snapshot.rows()?);
        let mut rows = self.rows.update(&snapshot)?;
        let mut index = 0;
        while let Some(row) = rows.next() {
            if full || row.dirty()? {
                let mut cells = self.cells.update(row)?;
                let mut values = Vec::with_capacity(usize::from(snapshot.cols()?));
                while let Some(cell) = cells.next() {
                    let style = cell.style()?;
                    let mut foreground = cell.fg_color()?.unwrap_or(colors.foreground);
                    let mut background = cell.bg_color()?.unwrap_or(colors.background);
                    if style.inverse {
                        std::mem::swap(&mut foreground, &mut background);
                    }
                    let width = match cell.raw_cell()?.wide()? {
                        CellWide::Narrow => 1,
                        CellWide::Wide => 2,
                        CellWide::SpacerHead | CellWide::SpacerTail => 0,
                    };
                    let mut text = String::new();
                    cell.graphemes_utf8(&mut text)?;
                    values.push(Cell {
                        text,
                        width,
                        foreground: color(foreground),
                        background: color(background),
                        style,
                    });
                }
                let values = Arc::new(values);
                if index < self.cached_rows.len() {
                    self.cached_rows[index] = values;
                } else {
                    self.cached_rows.push(values);
                }
                row.set_dirty(false)?;
            }
            index += 1;
        }
        self.cached_rows.truncate(index);
        let cursor = if snapshot.cursor_visible()? {
            snapshot.cursor_viewport()?.map(|position| Cursor {
                col: position.x,
                row: position.y,
                style: snapshot
                    .cursor_visual_style()
                    .unwrap_or(CursorVisualStyle::Block),
                color: color(colors.cursor.unwrap_or(colors.foreground)),
            })
        } else {
            None
        };
        snapshot.set_dirty(Dirty::Clean)?;
        let scroll = self.terminal.scrollbar()?;
        Ok(Grid {
            rows: self.cached_rows.clone(),
            cols: snapshot.cols()?,
            cursor,
            background: color(colors.background),
            scroll_offset: scroll.offset as usize,
            scroll_total: scroll.total as usize,
        })
    }
}

fn rgb(color: Color32) -> RgbColor {
    RgbColor {
        r: color.r(),
        g: color.g(),
        b: color.b(),
    }
}

fn color(rgb: RgbColor) -> Color32 {
    Color32::from_rgb(rgb.r, rgb.g, rgb.b)
}

fn paste_bytes(text: &str, bracketed: bool) -> Vec<u8> {
    // Escape bytes must not terminate bracketed paste early. Retain Unicode,
    // tabs and newlines, but exclude other terminal controls from clipboard data.
    let text = text.replace("\r\n", "\n").replace('\r', "\n");
    let safe: String = text
        .chars()
        .filter(|c| !c.is_control() || *c == '\n' || *c == '\t')
        .collect();
    if bracketed {
        format!("\x1b[200~{safe}\x1b[201~").into_bytes()
    } else {
        safe.replace('\n', "\r").into_bytes()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    pub(super) fn engine(cols: u16, rows: u16) -> Engine {
        Engine::new(
            GridSize {
                cols,
                rows,
                cell_width: 8,
                cell_height: 16,
            },
            Colors {
                foreground: Color32::WHITE,
                background: Color32::BLACK,
            },
        )
        .unwrap()
    }

    #[test]
    fn ghostty_parses_colors_unicode_cursor_and_alternate_screen() {
        let mut engine = engine(20, 4);
        engine
            .terminal
            .vt_write("\x1b[31mred\x1b[0m 界e\u{301}".as_bytes());
        let grid = engine.snapshot().unwrap();
        assert_eq!(grid.rows[0][0].text, "r");
        assert_ne!(grid.rows[0][0].foreground, Color32::WHITE);
        assert_eq!(grid.rows[0][4].text, "界");
        assert_eq!(grid.rows[0][4].width, 2);
        assert_eq!(grid.rows[0][5].width, 0);
        assert_eq!(grid.rows[0][6].text, "e\u{301}");
        assert_eq!(grid.cursor.unwrap().col, 7);
        engine.terminal.vt_write(b"\x1b[?1049h\x1b[Hfull screen");
        assert_eq!(engine.snapshot().unwrap().rows[0][0].text, "f");
        engine.terminal.vt_write(b"\x1b[?1049l");
        assert_eq!(engine.snapshot().unwrap().rows[0][0].text, "r");
    }

    #[test]
    fn unchanged_render_rows_are_shared_and_scrollback_is_bounded() {
        let mut engine = engine(80, 24);
        engine.terminal.vt_write(b"hello");
        let before = engine.snapshot().unwrap();
        let after = engine.snapshot().unwrap();
        assert!(Arc::ptr_eq(&before.rows[0], &after.rows[0]));
        for _ in 0..30_000 {
            engine.terminal.vt_write(b"x\r\n");
        }
        assert!(
            engine.terminal.scrollback_rows().unwrap() < 30_000,
            "{}",
            engine.terminal.scrollback_rows().unwrap()
        );
        engine.terminal.scroll_viewport(ScrollViewport::Top);
        assert_eq!(engine.snapshot().unwrap().scroll_offset, 0);
    }

    #[test]
    fn theme_changes_refresh_ansi_colors_without_overwriting_program_colors() {
        let mut engine = engine(20, 3);
        engine.terminal.vt_write(b"\x1b[32mgreen");
        let dark = engine.snapshot().unwrap().rows[0][0].foreground;
        engine
            .set_colors(Colors {
                foreground: Color32::BLACK,
                background: Color32::WHITE,
            })
            .unwrap();
        let light = engine.snapshot().unwrap().rows[0][0].foreground;
        assert_ne!(light, dark);
        assert!(light.g() < 160, "light theme must use a dark green");
        engine.terminal.vt_write(b"\x1b]4;2;rgb:12/34/56\x07");
        engine
            .set_colors(Colors {
                foreground: Color32::WHITE,
                background: Color32::BLACK,
            })
            .unwrap();
        assert_eq!(
            engine.snapshot().unwrap().rows[0][0].foreground,
            Color32::from_rgb(0x12, 0x34, 0x56)
        );
    }

    #[test]
    fn paste_honors_mode_normalizes_newlines_and_cannot_escape_brackets() {
        assert_eq!(paste_bytes("a\r\nb\nc", false), b"a\rb\rc");
        assert_eq!(
            paste_bytes("a\x1b[201~\0b", true),
            b"\x1b[200~a[201~b\x1b[201~"
        );
        let mut engine = engine(20, 3);
        engine.terminal.vt_write(b"\x1b[?2004h");
        assert_eq!(engine.paste("hi").unwrap(), b"\x1b[200~hi\x1b[201~");
    }
}
