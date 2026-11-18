//! Explicit diagnostic fixes are bound to one source revision and reviewed before editing.
use super::*;
use crate::diagnostic_fix_request::{Proposal, Request};
use crate::git::editor::{document_diff::DocumentDiff, full_view};
use std::{
    cell::RefCell,
    sync::atomic::{AtomicBool, Ordering},
};

pub(super) const SALT: &str = "tiptoptyp-diagnostic-fix";
pub(super) const MENU_LABEL: &str = "Suggest fix";

#[derive(Debug, Clone)]
pub(super) struct BoundDiagnostic {
    snapshot: crate::document::DocumentSnapshot,
    kind: DocumentKind,
    diagnostic: Diagnostic,
    cursor: CCursorRange,
}
impl PartialEq for BoundDiagnostic {
    fn eq(&self, other: &Self) -> bool {
        self.snapshot.key() == other.snapshot.key()
            && self.snapshot.source() == other.snapshot.source()
            && self.kind == other.kind
            && self.diagnostic == other.diagnostic
            && self.cursor == other.cursor
    }
}
impl Eq for BoundDiagnostic {}
impl BoundDiagnostic {
    fn current(&self, key: DocumentKey) -> bool {
        self.snapshot.key() == key
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct SourceCacheKey {
    source: preview_jump::SourceLocation,
    diagnostics: u64,
    preview: Option<u64>,
}
#[derive(Default)]
struct SourceCache {
    key: Option<SourceCacheKey>,
    target: Option<Arc<BoundDiagnostic>>,
    #[cfg(test)]
    rebuilds: usize,
}

pub(super) struct State {
    client: Arc<std::sync::Mutex<crate::diagnostic_fix::Client>>,
    job: crate::worker::ExclusiveJob<Reply>,
    next: u64,
    request: RequestAdmission,
    dialog: Option<Dialog>,
    last_error: Option<String>,
    source_cache: RefCell<SourceCache>,
}
impl Default for State {
    fn default() -> Self {
        Self {
            client: crate::diagnostic_fix::shared_client(),
            job: Default::default(),
            next: 0,
            request: RequestAdmission::Idle,
            dialog: None,
            last_error: None,
            source_cache: RefCell::default(),
        }
    }
}
impl Drop for State {
    fn drop(&mut self) {
        self.request.cancel();
    }
}
struct Review {
    proposal: Proposal,
    diff: DocumentDiff,
}
struct Reply {
    id: u64,
    result: Result<Review, String>,
}
impl crate::worker::OperationSummary for Reply {
    fn completion_summary(&self) -> String {
        if self.result.is_ok() {
            "Diagnostic suggestion finished."
        } else {
            "Diagnostic suggestion failed."
        }
        .into()
    }
}
/// Acceptance lifetime is independent of the worker lifetime: a closed or
/// stale review cancels admission immediately, while its worker still drains.
#[derive(Default)]
enum RequestAdmission {
    #[default]
    Idle,
    Running(AdmittedRequest),
    Cancelled(AdmittedRequest),
}
struct AdmittedRequest {
    id: u64,
    cancel: Arc<AtomicBool>,
}
impl Drop for AdmittedRequest {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Release);
    }
}
impl RequestAdmission {
    fn id(&self) -> Option<u64> {
        match self {
            Self::Idle => None,
            Self::Running(request) | Self::Cancelled(request) => Some(request.id),
        }
    }
    fn cancel(&mut self) {
        *self = match std::mem::replace(self, Self::Idle) {
            Self::Running(request) => {
                request.cancel.store(true, Ordering::Release);
                Self::Cancelled(request)
            }
            other => other,
        };
    }
    fn finish(&mut self, id: u64) -> bool {
        if self.id() != Some(id) {
            return false;
        }
        matches!(std::mem::take(self), Self::Running(request) if !request.cancel.load(Ordering::Acquire))
    }
}

struct Dialog {
    id: u64,
    target: BoundDiagnostic,
    state: ReviewState,
}
/// A result is either being prepared, reviewable, failed, or obsolete. Failed
/// application retains its comparison for inspection but cannot be accepted.
enum ReviewState {
    Preparing,
    Ready(Review),
    Failed {
        message: String,
        retained: Option<Review>,
    },
    Stale,
}
impl ReviewState {
    fn error(&self) -> Option<&str> {
        match self {
            Self::Failed { message, .. } => Some(message),
            Self::Stale => Some("The document changed; close this review and request a new fix."),
            _ => None,
        }
    }
    fn review(&self) -> Option<&Review> {
        match self {
            Self::Ready(review) => Some(review),
            Self::Failed { retained, .. } => retained.as_ref(),
            _ => None,
        }
    }
    fn fail(&mut self, message: String) {
        let retained = match std::mem::replace(self, Self::Preparing) {
            Self::Ready(review) => Some(review),
            Self::Failed { retained, .. } => retained,
            _ => None,
        };
        *self = Self::Failed { message, retained };
    }
    #[cfg(all(feature = "desktop-ui-tests", target_os = "macos"))]
    fn name(&self) -> &'static str {
        match self {
            Self::Preparing => "preparing",
            Self::Ready(_) => "reviewing",
            Self::Failed { .. } => "failed",
            Self::Stale => "stale",
        }
    }
}
impl State {
    pub(super) fn visible(&self) -> bool {
        self.dialog.is_some()
    }
    pub(super) fn busy(&self) -> bool {
        self.job.is_running()
    }
    pub(super) fn activity(&self) -> crate::activity::Activity {
        if self.busy() {
            crate::activity::Activity::Running
        } else if let Some(error) = self.last_error.as_ref() {
            crate::activity::Activity::Failed(error.clone())
        } else if self.visible() {
            crate::activity::Activity::Idle
        } else {
            crate::activity::Activity::Inactive("No diagnostic suggestion")
        }
    }
    fn accept_reply(&mut self, reply: Reply, current: Option<DocumentKey>) {
        let accepted = self.request.finish(reply.id)
            && self.dialog.as_ref().is_some_and(|dialog| {
                dialog.id == reply.id && current.is_some_and(|key| dialog.target.current(key))
            });
        if !accepted {
            return;
        }
        let dialog = self
            .dialog
            .as_mut()
            .expect("accepted reply owns its dialog");
        match reply.result {
            Ok(review) => {
                dialog.state = ReviewState::Ready(review);
            }
            Err(error) => {
                self.last_error = Some(error.clone());
                dialog.state = ReviewState::Failed {
                    message: error,
                    retained: None,
                };
            }
        }
    }
}

impl EditorApp {
    fn diagnostic_fix_eligible(&self, diagnostic: &Diagnostic) -> bool {
        !self.tabs.is_empty()
            && matches!(
                self.document().kind(),
                DocumentKind::Typst | DocumentKind::Tex
            )
            && diagnostic
                .location
                .is_some_and(|location| location.line > 0 && location.column > 0)
            && self.diagnostic_targets_current_document(diagnostic)
            && diagnostic.correction.as_ref().is_none_or(|correction| {
                correction
                    .document
                    .is_none_or(|key| key == self.document().key())
            })
    }

    pub(super) fn bind_diagnostic_fix(
        &self,
        diagnostic: &Diagnostic,
        cursor: CCursorRange,
    ) -> Option<BoundDiagnostic> {
        self.diagnostic_fix_eligible(diagnostic)
            .then(|| BoundDiagnostic {
                snapshot: self.document().snapshot(),
                kind: self.document().kind(),
                diagnostic: diagnostic.clone(),
                cursor,
            })
    }

    pub(super) fn diagnostic_fix_at_source(
        &self,
        source: preview_jump::SourceLocation,
    ) -> Option<Arc<BoundDiagnostic>> {
        if source.key != self.document().key() || self.diagnostic_fix.busy() {
            return None;
        }
        let key = SourceCacheKey {
            source,
            diagnostics: self.diagnostic_regions.generation,
            preview: self.tabs.preview_id(),
        };
        {
            let cache = self.diagnostic_fix.source_cache.borrow();
            if cache.key == Some(key) {
                // Repainting an open menu must not scan diagnostics, resolve
                // filesystem paths or clone the selected message again.
                return cache.target.clone();
            }
        }
        let (line, column) = self.editor_data.line_column_at_char(source.char_index);
        let nearest = self
            .preview
            .diagnostics
            .iter()
            .chain(&self.preview.editor_diagnostics)
            .filter(|diagnostic| {
                diagnostic
                    .location
                    .is_some_and(|location| location.line == line)
                    && self.diagnostic_fix_eligible(diagnostic)
            })
            .min_by_key(|diagnostic| diagnostic.location.unwrap().column.abs_diff(column));
        let target = nearest
            .and_then(|diagnostic| {
                self.bind_diagnostic_fix(
                    diagnostic,
                    CCursorRange::one(CCursor::new(source.char_index)),
                )
            })
            .map(Arc::new);
        let mut cache = self.diagnostic_fix.source_cache.borrow_mut();
        cache.key = Some(key);
        cache.target = target.clone();
        #[cfg(test)]
        {
            cache.rebuilds += 1;
        }
        target
    }

    pub(super) fn begin_diagnostic_fix(
        &mut self,
        target: BoundDiagnostic,
        context: &egui::Context,
    ) {
        if self.reject_read_only_edit(context)
            || self.document_flow_busy()
            || self.diagnostic_fix.busy()
        {
            return;
        }
        if self.tabs.is_empty()
            || !target.current(self.document().key())
            || target.snapshot.source() != self.document().source()
        {
            self.show_file_error("The document changed; reopen the diagnostic menu.".into());
            return;
        }
        self.close_app_popup();
        self.clear_editor_hover();
        self.clear_asset_hover();
        self.find_bar.close();
        ChildViewHost::close(context, find_bar::VIEWPORT_SALT);
        self.diagnostic_fix.next = self.diagnostic_fix.next.wrapping_add(1);
        let id = self.diagnostic_fix.next;
        let config = crate::liquid_runtime::Config::from_settings(&self.settings);
        self.diagnostic_fix.dialog = Some(Dialog {
            id,
            target: target.clone(),
            state: match &config {
                Ok(_) => ReviewState::Preparing,
                Err(error) => ReviewState::Failed {
                    message: error.clone(),
                    retained: None,
                },
            },
        });
        let Ok(config) = config else {
            self.diagnostic_fix.last_error = self
                .diagnostic_fix
                .dialog
                .as_ref()
                .and_then(|dialog| dialog.state.error().map(str::to_owned));
            context.request_repaint();
            return;
        };
        self.diagnostic_fix.last_error = None;
        let cancel = Arc::<AtomicBool>::default();
        self.diagnostic_fix.request = RequestAdmission::Running(AdmittedRequest {
            id,
            cancel: cancel.clone(),
        });
        let client = Arc::clone(&self.diagnostic_fix.client);
        if let Err(error) =
            self.diagnostic_fix
                .job
                .start_and_repaint("diagnostic-fix", context, move || {
                    let result = (|| {
                        let request = Request::new(
                            target.snapshot.source(),
                            target.kind,
                            &target.diagnostic,
                        )?;
                        let mut client = crate::diagnostic_fix::lock_client(&client, &cancel)?;
                        let replacement = client.generate(
                            &config.directory,
                            config.threads,
                            &request.prompt(),
                            &cancel,
                        )?;
                        if cancel.load(Ordering::Acquire) {
                            return Err("Suggestion cancelled.".into());
                        }
                        let proposal = request.proposal(replacement)?;
                        let diff =
                            DocumentDiff::new(proposal.before.clone(), &proposal.replacement);
                        Ok(Review { proposal, diff })
                    })();
                    Ok(Reply { id, result })
                })
        {
            self.diagnostic_fix.request = RequestAdmission::Idle;
            self.diagnostic_fix.last_error = Some(error.clone());
            self.diagnostic_fix
                .dialog
                .as_mut()
                .unwrap()
                .state
                .fail(error);
        }
    }

    pub(super) fn update_diagnostic_fix(&mut self) {
        let current = (!self.tabs.is_empty()).then(|| self.document().key());
        let stale_cache = self
            .diagnostic_fix
            .source_cache
            .borrow()
            .key
            .is_some_and(|key| {
                Some(key.source.key) != current
                    || key.diagnostics != self.diagnostic_regions.generation
                    || key.preview != self.tabs.preview_id()
            });
        if stale_cache {
            let cache = self.diagnostic_fix.source_cache.get_mut();
            cache.key = None;
            cache.target = None;
        }
        match self.diagnostic_fix.job.poll() {
            LatestJobPoll::Ready(reply) => self.diagnostic_fix.accept_reply(reply, current),
            LatestJobPoll::Failed(error) => {
                if let Some(id) = self.diagnostic_fix.request.id() {
                    self.diagnostic_fix.accept_reply(
                        Reply {
                            id,
                            result: Err(error),
                        },
                        current,
                    );
                }
            }
            LatestJobPoll::Pending | LatestJobPoll::Idle => {}
        }
        if let Some(dialog) = &mut self.diagnostic_fix.dialog
            && current.is_none_or(|key| !dialog.target.current(key))
        {
            self.diagnostic_fix.request.cancel();
            dialog.state = ReviewState::Stale;
        }
    }

    pub(super) fn close_diagnostic_fix(&mut self, context: &egui::Context) {
        self.diagnostic_fix.request.cancel();
        self.diagnostic_fix.dialog = None;
        ChildViewHost::close(context, SALT);
        crate::window_host::return_focus(context);
    }

    fn apply_diagnostic_fix(
        &mut self,
        target: &BoundDiagnostic,
        proposal: &Proposal,
    ) -> Result<(), String> {
        if self.overleaf_source_locked() {
            return Err("Wait for Overleaf to finish syncing before accepting the fix.".into());
        }
        if self.tabs.is_empty()
            || !target.current(self.document().key())
            || target.snapshot.source() != self.document().source()
        {
            return Err("The document changed; request a new fix.".into());
        }
        let replacement = proposal.apply_to(self.document().source())?;
        let start = self.document().source()[..proposal.byte_range.start]
            .chars()
            .count();
        let end = start + proposal.replacement.chars().count();
        self.document_mut()
            .edit(target.cursor, |source| *source = replacement);
        self.pending_editor_selection = Some(EditorSelection::Focus(start..end));
        self.find_bar.search.clear();
        self.mark_edited();
        Ok(())
    }

    pub(super) fn show_diagnostic_fix(&mut self, context: &egui::Context) {
        let busy = self.diagnostic_fix.busy();
        let Some(dialog) = &mut self.diagnostic_fix.dialog else {
            return;
        };
        let Some(owner) = context.input(|input| input.viewport().inner_rect) else {
            return;
        };
        let current = !self.tabs.is_empty()
            && dialog
                .target
                .current(self.tabs.current_record().document.key());
        let style = context.style_of(context.theme());
        let shortcuts = self.settings.effective_shortcuts();
        let mut action = None;
        ChildViewHost::show(
            context,
            &self.captures,
            ChildViewSpec::modal(SALT, "Suggest fix", owner, "diagnostic-fix"),
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
                    action = Some(Action::Cancel);
                }
                ui.painter()
                    .rect_filled(ui.max_rect(), 0.0, Color32::from_black_alpha(64));
                if let Some(clicked) =
                    review_card(ui.ctx(), dialog, busy, current, owner, &self.settings)
                {
                    action = Some(clicked);
                }
            },
        );
        match action {
            Some(Action::Cancel) => self.close_diagnostic_fix(context),
            Some(Action::Accept)
                if !busy
                    && current
                    && self
                        .diagnostic_fix
                        .dialog
                        .as_ref()
                        .is_some_and(|dialog| matches!(dialog.state, ReviewState::Ready(_))) =>
            {
                let dialog = self.diagnostic_fix.dialog.as_ref().unwrap();
                let target = dialog.target.clone();
                let proposal = dialog.state.review().map(|review| review.proposal.clone());
                if let Some(proposal) = proposal {
                    match self.apply_diagnostic_fix(&target, &proposal) {
                        Ok(()) => {
                            self.close_diagnostic_fix(context);
                            self.focus_source_editor(context);
                        }
                        Err(error) => self
                            .diagnostic_fix
                            .dialog
                            .as_mut()
                            .unwrap()
                            .state
                            .fail(error),
                    }
                }
            }
            _ => {}
        }
    }

    #[cfg(all(feature = "desktop-ui-tests", target_os = "macos"))]
    pub(super) fn diagnostic_fix_inspection(&self) -> serde_json::Value {
        let dialog = self.diagnostic_fix.dialog.as_ref();
        serde_json::json!({
            "visible": self.diagnostic_fix.visible(), "busy": self.diagnostic_fix.busy(),
            "current": dialog.is_some_and(|dialog| !self.tabs.is_empty() && dialog.target.current(self.document().key())),
            "phase": dialog.map(|dialog| dialog.state.name()).unwrap_or("closed"),
            "error": dialog.and_then(|dialog| dialog.state.error()),
            "line": dialog.and_then(|dialog| dialog.target.diagnostic.line()),
            "diagnostic_fingerprint": dialog.map(|dialog| crate::desktop_test::fingerprint(&dialog.target.diagnostic.message)),
            "comparison": dialog.and_then(|dialog| dialog.state.review()).map(|review| serde_json::json!({
                "before": crate::desktop_test::fingerprint(&review.diff.before), "after": crate::desktop_test::fingerprint(&review.diff.current),
                "before_chars": review.diff.before.chars().count(), "after_chars": review.diff.current.chars().count(),
                "style": self.settings.git_diff_style,
            })),
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Action {
    Cancel,
    Accept,
}

fn review_card(
    context: &egui::Context,
    dialog: &mut Dialog,
    busy: bool,
    current: bool,
    owner: Rect,
    settings: &AppSettings,
) -> Option<Action> {
    let style = context.style_of(context.theme());
    let mut action = None;
    egui::Area::new(viewport_scoped_id(context, "diagnostic-fix-card"))
        .order(egui::Order::Foreground)
        .anchor(egui::Align2::CENTER_CENTER, Vec2::ZERO)
        .show(context, |ui| {
            theme::dialog_card_frame(&style).show(ui, |ui| {
                ui.set_width((owner.width() - 48.0).clamp(1.0, 960.0));
                ui.set_max_height((owner.height() - 48.0).max(1.0));
                egui::ScrollArea::vertical()
                    .id_salt("diagnostic-fix-card-scroll")
                    .max_height((owner.height() - 48.0).max(1.0))
                    .show(ui, |ui| {
                        action = review_ui(ui, dialog, busy, current, owner.height(), settings);
                    });
            });
        });
    action
}

fn review_ui(
    ui: &mut egui::Ui,
    dialog: &Dialog,
    busy: bool,
    current: bool,
    height: f32,
    settings: &AppSettings,
) -> Option<Action> {
    let mut action = None;
    ui.strong(format!(
        "{} · suggested fix",
        crate::diagnostic_fix::MODEL_LABEL
    ));
    ui.label(format!(
        "Line {} · {}",
        dialog.target.diagnostic.line().unwrap_or(1),
        dialog.target.diagnostic.message
    ));
    ui.separator();
    if busy {
        ui.horizontal(|ui| {
            ui.spinner();
            ui.label("Preparing Liquid / suggesting fix…");
        });
    }
    if let Some(error) = dialog.state.error() {
        ui.colored_label(ui.visuals().error_fg_color, error);
    } else if let Some(review) = dialog.state.review() {
        liquid::comparison_headers(ui, settings.git_diff_style);
        let path = Path::new(if dialog.target.kind == DocumentKind::Tex {
            "fix.tex"
        } else {
            "fix.typ"
        });
        ui.allocate_ui(
            Vec2::new(ui.available_width(), (height - 220.0).clamp(40.0, 520.0)),
            |ui| {
                let id = viewport_scoped_id(ui.ctx(), "diagnostic-fix-diff");
                let viewport =
                    full_view::show_styled_comparison(ui, &review.diff, id, path, settings);
                #[cfg(all(feature = "desktop-ui-tests", target_os = "macos"))]
                crate::desktop_test::observe(
                    "diagnostic_fix.diff",
                    &ui.interact(viewport, id.with("observed"), Sense::hover()),
                );
                #[cfg(not(all(feature = "desktop-ui-tests", target_os = "macos")))]
                let _ = viewport;
            },
        );
    }
    ui.separator();
    ui.horizontal(|ui| {
        let accept = ui.add_enabled(
            current && !busy && matches!(dialog.state, ReviewState::Ready(_)),
            egui::Button::new("Accept"),
        );
        #[cfg(all(feature = "desktop-ui-tests", target_os = "macos"))]
        crate::desktop_test::observe("diagnostic_fix.accept", &accept);
        if accept.clicked() {
            action = Some(Action::Accept);
        }
        let cancel = ui.button("Cancel");
        #[cfg(all(feature = "desktop-ui-tests", target_os = "macos"))]
        crate::desktop_test::observe("diagnostic_fix.cancel", &cancel);
        if cancel.clicked() {
            action = Some(Action::Cancel);
        }
    });
    action
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(
        context: &egui::Context,
        root: &Path,
        kind: DocumentKind,
    ) -> (EditorApp, Diagnostic) {
        let mut app = EditorApp::dormant_for_tests(context, root.into());
        let path = root.join(if kind == DocumentKind::Tex {
            "main.tex"
        } else {
            "main.typ"
        });
        let source = "// α context\r\n#text(fill: bleu)[Hello]\r\n// preserve β\r\n";
        fs::write(&path, source).unwrap();
        app.document_mut()
            .replace_loaded_unprojected(source.into(), path.clone(), kind, None);
        let diagnostic = Diagnostic {
            correction: None,
            provider: Some("Tinymist".into()),
            severity: DiagnosticSeverity::Error,
            source: DiagnosticSource::File(path),
            location: Some(DiagnosticLocation {
                line: 2,
                column: 13,
            }),
            message: "unknown variable: bleu".into(),
            details: vec![],
        };
        (app, diagnostic)
    }

    fn target(app: &EditorApp, diagnostic: &Diagnostic) -> BoundDiagnostic {
        app.bind_diagnostic_fix(diagnostic, CCursorRange::one(CCursor::new(3)))
            .unwrap()
    }

    fn review(target: &BoundDiagnostic) -> Review {
        let request =
            Request::new(target.snapshot.source(), target.kind, &target.diagnostic).unwrap();
        let proposal = request.proposal("#text(fill: blue)[Hello]".into()).unwrap();
        let diff = DocumentDiff::new(proposal.before.clone(), &proposal.replacement);
        Review { proposal, diff }
    }

    fn dialog(target: BoundDiagnostic) -> Dialog {
        Dialog {
            id: 1,
            state: ReviewState::Ready(review(&target)),
            target,
        }
    }

    #[test]
    fn acceptance_is_one_undo_transaction_with_exact_unicode_context_and_crlf_preservation() {
        let directory = tempfile::tempdir().unwrap();
        let context = egui::Context::default();
        let (mut app, diagnostic) = fixture(&context, directory.path(), DocumentKind::Typst);
        let target = target(&app, &diagnostic);
        let proposed = review(&target);
        let original = app.document().source().to_owned();
        app.apply_diagnostic_fix(&target, &proposed.proposal)
            .unwrap();
        assert_eq!(app.document().source(), &original.replace("bleu", "blue"));
        assert!(
            app.apply_diagnostic_fix(&target, &proposed.proposal)
                .is_err()
        );
        app.undo_editor(&context, false);
        assert_eq!(app.document().source(), &original);
        assert!(
            app.apply_diagnostic_fix(&target, &proposed.proposal)
                .is_err(),
            "Undo also advances the revision"
        );
        assert_eq!(
            fs::read_to_string(app.document().path().as_ref().unwrap()).unwrap(),
            original
        );
    }

    #[test]
    fn cancelling_review_preserves_source_tabs_and_diagnostics() {
        let directory = tempfile::tempdir().unwrap();
        let context = egui::Context::default();
        let (mut app, diagnostic) = fixture(&context, directory.path(), DocumentKind::Typst);
        app.preview.editor_diagnostics = vec![diagnostic.clone()];
        let key = app.document().key();
        let tabs = app.tabs.ids().collect::<Vec<_>>();
        let original = app.document().source().to_owned();
        app.diagnostic_fix.dialog = Some(dialog(target(&app, &diagnostic)));
        assert!(app.source_edit_blocked());
        app.close_diagnostic_fix(&context);
        assert!(!app.diagnostic_fix.visible());
        assert!(!app.diagnostic_fix.busy());
        assert_eq!(app.document().key(), key);
        assert_eq!(app.document().source(), &original);
        assert_eq!(app.tabs.ids().collect::<Vec<_>>(), tabs);
        assert_eq!(app.preview.editor_diagnostics, vec![diagnostic]);
    }

    #[test]
    fn asynchronous_workspace_switch_waits_for_the_diagnostic_review_to_close() {
        let directory = tempfile::tempdir().unwrap();
        let base = directory.path().canonicalize().unwrap();
        let old = base.join("old-workspace");
        let selected = base.join("selected-workspace");
        fs::create_dir(&old).unwrap();
        fs::create_dir(&selected).unwrap();
        let context = egui::Context::default();
        let (mut app, diagnostic) = fixture(&context, &old, DocumentKind::Typst);
        let key = app.document().key();
        let tabs = app.tabs.ids().collect::<Vec<_>>();
        let original = app.document().source().clone();
        app.diagnostic_fix.dialog = Some(dialog(target(&app, &diagnostic)));

        app.request_workspace_switch(
            crate::app::workspace_switch::Destination::Local(selected.clone()),
            &context,
        );
        for _ in 0..3 {
            app.advance_workspace_switch(&context);
            assert_eq!(app.workspace_root, old);
            assert_eq!(app.tabs.ids().collect::<Vec<_>>(), tabs);
            assert_eq!(app.document().key(), key);
            assert_eq!(app.document().source(), &original);
            assert!(app.diagnostic_fix.visible());
            assert!(app.document_workflow.modal().is_none());
        }

        app.close_diagnostic_fix(&context);
        app.advance_workspace_switch(&context);

        assert_eq!(app.workspace_root, selected);
        assert!(app.tabs.is_empty());
        assert!(!app.diagnostic_fix.visible());
        assert_eq!(fs::read_to_string(old.join("main.typ")).unwrap(), original);
    }

    #[test]
    fn foreign_missing_location_and_identifiably_stale_diagnostics_cannot_start_a_fix() {
        let directory = tempfile::tempdir().unwrap();
        let context = egui::Context::default();
        let (app, diagnostic) = fixture(&context, directory.path(), DocumentKind::Typst);
        for source in [
            DiagnosticSource::Global,
            DiagnosticSource::File(directory.path().join("other.typ")),
        ] {
            let mut wrong = diagnostic.clone();
            wrong.source = source;
            assert!(
                app.bind_diagnostic_fix(&wrong, CCursorRange::default())
                    .is_none()
            );
        }
        let mut missing = diagnostic.clone();
        missing.location = None;
        assert!(
            app.bind_diagnostic_fix(&missing, CCursorRange::default())
                .is_none()
        );
        let mut stale = diagnostic;
        let mut correction =
            crate::diagnostics::DiagnosticCorrection::new(0..1, "x".into(), None, None, vec![]);
        let key = app.document().key();
        correction.document = Some(DocumentKey::new(
            key.owner,
            key.epoch,
            key.revision.wrapping_sub(1),
        ));
        stale.correction = Some(correction);
        assert!(
            app.bind_diagnostic_fix(&stale, CCursorRange::default())
                .is_none()
        );
    }

    #[test]
    fn reply_after_cancel_or_source_change_cannot_publish_or_edit_a_review() {
        let directory = tempfile::tempdir().unwrap();
        let context = egui::Context::default();
        for cancelled in [true, false] {
            let (mut app, diagnostic) = fixture(&context, directory.path(), DocumentKind::Typst);
            let target = target(&app, &diagnostic);
            let result = review(&target);
            app.diagnostic_fix.dialog = Some(Dialog {
                id: 1,
                target,
                state: ReviewState::Preparing,
            });
            app.diagnostic_fix.request = RequestAdmission::Running(AdmittedRequest {
                id: 1,
                cancel: Arc::default(),
            });
            if cancelled {
                app.close_diagnostic_fix(&context);
            } else {
                app.document_mut()
                    .edit(CCursorRange::default(), |source| source.push_str("edited"));
            }
            let key = app.document().key();
            app.diagnostic_fix.accept_reply(
                Reply {
                    id: 1,
                    result: Ok(result),
                },
                Some(key),
            );
            assert!(
                app.diagnostic_fix
                    .dialog
                    .as_ref()
                    .is_none_or(|dialog| dialog.state.review().is_none())
            );
            assert_eq!(app.document().key(), key);
        }
    }

    #[test]
    fn an_unrelated_reply_cannot_retire_the_current_diagnostic_request() {
        let directory = tempfile::tempdir().unwrap();
        let context = egui::Context::default();
        let (mut app, diagnostic) = fixture(&context, directory.path(), DocumentKind::Typst);
        let target = target(&app, &diagnostic);
        let result = review(&target);
        app.diagnostic_fix.dialog = Some(Dialog {
            id: 2,
            target,
            state: ReviewState::Preparing,
        });
        app.diagnostic_fix.request = RequestAdmission::Running(AdmittedRequest {
            id: 2,
            cancel: Arc::default(),
        });
        app.diagnostic_fix.accept_reply(
            Reply {
                id: 1,
                result: Ok(result),
            },
            Some(app.document().key()),
        );
        assert_eq!(app.diagnostic_fix.request.id(), Some(2));
        assert!(
            app.diagnostic_fix
                .dialog
                .as_ref()
                .unwrap()
                .state
                .review()
                .is_none()
        );
        let result = review(&app.diagnostic_fix.dialog.as_ref().unwrap().target);
        app.diagnostic_fix.accept_reply(
            Reply {
                id: 2,
                result: Ok(result),
            },
            Some(app.document().key()),
        );
        assert!(matches!(
            app.diagnostic_fix.dialog.as_ref().unwrap().state,
            ReviewState::Ready(_)
        ));
        assert_eq!(app.diagnostic_fix.request.id(), None);
    }

    #[test]
    fn source_menu_targets_the_nearest_diagnostic_on_the_clicked_line() {
        let directory = tempfile::tempdir().unwrap();
        let context = egui::Context::default();
        let (mut app, diagnostic) = fixture(&context, directory.path(), DocumentKind::Typst);
        let mut farther = diagnostic.clone();
        farther.location.as_mut().unwrap().column = 1;
        farther.message = "another diagnostic".into();
        app.preview.editor_diagnostics = vec![farther, diagnostic.clone()];
        app.prepare_editor_source_data();
        let character = "// α context\r\n#text(fill: ".chars().count();
        let source = preview_jump::SourceLocation {
            key: app.document().key(),
            char_index: character,
        };
        assert_eq!(
            app.diagnostic_fix_at_source(source).unwrap().diagnostic,
            diagnostic
        );
        assert!(
            app.diagnostic_fix_at_source(preview_jump::SourceLocation {
                char_index: 0,
                ..source
            })
            .is_none()
        );
    }

    #[test]
    fn closing_a_document_window_releases_its_cached_diagnostic_snapshot() {
        let directory = tempfile::tempdir().unwrap();
        let context = egui::Context::default();
        let (mut app, diagnostic) = fixture(&context, directory.path(), DocumentKind::Typst);
        app.preview.editor_diagnostics = vec![diagnostic];
        app.prepare_editor_source_data();
        let selected = app
            .diagnostic_fix_at_source(preview_jump::SourceLocation {
                key: app.document().key(),
                char_index: "// α context\r\n#text(fill: ".chars().count(),
            })
            .unwrap();
        let retired = Arc::downgrade(&selected);
        drop(selected);
        app.lifecycle = DocumentLifecycle::Active;
        app.finish_window_close();
        assert!(retired.upgrade().is_none());
        assert!(!app.diagnostic_fix.visible());
    }

    #[test]
    fn source_menu_cache_reuses_targets_and_invalidates_for_diagnostics_location_preview_and_document()
     {
        let directory = tempfile::tempdir().unwrap();
        let context = egui::Context::default();
        let (mut app, diagnostic) = fixture(&context, directory.path(), DocumentKind::Typst);
        let original = app.tabs.active_id().unwrap();
        app.lifecycle = DocumentLifecycle::Active;
        app.new_tab(&context);
        app.lifecycle = DocumentLifecycle::Dormant;
        let second = app.tabs.active_id().unwrap();
        app.activate_tab(original, &context);
        app.prepare_editor_source_data();
        app.preview.editor_diagnostics = vec![diagnostic.clone(); 2_000];
        let mut source = preview_jump::SourceLocation {
            key: app.document().key(),
            char_index: "// α context\r\n#text(fill: ".chars().count(),
        };
        let first = app.diagnostic_fix_at_source(source).unwrap();
        assert_eq!(app.diagnostic_fix.source_cache.borrow().rebuilds, 1);
        for _ in 0..64 {
            assert!(Arc::ptr_eq(
                &first,
                &app.diagnostic_fix_at_source(source).unwrap()
            ));
        }
        assert_eq!(app.diagnostic_fix.source_cache.borrow().rebuilds, 1);

        app.preview.editor_diagnostics.clear();
        app.diagnostic_regions.generation += 1;
        for _ in 0..16 {
            assert!(app.diagnostic_fix_at_source(source).is_none());
        }
        assert_eq!(
            app.diagnostic_fix.source_cache.borrow().rebuilds,
            2,
            "negative lookups are cached too"
        );

        let mut refreshed = diagnostic.clone();
        refreshed.message = "refreshed diagnostic".into();
        app.preview.editor_diagnostics = vec![refreshed.clone()];
        app.diagnostic_regions.generation += 1;
        let updated = app.diagnostic_fix_at_source(source).unwrap();
        assert!(!Arc::ptr_eq(&first, &updated));
        assert_eq!(updated.diagnostic, refreshed);
        assert_eq!(app.diagnostic_fix.source_cache.borrow().rebuilds, 3);

        source.char_index += 1;
        app.diagnostic_fix_at_source(source).unwrap();
        assert_eq!(app.diagnostic_fix.source_cache.borrow().rebuilds, 4);

        app.select_preview_tab(second, &context);
        assert_eq!(
            source.key,
            app.document().key(),
            "preview ownership changes without changing active source"
        );
        app.preview.editor_diagnostics = vec![diagnostic];
        app.diagnostic_fix_at_source(source).unwrap();
        assert_eq!(app.diagnostic_fix.source_cache.borrow().rebuilds, 5);

        app.document_mut()
            .edit(CCursorRange::default(), |source| source.push_str("changed"));
        app.update_diagnostic_fix();
        assert!(
            app.diagnostic_fix.source_cache.borrow().target.is_none(),
            "retired source snapshots are released without reopening a menu"
        );
        assert!(app.diagnostic_fix_at_source(source).is_none());
        source.key = app.document().key();
        app.prepare_editor_source_data();
        let current = app.diagnostic_fix_at_source(source).unwrap();
        assert_eq!(current.snapshot.key(), source.key);
        assert_eq!(app.diagnostic_fix.source_cache.borrow().rebuilds, 6);
    }

    #[test]
    fn source_context_menu_offers_the_bound_fix_without_requiring_a_selection() {
        use egui_kittest::{
            Harness,
            kittest::{NodeT as _, Queryable as _},
        };
        let directory = tempfile::tempdir().unwrap();
        let context = egui::Context::default();
        let (app, diagnostic) = fixture(&context, directory.path(), DocumentKind::Typst);
        let target = target(&app, &diagnostic);
        let state = (None::<BoundDiagnostic>, None::<EditorMenuAction>);
        let mut harness = Harness::builder()
            .with_size(Vec2::new(700.0, 650.0))
            .build_ui_state(
                |ui, state: &mut (Option<BoundDiagnostic>, Option<EditorMenuAction>)| {
                    show_editor_context_menu_ui(
                        ui,
                        EditorContextMenuOptions {
                            can_undo: false,
                            can_redo: false,
                            has_selection: false,
                            can_format: true,
                            can_sync_preview: false,
                            link: None,
                            table: None,
                            math: None,
                            liquid_selection: None,
                            diagnostic_fix: state.0.as_ref(),
                        },
                        &AppSettings::default().effective_shortcuts(),
                        &mut state.1,
                    );
                },
                state,
            );
        harness.run_steps(3);
        assert!(
            harness
                .get_by_label_contains(MENU_LABEL)
                .accesskit_node()
                .is_disabled()
        );
        harness.state_mut().0 = Some(target.clone());
        harness.run_steps(3);
        assert!(
            !harness
                .get_by_label_contains(MENU_LABEL)
                .accesskit_node()
                .is_disabled()
        );
        harness.get_by_label_contains(MENU_LABEL).click();
        harness.run_steps(3);
        assert_eq!(
            harness.state().1,
            Some(EditorMenuAction::SuggestFix(Box::new(target)))
        );
    }

    #[test]
    fn review_accept_is_enabled_only_for_a_ready_current_diff_and_cancel_stays_available() {
        use egui_kittest::{
            Harness,
            kittest::{NodeT as _, Queryable as _},
        };
        let directory = tempfile::tempdir().unwrap();
        let context = egui::Context::default();
        let (app, diagnostic) = fixture(&context, directory.path(), DocumentKind::Typst);
        let state = (
            dialog(target(&app, &diagnostic)),
            false,
            true,
            AppSettings::default(),
        );
        let mut fonts_ready = false;
        let mut harness = Harness::builder()
            .with_size(Vec2::new(960.0, 640.0))
            .build_ui_state(
                move |ui, state: &mut (Dialog, bool, bool, AppSettings)| {
                    if !fonts_ready {
                        theme::configure_test_fonts(ui.ctx());
                        fonts_ready = true;
                        return;
                    }
                    review_ui(ui, &state.0, state.1, state.2, 640.0, &state.3);
                },
                state,
            );
        harness.run_steps(3);
        assert!(
            !harness
                .get_by_label("Accept")
                .accesskit_node()
                .is_disabled()
        );
        for (busy, current) in [(true, true), (false, false)] {
            harness.state_mut().1 = busy;
            harness.state_mut().2 = current;
            harness.run_steps(3);
            assert!(
                harness
                    .get_by_label("Accept")
                    .accesskit_node()
                    .is_disabled()
            );
            assert!(
                !harness
                    .get_by_label("Cancel")
                    .accesskit_node()
                    .is_disabled()
            );
        }
        harness.state_mut().2 = true;
        harness
            .state_mut()
            .0
            .state
            .fail("No change suggested.".into());
        harness.run_steps(3);
        assert!(
            harness
                .get_by_label("Accept")
                .accesskit_node()
                .is_disabled()
        );
    }
}
