//! A guarded conflict draft lives in the source pane; computing its comparison
//! never blocks typing or the sibling preview.
use super::*;
use crate::git::editor::{document_diff::DocumentDiff, full_view};
use crate::overleaf::ConflictReview;

const STALE: &str = "Conflict changed. Reopen the review.";

impl OperationSummary for DocumentDiff {
    fn completion_summary(&self) -> String {
        "Conflict comparison ready".into()
    }
}

pub(in crate::app) struct Dialog {
    review: ConflictReview,
    path: PathBuf,
    result: String,
    editing: bool,
    focus_editor: bool,
    revision: u64,
    comparison: Comparison,
    choice_error: Option<String>,
    validity: ReviewValidity,
}
struct CachedComparison {
    revision: u64,
    diff: DocumentDiff,
}
enum Comparison {
    Unrequested,
    Computing {
        revision: u64,
        job: ExclusiveJob<DocumentDiff>,
        previous: Option<CachedComparison>,
    },
    Ready(CachedComparison),
    Failed {
        revision: u64,
        message: String,
        previous: Option<CachedComparison>,
    },
}
impl Comparison {
    fn cached(&self) -> Option<&CachedComparison> {
        match self {
            Self::Ready(cached) => Some(cached),
            Self::Computing { previous, .. } | Self::Failed { previous, .. } => previous.as_ref(),
            Self::Unrequested => None,
        }
    }
    fn into_cached(self) -> Option<CachedComparison> {
        match self {
            Self::Ready(cached) => Some(cached),
            Self::Computing { previous, .. } | Self::Failed { previous, .. } => previous,
            Self::Unrequested => None,
        }
    }
    fn running(&self) -> bool {
        matches!(self, Self::Computing { .. })
    }
    #[cfg(all(feature = "desktop-ui-tests", target_os = "macos"))]
    fn phase(&self) -> &'static str {
        match self {
            Self::Unrequested => "unrequested",
            Self::Computing { .. } => "computing",
            Self::Ready(_) => "ready",
            Self::Failed { .. } => "failed",
        }
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ReviewValidity {
    Current,
    Stale,
}
impl Dialog {
    fn new(review: ConflictReview, path: PathBuf) -> Self {
        Self {
            result: review.local.clone(),
            review,
            path,
            editing: false,
            focus_editor: false,
            revision: 0,
            comparison: Comparison::Unrequested,
            choice_error: None,
            validity: ReviewValidity::Current,
        }
    }

    fn changed(&mut self) {
        self.revision = self
            .revision
            .checked_add(1)
            .expect("merge revision exhausted");
        self.choice_error = None;
    }

    fn error(&self) -> Option<&str> {
        self.choice_error
            .as_deref()
            .or(match &self.comparison {
                Comparison::Failed {
                    revision, message, ..
                } if *revision == self.revision => Some(message.as_str()),
                _ => None,
            })
            .or((self.validity == ReviewValidity::Stale).then_some(STALE))
    }
    fn poll(&mut self, context: &egui::Context) {
        let comparison = std::mem::replace(&mut self.comparison, Comparison::Unrequested);
        self.comparison = match comparison {
            Comparison::Computing {
                revision,
                mut job,
                previous,
            } => match job.poll() {
                LatestJobPoll::Ready(diff) if revision == self.revision => {
                    Comparison::Ready(CachedComparison { revision, diff })
                }
                LatestJobPoll::Failed(message) if revision == self.revision => Comparison::Failed {
                    revision,
                    message,
                    previous,
                },
                LatestJobPoll::Ready(_) | LatestJobPoll::Failed(_) => {
                    previous.map_or(Comparison::Unrequested, Comparison::Ready)
                }
                _ => Comparison::Computing {
                    revision,
                    job,
                    previous,
                },
            },
            state => state,
        };
        if self.editing
            || self.comparison.running()
            || self
                .comparison
                .cached()
                .is_some_and(|cached| cached.revision == self.revision)
            || matches!(self.comparison, Comparison::Failed { revision, .. } if revision == self.revision)
        {
            return;
        }
        let previous =
            std::mem::replace(&mut self.comparison, Comparison::Unrequested).into_cached();
        let remote = self.review.remote.clone();
        let result = self.result.clone();
        let mut job = ExclusiveJob::default();
        self.comparison =
            match job.start_and_repaint("Compare Overleaf conflict", context, move || {
                Ok(DocumentDiff::new(remote, &result))
            }) {
                Ok(()) => Comparison::Computing {
                    revision: self.revision,
                    job,
                    previous,
                },
                Err(message) => Comparison::Failed {
                    revision: self.revision,
                    message,
                    previous,
                },
            };
    }

    #[cfg(all(feature = "desktop-ui-tests", target_os = "macos"))]
    pub(in crate::app) fn inspection(&self) -> serde_json::Value {
        serde_json::json!({
            "open": true,
            "loading": self.comparison.running(),
            "phase": self.comparison.phase(),
            "left": crate::desktop_test::fingerprint(&self.review.remote),
            "right": crate::desktop_test::fingerprint(&self.result),
            "local": crate::desktop_test::fingerprint(&self.review.local),
            "editing": self.editing,
            "current": self.validity == ReviewValidity::Current,
            "error": self.error(),
        })
    }
}

impl EditorApp {
    pub(in crate::app) fn open_overleaf_review(&mut self, context: &egui::Context, id: &str) {
        let Some((review, path)) = self.overleaf.connection.as_ref().and_then(|connection| {
            let review = connection.model.conflict_review(id)?;
            let path = connection
                .root
                .join(&connection.model.tree()?.entry(id)?.path);
            Some((review, path))
        }) else {
            self.notice = Some(Notice {
                message: "This conflict is no longer available.".into(),
                kind: NoticeKind::Info,
            });
            return;
        };
        let target = self
            .overleaf
            .tab_entities
            .iter()
            .find_map(|(&tab, entity)| {
                (entity == id && self.tabs.index_of(tab).is_some()).then_some(tab)
            })
            .or_else(|| {
                self.tabs.ids().find(|&tab| {
                    self.document_for_tab(tab)
                        .is_some_and(|document| document.path().as_ref() == Some(&path))
                })
            });
        let target = target.or_else(|| {
            if self.open_cached_overleaf_path(&path, context) {
                self.tabs
                    .active_id()
                    .filter(|&tab| self.overleaf_entity_for_tab(tab) == Some(id))
            } else {
                None
            }
        });
        let Some(target) = target else {
            self.notice = Some(Notice {
                message: "Open this file before reviewing its conflict.".into(),
                kind: NoticeKind::Info,
            });
            return;
        };
        if self.table_editor.is_some() {
            self.cancel_table_editor(context);
        }
        self.close_version_history(context);
        self.close_document_diff();
        if self.liquid.visible() {
            self.close_liquid(context);
        }
        self.activate_tab(target, context);
        self.workspace_chooser_visible = false;
        self.overleaf.visible = false;
        if let Some(browser) = self.overleaf.browser.as_ref() {
            let _ = browser.set_visible(false);
        }
        ChildViewHost::hide(context, SALT);
        if self.view_mode == ViewMode::Preview {
            self.view_mode = ViewMode::Split;
        }
        self.editor_completion = None;
        self.clear_editor_hover();
        self.clear_asset_hover();
        self.close_app_popup();
        self.find_bar.close();
        ChildViewHost::close(context, find_bar::VIEWPORT_SALT);
        let editor = source_editor_id(context);
        context.memory_mut(|memory| memory.surrender_focus(editor));
        let mut dialog = Dialog::new(review, path);
        dialog.poll(context);
        self.overleaf.review = Some(dialog);
        crate::window_host::return_focus(context);
        context.request_repaint();
    }

    pub(in crate::app) fn close_overleaf_review(&mut self, context: &egui::Context) {
        if self.overleaf.review.take().is_some() {
            self.finish_overleaf_review(context);
        }
    }

    fn finish_overleaf_review(&mut self, context: &egui::Context) {
        let cursor = self.editor_snapshot(context).cursor.as_sorted_char_range();
        self.pending_editor_selection = Some(EditorSelection::Focus(cursor.start.0..cursor.end.0));
        self.explorer.clear_keyboard_focus();
        // The source field is absent from this review frame. Release the
        // retiring control now; show_editor takes focus after painting it.
        if let Some(focused) = context.memory(|memory| memory.focused()) {
            context.memory_mut(|memory| memory.surrender_focus(focused));
        }
        context.request_repaint();
    }

    pub(in crate::app) fn show_overleaf_review_panel(&mut self, ui: &mut egui::Ui) {
        let Some(mut dialog) = self.overleaf.review.take() else {
            return;
        };
        dialog.validity = if self
            .overleaf
            .connection
            .as_ref()
            .is_some_and(|connection| connection.model.conflict_review_current(&dialog.review))
        {
            ReviewValidity::Current
        } else {
            ReviewValidity::Stale
        };
        dialog.poll(ui.ctx());
        ui.set_min_width(ui.available_width());
        let bounds = ui.available_rect_before_wrap();
        let revision = dialog.revision;
        let editing = dialog.editing;
        let action = ui
            .scope_builder(egui::UiBuilder::new().max_rect(bounds), |ui| {
                ui.set_clip_rect(ui.clip_rect().intersect(bounds));
                show(ui, &mut dialog, &self.settings)
            })
            .inner;
        ui.allocate_rect(bounds, Sense::hover());
        observe(
            "panel",
            &ui.interact(
                bounds,
                ui.id().with("overleaf-review-panel"),
                Sense::hover(),
            ),
        );
        let choice = match action {
            Some(Action::Cancel) => {
                self.overleaf.review = Some(dialog);
                self.close_overleaf_review(ui.ctx());
                return;
            }
            Some(Action::Remote) => Some(dialog.review.remote.clone()),
            Some(Action::Local) => Some(dialog.review.local.clone()),
            Some(Action::Apply) => Some(dialog.result.clone()),
            None => None,
        };
        if let Some(source) = choice {
            match self.apply_overleaf_review(ui.ctx(), &dialog.review, source) {
                Ok(()) => {
                    self.finish_overleaf_review(ui.ctx());
                    return;
                }
                Err(error) => dialog.choice_error = Some(error),
            }
        }
        if dialog.revision != revision || dialog.editing != editing {
            dialog.poll(ui.ctx());
        }
        self.overleaf.review = Some(dialog);
    }

    pub(in crate::app) fn handle_overleaf_review_shortcuts(
        &mut self,
        context: &egui::Context,
        viewport: egui::ViewportId,
    ) {
        if self.overleaf.review.is_none() {
            return;
        }
        let shortcuts = self.settings.effective_shortcuts();
        handle_window_management_shortcuts(context, viewport, &shortcuts);
        if let Some(shortcut) = shortcuts.egui(ShortcutAction::CaptureUi)
            && context.input_mut_for(viewport, |input| input.consume_shortcut(&shortcut))
        {
            self.captures.queue_for_viewport(viewport);
        }
        if let Some(action) = context.input_mut_for(viewport, |input| {
            consume_preview_zoom_shortcut(input, &shortcuts)
        }) {
            self.route_preview_zoom(context, action);
        }
        if viewport != context.viewport_id() {
            return;
        }
        let close = context.input_mut(|input| {
            input.consume_key(Modifiers::NONE, egui::Key::Escape)
                || [ShortcutAction::CloseTab, ShortcutAction::CloseWindow]
                    .into_iter()
                    .any(|action| {
                        shortcuts
                            .binding(action)
                            .is_some_and(|binding| input.consume_shortcut(&binding.egui()))
                    })
        });
        if close {
            self.close_overleaf_review(context);
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Action {
    Cancel,
    Remote,
    Local,
    Apply,
}

fn show(ui: &mut egui::Ui, dialog: &mut Dialog, settings: &AppSettings) -> Option<Action> {
    let mut action = None;
    ui.horizontal(|ui| {
        ui.strong("Overleaf conflict");
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            if button(ui, "cancel", "×", true).clicked() {
                action = Some(Action::Cancel);
            }
            ui.add(
                egui::Label::new(
                    dialog
                        .path
                        .file_name()
                        .unwrap_or_default()
                        .to_string_lossy(),
                )
                .truncate(),
            );
        });
    });
    ui.horizontal_wrapped(|ui| {
        if button(
            ui,
            "use_remote",
            "Use Overleaf",
            dialog.validity == ReviewValidity::Current,
        )
        .clicked()
        {
            action = Some(Action::Remote);
        }
        if button(
            ui,
            "use_local",
            "Keep local",
            dialog.validity == ReviewValidity::Current,
        )
        .clicked()
        {
            action = Some(Action::Local);
        }
        let edit = ui.selectable_label(
            dialog.editing,
            if dialog.editing {
                "Compare"
            } else {
                "Edit merge"
            },
        );
        observe("edit", &edit);
        if edit.clicked() {
            dialog.editing = !dialog.editing;
            dialog.focus_editor = dialog.editing;
        }
        if dialog.editing
            && button(
                ui,
                "apply",
                "Apply merge",
                dialog.validity == ReviewValidity::Current,
            )
            .clicked()
        {
            action = Some(Action::Apply);
        }
        if dialog.comparison.running() {
            ui.spinner();
        }
    });
    if let Some(error) = dialog.error() {
        ui.add(
            egui::Label::new(RichText::new(error).color(ui.visuals().error_fg_color)).truncate(),
        );
    }
    if dialog.editing {
        egui::ScrollArea::both()
            .id_salt("overleaf-merge-editor-scroll")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                let response = code_editor::editor(
                    ui,
                    ("overleaf-merge", &dialog.review.id),
                    &mut dialog.result,
                    dialog
                        .path
                        .extension()
                        .and_then(|extension| extension.to_str())
                        .unwrap_or("tex"),
                    settings,
                    4,
                );
                observe("editor", &response);
                if std::mem::take(&mut dialog.focus_editor) {
                    response.request_focus();
                }
                if response.changed() {
                    dialog.changed();
                }
            });
    } else {
        if settings.git_diff_style == crate::settings::GitDiffStyle::SideBySide {
            ui.columns(2, |columns| {
                columns[0].weak("Overleaf");
                columns[1].weak(if dialog.revision == 0 {
                    "Local draft"
                } else {
                    "Merged draft"
                });
            });
        } else {
            ui.weak(if dialog.revision == 0 {
                "Overleaf → Local draft"
            } else {
                "Overleaf → Merged draft"
            });
        }
        ui.separator();
        if let Some(CachedComparison { diff, .. }) = dialog.comparison.cached() {
            let id = ui.make_persistent_id(("overleaf-conflict-diff", &dialog.review.id));
            let bounds = full_view::show_styled_comparison(ui, diff, id, &dialog.path, settings);
            observe(
                "diff",
                &ui.interact(bounds, id.with("observed"), Sense::hover()),
            );
        } else {
            ui.weak("Preparing comparison…");
        }
    }
    action
}

fn button(ui: &mut egui::Ui, name: &str, label: &str, enabled: bool) -> egui::Response {
    let response = ui.add_enabled(enabled, egui::Button::new(label));
    observe(name, &response);
    response
}
fn observe(_name: &str, _response: &egui::Response) {
    #[cfg(all(feature = "desktop-ui-tests", target_os = "macos"))]
    crate::desktop_test::observe(&format!("overleaf.review.{_name}"), _response);
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui_kittest::{
        Harness,
        kittest::{NodeT as _, Queryable as _, by},
    };

    fn fixture() -> (Dialog, Model) {
        let mut model = Model::new(7);
        model
            .apply(BrowserEvent::Project {
                epoch: 7,
                project_id: "0123456789abcdef01234567".into(),
                name: "Paper".into(),
                entries: vec![Entry {
                    id: "a".into(),
                    path: "main.tex".into(),
                    kind: EntryKind::Doc,
                }],
                root_doc: Some("a".into()),
            })
            .unwrap();
        let snapshot = |source: &str| BrowserEvent::Document {
            epoch: 7,
            id: "a".into(),
            path: "main.tex".into(),
            source: source.into(),
        };
        model.apply(snapshot("old\n")).unwrap();
        model.set_draft("a", "local\n".into()).unwrap();
        model.apply(snapshot("remote\n")).unwrap();
        let review = model.conflict_review("a").unwrap();
        let mut dialog = Dialog::new(review, PathBuf::from("main.tex"));
        dialog.comparison = Comparison::Ready(CachedComparison {
            revision: 0,
            diff: DocumentDiff::new(dialog.review.remote.clone(), &dialog.result),
        });
        (dialog, model)
    }

    #[test]
    fn failed_comparison_latches_its_revision_without_losing_the_previous_review() {
        let (mut dialog, _) = fixture();
        let previous =
            std::mem::replace(&mut dialog.comparison, Comparison::Unrequested).into_cached();
        dialog.comparison = Comparison::Failed {
            revision: dialog.revision,
            message: "comparison failed".into(),
            previous,
        };
        let context = egui::Context::default();
        for _ in 0..100 {
            dialog.poll(&context);
        }
        assert!(!dialog.comparison.running());
        assert_eq!(dialog.error(), Some("comparison failed"));
        assert_eq!(dialog.comparison.cached().unwrap().diff.current, "local\n");
        dialog.editing = true;
        dialog.changed();
        dialog.poll(&context);
        assert!(dialog.error().is_none());
        assert!(!dialog.comparison.running());
    }

    #[test]
    fn comparison_uses_whole_remote_and_local_sources_and_stays_cached_while_idle() {
        let (mut dialog, model) = fixture();
        let diff = &dialog.comparison.cached().unwrap().diff;
        assert_eq!(diff.before, "remote\n");
        assert_eq!(diff.current, "local\n");
        assert!(model.conflict_review_current(&dialog.review));
        let context = egui::Context::default();
        for _ in 0..50 {
            dialog.poll(&context);
        }
        assert!(!dialog.comparison.running());
        assert_eq!(dialog.comparison.cached().unwrap().diff.builds, 1);
        assert!(!dialog.comparison.running());
    }

    #[test]
    fn typing_in_manual_merge_defers_comparison_work_until_returning_to_compare() {
        let (mut dialog, _) = fixture();
        let context = egui::Context::default();
        dialog.editing = true;
        dialog.result.push_str("merged");
        dialog.changed();
        for _ in 0..50 {
            dialog.poll(&context);
        }
        assert!(!dialog.comparison.running());
        assert!(!dialog.comparison.running());
        assert_eq!(dialog.comparison.cached().unwrap().diff.current, "local\n");
        assert_ne!(
            dialog.comparison.cached().map(|cached| cached.revision),
            Some(dialog.revision)
        );
    }

    #[test]
    fn docked_comparison_and_manual_merge_fit_the_source_pane_in_both_git_styles() {
        for style in [
            crate::settings::GitDiffStyle::SideBySide,
            crate::settings::GitDiffStyle::Unified,
        ] {
            let (dialog, _) = fixture();
            let settings = AppSettings {
                git_diff_style: style,
                ..Default::default()
            };
            let mut fonts_ready = false;
            let mut harness = Harness::builder()
                .with_size(Vec2::new(480.0, 360.0))
                .build_ui_state(
                    move |ui, state: &mut (Dialog, Option<Action>)| {
                        if !fonts_ready {
                            theme::configure_test_fonts(ui.ctx());
                            fonts_ready = true;
                            return;
                        }
                        if let Some(action) = show(ui, &mut state.0, &settings) {
                            state.1 = Some(action);
                        }
                    },
                    (dialog, None),
                );
            harness.run();
            for label in ["Use Overleaf", "Keep local", "Edit merge", "×"] {
                let rect = harness.get_by_label(label).rect();
                assert!(
                    rect.min.x >= 0.0
                        && rect.max.x <= 480.0
                        && rect.min.y >= 0.0
                        && rect.max.y <= 360.0,
                    "{style:?}: {label}: {rect:?}"
                );
            }
            harness.get_by_label("Edit merge").click();
            harness.run();
            harness
                .get(by().role(egui::accesskit::Role::MultilineTextInput))
                .click();
            harness.run();
            harness.event(egui::Event::Text(" merged".into()));
            harness.run();
            assert!(harness.state().0.result.contains("merged"));
            assert!(harness.state().0.revision > 0);
            assert_eq!(
                harness.state().0.review.local,
                "local\n",
                "editing changes only the merge draft"
            );
            assert!(harness.state().1.is_none());
            harness.get_by_label("Apply merge").click();
            harness.run();
            assert_eq!(harness.state().1, Some(Action::Apply));
        }
    }

    #[test]
    fn stale_review_keeps_both_versions_visible_but_disables_every_resolution() {
        let (mut dialog, mut model) = fixture();
        model.set_draft("a", "later local\n".into()).unwrap();
        dialog.validity = if model.conflict_review_current(&dialog.review) {
            ReviewValidity::Current
        } else {
            ReviewValidity::Stale
        };
        assert_eq!(dialog.validity, ReviewValidity::Stale);
        let mut fonts_ready = false;
        let mut harness = Harness::builder()
            .with_size(Vec2::new(560.0, 360.0))
            .build_ui_state(
                move |ui, dialog| {
                    if !fonts_ready {
                        theme::configure_test_fonts(ui.ctx());
                        fonts_ready = true;
                        return;
                    }
                    show(ui, dialog, &AppSettings::default());
                },
                dialog,
            );
        harness.run();
        for label in ["Use Overleaf", "Keep local"] {
            assert!(harness.get_by_label(label).accesskit_node().is_disabled());
        }
        assert!(harness.query_by_label(STALE).is_some());
        harness.get_by_label("Edit merge").click();
        harness.run();
        assert!(
            harness
                .get_by_label("Apply merge")
                .accesskit_node()
                .is_disabled()
        );
        assert_eq!(harness.state().review.remote, "remote\n");
        assert_eq!(harness.state().review.local, "local\n");
    }

    #[test]
    fn cancelling_the_docked_review_preserves_the_conflict_and_unsaved_source() {
        let directory = tempfile::tempdir().unwrap();
        let context = egui::Context::default();
        let mut app = EditorApp::dormant_for_tests(&context, directory.path().into());
        let (mut dialog, model) = fixture();
        let path = directory.path().join("main.tex");
        dialog.path = path.clone();
        app.document_mut().replace_loaded_unprojected(
            "old\n".into(),
            path,
            DocumentKind::Tex,
            None,
        );
        app.document_mut()
            .edit(CCursorRange::default(), |source| *source = "local\n".into());
        app.overleaf.connection = Some(Connection {
            project: ProjectUrl::parse("https://www.overleaf.com/project/0123456789abcdef01234567")
                .unwrap(),
            root: directory.path().into(),
            model,
            status: "Conflict".into(),
            transport: Transport::Online,
            storage: None,
            hydration: Hydration::default(),
        });
        let original = app
            .overleaf
            .connection
            .as_ref()
            .unwrap()
            .model
            .conflict_review("a")
            .unwrap();
        let source = app.document().source().clone();
        let key = app.document().key();
        let history = app.document().history_availability();
        let dirty = app.is_dirty();
        app.overleaf.review = Some(dialog);
        let mut fonts_ready = false;
        let mut cursor_ready = false;
        let mut harness = Harness::builder()
            .with_size(Vec2::new(560.0, 360.0))
            .build_ui_state(
                move |ui, app: &mut EditorApp| {
                    if !fonts_ready {
                        theme::configure_test_fonts(ui.ctx());
                        fonts_ready = true;
                        return;
                    }
                    if !cursor_ready {
                        app.store_editor_cursor(
                            ui.ctx(),
                            CCursorRange::two(CCursor::new(1), CCursor::new(3)),
                        );
                        cursor_ready = true;
                    }
                    let was_reviewing = app.overleaf.review.is_some();
                    app.show_editor(ui);
                    if was_reviewing && app.overleaf.review.is_none() {
                        assert_eq!(
                            app.pending_editor_selection,
                            Some(EditorSelection::Focus(1..3))
                        );
                        let editor = source_editor_id(ui.ctx());
                        assert!(!ui.memory(|memory| memory.has_focus(editor)));
                    }
                },
                app,
            );
        harness.run();
        harness.get_by_label("×").click();
        harness.run();
        let app = harness.state();
        assert!(app.overleaf.review.is_none());
        assert!(app.pending_editor_selection.is_none());
        let editor = source_editor_id(&harness.ctx);
        assert!(harness.ctx.memory(|memory| memory.has_focus(editor)));
        let cursor = egui::text_edit::TextEditState::load(&harness.ctx, editor)
            .unwrap()
            .cursor
            .char_range()
            .unwrap()
            .as_sorted_char_range();
        assert_eq!(cursor.start.0..cursor.end.0, 1..3);
        assert_eq!(app.document().source(), &source);
        assert_eq!(app.document().key(), key);
        assert_eq!(app.document().history_availability(), history);
        assert_eq!(app.is_dirty(), dirty);
        assert_eq!(
            app.overleaf
                .connection
                .as_ref()
                .unwrap()
                .model
                .conflict_review("a"),
            Some(original)
        );
    }
}
