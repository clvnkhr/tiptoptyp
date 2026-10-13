use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::Arc,
};

use eframe::egui::{self, Color32, FontId, Pos2, Rect, Sense, Stroke, StrokeKind, Vec2};
use libghostty_vt::{mouse, render::CursorVisualStyle, style::Underline};

use super::{
    engine::{Colors, Engine, Grid, GridSize},
    input::{self, MouseInput},
    session::{Command, Session, Status},
};

pub(crate) fn terminal_id(context: &egui::Context) -> egui::Id {
    crate::child_view::viewport_scoped_id(context, "terminal-grid")
}

pub(crate) fn terminal_find_id(context: &egui::Context) -> egui::Id {
    terminal_id(context).with("find")
}

pub(crate) fn terminal_panel_colors(ui: &egui::Ui) -> Colors {
    Colors {
        foreground: ui.visuals().text_color(),
        background: ui.visuals().extreme_bg_color,
    }
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
    display: RowDisplayCache,
    emoji: super::emoji::EmojiCache,
    mouse_held: Option<egui::PointerButton>,
    motion_cell: Option<(i32, i32)>,
    last_mouse_input: Option<MouseInput>,
    last_bell: u64,
    bell_pending: bool,
    last_beep: Option<std::time::Instant>,
    link_action: Option<super::TerminalLinkAction>,
    suppress_select_until_release: bool,
    find: super::find::TerminalFind,
}

/// Audible alert for terminal bells. macOS-only: other platforms keep the
/// tab badge without sound.
pub(super) fn alert_bell() {
    #[cfg(target_os = "macos")]
    objc2_app_kit::NSBeep();
}

/// Minimum gap between audible beeps; the badge still records every bell.
const BELL_BEEP_GAP: std::time::Duration = std::time::Duration::from_secs(1);

impl TerminalPane {
    pub(crate) fn request_focus(&mut self) {
        self.focus_requested = true;
    }

    #[cfg(test)]
    pub(crate) fn focus_requested_for_test(&self) -> bool {
        self.focus_requested
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
        if !visible {
            self.release_mouse(None);
            self.suppress_select_until_release = false;
            self.wheel_remainder = 0.0;
            if self.focused {
                self.send(Command::Focus(false));
                self.focused = false;
                self.preedit.clear();
            }
        }
    }

    /// Theme changes reach every started pane, including background programs.
    /// Retry mailbox backpressure on the next frame; unchanged colors do no work.
    pub(super) fn update_colors(&mut self, colors: Colors) -> bool {
        if self.colors == Some(colors) {
            return true;
        }
        if let Some(engine) = &mut self.fixture
            && let Err(error) = engine.set_colors(colors)
        {
            self.error = Some(error.to_string());
            return true;
        }
        if let Some(session) = &self.session
            && matches!(
                session.queue_colors(colors),
                Err(std::sync::mpsc::TrySendError::Full(_))
            )
        {
            return false;
        }
        // A disconnected worker has already published failure and cannot be
        // recolored. Stop retrying it rather than creating an idle repaint loop.
        self.colors = Some(colors);
        true
    }

    #[cfg(test)]
    pub(super) fn with_test_session(session: Session) -> Self {
        Self {
            session: Some(session),
            ..Self::default()
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
                self.colors = Some(colors);
            }
            Err(error) => self.error = Some(error.to_string()),
        }
    }

    #[cfg(all(feature = "desktop-ui-tests", target_os = "macos"))]
    pub(crate) fn content_revision_for_test(&self) -> Option<u64> {
        self.session.as_ref().map(Session::content_revision)
    }

    #[cfg(test)]
    pub(crate) fn fixture_size_for_test(&self) -> (usize, usize) {
        let size = self.size.expect("laid out terminal fixture");
        (usize::from(size.cols), usize::from(size.rows))
    }

    #[cfg(test)]
    pub(crate) fn feed_fixture_for_test(&mut self, output: &[u8]) {
        let engine = self.fixture.as_mut().expect("started terminal fixture");
        engine.terminal.vt_write(output);
        self.grid = Some(Arc::new(engine.snapshot().expect("fixture snapshot")));
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

    fn report_mouse(&mut self, input: MouseInput) {
        self.last_mouse_input = input.any_button_pressed.then_some(input);
        self.send(Command::Mouse(input));
    }

    /// A pane can disappear without another pointer-up event. Retain the last
    /// encoded position/geometry so every exit path can finish the TUI drag.
    fn release_mouse(&mut self, modifiers: Option<egui::Modifiers>) {
        self.motion_cell = None;
        if let (Some(button), Some(mut input)) =
            (self.mouse_held.take(), self.last_mouse_input.take())
        {
            input.action = mouse::Action::Release;
            input.button = input::mouse_button(button);
            input.any_button_pressed = false;
            if let Some(modifiers) = modifiers {
                input.mods = input::mods(modifiers);
            }
            self.send(Command::Mouse(input));
        }
    }

    pub(crate) fn bell_pending(&self) -> bool {
        self.bell_pending
    }

    pub(crate) fn take_link_action(&mut self) -> Option<super::TerminalLinkAction> {
        self.link_action.take()
    }

    /// Open terminal find and focus its field.
    pub(crate) fn open_find(&mut self) {
        self.focus_requested = false;
        self.find.open();
    }

    pub(crate) fn toggle_find(&mut self, context: &egui::Context) {
        let find_id = terminal_find_id(context);
        if self.find.is_open() && context.memory(|memory| memory.has_focus(find_id)) {
            self.find.close();
            self.request_focus();
        } else {
            self.open_find();
        }
    }

    pub(crate) fn find_open(&self) -> bool {
        self.find.is_open()
    }

    #[cfg(all(feature = "desktop-ui-tests", target_os = "macos"))]
    pub(crate) fn inspect_find(&self, context: &egui::Context) -> serde_json::Value {
        let find_id = terminal_find_id(context);
        let selected = self.find.selected_match();
        let (offset, rows) = self
            .grid
            .as_ref()
            .map_or((0, 0), |grid| (grid.scroll_offset, grid.rows.len()));
        serde_json::json!({
            "open": self.find.is_open(),
            "focused": context.memory(|memory| memory.has_focus(find_id)),
            "query": self.find.query,
            "selected": self.find.selected_index(),
            "matches": self.find.matches().len(),
            "pending": self.find.pending(),
            "selected_line": selected.map(|hit| hit.line),
            "selected_visible": selected.is_some_and(|hit| super::find::TerminalFind::visible_row(offset, rows, hit.line).is_some()),
            "scroll_offset": offset,
            "rows": rows,
        })
    }

    /// Step the find selection, scrolling the match into view.
    pub(crate) fn find_step(&mut self, previous: bool) {
        let Some(line) = self.find.step(previous) else {
            return;
        };
        let (total, rows, offset) = match &self.grid {
            Some(grid) => (grid.scroll_total, grid.rows.len(), grid.scroll_offset),
            None => return,
        };
        let delta = super::find::TerminalFind::reveal_delta(total, rows, offset, line);
        if delta != 0 {
            self.send(Command::Scroll(delta));
        }
    }

    fn show_find_bar(&mut self, ui: &mut egui::Ui) {
        // The field owns its own id so focus can be tested before consuming
        // keys: Enter and Escape must still reach the shell when the field
        // is not focused.
        let field_id = terminal_find_id(ui.ctx());
        let field_focused = ui.memory(|memory| memory.has_focus(field_id));
        // Keys first so Enter/Escape never reach the field or the shell.
        let mut step = None;
        if field_focused {
            if ui.input_mut(|input| input.consume_key(egui::Modifiers::SHIFT, egui::Key::Enter)) {
                step = Some(true);
            } else if ui
                .input_mut(|input| input.consume_key(egui::Modifiers::NONE, egui::Key::Enter))
            {
                step = Some(false);
            }
            if ui.input_mut(|input| input.consume_key(egui::Modifiers::NONE, egui::Key::Escape)) {
                self.find.close();
                self.request_focus();
                return;
            }
        }
        let mut close = false;
        let focus_field = self.find.consume_focus_request();
        let mut query_changed = false;
        let mut refocus = false;
        ui.horizontal(|ui| {
            ui.label("Find");
            let response = ui.add(
                egui::TextEdit::singleline(&mut self.find.query)
                    .char_limit(super::find::MAX_QUERY_CHARS)
                    .id(field_id)
                    .hint_text("Search scrollback")
                    .desired_width(220.0),
            );
            response.widget_info(|| {
                let mut info = egui::WidgetInfo::labeled(egui::WidgetType::TextEdit, true, "Search terminal scrollback");
                info.current_text_value = Some(self.find.query.clone());
                info
            });
            if focus_field {
                response.request_focus();
            }
            #[cfg(all(feature = "desktop-ui-tests", target_os = "macos"))]
            crate::desktop_test::observe("terminal.find.query", &response);
            query_changed = response.changed();
            let (selected, total) = self.find.match_count();
            let count = if self.find.query.is_empty() {
                "–".to_owned()
            } else if total == 0 {
                if self.find.pending() { "…" } else { "0/0" }.to_owned()
            } else {
                format!("{}/{}", selected + 1, total)
            };
            let detail = if self.find.truncated() {
                "Search is limited to recent output (4,096 rows / 512 KiB) and 8,192 matches."
            } else if self.find.pending() {
                "Searching terminal scrollback…"
            } else {
                "Case-insensitive search of terminal scrollback. Enter / Shift+Enter moves between matches."
            };
            ui.label(count).on_hover_text(detail);
            let previous = crate::app::icons::square_icon_button(ui, crate::app::icons::UiIcon::Up, "Previous terminal match", 22.0);
            let next = crate::app::icons::square_icon_button(ui, crate::app::icons::UiIcon::Down, "Next terminal match", 22.0);
            let dismiss = crate::app::icons::square_icon_button(ui, crate::app::icons::UiIcon::Close, "Close terminal find", 22.0);
            #[cfg(all(feature = "desktop-ui-tests", target_os = "macos"))] {
                crate::desktop_test::observe("terminal.find.previous", &previous);
                crate::desktop_test::observe("terminal.find.next", &next);
                crate::desktop_test::observe("terminal.find.close", &dismiss);
            }
            if previous.clicked() { step = Some(true); refocus = true; }
            if next.clicked() { step = Some(false); refocus = true; }
            close = dismiss.clicked();
        });
        // TextEdit installs its default filter while drawing. Keep Escape
        // owned until our next pass, instead of egui clearing focus first.
        if ui.memory(|memory| memory.has_focus(field_id)) {
            ui.memory_mut(|memory| {
                memory.set_focus_lock_filter(
                    field_id,
                    egui::EventFilter {
                        horizontal_arrows: true,
                        vertical_arrows: true,
                        escape: true,
                        ..Default::default()
                    },
                )
            });
        }
        if refocus {
            // Click-away processing can clear a same-pass request. Restore
            // the field on the next pass, after this click has been consumed.
            self.find.request_focus();
            ui.ctx().request_repaint();
        }
        if query_changed {
            self.find.mark_query_dirty();
        }
        if let Some(previous) = step {
            self.find_step(previous);
        }
        if close {
            self.find.close();
            self.request_focus();
        }
    }

    fn paint_find_matches(
        &self,
        ui: &egui::Ui,
        rect: Rect,
        cell: Vec2,
        grid: &Grid,
        foreground: egui::Color32,
    ) {
        if !self.find.is_open() || self.find.matches().is_empty() {
            return;
        }
        let selected = self.find.selected_index();
        let painter = ui.painter().with_clip_rect(rect);
        let matches = self.find.matches();
        let start = matches.partition_point(|hit| hit.line < grid.scroll_offset);
        let end = matches.partition_point(|hit| hit.line < grid.scroll_offset + grid.rows.len());
        for (index, hit) in matches[start..end].iter().enumerate() {
            let index = start + index;
            let Some(row) = super::find::TerminalFind::visible_row(
                grid.scroll_offset,
                grid.rows.len(),
                hit.line,
            ) else {
                continue;
            };
            let Some(cells) = grid.rows.get(row) else {
                continue;
            };
            let Some(columns) =
                super::find::TerminalFind::match_columns(cells, hit.length, hit.column)
            else {
                continue;
            };
            let highlight = Rect::from_min_max(
                Pos2::new(
                    rect.min.x + columns.start as f32 * cell.x,
                    rect.min.y + row as f32 * cell.y,
                ),
                Pos2::new(
                    rect.min.x + columns.end as f32 * cell.x,
                    rect.min.y + (row as f32 + 1.0) * cell.y,
                ),
            );
            if Some(index) == selected {
                painter.rect_filled(highlight, 0.0, foreground.gamma_multiply(0.45));
                painter.rect_stroke(
                    highlight,
                    0.0,
                    Stroke::new(1.0, foreground),
                    egui::StrokeKind::Inside,
                );
            } else {
                painter.rect_filled(highlight, 0.0, foreground.gamma_multiply(0.18));
            }
        }
    }

    /// The link under a pointer position on the grid, if any, with its row
    /// and cell columns for underlining. Computed on demand for one row so
    /// hovering costs a single scan, never a grid walk.
    fn link_at(
        grid: &Grid,
        rect: Rect,
        cell: Vec2,
        pos: Pos2,
    ) -> Option<(super::TerminalLinkAction, i32, std::ops::Range<usize>)> {
        let (col, row) = pointer_cell(pos, rect, cell)?;
        if col < 0 || row < 0 {
            return None;
        }
        let cells = grid.rows.get(row as usize)?;
        let (text, columns) = super::links::row_text_and_columns(cells);
        let links = super::links::find_links(&text);
        let link = super::links::link_at_column(&links, &columns, col as usize)?;
        let start = link.range.start;
        let end = link.range.end;
        if start >= columns.len() || end == 0 {
            return None;
        }
        let first = columns[start];
        let last = columns[end.min(columns.len()) - 1] + 1;
        let action = match link.target.clone() {
            super::links::LinkTarget::Url(url) => super::TerminalLinkAction::OpenUrl(url),
            super::links::LinkTarget::Path { path, line, column } => {
                super::TerminalLinkAction::OpenPath {
                    path: PathBuf::from(path),
                    line,
                    column,
                }
            }
        };
        Some((action, row, first..last))
    }

    /// Check for newly rung bells. Beeps at most once per second no matter
    /// how fast a program rings; marks the tab badge unless the pane is
    /// currently shown, which clears a pending badge.
    pub(crate) fn poll_bell(&mut self, shown: bool) {
        let Some(bells) = self.session.as_ref().map(Session::bells) else {
            return;
        };
        if bells > self.last_bell {
            self.last_bell = bells;
            let recent = self
                .last_beep
                .is_some_and(|last| last.elapsed() < BELL_BEEP_GAP);
            if !recent {
                self.last_beep = Some(std::time::Instant::now());
                alert_bell();
            }
            self.bell_pending = !shown;
        } else if shown {
            self.bell_pending = false;
        }
    }

    pub(crate) fn sync_snapshot(&mut self) {
        if let Some(snapshot) = self.session.as_ref().and_then(Session::try_snapshot) {
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
        let colors = terminal_panel_colors(ui);
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
        if self.find.is_open() {
            self.show_find_bar(ui);
        }
        self.find.poll(
            ui.ctx(),
            self.session.as_ref(),
            self.session.as_ref().map_or(0, Session::content_revision),
        );
        let font = crate::theme::terminal_font();
        let cell = ui.fonts_mut(|fonts| {
            Vec2::new(
                fonts.glyph_width(&font, 'M'),
                fonts.row_height(&font).ceil(),
            )
        });
        let (_, outer) = ui.allocate_space(ui.available_size().max(Vec2::ZERO));
        let rect = terminal_content_rect(outer, ui.clip_rect());
        let size = grid_size(rect.size(), cell, ui.ctx().pixels_per_point());
        let id = terminal_id(ui.ctx());
        let response = ui.interact(outer, id, Sense::click_and_drag());
        response.widget_info(|| {
            egui::WidgetInfo::labeled(egui::WidgetType::TextEdit, true, "Terminal input")
        });
        #[cfg(all(feature = "desktop-ui-tests", target_os = "macos"))]
        crate::desktop_test::observe("terminal.input", &response);
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
                    // A fresh shell starts its bell count over.
                    self.last_bell = 0;
                    self.bell_pending = false;
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
        if !self.update_colors(colors) {
            ui.ctx()
                .request_repaint_after(std::time::Duration::from_millis(50));
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
        // A program that enabled a mouse tracking mode owns the pointer: click,
        // drag, hover and wheel input are encoded to it instead of driving the
        // local selection and scrollback. Shift bypasses reporting so text can
        // still be selected, as on desktop terminals.
        let ppp = ui.ctx().pixels_per_point();
        let geometry = mouse_geometry(outer, rect, size, ppp);
        let (screen_w, screen_h) = (geometry.screen_width as f32, geometry.screen_height as f32);
        // Cmd+click follows links under the pointer instead of selecting or
        // reporting, even in mouse-tracking programs. Like the Shift bypass,
        // this matches desktop terminal behavior.
        let mut link_consumed = false;
        let bypass_gesture = self.suppress_select_until_release;
        let pointer_down = ui.input(|input| !input.pointer.any_down());
        if pointer_down {
            self.suppress_select_until_release = false;
        }
        if let Some(grid) = &self.grid
            && let Some(pos) = response.interact_pointer_pos()
            && ui.input(|input| input.pointer.primary_pressed() && input.modifiers.command)
            && let Some((action, _, _)) = Self::link_at(grid, rect, cell, pos)
        {
            // A consumed Cmd+click must not start a selection that the
            // following drag frames would then extend.
            self.suppress_select_until_release = true;
            self.selection = None;
            self.link_action = Some(match action {
                super::TerminalLinkAction::OpenPath { path, line, column } => {
                    let base = self
                        .session
                        .as_ref()
                        .map(|session| session.cwd.clone())
                        .or_else(|| self.fixture_cwd.clone())
                        .unwrap_or_else(|| cwd.to_owned());
                    super::TerminalLinkAction::OpenPath {
                        path: resolve_link_path(
                            &path,
                            &base,
                            std::env::var_os("HOME").as_deref().map(Path::new),
                        ),
                        line,
                        column,
                    }
                }
                url => url,
            });
            link_consumed = true;
        }
        let app_mouse = app_mouse_mode(self.grid.as_deref(), ui)
            && !bypass_gesture
            && !self.suppress_select_until_release;
        if !app_mouse {
            // Shift bypass, a tracking-mode change, or native focus loss can
            // happen without an egui pointer release; finish the program's drag.
            self.release_mouse(Some(ui.input(|input| input.modifiers)));
        }
        if app_mouse && !link_consumed {
            let button_events = ui.input(|input| {
                input
                    .events
                    .iter()
                    .filter_map(|event| match event {
                        egui::Event::PointerButton {
                            button,
                            pressed,
                            pos,
                            modifiers,
                            ..
                        } => Some((*button, *pressed, *pos, *modifiers)),
                        _ => None,
                    })
                    .collect::<Vec<_>>()
            });
            for (button, pressed, pos, modifiers) in button_events {
                let mapped = input::mouse_button(button);
                if pressed {
                    if let Some(mapped) = mapped
                        && response.contains_pointer()
                    {
                        self.mouse_held = Some(button);
                        self.motion_cell = None;
                        let (x, y) = surface_position(pos, outer, ppp, screen_w, screen_h);
                        self.report_mouse(MouseInput {
                            action: mouse::Action::Press,
                            button: Some(mapped),
                            mods: input::mods(modifiers),
                            x,
                            y,
                            geometry,
                            any_button_pressed: true,
                        });
                    }
                } else if self.mouse_held == Some(button) {
                    // Release a held button even outside the surface; a TUI
                    // drag-select otherwise never sees its ending event.
                    self.mouse_held = None;
                    self.motion_cell = None;
                    if let Some(mapped) = mapped {
                        let (x, y) = surface_position(pos, outer, ppp, screen_w, screen_h);
                        self.report_mouse(MouseInput {
                            action: mouse::Action::Release,
                            button: Some(mapped),
                            mods: input::mods(modifiers),
                            x,
                            y,
                            geometry,
                            any_button_pressed: false,
                        });
                    }
                }
            }
            if (response.hovered() || response.dragged())
                && let Some(pos) = response.interact_pointer_pos()
                && let Some(cell_pos) = pointer_cell(pos, rect, cell)
                && self.motion_cell != Some(cell_pos)
            {
                self.motion_cell = Some(cell_pos);
                let (x, y) = surface_position(pos, outer, ppp, screen_w, screen_h);
                self.report_mouse(MouseInput {
                    action: mouse::Action::Motion,
                    button: self.mouse_held.and_then(input::mouse_button),
                    mods: input::mods(ui.input(|input| input.modifiers)),
                    x,
                    y,
                    geometry,
                    any_button_pressed: self.mouse_held.is_some(),
                });
            }
        }
        if response.hovered() && ui.input(|input| input.focused) {
            let delta = ui.input_mut(|input| {
                let delta = input.smooth_scroll_delta.y;
                input.smooth_scroll_delta = Vec2::ZERO;
                delta
            });
            self.wheel_remainder -= delta / cell.y;
            let lines = self.wheel_remainder.trunc() as isize;
            if lines != 0 {
                self.wheel_remainder -= lines as f32;
                if app_mouse {
                    // Mouse-reporting programs receive wheel notches as
                    // button 4/5 presses, bounded per frame to protect the
                    // session's bounded input queue.
                    let button = input::wheel_button_for_lines(lines);
                    let mods = ui.input(|input| input.modifiers);
                    let pos = ui.input(|input| input.pointer.hover_pos());
                    for _ in 0..lines.unsigned_abs().min(MAX_WHEEL_EVENTS) {
                        let Some(pos) = pos else { break };
                        let (x, y) = surface_position(pos, outer, ppp, screen_w, screen_h);
                        self.report_mouse(MouseInput {
                            action: mouse::Action::Press,
                            button: Some(button),
                            mods: input::mods(mods),
                            x,
                            y,
                            geometry,
                            any_button_pressed: self.mouse_held.is_some(),
                        });
                    }
                } else {
                    self.send(Command::Scroll(lines));
                    self.selection = None;
                }
            }
        }
        if let Some(grid) = &self.grid {
            if !app_mouse
                && !link_consumed
                && !self.suppress_select_until_release
                && ui.input(|input| input.focused)
            {
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
                &mut self.display,
                &mut self.emoji,
            );
            self.paint_find_matches(ui, rect, cell, grid, colors.foreground);
            // Cmd-hover over a link gets a pointer cursor and an underline,
            // computed for the hovered row only.
            if response.hovered()
                && ui.input(|input| input.modifiers.command)
                && let Some(pos) = response.hover_pos()
                && let Some((_, row, columns)) = Self::link_at(grid, rect, cell, pos)
            {
                ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
                let painter = ui.painter().with_clip_rect(rect);
                let y = rect.min.y + (row as f32 + 1.0) * cell.y - 2.0;
                painter.line_segment(
                    [
                        Pos2::new(rect.min.x + columns.start as f32 * cell.x, y),
                        Pos2::new(rect.min.x + columns.end as f32 * cell.x, y),
                    ],
                    Stroke::new(1.0, colors.foreground),
                );
            }
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
            ui.painter().rect_filled(
                outer,
                0.0,
                crate::window_background::surface(ui, colors.background),
            );
        }
    }
}

fn resolve_link_path(path: &Path, cwd: &Path, home: Option<&Path>) -> PathBuf {
    if let Ok(relative) = path.strip_prefix("~")
        && let Some(home) = home
    {
        return home.join(relative);
    }
    cwd.join(path)
}

fn terminal_content_rect(outer: Rect, clip: Rect) -> Rect {
    Rect::from_min_max(
        Pos2::new((outer.left() + 6.0).min(outer.right()), outer.top()),
        outer.max,
    )
    .intersect(clip)
}

fn grid_size(available: Vec2, cell: Vec2, scale: f32) -> GridSize {
    GridSize {
        cols: (available.x / cell.x.max(1.0)).floor().clamp(2.0, 500.0) as u16,
        rows: (available.y / cell.y.max(1.0)).floor().clamp(1.0, 200.0) as u16,
        cell_width: (cell.x * scale).round().clamp(1.0, 512.0) as u16,
        cell_height: (cell.y * scale).round().clamp(1.0, 512.0) as u16,
    }
}

/// Wheel notches become at most this many reported button presses per frame,
/// keeping a fast flick from flooding the session's bounded input queue.
const MAX_WHEEL_EVENTS: usize = 8;

fn app_mouse_mode(grid: Option<&Grid>, ui: &egui::Ui) -> bool {
    grid.is_some_and(|grid| grid.mouse_tracking)
        && ui.input(|input| input.focused && !input.modifiers.shift)
}

/// The surface geometry Ghostty converts pointer positions with: the full
/// terminal area, its cell size, and the grid inset inside it.
fn mouse_geometry(outer: Rect, grid: Rect, size: GridSize, scale: f32) -> mouse::EncoderSize {
    let px = |value: f32| (value * scale).round().clamp(0.0, u32::MAX as f32) as u32;
    mouse::EncoderSize {
        screen_width: px(outer.width()).max(1),
        screen_height: px(outer.height()).max(1),
        cell_width: u32::from(size.cell_width.max(1)),
        cell_height: u32::from(size.cell_height.max(1)),
        padding_top: px(grid.min.y - outer.min.y),
        padding_bottom: px(outer.max.y - grid.max.y),
        padding_right: px(outer.max.x - grid.max.x),
        padding_left: px(grid.min.x - outer.min.x),
    }
}

fn surface_position(pos: Pos2, outer: Rect, scale: f32, width: f32, height: f32) -> (f32, f32) {
    (
        ((pos.x - outer.min.x) * scale).clamp(0.0, width),
        ((pos.y - outer.min.y) * scale).clamp(0.0, height),
    )
}

fn pointer_cell(pos: Pos2, rect: Rect, cell: Vec2) -> Option<(i32, i32)> {
    if !rect.contains(pos) || cell.x <= 0.0 || cell.y <= 0.0 {
        return None;
    }
    Some((
        ((pos.x - rect.min.x) / cell.x).floor() as i32,
        ((pos.y - rect.min.y) / cell.y).floor() as i32,
    ))
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
    entries: HashMap<(usize, [u32; 2], bool), FittedGlyph>,
}

struct FittedGlyph {
    // Retain the source so its pointer cannot be reused as another cache key.
    _source: Arc<egui::Galley>,
    galley: Arc<egui::Galley>,
    offset: Vec2,
}

impl GlyphCache {
    fn fit(&mut self, source: Arc<egui::Galley>, size: Vec2) -> (Arc<egui::Galley>, Vec2) {
        self.fit_kind(source, size, false)
    }

    fn fit_kind(
        &mut self,
        source: Arc<egui::Galley>,
        size: Vec2,
        icon: bool,
    ) -> (Arc<egui::Galley>, Vec2) {
        let (scale, offset) = if icon {
            icon_placement(source.mesh_bounds, size)
        } else {
            glyph_placement(source.mesh_bounds, size)
        };
        if scale == 1.0 && offset == Vec2::ZERO {
            return (source, offset);
        }
        let key = (
            Arc::as_ptr(&source) as usize,
            [size.x.to_bits(), size.y.to_bits()],
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
fn glyph_placement(bounds: Rect, size: Vec2) -> (f32, Vec2) {
    if !bounds.is_finite() || !bounds.is_positive() {
        return (1.0, Vec2::ZERO);
    }
    let available = Vec2::new(size.x.max(0.1), size.y.max(0.1));
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
fn icon_placement(bounds: Rect, size: Vec2) -> (f32, Vec2) {
    if !bounds.is_finite() || !bounds.is_positive() {
        return (1.0, Vec2::ZERO);
    }
    let width = size.x.max(0.1);
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

/// Retain text layouts for immutable Ghostty rows. The renderer continues to
/// submit ordinary text shapes, so font-atlas growth later in the editor pass
/// is handled by egui's tessellator rather than freezing normalized UV meshes.
#[derive(Default)]
struct RowDisplayCache {
    rows: HashMap<usize, RowDisplay>,
    metrics: Option<RowDisplayMetrics>,
    font_epoch: Option<Arc<egui::Galley>>,
    pass: u64,
}

#[derive(Clone, PartialEq)]
struct RowDisplayMetrics {
    cell: Vec2,
    font: FontId,
    pixels_per_point: f32,
    background: Color32,
}

struct RowDisplay {
    // Pin the row's allocation: an address may not be recycled as a cache key.
    _source: Arc<Vec<super::engine::Cell>>,
    backgrounds: Vec<egui::Shape>,
    ink: Vec<(Rect, egui::Shape)>,
    // Cached emoji shapes must outlive eviction from EmojiCache's bounded cache.
    _textures: Vec<egui::TextureHandle>,
    pass: u64,
}

impl RowDisplayCache {
    #[allow(clippy::too_many_arguments)]
    fn prepare(
        &mut self,
        ui: &egui::Ui,
        rect: Rect,
        cell: Vec2,
        font: &FontId,
        grid: &Grid,
        glyphs: &mut GlyphCache,
        emoji: &mut super::emoji::EmojiCache,
    ) {
        let metrics = RowDisplayMetrics {
            cell,
            font: font.clone(),
            pixels_per_point: ui.ctx().pixels_per_point(),
            background: grid.background,
        };
        // A cached lookup keeps this Galley alive. A rebuilt atlas or changed
        // fonts produces a different Arc, including atlas-fullness resets that
        // do not change the font definitions or the terminal cell dimensions.
        let epoch =
            ui.fonts_mut(|fonts| fonts.layout_no_wrap("M".into(), font.clone(), Color32::WHITE));
        if self.metrics.as_ref() != Some(&metrics)
            || self
                .font_epoch
                .as_ref()
                .is_none_or(|last| !Arc::ptr_eq(last, &epoch))
        {
            self.rows.clear();
            glyphs.entries.clear();
            self.metrics = Some(metrics);
            self.font_epoch = Some(epoch);
        }
        self.pass = self.pass.wrapping_add(1);
        let pass = self.pass;
        for (index, row) in grid.rows.iter().enumerate() {
            if !row_display_rect(row.len(), index, rect.min, cell).intersects(rect) {
                continue;
            }
            let key = Arc::as_ptr(row) as usize;
            self.rows
                .entry(key)
                .or_insert_with(|| {
                    build_row_display(ui, row.clone(), cell, font, grid.background, glyphs, emoji)
                })
                .pass = pass;
        }
        // A terminal holds only its current visible rows, never scroll history.
        self.rows.retain(|_, row| row.pass == pass);
    }

    fn row(&self, row: &Arc<Vec<super::engine::Cell>>) -> Option<&RowDisplay> {
        self.rows.get(&(Arc::as_ptr(row) as usize))
    }
}

fn row_display_rect(columns: usize, row: usize, origin: Pos2, cell: Vec2) -> Rect {
    Rect::from_min_size(
        origin + Vec2::new(0.0, row as f32 * cell.y),
        Vec2::new(columns as f32 * cell.x, cell.y),
    )
}

fn local_terminal_cell(col: usize, columns: f32, cell: Vec2) -> Rect {
    Rect::from_min_size(
        Pos2::new(col as f32 * cell.x, 0.0),
        Vec2::new(columns * cell.x, cell.y),
    )
}

fn terminal_foreground(cell: &super::engine::Cell) -> Color32 {
    if cell.style.faint {
        cell.foreground.gamma_multiply(0.65)
    } else {
        cell.foreground
    }
}

fn plain_ascii_cell(cell: &super::engine::Cell) -> bool {
    cell.width == 1
        && !cell.style.invisible
        // Italic overhang stays individually fitted and clipped as before.
        && !cell.style.italic
        && matches!(cell.text.as_bytes(), [b' '..=b'~'])
}

fn layout_terminal_text(
    ui: &egui::Ui,
    text: String,
    font: FontId,
    foreground: Color32,
) -> Arc<egui::Galley> {
    #[cfg(test)]
    ROW_LAYOUT_CALLS.with(|count| count.set(count.get() + 1));
    ui.painter()
        .layout_job(egui::text::LayoutJob::single_section(
            text,
            egui::TextFormat {
                font_id: font,
                color: foreground,
                ..Default::default()
            },
        ))
}

fn row_decorations(
    ink: &mut Vec<(Rect, egui::Shape)>,
    cell: &super::engine::Cell,
    rect: Rect,
    color: Color32,
) {
    if cell.style.underline != Underline::None {
        ink.push((
            Rect::EVERYTHING,
            egui::Shape::line_segment(
                [
                    rect.left_bottom() - Vec2::new(0.0, 1.0),
                    rect.right_bottom() - Vec2::new(0.0, 1.0),
                ],
                Stroke::new(1.0, color),
            ),
        ));
    }
    if cell.style.strikethrough {
        ink.push((
            Rect::EVERYTHING,
            egui::Shape::line_segment(
                [rect.left_center(), rect.right_center()],
                Stroke::new(1.0, color),
            ),
        ));
    }
}

fn build_row_display(
    ui: &egui::Ui,
    source: Arc<Vec<super::engine::Cell>>,
    cell_size: Vec2,
    font: &FontId,
    background: Color32,
    glyphs: &mut GlyphCache,
    emoji: &mut super::emoji::EmojiCache,
) -> RowDisplay {
    let mut backgrounds = Vec::new();
    let mut background_run: Option<(Rect, Color32, usize)> = None;
    for (col, cell) in source.iter().enumerate() {
        if cell.width == 0 {
            continue;
        }
        let rect = local_terminal_cell(col, f32::from(cell.width), cell_size);
        if let Some((run, color, end)) = &mut background_run
            && *color == cell.background
            && *end == col
        {
            run.max.x = rect.right();
            *end = col + usize::from(cell.width);
            continue;
        }
        if let Some((run, color, _)) = background_run.take()
            && color != background
        {
            backgrounds.push(egui::Shape::rect_filled(run, 0.0, color));
        }
        background_run = Some((rect, cell.background, col + usize::from(cell.width)));
    }
    if let Some((run, color, _)) = background_run
        && color != background
    {
        backgrounds.push(egui::Shape::rect_filled(run, 0.0, color));
    }

    let mut ink = Vec::new();
    let mut textures = Vec::new();
    let mut col = 0;
    while col < source.len() {
        let cell = &source[col];
        if cell.width == 0 || cell.style.invisible {
            col += 1;
            continue;
        }
        let foreground = terminal_foreground(cell);
        if plain_ascii_cell(cell) {
            let first = col;
            let mut text = cell.text.clone();
            col += 1;
            while let Some(next) = source.get(col) {
                if !plain_ascii_cell(next)
                    || next.style != cell.style
                    || next.foreground != cell.foreground
                {
                    break;
                }
                text.push_str(&next.text);
                col += 1;
            }
            let rect = local_terminal_cell(first, (col - first) as f32, cell_size);
            if !text.trim().is_empty() {
                let galley = layout_terminal_text(
                    ui,
                    text,
                    crate::theme::terminal_styled_font(cell.style.bold, false),
                    foreground,
                );
                // Preserve Ghostty's fixed column advances. Fitting an entire
                // run would shrink its spacing to compensate for one glyph's
                // overhang; clipping the run preserves the monospace grid.
                ink.push((rect, egui::Shape::galley(rect.min, galley, foreground)));
            }
            row_decorations(&mut ink, cell, rect, foreground);
            continue;
        }
        let rect = local_terminal_cell(col, f32::from(cell.width.max(1)), cell_size);
        if let Some(texture) = emoji.glyph(
            ui.ctx(),
            &cell.text,
            cell.width,
            font.size * ui.ctx().pixels_per_point(),
        ) {
            let size = texture.size_vec2();
            let scale = (rect.width() / size.x).min(rect.height() / size.y);
            let target = Rect::from_center_size(rect.center(), size * scale);
            let tint = if cell.style.faint {
                Color32::WHITE.gamma_multiply(0.65)
            } else {
                Color32::WHITE
            };
            ink.push((
                rect,
                egui::Shape::image(
                    texture.id(),
                    target,
                    Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)),
                    tint,
                ),
            ));
            textures.push(texture.clone());
        } else if !cell.text.is_empty() {
            let icon = terminal_icon(&cell.text);
            let ink_rect = if icon {
                local_terminal_cell(col, icon_columns(cell, source.get(col + 1)), cell_size)
            } else {
                rect
            };
            let family = if icon {
                crate::theme::terminal_icon_font(font.size * 1.5)
            } else {
                crate::theme::terminal_styled_font(cell.style.bold, cell.style.italic)
            };
            let galley = layout_terminal_text(ui, cell.text.clone(), family, foreground);
            let (galley, offset) = if icon {
                glyphs.fit_kind(galley, ink_rect.size(), true)
            } else {
                glyphs.fit(galley, ink_rect.size())
            };
            ink.push((
                ink_rect,
                egui::Shape::galley(ink_rect.min + offset, galley, foreground),
            ));
        }
        row_decorations(&mut ink, cell, rect, foreground);
        col += 1;
    }
    RowDisplay {
        _source: source,
        backgrounds,
        ink,
        _textures: textures,
        pass: 0,
    }
}

fn translated_cached_shape(source: &egui::Shape, delta: Vec2) -> egui::Shape {
    let mut shape = source.clone();
    if let egui::Shape::Text(text) = &mut shape {
        // TextShape::transform calls Arc::make_mut on the full Galley even for
        // a translation. Moving its origin alone preserves the retained layout.
        text.pos += delta;
    } else {
        shape.translate(delta);
    }
    shape
}

fn paint_terminal_selection(
    ui: &egui::Ui,
    rect: Rect,
    cell_size: Vec2,
    grid: &Grid,
    selection: Option<(usize, usize)>,
) {
    let Some((a, b)) = selection else {
        return;
    };
    let range = a.min(b)..a.max(b);
    let painter = ui.painter().with_clip_rect(rect);
    for (row_index, row) in grid.rows.iter().enumerate() {
        let start = row_index * usize::from(grid.cols);
        let from = range.start.saturating_sub(start).min(row.len());
        let to = range.end.saturating_sub(start).min(row.len());
        let slice = &row[from..to.max(from)];
        let Some(first) = slice.iter().position(|cell| cell.width != 0) else {
            continue;
        };
        let Some(last) = slice.iter().rposition(|cell| cell.width != 0) else {
            continue;
        };
        let left = from + first;
        let right = from + last + usize::from(slice[last].width);
        painter.rect_filled(
            Rect::from_min_size(
                rect.min + cell_size * Vec2::new(left as f32, row_index as f32),
                Vec2::new((right - left) as f32 * cell_size.x, cell_size.y),
            ),
            0.0,
            ui.visuals().selection.bg_fill,
        );
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
    display: &mut RowDisplayCache,
    emoji: &mut super::emoji::EmojiCache,
) {
    ui.painter().rect_filled(
        outer,
        0.0,
        crate::window_background::surface(ui, grid.background),
    );
    let painter = ui.painter().with_clip_rect(rect);
    display.prepare(ui, rect, cell_size, font, grid, glyphs, emoji);
    for (row_index, source) in grid.rows.iter().enumerate() {
        let Some(row) = display.row(source) else {
            continue;
        };
        let delta = rect.min.to_vec2() + Vec2::new(0.0, row_index as f32 * cell_size.y);
        for shape in &row.backgrounds {
            painter.add(translated_cached_shape(shape, delta));
        }
    }
    paint_terminal_selection(ui, rect, cell_size, grid, selection);
    for (row_index, source) in grid.rows.iter().enumerate() {
        let Some(row) = display.row(source) else {
            continue;
        };
        let delta = rect.min.to_vec2() + Vec2::new(0.0, row_index as f32 * cell_size.y);
        for (clip, shape) in &row.ink {
            let clip = clip.translate(delta).intersect(rect);
            if clip.is_positive() {
                painter
                    .with_clip_rect(clip)
                    .add(translated_cached_shape(shape, delta));
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
thread_local! {
    static ROW_LAYOUT_CALLS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

#[cfg(test)]
mod tests {
    use super::*;

    fn paint_fixture(pane: &mut TerminalPane, context: &egui::Context) -> egui::FullOutput {
        context.run_ui(Default::default(), |ui| {
            pane.show(ui, Path::new("/project"))
        })
    }

    fn render_fixture(output: &[u8]) -> (TerminalPane, egui::Context) {
        let context = egui::Context::default();
        crate::theme::configure_test_fonts(&context);
        let mut pane = TerminalPane::default();
        pane.prepare_fixture(output, Path::new("/project"));
        (pane, context)
    }

    #[test]
    fn terminal_grid_has_only_the_preserved_left_inset() {
        for size in [Vec2::new(800.0, 220.0), Vec2::new(320.0, 80.0)] {
            let outer = Rect::from_min_size(Pos2::new(12.0, 34.0), size);
            let content = terminal_content_rect(outer, outer);
            assert_eq!(content.left(), outer.left() + 6.0);
            assert_eq!(content.top(), outer.top());
            assert_eq!(content.bottom(), outer.bottom());
            assert_eq!(content.right(), outer.right());
            let geometry = mouse_geometry(
                outer,
                content,
                grid_size(content.size(), Vec2::new(8.0, 16.0), 2.0),
                2.0,
            );
            assert_eq!(geometry.padding_left, 12);
            assert_eq!(geometry.padding_top, 0);
            assert_eq!(geometry.padding_right, 0);
            assert_eq!(geometry.padding_bottom, 0);
        }
    }

    #[test]
    fn terminal_inset_stays_inside_tiny_and_clipped_surfaces() {
        let outer = Rect::from_min_size(Pos2::new(12.0, 34.0), Vec2::new(2.0, 1.0));
        let content = terminal_content_rect(outer, outer);
        assert!(outer.contains_rect(content));
        assert_eq!(content.width(), 0.0);
        assert_eq!(content.y_range(), outer.y_range());
        let outer = Rect::from_min_size(Pos2::new(12.0, 34.0), Vec2::new(300.0, 100.0));
        let clip = outer.shrink2(Vec2::new(10.0, 20.0));
        assert_eq!(terminal_content_rect(outer, clip), clip);
    }

    #[test]
    fn terminal_row_display_reuses_unchanged_text_on_editor_only_frames() {
        let (mut pane, context) = render_fixture(b"\x1b[?25labcdefghijklmnop");
        paint_fixture(&mut pane, &context).drop_without_applying_deltas();
        ROW_LAYOUT_CALLS.with(|count| count.set(0));
        for _ in 0..5 {
            paint_fixture(&mut pane, &context).drop_without_applying_deltas();
        }
        assert_eq!(
            ROW_LAYOUT_CALLS.with(std::cell::Cell::get),
            0,
            "unchanged terminal rows must not rebuild text when the editor repaints"
        );
    }

    #[test]
    fn terminal_row_display_merges_plain_ascii_runs() {
        let (mut pane, context) = render_fixture(b"\x1b[?25labcdefghijklmnop");
        let output = paint_fixture(&mut pane, &context);
        let texts = output
            .shapes
            .iter()
            .filter(|shape| matches!(shape.shape, egui::Shape::Text(_)))
            .count();
        output.drop_without_applying_deltas();
        assert!(
            texts <= 2,
            "one plain ASCII row should use a bounded number of text shapes; got {texts}"
        );
    }

    #[test]
    fn terminal_row_display_refreshes_only_dirty_rows_and_keeps_selection_dynamic() {
        let (mut pane, context) =
            render_fixture(b"\x1b[?25lfirst row\r\nsecond row\r\nthird row\x1b[2;1H");
        paint_fixture(&mut pane, &context).drop_without_applying_deltas();
        ROW_LAYOUT_CALLS.with(|count| count.set(0));
        pane.selection = Some((0, 5));
        pane.focus_requested = true;
        paint_fixture(&mut pane, &context).drop_without_applying_deltas();
        assert_eq!(
            ROW_LAYOUT_CALLS.with(std::cell::Cell::get),
            0,
            "selection and focus are overlays on retained terminal text"
        );
        // Keep the hidden cursor on the same row and overwrite the same
        // ten cells: no second cursor row or old-style tail should be dirty.
        pane.feed_fixture_for_test(b"\x1b[2;1Hchanged rw");
        paint_fixture(&mut pane, &context).drop_without_applying_deltas();
        assert_eq!(
            ROW_LAYOUT_CALLS.with(std::cell::Cell::get),
            1,
            "only the dirty row needs a new layout"
        );
    }

    #[test]
    fn terminal_row_display_invalidates_when_font_atlas_is_recreated() {
        let (mut pane, context) = render_fixture(b"\x1b[?25lretained row");
        paint_fixture(&mut pane, &context).drop_without_applying_deltas();
        let epoch = pane.display.font_epoch.clone().unwrap();
        let cell = pane.display.metrics.as_ref().unwrap().cell;
        crate::theme::recreate_test_font_atlas(&context);
        ROW_LAYOUT_CALLS.with(|count| count.set(0));
        paint_fixture(&mut pane, &context).drop_without_applying_deltas();
        assert_eq!(
            pane.display.metrics.as_ref().unwrap().cell,
            cell,
            "the cache must notice a rebuilt atlas even when cell metrics did not change"
        );
        assert!(!Arc::ptr_eq(
            &epoch,
            pane.display.font_epoch.as_ref().unwrap()
        ));
        assert!(ROW_LAYOUT_CALLS.with(std::cell::Cell::get) > 0);
        ROW_LAYOUT_CALLS.with(|count| count.set(0));
        paint_fixture(&mut pane, &context).drop_without_applying_deltas();
        assert_eq!(ROW_LAYOUT_CALLS.with(std::cell::Cell::get), 0);
    }

    #[test]
    fn terminal_row_display_invalidates_display_scale_and_colors() {
        let (mut pane, context) = render_fixture(b"\x1b[?25lscaled row");
        paint_fixture(&mut pane, &context).drop_without_applying_deltas();
        let epoch = pane.display.font_epoch.clone().unwrap();
        context.set_pixels_per_point(2.0);
        ROW_LAYOUT_CALLS.with(|count| count.set(0));
        paint_fixture(&mut pane, &context).drop_without_applying_deltas();
        assert!(!Arc::ptr_eq(
            &epoch,
            pane.display.font_epoch.as_ref().unwrap()
        ));
        assert!(ROW_LAYOUT_CALLS.with(std::cell::Cell::get) > 0);
        context.set_visuals(egui::Visuals::light());
        ROW_LAYOUT_CALLS.with(|count| count.set(0));
        paint_fixture(&mut pane, &context).drop_without_applying_deltas();
        assert!(
            ROW_LAYOUT_CALLS.with(std::cell::Cell::get) > 0,
            "palette switches must rebuild cached foreground and backgrounds"
        );
        ROW_LAYOUT_CALLS.with(|count| count.set(0));
        paint_fixture(&mut pane, &context).drop_without_applying_deltas();
        assert_eq!(ROW_LAYOUT_CALLS.with(std::cell::Cell::get), 0);
    }

    #[test]
    fn terminal_row_display_does_not_retain_old_output_history() {
        let (mut pane, context) = render_fixture(b"\x1b[?25lfirst row");
        paint_fixture(&mut pane, &context).drop_without_applying_deltas();
        for index in 0..100 {
            pane.feed_fixture_for_test(format!("\x1b[Hupdated row {index:03}").as_bytes());
            paint_fixture(&mut pane, &context).drop_without_applying_deltas();
            let grid = pane.grid.as_ref().unwrap();
            assert!(pane.display.rows.len() <= grid.rows.len());
            assert!(
                pane.display
                    .rows
                    .values()
                    .all(|entry| grid.rows.iter().any(|row| Arc::ptr_eq(row, &entry._source)))
            );
        }
    }

    #[test]
    fn terminal_row_display_merges_adjacent_cell_backgrounds() {
        let (mut pane, context) = render_fixture(b"\x1b[?25l\x1b[41mabcdefgh\x1b[0m");
        let output = paint_fixture(&mut pane, &context);
        let background = pane.grid.as_ref().unwrap().rows[0][0].background;
        let count = output
            .shapes
            .iter()
            .filter(
                |shape| matches!(&shape.shape, egui::Shape::Rect(rect) if rect.fill == background),
            )
            .count();
        output.drop_without_applying_deltas();
        assert_eq!(count, 1, "adjacent equal backgrounds share one rectangle");
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn terminal_row_display_retains_emoji_until_the_row_is_replaced() {
        let (mut pane, context) = render_fixture("\x1b[?25l😀".as_bytes());
        paint_fixture(&mut pane, &context).drop_without_applying_deltas();
        let texture = pane
            .display
            .rows
            .values()
            .flat_map(|row| &row._textures)
            .next()
            .expect("colored emoji row")
            .id();
        context
            .run_ui(Default::default(), |ui| {
                // More than the emoji cache's capacity of negative wide lookups
                // evicts the original handle, while its display row stays current.
                for character in '\u{4e00}'..'\u{5000}' {
                    let _ = pane.emoji.glyph(ui.ctx(), &character.to_string(), 2, 13.0);
                }
            })
            .drop_without_applying_deltas();
        assert!(
            context.tex_manager().read().meta(texture).is_some(),
            "a retained row must own its image even after glyph-cache eviction"
        );
        pane.feed_fixture_for_test(b"\x1b[2J\x1b[Hplain replacement");
        paint_fixture(&mut pane, &context).drop_without_applying_deltas();
        assert!(
            context.tex_manager().read().meta(texture).is_none(),
            "retired display rows must release their emoji images"
        );
    }

    #[test]
    fn terminal_row_display_keeps_unicode_and_italic_cells_individually_clipped() {
        let (mut pane, context) =
            render_fixture("\x1b[?25l\x1b[3mitalic\x1b[0m界e\u{301}\u{f120} ".as_bytes());
        paint_fixture(&mut pane, &context).drop_without_applying_deltas();
        let grid = pane.grid.as_ref().unwrap();
        let display = pane.display.row(&grid.rows[0]).unwrap();
        let cell = pane.display.metrics.as_ref().unwrap().cell;
        let text_clips: Vec<_> = display
            .ink
            .iter()
            .filter_map(|(clip, shape)| matches!(shape, egui::Shape::Text(_)).then_some(*clip))
            .collect();
        assert_eq!(text_clips.len(), 9);
        for clip in &text_clips[..6] {
            assert!(
                (clip.width() - cell.x).abs() < 0.001,
                "italic letters retain their one-cell clipping"
            );
        }
        assert!(
            (text_clips[6].width() - cell.x * 2.0).abs() < 0.001,
            "wide Unicode respects Ghostty's two-column allocation"
        );
        assert!(
            (text_clips[7].width() - cell.x).abs() < 0.001,
            "combining marks keep the original grapheme's one column"
        );
        assert!(
            (text_clips[8].width() - cell.x * 2.0).abs() < 0.001,
            "private-use icons may use the following blank as before"
        );
        ROW_LAYOUT_CALLS.with(|count| count.set(0));
        paint_fixture(&mut pane, &context).drop_without_applying_deltas();
        assert_eq!(ROW_LAYOUT_CALLS.with(std::cell::Cell::get), 0);
    }

    #[test]
    fn terminal_ascii_runs_preserve_fixed_column_advances_at_every_scale() {
        let text = "  AVMW{}()i|_,.  ";
        for scale in [1.0, 1.25, 1.5, 2.0] {
            for bold in [false, true] {
                let output = format!("\x1b[?25l{}{}", if bold { "\x1b[1m" } else { "" }, text);
                let (mut pane, context) = render_fixture(output.as_bytes());
                context.set_pixels_per_point(scale);
                paint_fixture(&mut pane, &context).drop_without_applying_deltas();
                let grid = pane.grid.as_ref().unwrap();
                let row = pane.display.row(&grid.rows[0]).unwrap();
                let cell = pane.display.metrics.as_ref().unwrap().cell;
                let text_shapes: Vec<_> = row
                    .ink
                    .iter()
                    .filter_map(|(_, shape)| {
                        if let egui::Shape::Text(text) = shape {
                            Some(text)
                        } else {
                            None
                        }
                    })
                    .collect();
                assert_eq!(text_shapes.len(), 1);
                let shape = text_shapes[0];
                assert_eq!(
                    shape.pos.x, 0.0,
                    "leading spaces are part of the fixed column run"
                );
                let glyphs: Vec<_> = shape
                    .galley
                    .rows
                    .iter()
                    .flat_map(|row| &row.glyphs)
                    .collect();
                assert_eq!(glyphs.len(), text.chars().count());
                for (column, glyph) in glyphs.iter().enumerate() {
                    assert!(
                        (glyph.advance_width - cell.x).abs() < 0.001,
                        "ASCII run must never scale its column advances: bold={bold} scale={scale} char={:?} advance={} column_width={}",
                        glyph.chr,
                        glyph.advance_width,
                        cell.x
                    );
                    assert!(
                        (glyph.pos.x - column as f32 * cell.x).abs() <= 1.0 / scale,
                        "shaped glyph must stay at its Ghostty column: bold={bold} scale={scale} column={column} x={} cell={}",
                        glyph.pos.x,
                        cell.x
                    );
                }
            }
        }
    }

    #[test]
    fn terminal_row_display_translation_preserves_shared_galley() {
        let context = egui::Context::default();
        context
            .run_ui(Default::default(), |ui| {
                let galley = ui.painter().layout_no_wrap(
                    "retained".into(),
                    FontId::monospace(13.0),
                    Color32::WHITE,
                );
                let bounds = galley.mesh_bounds;
                let source =
                    egui::Shape::galley(Pos2::new(3.0, 4.0), galley.clone(), Color32::WHITE);
                let translated = translated_cached_shape(&source, Vec2::new(10.0, 20.0));
                let egui::Shape::Text(text) = translated else {
                    panic!("text shape");
                };
                assert!(Arc::ptr_eq(&text.galley, &galley));
                assert_eq!(text.pos, Pos2::new(13.0, 24.0));
                assert_eq!(text.galley.mesh_bounds, bounds);
            })
            .drop_without_applying_deltas();
    }

    #[test]
    fn mouse_geometry_and_surface_positions_map_pointer_pixels() {
        let size = GridSize {
            cols: 10,
            rows: 4,
            cell_width: 16,
            cell_height: 32,
        };
        let outer = Rect::from_min_size(Pos2::new(10.0, 20.0), Vec2::new(200.0, 160.0));
        let grid = Rect::from_min_size(Pos2::new(18.0, 20.0), Vec2::new(192.0, 160.0));
        let geometry = mouse_geometry(outer, grid, size, 2.0);
        assert_eq!(geometry.screen_width, 400);
        assert_eq!(geometry.screen_height, 320);
        assert_eq!(geometry.cell_width, 16);
        assert_eq!(geometry.cell_height, 32);
        assert_eq!(geometry.padding_left, 16);
        assert_eq!(geometry.padding_top, 0);
        assert_eq!(geometry.padding_right, 0);
        assert_eq!(geometry.padding_bottom, 0);
        assert_eq!(
            surface_position(Pos2::new(18.0, 20.0), outer, 2.0, 400.0, 320.0),
            (16.0, 0.0)
        );
        assert_eq!(
            surface_position(Pos2::new(999.0, -20.0), outer, 2.0, 400.0, 320.0),
            (400.0, 0.0)
        );
    }

    #[test]
    fn app_mouse_mode_requires_tracking_and_shift_bypasses_it() {
        let mut engine = Engine::new(
            GridSize {
                cols: 20,
                rows: 4,
                cell_width: 8,
                cell_height: 16,
            },
            Colors {
                foreground: Color32::WHITE,
                background: Color32::BLACK,
            },
        )
        .unwrap();
        let idle = engine.snapshot().unwrap();
        let tracking_input = |shift: bool| egui::RawInput {
            events: vec![egui::Event::ModifiersChanged(egui::Modifiers {
                shift,
                ..Default::default()
            })],
            ..Default::default()
        };
        for shift in [false, true] {
            let context = egui::Context::default();
            context
                .run_ui(tracking_input(shift), |ui| {
                    assert!(!app_mouse_mode(Some(&idle), ui));
                })
                .drop_without_applying_deltas();
        }
        engine.terminal.vt_write(b"\x1b[?1002;1006h");
        let tracking = engine.snapshot().unwrap();
        for shift in [false, true] {
            let context = egui::Context::default();
            context
                .run_ui(tracking_input(shift), |ui| {
                    assert_eq!(app_mouse_mode(Some(&tracking), ui), !shift);
                })
                .drop_without_applying_deltas();
        }
    }

    #[test]
    fn mouse_reporting_pane_clicks_do_not_start_a_local_selection() {
        use egui_kittest::{Harness, kittest::Queryable as _};
        let mut pane = TerminalPane::default();
        pane.prepare_fixture(b"\x1b[?1002;1006hreporting", Path::new("/project"));
        let mut configured = false;
        let mut harness = Harness::builder()
            .with_size(Vec2::new(500.0, 220.0))
            .build_ui_state(
                move |ui, pane: &mut TerminalPane| {
                    if !configured {
                        crate::theme::configure_test_fonts(ui.ctx());
                        configured = true;
                        // Definitions apply next frame; painting now panics.
                        return;
                    }
                    pane.show(ui, Path::new("/project"));
                },
                pane,
            );
        harness.run_steps(2);
        assert!(
            harness
                .state()
                .grid
                .as_ref()
                .is_some_and(|grid| grid.mouse_tracking)
        );
        harness.get_by_label("Terminal input").click();
        harness.run_steps(1);
        assert!(
            harness.state().selection.is_none(),
            "a reporting program receives clicks; local selection must not start"
        );
    }

    #[test]
    fn palette_queue_backpressure_retries_once_instead_of_marking_unsent_colors_current() {
        let (session, commands) = Session::search_test_session();
        for _ in 0..64 {
            session.send(Command::Wake).unwrap();
        }
        let mut pane = TerminalPane::with_test_session(session);
        let colors = Colors {
            foreground: Color32::BLACK,
            background: Color32::WHITE,
        };
        assert!(!pane.update_colors(colors));
        assert_eq!(
            pane.colors, None,
            "a full mailbox did not accept the palette change"
        );
        assert_eq!(commands.try_iter().count(), 64);
        assert!(pane.update_colors(colors));
        assert!(matches!(commands.try_recv(), Ok(Command::Colors(sent)) if sent == colors));
        assert_eq!(pane.colors, Some(colors));
        pane.update_colors(colors);
        assert!(
            commands.try_recv().is_err(),
            "unchanged frames must not enqueue palette work"
        );
    }

    fn pressed_reporting_pane() -> (
        TerminalPane,
        egui::Context,
        std::sync::mpsc::Receiver<Command>,
    ) {
        let context = egui::Context::default();
        crate::theme::configure_test_fonts(&context);
        let (session, receiver) = Session::search_test_session();
        let mut pane = TerminalPane::default();
        pane.prepare_fixture(b"\x1b[?1002;1006hreporting", Path::new("/project"));
        pane.session = Some(session);
        let mut frame = |events| {
            context
                .run_ui(
                    egui::RawInput {
                        screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(500.0, 220.0))),
                        events,
                        ..Default::default()
                    },
                    |ui| pane.show(ui, Path::new("/project")),
                )
                .drop_without_applying_deltas();
        };
        frame(Vec::new());
        let pos = Pos2::new(80.0, 80.0);
        frame(vec![
            egui::Event::PointerMoved(pos),
            egui::Event::PointerButton {
                pos,
                button: egui::PointerButton::Primary,
                pressed: true,
                modifiers: egui::Modifiers::NONE,
            },
        ]);
        assert_eq!(pane.mouse_held, Some(egui::PointerButton::Primary));
        assert!(receiver.try_iter().any(|command| matches!(command, Command::Mouse(input) if input.action == mouse::Action::Press)));
        (pane, context, receiver)
    }

    #[test]
    fn hiding_a_reporting_pane_releases_the_drag_and_forgets_pointer_state() {
        let (mut pane, _context, receiver) = pressed_reporting_pane();
        let last = pane.last_mouse_input.unwrap();
        pane.set_visible(false);
        let events = receiver.try_iter().collect::<Vec<_>>();
        let released = events
            .iter()
            .find_map(|command| match command {
                Command::Mouse(input) if input.action == mouse::Action::Release => Some(input),
                _ => None,
            })
            .expect("hiding a pressed pane must release the program's mouse button");
        assert!(!released.any_button_pressed);
        assert_eq!((released.x, released.y), (last.x, last.y));
        assert_eq!(released.geometry.cell_width, last.geometry.cell_width);
        assert_eq!(released.geometry.padding_top, last.geometry.padding_top);
        assert!(pane.mouse_held.is_none());
        assert!(pane.motion_cell.is_none());
        pane.set_visible(false);
        assert!(
            !receiver
                .try_iter()
                .any(|command| matches!(command, Command::Mouse(_))),
            "repeated hidden frames must not send duplicate releases"
        );
    }

    #[test]
    fn losing_native_focus_releases_a_reporting_drag_without_waiting_for_pointer_up() {
        let (mut pane, context, receiver) = pressed_reporting_pane();
        context
            .run_ui(
                egui::RawInput {
                    screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(500.0, 220.0))),
                    focused: false,
                    ..Default::default()
                },
                |ui| pane.show(ui, Path::new("/project")),
            )
            .drop_without_applying_deltas();
        assert!(receiver.try_iter().any(|command| matches!(command, Command::Mouse(input) if input.action == mouse::Action::Release)), "switching apps must not leave the program's mouse button pressed");
        assert!(pane.mouse_held.is_none());
    }

    #[test]
    #[cfg(unix)]
    fn reporting_pane_click_reaches_a_live_tracking_program() {
        use crate::terminal::session::Session;
        use egui_kittest::{Harness, kittest::Queryable as _};
        use portable_pty::CommandBuilder;
        use std::time::{Duration, Instant};
        let root = tempfile::tempdir().unwrap();
        let mut command = CommandBuilder::new("/bin/sh");
        command.args([
            "-c",
            "stty -echo; stty -icanon min 1; printf '\\033[?1002;1006hREADY'; dd bs=1 count=9 2>/dev/null | od -An -t x1",
        ]);
        command.cwd(root.path());
        let session = Session::start_command(
            &egui::Context::default(),
            root.path(),
            GridSize {
                cols: 80,
                rows: 8,
                cell_width: 8,
                cell_height: 16,
            },
            Colors {
                foreground: Color32::WHITE,
                background: Color32::BLACK,
            },
            command,
        )
        .unwrap();
        let pane = TerminalPane {
            session: Some(session),
            ..TerminalPane::default()
        };
        let mut configured = false;
        let mut harness = Harness::builder()
            .with_size(Vec2::new(700.0, 200.0))
            .build_ui_state(
                move |ui, pane: &mut TerminalPane| {
                    if !configured {
                        crate::theme::configure_test_fonts(ui.ctx());
                        configured = true;
                        // Definitions apply next frame; painting now panics.
                        return;
                    }
                    pane.sync_snapshot();
                    pane.show(ui, Path::new("/project"));
                },
                pane,
            );
        let deadline = Instant::now() + Duration::from_secs(10);
        let grid_text = |harness: &Harness<'_, TerminalPane>| {
            harness
                .state()
                .grid
                .as_ref()
                .map(|grid| {
                    grid.rows
                        .iter()
                        .flat_map(|row| row.iter().map(|cell| cell.text.as_str()))
                        .collect::<String>()
                })
                .unwrap_or_default()
        };
        loop {
            harness.run_steps(2);
            if grid_text(&harness).contains("READY") {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "tracking program did not start: {}",
                grid_text(&harness)
            );
        }
        harness.get_by_label("Terminal input").click();
        loop {
            harness.run_steps(2);
            // The click's SGR press report always opens with button 0.
            if grid_text(&harness)
                .split_whitespace()
                .collect::<Vec<_>>()
                .join(" ")
                .contains("1b 5b 3c 30 3b")
            {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "click never reached the program: {}",
                grid_text(&harness)
            );
        }
    }

    #[test]
    #[cfg(unix)]
    fn shift_mid_drag_releases_the_button_to_the_program() {
        use crate::terminal::session::Session;
        use egui_kittest::{Harness, kittest::Queryable as _};
        use portable_pty::CommandBuilder;
        use std::time::{Duration, Instant};
        let root = tempfile::tempdir().unwrap();
        let mut command = CommandBuilder::new("/bin/sh");
        // Consume SGR reports until one ends in a release, then confirm.
        command.args([
            "-c",
            "stty -echo; stty -icanon min 1; printf '\\033[?1002;1006hREADY'; while true; do out=''; while IFS= read -r -n1 -d '' ch; do out=\"$out$ch\"; case \"$ch\" in M|m) break;; esac; done; printf '%s' \"$out\" | od -An -t x1; case \"$out\" in *m) break;; esac; done; printf DONE",
        ]);
        command.cwd(root.path());
        let session = Session::start_command(
            &egui::Context::default(),
            root.path(),
            GridSize {
                cols: 80,
                rows: 8,
                cell_width: 8,
                cell_height: 16,
            },
            Colors {
                foreground: Color32::WHITE,
                background: Color32::BLACK,
            },
            command,
        )
        .unwrap();
        let pane = TerminalPane {
            session: Some(session),
            ..TerminalPane::default()
        };
        let mut configured = false;
        let mut harness = Harness::builder()
            .with_size(Vec2::new(700.0, 200.0))
            .build_ui_state(
                move |ui, pane: &mut TerminalPane| {
                    if !configured {
                        crate::theme::configure_test_fonts(ui.ctx());
                        configured = true;
                        // Definitions apply next frame; painting now panics.
                        return;
                    }
                    pane.sync_snapshot();
                    pane.show(ui, Path::new("/project"));
                },
                pane,
            );
        let deadline = Instant::now() + Duration::from_secs(10);
        let grid_text = |harness: &Harness<'_, TerminalPane>| {
            harness
                .state()
                .grid
                .as_ref()
                .map(|grid| {
                    grid.rows
                        .iter()
                        .flat_map(|row| row.iter().map(|cell| cell.text.as_str()))
                        .collect::<String>()
                })
                .unwrap_or_default()
        };
        loop {
            harness.run_steps(2);
            if grid_text(&harness).contains("READY") {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "tracking program did not start: {}",
                grid_text(&harness)
            );
        }
        let surface = harness.get_by_label("Terminal input").rect();
        harness.drag_at(surface.center());
        harness.run_steps(2);
        harness.hover_at(surface.center() + Vec2::new(30.0, 20.0));
        harness.run_steps(2);
        // Shift bypasses reporting mid-drag; the held button must still be
        // released to the program instead of sticking down.
        harness.event(egui::Event::ModifiersChanged(egui::Modifiers {
            shift: true,
            ..Default::default()
        }));
        harness.run_steps(2);
        harness.event(egui::Event::ModifiersChanged(egui::Modifiers::default()));
        loop {
            harness.run_steps(2);
            let normalized = grid_text(&harness)
                .split_whitespace()
                .collect::<Vec<_>>()
                .join(" ");
            if grid_text(&harness).contains("DONE") && normalized.contains("6d") {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "drag release never reached the program: {}",
                grid_text(&harness)
            );
        }
    }

    #[test]
    fn icon_ink_is_normalized_without_overwriting_adjacent_text() {
        for width in [8.0, 16.0] {
            let bounds = Rect::from_min_max(Pos2::new(2.0, 4.0), Pos2::new(7.0, 10.0));
            let size = Vec2::new(width, 16.0);
            let (scale, offset) = icon_placement(bounds, size);
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
                            let (scale, offset) =
                                icon_placement(galley.mesh_bounds, Vec2::new(width * 2.0, 16.0));
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
            black_box(cache.fit(galley.clone(), size));
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
                black_box(cache.fit(galley.clone(), size));
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
                let cell = Vec2::new(7.82666 * columns, 16.0);
                let (scale, offset) = glyph_placement(bounds, cell);
                let ink = (bounds * scale).translate(offset);
                let available = Rect::from_min_size(Pos2::ZERO, cell).expand(0.001);
                assert!(available.contains_rect(ink));
                assert!(scale > 0.0 && scale <= 1.0);
            }
        }
        assert_eq!(
            glyph_placement(Rect::NOTHING, Vec2::ZERO),
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
                let first = cache.fit(source.clone(), size).0;
                for _ in 0..100 {
                    assert!(Arc::ptr_eq(&first, &cache.fit(source.clone(), size).0));
                }
                assert_eq!(cache.entries.len(), 1);
                let icon = cache.fit_kind(source.clone(), size, true).0;
                assert!(!Arc::ptr_eq(&first, &icon));
                assert!(Arc::ptr_eq(
                    &icon,
                    &cache.fit_kind(source.clone(), size, true).0
                ));
                let recolored =
                    ui.painter()
                        .layout_no_wrap("😀".into(), FontId::monospace(13.0), Color32::RED);
                assert!(!Arc::ptr_eq(&first, &cache.fit(recolored, size).0));
                for index in 0..GLYPH_CACHE_CAPACITY * 2 {
                    cache.fit(source.clone(), Vec2::new(1.0 + index as f32 / 100.0, 16.0));
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
    fn terminal_styled_faces_share_advances_and_resolve_bundled_hack() {
        use crate::theme::terminal_styled_font;
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
        context
            .run_ui(Default::default(), |ui| {
                ui.fonts_mut(|fonts| {
                    let regular = terminal_styled_font(false, false);
                    let width = fonts.glyph_width(&regular, 'M');
                    assert!(width > 0.0);
                    for (bold, italic) in [(true, false), (false, true), (true, true)] {
                        let face = terminal_styled_font(bold, italic);
                        assert_ne!(
                            face.family,
                            regular.family,
                            "bold={bold} italic={italic} must select its own face"
                        );
                        for character in ['M', 'i', ' ', '~'] {
                            let advance = fonts.glyph_width(&face, character);
                            assert!(
                                (advance - width).abs() < 0.01,
                                "bold={bold} italic={italic} {character:?} advances {advance}, but cells are {width} wide"
                            );
                        }
                        // The styled face carries real ink, not a fallback blank.
                        let galley = fonts.layout_no_wrap(
                            "Ag".into(),
                            face.clone(),
                            Color32::WHITE,
                        );
                        let uv = galley.rows[0].glyphs[0].uv_rect;
                        let atlas = fonts.image();
                        assert!(
                            (uv.min[1]..uv.max[1]).any(|y| (uv.min[0]..uv.max[0])
                                .any(|x| atlas[(x as usize, y as usize)].a() != 0)),
                            "bold={bold} italic={italic} has no visible ink"
                        );
                    }
                });
            })
            .drop_without_applying_deltas();
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

    #[test]
    fn find_bar_counts_steps_and_closes() {
        use egui_kittest::{Harness, kittest::Queryable as _};
        let mut pane = TerminalPane::default();
        pane.prepare_fixture(b"error one\nok\nerror two\n", Path::new("/project"));
        let mut configured = false;
        let mut harness = Harness::builder()
            .with_size(Vec2::new(500.0, 220.0))
            .build_ui_state(
                move |ui, pane: &mut TerminalPane| {
                    if !configured {
                        crate::theme::configure_test_fonts(ui.ctx());
                        configured = true;
                        return;
                    }
                    pane.show(ui, Path::new("/project"));
                    let _ = ui.ctx().read_response(terminal_find_id(ui.ctx()));
                },
                pane,
            );
        harness.run_steps(2);
        harness.state_mut().open_find();
        harness.run_steps(1);
        // The bar is visible with an empty query and no count yet.
        assert!(harness.query_by_label_contains("Find").is_some());
        // A query fills matches from the scrollback dump; stepping moves the
        // count and Escape closes the bar.
        harness.state_mut().find.dump = vec!["error one".into(), "ok".into(), "error two".into()];
        harness.state_mut().find.query = "error".into();
        harness.state_mut().find.mark_query_dirty();
        harness.run_steps(2);
        harness.get_by_label_contains("1/2");
        harness.run_steps(1);
        // Next steps the selection; the close button dismisses the bar.
        harness.get_by_label("Next terminal match").click();
        harness.run_steps(2);
        harness.get_by_label_contains("2/2");
        let find_id = terminal_find_id(&harness.ctx);
        assert!(harness.ctx.memory(|memory| memory.has_focus(find_id)));
        harness.get_by_label("Close terminal find").click();
        harness.run_steps(2);
        assert!(harness.query_by_label_contains("Find").is_none());
    }

    #[test]
    fn terminal_find_bounds_large_unicode_pastes_without_losing_focus() {
        use egui_kittest::{Harness, kittest::Queryable as _};
        let mut pane = TerminalPane::default();
        pane.prepare_fixture(b"DO NOT MODIFY", Path::new("/project"));
        let mut configured = false;
        let mut harness = Harness::builder()
            .with_size(Vec2::new(500.0, 220.0))
            .build_ui_state(
                move |ui, pane: &mut TerminalPane| {
                    if !configured {
                        crate::theme::configure_test_fonts(ui.ctx());
                        configured = true;
                        return;
                    }
                    pane.show(ui, Path::new("/project"));
                },
                pane,
            );
        harness.run_steps(2);
        harness.state_mut().open_find();
        harness.run_steps(2);
        harness.get_by_label("Search terminal scrollback").click();
        harness.run_steps(2);
        harness.event(egui::Event::Paste(
            "é".repeat(super::super::find::MAX_QUERY_CHARS * 8),
        ));
        harness.run_steps(2);
        let expected = "é".repeat(super::super::find::MAX_QUERY_CHARS);
        assert_eq!(
            harness.state().find.query,
            expected,
            "limit counts characters, not UTF-8 bytes"
        );
        // A further insertion at the limit is ignored while the field keeps
        // keyboard ownership; neither event reaches the terminal grid.
        harness.event(egui::Event::Text("Z".into()));
        harness.run_steps(2);
        assert_eq!(harness.state().find.query, expected);
        assert!(harness.state().find.is_open());
        let field_id = terminal_find_id(&harness.ctx);
        assert!(harness.ctx.memory(|memory| memory.has_focus(field_id)));
        let first_row = harness.state().grid.as_ref().unwrap().rows[0]
            .iter()
            .map(|cell| cell.text.as_str())
            .collect::<String>();
        assert_eq!(first_row.trim_end(), "DO NOT MODIFY");
    }

    #[test]
    fn escape_closes_find_only_when_its_field_focused() {
        let context = egui::Context::default();
        crate::theme::configure_test_fonts(&context);
        let mut pane = TerminalPane::default();
        pane.prepare_fixture(b"error one\nok\nerror two\n", Path::new("/project"));
        let frame = |pane: &mut TerminalPane, events: Vec<egui::Event>| {
            context
                .run_ui(
                    egui::RawInput {
                        screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(500.0, 220.0))),
                        events,
                        ..Default::default()
                    },
                    |ui| pane.show(ui, Path::new("/project")),
                )
                .drop_without_applying_deltas();
        };
        pane.open_find();
        frame(&mut pane, Vec::new());
        // Opening focuses the field, so Escape closes the bar.
        let field_id = super::super::terminal_id(&context).with("find");
        context.memory_mut(|memory| memory.request_focus(field_id));
        frame(&mut pane, Vec::new());
        frame(
            &mut pane,
            vec![egui::Event::Key {
                key: egui::Key::Escape,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: egui::Modifiers::default(),
            }],
        );
        assert!(!pane.find_open());
        // With focus elsewhere the shell keeps Escape and the bar stays.
        pane.open_find();
        frame(&mut pane, Vec::new());
        context.memory_mut(|memory| memory.surrender_focus(field_id));
        frame(
            &mut pane,
            vec![egui::Event::Key {
                key: egui::Key::Escape,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: egui::Modifiers::default(),
            }],
        );
        assert!(pane.find_open());
    }

    #[test]
    fn enter_steps_terminal_find_once_in_each_direction() {
        let context = egui::Context::default();
        crate::theme::configure_test_fonts(&context);
        let mut pane = TerminalPane::default();
        pane.prepare_fixture(b"one\r\ntwo\r\nthree", Path::new("/project"));
        pane.open_find();
        pane.find.dump = vec!["match one".into(), "match two".into(), "match three".into()];
        pane.find.query = "match".into();
        pane.find.mark_query_dirty();
        let frame = |pane: &mut TerminalPane, events| {
            context
                .run_ui(
                    egui::RawInput {
                        screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(500.0, 220.0))),
                        events,
                        ..Default::default()
                    },
                    |ui| pane.show(ui, Path::new("/project")),
                )
                .drop_without_applying_deltas();
        };
        frame(&mut pane, Vec::new());
        frame(&mut pane, Vec::new());
        assert_eq!(pane.find.selected_index(), Some(0));
        for (shift, selected) in [(false, 1), (true, 0), (true, 2)] {
            let modifiers = egui::Modifiers {
                shift,
                ..Default::default()
            };
            frame(
                &mut pane,
                vec![egui::Event::Key {
                    key: egui::Key::Enter,
                    physical_key: None,
                    pressed: true,
                    repeat: false,
                    modifiers,
                }],
            );
            assert_eq!(pane.find.selected_index(), Some(selected));
            frame(
                &mut pane,
                vec![egui::Event::Key {
                    key: egui::Key::Enter,
                    physical_key: None,
                    pressed: false,
                    repeat: false,
                    modifiers,
                }],
            );
        }
    }

    #[test]
    fn terminal_paths_resolve_relative_absolute_and_home_locations() {
        let cwd = Path::new("/project");
        let home = Some(Path::new("/home/user"));
        for (input, expected) in [
            ("src/main.rs", "/project/src/main.rs"),
            ("/tmp/a.tex", "/tmp/a.tex"),
            ("~/a.tex", "/home/user/a.tex"),
            ("~other/a.tex", "/project/~other/a.tex"),
        ] {
            assert_eq!(
                resolve_link_path(Path::new(input), cwd, home),
                Path::new(expected)
            );
        }
    }

    #[test]
    fn command_click_on_a_url_reports_an_open_action() {
        let context = egui::Context::default();
        crate::theme::configure_test_fonts(&context);
        let mut pane = TerminalPane::default();
        pane.prepare_fixture(
            b"\x1b[?1003h\x1b[?1006hsee https://example.com/a",
            Path::new("/project"),
        );
        let frame = |pane: &mut TerminalPane, events: Vec<egui::Event>| {
            context
                .run_ui(
                    egui::RawInput {
                        screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(500.0, 220.0))),
                        events,
                        ..Default::default()
                    },
                    |ui| pane.show(ui, Path::new("/project")),
                )
                .drop_without_applying_deltas();
        };
        frame(&mut pane, Vec::new());
        // Click the middle of the URL with Command held: the press must open
        // the link, not start a selection.
        let font = crate::theme::terminal_font();
        let cell = context.fonts_mut(|fonts| {
            Vec2::new(
                fonts.glyph_width(&font, 'M'),
                fonts.row_height(&font).ceil(),
            )
        });
        let pos = Pos2::new(6.0 + 14.5 * cell.x, 6.0 + 0.5 * cell.y);
        frame(
            &mut pane,
            vec![
                egui::Event::PointerMoved(pos),
                egui::Event::ModifiersChanged(egui::Modifiers {
                    command: true,
                    ..Default::default()
                }),
                egui::Event::PointerButton {
                    pos,
                    button: egui::PointerButton::Primary,
                    pressed: true,
                    modifiers: egui::Modifiers {
                        command: true,
                        ..Default::default()
                    },
                },
            ],
        );
        assert_eq!(
            pane.take_link_action(),
            Some(super::super::TerminalLinkAction::OpenUrl(
                "https://example.com/a".into()
            ))
        );
        assert!(
            pane.selection.is_none(),
            "a consumed Cmd+click must not select"
        );
        assert!(pane.suppress_select_until_release);
        assert!(pane.mouse_held.is_none());
        let moved = pos + Vec2::new(30.0, 0.0);
        frame(&mut pane, vec![egui::Event::PointerMoved(moved)]);
        assert!(pane.selection.is_none());
        assert!(
            pane.motion_cell.is_none(),
            "Cmd+drag must not reach a mouse-tracking program"
        );
        frame(
            &mut pane,
            vec![egui::Event::PointerButton {
                pos: moved,
                button: egui::PointerButton::Primary,
                pressed: false,
                modifiers: egui::Modifiers::default(),
            }],
        );
        assert!(!pane.suppress_select_until_release);
        assert!(
            pane.motion_cell.is_none(),
            "Cmd+release must not reach a mouse-tracking program"
        );
    }

    #[test]
    fn link_mapping_resolves_cells_to_targets() {
        let mut pane = TerminalPane::default();
        pane.prepare_fixture(b"see https://example.com/a", Path::new("/project"));
        let grid = pane.fixture.as_mut().unwrap().snapshot().unwrap();
        let rect = Rect::from_min_size(Pos2::ZERO, Vec2::new(400.0, 100.0));
        let cell = Vec2::new(8.0, 16.0);
        // Inside the URL (cols 4..23).
        let (action, row, columns) =
            TerminalPane::link_at(&grid, rect, cell, Pos2::new(14.5 * cell.x, 0.5 * cell.y))
                .expect("URL cell must resolve");
        assert_eq!(row, 0);
        assert_eq!(
            action,
            super::super::TerminalLinkAction::OpenUrl("https://example.com/a".into())
        );
        assert!(columns.start <= 14 && 14 < columns.end);
        // Outside it: prose and past the end.
        assert!(
            TerminalPane::link_at(&grid, rect, cell, Pos2::new(1.0 * cell.x, 0.5 * cell.y))
                .is_none()
        );
        assert!(
            TerminalPane::link_at(&grid, rect, cell, Pos2::new(40.0 * cell.x, 0.5 * cell.y))
                .is_none()
        );
    }

    #[test]
    #[cfg(unix)]
    fn hidden_bell_marks_the_tab_badge_until_viewed() {
        use portable_pty::CommandBuilder;
        use std::time::{Duration, Instant};
        let root = tempfile::tempdir().unwrap();
        let mut command = CommandBuilder::new("/bin/sh");
        command.args(["-c", "printf '\\aREADY'; sleep 10"]);
        command.cwd(root.path());
        let mut pane = TerminalPane {
            session: Some(
                Session::start_command(
                    &egui::Context::default(),
                    root.path(),
                    GridSize {
                        cols: 80,
                        rows: 8,
                        cell_width: 8,
                        cell_height: 16,
                    },
                    Colors {
                        foreground: egui::Color32::WHITE,
                        background: egui::Color32::BLACK,
                    },
                    command,
                )
                .unwrap(),
            ),
            ..Default::default()
        };
        // A bell while hidden marks the badge (and beeps once); viewing the
        // pane clears it.
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            pane.poll_bell(false);
            if pane.bell_pending() {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "hidden bell never marked the badge"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
        pane.poll_bell(true);
        assert!(!pane.bell_pending());
    }

    #[test]
    fn restart_resets_the_pane_without_keeping_prior_state() {
        let mut pane = TerminalPane::default();
        pane.prepare_fixture(b"previous output", Path::new("/original"));
        pane.grid = Some(Arc::new(pane.fixture.as_mut().unwrap().snapshot().unwrap()));
        pane.selection = Some((0, 15));
        pane.error = Some("previous error".into());
        pane.restart();
        assert!(pane.session.is_none());
        assert!(pane.fixture.is_none());
        assert!(pane.grid.is_none());
        assert!(pane.selection.is_none());
        assert!(pane.focus_requested);
        assert!(pane.error.is_none());
        assert_eq!(pane.status_text(), None);
        assert_eq!(
            pane.starting_directory(Path::new("/other")),
            Path::new("/other")
        );
    }
}
