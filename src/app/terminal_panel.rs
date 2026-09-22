use super::*;

impl EditorApp {
    pub(super) fn toggle_bottom_panel(&mut self, panel: BottomPanel, context: &egui::Context) {
        self.bottom_panel.toggle(panel);
        if self.bottom_panel == BottomPanel::Terminal {
            self.terminal.request_focus();
        } else if context.memory(|memory| memory.has_focus(terminal_id(context))) {
            context.memory_mut(|memory| memory.surrender_focus(terminal_id(context)));
        }
        self.terminal
            .set_visible(self.bottom_panel == BottomPanel::Terminal);
    }

    pub(super) fn show_bottom_panel(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            for (panel, label) in [
                (BottomPanel::Problems, "Problems"),
                (BottomPanel::Terminal, "Terminal"),
            ] {
                if ui
                    .selectable_label(self.bottom_panel == panel, label)
                    .clicked()
                {
                    self.bottom_panel = panel;
                    if panel == BottomPanel::Terminal {
                        self.terminal.request_focus();
                    }
                }
            }
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if ui.button("Close panel").clicked() {
                    self.bottom_panel = BottomPanel::Hidden;
                }
            });
        });
        ui.separator();
        self.terminal
            .set_visible(self.bottom_panel == BottomPanel::Terminal);
        match self.bottom_panel {
            BottomPanel::Problems => self.show_problems(ui),
            BottomPanel::Terminal => self.terminal.show(ui, &self.workspace_root),
            BottomPanel::Hidden => {}
        }
    }

    pub(super) fn terminal_focused(
        &self,
        context: &egui::Context,
        viewport: egui::ViewportId,
    ) -> bool {
        self.bottom_panel == BottomPanel::Terminal
            && viewport == context.viewport_id()
            && context.memory(|memory| memory.has_focus(terminal_id(context)))
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
        // as Ctrl+R and Ctrl+W. Keep its configurable toggle available, plus
        // Command-based host actions on macOS. Other platforms use the menus.
        if let Some(shortcut) = shortcuts.egui(ShortcutAction::Terminal)
            && context.input_mut(|input| input.consume_shortcut(&shortcut))
        {
            self.toggle_bottom_panel(BottomPanel::Terminal, context);
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

#[cfg(test)]
mod tests {
    use super::*;
    use egui_kittest::{Harness, kittest::Queryable as _};

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
        app.bottom_panel = BottomPanel::Problems;
        app.terminal
            .prepare_fixture(b"test terminal", Path::new("/project"));
        let mut harness = Harness::builder()
            .with_size(Vec2::new(800.0, 260.0))
            .build_ui_state(
                |ui, app: &mut EditorApp| {
                    app.handle_shortcuts(ui.ctx(), None);
                    app.process_native_menu_commands(ui.ctx(), None);
                    if app.bottom_panel != BottomPanel::Hidden {
                        app.show_bottom_panel(ui);
                    }
                },
                app,
            );
        harness.get_by_label("Terminal").click();
        harness.run();
        assert_eq!(harness.state().bottom_panel, BottomPanel::Terminal);
        assert!(harness.query_by_label("Terminal input").is_some());
        let input_rect = harness.get_by_label("Terminal input").rect();
        assert!(
            input_rect.height() > 160.0,
            "terminal header consumed the grid: {input_rect:?}"
        );
        harness.key_press_modifiers(Modifiers::CTRL, egui::Key::R);
        harness.key_press(egui::Key::Tab);
        harness.run();
        assert_eq!(harness.state().document().source(), "unchanged source");
        assert!(harness.state().compile_deadline.is_none());
        harness.get_by_label("Problems").click();
        harness.run();
        assert_eq!(harness.state().bottom_panel, BottomPanel::Problems);
        assert!(harness.query_by_label("No compiler diagnostics").is_some());
        harness.get_by_label("Close panel").click();
        harness.run();
        assert_eq!(harness.state().bottom_panel, BottomPanel::Hidden);
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
        assert_eq!(app.bottom_panel, BottomPanel::Terminal);
        app.execute_app_command(AppCommand::Problems, &context, None);
        // Problems are document-specific, but the shell remains available.
        assert_eq!(app.bottom_panel, BottomPanel::Terminal);
        app.execute_app_command(AppCommand::Terminal, &context, None);
        assert_eq!(app.bottom_panel, BottomPanel::Hidden);
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
        app.bottom_panel = BottomPanel::Terminal;
        app.terminal
            .prepare_fixture(b"test terminal", Path::new("/project"));
        app.finish_window_close();
        assert_eq!(app.bottom_panel, BottomPanel::Hidden);
    }
}
