use super::*;

impl EditorApp {
    pub(super) fn toggle_panel_maximized(&mut self, context: &egui::Context) {
        self.bottom_panel.toggle_maximized();
        self.sync_bottom_panel_focus(context);
        context.request_repaint();
    }

    /// Separate identities preserve the user's normal height while maximized.
    /// Return the layout used this frame, since a header click takes effect next frame.
    pub(super) fn show_bottom_panel_container(&mut self, ui: &mut egui::Ui) -> bool {
        if !self.bottom_panel.is_visible() {
            return false;
        }
        let maximized = self.bottom_panel.is_maximized();
        let id = crate::child_view::viewport_scoped_id(
            ui.ctx(),
            if maximized {
                "bottom-panel-maximized"
            } else {
                "bottom-panel"
            },
        );
        let available = ui.available_height().max(0.0);
        let panel = egui::Panel::bottom(id);
        let panel = if maximized {
            panel.resizable(false).exact_size(available)
        } else if self.snapshot_scene == Some(UiSnapshotScene::ActivityPanel) {
            panel.resizable(false).exact_size(120.0_f32.min(available))
        } else if matches!(
            self.snapshot_scene,
            Some(UiSnapshotScene::ProblemsPanel | UiSnapshotScene::TerminalPanel)
        ) {
            panel.resizable(false).exact_size(220.0_f32.min(available))
        } else {
            panel
                .resizable(true)
                .default_size(METRICS.chrome.bottom_panel_default_height)
                .min_size(METRICS.chrome.bottom_panel_min_height.min(available))
                .max_size(available)
        };
        panel.show(ui, |ui| {
            self.show_bottom_panel(ui);
            if self.bottom_panel.selected() == Some(PanelTab::Activity) {
                // Activity content is short when there are few indicators.
                // Keep its panel at the user's saved height when switching
                // from Terminal or Problems.
                ui.allocate_space(ui.available_size().max(Vec2::ZERO));
            }
        });
        maximized || ui.available_height() < 1.0
    }

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
                let problems = ui.selectable_label(
                    self.bottom_panel.selected() == Some(PanelTab::Problems), "Problems");
                #[cfg(all(feature = "desktop-ui-tests", target_os = "macos"))]
                crate::desktop_test::observe("panel.problems", &problems);
                if problems.clicked() {
                    self.bottom_panel.select(PanelTab::Problems);
                }
                let count = self.preview.diagnostics.len() + self.preview.editor_diagnostics.len();
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
            let terminal = ui.selectable_label(
                self.bottom_panel.selected() == Some(PanelTab::Terminal), "Terminal");
            #[cfg(all(feature = "desktop-ui-tests", target_os = "macos"))]
            crate::desktop_test::observe("panel.terminal", &terminal);
            if terminal.clicked() {
                self.bottom_panel.select(PanelTab::Terminal);
            }
            let activity = native_hover_text(ui.selectable_label(
                    self.bottom_panel.selected() == Some(PanelTab::Activity), "Activity"),
                    "App activity: green = ready or idle; yellow = waiting or working; red = failed; grey = not in use. Hover a name for details.");
            #[cfg(all(feature = "desktop-ui-tests", target_os = "macos"))]
            crate::desktop_test::observe("panel.activity", &activity);
            if activity.clicked() {
                self.bottom_panel.select(PanelTab::Activity);
            }
            // Reserve the square close control even for a long exit message.
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let close = panel_icon_button(ui, UiIcon::Close, "Close panel");
                #[cfg(all(feature = "desktop-ui-tests", target_os = "macos"))]
                crate::desktop_test::observe("panel.close", &close);
                if close.clicked() {
                    self.bottom_panel.hide();
                }
                let (icon, label) = if self.bottom_panel.is_maximized() {
                    (UiIcon::Restore, "Restore panel size")
                } else {
                    (UiIcon::Maximize, "Maximize panel")
                };
                let maximize = panel_icon_button(ui, icon, label);
                #[cfg(all(feature = "desktop-ui-tests", target_os = "macos"))]
                crate::desktop_test::observe("panel.maximize", &maximize);
                if maximize.clicked() {
                    self.bottom_panel.toggle_maximized();
                    ui.ctx().request_repaint();
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
            Some(PanelTab::Activity) => self.show_activity(ui),
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
            let restart =
                square_icon_button(ui, UiIcon::Refresh, "Restart terminal", PANEL_ACTION_SIZE);
            let detail = if restart.hovered() {
                format!(
                    "(start: {})",
                    self.terminal
                        .starting_directory(&self.workspace_root)
                        .display()
                )
            } else {
                String::new()
            };
            native_hover_text(restart.clone(), detail);
            if restart.clicked() {
                self.terminal.restart();
            }
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
        if let Some(shortcut) = shortcuts.egui(ShortcutAction::MaximizePanel)
            && context.input_mut(|input| input.consume_shortcut(&shortcut))
        {
            self.toggle_panel_maximized(context);
            return true;
        }
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

fn panel_icon_button(ui: &mut egui::Ui, icon: UiIcon, label: &str) -> egui::Response {
    native_hover_text(
        square_icon_button(ui, icon, label, PANEL_ACTION_SIZE),
        label,
    )
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
    fn panel_maximize_restores_large_resized_height_and_terminal_shortcut_focus() {
        let root = tempfile::tempdir().unwrap();
        let context = egui::Context::default();
        let mut app = EditorApp::dormant_window_for_tests(
            &context,
            root.path().into(),
            egui::ViewportId::ROOT,
        );
        app.snapshot_scene = None;
        theme::configure_editor_fonts(
            &context,
            Default::default(),
            Default::default(),
            false,
            400,
            400,
            None,
        );
        app.bottom_panel.select(PanelTab::Terminal);
        app.terminal
            .prepare_fixture(b"terminal fixture", Path::new("/project"));
        let mut initialized = false;
        let mut harness = Harness::builder()
            .with_size(Vec2::new(800.0, 720.0))
            .build_ui_state(
                move |ui, app: &mut EditorApp| {
                    // A user-resized height above the former 320-point cap.
                    if !initialized {
                        theme::configure_editor_fonts(
                            ui.ctx(),
                            Default::default(),
                            Default::default(),
                            false,
                            400,
                            400,
                            None,
                        );
                        let id = crate::child_view::viewport_scoped_id(ui.ctx(), "bottom-panel");
                        ui.ctx().data_mut(|data| {
                            data.insert_persisted(
                                id,
                                egui::PanelState {
                                    outer_rect: Rect::from_min_size(
                                        Pos2::ZERO,
                                        Vec2::new(800.0, 450.0),
                                    ),
                                },
                            )
                        });
                        initialized = true;
                    }
                    app.handle_shortcuts(ui.ctx(), None);
                    app.process_native_menu_commands(ui.ctx(), None);
                    if !app.show_bottom_panel_container(ui) {
                        ui.label("Document content");
                    }
                },
                app,
            );
        harness.run();
        let original = harness.get_by_label("Terminal input").rect();
        assert!(original.height() > 400.0);
        harness.get_by_label("Maximize panel").click();
        harness.run();
        let maximized = harness.get_by_label("Terminal input").rect();
        assert!(maximized.height() > 650.0);
        assert!(harness.query_by_label("Document content").is_none());
        harness.get_by_label("Restore panel size").click();
        harness.run();
        assert_eq!(harness.get_by_label("Terminal input").rect(), original);
        let panel_id = crate::child_view::viewport_scoped_id(&harness.ctx, "bottom-panel");
        let terminal_height = harness.ctx.data_mut(|data| {
            data.get_persisted::<egui::PanelState>(panel_id)
                .unwrap()
                .outer_rect
                .height()
        });
        harness.state_mut().bottom_panel.select(PanelTab::Activity);
        harness.run();
        let activity_height = harness.ctx.data_mut(|data| {
            data.get_persisted::<egui::PanelState>(panel_id)
                .unwrap()
                .outer_rect
                .height()
        });
        assert_eq!(activity_height, terminal_height);
        harness.state_mut().bottom_panel.select(PanelTab::Terminal);
        harness.run();
        harness.get_by_label("Terminal input").click();
        harness.run();
        let shortcut = harness
            .state()
            .settings
            .effective_shortcuts()
            .egui(ShortcutAction::MaximizePanel)
            .unwrap();
        harness.key_press_modifiers(shortcut.modifiers, shortcut.logical_key);
        harness.run();
        assert!(harness.state().bottom_panel.is_maximized());
        assert_eq!(
            harness.state().bottom_panel.selected(),
            Some(PanelTab::Terminal)
        );
        harness.key_press_modifiers(shortcut.modifiers, shortcut.logical_key);
        harness.run();
        assert!(!harness.state().bottom_panel.is_maximized());
        assert_eq!(harness.get_by_label("Terminal input").rect(), original);
        harness
            .state_mut()
            .enqueue_native_menu_command(AppCommand::MaximizePanel);
        harness.run();
        assert!(harness.state().bottom_panel.is_maximized());
        harness.set_size(Vec2::new(500.0, 350.0));
        harness.run();
        let small = harness.get_by_label("Terminal input").rect();
        assert!(small.bottom() <= 350.0 && small.top() >= 0.0);
    }

    #[test]
    fn problems_content_growth_does_not_resize_the_bottom_panel() {
        let root = tempfile::tempdir().unwrap();
        let context = egui::Context::default();
        let mut app = EditorApp::dormant_window_for_tests(
            &context,
            root.path().into(),
            egui::ViewportId::ROOT,
        );
        app.snapshot_scene = None;
        app.bottom_panel.select(PanelTab::Problems);
        let mut harness = Harness::builder()
            .with_size(Vec2::new(800.0, 720.0))
            .build_ui_state(
                |ui, app: &mut EditorApp| {
                    if !app.show_bottom_panel_container(ui) {
                        ui.label("Document content");
                    }
                },
                app,
            );
        harness.run();
        let panel_id = crate::child_view::viewport_scoped_id(&harness.ctx, "bottom-panel");
        let initial_height = harness.ctx.data_mut(|data| {
            data.get_persisted::<egui::PanelState>(panel_id)
                .unwrap()
                .outer_rect
                .height()
        });

        harness.state_mut().preview.diagnostics = (0..80)
            .map(|index| crate::diagnostics::Diagnostic {
                provider: None,
                severity: crate::diagnostics::DiagnosticSeverity::Error,
                source: crate::diagnostics::DiagnosticSource::Main,
                location: Some(crate::diagnostics::DiagnosticLocation {
                    line: index + 1,
                    column: 1,
                }),
                message: format!("Diagnostic {index}: {}", "detail ".repeat(20)),
                details: vec!["more context ".repeat(20)],
            })
            .collect();
        for _ in 0..8 {
            harness.run();
            let height = harness.ctx.data_mut(|data| {
                data.get_persisted::<egui::PanelState>(panel_id)
                    .unwrap()
                    .outer_rect
                    .height()
            });
            assert_eq!(height, initial_height);
        }
    }

    #[test]
    fn restart_tooltip_only_shows_the_shell_start_path() {
        let root = tempfile::tempdir().unwrap();
        let context = egui::Context::default();
        let mut app = EditorApp::dormant_for_tests(&context, root.path().into());
        app.bottom_panel.select(PanelTab::Terminal);
        app.terminal.prepare_fixture(b"test", Path::new("/project"));
        let mut detail = None;
        let mut harness = Harness::builder()
            .with_size(Vec2::new(500.0, 200.0))
            .build_ui_state(
                move |ui, state: &mut (EditorApp, Option<Arc<str>>)| {
                    tooltips::install_hover_runtime_config(ui.ctx(), Duration::ZERO);
                    state.0.show_bottom_panel(ui);
                    let id = native_hover_tooltip_id(ui.ctx());
                    state.1 = ui.ctx().data(|data| {
                        data.get_temp::<HoverTooltipOverlay>(id)
                            .map(|hover| hover.detail)
                    });
                },
                (app, detail.take()),
            );
        harness.get_by_label("Restart terminal").hover();
        harness.run();
        assert_eq!(harness.state().1.as_deref(), Some("(start: /project)"));
        assert!(harness.query_by_label("Starting directory").is_none());
    }

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
        assert_eq!(restart.size(), Vec2::splat(PANEL_ACTION_SIZE));
        assert!(restart.left() > input_rect.right());
        assert_eq!(restart.top(), input_rect.top());
        assert!(harness.query_by_label("Starting directory").is_none());
        assert!(harness.query_by_value("/project").is_none());
        harness.get_by_label("Terminal input").click();
        harness.run();
        harness.key_press_modifiers(Modifiers::CTRL, egui::Key::R);
        harness.key_press(egui::Key::Tab);
        harness.run();
        assert_eq!(harness.state().document().source(), "unchanged source");
        assert!(harness.state().compile_deadline.is_none());
        harness.get_by_label("Activity").click();
        harness.run();
        assert_eq!(
            harness.state().bottom_panel.selected(),
            Some(PanelTab::Activity)
        );
        assert!(harness.query_by_label("Terminal input").is_none());
        assert!(harness.state().compile_deadline.is_none());
        assert_eq!(harness.state().document().source(), "unchanged source");
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
