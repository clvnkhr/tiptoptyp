//! The compact title-bar layout picker shares the app's native popup host.
use super::*;

const CHECK_COLUMN: f32 = 20.0;
const LAYOUT_COMMANDS: [AppCommand; 5] = [
    AppCommand::Code,
    AppCommand::Split,
    AppCommand::Preview,
    AppCommand::Explorer,
    AppCommand::Panel,
];

#[derive(Debug, Clone, Copy)]
pub(super) struct LayoutMenuState {
    mode: ViewMode,
    explorer: bool,
    panel: bool,
    modes_enabled: bool,
    explorer_enabled: bool,
    panel_enabled: bool,
}

impl LayoutMenuState {
    fn checked(self, command: AppCommand) -> bool {
        match command {
            AppCommand::Code => self.mode == ViewMode::Code,
            AppCommand::Split => self.mode == ViewMode::Split,
            AppCommand::Preview => self.mode == ViewMode::Preview,
            AppCommand::Explorer => self.explorer,
            AppCommand::Panel => self.panel,
            _ => unreachable!("not a layout command"),
        }
    }

    fn enabled(self, command: AppCommand) -> bool {
        match command {
            AppCommand::Code | AppCommand::Split | AppCommand::Preview => self.modes_enabled,
            AppCommand::Explorer => self.explorer_enabled,
            AppCommand::Panel => self.panel_enabled,
            _ => unreachable!("not a layout command"),
        }
    }
}

impl EditorApp {
    pub(super) fn layout_menu_state(&self) -> LayoutMenuState {
        LayoutMenuState {
            mode: self.view_mode,
            explorer: self.explorer.panel_visible(),
            panel: self.bottom_panel.is_visible(),
            modes_enabled: self.native_command_enabled(AppCommand::Code),
            explorer_enabled: self.native_command_enabled(AppCommand::Explorer),
            panel_enabled: self.native_command_enabled(AppCommand::Panel),
        }
    }
}

fn row_shortcut(
    context: &egui::Context,
    shortcuts: &ShortcutBindings,
    command: AppCommand,
) -> String {
    shortcuts
        .egui(command_spec(command).shortcut_action)
        .map(|shortcut| context.format_shortcut(&shortcut))
        .unwrap_or_default()
}

pub(super) fn layout_popup_size(
    context: &egui::Context,
    style: &egui::Style,
    shortcuts: &ShortcutBindings,
) -> Vec2 {
    let width = LAYOUT_COMMANDS
        .into_iter()
        .map(|command| {
            popup_layout::menu_row_content_width(
                context,
                style,
                command_spec(command).title,
                &row_shortcut(context, shortcuts, command),
            ) + CHECK_COLUMN
        })
        .fold(180.0, f32::max);
    let margins = theme::menu_card_frame(style).total_margin().sum();
    popup_layout::menu_popup_size(width + margins.x, LAYOUT_COMMANDS.len(), 1, style)
}

pub(super) fn show_layout_popup_ui(
    ui: &mut egui::Ui,
    state: LayoutMenuState,
    shortcuts: &ShortcutBindings,
    action: &mut Option<AppPopupAction>,
) {
    ui.spacing_mut().item_spacing.y = theme::SPACE.small;
    for command in LAYOUT_COMMANDS {
        if command == AppCommand::Explorer {
            ui.add(egui::Separator::default().spacing(theme::SPACE.control));
        }
        let label = command_spec(command).title;
        let right = row_shortcut(ui.ctx(), shortcuts, command);
        let checked = state.checked(command);
        let response = ui
            .add_enabled_ui(state.enabled(command), |ui| {
                layout_row(ui, label, &right, checked)
            })
            .inner;
        #[cfg(all(feature = "desktop-ui-tests", target_os = "macos"))]
        crate::desktop_test::observe(
            match command {
                AppCommand::Code => "layout.code",
                AppCommand::Split => "layout.split",
                AppCommand::Preview => "layout.preview",
                AppCommand::Explorer => "layout.explorer",
                AppCommand::Panel => "layout.panel",
                _ => unreachable!("not a layout command"),
            },
            &response,
        );
        if response.clicked() {
            *action = Some(AppPopupAction::Command(command));
        }
    }
}

fn layout_row(ui: &mut egui::Ui, label: &str, right: &str, checked: bool) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(
        egui::vec2(ui.available_width().max(1.0), METRICS.menu.row_height),
        egui::Sense::click(),
    );
    let accessible = if right.is_empty() {
        label.to_owned()
    } else {
        format!("{label} {right}")
    };
    response.widget_info(|| {
        egui::WidgetInfo::selected(
            egui::WidgetType::SelectableLabel,
            ui.is_enabled(),
            checked,
            &accessible,
        )
    });
    if response.hovered()
        || response.has_focus()
        || response.is_pointer_button_down_on()
        || response.highlighted()
    {
        ui.painter().rect(
            rect,
            ui.visuals().widgets.hovered.corner_radius,
            ui.visuals().faint_bg_color,
            ui.visuals().widgets.hovered.bg_stroke,
            egui::StrokeKind::Inside,
        );
    }
    let color = if ui.is_enabled() {
        ui.visuals().text_color()
    } else {
        ui.visuals().weak_text_color()
    };
    let pad = ui.spacing().button_padding.x;
    let font = egui::TextStyle::Button.resolve(ui.style());
    if checked {
        paint_ui_icon(
            ui.painter(),
            Rect::from_center_size(
                rect.left_center() + egui::vec2(pad + 6.0, 0.0),
                Vec2::splat(12.0),
            ),
            UiIcon::Check,
            color,
        );
    }
    ui.painter().text(
        rect.left_center() + egui::vec2(pad + CHECK_COLUMN, 0.0),
        egui::Align2::LEFT_CENTER,
        label,
        font.clone(),
        color,
    );
    ui.painter().text(
        rect.right_center() - egui::vec2(pad, 0.0),
        egui::Align2::RIGHT_CENTER,
        right,
        font,
        ui.visuals().weak_text_color(),
    );
    response
}

// Native child windows are exercised by the desktop journey. Deterministic
// command-route tests render the same menu body inside their harness viewport.
#[cfg(test)]
pub(super) fn show_test_popup(ui: &mut egui::Ui, app: &mut EditorApp) {
    if !matches!(app.app_popup, Some(AppPopup::Layout { .. })) {
        return;
    }
    let mut action = None;
    let state = app.layout_menu_state();
    let shortcuts = app.settings.effective_shortcuts();
    egui::Area::new(ui.id().with("test-layout-popup"))
        .order(egui::Order::Foreground)
        .fixed_pos(Pos2::new(0.0, METRICS.chrome.toolbar_height))
        .show(ui.ctx(), |ui| {
            ui.set_width(240.0);
            show_layout_popup_ui(ui, state, &shortcuts, &mut action);
        });
    if let Some(action) = action {
        app.close_app_popup();
        app.pending_app_popup_action = Some(action);
        app.execute_pending_app_popup_action(ui.ctx(), None);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui_kittest::{
        Harness,
        kittest::{NodeT as _, Queryable as _},
    };

    #[test]
    fn layout_checkmarks_keep_modes_exclusive_and_toggles_independent() {
        assert_eq!(
            LAYOUT_COMMANDS,
            [
                AppCommand::Code,
                AppCommand::Split,
                AppCommand::Preview,
                AppCommand::Explorer,
                AppCommand::Panel
            ]
        );
        for mode in [ViewMode::Code, ViewMode::Split, ViewMode::Preview] {
            for explorer in [false, true] {
                for panel in [false, true] {
                    let state = LayoutMenuState {
                        mode,
                        explorer,
                        panel,
                        modes_enabled: true,
                        explorer_enabled: true,
                        panel_enabled: true,
                    };
                    assert_eq!(
                        LAYOUT_COMMANDS[..3]
                            .iter()
                            .filter(|command| state.checked(**command))
                            .count(),
                        1
                    );
                    assert_eq!(state.checked(AppCommand::Explorer), explorer);
                    assert_eq!(state.checked(AppCommand::Panel), panel);
                }
            }
        }
    }

    #[test]
    fn layout_rows_preserve_shortcuts_order_and_dispatch_exactly_once() {
        for command in LAYOUT_COMMANDS {
            let shortcuts = ShortcutBindings::current_defaults();
            let state = LayoutMenuState {
                mode: ViewMode::Split,
                explorer: true,
                panel: false,
                modes_enabled: true,
                explorer_enabled: true,
                panel_enabled: true,
            };
            let mut harness = Harness::builder()
                .with_size(Vec2::new(260.0, 200.0))
                .build_ui_state(
                    |ui, action: &mut Option<AppPopupAction>| {
                        show_layout_popup_ui(ui, state, &shortcuts, action)
                    },
                    None,
                );
            harness.run();
            let mut previous_bottom = 0.0;
            for row in LAYOUT_COMMANDS {
                let label = format!(
                    "{} {}",
                    command_spec(row).title,
                    row_shortcut(&harness.ctx, &shortcuts, row)
                );
                let rect = harness.get_by_label(&label).rect();
                assert!(rect.top() >= previous_bottom);
                previous_bottom = rect.bottom();
                assert_eq!(rect.height(), METRICS.menu.row_height);
                assert_eq!(
                    harness.get_by_label(&label).accesskit_node().toggled(),
                    Some(if state.checked(row) {
                        egui::accesskit::Toggled::True
                    } else {
                        egui::accesskit::Toggled::False
                    })
                );
            }
            let label = format!(
                "{} {}",
                command_spec(command).title,
                row_shortcut(&harness.ctx, &shortcuts, command)
            );
            harness.get_by_label(&label).click();
            harness.run();
            assert!(
                matches!(harness.state(), Some(AppPopupAction::Command(actual)) if *actual == command)
            );
            *harness.state_mut() = None;
            harness.run();
            assert!(harness.state().is_none());
        }
    }

    #[test]
    fn keyboard_focused_layout_row_is_outlined_without_reflow_and_enter_selects_it() {
        let shortcuts = ShortcutBindings::current_defaults();
        let state = LayoutMenuState {
            mode: ViewMode::Split,
            explorer: true,
            panel: false,
            modes_enabled: true,
            explorer_enabled: true,
            panel_enabled: true,
        };
        let mut harness = Harness::builder()
            .with_size(Vec2::new(260.0, 200.0))
            .build_ui_state(
                |ui, action: &mut Option<AppPopupAction>| {
                    show_layout_popup_ui(ui, state, &shortcuts, action);
                },
                None,
            );
        harness.run();
        let label = format!(
            "Code {}",
            row_shortcut(&harness.ctx, &shortcuts, AppCommand::Code)
        );
        let rect = harness.get_by_label(&label).rect();
        harness.key_press(egui::Key::Tab);
        harness.run();
        let focused = harness.get_by_label(&label);
        assert!(focused.accesskit_node().is_focused());
        assert_eq!(focused.rect(), rect);
        assert!(harness.output().shapes.iter().any(|clipped| {
            matches!(&clipped.shape, egui::Shape::Rect(shape) if shape.rect == rect && shape.stroke.width > 0.0)
        }), "keyboard focus must have a visible outline");
        harness.key_press(egui::Key::Enter);
        harness.run();
        assert!(matches!(
            harness.state(),
            Some(AppPopupAction::Command(AppCommand::Code))
        ));
    }

    #[test]
    fn layout_toolbar_is_one_compact_control_in_the_existing_titlebar() {
        for style in crate::settings::ToolbarStyle::ALL {
            for width in [900.0, 1200.0] {
                let root = tempfile::tempdir().unwrap();
                let context = egui::Context::default();
                let mut app = EditorApp::dormant_for_tests(&context, root.path().into());
                app.document_mut().replace_unprojected_untitled("unchanged");
                app.settings.toolbar_style = style;
                let mut harness = Harness::builder()
                    .with_size(Vec2::new(width, 240.0))
                    .build_ui_state(
                        |ui, app: &mut EditorApp| {
                            let toolbar = egui::Panel::top("layout-test-toolbar")
                                .exact_size(METRICS.chrome.toolbar_height)
                                .show(ui, |ui| app.show_toolbar(ui, None));
                            assert_eq!(toolbar.response.rect.height(), 30.0);
                        },
                        app,
                    );
                harness.run();
                let layout = harness.get_by_label("Layout").rect();
                assert!(layout.height() <= METRICS.chrome.toolbar_height);
                assert!(layout.width() < 100.0);
                assert!(harness.get_by_label("Settings").rect().right() <= layout.left());
                for old in ["Explorer", "Code", "Split", "Preview", "Panel"] {
                    assert!(
                        harness.query_by_label(old).is_none(),
                        "{old} must live in the picker"
                    );
                }
                harness.get_by_label("Layout").click();
                harness.run();
                assert!(matches!(
                    harness.state().app_popup,
                    Some(AppPopup::Layout { .. })
                ));
                assert_eq!(harness.state().view_mode, ViewMode::Split);
                assert_eq!(harness.state().document().source(), "unchanged");
                harness.get_by_label("Layout").click();
                harness.run();
                assert!(harness.state().app_popup.is_none());
            }
        }
    }

    #[test]
    fn unavailable_layout_modes_do_not_dispatch_and_sizes_fit_the_rows() {
        let shortcuts = ShortcutBindings::current_defaults();
        let state = LayoutMenuState {
            mode: ViewMode::Code,
            explorer: true,
            panel: true,
            modes_enabled: false,
            explorer_enabled: true,
            panel_enabled: true,
        };
        let mut harness = Harness::builder()
            .with_size(Vec2::new(260.0, 200.0))
            .build_ui_state(
                |ui, action: &mut Option<AppPopupAction>| {
                    let size = layout_popup_size(ui.ctx(), ui.style(), &shortcuts);
                    assert!(size.x < 300.0 && size.y < 200.0);
                    assert!(size.y >= 5.0 * METRICS.menu.row_height);
                    show_layout_popup_ui(ui, state, &shortcuts, action);
                },
                None,
            );
        harness.run();
        for command in &LAYOUT_COMMANDS[..3] {
            let label = format!(
                "{} {}",
                command_spec(*command).title,
                row_shortcut(&harness.ctx, &shortcuts, *command)
            );
            assert!(harness.get_by_label(&label).accesskit_node().is_disabled());
        }
        assert!(harness.state().is_none());
    }
}
