//! Presentation state and typed actions for the keyboard-shortcut editor.
//!
//! This module deliberately does not own settings or apply mutations. It
//! renders a snapshot and returns an intent for the application owner to
//! handle. That
//! keeps the frequently-painted native child view from cloning `AppSettings`
//! or reaching into application workflow state.

use super::METRICS;
use crate::{settings::AppSettings, shortcuts::ShortcutAction};
use eframe::egui::{self, RichText};

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
    let current = input.pending_settings.unwrap_or(input.settings);
    let bindings = current.effective_shortcuts();
    let mut selected_action = None;

    ui.horizontal(|ui| {
        ui.add(
            egui::TextEdit::singleline(&mut state.query)
                .hint_text("Search actions or groups")
                .desired_width(f32::INFINITY),
        );
        if ui.button("Reset all").clicked() {
            selected_action = Some(ShortcutEditorAction::ResetAll);
        }
    });
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
                ui.horizontal(|ui| {
                    let controls_width = 290.0;
                    let label_width = (ui.available_width() - controls_width).max(100.0);
                    ui.add_sized(
                        [label_width, METRICS.menu.row_height],
                        egui::Label::new(shortcut_action.label()),
                    );
                    ui.add_sized(
                        [100.0, METRICS.menu.row_height],
                        egui::Label::new(
                            RichText::new(
                                bindings
                                    .display(shortcut_action)
                                    .unwrap_or_else(|| "Unassigned".to_owned()),
                            )
                            .monospace(),
                        ),
                    );
                    if ui
                        .selectable_label(state.capture == Some(shortcut_action), "Change")
                        .clicked()
                    {
                        selected_action
                            .get_or_insert(ShortcutEditorAction::BeginCapture(shortcut_action));
                    }
                    if ui.button("Disable").clicked() {
                        selected_action
                            .get_or_insert(ShortcutEditorAction::Disable(shortcut_action));
                    }
                    if ui
                        .add_enabled(
                            current.shortcut_overrides.get(shortcut_action).is_some(),
                            egui::Button::new("Reset"),
                        )
                        .clicked()
                    {
                        selected_action.get_or_insert(ShortcutEditorAction::Reset(shortcut_action));
                    }
                });
            }
            if shown == 0 {
                ui.label(RichText::new("No shortcut actions found").weak());
            }
        });

    selected_action
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
    fn editor_renders_from_a_settings_snapshot_without_an_app_owner() {
        let context = egui::Context::default();
        let settings = AppSettings::default();
        let mut state = ShortcutEditorState::default();
        context
            .run_ui(Default::default(), |ui| {
                assert!(
                    show(
                        ui,
                        &mut state,
                        ShortcutEditorInput {
                            settings: &settings,
                            pending_settings: None,
                        },
                    )
                    .is_none()
                );
            })
            .drop_without_applying_deltas();
    }
}
