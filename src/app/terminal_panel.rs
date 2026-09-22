use super::*;

impl EditorApp {
    pub(super) fn toggle_panel(&mut self, context: &egui::Context) {
        self.bottom_panel.toggle_visibility();
        self.sync_bottom_panel_focus(context);
    }

    pub(super) fn toggle_bottom_panel(&mut self, tab: PanelTab, context: &egui::Context) {
        self.bottom_panel.toggle(tab);
        self.sync_bottom_panel_focus(context);
    }

    fn sync_bottom_panel_focus(&mut self, context: &egui::Context) {
        // Resolving a viewport-scoped ID reads the context. Do it before
        // entering either memory closure: reentering its write lock deadlocks.
        let id = terminal_id(context);
        let terminal_visible = self.bottom_panel.selected() == Some(PanelTab::Terminal);
        if terminal_visible {
            self.terminal.request_focus();
        } else {
            egui::Popup::close_id(context, terminal_directory_popup_id(context));
            if context.memory(|memory| memory.has_focus(id)) {
                context.memory_mut(|memory| memory.surrender_focus(id));
            }
        }
        self.terminal.set_visible(terminal_visible);
    }

    pub(super) fn show_bottom_panel(&mut self, ui: &mut egui::Ui) {
        let before = self.bottom_panel;
        if self.bottom_panel.selected() == Some(PanelTab::Terminal) {
            self.terminal.sync_snapshot();
        }
        ui.horizontal(|ui| {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 3.0;
                if ui
                    .selectable_label(
                        self.bottom_panel.selected() == Some(PanelTab::Problems),
                        "Problems",
                    )
                    .clicked()
                {
                    self.bottom_panel.select(PanelTab::Problems);
                }
                let count =
                    self.preview.diagnostics.len() + self.preview.tinymist_diagnostics.len();
                egui::Frame::new()
                    .fill(ui.visuals().widgets.inactive.bg_fill)
                    .corner_radius(6.0)
                    .inner_margin(egui::Margin::symmetric(4, 0))
                    .show(ui, |ui| {
                        ui.label(RichText::new(count.to_string()).size(theme::TYPE.supporting));
                    })
                    .response
                    .on_hover_text("Compiler diagnostics");
            });
            if ui
                .selectable_label(
                    self.bottom_panel.selected() == Some(PanelTab::Terminal),
                    "Terminal",
                )
                .clicked()
            {
                self.bottom_panel.select(PanelTab::Terminal);
            }
            // Reserve the square close control even for a long exit message.
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if panel_icon_button(ui, UiIcon::Close, "Close panel").clicked() {
                    self.bottom_panel.hide();
                }
                if self.bottom_panel.selected() == Some(PanelTab::Terminal)
                    && let Some(status) = self.terminal.status_text()
                {
                    ui.add(egui::Label::new(RichText::new(status).weak()).truncate());
                }
            });
        });
        if before != self.bottom_panel {
            self.sync_bottom_panel_focus(ui.ctx());
        }
        ui.separator();
        match self.bottom_panel.selected() {
            Some(PanelTab::Problems) => self.show_problems(ui),
            Some(PanelTab::Terminal) => self.show_terminal_panel(ui),
            None => {}
        }
    }

    fn show_terminal_panel(&mut self, ui: &mut egui::Ui) {
        let (_, available) = ui.allocate_space(ui.available_size().max(Vec2::ZERO));
        let (grid, actions) = terminal_panel_rects(available);
        ui.scope_builder(egui::UiBuilder::new().max_rect(grid), |ui| {
            ui.set_clip_rect(ui.clip_rect().intersect(grid));
            self.terminal.show(ui, &self.workspace_root);
        });
        ui.painter().vline(
            actions.left() - 3.0,
            available.y_range(),
            ui.visuals().widgets.noninteractive.bg_stroke,
        );
        ui.scope_builder(egui::UiBuilder::new().max_rect(actions), |ui| {
            ui.spacing_mut().item_spacing.y = 3.0;
            if panel_icon_button(ui, UiIcon::Refresh, "Restart terminal").clicked() {
                self.terminal.restart();
            }
            let info = panel_icon_button(ui, UiIcon::Folder, "Starting directory");
            if info.clicked() {
                let id = terminal_id(ui.ctx());
                ui.memory_mut(|memory| memory.surrender_focus(id));
            }
            let popup_width = 280.0_f32.min((grid.width() - 16.0).max(0.0));
            egui::Popup::from_toggle_button_response(&info)
                .id(terminal_directory_popup_id(ui.ctx()))
                .at_position(Pos2::new(
                    (grid.right() - popup_width - 16.0).max(grid.left()),
                    grid.top(),
                ))
                .align(egui::RectAlign::BOTTOM_START)
                .align_alternatives(&[])
                .width(popup_width)
                .show(|ui| {
                    // Keep the popover inside the bottom panel, below native preview surfaces.
                    egui::ScrollArea::vertical()
                        .max_height((grid.height() - 16.0).max(0.0))
                        .show(ui, |ui| {
                            ui.label(RichText::new("Shell starting directory").small().strong());
                            let path = self
                                .terminal
                                .starting_directory(&self.workspace_root)
                                .display()
                                .to_string();
                            ui.add(
                                egui::TextEdit::singleline(&mut path.as_str())
                                    .desired_width(f32::INFINITY),
                            );
                            ui.label(
                                RichText::new("Where the shell started; cd may change it.").small(),
                            );
                        });
                });
        });
    }

    pub(super) fn terminal_focused(
        &self,
        context: &egui::Context,
        viewport: egui::ViewportId,
    ) -> bool {
        let id = terminal_id(context);
        self.bottom_panel.selected() == Some(PanelTab::Terminal)
            && viewport == context.viewport_id()
            && context.memory(|memory| memory.has_focus(id))
    }

    pub(super) fn handle_terminal_shortcuts(
        &mut self,
        context: &egui::Context,
        viewport: egui::ViewportId,
        shortcuts: &ShortcutBindings,
        frame: Option<&eframe::Frame>,
    ) -> bool {
        if !self.terminal_focused(context, viewport) {
            return false;
        }
        // The terminal owns raw control keys, including editor defaults such
        // as Ctrl+R and Ctrl+W. Keep the panel/terminal toggles available, plus
        // Command-based host actions on macOS. Other platforms use the menus.
        if let Some(shortcut) = shortcuts.egui(ShortcutAction::Terminal)
            && context.input_mut(|input| input.consume_shortcut(&shortcut))
        {
            self.toggle_bottom_panel(PanelTab::Terminal, context);
            return true;
        }
        if let Some(shortcut) = shortcuts.egui(ShortcutAction::Panel)
            && context.input_mut(|input| input.consume_shortcut(&shortcut))
        {
            self.toggle_panel(context);
            return true;
        }
        if cfg!(target_os = "macos") {
            if context.input_mut(|input| tabs::consume_window_close(input, shortcuts, false)) {
                context.send_viewport_cmd(egui::ViewportCommand::Close);
            }
            let command = context.input_mut(|input| {
                if !input.modifiers.mac_cmd {
                    return None;
                }
                consume_shortcut(input, shortcuts, |_| true)
            });
            if let Some(command) = command
                && !self.route_terminal_edit_command(command, context, viewport)
            {
                self.execute_app_command(command, context, frame);
            }
        } else {
            // egui's semantic Ctrl+C/X/V events are editor conventions. In a
            // terminal these are raw controls; use Ctrl+Shift+C/V for clipboard.
            let modifiers = context.input(|input| input.modifiers);
            if modifiers.ctrl && modifiers.shift {
                if context.input_mut(|input| {
                    input.consume_key(Modifiers::CTRL | Modifiers::SHIFT, egui::Key::C)
                }) {
                    self.terminal.copy(context);
                }
                if context.input_mut(|input| {
                    input.consume_key(Modifiers::CTRL | Modifiers::SHIFT, egui::Key::V)
                }) {
                    context.send_viewport_cmd(egui::ViewportCommand::RequestPaste);
                }
            } else if modifiers.ctrl {
                context.input_mut(|input| {
                    for event in &mut input.events {
                        let key = match event {
                            egui::Event::Copy => egui::Key::C,
                            egui::Event::Cut => egui::Key::X,
                            egui::Event::Paste(_) => egui::Key::V,
                            _ => continue,
                        };
                        *event = egui::Event::Key {
                            key,
                            physical_key: None,
                            pressed: true,
                            repeat: false,
                            modifiers,
                        };
                    }
                });
            }
        }
        true
    }

    pub(super) fn route_terminal_edit_command(
        &mut self,
        command: AppCommand,
        context: &egui::Context,
        viewport: egui::ViewportId,
    ) -> bool {
        if !self.terminal_focused(context, viewport) {
            return false;
        }
        match command {
            AppCommand::Copy | AppCommand::Cut => self.terminal.copy(context),
            AppCommand::Paste => {
                context.send_viewport_cmd_to(viewport, egui::ViewportCommand::RequestPaste);
            }
            AppCommand::SelectAll => self.terminal.select_all(),
            AppCommand::Undo
            | AppCommand::Redo
            | AppCommand::ToggleComment
            | AppCommand::Format => {}
            _ => return false,
        }
        true
    }
}

const PANEL_ACTION_SIZE: f32 = 22.0;
const PANEL_ACTION_GAP: f32 = 6.0;

fn terminal_directory_popup_id(context: &egui::Context) -> egui::Id {
    crate::child_view::viewport_scoped_id(context, "terminal-directory-popup")
}

fn panel_icon_button(ui: &mut egui::Ui, icon: UiIcon, label: &str) -> egui::Response {
    let response = ui.add_sized(Vec2::splat(PANEL_ACTION_SIZE), egui::Button::new(""));
    response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, label));
    paint_ui_icon(
        ui.painter(),
        response.rect.shrink(5.0),
        icon,
        ui.style().interact(&response).fg_stroke.color,
    );
    native_hover_text(response, label)
}

fn terminal_panel_rects(available: Rect) -> (Rect, Rect) {
    let actions_left = (available.right() - PANEL_ACTION_SIZE).max(available.left());
    let grid_right = (actions_left - PANEL_ACTION_GAP).max(available.left());
    (
        Rect::from_min_max(available.min, Pos2::new(grid_right, available.bottom())),
        Rect::from_min_max(Pos2::new(actions_left, available.top()), available.max),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui_kittest::{Harness, kittest::Queryable as _};

    #[test]
    fn switching_away_from_a_focused_terminal_releases_focus_without_locking() {
        let root = tempfile::tempdir().unwrap();
        let context = egui::Context::default();
        let mut app = EditorApp::dormant_for_tests(&context, root.path().into());
        let id = terminal_id(&context);
        app.toggle_bottom_panel(PanelTab::Problems, &context);
        app.toggle_bottom_panel(PanelTab::Terminal, &context);
        context.memory_mut(|memory| memory.request_focus(id));
        assert!(app.terminal_focused(&context, context.viewport_id()));
        app.toggle_bottom_panel(PanelTab::Problems, &context);
        assert_eq!(app.bottom_panel.selected(), Some(PanelTab::Problems));
        assert!(!context.memory(|memory| memory.has_focus(id)));
        // Closing/reopening the container must retain Terminal and release its focus too.
        app.toggle_bottom_panel(PanelTab::Terminal, &context);
        context.memory_mut(|memory| memory.request_focus(id));
        app.toggle_panel(&context);
        assert!(!app.bottom_panel.is_visible());
        assert!(!context.memory(|memory| memory.has_focus(id)));
        app.toggle_panel(&context);
        assert_eq!(app.bottom_panel.selected(), Some(PanelTab::Terminal));
    }

    #[test]
    fn panel_tabs_switch_semantically_and_close_without_mutating_source() {
        let root = tempfile::tempdir().unwrap();
        let context = egui::Context::default();
        let mut app = EditorApp::dormant_window_for_tests(
            &context,
            root.path().into(),
            egui::ViewportId::ROOT,
        );
        app.document_mut()
            .replace_unprojected_untitled("unchanged source");
        app.bottom_panel.select(PanelTab::Problems);
        app.terminal
            .prepare_fixture(b"test terminal", Path::new("/project"));
        let mut harness = Harness::builder()
            .with_size(Vec2::new(800.0, 260.0))
            .build_ui_state(
                |ui, app: &mut EditorApp| {
                    app.handle_shortcuts(ui.ctx(), None);
                    app.process_native_menu_commands(ui.ctx(), None);
                    if app.bottom_panel.is_visible() {
                        app.show_bottom_panel(ui);
                    }
                },
                app,
            );
        harness.get_by_label("Terminal").click();
        harness.run();
        assert_eq!(
            harness.state().bottom_panel.selected(),
            Some(PanelTab::Terminal)
        );
        assert!(harness.query_by_label("Terminal input").is_some());
        let input_rect = harness.get_by_label("Terminal input").rect();
        assert!(
            input_rect.height() > 200.0,
            "terminal header consumed the grid: {input_rect:?}"
        );
        let restart = harness.get_by_label("Restart terminal").rect();
        let directory = harness.get_by_label("Starting directory").rect();
        assert_eq!(restart.size(), Vec2::splat(PANEL_ACTION_SIZE));
        assert_eq!(directory.size(), restart.size());
        assert!(restart.left() > input_rect.right());
        assert_eq!(restart.top(), input_rect.top());
        assert!(directory.top() > restart.bottom());
        assert_eq!(directory.left(), restart.left());
        assert!(harness.query_by_value("/project").is_none());
        harness.get_by_label("Starting directory").click();
        harness.run();
        let path = harness
            .get(
                egui_kittest::kittest::by()
                    .role(egui::accesskit::Role::TextInput)
                    .value("/project"),
            )
            .rect();
        assert!(input_rect.contains_rect(path));
        harness.get_by_label("Where the shell started; cd may change it.");
        harness.key_press(egui::Key::Escape);
        harness.run();
        assert!(harness.query_by_value("/project").is_none());
        harness.get_by_label("Terminal input").click();
        harness.run();
        harness.key_press_modifiers(Modifiers::CTRL, egui::Key::R);
        harness.key_press(egui::Key::Tab);
        harness.run();
        assert_eq!(harness.state().document().source(), "unchanged source");
        assert!(harness.state().compile_deadline.is_none());
        harness.get_by_label("Problems").click();
        harness.run();
        assert_eq!(
            harness.state().bottom_panel.selected(),
            Some(PanelTab::Problems)
        );
        assert!(harness.query_by_label("No compiler diagnostics").is_some());
        let problems = harness.get_by_label("Problems").rect();
        let badge = harness.get_by_label("0").rect();
        assert!(badge.left() > problems.right());
        assert!((badge.center().y - problems.center().y).abs() < 1.0);
        assert!(harness.query_by_label("0 diagnostics").is_none());
        harness.get_by_label("Close panel").click();
        harness.run();
        assert!(!harness.state().bottom_panel.is_visible());
    }

    #[test]
    fn terminal_action_strip_preserves_height_and_never_overlaps_the_grid() {
        for size in [
            Vec2::new(800.0, 180.0),
            Vec2::new(320.0, 80.0),
            Vec2::new(20.0, 60.0),
        ] {
            let available = Rect::from_min_size(Pos2::new(12.0, 34.0), size);
            let (grid, actions) = terminal_panel_rects(available);
            assert!(available.contains_rect(grid));
            assert!(available.contains_rect(actions));
            assert!(grid.right() <= actions.left());
            assert_eq!(grid.height(), available.height());
            assert_eq!(actions.height(), available.height());
            assert!(actions.width() <= PANEL_ACTION_SIZE);
        }
    }

    #[test]
    fn directory_popover_stays_in_a_short_panel_and_does_not_reopen_with_the_tab() {
        let root = tempfile::tempdir().unwrap();
        let context = egui::Context::default();
        let mut app = EditorApp::dormant_for_tests(&context, root.path().into());
        app.bottom_panel.select(PanelTab::Terminal);
        let path = format!("/project/{}", "a-long-starting-directory-".repeat(20));
        app.terminal.prepare_fixture(b"test", Path::new(&path));
        let mut harness = Harness::builder()
            .with_size(Vec2::new(320.0, 120.0))
            .build_ui_state(|ui, app: &mut EditorApp| app.show_bottom_panel(ui), app);
        harness.run();
        let grid = harness.get_by_label("Terminal input").rect();
        harness.get_by_label("Starting directory").click();
        harness.run();
        let field = harness
            .get(
                egui_kittest::kittest::by()
                    .role(egui::accesskit::Role::TextInput)
                    .value(path.as_str()),
            )
            .rect();
        assert!(
            grid.contains_rect(field),
            "directory field escaped panel: {field:?} vs {grid:?}"
        );
        let explanation = harness
            .get_by_label("Where the shell started; cd may change it.")
            .rect();
        assert!(
            grid.contains_rect(explanation),
            "directory explanation escaped panel: {explanation:?} vs {grid:?}"
        );
        harness.get_by_label("Problems").click();
        harness.run();
        harness.get_by_label("Terminal").click();
        harness.run();
        assert!(harness.query_by_label("Shell starting directory").is_none());
    }

    #[test]
    fn terminal_menu_and_shortcut_work_in_an_empty_workspace() {
        let root = tempfile::tempdir().unwrap();
        let context = egui::Context::default();
        let mut app = EditorApp::dormant_window_for_tests(
            &context,
            root.path().into(),
            egui::ViewportId::ROOT,
        );
        app.tabs = tabs::Tabs::default();
        assert!(app.native_command_enabled(AppCommand::Terminal));
        app.execute_app_command(AppCommand::Terminal, &context, None);
        assert_eq!(app.bottom_panel.selected(), Some(PanelTab::Terminal));
        app.execute_app_command(AppCommand::Panel, &context, None);
        assert!(!app.bottom_panel.is_visible());
        app.execute_app_command(AppCommand::Panel, &context, None);
        // The shared container remains available and remembers its tab.
        assert_eq!(app.bottom_panel.selected(), Some(PanelTab::Terminal));
        app.execute_app_command(AppCommand::Terminal, &context, None);
        assert!(!app.bottom_panel.is_visible());
    }

    #[test]
    fn closing_window_discards_its_terminal_and_panel_state() {
        let root = tempfile::tempdir().unwrap();
        let context = egui::Context::default();
        let mut app = EditorApp::dormant_window_for_tests(
            &context,
            root.path().into(),
            egui::ViewportId::ROOT,
        );
        app.lifecycle.request_resume();
        app.lifecycle.activate();
        app.bottom_panel.select(PanelTab::Terminal);
        app.terminal
            .prepare_fixture(b"test terminal", Path::new("/project"));
        app.finish_window_close();
        assert!(!app.bottom_panel.is_visible());
    }
}
