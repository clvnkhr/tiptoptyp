//! Local TeX transformations own one source revision from menu to acceptance.
use super::*;
use crate::git::editor::{document_diff::DocumentDiff, full_view};
use crate::tex::refactor::{InlineScope, Plan};

pub(super) const SALT: &str = "tiptoptyp-tex-refactor";
pub(super) const MENU_LABELS: [&str; 3] = [
    "Inline command…",
    "Inline all uses in this file…",
    "Extract selection to command…",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Operation {
    Inline,
    InlineAll,
    Extract,
}
impl Operation {
    pub(super) const ALL: [Self; 3] = [Self::Inline, Self::InlineAll, Self::Extract];
    fn title(self) -> &'static str {
        match self {
            Self::Inline => "Inline command",
            Self::InlineAll => "Inline all uses",
            Self::Extract => "Extract command",
        }
    }
    #[cfg(all(feature = "desktop-ui-tests", target_os = "macos"))]
    pub(super) fn target(self) -> &'static str {
        match self {
            Self::Inline => "tex_refactor.inline",
            Self::InlineAll => "tex_refactor.inline_all",
            Self::Extract => "tex_refactor.extract",
        }
    }
}

#[derive(Debug, Clone)]
pub(super) struct Target {
    tab: u64,
    snapshot: crate::document::DocumentSnapshot,
    caret_character: usize,
    selection_characters: Range<usize>,
    cursor: CCursorRange,
}
impl PartialEq for Target {
    fn eq(&self, other: &Self) -> bool {
        self.tab == other.tab
            && self.snapshot.key() == other.snapshot.key()
            && self.caret_character == other.caret_character
            && self.selection_characters == other.selection_characters
            && self.cursor == other.cursor
    }
}
impl Eq for Target {}
impl Target {
    pub(super) fn capture(
        app: &EditorApp,
        location: preview_jump::SourceLocation,
        selection: Option<&liquid::BoundSelection>,
    ) -> Option<Self> {
        if app.tabs.is_empty()
            || app.document().kind() != DocumentKind::Tex
            || app.source_edit_blocked()
            || location.key != app.document().key()
        {
            return None;
        }
        let cursor = selection
            .and_then(|selection| selection.selection_cursor(location.key))
            .unwrap_or_else(|| CCursorRange::one(CCursor::new(location.char_index)));
        let selected = cursor.as_sorted_char_range();
        Some(Self {
            tab: app.tabs.active_id()?,
            snapshot: app.document().snapshot(),
            caret_character: location.char_index,
            selection_characters: selected.start.0..selected.end.0,
            cursor,
        })
    }
    pub(super) fn has_selection(&self) -> bool {
        !self.selection_characters.is_empty()
    }
    fn caret_byte(&self) -> Result<usize, String> {
        byte_offset(self.snapshot.source(), self.caret_character)
            .ok_or_else(|| "The source position changed. Reopen the menu.".into())
    }
    fn selection_bytes(&self) -> Result<Range<usize>, String> {
        let source = self.snapshot.source();
        let start = byte_offset(source, self.selection_characters.start);
        let end = byte_offset(source, self.selection_characters.end);
        match (start, end) {
            (Some(start), Some(end)) => Ok(start..end),
            _ => Err("The selection changed. Select the expression again.".into()),
        }
    }
    fn current(&self, app: &EditorApp) -> bool {
        app.tabs.active_id() == Some(self.tab)
            && !app.tabs.is_empty()
            && app.document().key() == self.snapshot.key()
            && app.document().kind() == DocumentKind::Tex
    }
}

fn byte_offset(source: &str, character: usize) -> Option<usize> {
    source
        .char_indices()
        .map(|(byte, _)| byte)
        .chain(std::iter::once(source.len()))
        .nth(character)
}

#[derive(Default)]
pub(super) enum State {
    #[default]
    Closed,
    Naming(Draft),
    Reviewing(Review),
    Rejected {
        target: Target,
        message: String,
    },
    Stale,
}
pub(super) struct Draft {
    target: Target,
    name: String,
    error: Option<String>,
    focus_name: bool,
}
pub(super) struct Review {
    target: Target,
    plan: Plan,
    /// Display context only; Target and Plan retain both full source documents.
    diff: DocumentDiff,
    presentation: Presentation,
}

enum Presentation {
    AwaitingFirstPaint,
    Presented,
}

impl Review {
    fn present(&mut self, ui: &egui::Ui, id: egui::Id) {
        if matches!(self.presentation, Presentation::AwaitingFirstPaint) {
            full_view::reset_comparison_scroll(ui, id);
            self.presentation = Presentation::Presented;
        }
    }
}

/// Keep the first changed alignment row visible when opening a late-file edit.
/// Retain every changed row and everything between the first and last change;
/// only outside unchanged lines can be omitted. This runs once per proposal.
fn review_diff(before: &str, after: &str) -> DocumentDiff {
    let pairs = crate::git::editor::document_diff::align_lines(before, after);
    let Some(first) = pairs.iter().position(|pair| pair.changed) else {
        return DocumentDiff::new(before.into(), after);
    };
    let last = pairs
        .iter()
        .rposition(|pair| pair.changed)
        .expect("a first change has a last change");
    let start = first.saturating_sub(3);
    let end = last.saturating_add(4).min(pairs.len());
    let before_lines: Vec<_> = before.split_inclusive('\n').collect();
    let after_lines: Vec<_> = after.split_inclusive('\n').collect();
    let mut old = String::new();
    let mut new = String::new();
    for pair in &pairs[start..end] {
        if let Some(line) = pair.old {
            old.push_str(before_lines[line]);
        }
        if let Some(line) = pair.new {
            new.push_str(after_lines[line]);
        }
    }
    DocumentDiff::new(old, &new)
}

impl State {
    pub(super) fn visible(&self) -> bool {
        !matches!(self, Self::Closed)
    }
    fn owner(&self) -> Option<&Target> {
        match self {
            Self::Naming(draft) => Some(&draft.target),
            Self::Reviewing(review) => Some(&review.target),
            Self::Rejected { target, .. } => Some(target),
            Self::Closed | Self::Stale => None,
        }
    }
    fn review(target: Target, result: Result<Plan, String>) -> Self {
        match result {
            Ok(plan) => {
                let diff = review_diff(target.snapshot.source(), plan.preview());
                Self::Reviewing(Review {
                    target,
                    plan,
                    diff,
                    presentation: Presentation::AwaitingFirstPaint,
                })
            }
            Err(message) => Self::Rejected { target, message },
        }
    }
    #[cfg(any(test, all(feature = "desktop-ui-tests", target_os = "macos")))]
    fn phase(&self) -> &'static str {
        match self {
            Self::Closed => "closed",
            Self::Naming(_) => "naming",
            Self::Reviewing(_) => "reviewing",
            Self::Rejected { .. } => "rejected",
            Self::Stale => "stale",
        }
    }
}

impl EditorApp {
    pub(super) fn begin_tex_refactor(
        &mut self,
        target: Target,
        operation: Operation,
        context: &egui::Context,
    ) {
        if !target.current(self) || self.source_edit_blocked() || self.document_flow_busy() {
            return;
        }
        self.auto_pair_input.cancel();
        self.close_app_popup();
        self.clear_editor_hover();
        self.clear_asset_hover();
        self.find_bar.close();
        ChildViewHost::close(context, find_bar::VIEWPORT_SALT);
        self.tex_refactor = match operation {
            Operation::Extract => State::Naming(Draft {
                target,
                name: String::new(),
                error: None,
                focus_name: true,
            }),
            Operation::Inline | Operation::InlineAll => {
                let scope = if operation == Operation::InlineAll {
                    InlineScope::AllOccurrences
                } else {
                    InlineScope::Occurrence
                };
                let result = target.caret_byte().and_then(|caret| {
                    crate::tex::refactor::inline(target.snapshot.source(), caret, scope)
                });
                State::review(target, result)
            }
        };
        context.request_repaint();
    }

    pub(super) fn update_tex_refactor(&mut self) {
        if self
            .tex_refactor
            .owner()
            .is_some_and(|owner| !owner.current(self))
        {
            self.tex_refactor = State::Stale;
        }
    }

    pub(super) fn close_tex_refactor(&mut self, context: &egui::Context) {
        self.tex_refactor = State::Closed;
        ChildViewHost::close(context, SALT);
        crate::window_host::return_focus(context);
    }

    fn preview_tex_extraction(&mut self) {
        if !matches!(self.tex_refactor, State::Naming(_)) {
            return;
        }
        let State::Naming(mut draft) = std::mem::take(&mut self.tex_refactor) else {
            return;
        };
        if !draft.target.current(self) || self.source_edit_blocked() {
            self.tex_refactor = State::Stale;
            return;
        }
        match draft.target.selection_bytes().and_then(|selection| {
            crate::tex::refactor::extract(draft.target.snapshot.source(), selection, &draft.name)
        }) {
            Ok(plan) => self.tex_refactor = State::review(draft.target, Ok(plan)),
            Err(error) => {
                draft.error = Some(error);
                self.tex_refactor = State::Naming(draft);
            }
        }
    }

    fn accept_tex_refactor(&mut self) -> Result<(), String> {
        if !matches!(self.tex_refactor, State::Reviewing(_)) {
            return Ok(());
        }
        let State::Reviewing(review) = std::mem::take(&mut self.tex_refactor) else {
            return Ok(());
        };
        if !review.target.current(self) || self.source_edit_blocked() {
            self.tex_refactor = State::Stale;
            return Err("The document changed. Request the refactoring again.".into());
        }
        let applied = match review.plan.apply(self.document().source()) {
            Ok(applied) => applied,
            Err(message) => {
                self.tex_refactor = State::Rejected {
                    target: review.target,
                    message: message.clone(),
                };
                return Err(message);
            }
        };
        let Some(selected) = applied.source.get(applied.selection.clone()) else {
            self.tex_refactor = State::Rejected {
                target: review.target,
                message: "The proposed selection is invalid.".into(),
            };
            return Err("The proposed selection is invalid.".into());
        };
        let start = applied.source[..applied.selection.start].chars().count();
        let end = start + selected.chars().count();
        self.document_mut()
            .edit(review.target.cursor, |source| *source = applied.source);
        self.pending_editor_selection = Some(EditorSelection::Focus(start..end));
        self.find_bar.search.clear();
        self.mark_edited();
        Ok(())
    }

    pub(super) fn show_tex_refactor(&mut self, context: &egui::Context) {
        if !self.tex_refactor.visible() {
            return;
        }
        let Some(owner) = context.input(|input| input.viewport().inner_rect) else {
            return;
        };
        let style = context.style_of(context.theme());
        let shortcuts = self.settings.effective_shortcuts();
        let mut action = None;
        ChildViewHost::show(
            context,
            &self.captures,
            ChildViewSpec::modal(SALT, "TeX refactoring", owner, "tex-refactor"),
            context.theme(),
            &style,
            |ui, input| {
                handle_window_management_shortcuts(ui.ctx(), ui.ctx().viewport_id(), &shortcuts);
                if input.close_requested
                    || input.escape_pressed
                    || [ShortcutAction::CloseWindow, ShortcutAction::CloseTab]
                        .into_iter()
                        .any(|key| {
                            shortcuts.binding(key).is_some_and(|key| {
                                ui.input_mut(|input| input.consume_shortcut(&key.egui()))
                            })
                        })
                {
                    action = Some(merge_action(action, Action::Cancel));
                }
                ui.painter()
                    .rect_filled(ui.max_rect(), 0.0, Color32::from_black_alpha(64));
                if let Some(clicked) = review_card(
                    ui.ctx(),
                    &mut self.tex_refactor,
                    &self.settings,
                    ui.max_rect(),
                ) {
                    action = Some(merge_action(action, clicked));
                }
            },
        );
        match action {
            Some(Action::Cancel) => self.close_tex_refactor(context),
            Some(Action::Preview) => self.preview_tex_extraction(),
            Some(Action::Accept)
                if self.accept_tex_refactor().is_ok() && !self.tex_refactor.visible() =>
            {
                self.close_tex_refactor(context);
                self.focus_source_editor(context);
            }
            Some(Action::Accept) | None => {}
        }
    }

    #[cfg(all(feature = "desktop-ui-tests", target_os = "macos"))]
    pub(super) fn tex_refactor_inspection(&self) -> serde_json::Value {
        serde_json::json!({
            "visible": self.tex_refactor.visible(),
            "phase": self.tex_refactor.phase(),
            "current": self.tex_refactor.owner().is_some_and(|owner| owner.current(self)),
            "comparison": match &self.tex_refactor {
                State::Reviewing(review) => Some(serde_json::json!({
                    "before": crate::desktop_test::fingerprint(review.target.snapshot.source()),
                    "after": crate::desktop_test::fingerprint(review.plan.preview()),
                    "occurrences": review.plan.occurrences(),
                })),
                _ => None,
            },
        })
    }
}

#[derive(Clone, Copy)]
enum Action {
    Cancel,
    Preview,
    Accept,
}

/// Cancellation is absorbing within one native input batch, including a late
/// button release delivered together with Escape or a native close event.
fn merge_action(current: Option<Action>, incoming: Action) -> Action {
    if matches!(current, Some(Action::Cancel)) || matches!(incoming, Action::Cancel) {
        Action::Cancel
    } else {
        incoming
    }
}

fn review_card(
    context: &egui::Context,
    state: &mut State,
    settings: &AppSettings,
    bounds: Rect,
) -> Option<Action> {
    let style = context.style_of(context.theme());
    let mut action = None;
    egui::Area::new(viewport_scoped_id(context, "tex-refactor-card"))
        .order(egui::Order::Foreground)
        .anchor(egui::Align2::CENTER_CENTER, Vec2::ZERO)
        .show(context, |ui| {
            theme::dialog_card_frame(&style).show(ui, |ui| {
                ui.set_width((bounds.width() - 48.0).clamp(1.0, 960.0));
                // Area can retain its initial/naming size. Reset the content
                // limit from this child's actual viewport before scrolling so
                // the diff and footer can grow without stale clip feedback.
                let maximum = (bounds.height() - 48.0).max(1.0);
                ui.set_max_height(maximum);
                egui::ScrollArea::vertical()
                    .id_salt("tex-refactor-card-scroll")
                    .max_height(maximum)
                    .show(ui, |ui| {
                        action = review_ui(ui, state, settings, bounds.height());
                    });
            });
        });
    action
}

#[cfg(test)]
fn record_review_hit(name: &'static str, response: &egui::Response) {
    response.ctx.data_mut(|data| {
        data.insert_temp(
            egui::Id::new(("tex-refactor-hit", name)),
            response.interact_rect,
        );
    });
}

fn review_ui(
    ui: &mut egui::Ui,
    state: &mut State,
    settings: &AppSettings,
    height: f32,
) -> Option<Action> {
    let mut action = None;
    match state {
        State::Closed => return None,
        State::Naming(draft) => {
            ui.strong(Operation::Extract.title());
            ui.horizontal(|ui| {
                let label = ui.label("Command");
                let name = ui
                    .add(egui::TextEdit::singleline(&mut draft.name).hint_text("\\mycommand"))
                    .labelled_by(label.id);
                #[cfg(test)]
                record_review_hit("name", &name);
                if draft.focus_name {
                    name.request_focus();
                    draft.focus_name = false;
                }
                #[cfg(all(feature = "desktop-ui-tests", target_os = "macos"))]
                crate::desktop_test::observe("tex_refactor.name", &name);
            });
            if let Some(error) = &draft.error {
                ui.colored_label(ui.visuals().error_fg_color, error);
            }
        }
        State::Reviewing(review) => {
            ui.horizontal(|ui| {
                ui.strong(review.plan.label());
                if review.plan.occurrences() > 1 {
                    ui.label(format!("{} uses", review.plan.occurrences()));
                }
            });
            liquid::comparison_headers(ui, settings.git_diff_style);
            ui.allocate_ui(
                Vec2::new(ui.available_width(), (height - 190.0).clamp(40.0, 520.0)),
                |ui| {
                    let id = viewport_scoped_id(ui.ctx(), "tex-refactor-diff");
                    review.present(ui, id);
                    let viewport = full_view::show_styled_comparison(
                        ui,
                        &review.diff,
                        id,
                        Path::new("refactoring.tex"),
                        settings,
                    );
                    #[cfg(any(test, all(feature = "desktop-ui-tests", target_os = "macos")))]
                    let response = ui.interact(viewport, id.with("observed"), Sense::hover());
                    #[cfg(test)]
                    record_review_hit("diff", &response);
                    #[cfg(all(feature = "desktop-ui-tests", target_os = "macos"))]
                    crate::desktop_test::observe("tex_refactor.diff", &response);
                    #[cfg(not(any(test, all(feature = "desktop-ui-tests", target_os = "macos"))))]
                    let _ = viewport;
                },
            );
        }
        State::Rejected { message, .. } => {
            ui.strong("Cannot refactor this command");
            ui.colored_label(ui.visuals().error_fg_color, message);
        }
        State::Stale => {
            ui.colored_label(
                ui.visuals().error_fg_color,
                "The document changed. Close this review and request the refactoring again.",
            );
        }
    }
    ui.separator();
    ui.horizontal(|ui| {
        let naming = matches!(state, State::Naming(_));
        let accept = ui.add_enabled(
            matches!(state, State::Reviewing(_)) || naming,
            egui::Button::new(if naming { "Preview" } else { "Accept" }),
        );
        #[cfg(all(feature = "desktop-ui-tests", target_os = "macos"))]
        crate::desktop_test::observe(
            if naming {
                "tex_refactor.preview"
            } else {
                "tex_refactor.accept"
            },
            &accept,
        );
        #[cfg(test)]
        record_review_hit(if naming { "preview" } else { "accept" }, &accept);
        if accept.clicked() {
            action = Some(if naming {
                Action::Preview
            } else {
                Action::Accept
            });
        }
        let cancel = ui.button("Cancel");
        #[cfg(all(feature = "desktop-ui-tests", target_os = "macos"))]
        crate::desktop_test::observe("tex_refactor.cancel", &cancel);
        #[cfg(test)]
        record_review_hit("cancel", &cancel);
        if cancel.clicked() {
            action = Some(Action::Cancel);
        }
    });
    action
}

#[cfg(test)]
mod tests {
    use super::*;
    const SOURCE: &str = "% café α\r\n\\documentclass{article}\r\n\\newcommand{\\sq}[1]{#1^2}\r\n\\begin{document}\r\n$\\sq{x}_i + \\sq{y}$\r\n\\end{document}\r\n";

    fn fixture(context: &egui::Context, root: &Path) -> (EditorApp, Target) {
        let mut app = EditorApp::dormant_for_tests(context, root.into());
        app.snapshot_scene = None;
        app.settings.auto_save = false;
        let path = root.join("main.tex");
        fs::write(&path, SOURCE).unwrap();
        app.document_mut().replace_loaded_unprojected(
            SOURCE.into(),
            path,
            DocumentKind::Tex,
            Some(fingerprint(SOURCE.as_bytes())),
        );
        app.document_mut().set_history_reset(false);
        let byte = SOURCE.find("$\\sq").unwrap() + 2;
        let location = preview_jump::SourceLocation {
            key: app.document().key(),
            char_index: SOURCE[..byte].chars().count(),
        };
        let target = Target::capture(&app, location, None).unwrap();
        (app, target)
    }

    #[test]
    fn review_acceptance_is_one_unicode_aware_undo_transaction_and_cannot_reapply() {
        let root = tempfile::tempdir().unwrap();
        let context = egui::Context::default();
        let (mut app, target) = fixture(&context, root.path());
        let original_cursor = target.cursor;
        app.begin_tex_refactor(target, Operation::Inline, &context);
        let State::Reviewing(review) = &app.tex_refactor else {
            panic!("inline should be reviewable")
        };
        let proposed = review.plan.preview().to_owned();
        assert_ne!(proposed, SOURCE);
        assert_eq!(app.document().source(), SOURCE);
        app.accept_tex_refactor().unwrap();
        assert_eq!(app.document().source(), &proposed);
        let accepted_key = app.document().key();
        app.accept_tex_refactor().unwrap();
        assert_eq!(app.document().key(), accepted_key);
        let undo = app
            .document_mut()
            .history_step(false, CCursorRange::default())
            .unwrap();
        assert_eq!(app.document().source(), SOURCE);
        assert_eq!(undo.cursor, original_cursor);
        assert!(
            app.document_mut()
                .history_step(false, CCursorRange::default())
                .is_none()
        );
        app.document_mut()
            .history_step(true, CCursorRange::default())
            .unwrap();
        assert_eq!(app.document().source(), &proposed);
        assert_eq!(
            fs::read_to_string(root.path().join("main.tex")).unwrap(),
            SOURCE
        );
    }

    #[test]
    fn review_never_revives_after_source_changes_even_when_text_is_restored() {
        let root = tempfile::tempdir().unwrap();
        let context = egui::Context::default();
        let (mut app, target) = fixture(&context, root.path());
        app.begin_tex_refactor(target, Operation::InlineAll, &context);
        app.document_mut()
            .edit(CCursorRange::default(), |source| source.push('%'));
        app.update_tex_refactor();
        assert_eq!(app.tex_refactor.phase(), "stale");
        app.document_mut().edit(CCursorRange::default(), |source| {
            source.pop();
        });
        app.update_tex_refactor();
        app.accept_tex_refactor().unwrap();
        assert_eq!(app.tex_refactor.phase(), "stale");
        assert_eq!(app.document().source(), SOURCE);
    }

    #[test]
    fn exact_tab_is_required_even_if_a_source_key_matches() {
        let root = tempfile::tempdir().unwrap();
        let context = egui::Context::default();
        let (mut app, mut target) = fixture(&context, root.path());
        target.tab = target.tab.wrapping_add(1);
        app.begin_tex_refactor(target, Operation::Inline, &context);
        assert_eq!(app.tex_refactor.phase(), "closed");
        assert_eq!(app.document().source(), SOURCE);
    }

    #[test]
    fn late_preview_or_accept_in_another_phase_does_not_consume_current_work() {
        let root = tempfile::tempdir().unwrap();
        let context = egui::Context::default();
        let (mut app, target) = fixture(&context, root.path());
        app.begin_tex_refactor(target.clone(), Operation::Inline, &context);
        app.preview_tex_extraction();
        assert_eq!(app.tex_refactor.phase(), "reviewing");
        app.close_tex_refactor(&context);
        app.begin_tex_refactor(target, Operation::Extract, &context);
        app.accept_tex_refactor().unwrap();
        assert_eq!(app.tex_refactor.phase(), "naming");
        app.close_tex_refactor(&context);
        app.close_tex_refactor(&context);
        app.preview_tex_extraction();
        assert_eq!(app.tex_refactor.phase(), "closed");
    }

    #[test]
    fn failed_extraction_keeps_its_draft_for_correction_without_editing_source() {
        let root = tempfile::tempdir().unwrap();
        let context = egui::Context::default();
        let (mut app, mut target) = fixture(&context, root.path());
        let begin = SOURCE.find("x}_i").unwrap();
        let start = SOURCE[..begin].chars().count();
        target.selection_characters = start..start + 1;
        app.begin_tex_refactor(target, Operation::Extract, &context);
        let State::Naming(draft) = &mut app.tex_refactor else {
            panic!()
        };
        draft.name = "bad_name".into();
        app.preview_tex_extraction();
        let State::Naming(draft) = &mut app.tex_refactor else {
            panic!()
        };
        assert!(draft.error.is_some());
        assert_eq!(draft.name, "bad_name");
        assert_eq!(app.document().source(), SOURCE);
    }

    #[test]
    fn extraction_draft_becomes_stale_when_its_owner_changes() {
        let root = tempfile::tempdir().unwrap();
        let context = egui::Context::default();
        let (mut app, target) = fixture(&context, root.path());
        app.begin_tex_refactor(target, Operation::Extract, &context);
        app.document_mut()
            .edit(CCursorRange::default(), |source| source.push('%'));
        app.preview_tex_extraction();
        assert_eq!(app.tex_refactor.phase(), "stale");
    }

    #[test]
    fn byte_offsets_reject_invalid_scalar_positions_and_preserve_unicode() {
        assert_eq!(byte_offset("α🙂x", 0), Some(0));
        assert_eq!(byte_offset("α🙂x", 1), Some(2));
        assert_eq!(byte_offset("α🙂x", 2), Some(6));
        assert_eq!(byte_offset("α🙂x", 3), Some(7));
        assert_eq!(byte_offset("α🙂x", 4), None);
    }

    #[test]
    fn source_edit_commands_cannot_mutate_the_editor_beneath_a_review() {
        let root = tempfile::tempdir().unwrap();
        let context = egui::Context::default();
        let (mut app, target) = fixture(&context, root.path());
        app.begin_tex_refactor(target, Operation::Inline, &context);
        let key = app.document().key();
        for command in [
            AppCommand::Undo,
            AppCommand::Redo,
            AppCommand::Cut,
            AppCommand::Paste,
        ] {
            app.execute_app_command(command, &context, None);
            assert_eq!(app.document().key(), key);
            assert_eq!(app.tex_refactor.phase(), "reviewing");
        }
        app.execute_app_command(AppCommand::CloseTab, &context, None);
        assert_eq!(app.tex_refactor.phase(), "closed");
        assert_eq!(app.document().source(), SOURCE);
    }

    #[test]
    fn context_actions_share_geometry_and_extraction_requires_a_selection() {
        use egui_kittest::{
            Harness,
            kittest::{NodeT as _, Queryable as _},
        };
        let root = tempfile::tempdir().unwrap();
        let context = egui::Context::default();
        let (_, target) = fixture(&context, root.path());
        let mut harness = Harness::builder()
            .with_size(Vec2::new(800.0, 850.0))
            .build_ui_state(
                |ui, state: &mut (Target, Option<EditorMenuAction>)| {
                    show_editor_context_menu_ui(
                        ui,
                        EditorContextMenuOptions {
                            can_undo: false,
                            can_redo: false,
                            has_selection: state.0.has_selection(),
                            can_format: true,
                            can_sync_preview: false,
                            link: None,
                            table: None,
                            math: None,
                            liquid_selection: None,
                            diagnostic_fix: None,
                            refactor: Some(&state.0),
                        },
                        &AppSettings::default().effective_shortcuts(),
                        &mut state.1,
                    );
                },
                (target.clone(), None),
            );
        harness.run_steps(3);
        assert!(
            harness
                .get_by_label_contains(MENU_LABELS[2])
                .accesskit_node()
                .is_disabled()
        );
        let bindings = AppSettings::default().effective_shortcuts();
        let ordinary = editor_menu_row_specs(false, false, None, true, false, &bindings, &|_| {
            String::new()
        });
        let extended = editor_menu_row_specs(false, false, None, true, true, &bindings, &|_| {
            String::new()
        });
        assert_eq!(extended.len(), ordinary.len() + MENU_LABELS.len());
        for label in MENU_LABELS {
            assert!(extended.iter().any(|row| row.label == label));
        }
        harness.get_by_label_contains(MENU_LABELS[1]).click();
        harness.run_steps(2);
        assert_eq!(
            harness.state().1,
            Some(EditorMenuAction::Refactor(target, Operation::InlineAll))
        );
    }

    #[test]
    fn review_cancel_control_retires_the_proposal_without_editing_source() {
        use egui_kittest::{Harness, kittest::Queryable as _};
        let root = tempfile::tempdir().unwrap();
        let context = egui::Context::default();
        let (mut app, target) = fixture(&context, root.path());
        app.begin_tex_refactor(target, Operation::Inline, &context);
        let mut harness = Harness::builder()
            .with_size(Vec2::new(1000.0, 850.0))
            .build_ui_state(
                |ui, state: &mut (EditorApp, bool)| {
                    let (app, fonts_ready) = state;
                    if !*fonts_ready {
                        theme::configure_editor_fonts(
                            ui.ctx(),
                            Default::default(),
                            Default::default(),
                            false,
                            400,
                            400,
                            None,
                        );
                        *fonts_ready = true;
                        return;
                    }
                    if let Some(action) = review_ui(ui, &mut app.tex_refactor, &app.settings, 800.0)
                    {
                        match action {
                            Action::Accept => {
                                app.accept_tex_refactor().unwrap();
                            }
                            Action::Cancel => app.tex_refactor = State::Closed,
                            Action::Preview => app.preview_tex_extraction(),
                        }
                    }
                },
                (app, false),
            );
        harness.run_steps(3);
        harness.get_by_label("Cancel").click();
        harness.run_steps(2);
        assert_eq!(harness.state().0.document().source(), SOURCE);
        assert_eq!(harness.state().0.tex_refactor.phase(), "closed");
    }

    #[test]
    fn every_visible_refactor_phase_locks_source_until_its_cancel_transition() {
        let root = tempfile::tempdir().unwrap();
        let context = egui::Context::default();
        let (mut app, target) = fixture(&context, root.path());
        let key = app.document().key();
        for (operation, reject, expected) in [
            (Operation::Extract, false, "naming"),
            (Operation::Inline, false, "reviewing"),
            (Operation::Inline, true, "rejected"),
        ] {
            let mut requested = target.clone();
            if reject {
                requested.caret_character = 0;
            }
            app.begin_tex_refactor(requested, operation, &context);
            assert_eq!(app.tex_refactor.phase(), expected);
            assert!(app.source_edit_blocked());
            app.insert_editor_text(&context, "unrequested edit");
            app.begin_tex_refactor(target.clone(), Operation::InlineAll, &context);
            assert_eq!(app.document().key(), key);
            assert_eq!(app.document().source(), SOURCE);
            assert_eq!(app.tex_refactor.phase(), expected);
            app.close_tex_refactor(&context);
            assert!(!app.source_edit_blocked());
        }
        app.begin_tex_refactor(target, Operation::Inline, &context);
        app.document_mut()
            .edit(CCursorRange::default(), |source| source.push('%'));
        app.update_tex_refactor();
        let changed = app.document().key();
        assert_eq!(app.tex_refactor.phase(), "stale");
        assert!(app.source_edit_blocked());
        app.insert_editor_text(&context, "unrequested edit");
        assert_eq!(app.document().key(), changed);
        assert_eq!(app.document().source(), &format!("{SOURCE}%"));
        app.close_tex_refactor(&context);
        assert!(!app.source_edit_blocked());
    }

    #[test]
    fn an_admitted_pending_tex_formatter_cannot_edit_or_save_beneath_review() {
        let root = tempfile::tempdir().unwrap();
        let context = egui::Context::default();
        let (mut app, target) = fixture(&context, root.path());
        let key = app.document().key();
        let cursor = CCursorRange::two(CCursor::new(12), CCursor::new(3));
        app.store_editor_cursor(&context, cursor);
        let mut settings = app.settings.tex.clone();
        settings.texlab_enabled = false;
        settings.lint = false;
        settings.formatter = crate::tex::settings::Formatter::Disabled;
        let uri: String = url::Url::from_file_path(root.path().join("main.tex"))
            .unwrap()
            .into();
        app.tex_service.synchronize(
            crate::tex::Snapshot {
                generation: Generation(0),
                key,
                uri: uri.clone(),
                source: SOURCE.into(),
                root: root.path().into(),
                settings,
                tools: app.tex_tools.clone(),
            },
            crate::worker::RepaintTarget::current(&context),
        );
        let generation = app.tex_service.identity().unwrap().generation;
        let token = app.formatting.request(key, formatting::Intent::Save);
        app.begin_tex_refactor(target, Operation::Inline, &context);
        assert!(app.source_edit_blocked());
        assert!(app.editor_accepts_reply(
            crate::tinymist_sync::ReplyIdentity {
                generation,
                uri: &uri,
                version: revision_as_i32(key.revision),
                key,
            },
            key,
        ));
        app.receive_formatted_document(
            &context,
            generation,
            &uri,
            revision_as_i32(key.revision),
            token,
            vec![LspTextEdit {
                range: LspRange {
                    start: lsp_position_at_scalar(SOURCE, ScalarOffset::new(0)),
                    end: lsp_position_at_scalar(SOURCE, ScalarOffset::new(SOURCE.chars().count())),
                },
                new_text: "Formatter must not replace this source.".into(),
            }],
        );
        assert_eq!(app.document().key(), key);
        assert_eq!(app.document().source(), SOURCE);
        assert_eq!(app.editor_snapshot(&context).cursor, cursor);
        assert_eq!(app.tex_refactor.phase(), "reviewing");
        assert!(app.formatting.request_key().is_none());
        assert!(!app.formatting.saves_after_reply());
        assert!(!app.local_save.is_running());
        assert_eq!(
            fs::read_to_string(root.path().join("main.tex")).unwrap(),
            SOURCE
        );
        app.close_tex_refactor(&context);
        assert!(!app.source_edit_blocked());
    }

    #[test]
    fn a_late_change_opens_with_three_context_lines_and_preserves_crlf() {
        let lines: Vec<_> = (0..600).map(|line| format!("line {line:03}\r\n")).collect();
        let before = lines.concat();
        let after = before.replace("line 500\r\n", "changed 500\r\n");
        let diff = review_diff(&before, &after);
        assert_eq!(diff.before, lines[497..504].concat());
        assert_eq!(diff.current, diff.before.replace("line 500", "changed 500"));
        assert_eq!(diff.pairs.iter().position(|pair| pair.changed), Some(3));
        assert_eq!(diff.pairs.iter().filter(|pair| pair.changed).count(), 1);
        assert!(!diff.before.contains("line 000"));
        assert!(!diff.before.replace("\r\n", "").contains('\n'));
    }

    #[test]
    fn separate_changed_hunks_keep_every_intervening_unchanged_line() {
        let lines: Vec<_> = (0..600).map(|line| format!("line {line:03}\n")).collect();
        let before = lines.concat();
        let after = before
            .replace("line 005\n", "changed 005\n")
            .replace("line 500\n", "changed 500\n");
        let diff = review_diff(&before, &after);
        assert_eq!(diff.before, lines[2..504].concat());
        assert_eq!(
            diff.current,
            diff.before
                .replace("line 005", "changed 005")
                .replace("line 500", "changed 500")
        );
        assert!(diff.before.contains("line 250\n"));
        assert_eq!(diff.pairs.iter().filter(|pair| pair.changed).count(), 2);
    }

    #[test]
    fn inserted_and_deleted_alignment_rows_keep_context_on_both_sides() {
        let before = "zero\none\ntwo\nthree\nfour\nfive\nsix\nseven\neight\nnine\n";
        let inserted = before.replace("five\n", "inserted\nfive\n");
        let diff = review_diff(before, &inserted);
        assert_eq!(diff.before, "two\nthree\nfour\nfive\nsix\nseven\n");
        assert_eq!(
            diff.current,
            "two\nthree\nfour\ninserted\nfive\nsix\nseven\n"
        );
        assert_eq!(diff.pairs.iter().filter(|pair| pair.changed).count(), 1);
        let removed = before.replace("five\n", "");
        let diff = review_diff(before, &removed);
        assert_eq!(diff.before, "two\nthree\nfour\nfive\nsix\nseven\neight\n");
        assert_eq!(diff.current, "two\nthree\nfour\nsix\nseven\neight\n");
        assert_eq!(diff.pairs.iter().filter(|pair| pair.changed).count(), 1);
    }

    #[test]
    fn review_context_handles_empty_and_unchanged_documents_and_eof_without_newline() {
        for (before, after) in [
            ("", ""),
            ("", "inserted\n"),
            ("removed\r\n", ""),
            ("same\r\n", "same\r\n"),
            ("head\n\n\ntail", "head\nnew\n\ntail"),
        ] {
            let diff = review_diff(before, after);
            assert_eq!(diff.before, before);
            assert_eq!(diff.current, after);
        }
    }

    #[test]
    fn cancellation_wins_over_other_actions_in_any_input_batch_order() {
        for events in [
            vec![Action::Cancel, Action::Accept],
            vec![Action::Accept, Action::Cancel],
            vec![Action::Cancel, Action::Preview, Action::Accept],
            vec![Action::Preview, Action::Cancel, Action::Accept],
            vec![Action::Cancel, Action::Cancel],
        ] {
            let mut chosen = None;
            for event in events {
                chosen = Some(merge_action(chosen, event));
            }
            assert!(matches!(chosen, Some(Action::Cancel)));
        }
        assert!(matches!(merge_action(None, Action::Accept), Action::Accept));
        assert!(matches!(
            merge_action(None, Action::Preview),
            Action::Preview
        ));
    }

    #[test]
    fn a_new_review_resets_scroll_once_and_repeated_paints_keep_user_scroll() {
        let root = tempfile::tempdir().unwrap();
        let context = egui::Context::default();
        let (mut app, target) = fixture(&context, root.path());
        let key = app.document().key();
        let id = egui::Id::new("presentation-test");
        let mut scroll_ids = [egui::Id::NULL; 2];
        let mut paint = |app: &mut EditorApp| {
            context
                .run_ui(egui::RawInput::default(), |ui| {
                    scroll_ids = ["version-comparison-scroll", "unified-comparison-scroll"]
                        .map(|salt| ui.make_persistent_id(egui::IdSalt::new(id.with(salt))));
                    let State::Reviewing(review) = &mut app.tex_refactor else {
                        panic!()
                    };
                    review.present(ui, id);
                })
                .drop_without_applying_deltas();
            scroll_ids
        };
        app.begin_tex_refactor(target.clone(), Operation::Inline, &context);
        let ids = paint(&mut app);
        for scroll in ids {
            assert_eq!(
                egui::scroll_area::State::load(&context, scroll)
                    .unwrap()
                    .offset,
                Vec2::ZERO
            );
            let mut state = egui::scroll_area::State::default();
            state.offset = Vec2::new(300.0, 1700.0);
            state.store(&context, scroll);
        }
        assert_eq!(paint(&mut app), ids);
        for scroll in ids {
            assert_eq!(
                egui::scroll_area::State::load(&context, scroll)
                    .unwrap()
                    .offset,
                Vec2::new(300.0, 1700.0)
            );
        }
        app.close_tex_refactor(&context);
        app.begin_tex_refactor(target, Operation::InlineAll, &context);
        assert_eq!(paint(&mut app), ids);
        for scroll in ids {
            assert_eq!(
                egui::scroll_area::State::load(&context, scroll)
                    .unwrap()
                    .offset,
                Vec2::ZERO
            );
        }
        assert_eq!(app.document().key(), key);
        assert_eq!(app.document().source(), SOURCE);
    }

    #[test]
    fn production_card_keeps_review_footer_visible_after_initial_and_naming_layouts() {
        use egui_kittest::{Harness, kittest::Queryable as _};
        for size in [
            Vec2::new(1512.0, 829.0),
            Vec2::new(800.0, 500.0),
            Vec2::new(640.0, 360.0),
        ] {
            for style in crate::settings::GitDiffStyle::ALL {
                for grows_from_naming in [false, true] {
                    let root = tempfile::tempdir().unwrap();
                    let context = egui::Context::default();
                    let (mut app, mut target) = fixture(&context, root.path());
                    app.settings.git_diff_style = style;
                    if grows_from_naming {
                        let source = SOURCE
                            .replace("\\end{document}", "$a^{x_i+\\alpha}+z$\r\n\\end{document}");
                        let path = root.path().join("main.tex");
                        fs::write(&path, &source).unwrap();
                        app.document_mut().replace_loaded_unprojected(
                            source.clone(),
                            path,
                            DocumentKind::Tex,
                            Some(fingerprint(source.as_bytes())),
                        );
                        let byte = source.find("x_i+\\alpha").unwrap();
                        let start = source[..byte].chars().count();
                        target = Target::capture(
                            &app,
                            preview_jump::SourceLocation {
                                key: app.document().key(),
                                char_index: start,
                            },
                            None,
                        )
                        .unwrap();
                        target.selection_characters = start..start + "x_i+\\alpha".chars().count();
                        app.begin_tex_refactor(target, Operation::Extract, &context);
                    } else {
                        app.begin_tex_refactor(target, Operation::Inline, &context);
                    }
                    let source = app.document().source().to_owned();
                    let mut harness = Harness::builder().with_size(size).build_ui_state(
                        |ui, state: &mut (EditorApp, bool)| {
                            let (app, fonts_ready) = state;
                            if !*fonts_ready {
                                theme::configure_editor_fonts(
                                    ui.ctx(),
                                    Default::default(),
                                    Default::default(),
                                    false,
                                    400,
                                    400,
                                    None,
                                );
                                *fonts_ready = true;
                                return;
                            }
                            if let Some(action) = review_card(
                                ui.ctx(),
                                &mut app.tex_refactor,
                                &app.settings,
                                ui.max_rect(),
                            ) {
                                match action {
                                    Action::Preview => app.preview_tex_extraction(),
                                    Action::Accept => {
                                        app.accept_tex_refactor().unwrap();
                                    }
                                    Action::Cancel => app.close_tex_refactor(ui.ctx()),
                                }
                            }
                        },
                        (app, false),
                    );
                    harness.run_steps(3);
                    if grows_from_naming {
                        let State::Naming(draft) = &mut harness.state_mut().0.tex_refactor else {
                            panic!()
                        };
                        draft.name = "energy".into();
                        harness.get_by_label("Preview").click();
                        harness.run_steps(3);
                    }
                    assert_eq!(harness.state().0.tex_refactor.phase(), "reviewing");
                    let bounds = Rect::from_min_size(Pos2::ZERO, size);
                    let hit = |name| {
                        harness
                            .ctx
                            .data(|data| {
                                data.get_temp::<Rect>(egui::Id::new(("tex-refactor-hit", name)))
                            })
                            .unwrap()
                    };
                    let diff = hit("diff");
                    for name in ["diff", "accept", "cancel"] {
                        let rect = hit(name);
                        assert!(
                            rect.is_finite() && rect.is_positive(),
                            "{size:?} {style:?} naming={grows_from_naming}: {name} clipped to {rect:?}"
                        );
                        assert!(
                            bounds.contains_rect(rect),
                            "{name} must stay in the actual child viewport"
                        );
                    }
                    assert!(hit("accept").top() >= diff.bottom());
                    assert!(hit("cancel").top() >= diff.bottom());
                    harness.get_by_label("Cancel").click();
                    harness.run_steps(2);
                    assert_eq!(harness.state().0.tex_refactor.phase(), "closed");
                    assert_eq!(harness.state().0.document().source(), &source);
                }
            }
        }
    }
}
