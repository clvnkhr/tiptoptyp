//! Presentation state and typed actions for the keyboard-shortcut editor.
//!
//! This module deliberately does not own settings or apply mutations. It
//! renders a snapshot and returns an intent for the application owner to
//! handle. That
//! keeps the frequently-painted native child view from cloning `AppSettings`
//! or reaching into application workflow state.

use crate::{settings::AppSettings, shortcuts::ShortcutAction};
use eframe::egui::{self, Align, Layout, RichText, Vec2};

#[derive(Debug, Default)]
pub(super) struct ShortcutEditorState {
    pub(super) query: String,
    pub(super) capture: Option<ShortcutAction>,
    pub(super) notice: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ShortcutEditorAction {
    BeginCapture(ShortcutAction),
    Disable(ShortcutAction),
    Reset(ShortcutAction),
    ResetAll,
}

pub(super) struct ShortcutEditorInput<'a> {
    pub(super) settings: &'a AppSettings,
    pub(super) pending_settings: Option<&'a AppSettings>,
}

pub(super) fn show(
    ui: &mut egui::Ui,
    state: &mut ShortcutEditorState,
    input: ShortcutEditorInput<'_>,
) -> Option<ShortcutEditorAction> {
    crate::theme::apply_compact_tool_spacing(ui);
    let current = input.pending_settings.unwrap_or(input.settings);
    let bindings = current.effective_shortcuts();
    let mut selected_action = None;
    let controls_width = action_controls_width(ui);

    let row_height = ui
        .text_style_height(&egui::TextStyle::Body)
        .max(ui.text_style_height(&egui::TextStyle::Button))
        + ui.spacing().button_padding.y * 2.0;
    ui.allocate_ui_with_layout(
        Vec2::new(
            ui.available_width(),
            row_height.max(ui.spacing().interact_size.y),
        ),
        Layout::right_to_left(Align::Center),
        |ui| {
            if crate::app::icons::action_button(ui, "Reset all").clicked() {
                selected_action = Some(ShortcutEditorAction::ResetAll);
            }
            ui.add(
                egui::TextEdit::singleline(&mut state.query)
                    .hint_text("Search actions or groups")
                    .horizontal_align(Align::Min)
                    .desired_width(ui.available_width().max(1.0)),
            );
        },
    );
    if let Some(capture) = state.capture {
        ui.colored_label(
            ui.visuals().selection.stroke.color,
            format!(
                "Press the new shortcut for {} · Backspace disables · Esc cancels",
                capture.label()
            ),
        );
    } else if let Some(message) = state.notice.as_deref() {
        ui.label(RichText::new(message).weak());
    }
    ui.separator();

    let terms = state
        .query
        .split_whitespace()
        .map(str::to_lowercase)
        .collect::<Vec<_>>();
    if !bindings.conflicts().is_empty() {
        ui.colored_label(
            ui.visuals().warn_fg_color,
            format!(
                "{} saved shortcut conflict{} could not be activated",
                bindings.conflicts().len(),
                if bindings.conflicts().len() == 1 {
                    ""
                } else {
                    "s"
                }
            ),
        );
    }
    egui::ScrollArea::vertical()
        .id_salt("shortcut-editor-scroll")
        .auto_shrink([false, false])
        .show(ui, |ui| {
            let mut group = None;
            let mut shown = 0usize;
            for shortcut_action in ShortcutAction::ALL {
                let haystack = format!(
                    "{} {} {}",
                    shortcut_action.group(),
                    shortcut_action.label(),
                    shortcut_action.id()
                )
                .to_lowercase();
                if !terms.iter().all(|term| haystack.contains(term)) {
                    continue;
                }
                if group != Some(shortcut_action.group()) {
                    if group.is_some() {
                        ui.separator();
                    }
                    ui.label(RichText::new(shortcut_action.group()).strong());
                    group = Some(shortcut_action.group());
                }
                shown += 1;
                let binding = bindings
                    .display(shortcut_action)
                    .unwrap_or_else(|| "Unassigned".to_owned());
                if let Some(action) = action_row(
                    ui,
                    state.capture,
                    shortcut_action,
                    &binding,
                    current.shortcut_overrides.get(shortcut_action).is_some(),
                    controls_width,
                ) {
                    selected_action.get_or_insert(action);
                }
            }
            if shown == 0 {
                ui.label(RichText::new("No shortcut actions found").weak());
            }
        });

    selected_action
}

fn action_controls_width(ui: &egui::Ui) -> f32 {
    let style = ui.style().button_style(
        &egui::widget_style::Classes::default(),
        egui::widget_style::WidgetState::Inactive,
    );
    let font = style.text_style.font_id;
    let gap = ui.spacing().item_spacing.x;
    ["Change", "Disable", "Reset"]
        .into_iter()
        .map(|label| {
            ui.fonts_mut(|fonts| {
                fonts.layout_no_wrap(label.into(), font.clone(), egui::Color32::WHITE)
            })
            .size()
            .x + style.frame.inner_margin.sum().x
        })
        .sum::<f32>()
        + gap * 2.0
        + 4.0
}

fn action_row(
    ui: &mut egui::Ui,
    capture: Option<ShortcutAction>,
    action: ShortcutAction,
    binding: &str,
    reset_enabled: bool,
    controls_width: f32,
) -> Option<ShortcutEditorAction> {
    ui.push_id(action, |ui| {
        let gap = ui.spacing().item_spacing.x;
        let available = ui.available_width();
        let row_height = ui
            .spacing()
            .interact_size
            .y
            .max(ui.text_style_height(&egui::TextStyle::Body));
        let mut selected = None;
        let mut controls = |ui: &mut egui::Ui| {
            if ui
                .selectable_label(capture == Some(action), "Change")
                .clicked()
            {
                selected = Some(ShortcutEditorAction::BeginCapture(action));
            }
            if crate::app::icons::action_button(ui, "Disable").clicked() {
                selected.get_or_insert(ShortcutEditorAction::Disable(action));
            }
            if crate::app::icons::action_button_enabled(ui, reset_enabled, "Reset").clicked() {
                selected.get_or_insert(ShortcutEditorAction::Reset(action));
            }
        };
        if available < controls_width + 120.0 + gap * 2.0 {
            ui.horizontal(|ui| {
                let binding_width = (available * 0.3).clamp(50.0, 112.0);
                ui.add_sized(
                    [(available - binding_width - gap).max(1.0), row_height],
                    egui::Label::new(action.label()).wrap(),
                );
                ui.add_sized(
                    [binding_width, row_height],
                    egui::Label::new(RichText::new(binding).monospace()).wrap(),
                );
            });
            ui.horizontal_wrapped(&mut controls);
        } else {
            let flexible = available - controls_width - gap * 2.0;
            let binding_width = (flexible * 0.4).clamp(50.0, 112.0);
            ui.horizontal(|ui| {
                ui.add_sized(
                    [(flexible - binding_width).max(1.0), row_height],
                    egui::Label::new(action.label()).wrap(),
                );
                ui.add_sized(
                    [binding_width, row_height],
                    egui::Label::new(RichText::new(binding).monospace()).wrap(),
                );
                ui.allocate_ui_with_layout(
                    Vec2::new(controls_width, row_height),
                    Layout::left_to_right(Align::Center),
                    controls,
                );
            });
        }
        selected
    })
    .inner
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn presentation_actions_are_typed_and_do_not_embed_settings() {
        assert_eq!(
            ShortcutEditorAction::BeginCapture(ShortcutAction::Find),
            ShortcutEditorAction::BeginCapture(ShortcutAction::Find)
        );
        assert_ne!(
            ShortcutEditorAction::Disable(ShortcutAction::Find),
            ShortcutEditorAction::Reset(ShortcutAction::Find)
        );
        assert!(ShortcutEditorState::default().query.is_empty());
    }

    #[test]
    fn full_shortcut_window_keeps_search_at_top_and_first_action_visible() {
        use egui_kittest::{
            Harness,
            kittest::{Queryable as _, by},
        };
        let settings = AppSettings::default();
        let mut harness = Harness::builder()
            .with_size(Vec2::new(620.0, 540.0))
            .build_ui_state(
                |ui, state: &mut ShortcutEditorState| {
                    show(
                        ui,
                        state,
                        ShortcutEditorInput {
                            settings: &settings,
                            pending_settings: None,
                        },
                    );
                },
                ShortcutEditorState::default(),
            );
        harness.run();
        let search = harness
            .get(by().role(egui::accesskit::Role::TextInput).value(""))
            .rect();
        let first = harness.get_by_label("Settings").rect();
        assert!(
            search.top() <= 16.0,
            "search row uses remaining window height: {search:?}"
        );
        assert!(
            first.top() > search.bottom() && first.top() < 100.0,
            "first action: {first:?}"
        );
        assert!(
            first.bottom() < 540.0,
            "first action is outside viewport: {first:?}"
        );
    }

    #[test]
    fn compact_shortcut_search_and_row_actions_fit_minimum_and_large_windows() {
        use egui_kittest::{Harness, kittest::Queryable as _};
        for width in [360.0, 440.0, 620.0, 1000.0] {
            let settings = AppSettings::default();
            let mut harness = Harness::builder()
                .with_size(Vec2::new(width, 400.0))
                .build_ui_state(
                    |ui, state: &mut ShortcutEditorState| {
                        show(
                            ui,
                            state,
                            ShortcutEditorInput {
                                settings: &settings,
                                pending_settings: None,
                            },
                        );
                    },
                    ShortcutEditorState {
                        query: "application.settings".into(),
                        ..Default::default()
                    },
                );
            harness.run();
            for label in ["Reset all", "Settings", "Change", "Disable", "Reset"] {
                let bounds = harness.get_by_label(label).rect();
                assert!(
                    bounds.left() >= 0.0 && bounds.right() <= width,
                    "{width}px {label}: {bounds:?}"
                );
            }
        }
    }

    #[test]
    fn compact_shortcut_actions_stay_typed_and_reachable_at_large_fonts() {
        use egui_kittest::{Harness, kittest::Queryable as _};
        let mut harness = Harness::builder()
            .with_size(Vec2::new(440.0, 300.0))
            .build_ui_state(
                |ui, selected: &mut Option<ShortcutEditorAction>| {
                    crate::theme::apply_compact_tool_spacing(ui);
                    let width = action_controls_width(ui);
                    if let Some(action) =
                        action_row(ui, None, ShortcutAction::Find, "⌘⇧F", true, width)
                    {
                        *selected = Some(action);
                    }
                },
                None,
            );
        harness.ctx.style_mut_of(harness.ctx.theme(), |style| {
            for kind in [egui::TextStyle::Body, egui::TextStyle::Button] {
                style
                    .text_styles
                    .insert(kind, egui::FontId::proportional(28.0));
            }
        });
        harness.run();
        for (label, action) in [
            (
                "Change",
                ShortcutEditorAction::BeginCapture(ShortcutAction::Find),
            ),
            (
                "Disable",
                ShortcutEditorAction::Disable(ShortcutAction::Find),
            ),
            ("Reset", ShortcutEditorAction::Reset(ShortcutAction::Find)),
        ] {
            let bounds = harness.get_by_label(label).rect();
            assert!(
                bounds.right() <= 440.0 && bounds.bottom() <= 300.0,
                "{label}: {bounds:?}"
            );
            harness.get_by_label(label).click();
            harness.run();
            assert_eq!(*harness.state(), Some(action));
        }
    }

    #[test]
    fn editor_renders_from_a_settings_snapshot_without_an_app_owner() {
        use egui_kittest::Harness;
        let settings = AppSettings::default();
        let mut harness = Harness::builder().build_ui_state(
            |ui, (state, settings): &mut (ShortcutEditorState, AppSettings)| {
                assert!(
                    show(
                        ui,
                        state,
                        ShortcutEditorInput {
                            settings,
                            pending_settings: None,
                        },
                    )
                    .is_none()
                );
            },
            (ShortcutEditorState::default(), settings),
        );
        harness.run();
    }
}
