use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

use eframe::egui::Color32;
use libghostty_vt::{
    RenderState, Terminal, TerminalOptions, key, mouse,
    render::{CellIterator, CursorVisualStyle, Dirty, RowIterator},
    screen::CellWide,
    style::{PaletteIndex, RgbColor, Style},
    terminal::{ColorScheme, Mode, ScrollViewport},
};

use super::input::{KeyInput, MouseInput};

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
impl Colors {
    /// Relative-luminance test shared by the ANSI palette switch and the
    /// child theme hint: both key off the background, not the app chrome.
    pub(super) fn is_light(&self) -> bool {
        let background = self.background;
        u32::from(background.r()) * 299
            + u32::from(background.g()) * 587
            + u32::from(background.b()) * 114
            > 128_000
    }
    /// rxvt-style `fg;bg` pair so child TUI programs detect the theme:
    /// light text on a dark background and vice versa.
    pub(super) fn colorfgbg(&self) -> &'static str {
        if self.is_light() { "0;15" } else { "15;0" }
    }
    /// Contour/Ghostty color-scheme identity for mode 2031 reports.
    pub(super) fn scheme(&self) -> ColorScheme {
        if self.is_light() {
            ColorScheme::Light
        } else {
            ColorScheme::Dark
        }
    }
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
    /// The program enabled a mouse tracking mode; pointer input belongs to it.
    pub mouse_tracking: bool,
}

/// All handles stay on their creating thread. Only `Grid` crosses to the UI.
pub(super) struct Engine {
    pub terminal: Terminal<'static, 'static>,
    render: RenderState<'static>,
    rows: RowIterator<'static>,
    cells: CellIterator<'static>,
    keys: key::Encoder<'static>,
    key_event: key::Event<'static>,
    mouse: mouse::Encoder<'static>,
    mouse_event: mouse::Event<'static>,
    cached_rows: Vec<Arc<Vec<Cell>>>,
    /// Live light/dark identity for `CSI ? 996 n` answers. Updated by every
    /// palette switch so queries track the theme without a restart.
    scheme_light: Arc<AtomicBool>,
}

impl Engine {
    pub fn new(size: GridSize, colors: Colors) -> Result<Self, libghostty_vt::Error> {
        let scheme_light = Arc::new(AtomicBool::new(colors.is_light()));
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
            mouse: mouse::Encoder::new()?,
            mouse_event: mouse::Event::new()?,
            cached_rows: Vec::new(),
            scheme_light: scheme_light.clone(),
        };
        // Answer `CSI ? 996 n` color-scheme queries with the live theme so
        // programs learn light/dark without a restart.
        engine.terminal.on_color_scheme(move |_| {
            Some(if scheme_light.load(Ordering::Acquire) {
                ColorScheme::Light
            } else {
                ColorScheme::Dark
            })
        })?;
        // Bound unterminated application-control strings too.
        engine.terminal.set_apc_max_bytes(Some(1024 * 1024))?;
        engine.set_colors(colors)?;
        engine.resize(size)?;
        Ok(engine)
    }

    /// Switches the palette, returning the new scheme when it flipped
    /// light/dark. The worker turns a flip into an unsolicited
    /// `CSI ? 997` report for programs that enabled mode 2031.
    pub fn set_colors(
        &mut self,
        colors: Colors,
    ) -> Result<Option<ColorScheme>, libghostty_vt::Error> {
        self.terminal
            .set_default_fg_color(Some(rgb(colors.foreground)))?
            .set_default_bg_color(Some(rgb(colors.background)))?
            .set_default_cursor_color(Some(rgb(colors.foreground)))?;
        // ANSI's first sixteen colors need a light palette too: Ghostty's
        // default pale greens/yellows have poor contrast on a light workspace.
        let ansi = if colors.is_light() {
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
        let scheme = colors.scheme();
        let light = scheme == ColorScheme::Light;
        let was_light = self.scheme_light.swap(light, Ordering::AcqRel);
        if light == was_light {
            Ok(None)
        } else {
            Ok(Some(scheme))
        }
    }

    /// Unsolicited color-scheme bytes for the running program, or `None`
    /// when it did not opt into mode 2031 reports or the scheme did not flip.
    pub(super) fn color_scheme_notification(
        &self,
        flipped: Option<ColorScheme>,
    ) -> Result<Option<Vec<u8>>, libghostty_vt::Error> {
        let Some(scheme) = flipped else {
            return Ok(None);
        };
        if !self.terminal.mode(Mode::COLOR_SCHEME_REPORT)? {
            return Ok(None);
        }
        let mut report = [0; 16];
        let len = scheme.encode_report(&mut report)?;
        Ok(Some(report[..len].to_vec()))
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

    /// Encodes one pointer event for the program, honoring the tracking mode
    /// and output format it selected. Events it does not want encode to empty.
    pub fn mouse(&mut self, input: MouseInput) -> Result<Vec<u8>, libghostty_vt::Error> {
        self.mouse_event
            .set_action(input.action)
            .set_button(input.button)
            .set_mods(input.mods)
            .set_position(mouse::Position {
                x: input.x,
                y: input.y,
            });
        self.mouse
            .set_options_from_terminal(&self.terminal)
            .set_size(input.geometry)
            .set_any_button_pressed(input.any_button_pressed)
            .set_track_last_cell(true);
        let mut bytes = Vec::new();
        self.mouse.encode_to_vec(&self.mouse_event, &mut bytes)?;
        Ok(bytes)
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
            mouse_tracking: self.terminal.is_mouse_tracking()?,
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
    fn grapheme_widths_keep_emoji_wide_and_private_use_icons_narrow() {
        let mut engine = engine(40, 2);
        engine
            .terminal
            .vt_write("0😀界e\u{301}\u{f120}X".as_bytes());
        let grid = engine.snapshot().unwrap();
        let cells: Vec<_> = grid.rows[0]
            .iter()
            .take(8)
            .map(|cell| (cell.text.as_str(), cell.width))
            .collect();
        assert_eq!(
            cells,
            [
                ("0", 1),
                ("😀", 2),
                ("", 0),
                ("界", 2),
                ("", 0),
                ("e\u{301}", 1),
                ("\u{f120}", 1),
                ("X", 1)
            ]
        );
        assert_eq!(grid.cursor.unwrap().col, 8);
    }

    #[test]
    fn emoji_sequences_keep_one_two_column_grapheme_and_the_next_cursor_position() {
        for text in [
            "🦀",
            "📦",
            "👩‍💻",
            "🇬🇧",
            "👍🏽",
            "1\u{fe0f}\u{20e3}",
            "❤\u{fe0f}",
        ] {
            let mut engine = engine(20, 2);
            // Applications opt into grapheme clustering with DEC mode 2027.
            engine
                .terminal
                .set_mode(Mode::GRAPHEME_CLUSTER, true)
                .unwrap();
            engine.terminal.vt_write(format!("{text}X").as_bytes());
            let grid = engine.snapshot().unwrap();
            assert_eq!(grid.rows[0][0].text, text);
            assert_eq!(grid.rows[0][0].width, 2, "{text}");
            assert_eq!(grid.rows[0][1].width, 0, "{text}");
            assert_eq!(grid.rows[0][2].text, "X", "{text}");
            assert_eq!(grid.cursor.unwrap().col, 3, "{text}");
        }
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
    fn color_scheme_query_and_reports_track_theme_flips() {
        use std::cell::RefCell;
        use std::rc::Rc;
        fn query_scheme(engine: &mut Engine, replies: &Rc<RefCell<Vec<u8>>>) -> String {
            replies.borrow_mut().clear();
            engine.terminal.vt_write(b"\x1b[?996n");
            String::from_utf8(replies.borrow().clone()).unwrap()
        }
        let mut engine = engine(20, 3);
        let replies = Rc::new(RefCell::new(Vec::<u8>::new()));
        let sink = replies.clone();
        engine
            .terminal
            .on_pty_write(move |_, bytes| sink.borrow_mut().extend_from_slice(bytes))
            .unwrap();
        // Black fixture background reports dark.
        assert!(
            query_scheme(&mut engine, &replies).contains("?997;1"),
            "{:?}",
            replies.borrow()
        );
        // No flip, no report, even with mode 2031 enabled.
        engine.terminal.vt_write(b"\x1b[?2031h");
        let dark = Colors {
            foreground: Color32::WHITE,
            background: Color32::BLACK,
        };
        assert_eq!(engine.set_colors(dark).unwrap(), None);
        assert_eq!(engine.color_scheme_notification(None).unwrap(), None);
        // A flip answers queries light but stays silent without opt-in...
        engine.terminal.vt_write(b"\x1b[?2031l");
        let light = Colors {
            foreground: Color32::BLACK,
            background: Color32::WHITE,
        };
        let flipped = engine.set_colors(light).unwrap();
        assert!(
            query_scheme(&mut engine, &replies).contains("?997;2"),
            "{:?}",
            replies.borrow()
        );
        assert_eq!(engine.color_scheme_notification(flipped).unwrap(), None);
        // ...and reports the flipped scheme once the program opts in.
        engine.terminal.vt_write(b"\x1b[?2031h");
        let flipped = engine.set_colors(dark).unwrap();
        assert_eq!(
            engine.color_scheme_notification(flipped).unwrap(),
            Some(b"\x1b[?997;1n".to_vec())
        );
    }

    #[test]
    fn live_palette_switches_reach_programs_through_color_queries() {
        use std::cell::RefCell;
        use std::rc::Rc;
        let mut engine = engine(20, 3);
        let replies = Rc::new(RefCell::new(Vec::<u8>::new()));
        let sink = replies.clone();
        engine
            .terminal
            .on_pty_write(move |_, bytes| sink.borrow_mut().extend_from_slice(bytes))
            .unwrap();
        let query = |engine: &mut Engine| {
            replies.borrow_mut().clear();
            engine.terminal.vt_write(b"\x1b]11;?\x1b\\");
            engine.terminal.vt_write(b"\x1b]4;2;?\x1b\\");
            String::from_utf8(replies.borrow().clone()).unwrap()
        };
        // Black background, dark-theme green.
        let dark = query(&mut engine);
        assert!(dark.contains("]11;rgb:0000/0000/0000"), "{dark:?}");
        assert!(dark.contains("]4;2;rgb:5050/fafa/7b7b"), "{dark:?}");
        engine
            .set_colors(Colors {
                foreground: Color32::BLACK,
                background: Color32::WHITE,
            })
            .unwrap();
        // Already-running programs see the switched theme on their next query.
        let light = query(&mut engine);
        assert!(light.contains("]11;rgb:ffff/ffff/ffff"), "{light:?}");
        assert!(!light.contains("5050/fafa/7b7b"), "{light:?}");
    }

    #[test]
    fn child_theme_hint_follows_background_luminance() {
        let dark = Colors {
            foreground: Color32::WHITE,
            background: Color32::BLACK,
        };
        assert!(!dark.is_light());
        assert_eq!(dark.colorfgbg(), "15;0");
        let light = Colors {
            foreground: Color32::BLACK,
            background: Color32::WHITE,
        };
        assert!(light.is_light());
        assert_eq!(light.colorfgbg(), "0;15");
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
    fn negative_scroll_deltas_move_the_viewport_toward_older_lines() {
        let mut engine = engine(20, 4);
        for _ in 0..10 {
            engine.terminal.vt_write(b"line\r\n");
        }
        engine.terminal.scroll_viewport(ScrollViewport::Bottom);
        let bottom = engine.snapshot().unwrap().scroll_offset;
        engine.terminal.scroll_viewport(ScrollViewport::Delta(-2));
        let up = engine.snapshot().unwrap().scroll_offset;
        assert_eq!(up + 2, bottom, "negative delta must scroll up");
        engine.terminal.scroll_viewport(ScrollViewport::Delta(2));
        assert_eq!(engine.snapshot().unwrap().scroll_offset, bottom);
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

    #[test]
    fn mouse_events_follow_the_programs_tracking_and_format() {
        use libghostty_vt::key::Mods;
        let geometry = mouse::EncoderSize {
            screen_width: 160,
            screen_height: 64,
            cell_width: 8,
            cell_height: 16,
            padding_top: 0,
            padding_bottom: 0,
            padding_right: 0,
            padding_left: 0,
        };
        let input = |action, button, x, y, geometry| MouseInput {
            action,
            button: Some(button),
            mods: Mods::empty(),
            x,
            y,
            geometry,
            any_button_pressed: true,
        };

        let mut engine = engine(20, 4);
        // Silent until the program enables a tracking mode.
        assert!(
            engine
                .mouse(input(
                    mouse::Action::Press,
                    mouse::Button::Left,
                    20.0,
                    16.0,
                    geometry
                ))
                .unwrap()
                .is_empty()
        );
        assert!(!engine.snapshot().unwrap().mouse_tracking);
        engine.terminal.vt_write(b"\x1b[?1002;1006h");
        assert!(engine.snapshot().unwrap().mouse_tracking);
        // SGR reports 1-based cells: 8x16 pixel cells put (20,16) at 3;2.
        assert_eq!(
            engine
                .mouse(input(
                    mouse::Action::Press,
                    mouse::Button::Left,
                    20.0,
                    16.0,
                    geometry
                ))
                .unwrap(),
            b"\x1b[<0;3;2M"
        );
        assert_eq!(
            engine
                .mouse(input(
                    mouse::Action::Release,
                    mouse::Button::Left,
                    20.0,
                    16.0,
                    geometry
                ))
                .unwrap(),
            b"\x1b[<0;3;2m"
        );
        // Drag motion carries button+32; hover motion stays unreported in
        // button-event tracking.
        assert_eq!(
            engine
                .mouse(input(
                    mouse::Action::Motion,
                    mouse::Button::Left,
                    12.0,
                    32.0,
                    geometry
                ))
                .unwrap(),
            b"\x1b[<32;2;3M"
        );
        let hover = MouseInput {
            action: mouse::Action::Motion,
            button: None,
            mods: Mods::empty(),
            x: 12.0,
            y: 32.0,
            geometry,
            any_button_pressed: false,
        };
        assert!(engine.mouse(hover).unwrap().is_empty());
        // Wheel notches arrive as buttons 4/5.
        assert_eq!(
            engine
                .mouse(input(
                    mouse::Action::Press,
                    mouse::Button::Four,
                    20.0,
                    16.0,
                    geometry
                ))
                .unwrap(),
            b"\x1b[<64;3;2M"
        );
        // A grid inset shifts the reported cell, not the screen position.
        let padded = mouse::EncoderSize {
            padding_left: 8,
            padding_top: 16,
            ..geometry
        };
        assert_eq!(
            engine
                .mouse(input(
                    mouse::Action::Press,
                    mouse::Button::Left,
                    20.0,
                    16.0,
                    padded
                ))
                .unwrap(),
            b"\x1b[<0;2;1M"
        );
    }
}
