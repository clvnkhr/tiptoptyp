//! One source-owned TeX scan; only its unchanged owner may apply the result.
use super::*;
use crate::tex::autofix::{Plan, Settings};
use std::sync::atomic::{AtomicBool, Ordering};

#[cfg(test)]
mod save_tests;

#[derive(Default)]
pub(super) enum State {
    #[default]
    Idle,
    Running(Box<Task>),
    Cancelling(Box<Task>),
    Failed(String),
}

pub(super) struct Task {
    tab: u64,
    key: DocumentKey,
    source: Arc<str>,
    settings: Settings,
    markers: crate::comment_controls::Settings,
    tool: Option<crate::toolchain::ToolResolution>,
    purpose: Purpose,
    cancel: Arc<AtomicBool>,
    job: LatestJob<Plan>,
}

enum Purpose {
    Edit,
    Save {
        path: PathBuf,
        intent: SaveIntent,
        format_after: bool,
        continuation: Option<tiptoptyp_core::workflow::SaveContinuationToken>,
    },
}
impl Drop for Task {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Release);
    }
}
impl State {
    pub(super) fn activity(&self) -> crate::activity::Activity {
        use crate::activity::Activity;
        match self {
            Self::Idle => Activity::Idle,
            Self::Running(_) | Self::Cancelling(_) => Activity::Running,
            Self::Failed(error) => Activity::Failed(error.clone()),
        }
    }
    pub(super) fn busy(&self) -> bool {
        matches!(self, Self::Running(_) | Self::Cancelling(_))
    }
    fn holds_continuation(
        &self,
        token: Option<tiptoptyp_core::workflow::SaveContinuationToken>,
    ) -> bool {
        match self {
            Self::Running(task) | Self::Cancelling(task) => matches!(task.purpose,
                Purpose::Save { continuation: Some(owned), .. } if token == Some(owned)),
            Self::Idle | Self::Failed(_) => false,
        }
    }
    #[cfg(test)]
    pub(super) fn wait_until_ready_for_test(&mut self) -> Result<(), String> {
        match self {
            Self::Running(task) | Self::Cancelling(task) => task.job.wait_until_ready_for_test(),
            Self::Idle | Self::Failed(_) => Err("No autofix worker is running".into()),
        }
    }
    pub(super) fn cancel(&mut self) {
        if matches!(self, Self::Running(_)) {
            let Self::Running(task) = std::mem::take(self) else {
                unreachable!()
            };
            task.cancel.store(true, Ordering::Release);
            *self = Self::Cancelling(task);
        }
    }
    pub(super) fn cancel_tab(&mut self, tab: Option<u64>) {
        if matches!(self, Self::Running(task) if Some(task.tab) == tab) {
            self.cancel();
        }
    }
    #[cfg(all(feature = "desktop-ui-tests", target_os = "macos"))]
    pub(super) fn inspect(&self) -> serde_json::Value {
        serde_json::json!({"phase": match self {
            Self::Idle => "idle", Self::Running(_) => "running",
            Self::Cancelling(_) => "cancelling", Self::Failed(_) => "failed",
        }})
    }
}

impl Task {
    fn admits(
        &self,
        key: Option<DocumentKey>,
        settings: &Settings,
        markers: &crate::comment_controls::Settings,
        tool: Option<&crate::toolchain::ToolResolution>,
    ) -> bool {
        key == Some(self.key)
            && settings == &self.settings
            && markers == &self.markers
            && self.tool.as_ref() == tool
            && !self.cancel.load(Ordering::Acquire)
    }
}

/// Map both scalar endpoints in a single ordered pass through the original
/// source. Work is linear in source plus edit count, including Unicode text.
fn map_cursor(
    source: &str,
    edits: &[crate::tex::autofix::Edit],
    cursor: CCursorRange,
) -> CCursorRange {
    let map = |index: usize| {
        let mut old_byte = 0;
        let mut old_scalar = 0;
        let mut delta = 0_isize;
        for edit in edits {
            let start = old_scalar + source[old_byte..edit.range.start].chars().count();
            let end = start + source[edit.range.clone()].chars().count();
            if index <= start {
                return (index as isize + delta) as usize;
            }
            let added = edit.replacement.chars().count();
            if index < end {
                return (start as isize + delta) as usize + added;
            }
            delta += added as isize - (end - start) as isize;
            old_byte = edit.range.end;
            old_scalar = end;
        }
        (index as isize + delta) as usize
    };
    CCursorRange {
        primary: CCursor {
            index: CCursor::new(map(cursor.primary.index.0)).index,
            ..cursor.primary
        },
        secondary: CCursor {
            index: CCursor::new(map(cursor.secondary.index.0)).index,
            ..cursor.secondary
        },
        h_pos: cursor.h_pos,
    }
}

impl EditorApp {
    /// Save preparation owns the same close action until it hands off to the
    /// durable writer or explicitly retires its own continuation.
    pub(super) fn finish_document_dispatch(&mut self) {
        if !self
            .autofix
            .holds_continuation(self.document_workflow.continuation_token())
        {
            self.document_workflow.finish_dispatch();
        }
    }
    pub(super) fn begin_autofix(&mut self, context: &egui::Context) {
        self.begin_autofix_selected(None, context);
    }

    pub(super) fn begin_autofix_rule(
        &mut self,
        rule: crate::tex::autofix::Rule,
        context: &egui::Context,
    ) {
        if rule.enabled(&self.settings.autofix) {
            self.begin_autofix_selected(Some(rule), context);
        }
    }

    fn begin_autofix_selected(
        &mut self,
        selected: Option<crate::tex::autofix::Rule>,
        context: &egui::Context,
    ) {
        if self.reject_read_only_edit(context)
            || self.document().kind() != DocumentKind::Tex
            || self.document_flow_busy()
            || self.autofix.busy()
        {
            return;
        }
        if !crate::tex::autofix::Rule::ALL
            .iter()
            .any(|rule| rule.enabled(&self.settings.autofix))
        {
            self.notice = Some(Notice {
                message: "Enable TeX rules in Settings → Autofixing.".into(),
                kind: NoticeKind::Info,
            });
            return;
        }
        let Some(tab) = self.tabs.active_id() else {
            return;
        };
        self.start_autofix(tab, Purpose::Edit, selected, context);
    }

    pub(super) fn prepare_autofix_save(
        &mut self,
        tab: u64,
        path: PathBuf,
        intent: SaveIntent,
        format_after: bool,
        context: &egui::Context,
    ) -> bool {
        if self.autofix.busy() {
            if intent == SaveIntent::Auto {
                self.set_tab_autosave(tab, Some(Instant::now() + AUTOSAVE_RETRY_DELAY));
            } else {
                self.notice = Some(Notice {
                    message: "TeX autofixing is still running; save again when it finishes.".into(),
                    kind: NoticeKind::Info,
                });
            }
            return false;
        }
        let continuation = self.document_workflow.continuation_token();
        self.start_autofix(
            tab,
            Purpose::Save {
                path,
                intent,
                format_after,
                continuation,
            },
            None,
            context,
        )
    }

    fn start_autofix(
        &mut self,
        tab: u64,
        purpose: Purpose,
        selected: Option<crate::tex::autofix::Rule>,
        context: &egui::Context,
    ) -> bool {
        let Some(document) = self.document_for_tab(tab) else {
            return false;
        };
        let key = document.key();
        let source: Arc<str> = document.source().as_str().into();
        let settings = self.settings.autofix.clone();
        let markers = self.settings.comment_controls.clone();
        let project_root = self
            .tab_workspace(tab)
            .unwrap_or(&self.workspace_root)
            .to_owned();
        let tool = self
            .tex_tools
            .badness
            .is_available()
            .then(|| self.tex_tools.badness.clone());
        let program = tool.clone();
        let cancel = Arc::<AtomicBool>::default();
        let (worker_source, mut worker_settings, worker_markers, worker_cancel) = (
            source.clone(),
            settings.clone(),
            markers.clone(),
            cancel.clone(),
        );
        if let Some(rule) = selected {
            worker_settings
                .enabled
                .retain(|candidate| *candidate == rule);
        }
        let mut job = LatestJob::default();
        match job.start_and_repaint("TeX autofix", context, move || {
            crate::tex::autofix::compute(
                &worker_source,
                &worker_settings,
                &worker_markers,
                &project_root,
                program.as_ref(),
                &worker_cancel,
            )
        }) {
            Ok(()) => {
                self.autofix = State::Running(Box::new(Task {
                    tab,
                    key,
                    source,
                    settings,
                    markers,
                    tool,
                    purpose,
                    cancel,
                    job,
                }));
                true
            }
            Err(error) => {
                if let Purpose::Save {
                    continuation: Some(token),
                    ..
                } = &purpose
                    && self.document_workflow.continuation_token() == Some(*token)
                {
                    self.document_workflow.cancel_continuation();
                }
                self.autofix = State::Failed(error.clone());
                self.show_file_error(error);
                false
            }
        }
    }

    pub(super) fn update_autofix(&mut self, context: &egui::Context) {
        if let State::Running(task) = &self.autofix
            && !self.admits_autofix(task)
        {
            self.autofix.cancel();
        }
        let result = match &mut self.autofix {
            State::Running(task) | State::Cancelling(task) => task.job.poll(),
            State::Idle | State::Failed(_) => return,
        };
        match result {
            LatestJobPoll::Pending | LatestJobPoll::Idle => {}
            LatestJobPoll::Ready(plan) => {
                let phase = std::mem::take(&mut self.autofix);
                match phase {
                    State::Running(task)
                        if self.admits_autofix(&task)
                            && self.document_for_tab(task.tab).is_some_and(|doc| {
                                doc.source().as_str() == task.source.as_ref()
                            }) =>
                    {
                        if let Purpose::Save { path, intent, .. } = &task.purpose
                            && !self.autofix_save_admitted(task.tab, path, *intent)
                        {
                            self.autofix_save_cancelled(&task);
                            return;
                        }
                        if self.apply_autofix_plan_to_tab(task.tab, plan, context)
                            && let Purpose::Save {
                                path,
                                intent,
                                format_after,
                                ..
                            } = &task.purpose
                        {
                            self.submit_save_prepared(
                                task.tab,
                                path.clone(),
                                *intent,
                                *format_after,
                                context,
                            );
                        }
                    }
                    State::Running(task) | State::Cancelling(task) => {
                        self.autofix_save_cancelled(&task)
                    }
                    State::Idle | State::Failed(_) => unreachable!("a completion owns its task"),
                }
            }
            LatestJobPoll::Failed(error) => {
                let phase = std::mem::take(&mut self.autofix);
                if let State::Running(task) = phase {
                    self.autofix_save_cancelled(&task);
                    self.autofix = State::Failed(error.clone());
                    self.show_file_error(error);
                } else {
                    if let State::Cancelling(task) = phase {
                        self.autofix_save_cancelled(&task);
                    }
                    self.autofix = State::Idle;
                }
            }
        }
    }

    fn admits_autofix(&self, task: &Task) -> bool {
        let key = match &task.purpose {
            Purpose::Edit => (!self.source_edit_blocked()
                && self.tabs.active_id() == Some(task.tab))
            .then(|| self.document().key()),
            Purpose::Save { continuation, .. } => {
                if !self.settings.autofix_on_save
                    || (self.tabs.active_id() == Some(task.tab) && self.source_edit_locked())
                    || self.document_workflow.continuation_token() != *continuation
                    || self.document_workflow.save_in_flight()
                    || (continuation.is_some() && self.tabs.active_id() != Some(task.tab))
                {
                    return false;
                }
                self.document_for_tab(task.tab)
                    .map(|document| document.key())
            }
        };
        task.admits(
            key,
            &self.settings.autofix,
            &self.settings.comment_controls,
            self.tex_tools
                .badness
                .is_available()
                .then_some(&self.tex_tools.badness),
        )
    }

    fn autofix_save_cancelled(&mut self, task: &Task) {
        if let Purpose::Save {
            intent,
            continuation,
            ..
        } = task.purpose
        {
            if continuation.is_some() && self.document_workflow.continuation_token() == continuation
            {
                self.document_workflow.cancel_continuation();
            }
            if intent == SaveIntent::Auto {
                self.set_tab_autosave(task.tab, Some(Instant::now() + AUTOSAVE_RETRY_DELAY));
            } else if self
                .notice
                .as_ref()
                .is_none_or(|notice| !matches!(notice.kind, NoticeKind::Error))
            {
                self.notice = Some(Notice { message: "Save paused because the document or autofix settings changed. Save again to continue.".into(), kind: NoticeKind::Info });
            }
        }
    }

    #[cfg(test)]
    fn apply_autofix_plan(&mut self, plan: Plan, context: &egui::Context) {
        self.apply_autofix_plan_to_tab(self.tabs.active_id().unwrap(), plan, context);
    }

    fn apply_autofix_plan_to_tab(&mut self, tab: u64, plan: Plan, context: &egui::Context) -> bool {
        let Some(document) = self.document_for_tab(tab) else {
            return false;
        };
        let text = match plan.apply(document.source()) {
            Ok(text) => text,
            Err(error) => {
                self.autofix = State::Failed(error.into());
                self.show_file_error(error.into());
                return false;
            }
        };
        if self.tabs.active_id() != Some(tab) {
            let record = self.tabs.record_mut(tab).unwrap();
            if text != record.document.source().as_str() {
                let cursor = record
                    .editor
                    .as_ref()
                    .and_then(|editor| editor.cursor.char_range())
                    .unwrap_or_default();
                let mapped = map_cursor(record.document.source(), &plan.edits, cursor);
                record.document.edit(cursor, |source| *source = text);
                record.document.take_edit();
                if let Some(editor) = &mut record.editor {
                    editor.clear_undoer();
                    editor.cursor.set_char_range(Some(mapped));
                }
                record.pdf_output.invalidate();
                if self.preview_document_kind() == DocumentKind::Tex {
                    self.invalidate_tex_build_inputs();
                }
                self.schedule_project_index();
            }
        } else if text != self.document().source().as_str() {
            let cursor = self.editor_snapshot(context).cursor;
            let mapped = map_cursor(self.document().source(), &plan.edits, cursor);
            self.document_mut().edit(cursor, |source| *source = text);
            let selection = mapped.secondary.index.0..mapped.primary.index.0;
            self.pending_editor_selection = Some(
                if context.memory(|memory| memory.has_focus(source_editor_id(context))) {
                    EditorSelection::Focus(selection)
                } else {
                    EditorSelection::Search(selection)
                },
            );
            self.find_bar.search.clear();
            self.mark_edited();
        }
        let message = if plan.fixes == 0 {
            "No enabled TeX autofixes found.".to_owned()
        } else {
            format!(
                "Applied {} TeX autofix{}.",
                plan.fixes,
                if plan.fixes == 1 { "" } else { "es" }
            )
        };
        self.notice = Some(Notice {
            message: plan
                .warning
                .map_or_else(|| message.clone(), |warning| format!("{message} {warning}")),
            kind: NoticeKind::Info,
        });
        context.request_repaint();
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tex::autofix::Rule;

    fn fixture(context: &egui::Context, root: &Path) -> EditorApp {
        let mut app = EditorApp::dormant_for_tests(context, root.into());
        app.document_mut().replace_loaded_unprojected(
            "α see  \\eqref{x} and  \\ref{y}.".into(),
            root.join("fix.tex"),
            DocumentKind::Tex,
            None,
        );
        app.settings.auto_save = false;
        app.document_mut().set_history_reset(false);
        app
    }
    fn plan(source: &str) -> Plan {
        let settings = Settings {
            enabled: [Rule::ReferenceTies].into(),
        };
        crate::tex::autofix::compute(
            source,
            &settings,
            &Default::default(),
            Path::new("."),
            None,
            &AtomicBool::new(false),
        )
        .unwrap()
    }
    fn ready_task(app: &EditorApp) -> Box<Task> {
        let result = plan(app.document().source());
        let mut job = LatestJob::default();
        job.start("autofix-test", move || Ok(result)).unwrap();
        job.wait_until_ready_for_test().unwrap();
        Box::new(Task {
            tab: app.tabs.active_id().unwrap(),
            tool: app
                .tex_tools
                .badness
                .is_available()
                .then(|| app.tex_tools.badness.clone()),
            purpose: Purpose::Edit,
            key: app.document().key(),
            source: app.document().source().as_str().into(),
            settings: app.settings.autofix.clone(),
            markers: app.settings.comment_controls.clone(),
            cancel: Arc::default(),
            job,
        })
    }
    #[test]
    fn autofix_completion_is_one_undoable_edit_and_repeated_poll_does_nothing() {
        let root = tempfile::tempdir().unwrap();
        let context = egui::Context::default();
        let mut app = fixture(&context, root.path());
        let original = app.document().source().to_owned();
        app.autofix = State::Running(ready_task(&app));
        app.update_autofix(&context);
        assert_eq!(app.document().source(), "α see~\\eqref{x} and~\\ref{y}.");
        let key = app.document().key();
        app.update_autofix(&context);
        assert_eq!(app.document().key(), key);
        assert!(matches!(app.autofix, State::Idle));
        app.document_mut()
            .history_step(false, CCursorRange::default())
            .unwrap();
        assert_eq!(app.document().source(), &original);
        assert!(
            app.document_mut()
                .history_step(false, CCursorRange::default())
                .is_none()
        );
    }
    #[test]
    fn autofix_rejects_changed_source_owner_preferences_markers_and_read_only_source() {
        for change in 0..6 {
            let root = tempfile::tempdir().unwrap();
            let context = egui::Context::default();
            let mut app = fixture(&context, root.path());
            let task = ready_task(&app);
            let cancel = task.cancel.clone();
            app.autofix = State::Running(task);
            match change {
                0 => {
                    app.document_mut()
                        .edit(CCursorRange::default(), |source| source.push('!'));
                }
                1 => {
                    let source = app.document().source().to_owned();
                    app.document_mut().replace_loaded_unprojected(
                        source,
                        root.path().join("other.tex"),
                        DocumentKind::Tex,
                        None,
                    );
                }
                2 => {
                    let rule = Rule::ALL[0];
                    let enabled = rule.enabled(&app.settings.autofix);
                    rule.set_enabled(&mut app.settings.autofix, !enabled);
                }
                3 => app.settings.comment_controls.both.off = "pause".into(),
                4 => app.autofix.cancel(),
                5 => {
                    app.tabs = app.tabs.empty_after();
                }
                _ => unreachable!(),
            }
            let source = app.document().source().to_owned();
            let key = app.document().key();
            app.update_autofix(&context);
            assert_eq!(app.document().source(), &source, "change {change}");
            assert_eq!(app.document().key(), key, "change {change}");
            assert!(cancel.load(Ordering::Acquire));
            assert!(matches!(app.autofix, State::Idle));
        }
    }
    #[test]
    fn autofix_repeated_shortcuts_do_not_replace_a_pending_worker_and_owner_departure_cancels() {
        let root = tempfile::tempdir().unwrap();
        let context = egui::Context::default();
        let mut app = fixture(&context, root.path());
        let task = ready_task(&app);
        let cancel = task.cancel.clone();
        app.autofix = State::Running(task);
        app.begin_autofix(&context);
        assert!(!cancel.load(Ordering::Acquire));
        assert!(matches!(app.autofix, State::Running(_)));
        app.reset_document_services();
        assert!(cancel.load(Ordering::Acquire));
        assert!(matches!(app.autofix, State::Cancelling(_)));
        app.begin_autofix(&context);
        assert!(matches!(app.autofix, State::Cancelling(_)));
        app.update_autofix(&context);
        assert!(matches!(app.autofix, State::Idle));
    }
    #[test]
    fn autofix_maps_unicode_and_reversed_selection_without_repeated_source_scans() {
        let source = "α see  \\eqref{x} and  \\ref{y}.";
        let edits = plan(source).edits;
        let cursor = CCursorRange::two(CCursor::new(source.chars().count()), CCursor::new(6));
        let mapped = map_cursor(source, &edits, cursor);
        assert_eq!(mapped.primary.index.0, 6);
        assert_eq!(mapped.secondary.index.0, cursor.secondary.index.0 - 2);
    }
    #[test]
    fn autofix_command_is_tex_only_and_never_mutates_typst() {
        let root = tempfile::tempdir().unwrap();
        let context = egui::Context::default();
        let mut app = fixture(&context, root.path());
        assert!(app.native_command_enabled(AppCommand::Autofix));
        for kind in [DocumentKind::Typst, DocumentKind::Text] {
            app.document_mut().replace_loaded_unprojected(
                "see  \\ref{x}".into(),
                root.path().join("other.typ"),
                kind,
                None,
            );
            let key = app.document().key();
            assert!(!app.native_command_enabled(AppCommand::Autofix));
            app.execute_app_command(AppCommand::Autofix, &context, None);
            assert_eq!(app.document().key(), key);
            assert!(matches!(app.autofix, State::Idle));
        }
    }
}

#[cfg(test)]
mod focus_tests {
    use super::*;
    use crate::tex::autofix::Rule;
    #[test]
    fn async_autofix_preserves_selection_direction_and_other_field_focus() {
        for source_focused in [true, false] {
            let root = tempfile::tempdir().unwrap();
            let context = egui::Context::default();
            let mut app = EditorApp::dormant_for_tests(&context, root.path().into());
            let source = "α see  \\eqref{x}";
            app.document_mut().replace_loaded_unprojected(
                source.into(),
                root.path().join("focus.tex"),
                DocumentKind::Tex,
                None,
            );
            let cursor = CCursorRange::two(CCursor::new(source.chars().count()), CCursor::new(2));
            let mut state = egui::text_edit::TextEditState::default();
            state.cursor.set_char_range(Some(cursor));
            state.store(&context, source_editor_id(&context));
            let field = if source_focused {
                source_editor_id(&context)
            } else {
                egui::Id::new("find-query")
            };
            context.memory_mut(|memory| memory.request_focus(field));
            let settings = Settings {
                enabled: [Rule::ReferenceTies].into(),
            };
            let plan = crate::tex::autofix::compute(
                source,
                &settings,
                &Default::default(),
                root.path(),
                None,
                &AtomicBool::new(false),
            )
            .unwrap();
            app.apply_autofix_plan(plan, &context);
            let pending = app.pending_editor_selection.as_ref().unwrap();
            assert_eq!(pending.takes_focus(), source_focused);
            let pending = pending.range();
            let actual = CCursorRange::two(CCursor::new(pending.start), CCursor::new(pending.end));
            assert_eq!(actual.primary.index.0, 2);
            assert_eq!(actual.secondary.index.0, source.chars().count() - 1);
            assert!(context.memory(|memory| memory.has_focus(field)));
        }
    }
}
