//! Explicit, selection-bound Liquid checks. Background Harper results stay separate.
use super::*;
use crate::git::editor::{document_diff::DocumentDiff, full_view};
use crate::liquid_grammar::{ChunkCorrection, SourceEdit};
use crate::liquid_runtime::{Client, Config};
use std::sync::atomic::{AtomicBool, Ordering};

pub(super) const SALT: &str = "tiptoptyp-liquid-review";
pub(super) const MENU_LABEL: &str = "Check selection with Liquid";

#[derive(Debug, Clone)]
pub(super) struct BoundSelection {
    snapshot: crate::document::DocumentSnapshot,
    kind: DocumentKind,
    range: Range<usize>,
    cursor: [usize; 2],
}
impl PartialEq for BoundSelection {
    fn eq(&self, other: &Self) -> bool {
        self.snapshot.key() == other.snapshot.key()
            && self.kind == other.kind
            && self.range == other.range
            && self.cursor == other.cursor
            && self.snapshot.source() == other.snapshot.source()
    }
}
impl Eq for BoundSelection {}
impl BoundSelection {
    fn new(
        snapshot: crate::document::DocumentSnapshot,
        kind: DocumentKind,
        cursor: CCursorRange,
    ) -> Option<Self> {
        let sorted = cursor.as_sorted_char_range();
        let range = sorted.start.0..sorted.end.0;
        if !matches!(kind, DocumentKind::Typst | DocumentKind::Tex) || range.is_empty() {
            return None;
        }
        Some(Self {
            snapshot,
            kind,
            range,
            cursor: [cursor.primary.index.0, cursor.secondary.index.0],
        })
    }
    fn current(&self, key: DocumentKey) -> bool {
        self.snapshot.key() == key
    }
}
impl AppPopup {
    pub(super) fn with_liquid_selection(
        mut self,
        snapshot: crate::document::DocumentSnapshot,
        kind: DocumentKind,
        cursor: Option<CCursorRange>,
    ) -> Self {
        if let Self::Editor {
            liquid_selection, ..
        } = &mut self
        {
            *liquid_selection =
                cursor.and_then(|cursor| BoundSelection::new(snapshot, kind, cursor));
        }
        self
    }
}

pub(super) struct State {
    client: Arc<std::sync::Mutex<Client>>,
    job: crate::worker::ExclusiveJob<Reply>,
    next: u64,
    active: Option<u64>,
    dialog: Option<Dialog>,
    progress: Option<crate::worker::LatestReceiver<Progress>>,
    cancel: Arc<AtomicBool>,
}
#[derive(Clone, Copy, Debug)]
struct ChunkStatus {
    passage: usize,
    passages: usize,
    chunk: usize,
    chunks: usize,
    model_tokens: usize,
}
struct Progress {
    id: u64,
    sent: Arc<SentInput>,
    review: Option<Arc<Review>>,
    status: ChunkStatus,
}
struct Reply {
    id: u64,
    sent: Arc<SentInput>,
    result: Result<Review, String>,
}
struct SentInput {
    fragments: Vec<String>,
    display: String,
}
impl SentInput {
    fn new(fragments: Vec<String>) -> Arc<Self> {
        let display = fragments.join("\n\n");
        Arc::new(Self { fragments, display })
    }
}
impl crate::worker::OperationSummary for Reply {
    fn completion_summary(&self) -> String {
        if self.result.is_ok() {
            "Liquid selection check finished."
        } else {
            "Liquid selection check failed."
        }
        .into()
    }
}
impl Default for State {
    fn default() -> Self {
        Self {
            client: crate::liquid_runtime::shared_client(),
            job: Default::default(),
            next: 0,
            active: None,
            dialog: None,
            progress: None,
            cancel: Arc::default(),
        }
    }
}
impl Drop for State {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Release);
    }
}

struct Review {
    edits: Vec<SourceEdit>,
    diff: DocumentDiff,
}
fn prepare_review(target: &BoundSelection, edits: Vec<SourceEdit>) -> Result<Review, String> {
    let before = char_range_slice(target.snapshot.source(), target.range.clone())
        .ok_or("The selected text is no longer available.")?
        .to_owned();
    let mut after = String::with_capacity(before.len());
    let mut ordered: Vec<_> = edits.iter().collect();
    ordered.sort_unstable_by_key(|edit| edit.range.start);
    let mut previous_end = target.range.start;
    let mut local_ranges = Vec::with_capacity(ordered.len());
    for edit in &ordered {
        if edit.range.start < previous_end
            || edit.range.start < target.range.start
            || edit.range.end > target.range.end
            || edit.range.start > edit.range.end
        {
            return Err("Liquid returned an edit outside the selected text.".into());
        }
        let local = edit.range.start - target.range.start..edit.range.end - target.range.start;
        local_ranges.push(local);
        previous_end = edit.range.end;
    }
    let mapped = byte_ranges(&before, &local_ranges)?;
    let mut cursor = 0;
    for (bytes, edit) in mapped.into_iter().zip(ordered) {
        after.push_str(&before[cursor..bytes.start]);
        after.push_str(&edit.replacement);
        cursor = bytes.end;
    }
    after.push_str(&before[cursor..]);
    Ok(Review {
        edits,
        diff: DocumentDiff::new(before, &after),
    })
}

// Convert all scalar endpoints together; applying many corrections near EOF
// must never rescan the entire original document once per correction.
fn byte_ranges(source: &str, ranges: &[Range<usize>]) -> Result<Vec<Range<usize>>, String> {
    let mut endpoints: Vec<_> = ranges
        .iter()
        .flat_map(|range| [range.start, range.end])
        .collect();
    endpoints.sort_unstable();
    endpoints.dedup();
    let mut bytes = Vec::with_capacity(endpoints.len());
    let mut next = 0;
    for (scalar, (byte, _)) in source
        .char_indices()
        .chain(std::iter::once((source.len(), '\0')))
        .enumerate()
    {
        if endpoints.get(next) == Some(&scalar) {
            bytes.push(byte);
            next += 1;
        }
        if next == endpoints.len() {
            break;
        }
    }
    if next != endpoints.len() {
        return Err("Liquid returned an invalid text location.".into());
    }
    ranges
        .iter()
        .map(|range| {
            if range.start > range.end {
                return Err("Liquid returned an invalid text location.".into());
            }
            Ok(bytes[endpoints.binary_search(&range.start).unwrap()]
                ..bytes[endpoints.binary_search(&range.end).unwrap()])
        })
        .collect()
}

struct Dialog {
    id: u64,
    target: BoundSelection,
    review: Option<Arc<Review>>,
    sent: Option<Arc<SentInput>>,
    progress: Option<ChunkStatus>,
    sent_open: bool,
    error: Option<String>,
}
impl State {
    pub(super) fn busy(&self) -> bool {
        self.job.is_running()
    }
    pub(super) fn visible(&self) -> bool {
        self.dialog.is_some()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Action {
    Close,
    Apply,
}

impl EditorApp {
    pub(super) fn begin_liquid_check(&mut self, target: BoundSelection, context: &egui::Context) {
        if self.document_flow_busy() || self.liquid.busy() {
            return;
        }
        if self.tabs.is_empty() || !target.current(self.document().key()) {
            self.show_file_error("The selection changed; select the text and try again.".into());
            return;
        }
        self.close_app_popup();
        self.clear_editor_hover();
        self.clear_asset_hover();
        self.find_bar.close();
        ChildViewHost::close(context, find_bar::VIEWPORT_SALT);
        self.launch_liquid(target, context);
    }

    fn launch_liquid(&mut self, target: BoundSelection, context: &egui::Context) {
        if self.liquid.busy() {
            return;
        }
        self.liquid.next = self.liquid.next.wrapping_add(1);
        let id = self.liquid.next;
        let config = Config::from_settings(&self.settings);
        self.liquid.dialog = Some(Dialog {
            id,
            target: target.clone(),

            review: None,
            sent: None,
            progress: None,
            sent_open: false,
            error: config.as_ref().err().cloned(),
        });
        let Ok(config) = config else {
            context.request_repaint();
            return;
        };
        self.liquid.active = Some(id);
        self.liquid.cancel = Arc::default();
        let cancel = self.liquid.cancel.clone();
        let (updates, progress) = crate::worker::latest_channel();
        self.liquid.progress = Some(progress);
        let repaint = crate::worker::RepaintTarget::current(context);
        let client = Arc::clone(&self.liquid.client);
        let input_options = self.settings.liquid_input.clone();
        let prose_rules = self.settings.tex_prose_rules.clone();
        if let Err(error) =
            self.liquid
                .job
                .start_and_repaint("liquid-selection", context, move || {
                    // Parsing the full source establishes the selected text's syntax
                    // context. Preparation and inference both stay off the UI thread.
                    let selection = crate::liquid_grammar::Selection::with_prose_rules(
                        target.snapshot.source(),
                        target.kind,
                        target.range.clone(),
                        input_options,
                        Some(&prose_rules),
                    )?;
                    let texts: Vec<_> = selection
                        .fragments
                        .iter()
                        .map(|fragment| fragment.text.clone())
                        .collect();
                    let sent = SentInput::new(texts);
                    let result = (|| {
                        let mut worker = client.lock().unwrap_or_else(|poisoned| {
                            let mut guard = poisoned.into_inner();
                            *guard = Client::default();
                            client.clear_poison();
                            guard
                        });
                        let mut chunks = Vec::new();
                        let mut incremental_edits = Vec::new();
                        let mut latest_review = None;
                        let mut last_diff = std::time::Instant::now();
                        let mut dirty_diff = false;
                        worker.correct_stream(
                            &config,
                            &sent.fragments,
                            &cancel,
                            |fragment, chunk| {
                                let correction = ChunkCorrection {
                                    fragment,
                                    range: chunk.byte_range,
                                    corrected: chunk.corrected.to_owned(),
                                };
                                let edits = selection.map_chunk(&correction)?;
                                dirty_diff |= !edits.is_empty();
                                incremental_edits.extend(edits);
                                chunks.push(correction);
                                // Whole-selection comparison work is coalesced; model
                                // chunks and the latest progress mailbox stay bounded.
                                if dirty_diff
                                    && (latest_review.is_none()
                                        || last_diff.elapsed() >= Duration::from_millis(100))
                                {
                                    latest_review = Some(Arc::new(prepare_review(
                                        &target,
                                        incremental_edits.clone(),
                                    )?));
                                    last_diff = std::time::Instant::now();
                                    dirty_diff = false;
                                }
                                updates
                                    .send(Progress {
                                        id,
                                        sent: sent.clone(),
                                        review: latest_review.clone(),
                                        status: ChunkStatus {
                                            passage: fragment + 1,
                                            passages: sent.fragments.len(),
                                            chunk: chunk.completed,
                                            chunks: chunk.total,
                                            model_tokens: chunk.token_count,
                                        },
                                    })
                                    .map_err(|_| "Liquid check cancelled.".to_owned())?;
                                repaint.request_repaint();
                                Ok(())
                            },
                        )?;
                        if cancel.load(Ordering::Acquire) {
                            return Err("Liquid check cancelled.".into());
                        }
                        let edits =
                            selection.map_chunk_corrections(target.snapshot.source(), &chunks)?;
                        prepare_review(&target, edits)
                    })();
                    Ok(Reply { id, sent, result })
                })
        {
            self.liquid.active = None;
            self.liquid.progress = None;
            self.liquid.cancel.store(true, Ordering::Release);
            self.liquid.dialog.as_mut().unwrap().error = Some(error);
        }
    }

    pub(super) fn update_liquid(&mut self) {
        if let Some(progress) = self
            .liquid
            .progress
            .as_ref()
            .and_then(|receiver| receiver.try_recv().ok())
        {
            self.accept_liquid_progress(progress);
        }
        match self.liquid.job.poll() {
            LatestJobPoll::Ready(reply) => {
                self.liquid.active = None;
                self.liquid.progress = None;
                if let Some(dialog) = &mut self.liquid.dialog
                    && dialog.id == reply.id
                    && !self.tabs.is_empty()
                    && dialog
                        .target
                        .current(self.tabs.current_record().document.key())
                {
                    dialog.sent = Some(reply.sent);
                    match reply.result {
                        Ok(review) => {
                            dialog.review = Some(Arc::new(review));
                            dialog.error = None;
                        }
                        Err(error) => {
                            dialog.review = None;
                            dialog.error = Some(error);
                        }
                    }
                }
            }
            LatestJobPoll::Failed(error) => {
                let active = self.liquid.active.take();
                self.liquid.progress = None;
                if let Some(dialog) = &mut self.liquid.dialog
                    && Some(dialog.id) == active
                {
                    dialog.error = Some(error);
                }
            }
            LatestJobPoll::Pending | LatestJobPoll::Idle => {}
        }
        if let Some(dialog) = &mut self.liquid.dialog
            && (self.tabs.is_empty()
                || !dialog
                    .target
                    .current(self.tabs.current_record().document.key()))
        {
            self.liquid.cancel.store(true, Ordering::Release);
            self.liquid.progress = None;
            dialog.review = None;
            dialog.error = Some(
                "The document changed; close this review and check the selection again.".into(),
            );
        }
    }

    fn accept_liquid_progress(&mut self, progress: Progress) {
        if self.liquid.active != Some(progress.id)
            || self.liquid.cancel.load(Ordering::Acquire)
            || progress.status.passage == 0
            || progress.status.passage > progress.status.passages
            || progress.status.chunk == 0
            || progress.status.chunk > progress.status.chunks
            || !(1..=128).contains(&progress.status.model_tokens)
        {
            return;
        }
        if let Some(dialog) = &mut self.liquid.dialog
            && dialog.id == progress.id
            && !self.tabs.is_empty()
            && dialog
                .target
                .current(self.tabs.current_record().document.key())
        {
            dialog.sent = Some(progress.sent);
            dialog.progress = Some(progress.status);
            dialog.review = progress.review;
        }
    }

    pub(super) fn close_liquid(&mut self, context: &egui::Context) {
        // Retain the tracked operation after closing. A second check must wait,
        // and process shutdown waits for preparation/inference cleanup.
        self.liquid.cancel.store(true, Ordering::Release);
        self.liquid.progress = None;
        self.liquid.dialog = None;
        ChildViewHost::close(context, SALT);
        crate::window_host::return_focus(context);
    }

    fn apply_liquid_edits(
        &mut self,
        target: &BoundSelection,
        edits: &[SourceEdit],
        _context: &egui::Context,
    ) -> Result<(), String> {
        if self.tabs.is_empty()
            || !target.current(self.document().key())
            || target.snapshot.source() != self.document().source()
        {
            return Err("The document changed; check the selection again.".into());
        }
        if edits.is_empty() {
            return Ok(());
        }
        // Validate every edit before the single undo transaction. The projection
        // core already enforces source mapping; these checks defend UI admission.
        let mut ordered: Vec<_> = edits.iter().collect();
        ordered.sort_unstable_by_key(|edit| edit.range.start);
        let mut previous_end = target.range.start;
        let mut byte_edits = Vec::with_capacity(ordered.len());
        let mut shift: isize = 0;
        for edit in &ordered {
            if edit.range.start < previous_end
                || edit.range.start < target.range.start
                || edit.range.end > target.range.end
                || edit.range.start > edit.range.end
            {
                return Err("Liquid returned an edit outside the selected text.".into());
            }
            previous_end = edit.range.end;
            shift += edit.replacement.chars().count() as isize - edit.range.len() as isize;
        }
        let ranges: Vec<_> = ordered.iter().map(|edit| edit.range.clone()).collect();
        let mapped = byte_ranges(self.document().source(), &ranges)?;
        for (edit, range) in ordered.iter().zip(mapped) {
            byte_edits.push((range, edit.replacement.as_str()));
        }
        let cursor = CCursorRange::two(
            CCursor::new(target.cursor[0]),
            CCursor::new(target.cursor[1]),
        );
        let mut replacement = String::with_capacity(self.document().source().len());
        let mut byte = 0;
        for (range, text) in byte_edits {
            replacement.push_str(&self.document().source()[byte..range.start]);
            replacement.push_str(text);
            byte = range.end;
        }
        replacement.push_str(&self.document().source()[byte..]);
        self.document_mut()
            .edit(cursor, |source| *source = replacement);
        let end = target.range.end.saturating_add_signed(shift);
        self.pending_editor_selection = Some(EditorSelection::Focus(target.range.start..end));
        self.find_bar.search.clear();
        self.mark_edited();
        Ok(())
    }

    pub(super) fn show_liquid_review(&mut self, context: &egui::Context) {
        let busy = self.liquid.busy();
        let Some(dialog) = &mut self.liquid.dialog else {
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
        let settings = &self.settings;
        let mut action = None;
        ChildViewHost::show(
            context,
            &self.captures,
            ChildViewSpec::modal(SALT, "Liquid grammar", owner, "liquid"),
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
                    action = Some(Action::Close);
                }
                ui.painter()
                    .rect_filled(ui.max_rect(), 0.0, Color32::from_black_alpha(64));
                if let Some(clicked) = review_card(ui.ctx(), dialog, busy, current, owner, settings)
                {
                    action = Some(clicked);
                }
            },
        );
        match action {
            Some(Action::Close) => self.close_liquid(context),
            Some(Action::Apply) if !busy && current => {
                let dialog = self.liquid.dialog.as_ref().unwrap();
                let target = dialog.target.clone();
                let edits = dialog
                    .review
                    .as_ref()
                    .map(|review| review.edits.clone())
                    .unwrap_or_default();
                match self.apply_liquid_edits(&target, &edits, context) {
                    Ok(()) => {
                        self.close_liquid(context);
                        self.focus_source_editor(context);
                    }
                    Err(error) => self.liquid.dialog.as_mut().unwrap().error = Some(error),
                }
            }
            _ => {}
        }
    }

    #[cfg(all(feature = "desktop-ui-tests", target_os = "macos"))]
    pub(super) fn liquid_inspection(&self) -> serde_json::Value {
        serde_json::json!({"visible": self.liquid.visible(), "busy": self.liquid.busy(),
            "input_options": self.settings.liquid_input,
            "progress": self.liquid.dialog.as_ref().and_then(|dialog| dialog.progress).map(|progress| serde_json::json!({
                "passage": progress.passage, "passages": progress.passages,
                "chunk": progress.chunk, "chunks": progress.chunks, "model_tokens": progress.model_tokens,
            })),
            "sent": self.liquid.dialog.as_ref().and_then(|dialog| dialog.sent.as_ref()).map(|sent| sent.fragments.iter().map(|text| serde_json::json!({ "fingerprint": crate::desktop_test::fingerprint(text), "chars": text.chars().count() })).collect::<Vec<_>>()),
            "sent_open": self.liquid.dialog.as_ref().is_some_and(|dialog| dialog.sent_open),
            "changes": self.liquid.dialog.as_ref().and_then(|dialog| dialog.review.as_ref()).map(|review| review.edits.len()),
            "comparison": self.liquid.dialog.as_ref().and_then(|dialog| dialog.review.as_ref()).map(|review| serde_json::json!({
                "before": crate::desktop_test::fingerprint(&review.diff.before),
                "after": crate::desktop_test::fingerprint(&review.diff.current),
                "before_chars": review.diff.before.chars().count(), "after_chars": review.diff.current.chars().count(),
                "style": self.settings.git_diff_style,
            })),
            "error": self.liquid.dialog.as_ref().and_then(|dialog| dialog.error.as_ref()),
            "current": self.liquid.dialog.as_ref().is_some_and(|dialog| !self.tabs.is_empty() && dialog.target.current(self.tabs.current_record().document.key()))})
    }
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
    egui::Area::new(viewport_scoped_id(context, "liquid-review-card"))
        .order(egui::Order::Foreground)
        .anchor(egui::Align2::CENTER_CENTER, Vec2::ZERO)
        .show(context, |ui| {
            theme::dialog_card_frame(&style).show(ui, |ui| {
                ui.set_width((owner.width() - 48.0).clamp(1.0, 960.0));
                // Area remembers the loading card's short size. Reset the
                // content limit before scrolling so results can grow instead
                // of feeding that stale height back into their own clipping.
                ui.set_max_height((owner.height() - 48.0).max(1.0));
                egui::ScrollArea::vertical()
                    .id_salt("liquid-review-card-scroll")
                    .max_height((owner.height() - 48.0).max(1.0))
                    .show(ui, |ui| {
                        if let Some(clicked) =
                            review_ui(ui, dialog, busy, current, owner.height(), settings)
                        {
                            action = Some(clicked);
                        }
                    });
            });
        });
    action
}

#[cfg(test)]
fn record_review_hit(name: &str, response: &egui::Response) {
    response.ctx.data_mut(|data| {
        data.insert_temp(
            egui::Id::new(("liquid-review-hit", name)),
            response.interact_rect,
        )
    });
}

fn review_ui(
    ui: &mut egui::Ui,
    dialog: &mut Dialog,
    busy: bool,
    current: bool,
    height: f32,
    settings: &AppSettings,
) -> Option<Action> {
    let mut action = None;
    let sent_state = dialog.sent.as_ref().map(|_| {
        egui::collapsing_header::CollapsingState::load_with_default_open(
            ui.ctx(),
            ui.make_persistent_id(("liquid-sent-input", dialog.id)),
            false,
        )
    });
    let sent_openness = sent_state
        .as_ref()
        .map_or(0.0, |state| state.openness(ui.ctx()));
    let sent_header = sent_state.as_ref().map_or(0.0, |_| {
        ui.spacing().interact_size.y.max(
            ui.text_style_height(&egui::TextStyle::Button) + ui.spacing().button_padding.y * 2.0,
        ) + ui.spacing().item_spacing.y
    });
    let sent_body = 120.0
        + ui.spacing().item_spacing.y
        + if dialog
            .sent
            .as_ref()
            .is_some_and(|sent| sent.fragments.len() > 1)
        {
            ui.text_style_height(&egui::TextStyle::Body) + ui.spacing().item_spacing.y
        } else {
            0.0
        };
    ui.horizontal(|ui| {
        ui.strong("Liquid · selection");
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            // Plain buttons avoid explanatory hover overlays on this review.
            if ui.button("×").clicked() {
                action = Some(Action::Close);
            }
        });
    });
    ui.separator();
    if busy {
        ui.horizontal(|ui| {
            ui.spinner();
            if let Some(progress) = dialog.progress {
                ui.label(format!(
                    "Passage {}/{} · chunk {}/{}",
                    progress.passage, progress.passages, progress.chunk, progress.chunks
                ));
            } else {
                ui.label("Checking selection…");
            }
        });
    }
    if let Some(error) = &dialog.error {
        ui.colored_label(ui.visuals().error_fg_color, error);
    } else if let Some(review) = &dialog.review {
        if review.edits.is_empty() && !busy {
            ui.label("No corrections suggested.");
        }
        comparison_headers(ui, settings.git_diff_style);
        let path = Path::new(match dialog.target.kind {
            DocumentKind::Tex => "selection.tex",
            _ => "selection.typ",
        });
        let diff_height = review_diff_height(height, sent_header, sent_body, sent_openness);
        ui.allocate_ui(Vec2::new(ui.available_width(), diff_height), |ui| {
            let id = viewport_scoped_id(ui.ctx(), "liquid-selection-diff");
            let viewport = full_view::show_styled_comparison(ui, &review.diff, id, path, settings);
            #[cfg(all(feature = "desktop-ui-tests", target_os = "macos"))]
            crate::desktop_test::observe(
                "liquid.diff",
                &ui.interact(viewport, id.with("observed"), Sense::hover()),
            );
            #[cfg(not(all(feature = "desktop-ui-tests", target_os = "macos")))]
            let _ = viewport;
        });
    }
    if let (Some(sent), Some(state)) = (&dialog.sent, sent_state) {
        let mut clicked = false;
        let mut header = state.show_header(ui, |ui| {
            let response = ui.add(egui::Button::new("Liquid input").frame(false));
            clicked = response.clicked();
            #[cfg(test)]
            record_review_hit("sent", &response);
            #[cfg(all(feature = "desktop-ui-tests", target_os = "macos"))]
            crate::desktop_test::observe("liquid.sent", &response);
        });
        if clicked {
            header.toggle();
        }
        dialog.sent_open = header.is_open();
        header.body_unindented(|ui| {
            if sent.fragments.len() > 1 {
                ui.weak(format!(
                    "{} passages · checked in token-sized chunks",
                    sent.fragments.len()
                ));
            }
            egui::ScrollArea::both()
                .id_salt("liquid-sent-scroll")
                .max_height(120.0)
                .show(ui, |ui| {
                    ui.add(egui::Label::new(RichText::new(&sent.display).monospace()).wrap());
                });
        });
    }
    ui.separator();
    ui.horizontal(|ui| {
        let apply = ui.add_enabled(
            current
                && !busy
                && dialog.error.is_none()
                && dialog
                    .review
                    .as_ref()
                    .is_some_and(|review| !review.edits.is_empty()),
            egui::Button::new("Apply"),
        );
        #[cfg(test)]
        record_review_hit("apply", &apply);
        #[cfg(all(feature = "desktop-ui-tests", target_os = "macos"))]
        crate::desktop_test::observe("liquid.apply", &apply);
        if apply.clicked() {
            action = Some(Action::Apply);
        }
        let close = ui.button(if busy { "Close" } else { "Cancel" });
        #[cfg(all(feature = "desktop-ui-tests", target_os = "macos"))]
        crate::desktop_test::observe("liquid.cancel", &close);
        if close.clicked() {
            action = Some(Action::Close);
        }
    });
    action
}

fn comparison_headers(ui: &mut egui::Ui, style: crate::settings::GitDiffStyle) {
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = theme::SPACE.small;
        match style {
            crate::settings::GitDiffStyle::SideBySide => {
                let width = (ui.available_width() - theme::SPACE.small) / 2.0;
                ui.allocate_ui_with_layout(
                    Vec2::new(width, 20.0),
                    Layout::left_to_right(Align::Center),
                    |ui| {
                        ui.set_min_width(width);
                        ui.weak("Before");
                    },
                );
                ui.weak("After");
            }
            crate::settings::GitDiffStyle::Unified => {
                ui.weak("Before → After");
            }
        }
    });
}

// The same responsive openness drives egui's body and the diff reservation.
// In particular, closing keeps its animated space until the body is gone.
fn review_diff_height(owner_height: f32, sent_header: f32, sent_body: f32, openness: f32) -> f32 {
    (owner_height - 180.0 - sent_header - sent_body * openness.clamp(0.0, 1.0)).clamp(40.0, 520.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn target(app: &EditorApp, range: Range<usize>) -> BoundSelection {
        BoundSelection::new(
            app.document().snapshot(),
            app.document().kind(),
            CCursorRange::two(CCursor::new(range.start), CCursor::new(range.end)),
        )
        .unwrap()
    }

    #[test]
    fn selection_admission_requires_tex_or_typst_and_nonempty_range() {
        let key = DocumentKey::new(tiptoptyp_core::document::WindowSessionId::new(1), 1, 1);
        for kind in [DocumentKind::Typst, DocumentKind::Tex, DocumentKind::Text] {
            let snapshot = crate::document::DocumentSnapshot::fixture(key, "A sentence.");
            assert_eq!(
                BoundSelection::new(
                    snapshot.clone(),
                    kind,
                    CCursorRange::two(CCursor::new(0), CCursor::new(5))
                )
                .is_some(),
                kind != DocumentKind::Text
            );
            assert!(
                BoundSelection::new(snapshot, kind, CCursorRange::one(CCursor::new(1))).is_none()
            );
        }
    }

    #[test]
    fn edits_are_one_undoable_transaction_and_cannot_write_a_newer_document() {
        let directory = tempfile::tempdir().unwrap();
        let context = egui::Context::default();
        let mut app = EditorApp::dormant_for_tests(&context, directory.path().into());
        let original = "Tao’s estmates are continous.";
        app.document_mut().replace_unprojected_untitled(original);
        let selected = target(&app, 6..original.chars().count());
        let edits = vec![
            SourceEdit {
                range: 6..14,
                replacement: "estimates".into(),
            },
            SourceEdit {
                range: 19..28,
                replacement: "continuous".into(),
            },
        ];
        app.apply_liquid_edits(&selected, &edits, &context).unwrap();
        assert_eq!(app.document().source(), "Tao’s estimates are continuous.");
        assert!(app.apply_liquid_edits(&selected, &edits, &context).is_err());
        app.undo_editor(&context, false);
        assert_eq!(app.document().source(), original);
        assert!(
            app.apply_liquid_edits(&selected, &edits, &context).is_err(),
            "undo is still a new revision"
        );
    }

    #[test]
    fn a_bad_edit_rejects_the_whole_batch_without_a_source_or_revision_change() {
        let directory = tempfile::tempdir().unwrap();
        let context = egui::Context::default();
        let mut app = EditorApp::dormant_for_tests(&context, directory.path().into());
        app.document_mut()
            .replace_unprojected_untitled("These estimates holds.");
        let selected = target(&app, 6..15);
        let before = app.document().snapshot();
        assert!(
            app.apply_liquid_edits(
                &selected,
                &[
                    SourceEdit {
                        range: 6..15,
                        replacement: "bounds".into()
                    },
                    SourceEdit {
                        range: 16..21,
                        replacement: "hold".into()
                    },
                ],
                &context
            )
            .is_err()
        );
        assert_eq!(app.document().key(), before.key());
        assert_eq!(app.document().source(), before.source());
    }

    #[test]
    fn cancelling_review_never_clears_harper_or_starts_a_worker() {
        let directory = tempfile::tempdir().unwrap();
        let context = egui::Context::default();
        let mut app = EditorApp::dormant_for_tests(&context, directory.path().into());
        app.document_mut()
            .replace_unprojected_untitled("This is the the result.");
        let harper = vec![Diagnostic {
            correction: None,
            provider: Some("Harper".into()),
            severity: DiagnosticSeverity::Help,
            source: DiagnosticSource::Main,
            location: Some(DiagnosticLocation {
                line: 1,
                column: 13,
            }),
            message: "Repeated word".into(),
            details: vec!["Remove the second ‘the’.".into()],
        }];
        app.writing.diagnostics = harper.clone();
        let selected = target(&app, 0..app.document().source().chars().count());
        app.liquid.dialog = Some(Dialog {
            id: 1,
            target: selected,

            review: None,
            sent: None,
            progress: None,
            sent_open: false,
            error: None,
        });
        app.close_liquid(&context);
        assert!(!app.liquid.visible());
        assert!(!app.liquid.busy());
        assert_eq!(app.writing.diagnostics, harper);
        assert_eq!(app.document().source(), "This is the the result.");
    }

    #[test]
    fn closing_a_busy_review_retains_the_tracked_job_and_refuses_duplicate_work() {
        let directory = tempfile::tempdir().unwrap();
        let context = egui::Context::default();
        let mut app = EditorApp::dormant_for_tests(&context, directory.path().into());
        app.document_mut()
            .replace_unprojected_untitled("This result hold.");
        let selected = target(&app, 0..app.document().source().chars().count());
        app.liquid.next = 1;
        app.liquid.active = Some(1);
        app.liquid.dialog = Some(Dialog {
            id: 1,
            target: selected.clone(),

            review: None,
            sent: None,
            progress: None,
            sent_open: false,
            error: None,
        });
        let (release, wait) = std::sync::mpsc::channel();
        let worker_target = selected.clone();
        app.liquid
            .job
            .start_and_repaint("test-liquid-pending", &context, move || {
                wait.recv().map_err(|error| error.to_string())?;
                Ok(Reply {
                    id: 1,
                    sent: SentInput::new(vec!["This result hold.".into()]),
                    result: prepare_review(
                        &worker_target,
                        vec![SourceEdit {
                            range: 12..16,
                            replacement: "holds".into(),
                        }],
                    ),
                })
            })
            .unwrap();
        assert!(crate::worker::has_active_operations());
        app.close_liquid(&context);
        assert!(app.liquid.busy(), "closing must retain the tracked job");
        assert!(
            crate::worker::has_active_operations(),
            "closing the modal must not release the process-exit lease"
        );
        app.begin_liquid_check(selected, &context);
        assert_eq!(app.liquid.next, 1, "a duplicate request must not start");
        assert!(!app.liquid.visible());
        release.send(()).unwrap();
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while app.liquid.busy() {
            assert!(
                std::time::Instant::now() < deadline,
                "test worker failed to finish"
            );
            app.update_liquid();
            std::thread::sleep(Duration::from_millis(1));
        }
        assert!(!app.liquid.busy());
        assert!(
            !app.liquid.visible(),
            "a closed review cannot be reopened by completion"
        );
        assert_eq!(app.document().source(), "This result hold.");
    }

    #[test]
    fn review_owns_source_commands_and_close_only_dismisses_the_review() {
        let directory = tempfile::tempdir().unwrap();
        let context = egui::Context::default();
        let mut app = EditorApp::dormant_for_tests(&context, directory.path().into());
        app.document_mut().replace_unprojected_untitled("A result.");
        let cursor = CCursorRange::one(CCursor::new(0));
        app.document_mut()
            .edit(cursor, |source| source.push_str(" More."));
        let selected = target(&app, 0..app.document().source().chars().count());
        app.liquid.dialog = Some(Dialog {
            id: 1,
            target: selected,

            review: None,
            sent: None,
            progress: None,
            sent_open: false,
            error: None,
        });
        app.execute_app_command(AppCommand::Undo, &context, None);
        assert_eq!(app.document().source(), "A result. More.");
        app.execute_app_command(AppCommand::CloseTab, &context, None);
        assert!(!app.liquid.visible());
        assert!(!app.tabs.is_empty());
        assert_eq!(app.document().source(), "A result. More.");
    }

    #[test]
    fn review_errors_disable_apply_without_a_manual_installation_step() {
        use egui_kittest::kittest::{NodeT as _, Queryable as _};
        let key = DocumentKey::new(tiptoptyp_core::document::WindowSessionId::new(1), 1, 1);
        for error in [
            "Liquid’s local files are unavailable. Check its data folder in Settings → Writing.",
            "A selected paragraph is too long for Liquid. Select shorter sentences.",
            "Liquid setup cannot find its bundled model. Reinstall the complete app package.",
        ] {
            let selected = BoundSelection::new(
                crate::document::DocumentSnapshot::fixture(key, "A result."),
                DocumentKind::Typst,
                CCursorRange::two(CCursor::new(0), CCursor::new(9)),
            )
            .unwrap();
            let mut dialog = Dialog {
                id: 1,
                target: selected,

                review: None,
                sent: None,
                progress: None,
                sent_open: false,
                error: Some(error.into()),
            };
            let mut harness = egui_kittest::Harness::builder()
                .with_size(Vec2::new(640.0, 320.0))
                .build_ui(move |ui| {
                    review_ui(ui, &mut dialog, false, true, 320.0, &AppSettings::default());
                });
            harness.run();
            assert!(harness.get_by_label("Apply").accesskit_node().is_disabled());
            assert!(harness.query_by_label("Install Liquid").is_none());
        }
    }

    #[test]
    fn comparison_keeps_the_complete_selected_source_and_rebases_unicode_edits() {
        let directory = tempfile::tempdir().unwrap();
        let context = egui::Context::default();
        let mut app = EditorApp::dormant_for_tests(&context, directory.path().into());
        let source = "outside αβγ
#emph[Before. A continous $x$ estimate. After.]
outside";
        app.document_mut().replace_unprojected_untitled(source);
        let start = source
            .chars()
            .position(|character| character == '#')
            .unwrap();
        let end = source.chars().count() - "\noutside".chars().count();
        let selected = target(&app, start..end);
        let typo = source[..source.find("continous").unwrap()].chars().count();
        let review = prepare_review(
            &selected,
            vec![SourceEdit {
                range: typo..typo + 9,
                replacement: "continuous".into(),
            }],
        )
        .unwrap();
        assert_eq!(
            review.diff.before,
            "#emph[Before. A continous $x$ estimate. After.]"
        );
        assert_eq!(
            review.diff.current,
            "#emph[Before. A continuous $x$ estimate. After.]"
        );
        assert_eq!(review.diff.builds, 1);
        assert!(review.diff.pairs.iter().any(|pair| pair.changed));
        assert_eq!(
            app.document().source(),
            source,
            "preparing a review is immutable"
        );
        assert!(
            prepare_review(
                &selected,
                vec![SourceEdit {
                    range: 0..1,
                    replacement: "x".into()
                }]
            )
            .is_err()
        );
    }

    #[test]
    fn separate_editor_windows_share_one_lazy_liquid_client() {
        let first = State::default();
        let second = State::default();
        assert!(Arc::ptr_eq(&first.client, &second.client));
        assert!(!first.busy());
        assert!(!second.busy());
        assert!(!first.visible());
        assert!(!second.visible());
    }

    #[test]
    fn batched_scalar_mapping_handles_many_unicode_edits_and_rejects_bad_bounds() {
        let source = format!("{}{}", "α".repeat(10_000), "aβc ".repeat(128));
        let ranges: Vec<_> = (0..128)
            .map(|index| 10_001 + index * 4..10_002 + index * 4)
            .collect();
        let mapped = byte_ranges(&source, &ranges).unwrap();
        assert_eq!(mapped.len(), 128);
        for range in mapped {
            assert_eq!(&source[range], "β");
        }
        assert!(
            byte_ranges(
                &source,
                std::slice::from_ref(&Range {
                    start: source.chars().count(),
                    end: source.chars().count() + 1
                })
            )
            .is_err()
        );
        assert!(
            byte_ranges(
                &source,
                std::slice::from_ref(&Range {
                    start: 10_002,
                    end: 10_001
                })
            )
            .is_err()
        );
        assert!(byte_ranges(&source, &[]).unwrap().is_empty());
    }

    #[test]
    fn a_failed_runtime_retains_the_exact_prepared_model_input() {
        let directory = tempfile::tempdir().unwrap();
        let context = egui::Context::default();
        let mut app = EditorApp::dormant_for_tests(&context, directory.path().into());
        app.document_mut()
            .replace_unprojected_untitled("#emph[She go.]");
        app.liquid.dialog = Some(Dialog {
            id: 1,
            target: target(&app, 0..app.document().source().chars().count()),

            review: None,
            sent: None,
            progress: None,
            sent_open: false,
            error: None,
        });
        app.liquid.active = Some(1);
        app.liquid.job.start_and_repaint("test-liquid-failure", &context, || Ok(Reply {
            id: 1, sent: SentInput::new(vec!["She go.".into()]), result: Err("Liquid’s local files are unavailable. Check its data folder in Settings → Writing.".into()),
        })).unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        while app.liquid.busy() {
            assert!(Instant::now() < deadline, "test worker failed to finish");
            app.update_liquid();
            std::thread::sleep(Duration::from_millis(1));
        }
        let dialog = app.liquid.dialog.as_ref().unwrap();
        assert_eq!(dialog.sent.as_ref().unwrap().fragments, ["She go."]);
        assert_eq!(dialog.sent.as_ref().unwrap().display, "She go.");
        assert!(dialog.error.as_ref().unwrap().contains("local files"));
        assert!(dialog.review.is_none());
        assert_eq!(app.document().source(), "#emph[She go.]");
    }

    #[test]
    fn exact_sent_text_is_collapsed_by_default_and_read_only_when_opened() {
        use egui_kittest::kittest::Queryable as _;
        let key = DocumentKey::new(tiptoptyp_core::document::WindowSessionId::new(1), 1, 1);
        let selected = BoundSelection::new(
            crate::document::DocumentSnapshot::fixture(key, "We estimate $u$."),
            DocumentKind::Typst,
            CCursorRange::two(CCursor::new(0), CCursor::new(16)),
        )
        .unwrap();
        let dialog = Dialog {
            id: 1,
            target: selected,

            review: None,
            sent: Some(SentInput::new(vec!["We estimate x.".into()])),
            progress: None,
            sent_open: false,
            error: Some("Runtime unavailable".into()),
        };
        let mut harness = egui_kittest::Harness::builder()
            .with_size(Vec2::new(640.0, 400.0))
            .build_ui_state(
                |ui, dialog: &mut Dialog| {
                    review_ui(ui, dialog, false, true, 400.0, &AppSettings::default());
                },
                dialog,
            );
        harness.run();
        assert!(!harness.state().sent_open);
        assert!(harness.query_by_label("We estimate x.").is_none());
        harness.get_by_label("Liquid input").click();
        harness.run();
        assert!(harness.state().sent_open);
        harness.get_by_label("We estimate x.");
        harness.key_press(egui::Key::A);
        harness.run();
        assert_eq!(
            harness.state().sent.as_ref().unwrap().display,
            "We estimate x."
        );
    }

    #[test]
    fn disclosure_reserves_its_animation_space_through_opening_and_closing() {
        for owner in [480.0_f32, 720.0, 920.0] {
            for openness in [0.0, 0.01, 0.2, 0.5, 0.9, 1.0, 0.9, 0.5, 0.2, 0.01, 0.0] {
                let diff = review_diff_height(owner, 28.0, 160.0, openness);
                assert!(180.0 + 28.0 + 160.0 * openness + diff <= owner + 0.01);
            }
        }
        assert!(
            review_diff_height(720.0, 28.0, 160.0, 0.0)
                > review_diff_height(720.0, 28.0, 160.0, 1.0)
        );
    }

    #[test]
    fn disclosure_transitions_keep_the_apply_footer_inside_the_review() {
        use egui_kittest::kittest::Queryable as _;
        let key = DocumentKey::new(tiptoptyp_core::document::WindowSessionId::new(1), 1, 1);
        let source = "We estimate $u$.\n".repeat(20);
        let selected = BoundSelection::new(
            crate::document::DocumentSnapshot::fixture(key, &source),
            DocumentKind::Typst,
            CCursorRange::two(CCursor::new(0), CCursor::new(source.chars().count())),
        )
        .unwrap();
        let review = prepare_review(
            &selected,
            vec![SourceEdit {
                range: 3..11,
                replacement: "bound".into(),
            }],
        )
        .unwrap();
        let dialog = Dialog {
            id: 1,
            target: selected,

            review: Some(Arc::new(review)),
            sent: Some(SentInput::new(vec![
                "First fragment. ".repeat(30),
                "Second fragment. ".repeat(30),
            ])),
            progress: None,
            sent_open: false,
            error: None,
        };
        let mut harness = egui_kittest::Harness::builder()
            .with_size(Vec2::new(900.0, 600.0))
            .with_step_dt(1.0 / 60.0)
            .build_ui_state(
                |ui, state: &mut (Dialog, bool)| {
                    if !state.1 {
                        theme::configure_editor_fonts(
                            ui.ctx(),
                            Default::default(),
                            Default::default(),
                            false,
                            400,
                            400,
                            None,
                        );
                        ui.ctx()
                            .style_mut_of(ui.ctx().theme(), |style| style.animation_time = 0.25);
                        state.1 = true;
                        return;
                    }
                    review_ui(
                        ui,
                        &mut state.0,
                        false,
                        true,
                        600.0,
                        &AppSettings::default(),
                    );
                },
                (dialog, false),
            );
        harness.run();
        for _ in 0..2 {
            harness.get_by_label("Liquid input").click();
            for _ in 0..20 {
                harness.step();
                assert!(
                    harness.get_by_label("Apply").rect().bottom() <= 600.0,
                    "footer escaped during disclosure animation"
                );
            }
        }
        assert!(!harness.state().0.sent_open);
    }

    #[test]
    fn centered_card_grows_from_spinner_to_review_with_live_disclosure_and_footer_hits() {
        let key = DocumentKey::new(tiptoptyp_core::document::WindowSessionId::new(1), 1, 1);
        let source = "#strong[This is fine. She go to school every day.]";
        let selected = BoundSelection::new(
            crate::document::DocumentSnapshot::fixture(key, source),
            DocumentKind::Typst,
            CCursorRange::two(CCursor::new(0), CCursor::new(source.chars().count())),
        )
        .unwrap();
        let dialog = Dialog {
            id: 1,
            target: selected,

            review: None,
            sent: None,
            progress: None,
            sent_open: false,
            error: None,
        };
        let owner = Rect::from_min_size(Pos2::ZERO, Vec2::new(1400.0, 883.0));
        let mut harness = egui_kittest::Harness::builder()
            .with_size(owner.size())
            .build_ui_state(
                move |ui, state: &mut (Dialog, bool, bool)| {
                    if !state.2 {
                        theme::configure_editor_fonts(
                            ui.ctx(),
                            Default::default(),
                            Default::default(),
                            false,
                            400,
                            400,
                            None,
                        );
                        state.2 = true;
                        return;
                    }
                    review_card(
                        ui.ctx(),
                        &mut state.0,
                        state.1,
                        true,
                        owner,
                        &AppSettings::default(),
                    );
                },
                (dialog, true, false),
            );
        harness.run_steps(5);
        let selected = harness.state().0.target.clone();
        let typo = source[..source.find("go to").unwrap()].chars().count();
        harness.state_mut().0.review = Some(Arc::new(
            prepare_review(
                &selected,
                vec![SourceEdit {
                    range: typo..typo + 2,
                    replacement: "goes".into(),
                }],
            )
            .unwrap(),
        ));
        harness.state_mut().0.sent = Some(SentInput::new(vec![
            "This is fine. She go to school every day.".into(),
        ]));
        harness.state_mut().1 = false;
        harness.run_steps(4);
        for name in ["sent", "apply"] {
            let hit = harness
                .ctx
                .data(|data| data.get_temp::<Rect>(egui::Id::new(("liquid-review-hit", name))))
                .unwrap();
            assert!(
                hit.is_positive(),
                "{name} must have a nonempty clipped hit rectangle: {hit:?}"
            );
            assert!(
                owner.contains_rect(hit),
                "{name} escaped the owner: {hit:?}"
            );
        }
        let hit = harness
            .ctx
            .data(|data| data.get_temp::<Rect>(egui::Id::new(("liquid-review-hit", "sent"))))
            .unwrap();
        harness.event(egui::Event::PointerMoved(hit.center()));
        for pressed in [true, false] {
            harness.event(egui::Event::PointerButton {
                pos: hit.center(),
                button: egui::PointerButton::Primary,
                pressed,
                modifiers: Modifiers::NONE,
            });
            harness.step();
        }
        harness.run();
        assert!(
            harness.state().0.sent_open,
            "physical pointer input must open the hosted disclosure"
        );
    }

    #[test]
    fn comparison_headers_align_with_both_source_columns_on_the_same_baseline() {
        use egui_kittest::kittest::Queryable as _;
        let mut harness = egui_kittest::Harness::builder()
            .with_size(Vec2::new(960.0, 100.0))
            .build_ui_state(
                |ui, bounds: &mut Rect| {
                    *bounds = ui.available_rect_before_wrap();
                    comparison_headers(ui, crate::settings::GitDiffStyle::SideBySide);
                },
                Rect::NOTHING,
            );
        harness.run();
        let before = harness.get_by_label("Before").rect();
        let after = harness.get_by_label("After").rect();
        let bounds = *harness.state();
        let right_column = bounds.left() + (bounds.width() + theme::SPACE.small) / 2.0;
        assert!(
            (before.left() - bounds.left()).abs() < 0.5,
            "Before must start at the left source column: {before:?}"
        );
        assert!(
            (after.left() - right_column).abs() < 0.5,
            "After must start at the right source column: {after:?}"
        );
        assert!(
            (before.bottom() - after.bottom()).abs() < 0.5,
            "headers must share their text baseline: {before:?} / {after:?}"
        );
    }
    #[test]
    fn streaming_diff_is_visible_but_apply_waits_for_successful_complete_review() {
        use egui_kittest::kittest::{NodeT as _, Queryable as _};
        let directory = tempfile::tempdir().unwrap();
        let context = egui::Context::default();
        let mut app = EditorApp::dormant_for_tests(&context, directory.path().into());
        app.document_mut()
            .replace_unprojected_untitled("She go. He go.");
        let selected = target(&app, 0..14);
        let review = Arc::new(
            prepare_review(
                &selected,
                vec![SourceEdit {
                    range: 4..6,
                    replacement: "goes".into(),
                }],
            )
            .unwrap(),
        );
        assert_eq!(review.diff.current, "She goes. He go.");
        let dialog = Dialog {
            id: 7,
            target: selected.clone(),
            review: Some(review.clone()),
            sent: Some(SentInput::new(vec!["She go. He go.".into()])),
            progress: Some(ChunkStatus {
                passage: 1,
                passages: 1,
                chunk: 1,
                chunks: 2,
                model_tokens: 8,
            }),
            sent_open: false,
            error: None,
        };
        let mut fonts_ready = false;
        let mut harness = egui_kittest::Harness::builder()
            .with_size(Vec2::new(800.0, 600.0))
            .build_ui_state(
                move |ui, state: &mut (Dialog, bool)| {
                    if !fonts_ready {
                        theme::configure_test_fonts(ui.ctx());
                        fonts_ready = true;
                        return;
                    }
                    review_ui(
                        ui,
                        &mut state.0,
                        state.1,
                        true,
                        600.0,
                        &AppSettings::default(),
                    );
                },
                (dialog, true),
            );
        // The busy view deliberately animates; observe bounded frames instead
        // of waiting for an idle frame that cannot exist until checking ends.
        harness.run_steps(3);
        harness.get_by_label("Passage 1/1 · chunk 1/2");
        harness.get_by_label("Before → After");
        assert!(harness.get_by_label("Apply").accesskit_node().is_disabled());
        harness.state_mut().1 = false;
        harness.state_mut().0.error = Some("The final chunk failed.".into());
        harness.run();
        assert!(
            harness.get_by_label("Apply").accesskit_node().is_disabled(),
            "a failed final chunk cannot make the partial correction applyable"
        );
        harness.state_mut().0.error = None;
        harness.state_mut().0.review =
            Some(Arc::new(prepare_review(&selected, Vec::new()).unwrap()));
        harness.run();
        assert!(
            harness.get_by_label("Apply").accesskit_node().is_disabled(),
            "an empty complete result cannot reuse partial suggestions"
        );
        harness.state_mut().0.review = Some(review);
        harness.run();
        assert!(!harness.get_by_label("Apply").accesskit_node().is_disabled());
        assert_eq!(app.document().source(), "She go. He go.");
    }

    #[test]
    fn stale_cancelled_and_invalidated_chunk_updates_cannot_reopen_or_enable_review() {
        let directory = tempfile::tempdir().unwrap();
        let context = egui::Context::default();
        let mut app = EditorApp::dormant_for_tests(&context, directory.path().into());
        app.document_mut()
            .replace_unprojected_untitled("She go. He go.");
        let selected = target(&app, 0..14);
        app.liquid.active = Some(7);
        app.liquid.dialog = Some(Dialog {
            id: 7,
            target: selected.clone(),
            review: None,
            sent: None,
            progress: None,
            sent_open: false,
            error: None,
        });
        let progress = |id| Progress {
            id,
            sent: SentInput::new(vec!["She go. He go.".into()]),
            review: Some(Arc::new(
                prepare_review(
                    &selected,
                    vec![SourceEdit {
                        range: 4..6,
                        replacement: "goes".into(),
                    }],
                )
                .unwrap(),
            )),
            status: ChunkStatus {
                passage: 1,
                passages: 1,
                chunk: 1,
                chunks: 2,
                model_tokens: 8,
            },
        };
        app.accept_liquid_progress(progress(6));
        assert!(app.liquid.dialog.as_ref().unwrap().review.is_none());
        app.accept_liquid_progress(progress(7));
        assert!(app.liquid.dialog.as_ref().unwrap().review.is_some());
        app.document_mut()
            .replace_unprojected_untitled("Edited source.");
        app.update_liquid();
        assert!(app.liquid.cancel.load(Ordering::Acquire));
        assert!(app.liquid.dialog.as_ref().unwrap().review.is_none());
        app.accept_liquid_progress(progress(7));
        assert!(app.liquid.dialog.as_ref().unwrap().review.is_none());
        app.close_liquid(&context);
        app.accept_liquid_progress(progress(7));
        assert!(!app.liquid.visible());
        assert_eq!(app.document().source(), "Edited source.");
    }
}
