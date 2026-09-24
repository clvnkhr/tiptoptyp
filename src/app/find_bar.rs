//! Find/Replace presentation state and control events.
//!
//! The bar owns its query controls and the existing revision-keyed search
//! cache. It does not edit documents or decide whether an edit is admitted;
//! those effects remain in the document-window owner.

use super::{METRICS, UiIcon, icon_button, native_hover_text, theme, viewport_scoped_id};
use crate::{document::DocumentKey, search::SearchSession};
use eframe::egui::{self, RichText};

pub(super) struct FindBarState {
    pub(super) visible: bool,
    pub(super) replace_visible: bool,
    pub(super) query: String,
    pub(super) replacement: String,
    pub(super) search: SearchSession,
    pub(super) case_sensitive: bool,
    pub(super) regex: bool,
    pub(super) focus: bool,
    pub(super) native_height: Option<f32>,
}

impl Default for FindBarState {
    fn default() -> Self {
        Self {
            visible: false,
            replace_visible: false,
            query: String::new(),
            replacement: String::new(),
            search: SearchSession::default(),
            case_sensitive: true,
            regex: false,
            focus: false,
            native_height: None,
        }
    }
}

pub(super) const VIEWPORT_SALT: &str = "find-replace-overlay";

/// Owner-local bounds may cross the editor/preview divider, never the window edge.
pub(super) fn overlay_bounds(owner: egui::Rect, editor: egui::Rect, height: f32) -> egui::Rect {
    let inset = theme::SPACE
        .content
        .min(owner.width().min(owner.height()).max(0.0) * 0.25);
    let available = owner.shrink(inset);
    let width = METRICS
        .editor
        .find_overlay_max_width
        .min(available.width())
        .max(1.0);
    let height = height.min(available.height()).max(1.0);
    let x = (editor.left() + inset).clamp(
        available.left(),
        (available.right() - width).max(available.left()),
    );
    let y = (editor.top() + inset).clamp(
        available.top(),
        (available.bottom() - height).max(available.top()),
    );
    egui::Rect::from_min_size(egui::pos2(x, y), egui::vec2(width, height))
}

pub(super) fn query_id(context: &egui::Context) -> egui::Id {
    viewport_scoped_id(context, "find-query")
}

pub(super) fn replacement_id(context: &egui::Context) -> egui::Id {
    viewport_scoped_id(context, "find-replacement")
}

pub(super) fn overlay_layer(context: &egui::Context) -> egui::LayerId {
    egui::LayerId::new(
        egui::Order::Foreground,
        viewport_scoped_id(context, "find-replace-overlay"),
    )
}

pub(super) fn has_focus(context: &egui::Context) -> bool {
    let child = crate::child_view::scoped_child_viewport_id(context, VIEWPORT_SALT);
    if context.input(|input| {
        input
            .raw
            .viewports
            .get(&child)
            .is_some_and(|info| info.focused == Some(true))
    }) {
        return true;
    }
    let focused = context.memory(|memory| memory.focused());
    focused.is_some_and(|id| {
        id == query_id(context)
            || id == replacement_id(context)
            || context
                .read_response(id)
                .is_some_and(|response| response.layer_id == overlay_layer(context))
    })
}

impl FindBarState {
    pub(super) fn close(&mut self) {
        self.visible = false;
        self.replace_visible = false;
        self.focus = false;
        // Closing changes presentation only. The revision/query-keyed session
        // keeps the selected match until the document or query actually changes.
    }
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub(super) struct FindBarActions {
    pub(super) previous: bool,
    pub(super) next: bool,
    pub(super) replace_one: bool,
    pub(super) replace_all: bool,
    pub(super) closed: bool,
}

pub(super) fn show(
    ui: &mut egui::Ui,
    state: &mut FindBarState,
    source: &str,
    document_key: DocumentKey,
    editable: bool,
) -> FindBarActions {
    let match_status = {
        let search_results = state.search.results(
            source,
            document_key,
            &state.query,
            state.case_sensitive,
            state.regex,
        );
        if search_results.error().is_some() {
            "Invalid pattern".to_owned()
        } else {
            let total = search_results.len();
            format!("{}/{total}", state.search.selected_ordinal().unwrap_or(0))
        }
    };
    let mut actions = FindBarActions::default();
    let mut query_changed = false;

    ui.horizontal_wrapped(|ui| {
        let response = ui.add(
            egui::TextEdit::singleline(&mut state.query)
                .id(query_id(ui.ctx()))
                .hint_text("Find")
                .desired_width(METRICS.editor.find_field_width),
        );
        if state.focus {
            response.request_focus();
            state.focus = false;
        }
        if response.changed() {
            state.search.clear();
            query_changed = true;
        }
        if response.lost_focus() && ui.input(|input| input.key_pressed(egui::Key::Enter)) {
            if ui.input(|input| input.modifiers.shift) {
                actions.previous = true;
            } else {
                actions.next = true;
            }
            // Single-line TextEdit relinquishes focus on Enter. Return it
            // immediately so repeated Enter/Shift+Enter keeps navigating.
            response.request_focus();
        }
        ui.label(
            RichText::new(match_status)
                .size(theme::TYPE.supporting)
                .weak(),
        );
        let case_label = if state.case_sensitive { "Aa" } else { "aa" };
        if native_hover_text(
            ui.selectable_label(state.case_sensitive, case_label),
            if state.case_sensitive {
                "Case-sensitive matching"
            } else {
                "Case-insensitive matching"
            },
        )
        .clicked()
        {
            state.case_sensitive = !state.case_sensitive;
            state.search.clear();
        }
        let regex_button = native_hover_text(
            ui.selectable_label(state.regex, ".*"),
            "Regular expression mode. Supports ., *, +, ?, [], ^, $, \\d, \\w, and \\s.",
        );
        regex_button.context_menu(|ui| {
            ui.label(RichText::new("Regular expressions").strong());
            ui.label(".  any character");
            ui.label("*  zero or more · +  one or more");
            ui.label("?  optional · ^  start · $  end");
            ui.label("[abc] [a-z] [^0-9]  character classes");
            ui.label("\\d digit · \\w word · \\s whitespace");
            ui.label("Replacement text is literal; capture expansion is not supported.");
        });
        if regex_button.clicked() {
            state.regex = !state.regex;
            state.search.clear();
        }
        if ui
            .selectable_label(state.replace_visible, "Replace")
            .on_hover_text("Show or hide replace fields")
            .clicked()
        {
            state.replace_visible = !state.replace_visible;
        }
        actions.previous |= icon_button(ui, UiIcon::Up, "Previous match · Shift+Enter").clicked();
        actions.next |= icon_button(ui, UiIcon::Down, "Next match · Enter").clicked();
        if icon_button(ui, UiIcon::Close, "Close · Esc").clicked() {
            state.close();
            actions.closed = true;
        }
    });

    if state.replace_visible {
        ui.horizontal(|ui| {
            ui.add(
                egui::TextEdit::singleline(&mut state.replacement)
                    .id(replacement_id(ui.ctx()))
                    .hint_text("Replace")
                    .desired_width(METRICS.editor.find_field_width),
            );
            actions.replace_one |=
                crate::app::icons::action_button_enabled(ui, editable, "Replace").clicked();
            actions.replace_all |=
                crate::app::icons::action_button_enabled(ui, editable, "Replace all").clicked();
        });
    }

    if query_changed {
        actions.next = true;
    }
    actions
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn replace_all_has_its_own_accessible_action_and_respects_read_only() {
        use egui_kittest::{
            Harness,
            kittest::{NodeT as _, Queryable as _},
        };
        for editable in [true, false] {
            let state = FindBarState {
                replace_visible: true,
                query: "alpha".into(),
                ..Default::default()
            };
            let mut harness = Harness::builder()
                .with_size(egui::vec2(620.0, 160.0))
                .build_ui_state(
                    move |ui, (state, replaced): &mut (FindBarState, bool)| {
                        let actions = show(
                            ui,
                            state,
                            "alpha alpha",
                            DocumentKey::new(
                                tiptoptyp_core::document::WindowSessionId::new(1),
                                0,
                                0,
                            ),
                            editable,
                        );
                        *replaced |= actions.replace_all;
                        assert!(!actions.replace_one);
                    },
                    (state, false),
                );
            harness.run();
            let button =
                harness.get_by_role_and_label(egui::accesskit::Role::Button, "Replace all");
            if editable {
                button.click();
                harness.run();
                assert!(harness.state().1);
            } else {
                assert!(button.accesskit_node().is_disabled());
                assert!(!harness.state().1);
            }
        }
    }

    #[test]
    fn overlay_crosses_the_editor_divider_but_stays_inside_its_owner() {
        let owner = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1000.0, 600.0));
        let editor = egui::Rect::from_min_max(egui::pos2(200.0, 60.0), egui::pos2(480.0, 550.0));
        let bounds = overlay_bounds(owner, editor, 100.0);
        assert!(bounds.right() > editor.right());
        assert!(owner.contains_rect(bounds));
        assert_eq!(bounds.width(), METRICS.editor.find_overlay_max_width);
        for size in [egui::vec2(320.0, 200.0), egui::vec2(1.0, 1.0)] {
            let owner = egui::Rect::from_min_size(egui::Pos2::ZERO, size);
            let bounds = overlay_bounds(owner, owner, 500.0);
            assert!(bounds.is_finite() && bounds.is_positive());
            if size.x > 1.0 {
                assert!(owner.contains_rect(bounds));
            }
        }
    }

    #[test]
    fn default_bar_is_closed_and_keeps_search_state_local() {
        let state = FindBarState::default();
        assert!(!state.visible);
        assert!(state.query.is_empty());
        assert!(state.replacement.is_empty());
        assert!(state.case_sensitive);
        assert!(!state.regex);
    }

    #[test]
    fn actions_are_plain_control_intents() {
        let actions = FindBarActions {
            next: true,
            replace_all: true,
            ..Default::default()
        };
        assert!(actions.next && actions.replace_all);
        assert!(!actions.closed);
    }

    #[test]
    fn bar_renders_from_local_state_without_an_app_owner() {
        let context = egui::Context::default();
        let mut state = FindBarState::default();
        let source = "alpha beta";
        context
            .run_ui(Default::default(), |ui| {
                let actions = show(
                    ui,
                    &mut state,
                    source,
                    DocumentKey::new(tiptoptyp_core::document::WindowSessionId::new(1), 0, 0),
                    true,
                );
                assert!(!actions.next);
                assert_eq!(
                    state
                        .search
                        .results(
                            source,
                            DocumentKey::new(
                                tiptoptyp_core::document::WindowSessionId::new(1),
                                0,
                                0
                            ),
                            "",
                            true,
                            false
                        )
                        .len(),
                    0
                );
            })
            .drop_without_applying_deltas();
    }
}
