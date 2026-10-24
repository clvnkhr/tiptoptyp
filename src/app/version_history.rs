//! Native revisions are read and compared off-thread; restoring uses the normal
//! guarded save path after preserving the current (possibly unsaved) buffer.
use super::*;
use crate::{
    file_versions::VersionInfo, git::editor::document_diff::DocumentDiff, worker::ExclusiveJob,
};
use std::sync::atomic::{AtomicBool, Ordering};

#[path = "version_history_pdf.rs"]
mod pdf;
#[cfg(test)]
#[path = "version_history_tests.rs"]
mod tests;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Choice {
    Current,
    Saved(usize),
}
impl Choice {
    fn rank(self) -> usize {
        match self {
            Self::Current => 0,
            Self::Saved(i) => i + 1,
        }
    }
}
fn ordered(mut pair: [Choice; 2]) -> [Choice; 2] {
    if pair[0].rank() < pair[1].rank() {
        pair.swap(0, 1);
    }
    pair
}
struct Inventory {
    initial: usize,
    versions: Vec<VersionInfo>,
    modified: Option<SystemTime>,
}
impl crate::worker::OperationSummary for Inventory {
    fn completion_summary(&self) -> String {
        "Local versions loaded".into()
    }
}
struct Comparison {
    choices: [Choice; 2],
    sources: [Arc<str>; 2],
    diff: DocumentDiff,
}
impl crate::worker::OperationSummary for Comparison {
    fn completion_summary(&self) -> String {
        "Version comparison loaded".into()
    }
}
struct RestoreReady {
    source: Arc<str>,
}
impl crate::worker::OperationSummary for RestoreReady {
    fn completion_summary(&self) -> String {
        "Current document backed up before restore".into()
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Action {
    Close,
    Select(usize, Choice),
    Pdf(usize),
    Restore(usize),
}

pub(super) struct Dialog {
    key: DocumentKey,
    path: PathBuf,
    current: Arc<str>,
    unsaved: bool,
    project_root: PathBuf,
    engine: Option<crate::compiler::EngineConfig>,
    inventory: Option<Inventory>,
    list_job: ExclusiveJob<Inventory>,
    choices: [Choice; 2],
    comparison: Option<Comparison>,
    comparison_job: ExclusiveJob<Comparison>,
    comparison_requested: Option<[Choice; 2]>,
    backup_job: ExclusiveJob<RestoreReady>,
    pdf_job: ExclusiveJob<pdf::Preview>,
    pdf_preview: Option<pdf::Preview>,
    live_pdfium_view: Option<pdfium_view::PdfiumView>,
    cancel: Arc<AtomicBool>,
    error: Option<String>,
    notice: Option<String>,
}
impl Drop for Dialog {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Release);
    }
}
impl Dialog {
    pub(super) fn activity(&self) -> crate::activity::Activity {
        history_activity(
            [
                self.list_job.activity(),
                self.comparison_job.activity(),
                self.backup_job.activity(),
                self.pdf_job.activity(),
            ],
            self.error.as_deref(),
        )
    }

    fn new(
        key: DocumentKey,
        path: PathBuf,
        current: String,
        unsaved: bool,
        project_root: PathBuf,
        engine: Option<crate::compiler::EngineConfig>,
    ) -> Self {
        Self {
            key,
            path,
            current: current.into(),
            unsaved,
            project_root,
            engine,
            inventory: None,
            list_job: Default::default(),
            choices: [Choice::Current; 2],
            comparison: None,
            comparison_job: Default::default(),
            comparison_requested: None,
            backup_job: Default::default(),
            pdf_job: Default::default(),
            pdf_preview: None,
            live_pdfium_view: None,
            cancel: Arc::new(AtomicBool::new(false)),
            error: None,
            notice: None,
        }
    }
    #[cfg(all(feature = "desktop-ui-tests", target_os = "macos"))]
    pub(super) fn inspect(&self, viewer: &pdfium_view::PdfiumView) -> serde_json::Value {
        let comparison = self
            .comparison
            .as_ref()
            .filter(|c| c.choices == self.choices);
        let can_restore = [0, 1].map(|side| {
            self.loaded()
                && !self.busy()
                && comparison.is_some_and(|c| c.sources[side] != self.current)
        });
        serde_json::json!({
            "visible": true,
            "loading": self.list_job.is_running() || self.comparison_job.is_running(),
            "count": self.inventory.as_ref().map_or(0, |i| i.versions.len()),
            "left": comparison.map(|c| crate::desktop_test::fingerprint(&c.sources[0])),
            "right": comparison.map(|c| crate::desktop_test::fingerprint(&c.sources[1])),
            "selected": self.choices.map(|c| match c { Choice::Current => None, Choice::Saved(index) => Some(index) }),
            "can_restore": can_restore,
            "rendering": self.pdf_job.is_running(),
            "preview": self.pdf_preview.as_ref().map(|preview| serde_json::json!({
                "side": preview.side,
                "source_fingerprint": crate::desktop_test::fingerprint(&preview.source),
                "pages": if viewer.ready_for(Some(preview.bytes())) { viewer.controls_snapshot().count } else { 0 },
            })),
            "error": self.error,
        })
    }
    pub(super) fn preview_pdf(&self) -> Option<(&Path, &Arc<[u8]>)> {
        self.pdf_preview
            .as_ref()
            .map(|preview| (preview.path(), preview.bytes()))
    }
    fn start(&mut self, context: &egui::Context) {
        let path = self.path.clone();
        let current = self.current.clone();
        if let Err(error) =
            self.list_job
                .start_and_repaint("Read local versions", context, move || {
                    let versions = crate::file_versions::list(&path)?;
                    let initial = usize::from(
                        versions.len() > 1
                            && versions
                                .first()
                                .and_then(|v| crate::file_versions::read(&path, &v.id).ok())
                                .is_some_and(|source| source == current.as_ref()),
                    );
                    Ok(Inventory {
                        initial,
                        versions,
                        modified: fs::metadata(path).ok().and_then(|m| m.modified().ok()),
                    })
                })
        {
            self.error = Some(error);
        }
    }
    fn busy(&self) -> bool {
        self.backup_job.is_running() || self.pdf_job.is_running()
    }
    fn loaded(&self) -> bool {
        self.comparison
            .as_ref()
            .is_some_and(|c| c.choices == self.choices)
            && !self.comparison_job.is_running()
    }
    fn poll(&mut self, context: &egui::Context) -> Option<Arc<str>> {
        match self.list_job.poll() {
            LatestJobPoll::Ready(inventory) => {
                if !inventory.versions.is_empty() {
                    self.choices = [Choice::Saved(inventory.initial), Choice::Current];
                }
                self.inventory = Some(inventory);
                self.request_comparison(context);
            }
            LatestJobPoll::Failed(e) => self.error = Some(e),
            _ => {}
        }
        match self.comparison_job.poll() {
            LatestJobPoll::Ready(comparison) => {
                self.comparison_requested = None;
                let current = comparison.choices == self.choices;
                self.comparison = Some(comparison);
                if !current {
                    self.request_comparison(context);
                }
            }
            LatestJobPoll::Failed(e) => {
                let obsolete = self
                    .comparison_requested
                    .take()
                    .is_some_and(|requested| requested != self.choices);
                if obsolete {
                    self.request_comparison(context);
                } else {
                    self.error = Some(e);
                }
            }
            _ => {}
        }
        match self.pdf_job.poll() {
            LatestJobPoll::Ready(preview) => {
                self.notice = Some(format!("Preview: {}", preview.label));
                self.pdf_preview = Some(preview);
            }
            LatestJobPoll::Failed(e) => self.error = Some(e),
            _ => {}
        }
        match self.backup_job.poll() {
            LatestJobPoll::Ready(ready) => return Some(ready.source),
            LatestJobPoll::Failed(e) => {
                self.error = Some(format!(
                    "Restore cancelled: could not back up the current document. {e}"
                ))
            }
            _ => {}
        }
        None
    }
    fn request_comparison(&mut self, context: &egui::Context) {
        if self.comparison_job.is_running() {
            return;
        }
        let Some(inventory) = &self.inventory else {
            return;
        };
        if inventory.versions.is_empty() {
            return;
        }
        let choices = self.choices;
        let ids = choices.map(|choice| match choice {
            Choice::Current => None,
            Choice::Saved(index) => Some(inventory.versions[index].id.clone()),
        });
        let path = self.path.clone();
        let current = self.current.clone();
        self.comparison_requested = Some(choices);
        if let Err(error) =
            self.comparison_job
                .start_and_repaint("Compare local versions", context, move || {
                    let load = |id: &Option<String>| -> Result<Arc<str>, String> {
                        match id {
                            Some(id) => crate::file_versions::read(&path, id).map(Arc::from),
                            None => Ok(current.clone()),
                        }
                    };
                    let left = load(&ids[0])?;
                    let right = if ids[0] == ids[1] {
                        left.clone()
                    } else {
                        load(&ids[1])?
                    };
                    let diff = DocumentDiff::new(left.to_string(), &right);
                    Ok(Comparison {
                        choices,
                        sources: [left, right],
                        diff,
                    })
                })
        {
            self.error = Some(error);
        }
    }
    fn choose(&mut self, side: usize, choice: Choice, context: &egui::Context) {
        self.choices[side] = choice;
        self.choices = ordered(self.choices);
        self.error = None;
        self.request_comparison(context);
    }
    fn label(&self, choice: Choice) -> String {
        match choice {
            Choice::Current if self.unsaved => "Current edits · Not saved".into(),
            Choice::Current => self
                .inventory
                .as_ref()
                .and_then(|i| i.modified)
                .map_or_else(
                    || "Current document".into(),
                    |date| format!("Current · {}", timestamp(date)),
                ),
            Choice::Saved(index) => format!(
                "Saved · {}",
                timestamp(self.inventory.as_ref().unwrap().versions[index].saved_at)
            ),
        }
    }
}

impl EditorApp {
    pub(super) fn begin_version_history(&mut self, context: &egui::Context) {
        if self.document_flow_busy()
            || self.save_job.is_running()
            || !self.native_command_enabled(AppCommand::RevertTo)
        {
            return;
        }
        let Some(path) = self.document().path().clone() else {
            return;
        };
        let source = match self.canonical_document_source() {
            Ok(s) => s,
            Err(e) => {
                self.show_file_error(e);
                return;
            }
        };
        let engine = match self.document().kind() {
            DocumentKind::Typst => {
                Some(self.configured_build_engine(crate::language_support::BuildEngineKind::Typst))
            }
            DocumentKind::Tex if self.settings.tex.build_enabled => {
                Some(self.configured_build_engine(crate::language_support::BuildEngineKind::Tex))
            }
            _ => None,
        };
        let mut dialog = Dialog::new(
            self.document().key(),
            path,
            source,
            self.document().is_dirty(),
            self.project_root(),
            engine,
        );
        dialog.start(context);
        self.version_history = Some(dialog);
        self.view_mode = ViewMode::Split;
        if self.preview_source_popped_out() {
            self.restore_preview_window(context);
        }
        self.close_document_diff();
        let editor = source_editor_id(context);
        context.memory_mut(|memory| memory.surrender_focus(editor));
        self.editor_completion = None;
        self.clear_editor_hover();
        self.clear_asset_hover();
        self.close_app_popup();
        self.find_bar.close();
        ChildViewHost::close(context, find_bar::VIEWPORT_SALT);
        context.request_repaint();
    }
    pub(super) fn close_version_history(&mut self, context: &egui::Context) {
        if let Some(mut dialog) = self.version_history.take() {
            if dialog.pdf_preview.is_some() {
                self.pdfium_preview = dialog.live_pdfium_view.take().unwrap_or_default();
                self.preview_controls.close_for_version_history();
            }
            self.focus_source_editor(context);
        }
    }
    pub(super) fn poll_version_history(&mut self, context: &egui::Context) {
        let source_locked = self.overleaf_source_locked();
        let Some(dialog) = self.version_history.as_mut() else {
            return;
        };
        let had_preview = dialog.pdf_preview.is_some();
        let restore = dialog.poll(context);
        if !had_preview && dialog.pdf_preview.is_some() {
            dialog.live_pdfium_view = Some(std::mem::take(&mut self.pdfium_preview));
            self.preview_controls
                .select(preview_controls::Target::Source);
            self.preview_controls.close_for_version_history();
        }
        if let Some(source) = restore {
            if source_locked {
                self.version_history.as_mut().unwrap().error =
                    Some("Wait for Overleaf to finish syncing before restoring a version.".into());
                return;
            }
            let key = dialog.key;
            let path = dialog.path.clone();
            if self.document().key() != key
                || self.document().path().as_ref() != Some(&path)
                || self.save_job.is_running()
            {
                self.version_history.as_mut().unwrap().error = Some("The document changed while the version was loading. Close and reopen Revert To.".into());
                return;
            }
            let replacement = self.document().project_canonical_change(
                key,
                tiptoptyp_core::text::AppliedTextEdits {
                    text: source.to_string(),
                    mapped_offsets: [ScalarOffset::new(0); 2],
                },
            );
            match replacement {
                Ok(replacement) => {
                    let cursor = self.editor_snapshot(context).cursor;
                    self.close_version_history(context);
                    self.document_mut().edit(cursor, |s| *s = replacement.text);
                    self.close_document_diff();
                    self.find_bar.search.clear();
                    self.mark_edited();
                    self.pending_editor_selection = Some(EditorSelection::Focus(0..0));
                    self.save_to_with_intent(path, SaveIntent::Explicit, context);
                }
                Err(error) => {
                    self.version_history.as_mut().unwrap().error = Some(error.to_string())
                }
            }
        }
    }
    pub(super) fn show_version_history_panel(&mut self, ui: &mut egui::Ui) {
        // Occupy the same bounded source pane as the docked Git diff. Keeping
        // this in the root viewport also leaves its sibling preview visible.
        ui.set_min_width(ui.available_width());
        let bounds = ui.available_rect_before_wrap();
        let mut action = None;
        ui.scope_builder(egui::UiBuilder::new().max_rect(bounds), |ui| {
            ui.set_clip_rect(ui.clip_rect().intersect(bounds));
            action = show(ui, self.version_history.as_ref().unwrap(), &self.settings);
        });
        ui.allocate_rect(bounds, Sense::hover());
        #[cfg(all(feature = "desktop-ui-tests", target_os = "macos"))]
        crate::desktop_test::observe(
            "versions.panel",
            &ui.interact(bounds, ui.id().with("versions-panel"), Sense::hover()),
        );
        let context = ui.ctx();
        match action {
            Some(Action::Close) => self.close_version_history(context),
            Some(Action::Select(side, choice)) => self
                .version_history
                .as_mut()
                .unwrap()
                .choose(side, choice, context),
            Some(Action::Restore(side)) => {
                let dialog = self.version_history.as_mut().unwrap();
                if !dialog.loaded() || dialog.busy() {
                    return;
                }
                let source = dialog.comparison.as_ref().unwrap().sources[side].clone();
                let current = dialog.current.clone();
                let path = dialog.path.clone();
                if source == current {
                    return;
                }
                dialog.error = None;
                if let Err(e) = dialog.backup_job.start_and_repaint(
                    "Back up current version",
                    context,
                    move || {
                        crate::file_versions::checkpoint_contents(&path, &current)?;
                        Ok(RestoreReady { source })
                    },
                ) {
                    dialog.error = Some(e);
                }
            }
            Some(Action::Pdf(side)) => self.render_version_pdf(context, side),
            None => {}
        }
    }
    fn render_version_pdf(&mut self, context: &egui::Context, side: usize) {
        let dialog = self.version_history.as_mut().unwrap();
        if !dialog.loaded() || dialog.busy() || dialog.engine.is_none() {
            return;
        }
        let source = dialog.comparison.as_ref().unwrap().sources[side].clone();
        let label = dialog.label(dialog.choices[side]);
        let request = crate::version_render::Request {
            original: dialog.path.clone(),
            source: source.to_string(),
            project_root: dialog.project_root.clone(),
            engine: dialog.engine.clone().unwrap(),
        };
        let cancel = dialog.cancel.clone();
        dialog.error = None;
        if let Err(error) =
            dialog
                .pdf_job
                .start_and_repaint("Render version PDF", context, move || {
                    pdf::render(request, side, source, label, &cancel)
                })
        {
            dialog.error = Some(error);
        }
    }
    pub(super) fn version_pdf_active(&self) -> bool {
        self.version_history
            .as_ref()
            .is_some_and(|history| history.preview_pdf().is_some())
    }
    pub(super) fn handle_version_history_shortcuts(
        &mut self,
        context: &egui::Context,
        viewport: egui::ViewportId,
    ) {
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
        if viewport == context.viewport_id() {
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
                self.close_version_history(context);
            } else if let Some(command) = context.input_mut(|input| {
                consume_shortcut(input, &shortcuts, |command| {
                    matches!(
                        command,
                        AppCommand::Find | AppCommand::Copy | AppCommand::SelectAll
                    )
                })
            }) {
                self.execute_app_command(command, context, None);
            }
        }
    }
}

fn show(ui: &mut egui::Ui, dialog: &Dialog, settings: &AppSettings) -> Option<Action> {
    theme::apply_compact_tool_spacing(ui);
    let mut action = None;
    ui.horizontal(|ui| {
        ui.strong("Revert To");
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
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            let close = ui.button("Close");
            #[cfg(all(feature = "desktop-ui-tests", target_os = "macos"))]
            crate::desktop_test::observe("versions.close", &close);
            if close.clicked() {
                action = Some(Action::Close);
            }
        });
    });
    ui.separator();
    if dialog.list_job.is_running() {
        ui.spinner();
        ui.label("Loading saved versions…");
    }
    if dialog
        .inventory
        .as_ref()
        .is_some_and(|i| i.versions.is_empty())
    {
        ui.label("No saved versions yet.");
    }
    if let Some(inventory) = &dialog.inventory
        && !inventory.versions.is_empty()
    {
        ui.columns(2, |columns| {
            for (side, ui) in columns.iter_mut().enumerate() {
                ui.horizontal(|ui| {
                    ui.strong(if side == 0 { "Older" } else { "Newer" });
                    let selector = egui::ComboBox::from_id_salt(("version-choice", side))
                        .selected_text(dialog.label(dialog.choices[side]))
                        .wrap_mode(egui::TextWrapMode::Truncate)
                        .width((ui.available_width() - 12.0).max(1.0))
                        .show_ui(ui, |ui| {
                            for choice in std::iter::once(Choice::Current)
                                .chain((0..inventory.versions.len()).map(Choice::Saved))
                            {
                                let row = ui.selectable_label(
                                    dialog.choices[side] == choice,
                                    dialog.label(choice),
                                );
                                #[cfg(all(feature = "desktop-ui-tests", target_os = "macos"))]
                                {
                                    let index = match choice {
                                        Choice::Current => "current".into(),
                                        Choice::Saved(index) => index.to_string(),
                                    };
                                    crate::desktop_test::observe(
                                        &format!(
                                            "versions.choice.{}.{index}",
                                            if side == 0 { "older" } else { "newer" }
                                        ),
                                        &row,
                                    );
                                }
                                if row.clicked() {
                                    action = Some(Action::Select(side, choice));
                                }
                            }
                        });
                    #[cfg(all(feature = "desktop-ui-tests", target_os = "macos"))]
                    crate::desktop_test::observe(
                        if side == 0 {
                            "versions.select.older"
                        } else {
                            "versions.select.newer"
                        },
                        &selector.response,
                    );
                    #[cfg(not(all(feature = "desktop-ui-tests", target_os = "macos")))]
                    let _ = selector;
                });
                ui.horizontal_wrapped(|ui| {
                    let pdf = ui.add_enabled(
                        dialog.loaded() && !dialog.busy() && dialog.engine.is_some(),
                        egui::Button::new("Preview PDF").selected(
                            dialog.pdf_preview.as_ref().is_some_and(|preview| {
                                preview.side == side
                                    && dialog.comparison.as_ref().is_some_and(|comparison| {
                                        preview.source == comparison.sources[side]
                                    })
                            }),
                        ),
                    );
                    let restore_enabled = dialog.loaded()
                        && !dialog.busy()
                        && dialog
                            .comparison
                            .as_ref()
                            .is_some_and(|c| c.sources[side] != dialog.current);
                    let restore =
                        ui.add_enabled(restore_enabled, egui::Button::new("Restore version"));
                    #[cfg(all(feature = "desktop-ui-tests", target_os = "macos"))]
                    {
                        crate::desktop_test::observe(
                            if side == 0 {
                                "versions.older.pdf"
                            } else {
                                "versions.newer.pdf"
                            },
                            &pdf,
                        );
                        crate::desktop_test::observe(
                            if side == 0 {
                                "versions.older.restore"
                            } else {
                                "versions.newer.restore"
                            },
                            &restore,
                        );
                    }
                    if pdf.clicked() {
                        action = Some(Action::Pdf(side));
                    }
                    if restore.clicked() {
                        action = Some(Action::Restore(side));
                    }
                });
            }
        });
        ui.separator();
    }
    if dialog.backup_job.is_running() {
        ui.horizontal(|ui| {
            ui.spinner();
            ui.label("Backing up current document…");
        });
    }
    if dialog.pdf_job.is_running() {
        ui.horizontal(|ui| {
            ui.spinner();
            ui.label("Rendering version PDF…");
        });
    }
    if let Some(error) = &dialog.error {
        egui::ScrollArea::vertical()
            .id_salt("version-error")
            .max_height(80.0)
            .show(ui, |ui| {
                ui.colored_label(theme::palette(ui.ctx()).error, error);
            });
    }
    if let Some(notice) = &dialog.notice {
        ui.label(notice);
    }
    if dialog.comparison_job.is_running() {
        ui.label("Loading comparison…");
    }
    if let Some(comparison) = &dialog.comparison
        && comparison.choices == dialog.choices
    {
        let id = viewport_scoped_id(ui.ctx(), "version-diff");
        let viewport = crate::git::editor::full_view::show_comparison(
            ui,
            &comparison.diff,
            id,
            &dialog.path,
            settings,
        );
        #[cfg(all(feature = "desktop-ui-tests", target_os = "macos"))]
        crate::desktop_test::observe(
            "versions.diff",
            &ui.interact(viewport, id.with("observed"), Sense::hover()),
        );
        #[cfg(not(all(feature = "desktop-ui-tests", target_os = "macos")))]
        let _ = viewport;
    }
    action
}

fn history_activity(
    jobs: [crate::activity::Activity; 4],
    error: Option<&str>,
) -> crate::activity::Activity {
    use crate::activity::Activity;
    if jobs.iter().any(|job| matches!(job, Activity::Running)) {
        Activity::Running
    } else if let Some(error) = error {
        Activity::Failed(error.into())
    } else {
        jobs.into_iter()
            .find(|job| matches!(job, Activity::Failed(_)))
            .unwrap_or(Activity::Idle)
    }
}

// Gregorian civil date from Unix days; explicit UTC is deterministic and avoids
// locale-dependent ambiguous dates without introducing another date library.
fn timestamp(time: SystemTime) -> String {
    let seconds = time
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let days = (seconds / 86400) as i64 + 719468;
    let era = days / 146097;
    let doe = days - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let mut year = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = mp + if mp < 10 { 3 } else { -9 };
    year += i64::from(month <= 2);
    format!(
        "{year:04}-{month:02}-{day:02} {:02}:{:02}:{:02} UTC",
        seconds / 3600 % 24,
        seconds / 60 % 60,
        seconds % 60
    )
}

#[cfg(test)]
mod activity_tests {
    use super::*;
    use crate::activity::Activity;

    #[test]
    fn every_version_operation_remains_busy_before_previous_failures() {
        for running in 0..4 {
            let mut jobs = std::array::from_fn(|_| Activity::Failed("old failure".into()));
            jobs[running] = Activity::Running;
            assert_eq!(
                history_activity(jobs, Some("previous request failed")),
                Activity::Running
            );
        }
        assert_eq!(
            history_activity(std::array::from_fn(|_| Activity::Idle), None),
            Activity::Idle
        );
        assert_eq!(
            history_activity(
                std::array::from_fn(|_| Activity::Idle),
                Some("restore failed")
            ),
            Activity::Failed("restore failed".into())
        );
        for failed in 0..4 {
            let mut jobs = std::array::from_fn(|_| Activity::Idle);
            jobs[failed] = Activity::Failed("worker failed".into());
            assert_eq!(
                history_activity(jobs, None),
                Activity::Failed("worker failed".into())
            );
        }
    }

    #[test]
    fn reading_version_activity_never_starts_inventory_or_pdf_work() {
        let key = DocumentKey::new(tiptoptyp_core::document::WindowSessionId::new(42), 1, 1);
        let dialog = Dialog::new(
            key,
            "/paper.typ".into(),
            "Source".into(),
            false,
            "/".into(),
            None,
        );
        assert_eq!(dialog.activity(), Activity::Idle);
        assert_eq!(dialog.activity(), Activity::Idle);
        assert!(dialog.inventory.is_none());
        assert!(dialog.comparison.is_none());
        assert!(!dialog.list_job.is_running());
        assert!(!dialog.comparison_job.is_running());
        assert!(!dialog.backup_job.is_running());
        assert!(!dialog.pdf_job.is_running());
    }
}
