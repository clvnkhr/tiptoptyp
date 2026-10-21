//! Review metadata before appending it to a bibliography or inserting a citation.
use super::*;
use crate::citation_import::{self, BibliographySnapshot, Citation, ImportResult};
use std::sync::atomic::{AtomicBool, Ordering};

pub(super) const SALT: &str = "tiptoptyp-citation-import";
#[derive(Clone)]
struct Target {
    key: DocumentKey,
    cursor: CCursorRange,
    kind: DocumentKind,
}
enum Reply {
    Loaded(Citation, Arc<BibliographySnapshot>),
    Imported(ImportResult),
}
pub(super) struct Dialog {
    query: String,
    destination: String,
    key: String,
    bibtex: String,
    snapshot: Option<Arc<BibliographySnapshot>>,
    target: Option<Target>,
    insert: bool,
    completed: Option<(String, bool)>,
    error: Option<String>,
    job: LatestJob<Reply>,
    cancel: Arc<AtomicBool>,
    picker: Option<PendingDialog<()>>,
    focus: bool,
    pub(super) edit_events: Vec<egui::Event>,
}
impl Drop for Dialog {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Release);
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Action {
    Lookup,
    Import,
    Back,
    Pick,
    Open,
    Close,
}
impl Dialog {
    fn busy(&self) -> bool {
        self.job.is_running() || self.picker.is_some()
    }
    fn lookup(&mut self, context: &egui::Context, dirty: Vec<PathBuf>) {
        if self.busy() {
            return;
        }
        self.error = None;
        let identifier = self.query.clone();
        let path = PathBuf::from(self.destination.trim());
        self.cancel = Arc::default();
        let cancel = self.cancel.clone();
        if let Err(error) = self
            .job
            .start_and_repaint("Find citation", context, move || {
                reject_dirty_bibliography(&path, &dirty)?;
                let snapshot = BibliographySnapshot::read(&path)?;
                let citation = lookup_metadata(&identifier, &cancel)?;
                Ok(Reply::Loaded(citation, Arc::new(snapshot)))
            })
        {
            self.error = Some(error);
        }
    }
    fn import(&mut self, context: &egui::Context, dirty: Vec<PathBuf>) {
        if self.busy() || self.completed.is_some() {
            return;
        }
        let Some(snapshot) = self.snapshot.clone() else {
            return;
        };
        let key = self.key.clone();
        let bibtex = self.bibtex.clone();
        self.cancel = Arc::default();
        let cancel = self.cancel.clone();
        self.error = None;
        if let Err(error) = self
            .job
            .start_and_repaint("Import citation", context, move || {
                reject_dirty_bibliography(&snapshot.path, &dirty)?;
                let reviewed = citation_import::rekey(&bibtex, &key)?;
                snapshot.import(&reviewed, &cancel).map(Reply::Imported)
            })
        {
            self.error = Some(error);
        }
    }
}
fn lookup_metadata(identifier: &str, cancel: &AtomicBool) -> Result<Citation, String> {
    #[cfg(all(feature = "desktop-ui-tests", target_os = "macos"))]
    if identifier.trim() == "10.5555/tiptoptyp-desktop-test"
        && crate::desktop_test::enabled()
        && let Some(path) = std::env::var_os("TIPTOPTYP_UI_TEST_CITATION_METADATA")
    {
        use std::io::Read;
        let mut source = String::new();
        std::fs::File::open(path)
            .map_err(|e| e.to_string())?
            .take(256 * 1024 + 1)
            .read_to_string(&mut source)
            .map_err(|e| e.to_string())?;
        if source.len() > 256 * 1024 {
            return Err("The citation entry is too large.".into());
        }
        if cancel.load(Ordering::Acquire) {
            return Err("Citation import cancelled.".into());
        }
        let entry = citation_import::parse_entry(&source)?;
        let parsed = citation_import::Identifier::parse(identifier)?;
        // Validate the actual parsed DOI field, never inject UI state.
        if entry.doi().as_deref() != Some("10.5555/tiptoptyp-desktop-test") {
            return Err("The native-test citation fixture has the wrong DOI.".into());
        }
        return Ok(Citation {
            identifier: parsed,
            key: entry.key,
            bibtex: entry.bibtex,
        });
    }
    citation_import::lookup(identifier, cancel)
}

// Canonical alias checks belong to the job, never the frame path.
fn reject_dirty_bibliography(path: &Path, dirty: &[PathBuf]) -> Result<(), String> {
    let resolved = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_owned());
    if dirty
        .iter()
        .any(|open| std::fs::canonicalize(open).unwrap_or_else(|_| open.clone()) == resolved)
    {
        return Err("Save the bibliography’s unsaved edits before importing a citation.".into());
    }
    Ok(())
}
fn suggested_bibliography(source: &str, kind: DocumentKind, directory: &Path) -> PathBuf {
    for line in source.lines() {
        let line = line.trim_start();
        if line.starts_with('%') || line.starts_with("//") {
            continue;
        }
        let name = if kind == DocumentKind::Tex {
            ["\\addbibresource", "\\bibliography"]
                .iter()
                .find_map(|command| {
                    let rest = line.strip_prefix(command)?.trim_start();
                    let rest = if rest.starts_with('[') {
                        rest.split_once(']')?.1.trim_start()
                    } else {
                        rest
                    };
                    rest.strip_prefix('{')?
                        .split_once('}')
                        .map(|(value, _)| value.split(',').next().unwrap_or(value))
                })
        } else if kind.is_typst() {
            line.strip_prefix("#bibliography")
                .and_then(|rest| rest.trim_start().strip_prefix('('))
                .and_then(|rest| rest.trim_start().strip_prefix('"'))
                .and_then(|rest| rest.split_once('"').map(|(value, _)| value))
        } else {
            None
        };
        if let Some(name) =
            name.filter(|name| !name.is_empty() && !name.contains(['\0', '\\', '#', '{', '}']))
        {
            let mut path = directory.join(name.trim());
            if path.extension().is_none() {
                path.set_extension("bib");
            }
            if path
                .extension()
                .is_some_and(|ext| ext.eq_ignore_ascii_case("bib"))
            {
                return path;
            }
        }
    }
    directory.join("references.bib")
}
fn citation_text(kind: DocumentKind, key: &str) -> String {
    if kind == DocumentKind::Tex {
        format!("\\cite{{{key}}}")
    } else if typst_syntax::is_valid_label_literal_id(key) && !key.ends_with(['.', ':']) {
        format!("@{key}")
    } else {
        // Bare references trim trailing ./: and cannot represent + in BibTeX keys.
        format!("#cite(label(\"{key}\"))")
    }
}
impl EditorApp {
    pub(super) fn begin_citation(&mut self, context: &egui::Context) {
        if self.document_flow_busy() {
            return;
        }
        let target = (!self.tabs.is_empty()
            && self.document().kind().typesetting_language().is_some())
        .then(|| Target {
            key: self.document().key(),
            cursor: self.editor_snapshot(context).cursor,
            kind: self.document().kind(),
        });
        let directory = self
            .document()
            .path()
            .as_ref()
            .and_then(|path| path.parent())
            .map(Path::to_path_buf)
            .unwrap_or_else(|| self.workspace_root.clone());
        let destination =
            suggested_bibliography(self.document().source(), self.document().kind(), &directory)
                .to_string_lossy()
                .into_owned();
        self.citation = Some(Dialog {
            query: String::new(),
            destination,
            key: String::new(),
            bibtex: String::new(),
            snapshot: None,
            insert: target.is_some(),
            target,
            completed: None,
            error: None,
            job: Default::default(),
            cancel: Arc::default(),
            picker: None,
            focus: true,
            edit_events: Vec::new(),
        });
        self.format_request_key = None;
        self.manual_format_revision = None;
        self.clear_editor_hover();
        self.clear_asset_hover();
        self.close_app_popup();
        self.find_bar.close();
        ChildViewHost::close(context, find_bar::VIEWPORT_SALT);
        context.request_repaint();
    }
    pub(super) fn close_citation(&mut self, context: &egui::Context) {
        if self.citation.take().is_some() {
            ChildViewHost::close(context, SALT);
            crate::window_host::return_focus(context);
        }
    }
    fn dirty_bibliographies(&self) -> Vec<PathBuf> {
        self.tabs
            .ids()
            .filter_map(|id| self.document_for_tab(id))
            .filter(|document| document.is_dirty())
            .filter_map(|document| document.path().clone())
            .collect()
    }
    fn insert_imported_citation(&mut self, target: &Target, key: &str) -> Result<(), String> {
        if self.tabs.is_empty() || self.document().key() != target.key {
            return Err(
                "Citation saved; the document changed, so no citation was inserted.".into(),
            );
        }
        citation_import::validate_key(key)?;
        let range = target.cursor.as_sorted_char_range();
        let start = range.start.0;
        let bytes = char_range_to_byte_range(self.document().source(), start..range.end.0)
            .ok_or("Citation saved; the insertion point changed.")?;
        let follows_label = self.document().source()[bytes.end..]
            .chars()
            .next()
            .is_some_and(|c| c.is_alphanumeric() || matches!(c, '_' | '-' | ':' | '.'));
        let text = if target.kind.is_typst() && follows_label {
            format!("#cite(label({key:?}))")
        } else {
            citation_text(target.kind, key)
        };
        self.document_mut()
            .edit(target.cursor, |source| source.replace_range(bytes, &text));
        self.pending_editor_selection =
            Some(EditorSelection::Focus(start..start + text.chars().count()));
        self.find_bar.search.clear();
        self.mark_edited();
        Ok(())
    }
    pub(super) fn show_citation(&mut self, context: &egui::Context, frame: Option<&eframe::Frame>) {
        let Some(dialog) = &mut self.citation else {
            return;
        };
        if let DialogPoll::Ready {
            selection: Some(file),
            ..
        } = crate::workflow::poll_dialog(&mut dialog.picker, context)
        {
            dialog.destination = file.path().to_string_lossy().into_owned();
        }
        let imported = match dialog.job.poll() {
            LatestJobPoll::Ready(Reply::Loaded(citation, snapshot)) => {
                dialog.query = citation.identifier.value().to_owned();
                dialog.key = citation.key;
                dialog.bibtex = citation.bibtex;
                dialog.snapshot = Some(snapshot);
                dialog.error = None;
                None
            }
            LatestJobPoll::Ready(Reply::Imported(result)) => {
                dialog.completed = Some((result.key.clone(), result.added));
                dialog
                    .insert
                    .then(|| dialog.target.clone())
                    .flatten()
                    .map(|target| (target, result.key))
            }
            LatestJobPoll::Failed(error) => {
                dialog.error = Some(error);
                None
            }
            _ => None,
        };
        if let Some((target, key)) = imported
            && let Err(error) = self.insert_imported_citation(&target, &key)
        {
            self.citation.as_mut().unwrap().error = Some(error);
        }
        let Some(owner) = context.input(|input| input.viewport().inner_rect) else {
            return;
        };
        let style = context.style_of(context.theme());
        let shortcuts = self.settings.effective_shortcuts();
        let child = scoped_child_viewport_id(context, SALT);
        let admitted = self
            .pending_widget_paste
            .is_some_and(|pending| pending.admits(child, context.cumulative_frame_nr()));
        let mut paste_requested = false;
        let mut paste_admitted = false;
        let mut action = None;
        let dialog = self.citation.as_mut().unwrap();
        ChildViewHost::show(
            context,
            &self.captures,
            ChildViewSpec::modal(SALT, "Import Citation", owner, "citation-import"),
            context.theme(),
            &style,
            |ui, input| {
                handle_window_management_shortcuts(ui.ctx(), ui.ctx().viewport_id(), &shortcuts);
                if input.close_requested
                    || input.escape_pressed
                    || [ShortcutAction::CloseTab, ShortcutAction::CloseWindow]
                        .into_iter()
                        .any(|action| {
                            shortcuts.binding(action).is_some_and(|key| {
                                ui.input_mut(|input| input.consume_shortcut(&key.egui()))
                            })
                        })
                {
                    action = Some(Action::Close);
                }
                let edits = prepare_child_widget_edit_events(ui.ctx(), &shortcuts, admitted);
                paste_requested |= edits.paste_requested;
                paste_admitted |= edits.requested_paste_admitted;
                let _queued =
                    crate::child_view::begin_queued_edit_events(ui.ctx(), &mut dialog.edit_events);
                ui.painter()
                    .rect_filled(ui.max_rect(), 0.0, Color32::from_black_alpha(64));
                egui::Area::new(viewport_scoped_id(ui.ctx(), "citation-card"))
                    .order(egui::Order::Foreground)
                    .anchor(egui::Align2::CENTER_CENTER, Vec2::ZERO)
                    .show(ui.ctx(), |ui| {
                        theme::dialog_card_frame(&style).show(ui, |ui| {
                            ui.set_width((owner.width() - 48.0).clamp(1.0, 700.0));
                            ui.set_height((owner.height() - 48.0).clamp(1.0, 430.0));
                            if let Some(clicked) = show(ui, dialog)
                                && action != Some(Action::Close)
                            {
                                action = Some(clicked);
                            }
                        });
                    });
            },
        );
        if paste_admitted {
            self.pending_widget_paste = None;
        }
        if paste_requested {
            self.request_widget_paste(context, child);
        }
        match action {
            Some(Action::Close) => self.close_citation(context),
            Some(Action::Lookup) => {
                let dirty = self.dirty_bibliographies();
                self.citation.as_mut().unwrap().lookup(context, dirty);
            }
            Some(Action::Import) => {
                let dirty = self.dirty_bibliographies();
                self.citation.as_mut().unwrap().import(context, dirty);
            }
            Some(Action::Back) => {
                let dialog = self.citation.as_mut().unwrap();
                dialog.snapshot = None;
                dialog.error = None;
                dialog.focus = true;
            }
            Some(Action::Open) => {
                let path = self
                    .citation
                    .as_ref()
                    .unwrap()
                    .snapshot
                    .as_ref()
                    .map(|snapshot| snapshot.path.clone());
                self.close_citation(context);
                if let Some(path) = path {
                    self.open_tab_path(path, context);
                }
            }
            Some(Action::Pick) => {
                if let Some(mut picker) = self.native_file_dialog(frame) {
                    #[cfg(any(target_os = "macos", target_os = "windows"))]
                    if let Some(parent) = crate::native_window::for_viewport(context, child) {
                        picker = picker.set_parent(&parent);
                    }
                    let dialog = self.citation.as_mut().unwrap();
                    if let Some(parent) = Path::new(&dialog.destination)
                        .parent()
                        .filter(|path| path.is_absolute())
                    {
                        picker = picker.set_directory(parent);
                    }
                    dialog.picker = Some(PendingDialog::new(
                        (),
                        picker
                            .set_title("Choose bibliography")
                            .add_filter("BibTeX bibliography", &["bib"])
                            .set_file_name("references.bib")
                            .save_file(),
                    ));
                }
            }
            None => {}
        }
    }
    #[cfg(all(feature = "desktop-ui-tests", target_os = "macos"))]
    pub(super) fn citation_inspection(&self) -> serde_json::Value {
        let Some(dialog) = &self.citation else {
            return serde_json::json!({"visible":false,"busy":false,"review":false,"completed":false,
                "query_fingerprint":null,"destination_fingerprint":null,"key_fingerprint":null,
                "bibtex_fingerprint":null,"insert":false,"added":null,"error":null});
        };
        serde_json::json!({"visible":true,"busy":dialog.busy(),"review":dialog.snapshot.is_some(),
            "completed":dialog.completed.is_some(),"query_fingerprint":crate::desktop_test::fingerprint(&dialog.query),
            "destination_fingerprint":crate::desktop_test::fingerprint(&dialog.destination),
            "key_fingerprint":crate::desktop_test::fingerprint(&dialog.key),
            "bibtex_fingerprint":crate::desktop_test::fingerprint(&dialog.bibtex),"insert":dialog.insert,
            "added":dialog.completed.as_ref().map(|(_,added)| *added),"error":dialog.error})
    }
}
fn observe(_name: &str, _response: &egui::Response) {
    #[cfg(all(feature = "desktop-ui-tests", target_os = "macos"))]
    crate::desktop_test::observe(&format!("citation.{_name}"), _response);
}
fn show(ui: &mut egui::Ui, dialog: &mut Dialog) -> Option<Action> {
    theme::apply_compact_tool_spacing(ui);
    let mut action = None;
    ui.horizontal(|ui| {
        ui.strong("Import Citation");
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            let close = icon_button(ui, UiIcon::Close, "Close citation import");
            observe("close", &close);
            if close.clicked() {
                action = Some(Action::Close);
            }
        });
    });
    ui.separator();
    let busy = dialog.busy();
    let review = dialog.snapshot.is_some();
    let complete = dialog.completed.is_some();
    let bounds = ui.available_rect_before_wrap();
    let footer_height = ui.spacing().interact_size.y * 2.0 + ui.spacing().item_spacing.y;
    let split = (bounds.bottom() - footer_height).max(bounds.top());
    let body = Rect::from_min_max(bounds.min, Pos2::new(bounds.right(), split));
    ui.scope_builder(
        egui::UiBuilder::new()
            .id_salt("citation-body")
            .max_rect(body),
        |ui| {
            ui.set_clip_rect(ui.clip_rect().intersect(body));
            egui::ScrollArea::vertical()
                .id_salt("citation-scroll")
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    ui.add_enabled_ui(!busy && !complete, |ui| {
                        if !review {
                            let label = ui.label("DOI or arXiv ID / URL");
                            let response = ui.add(
                                egui::TextEdit::singleline(&mut dialog.query)
                                    .id_salt("citation-query")
                                    .desired_width(f32::INFINITY)
                                    .hint_text("10.1000/example · arXiv:2401.01234"),
                            );
                            let response = response.labelled_by(label.id);
                            observe("query", &response);
                            if dialog.focus {
                                response.request_focus();
                                dialog.focus = false;
                            }
                            if response.lost_focus()
                                && ui.input(|input| input.key_pressed(egui::Key::Enter))
                            {
                                action = Some(Action::Lookup);
                            }
                            ui.label("Bibliography");
                            ui.horizontal(|ui| {
                                let width = (ui.available_width() - 76.0).max(1.0);
                                let field = ui.add(
                                    egui::TextEdit::singleline(&mut dialog.destination)
                                        .id_salt("citation-path")
                                        .desired_width(width),
                                );
                                field.widget_info(|| {
                                    egui::WidgetInfo::labeled(
                                        egui::WidgetType::TextEdit,
                                        true,
                                        "Bibliography",
                                    )
                                });
                                observe("destination", &field);
                                let choose = ui.button("Choose…");
                                observe("choose", &choose);
                                if choose.clicked() {
                                    action = Some(Action::Pick);
                                }
                            });
                        } else {
                            ui.weak(format!(
                                "{} · {}",
                                dialog.query,
                                if dialog
                                    .snapshot
                                    .as_ref()
                                    .is_some_and(|snapshot| snapshot.exists)
                                {
                                    "Append to bibliography"
                                } else {
                                    "Create bibliography"
                                }
                            ));
                            ui.weak(&dialog.destination);
                            ui.horizontal(|ui| {
                                let label = ui.label("Citation key");
                                let response = ui.add(
                                    egui::TextEdit::singleline(&mut dialog.key)
                                        .id_salt("citation-key")
                                        .desired_width(240.0),
                                );
                                let response = response.labelled_by(label.id);
                                observe("key", &response);
                            });
                            let field = ui.add(
                                egui::TextEdit::multiline(&mut dialog.bibtex)
                                    .id_salt("citation-bibtex")
                                    .code_editor()
                                    .desired_width(f32::INFINITY)
                                    .desired_rows(9),
                            );
                            field.widget_info(|| {
                                egui::WidgetInfo::labeled(
                                    egui::WidgetType::TextEdit,
                                    true,
                                    "BibTeX entry",
                                )
                            });
                            observe("bibtex", &field);
                        }
                        if dialog.target.is_some() {
                            let insert =
                                ui.checkbox(&mut dialog.insert, "Insert citation at selection");
                            observe("insert", &insert);
                        }
                    });
                    if let Some((key, added)) = &dialog.completed {
                        ui.colored_label(
                            ui.visuals().selection.stroke.color,
                            if *added {
                                format!("Imported {key}")
                            } else {
                                format!("Already in bibliography: {key}")
                            },
                        );
                    }
                    if let Some(error) = &dialog.error {
                        ui.colored_label(ui.visuals().error_fg_color, error);
                    }
                });
        },
    );
    ui.scope_builder(
        egui::UiBuilder::new()
            .id_salt("citation-footer")
            .max_rect(Rect::from_min_max(
                Pos2::new(bounds.left(), split),
                bounds.max,
            )),
        |ui| {
            ui.separator();
            ui.horizontal_wrapped(|ui| {
                if complete {
                    let open = ui
                        .push_id("open-bibliography", |ui| ui.button("Open bibliography"))
                        .inner;
                    observe("open", &open);
                    if open.clicked() {
                        action = Some(Action::Open);
                    }
                    let done = ui.push_id("done", |ui| ui.button("Done")).inner;
                    observe("done", &done);
                    if done.clicked() {
                        action = Some(Action::Close);
                    }
                } else {
                    if review {
                        let back = ui
                            .push_id("back", |ui| {
                                ui.add_enabled(!busy, egui::Button::new("Back"))
                            })
                            .inner;
                        observe("back", &back);
                        if back.clicked() {
                            action = Some(Action::Back);
                        }
                    }
                    let button = ui
                        .push_id(if review { "import" } else { "lookup" }, |ui| {
                            ui.add_enabled(
                                !busy
                                    && (review
                                        || (!dialog.query.trim().is_empty()
                                            && !dialog.destination.trim().is_empty())),
                                egui::Button::new(if review { "Import" } else { "Find citation" }),
                            )
                        })
                        .inner;
                    observe(if review { "import" } else { "lookup" }, &button);
                    if button.clicked() {
                        action = Some(if review {
                            Action::Import
                        } else {
                            Action::Lookup
                        });
                    }
                    if busy {
                        ui.spinner();
                        ui.weak(if review { "Saving…" } else { "Looking up…" });
                    }
                }
            });
        },
    );
    action
}
#[cfg(test)]
mod tests {
    use super::*;
    use egui_kittest::{
        Harness,
        kittest::{NodeT, Queryable},
    };

    fn fixture() -> Dialog {
        Dialog { query: "10.5555/tiptoptyp-desktop-test".into(), destination: "/tmp/references.bib".into(),
            key: "Fixture2026".into(), bibtex: "@article{Fixture2026, title={A reviewed title}, doi={10.5555/tiptoptyp-desktop-test}}".into(),
            snapshot: None, target: None, insert: false, completed: None, error: None,
            job: Default::default(), cancel: Arc::default(), picker: None, focus: true, edit_events: Vec::new() }
    }
    fn harness(dialog: Dialog) -> Harness<'static, (Dialog, Option<Action>)> {
        let mut fonts_ready = false;
        Harness::builder()
            .with_size(Vec2::new(480.0, 330.0))
            .build_ui_state(
                move |ui, state: &mut (Dialog, Option<Action>)| {
                    if !fonts_ready {
                        theme::configure_test_fonts(ui.ctx());
                        fonts_ready = true;
                        return;
                    }
                    // Key press and release are separate frames in egui_kittest.
                    if let Some(action) = show(ui, &mut state.0) {
                        state.1 = Some(action);
                    }
                },
                (dialog, None),
            )
    }

    #[test]
    fn lookup_requires_a_query_and_destination_and_enter_submits_the_field() {
        let mut dialog = fixture();
        dialog.query.clear();
        let mut harness = harness(dialog);
        harness.run();
        assert!(
            harness
                .get_by_label("Find citation")
                .accesskit_node()
                .is_disabled()
        );
        harness.get_by_label("DOI or arXiv ID / URL").focus();
        harness.step();
        assert!(harness.get_by_label("DOI or arXiv ID / URL").is_focused());
        harness
            .get_by_label("DOI or arXiv ID / URL")
            .type_text("10.1111/old-query");
        harness.run();
        harness.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::A);
        harness.step();
        harness
            .get_by_label("DOI or arXiv ID / URL")
            .type_text("10.5555/tiptoptyp-desktop-test");
        harness.run();
        assert_eq!(harness.state().0.query, "10.5555/tiptoptyp-desktop-test");
        assert!(
            !harness
                .get_by_label("Find citation")
                .accesskit_node()
                .is_disabled()
        );
        assert!(harness.get_by_label("DOI or arXiv ID / URL").is_focused());
        harness.state_mut().1 = None;
        harness.key_press(egui::Key::Enter);
        harness.step();
        assert_eq!(harness.state().1, Some(Action::Lookup));
        harness.state_mut().0.destination.clear();
        harness.run();
        assert!(
            harness
                .get_by_label("Find citation")
                .accesskit_node()
                .is_disabled()
        );
    }

    #[test]
    fn reviewed_key_and_source_are_editable_while_footer_remains_visible() {
        let directory = tempfile::tempdir().unwrap();
        let mut dialog = fixture();
        dialog.snapshot = Some(Arc::new(
            BibliographySnapshot::read(&directory.path().join("references.bib")).unwrap(),
        ));
        let mut harness = harness(dialog);
        harness.run();
        harness.get_by_label("Citation key").focus();
        harness.step();
        assert!(harness.get_by_label("Citation key").is_focused());
        harness.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::A);
        harness.step();
        harness
            .get_by_label("Citation key")
            .type_text("Reviewed2026");
        harness.run();
        assert_eq!(harness.state().0.key, "Reviewed2026");
        harness.get_by_label("BibTeX entry").focus();
        harness.step();
        assert!(harness.get_by_label("BibTeX entry").is_focused());
        harness.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::A);
        harness.step();
        let edited = "@article{Reviewed2026, title={The edited title}}";
        harness.get_by_label("BibTeX entry").type_text(edited);
        harness.run();
        assert_eq!(harness.state().0.bibtex, edited);
        let import = harness.get_by_label("Import");
        assert!(
            import.rect().bottom() <= 330.0 && import.rect().top() > 250.0,
            "{:?}",
            import.rect()
        );
        harness.state_mut().1 = None;
        harness.get_by_label("Import").click();
        harness.step();
        assert_eq!(harness.state().1, Some(Action::Import));
        harness.state_mut().0.completed = Some(("Fixture2026".into(), true));
        harness.run();
        harness.state_mut().1 = None;
        harness.get_by_label("Open bibliography").click();
        harness.step();
        assert_eq!(harness.state().1, Some(Action::Open));
        assert!(harness.get_by_label("Done").rect().bottom() <= 330.0);
    }

    #[test]
    fn typst_citation_keys_preserve_punctuation_that_bare_references_trim_or_reject() {
        assert_eq!(
            citation_text(DocumentKind::Typst, "Smith2026"),
            "@Smith2026"
        );
        for key in ["Smith2026.", "Smith2026:", "Smith+2026", "Smith2026+"] {
            citation_import::validate_key(key).unwrap();
            let citation = citation_text(DocumentKind::Typst, key);
            assert_eq!(citation, format!("#cite(label(\"{key}\"))"));
            assert!(
                typst_syntax::Source::detached(citation)
                    .root()
                    .errors_and_warnings()
                    .0
                    .is_empty()
            );
            assert_eq!(
                citation_text(DocumentKind::Tex, key),
                format!("\\cite{{{key}}}")
            );
        }
    }

    #[test]
    fn closing_citation_cancels_pending_work_and_does_not_change_the_source() {
        let context = egui::Context::default();
        let directory = tempfile::tempdir().unwrap();
        let mut app = EditorApp::dormant_for_tests(&context, directory.path().into());
        app.begin_citation(&context);
        let cancel = app.citation.as_ref().unwrap().cancel.clone();
        app.close_citation(&context);
        assert!(cancel.load(Ordering::Acquire));
        assert!(app.citation.is_none());
    }

    #[test]
    fn bibliography_inference_uses_project_declarations() {
        let directory = Path::new("/project");
        for (source, kind, expected) in [
            ("\\bibliography{refs,other}", DocumentKind::Tex, "refs.bib"),
            (
                "\\addbibresource[location=local]{bib/papers.bib}",
                DocumentKind::Tex,
                "bib/papers.bib",
            ),
            (
                "#bibliography(\"works.bib\")",
                DocumentKind::Typst,
                "works.bib",
            ),
            (
                "% \\bibliography{ignore}\nHello",
                DocumentKind::Tex,
                "references.bib",
            ),
        ] {
            assert_eq!(
                suggested_bibliography(source, kind, directory),
                directory.join(expected)
            );
        }
    }
    #[test]
    fn insertion_is_bound_to_document_and_one_undo_transaction() {
        let root = tempfile::tempdir().unwrap();
        let context = egui::Context::default();
        let mut app = EditorApp::dormant_for_tests(&context, root.path().into());
        app.document_mut().replace_loaded_unprojected(
            "See here.".into(),
            root.path().join("paper.tex"),
            DocumentKind::Tex,
            None,
        );
        let target = Target {
            key: app.document().key(),
            cursor: CCursorRange::two(CCursor::new(4), CCursor::new(8)),
            kind: DocumentKind::Tex,
        };
        app.insert_imported_citation(&target, "Tao2026").unwrap();
        assert_eq!(app.document().source(), "See \\cite{Tao2026}.");
        assert!(app.insert_imported_citation(&target, "other").is_err());
        app.execute_app_command(AppCommand::Undo, &context, None);
        assert_eq!(app.document().source(), "See here.");
        assert_eq!(citation_text(DocumentKind::Typst, "Tao2026"), "@Tao2026");
    }
    #[test]
    fn typst_insertion_before_text_cannot_extend_the_citation_key() {
        let root = tempfile::tempdir().unwrap();
        let context = egui::Context::default();
        let mut app = EditorApp::dormant_for_tests(&context, root.path().into());
        app.document_mut().replace_loaded_unprojected(
            "See more".into(),
            root.path().join("paper.typ"),
            DocumentKind::Typst,
            None,
        );
        let target = Target {
            key: app.document().key(),
            cursor: CCursorRange::one(CCursor::new(4)),
            kind: DocumentKind::Typst,
        };
        app.insert_imported_citation(&target, "Tao2026").unwrap();
        assert_eq!(app.document().source(), "See #cite(label(\"Tao2026\"))more");
    }

    #[test]
    fn modal_owns_parent_commands_and_closes_without_source_edits() {
        let root = tempfile::tempdir().unwrap();
        let context = egui::Context::default();
        let mut app = EditorApp::dormant_for_tests(&context, root.path().into());
        app.document_mut().replace_loaded_unprojected(
            "untouched".into(),
            root.path().join("paper.typ"),
            DocumentKind::Typst,
            None,
        );
        app.begin_citation(&context);
        assert!(app.document_flow_busy());
        app.execute_app_command(AppCommand::New, &context, None);
        app.execute_app_command(AppCommand::ToggleComment, &context, None);
        assert_eq!(app.document().source(), "untouched");
        app.execute_app_command(AppCommand::CloseTab, &context, None);
        assert!(app.citation.is_none());
    }
    #[test]
    #[ignore = "captures fresh compact integration framebuffers for visual review"]
    #[cfg(target_os = "macos")]
    fn integration_modal_framebuffer_review() {
        let directory = tempfile::tempdir().unwrap();
        for dark in [false, true] {
            for scene in [
                "texdoc-results",
                "citation-lookup",
                "citation-review",
                "citation-success",
            ] {
                let context = egui::Context::default();
                theme::configure_test_fonts(&context);
                let palette = crate::builtin_themes::find(if dark {
                    "catppuccin-mocha"
                } else {
                    "catppuccin-latte"
                })
                .unwrap();
                theme::set_imported_palette(&context, Some((dark, palette.palette)));
                theme::configure_styles(&context);
                context.set_theme(if dark {
                    egui::Theme::Dark
                } else {
                    egui::Theme::Light
                });
                let mut dialog = fixture();
                dialog.destination = "/project/bibliography/references.bib".into();
                dialog.target = Some(Target {
                    key: DocumentSession::new(
                        tiptoptyp_core::document::WindowSessionId::new(0),
                        "",
                        DocumentKind::Typst,
                    )
                    .key(),
                    cursor: CCursorRange::default(),
                    kind: DocumentKind::Typst,
                });
                dialog.insert = true;
                if scene != "citation-lookup" {
                    dialog.snapshot = Some(Arc::new(
                        BibliographySnapshot::read(&directory.path().join("references.bib"))
                            .unwrap(),
                    ));
                }
                if scene == "citation-success" {
                    dialog.completed = Some(("Fixture2026".into(), true));
                }
                let mut framebuffer = crate::test_framebuffer::Framebuffer::new([900, 580]);
                let destination = PathBuf::from(format!(
                    ".tiptoptyp/screenshots/agent-review/integrations/{}-{scene}.png",
                    if dark { "dark" } else { "light" }
                ));
                std::fs::create_dir_all(destination.parent().unwrap()).unwrap();
                for frame in 0..4 {
                    let output = context.run_ui(egui::RawInput {
                        screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(900.0,580.0))),
                        time:Some(frame as f64 / 60.0), ..Default::default()
                    }, |ui| {
                        ui.painter().rect_filled(ui.max_rect(), 0.0, ui.visuals().panel_fill);
                        egui::Area::new(egui::Id::new("integration-review-card")).anchor(egui::Align2::CENTER_CENTER, Vec2::ZERO).show(ui.ctx(), |ui| {
                            theme::dialog_card_frame(ui.style()).show(ui, |ui| {
                                let texdoc = scene == "texdoc-results";
                                ui.set_width(if texdoc {620.0} else {700.0});
                                ui.set_height(if texdoc {390.0} else {430.0});
                                if texdoc { super::super::tex_documentation::show_framebuffer_results(ui); }
                                else { let _ = show(ui, &mut dialog); }
                            });
                        });
                    });
                    framebuffer.render(
                        &context,
                        output,
                        (frame == 3).then_some(destination.as_path()),
                    );
                }
                assert!(destination.metadata().unwrap().len() > 1000);
            }
        }
    }
}
