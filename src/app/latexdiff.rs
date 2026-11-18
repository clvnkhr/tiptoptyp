//! An owned comparison form; filesystem reads and publication stay on its worker.
use super::*;
use crate::tex::diff::{self, Cancellation, Completed, Request};
use std::sync::atomic::Ordering;

pub(super) const SALT: &str = "tiptoptyp-latexdiff";
const ERROR_HEIGHT: f32 = 56.0;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Action {
    Older,
    Newer,
    Output,
    Generate,
    CancelGeneration,
    Close,
}
pub(super) struct Dialog {
    older: String,
    newer: String,
    destination: String,
    destination_edited: bool,
    current: Option<(PathBuf, String)>,
    include_edits: bool,
    focus_file: bool,
    settings: diff::Settings,
    validated_directory: Option<String>,
    error: Option<String>,
    operation: Operation,
    pub(super) edit_events: Vec<egui::Event>,
}
/// File picking and generation are mutually exclusive. A generation owns both
/// the worker receipt and its cancellation guard until it is fully drained.
#[derive(Default)]
enum Operation {
    #[default]
    Idle,
    Picking(PendingDialog<Action>),
    Generating(Generation),
    Cancelling(Generation),
}
struct Generation {
    job: LatestJob<Completed>,
    cancel: Cancellation,
}
impl Operation {
    fn generating(&self) -> bool {
        matches!(self, Self::Generating(_) | Self::Cancelling(_))
    }
    fn cancelling(&self) -> bool {
        matches!(self, Self::Cancelling(_))
    }
    fn generation_mut(&mut self) -> Option<&mut Generation> {
        match self {
            Self::Generating(generation) | Self::Cancelling(generation) => Some(generation),
            _ => None,
        }
    }
    #[cfg(all(feature = "desktop-ui-tests", target_os = "macos"))]
    fn name(&self) -> &'static str {
        match self {
            Self::Idle => "idle",
            Self::Picking(_) => "picking",
            Self::Generating(_) => "generating",
            Self::Cancelling(_) => "cancelling",
        }
    }
}
impl Dialog {
    fn new(current: Option<(PathBuf, String)>, settings: diff::Settings) -> Self {
        let newer = current
            .as_ref()
            .map(|(path, _)| path.to_string_lossy().into_owned())
            .unwrap_or_default();
        let destination = suggested_destination(&newer);
        Self {
            older: String::new(),
            newer,
            destination,
            destination_edited: false,
            current,
            include_edits: true,
            focus_file: true,
            settings,
            validated_directory: None,
            error: None,
            operation: Operation::Idle,
            edit_events: Vec::new(),
        }
    }
    pub(super) fn activity(&self) -> crate::activity::Activity {
        use crate::activity::Activity;
        match &self.operation {
            Operation::Generating(_) | Operation::Cancelling(_) => Activity::Running,
            Operation::Picking(_) => Activity::Pending("Choose a file"),
            Operation::Idle => self
                .error
                .as_ref()
                .map_or(Activity::Idle, |error| Activity::Failed(error.clone())),
        }
    }

    fn busy(&self) -> bool {
        !matches!(self.operation, Operation::Idle)
    }
    fn invalidate(&mut self) {
        self.error = None;
    }
    fn validate_files(&self) -> Result<(), String> {
        validate_tex_path(&self.older, "older file")?;
        validate_tex_path(&self.newer, "newer file")?;
        Ok(())
    }
    fn destination(&self) -> Result<PathBuf, String> {
        validate_tex_path(&self.destination, "output file")
    }
    fn request(&self) -> Result<Request, String> {
        self.validate_files()?;
        self.settings.validate_syntax()?;
        // Validation that touches the filesystem or resolves tools runs in generate().
        Ok(Request {
            older: self.older.trim().into(),
            newer: self.newer.trim().into(),
            source: self
                .include_edits
                .then_some(self.current.as_ref())
                .flatten()
                .filter(|(path, _)| path == Path::new(self.newer.trim()))
                .map(|(_, source)| source.clone()),
            settings: self.settings.clone(),
        })
    }
    fn start(&mut self, context: &egui::Context) {
        if self.busy() {
            return;
        }
        let input = self
            .request()
            .and_then(|request| Ok((request, self.destination()?)));
        match input {
            Ok((request, destination)) => {
                self.invalidate();
                let cancel = Cancellation::default();
                let token = cancel.0.clone();
                let mut job = LatestJob::default();
                match job.start_and_repaint("Create LaTeX diff", context, move || {
                    diff::generate_and_save(request, &destination, &token)
                }) {
                    Ok(()) => self.operation = Operation::Generating(Generation { job, cancel }),
                    Err(error) => self.error = Some(error),
                }
            }
            Err(error) => self.error = Some(error),
        }
    }
    fn cancel_generation(&mut self) {
        self.operation = match std::mem::replace(&mut self.operation, Operation::Idle) {
            Operation::Generating(generation) => {
                generation.cancel.0.store(true, Ordering::Release);
                Operation::Cancelling(generation)
            }
            other => other,
        };
    }
    fn poll(&mut self, context: &egui::Context) -> Option<Completed> {
        if matches!(self.operation, Operation::Picking(_)) {
            let Operation::Picking(picker) =
                std::mem::replace(&mut self.operation, Operation::Idle)
            else {
                unreachable!()
            };
            let mut picker = Some(picker);
            match crate::workflow::poll_dialog(&mut picker, context) {
                DialogPoll::Pending => {
                    self.operation =
                        Operation::Picking(picker.expect("pending picker is retained"));
                }
                DialogPoll::Ready {
                    request: action,
                    selection: Some(file),
                } => {
                    let value = file.path().to_string_lossy().into_owned();
                    match action {
                        Action::Older => self.older = value,
                        Action::Newer => {
                            self.newer = value;
                            if !self.destination_edited {
                                self.destination = suggested_destination(&self.newer);
                            }
                        }
                        Action::Output => {
                            self.destination = value;
                            self.destination_edited = true;
                        }
                        _ => unreachable!(),
                    }
                    self.invalidate();
                }
                DialogPoll::Ready {
                    selection: None, ..
                } => {}
                DialogPoll::Idle => unreachable!("an installed picker must be polled"),
            }
        }
        let outcome = self
            .operation
            .generation_mut()
            .map(|generation| generation.job.poll());
        match outcome {
            Some(LatestJobPoll::Ready(completed)) => {
                self.operation = Operation::Idle;
                self.validated_directory = Some(self.settings.command.directory.clone());
                self.error = None;
                Some(completed)
            }
            Some(LatestJobPoll::Failed(error)) => {
                self.operation = Operation::Idle;
                self.error = Some(error);
                None
            }
            _ => None,
        }
    }
}
fn validate_tex_path(value: &str, role: &str) -> Result<PathBuf, String> {
    let path = PathBuf::from(value.trim());
    if value.trim().is_empty() {
        return Err(format!("Choose the {role}."));
    }
    if !path.is_absolute() || value.contains('\0') {
        return Err(format!("Choose an absolute path for the {role}."));
    }
    if !crate::document::is_tex_path(&path) {
        return Err(format!(
            "The {role} must be a LaTeX source file (.tex, .sty, .cls, .ltx or .latex)."
        ));
    }
    Ok(path)
}
fn suggested_destination(newer: &str) -> String {
    let path = Path::new(newer.trim());
    let Some(stem) = path.file_stem() else {
        return String::new();
    };
    path.with_file_name(format!("{}-diff.tex", stem.to_string_lossy()))
        .to_string_lossy()
        .into_owned()
}

pub(super) fn prepare_snapshot(app: &mut EditorApp, context: &egui::Context) {
    if app.latexdiff.is_some() {
        return;
    }
    app.begin_latexdiff(context);
    if let Some(dialog) = &mut app.latexdiff {
        dialog.older = "/project/revision-before.tex".into();
        dialog.newer = "/project/revision-after.tex".into();
        dialog.destination = "/project/revision-after-diff.tex".into();
    }
}

impl EditorApp {
    pub(super) fn begin_latexdiff(&mut self, context: &egui::Context) {
        if self.document_flow_busy() {
            return;
        }
        let current = (!self.tabs.is_empty() && self.document().kind() == DocumentKind::Tex)
            .then(|| {
                self.document()
                    .path()
                    .clone()
                    .map(|path| (path, self.document().source().to_owned()))
            })
            .flatten();
        self.formatting.cancel();
        self.latexdiff = Some(Dialog::new(current, self.settings.latexdiff.clone()));
        self.clear_editor_hover();
        self.clear_asset_hover();
        self.close_app_popup();
        self.find_bar.close();
        ChildViewHost::close(context, find_bar::VIEWPORT_SALT);
        context.request_repaint();
    }
    pub(super) fn close_latexdiff(&mut self, context: &egui::Context) {
        if let Some(dialog) = self.latexdiff.take() {
            if dialog.settings != self.settings.latexdiff
                && dialog.settings.validate_syntax().is_ok()
                && (dialog.settings.command.directory == self.settings.latexdiff.command.directory
                    || dialog.validated_directory.as_ref()
                        == Some(&dialog.settings.command.directory))
            {
                let mut edited = self.settings.clone();
                edited.latexdiff = dialog.settings.clone();
                self.queue_settings(edited, context);
            }
            // Dropping Cancellation signals the worker; dropping LatestJob never joins it.
            ChildViewHost::close(context, SALT);
            crate::window_host::return_focus(context);
        }
    }
    pub(super) fn show_latexdiff(
        &mut self,
        context: &egui::Context,
        frame: Option<&eframe::Frame>,
    ) {
        let Some(dialog) = &mut self.latexdiff else {
            return;
        };
        if let Some(completed) = dialog.poll(context) {
            if !completed.output.warnings.trim().is_empty() {
                self.push_status_log(
                    format!("LaTeXdiff messages: {}", completed.output.warnings.trim()),
                    NoticeKind::Info,
                );
            }
            self.open_created_latexdiff(completed.path, context);
            return;
        }
        let Some(owner) = context.input(|input| input.viewport().inner_rect) else {
            return;
        };
        let appearance = context.theme();
        let style = context.style_of(appearance);
        let shortcuts = self.settings.effective_shortcuts();
        let child = scoped_child_viewport_id(context, SALT);
        let admitted = self
            .pending_widget_paste
            .is_some_and(|pending| pending.admits(child, context.cumulative_frame_nr()));
        let mut paste_requested = false;
        let mut paste_admitted = false;
        let mut action = None;
        ChildViewHost::show(
            context,
            &self.captures,
            ChildViewSpec::modal(SALT, "Create LaTeX Diff", owner, "latexdiff"),
            appearance,
            &style,
            |ui, input| {
                handle_window_management_shortcuts(ui.ctx(), ui.ctx().viewport_id(), &shortcuts);
                if close_requested(ui.ctx(), input, &shortcuts) {
                    action = Some(Action::Close);
                }
                let edits = prepare_child_widget_edit_events(ui.ctx(), &shortcuts, admitted);
                paste_requested |= edits.paste_requested;
                paste_admitted |= edits.requested_paste_admitted;
                let _queued =
                    crate::child_view::begin_queued_edit_events(ui.ctx(), &mut dialog.edit_events);
                ui.painter()
                    .rect_filled(ui.max_rect(), 0.0, Color32::from_black_alpha(64));
                egui::Area::new(viewport_scoped_id(ui.ctx(), "latexdiff-card"))
                    .order(egui::Order::Foreground)
                    .anchor(egui::Align2::CENTER_CENTER, Vec2::ZERO)
                    .show(ui.ctx(), |ui| {
                        theme::dialog_card_frame(&style).show(ui, |ui| {
                            ui.set_width(card_width(owner.width()));
                            ui.set_height(card_height(owner.height()));
                            if let Some(clicked) = show(ui, dialog) {
                                action = Some(clicked);
                            }
                        });
                    });
            },
        );
        for viewport in [child, context.viewport_id()] {
            if let Some(url) = context.data_mut(|data| {
                data.remove_temp::<String>(egui::Id::new(("settings-documentation", viewport)))
            }) {
                self.follow_preview_link(&url);
            }
        }
        if paste_admitted {
            self.pending_widget_paste = None;
        }
        if paste_requested {
            self.request_widget_paste(context, child);
        }
        match action {
            Some(Action::Close) => self.close_latexdiff(context),
            Some(Action::Generate) => self.latexdiff.as_mut().unwrap().start(context),
            Some(Action::CancelGeneration) => self.latexdiff.as_mut().unwrap().cancel_generation(),
            Some(action @ (Action::Older | Action::Newer | Action::Output)) => {
                self.pick_latexdiff_file(context, frame, action)
            }
            None => {}
        }
    }
    fn pick_latexdiff_file(
        &mut self,
        context: &egui::Context,
        frame: Option<&eframe::Frame>,
        action: Action,
    ) {
        let Some(mut picker) = self.native_file_dialog(frame) else {
            return;
        };
        #[cfg(any(target_os = "macos", target_os = "windows"))]
        if let Some(parent) =
            crate::native_window::for_viewport(context, scoped_child_viewport_id(context, SALT))
        {
            picker = picker.set_parent(&parent);
        }
        let dialog = self.latexdiff.as_mut().unwrap();
        if dialog.busy() {
            return;
        }
        // The picker itself resolves the initial folder; no stat calls in the frame path.
        let source = if action == Action::Output {
            &dialog.destination
        } else {
            &dialog.newer
        };
        if let Some(parent) = Path::new(source).parent().filter(|p| p.is_absolute()) {
            picker = picker.set_directory(parent);
        }
        picker = picker.add_filter("LaTeX source", crate::document::TEX_SOURCE_EXTENSIONS);
        dialog.operation = Operation::Picking(if action == Action::Output {
            if let Some(name) = Path::new(&dialog.destination).file_name() {
                picker = picker.set_file_name(name.to_string_lossy());
            }
            PendingDialog::new(
                action,
                picker
                    .set_title("Choose a new diff output file")
                    .save_file(),
            )
        } else {
            PendingDialog::new(
                action,
                picker
                    .set_title(if action == Action::Older {
                        "Choose older TeX file"
                    } else {
                        "Choose newer TeX file"
                    })
                    .pick_file(),
            )
        });
    }
    #[cfg(all(feature = "desktop-ui-tests", target_os = "macos"))]
    pub(super) fn latexdiff_inspection(&self) -> serde_json::Value {
        let Some(dialog) = &self.latexdiff else {
            return serde_json::json!({"visible":false,"busy":false,"phase":"closed","error":null,
                "older_fingerprint":null,"newer_fingerprint":null,"output_fingerprint":null,"arguments_fingerprint":null});
        };
        serde_json::json!({"visible":true,"busy":dialog.busy(),"phase":dialog.operation.name(),"error":dialog.error,
            "older_fingerprint":crate::desktop_test::fingerprint(&dialog.older),
            "newer_fingerprint":crate::desktop_test::fingerprint(&dialog.newer),
            "output_fingerprint":crate::desktop_test::fingerprint(&dialog.destination),
            "arguments_fingerprint":crate::desktop_test::fingerprint(&dialog.settings.command.arguments)})
    }
}
fn card_width(owner_width: f32) -> f32 {
    (owner_width - 48.0).clamp(1.0, 660.0)
}
fn card_height(owner_height: f32) -> f32 {
    (owner_height - 48.0).clamp(1.0, 480.0)
}
fn close_requested(
    context: &egui::Context,
    input: crate::child_view::ChildViewInput,
    shortcuts: &ShortcutBindings,
) -> bool {
    input.close_requested
        || input.escape_pressed
        || [ShortcutAction::CloseWindow, ShortcutAction::CloseTab]
            .into_iter()
            .any(|action| {
                shortcuts.binding(action).is_some_and(|key| {
                    context.input_mut(|input| input.consume_shortcut(&key.egui()))
                })
            })
}
fn observe(_name: &str, _response: &egui::Response) {
    #[cfg(all(feature = "desktop-ui-tests", target_os = "macos"))]
    crate::desktop_test::observe(&format!("latexdiff.{_name}"), _response);
}
fn show(ui: &mut egui::Ui, dialog: &mut Dialog) -> Option<Action> {
    theme::apply_compact_tool_spacing(ui);
    let mut action = None;
    ui.horizontal(|ui| {
        ui.strong("Create LaTeX Diff");
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            let close = icon_button(ui, UiIcon::Close, "Close LaTeX diff");
            observe("close", &close);
            if close.clicked() {
                action = Some(Action::Close);
            }
        });
    });
    ui.separator();
    let busy = dialog.busy();
    let bounds = ui.available_rect_before_wrap();
    let footer_top = (bounds.bottom() - footer_height(ui, dialog)).max(bounds.top());
    let body = Rect::from_min_max(bounds.min, Pos2::new(bounds.right(), footer_top));
    let footer = Rect::from_min_max(Pos2::new(bounds.left(), footer_top), bounds.max);
    ui.scope_builder(
        egui::UiBuilder::new()
            .id_salt("latexdiff-body")
            .max_rect(body),
        |ui| {
            ui.set_clip_rect(ui.clip_rect().intersect(body));
            egui::ScrollArea::vertical()
                .id_salt("latexdiff-options-scroll")
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    let mut changed = false;
                    ui.add_enabled_ui(!busy, |ui| {
                        let (edited, choose) = path_field(
                            ui,
                            "Older file",
                            &mut dialog.older,
                            "older",
                            std::mem::take(&mut dialog.focus_file),
                        );
                        changed |= edited;
                        if choose {
                            action = Some(Action::Older);
                        }
                        let (edited, choose) =
                            path_field(ui, "Newer file", &mut dialog.newer, "newer", false);
                        changed |= edited;
                        if edited && !dialog.destination_edited {
                            dialog.destination = suggested_destination(&dialog.newer);
                        }
                        if choose {
                            action = Some(Action::Newer);
                        }
                        if dialog
                            .current
                            .as_ref()
                            .is_some_and(|(path, _)| path == Path::new(dialog.newer.trim()))
                        {
                            changed |= ui
                                .checkbox(
                                    &mut dialog.include_edits,
                                    "Include unsaved editor changes",
                                )
                                .changed();
                        }
                        let (edited, choose) =
                            path_field(ui, "Output file", &mut dialog.destination, "output", false);
                        changed |= edited;
                        if edited {
                            dialog.destination_edited = true;
                        }
                        if choose {
                            action = Some(Action::Output);
                        }
                        ui.add_space(4.0);
                        changed |= preferences(ui, &mut dialog.settings);
                    });
                    if changed {
                        dialog.invalidate();
                    }
                });
        },
    );
    ui.scope_builder(
        egui::UiBuilder::new()
            .id_salt("latexdiff-actions")
            .max_rect(footer),
        |ui| {
            ui.set_clip_rect(ui.clip_rect().intersect(footer));
            ui.separator();
            if let Some(error) = &dialog.error {
                egui::ScrollArea::vertical()
                    .id_salt("latexdiff-error-scroll")
                    .max_height(ERROR_HEIGHT)
                    .show(ui, |ui| {
                        let status = ui.colored_label(ui.visuals().error_fg_color, error);
                        observe("status", &status);
                    });
                ui.add_space(4.0);
            }
            ui.allocate_ui_with_layout(
                Vec2::new(ui.available_width(), ui.spacing().interact_size.y),
                Layout::right_to_left(Align::Center).with_main_wrap(true),
                |ui| {
                    for (name, id, clicked) in footer_actions(dialog) {
                        let enabled = match clicked {
                            Action::Generate => !busy,
                            Action::CancelGeneration => !dialog.operation.cancelling(),
                            _ => true,
                        };
                        let button = if clicked == Action::Generate {
                            egui::Button::new(RichText::new(name).strong())
                                .fill(ui.visuals().selection.bg_fill)
                        } else {
                            egui::Button::new(name)
                        };
                        let response = ui.push_id(id, |ui| ui.add_enabled(enabled, button)).inner;
                        observe(id, &response);
                        if response.clicked() {
                            action = Some(clicked);
                        }
                    }
                    if dialog.operation.generating() {
                        ui.spinner();
                        ui.weak(if dialog.operation.cancelling() {
                            "Canceling…"
                        } else {
                            "Creating diff…"
                        });
                    }
                },
            );
        },
    );
    ui.advance_cursor_after_rect(bounds);
    action
}
fn path_field(
    ui: &mut egui::Ui,
    name: &str,
    value: &mut String,
    id: &str,
    focus: bool,
) -> (bool, bool) {
    let label = ui.label(name);
    ui.horizontal(|ui| {
        let width = (ui.available_width()
            - text_width(ui, "Choose…")
            - ui.spacing().button_padding.x * 2.0
            - ui.spacing().item_spacing.x)
            .max(1.0);
        let field = ui
            .add_sized(
                [width, ui.spacing().interact_size.y],
                egui::TextEdit::singleline(value)
                    .id_salt(("latexdiff-file", id))
                    .hint_text("/path/to/document.tex")
                    .desired_width(width),
            )
            .labelled_by(label.id);
        observe(id, &field);
        if focus {
            field.request_focus();
        }
        let choose = ui.button("Choose…");
        observe(&format!("choose_{id}"), &choose);
        (field.changed(), choose.clicked())
    })
    .inner
}
fn footer_actions(dialog: &Dialog) -> Vec<(&'static str, &'static str, Action)> {
    if dialog.operation.generating() {
        vec![("Cancel", "cancel", Action::CancelGeneration)]
    } else {
        vec![
            ("Create diff", "generate", Action::Generate),
            ("Cancel", "cancel", Action::Close),
        ]
    }
}
fn text_width(ui: &egui::Ui, text: &str) -> f32 {
    ui.painter()
        .layout_no_wrap(
            text.into(),
            egui::TextStyle::Body.resolve(ui.style()),
            ui.visuals().text_color(),
        )
        .size()
        .x
}
fn footer_height(ui: &egui::Ui, dialog: &Dialog) -> f32 {
    let mut widths: Vec<_> = footer_actions(dialog)
        .iter()
        .map(|(label, _, _)| text_width(ui, label) + ui.spacing().button_padding.x * 2.0 + 4.0)
        .collect();
    if dialog.operation.generating() {
        widths.push(
            ui.spacing().interact_size.y
                + ui.spacing().item_spacing.x
                + text_width(ui, "Creating diff…"),
        );
    }
    let rows = wrapped_rows(ui.available_width(), &widths, ui.spacing().item_spacing.x);
    let error_height = dialog.error.as_ref().map_or(0.0, |error| {
        ui.painter()
            .layout(
                error.clone(),
                egui::TextStyle::Body.resolve(ui.style()),
                ui.visuals().error_fg_color,
                ui.available_width(),
            )
            .size()
            .y
            .min(ERROR_HEIGHT)
            + 4.0
    });
    rows as f32 * (ui.spacing().interact_size.y + ui.spacing().item_spacing.y + 2.0)
        + 8.0
        + error_height
}
fn wrapped_rows(width: f32, items: &[f32], gap: f32) -> usize {
    let mut rows = 1;
    let mut used = 0.0;
    for &item in items {
        if used > 0.0 && used + gap + item > width {
            rows += 1;
            used = item;
        } else {
            used += if used == 0.0 { item } else { gap + item };
        }
    }
    rows
}
pub(super) fn preferences(ui: &mut egui::Ui, settings: &mut diff::Settings) -> bool {
    let mut changed = false;
    ui.horizontal_wrapped(|ui| {
        ui.label("Markup");
        egui::ComboBox::from_id_salt("latexdiff-markup")
            .selected_text(settings.markup.label())
            .show_ui(ui, |ui| {
                for markup in diff::Markup::ALL {
                    changed |= ui
                        .selectable_value(&mut settings.markup, markup, markup.label())
                        .changed();
                }
            });
        changed |= ui
            .checkbox(&mut settings.flatten, "Include subfiles")
            .changed();
    });
    let label = ui.label("Arguments");
    let arguments = ui
        .add(
            egui::TextEdit::singleline(&mut settings.command.arguments)
                .id_salt("latexdiff-arguments")
                .desired_width(ui.available_width())
                .hint_text("--math-markup=1"),
        )
        .labelled_by(label.id);
    observe("options.arguments", &arguments);
    changed |= arguments.changed();
    egui::CollapsingHeader::new("Advanced").show(ui, |ui| {
        for (name, value, example) in [
            (
                "Executable",
                &mut settings.executable,
                "latexdiff (auto-detect)",
            ),
            (
                "Environment",
                &mut settings.command.environment,
                "{\"LC_ALL\":\"C\"}",
            ),
            (
                "Working directory",
                &mut settings.command.directory,
                "Newer file’s directory",
            ),
        ] {
            let label = ui.label(name);
            changed |= ui
                .add(
                    egui::TextEdit::singleline(value)
                        .id_salt(("latexdiff-option", name))
                        .desired_width(ui.available_width())
                        .hint_text(example),
                )
                .labelled_by(label.id)
                .changed();
        }
        super::settings_controls::documentation_link(ui, "LaTeXdiff documentation", diff::DOCS);
        if ui.button("Reset LaTeXdiff options").clicked() {
            *settings = diff::Settings::default();
            changed = true;
        }
    });
    changed
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui_kittest::{
        Harness,
        kittest::{NodeT, Queryable},
    };
    fn fixture() -> Dialog {
        let mut dialog = Dialog::new(
            Some((PathBuf::from("/tmp/paper.tex"), "unsaved text".into())),
            diff::Settings::default(),
        );
        dialog.older = "/tmp/older.tex".into();
        dialog
    }
    #[test]
    fn activity_reads_ready_error_and_pending_picker_without_polling() {
        use crate::activity::Activity;
        let mut dialog = fixture();
        assert_eq!(dialog.activity(), Activity::Idle);
        dialog.error = Some("The older file is unavailable".into());
        assert_eq!(
            dialog.activity(),
            Activity::Failed("The older file is unavailable".into())
        );
        dialog.operation = Operation::Picking(PendingDialog::new(
            Action::Older,
            std::future::poll_fn(|_| panic!("Activity must not poll file dialogs")),
        ));
        assert_eq!(dialog.activity(), Activity::Pending("Choose a file"));
        assert_eq!(dialog.activity(), Activity::Pending("Choose a file"));
        assert!(
            matches!(dialog.operation, Operation::Picking(_)),
            "observing must not poll or consume the picker"
        );
        assert!(!dialog.operation.generating());
        dialog.operation = Operation::Idle;
        dialog.error = None;
        assert_eq!(dialog.activity(), Activity::Idle);
    }

    fn controlled_generation(
        dialog: &mut Dialog,
        operation: impl FnOnce() -> Result<Completed, String> + Send + 'static,
    ) -> Arc<std::sync::atomic::AtomicBool> {
        assert!(!dialog.busy());
        let cancel = Cancellation::default();
        let token = cancel.0.clone();
        let mut job = LatestJob::default();
        job.start("controlled generation", operation).unwrap();
        dialog.operation = Operation::Generating(Generation { job, cancel });
        token
    }
    fn wait_for_generation(dialog: &mut Dialog) {
        dialog
            .operation
            .generation_mut()
            .unwrap()
            .job
            .wait_until_ready_for_test()
            .unwrap();
    }

    fn ui_harness(dialog: Dialog, width: f32) -> Harness<'static, (Dialog, Option<Action>)> {
        Harness::builder()
            .with_size(Vec2::new(width, 440.0))
            .build_ui_state(
                |ui, state: &mut (Dialog, Option<Action>)| {
                    if let Some(action) = show(ui, &mut state.0) {
                        if action == Action::CancelGeneration {
                            state.0.cancel_generation();
                        }
                        state.1 = Some(action);
                    }
                },
                (dialog, None),
            )
    }
    #[test]
    fn one_page_validates_all_paths_and_options_before_starting_a_worker() {
        let context = egui::Context::default();
        let mut dialog = Dialog::new(None, Default::default());
        dialog.start(&context);
        assert!(dialog.error.as_deref().unwrap().contains("older"));
        assert!(!dialog.operation.generating());
        dialog.older = "/not-on-disk/old.tex".into();
        dialog.newer = "/not-on-disk/new.tex".into();
        dialog.destination = "/not-on-disk/review.txt".into();
        dialog.start(&context);
        assert!(dialog.error.as_deref().unwrap().contains(".tex"));
        assert!(!dialog.operation.generating());
        dialog.destination = "/not-on-disk/review.tex".into();
        dialog.settings.command.arguments = "--math-markup=1".into();
        // Preparing the complete request never reads these nonexistent paths.
        assert!(dialog.request().is_ok());
        assert_eq!(
            dialog.destination().unwrap(),
            Path::new("/not-on-disk/review.tex")
        );
        dialog.settings.command.environment = "[]".into();
        dialog.start(&context);
        assert!(dialog.error.is_some());
        assert!(!dialog.operation.generating());
        for value in ["relative.tex", "/tmp/file.pdf", "/tmp/file.tex\0"] {
            assert!(validate_tex_path(value, "input").is_err());
        }
        assert!(validate_tex_path("/tmp/paper.TEX", "input").is_ok());
        for extension in crate::document::TEX_SOURCE_EXTENSIONS {
            assert!(
                validate_tex_path(
                    &format!("/tmp/source.{}", extension.to_ascii_uppercase()),
                    "input"
                )
                .is_ok()
            );
        }
    }

    #[test]
    fn editor_snapshot_only_applies_to_the_selected_newer_document() {
        let mut dialog = fixture();
        assert_eq!(
            dialog.request().unwrap().source.as_deref(),
            Some("unsaved text")
        );
        dialog.include_edits = false;
        assert!(dialog.request().unwrap().source.is_none());
        dialog.include_edits = true;
        dialog.newer = "/tmp/another.tex".into();
        assert!(dialog.request().unwrap().source.is_none());
        dialog.older.clear();
        assert!(dialog.request().unwrap_err().contains("Choose"));
    }
    #[test]
    fn one_page_exposes_every_path_and_option_without_navigation() {
        let mut harness = ui_harness(fixture(), 720.0);
        harness.run();
        for name in [
            "Older file",
            "Newer file",
            "Output file",
            "Arguments",
            "Markup",
            "Include subfiles",
            "Create diff",
        ] {
            assert!(harness.query_by_label(name).is_some(), "missing {name}");
        }
        assert!(harness.query_by_label("Next").is_none());
        assert!(harness.query_by_label("Back").is_none());
        harness.get_by_label("Older file").click();
        harness.run();
        harness.get_by_label("Older file").type_text("x");
        harness.run();
        assert!(harness.state().0.older.contains('x'));
        harness.get_by_label("Arguments").click();
        harness.run();
        harness
            .get_by_label("Arguments")
            .type_text("--math-markup=1");
        harness.run();
        assert_eq!(
            harness.state().0.settings.command.arguments,
            "--math-markup=1"
        );
        harness.get_by_label("Output file").click();
        harness.run();
        harness.get_by_label("Output file").type_text("x");
        harness.run();
        assert!(harness.state().0.destination_edited);
        assert!(harness.state().0.older.contains('x'));
        assert_eq!(
            harness.state().0.settings.command.arguments,
            "--math-markup=1"
        );
    }

    #[test]
    fn a_newer_path_updates_default_output_but_preserves_a_custom_output() {
        assert_eq!(
            suggested_destination("/tmp/paper.v2.tex"),
            "/tmp/paper.v2-diff.tex"
        );
        assert_eq!(suggested_destination(""), "");
        let mut harness = ui_harness(fixture(), 720.0);
        harness.run();
        harness.get_by_label("Newer file").click();
        harness.run();
        harness.get_by_label("Newer file").type_text("x");
        harness.run();
        assert_eq!(
            harness.state().0.destination,
            suggested_destination(&harness.state().0.newer)
        );
        harness.state_mut().0.destination_edited = true;
        harness.state_mut().0.destination = "/tmp/custom.tex".into();
        harness.get_by_label("Newer file").type_text("y");
        harness.run();
        assert_eq!(harness.state().0.destination, "/tmp/custom.tex");
    }
    #[test]
    fn pending_worker_is_cancellable_without_waiting_or_starting_a_second_job() {
        let mut dialog = fixture();
        let (started_tx, started_rx) = std::sync::mpsc::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let token = controlled_generation(&mut dialog, move || {
            started_tx.send(()).unwrap();
            release_rx.recv().unwrap();
            Err("LaTeXdiff canceled".into())
        });
        started_rx.recv().unwrap();
        dialog.start(&egui::Context::default());
        assert!(
            Arc::ptr_eq(&token, &dialog.operation.generation_mut().unwrap().cancel.0),
            "busy start must not replace or duplicate the worker"
        );
        let mut harness = ui_harness(dialog, 720.0);
        harness.run_steps(2);
        assert!(harness.query_by_label("Create diff").is_none());
        for name in [
            "Older file",
            "Newer file",
            "Output file",
            "Arguments",
            "Include subfiles",
        ] {
            assert!(
                harness.get_by_label(name).accesskit_node().is_disabled(),
                "generation must protect {name} from edits"
            );
        }
        harness.get_by_label("Cancel").click();
        harness.run_steps(2);
        assert!(token.load(Ordering::Acquire));
        assert!(harness.state().0.busy());
        harness.state_mut().0.cancel_generation();
        assert!(
            harness.state().0.busy(),
            "repeated cancellation must retain the worker until it drains"
        );
        assert!(matches!(
            harness.state().0.operation,
            Operation::Cancelling(_)
        ));
        assert!(
            harness
                .get_by_label("Cancel")
                .accesskit_node()
                .is_disabled()
        );
        release_tx.send(()).unwrap();
        wait_for_generation(&mut harness.state_mut().0);
        assert!(
            harness
                .state_mut()
                .0
                .poll(&egui::Context::default())
                .is_none()
        );
        assert!(!harness.state().0.busy());
        assert!(
            harness
                .state()
                .0
                .error
                .as_deref()
                .unwrap()
                .contains("canceled")
        );
        harness.run_steps(2);
        assert!(harness.query_by_label("Create diff").is_some());
        assert!(matches!(harness.state().0.operation, Operation::Idle));
        drop(harness);
        assert!(token.load(Ordering::Acquire));
    }

    #[test]
    fn cancelling_a_file_picker_returns_to_the_existing_editable_form() {
        let context = egui::Context::default();
        let mut dialog = fixture();
        dialog.operation =
            Operation::Picking(PendingDialog::new(Action::Older, std::future::ready(None)));
        assert!(dialog.busy());
        assert!(dialog.poll(&context).is_none());
        assert!(!dialog.busy());
        assert_eq!(dialog.older, "/tmp/older.tex");
        assert_eq!(dialog.newer, "/tmp/paper.tex");
        assert!(dialog.poll(&context).is_none());
    }
    #[test]
    fn successful_creation_is_handed_off_exactly_once_without_another_button() {
        let context = egui::Context::default();
        let mut dialog = fixture();
        dialog.settings.command.directory = "/tmp/project".into();
        controlled_generation(&mut dialog, || {
            Ok(Completed {
                path: "/tmp/paper-diff.tex".into(),
                output: diff::Output {
                    source: "generated".into(),
                    warnings: "A tool message".into(),
                    older: "/tmp/older.tex".into(),
                    newer: "/tmp/paper.tex".into(),
                },
            })
        });
        wait_for_generation(&mut dialog);
        let completed = dialog
            .poll(&context)
            .expect("creation must trigger its handoff");
        assert_eq!(completed.path, Path::new("/tmp/paper-diff.tex"));
        assert_eq!(completed.output.source, "generated");
        assert_eq!(completed.output.warnings, "A tool message");
        assert_eq!(dialog.validated_directory.as_deref(), Some("/tmp/project"));
        assert!(
            dialog.poll(&context).is_none(),
            "a receipt opens and compiles only once"
        );
        assert_eq!(dialog.older, "/tmp/older.tex");
        assert_eq!(dialog.newer, "/tmp/paper.tex");
    }

    #[test]
    fn failed_generation_keeps_the_same_form_and_drafts_for_correction() {
        let context = egui::Context::default();
        let mut dialog = fixture();
        controlled_generation(&mut dialog, || Err("The output file already exists".into()));
        wait_for_generation(&mut dialog);
        assert!(dialog.poll(&context).is_none());
        let mut harness = ui_harness(dialog, 720.0);
        harness.run();
        assert!(
            harness
                .query_by_label("The output file already exists")
                .is_some()
        );
        assert!(harness.query_by_label("Create diff").is_some());
        assert_eq!(harness.state().0.older, "/tmp/older.tex");
        assert_eq!(harness.state().0.newer, "/tmp/paper.tex");
        assert_eq!(harness.state().0.destination, "/tmp/paper-diff.tex");
    }

    #[test]
    fn changing_options_clears_a_previous_error_without_discarding_the_form() {
        let mut dialog = fixture();
        dialog.error = Some("An invalid option".into());
        let mut harness = ui_harness(dialog, 720.0);
        harness.run();
        harness.get_by_label("Include subfiles").click();
        harness.run();
        assert!(harness.state().0.error.is_none());
        assert_eq!(harness.state().0.older, "/tmp/older.tex");
        assert_eq!(harness.state().0.newer, "/tmp/paper.tex");
        assert_eq!(harness.state().0.destination, "/tmp/paper-diff.tex");
    }

    #[test]
    fn single_page_keeps_actions_visible_with_narrow_width_long_errors_and_advanced_options() {
        for width in [320.0, 660.0] {
            let mut dialog = fixture();
            dialog.error = Some("A comparison error. ".repeat(200));
            let mut harness = ui_harness(dialog, width);
            harness.run();
            let footer = harness.get_by_label("Create diff").rect();
            assert!(
                footer.left() >= 0.0 && footer.right() <= width && footer.bottom() <= 440.0,
                "{width}: {footer:?}"
            );
            for name in ["Older file", "Newer file", "Output file", "Arguments"] {
                let rect = harness.get_by_label(name).rect();
                assert!(
                    rect.width() > 200.0 && rect.left() >= 0.0 && rect.right() <= width,
                    "{name}: {rect:?}"
                );
            }
            harness.get_by_label("Advanced").click();
            harness.run();
            assert_eq!(harness.get_by_label("Create diff").rect(), footer);
            for name in ["Executable", "Environment", "Working directory"] {
                let rect = harness.get_by_label(name).rect();
                assert!(
                    rect.left() >= 0.0 && rect.right() <= width,
                    "{name}: {rect:?}"
                );
            }
        }
        for width in [100.0, 320.0, 1200.0] {
            assert!(card_width(width) + 32.0 <= width);
        }
        for height in [100.0, 320.0, 600.0] {
            assert!(card_height(height) + 32.0 <= height);
        }
    }

    #[test]
    fn documentation_uses_the_existing_child_safe_link_route() {
        let mut harness = ui_harness(fixture(), 720.0);
        harness.run();
        harness.get_by_label("Advanced").click();
        harness.run();
        harness.get_by_label("LaTeXdiff documentation").click();
        harness.run();
        let url = harness.ctx.data(|data| {
            data.get_temp::<String>(egui::Id::new((
                "settings-documentation",
                harness.ctx.viewport_id(),
            )))
        });
        assert_eq!(url.as_deref(), Some(diff::DOCS));
    }
    #[test]
    fn typing_in_a_modal_field_cannot_edit_source_or_execute_parent_commands() {
        let root = tempfile::tempdir().unwrap();
        let context = egui::Context::default();
        let mut app = EditorApp::dormant_for_tests(&context, root.path().into());
        app.document_mut().replace_loaded_unprojected(
            "untouched".into(),
            root.path().join("paper.tex"),
            DocumentKind::Tex,
            None,
        );
        app.begin_latexdiff(&context);
        assert!(app.document_flow_busy());
        app.execute_app_command(AppCommand::ToggleComment, &context, None);
        app.execute_app_command(AppCommand::New, &context, None);
        app.execute_app_command(AppCommand::Find, &context, None);
        assert_eq!(app.document().source(), "untouched");
        assert!(!app.find_bar.visible);
        let mut harness = Harness::builder()
            .with_size(Vec2::new(720.0, 600.0))
            .build_ui_state(
                |ui, dialog| {
                    let _ = show(ui, dialog);
                },
                app.latexdiff.take().unwrap(),
            );
        harness.run();
        harness.get_by_label("Older file").click();
        harness.run();
        harness.get_by_label("Older file").type_text("x");
        harness.run();
        assert!(harness.state().older.contains('x'));
        app.latexdiff = Some(fixture());
        app.execute_app_command(AppCommand::CloseTab, &context, None);
        assert!(app.latexdiff.is_none());
        assert_eq!(app.document().source(), "untouched");
    }
    #[test]
    fn escape_and_cmd_w_close_before_field_keyboard_normalization() {
        let shortcuts = ShortcutBindings::defaults(ShortcutPlatform::MacOs);
        for (key, modifiers) in [
            (egui::Key::Escape, egui::Modifiers::NONE),
            (egui::Key::W, egui::Modifiers::COMMAND),
            (
                egui::Key::W,
                egui::Modifiers::COMMAND | egui::Modifiers::SHIFT,
            ),
        ] {
            let context = egui::Context::default();
            let mut closed = false;
            let mut output = context.run_ui(
                egui::RawInput {
                    events: vec![egui::Event::Key {
                        key,
                        physical_key: None,
                        pressed: true,
                        repeat: false,
                        modifiers,
                    }],
                    ..Default::default()
                },
                |ui| {
                    closed = close_requested(
                        ui.ctx(),
                        crate::child_view::ChildViewInput {
                            escape_pressed: ui.input(|input| input.key_pressed(egui::Key::Escape)),
                            ..Default::default()
                        },
                        &shortcuts,
                    );
                    let _edits = prepare_child_widget_edit_events(ui.ctx(), &shortcuts, false);
                },
            );
            output.textures_delta.clear();
            assert!(closed, "{key:?} {modifiers:?}");
        }
    }
    #[test]
    fn minimize_in_the_modal_is_routed_to_the_document_owner() {
        let context = egui::Context::default();
        context.set_embed_viewports(false);
        let owner = egui::ViewportId::from_hash_of("document-owner");
        crate::window_host::set_management_owner(&context, egui::ViewportId::ROOT, Some(owner));
        let shortcut = ShortcutBindings::defaults(ShortcutPlatform::MacOs);
        let key = shortcut.egui(ShortcutAction::Minimize).unwrap();
        let mut output = context.run_ui(
            egui::RawInput {
                events: vec![egui::Event::Key {
                    key: key.logical_key,
                    physical_key: None,
                    pressed: true,
                    repeat: false,
                    modifiers: key.modifiers,
                }],
                ..Default::default()
            },
            |ui| {
                ui.ctx()
                    .show_viewport_deferred(owner, egui::ViewportBuilder::default(), |_, _| {});
                handle_window_management_shortcuts(ui.ctx(), ui.ctx().viewport_id(), &shortcut);
                let _edits = prepare_child_widget_edit_events(ui.ctx(), &shortcut, false);
            },
        );
        output.textures_delta.clear();
        assert!(
            output.viewport_output[&owner]
                .commands
                .contains(&egui::ViewportCommand::Minimized(true))
        );
        assert!(
            !output.viewport_output[&egui::ViewportId::ROOT]
                .commands
                .contains(&egui::ViewportCommand::Minimized(true))
        );
        output.textures_delta.clear();
    }
    #[test]
    fn json_settings_explain_invalid_markup_and_environment() {
        let mut value = serde_json::to_value(AppSettings::default()).unwrap();
        value["latexdiff"]["markup"] = serde_json::json!("rainbow");
        let error = crate::settings_json::parse(&value.to_string()).unwrap_err();
        assert!(
            error.contains("latexdiff.markup") && error.contains("choose one of"),
            "{error}"
        );
        value["latexdiff"]["markup"] = serde_json::json!("underline");
        value["latexdiff"]["command"]["environment"] = serde_json::json!("[]");
        assert!(
            crate::settings_json::parse(&value.to_string())
                .unwrap_err()
                .contains("latexdiff")
        );
    }
    #[test]
    fn closing_dialog_persists_options_without_restarting_tex_or_preview() {
        let root = tempfile::tempdir().unwrap();
        let context = egui::Context::default();
        let mut app = EditorApp::dormant_for_tests(&context, root.path().into());
        let theme = crate::presentation::active_theme_request(&app.settings, None, None);
        let before = crate::presentation::ResolvedPresentationRequest::resolve(
            &app.settings,
            theme.clone(),
            0,
        );
        app.begin_latexdiff(&context);
        app.latexdiff.as_mut().unwrap().settings.markup = diff::Markup::Color;
        app.close_latexdiff(&context);
        let pending = app.pending_settings.as_ref().expect("options persisted");
        assert_eq!(pending.latexdiff.markup, diff::Markup::Color);
        let after = crate::presentation::ResolvedPresentationRequest::resolve(pending, theme, 0);
        assert_eq!(
            before, after,
            "comparison options have no presentation or service effects"
        );
        assert!(app.latexdiff.is_none());
    }
    #[test]
    fn closing_a_draft_cannot_save_an_unverified_working_directory() {
        let root = tempfile::tempdir().unwrap();
        let context = egui::Context::default();
        let mut app = EditorApp::dormant_for_tests(&context, root.path().into());
        app.begin_latexdiff(&context);
        app.latexdiff.as_mut().unwrap().settings.command.directory =
            root.path().join("absent").to_string_lossy().into_owned();
        app.close_latexdiff(&context);
        assert!(app.pending_settings.is_none());
        assert!(app.settings.latexdiff.command.directory.is_empty());
    }
    #[test]
    fn a_custom_shortcut_opens_the_modal_from_the_editor() {
        let root = tempfile::tempdir().unwrap();
        let context = egui::Context::default();
        let mut app = EditorApp::dormant_for_tests(&context, root.path().into());
        app.document_mut().replace_loaded_unprojected(
            "source".into(),
            root.path().join("paper.tex"),
            DocumentKind::Tex,
            None,
        );
        let chord = crate::shortcuts::ShortcutChord::primary(egui::Key::F9);
        app.settings.shortcut_overrides.assign(
            ShortcutAction::LatexDiff,
            chord,
            ShortcutPlatform::current(),
        );
        let key = chord.egui();
        let mut output = context.run_ui(
            egui::RawInput {
                events: vec![egui::Event::Key {
                    key: key.logical_key,
                    physical_key: None,
                    pressed: true,
                    repeat: false,
                    modifiers: key.modifiers,
                }],
                ..Default::default()
            },
            |ui| app.handle_shortcuts(ui.ctx(), None),
        );
        output.textures_delta.clear();
        assert!(app.latexdiff.is_some());
        assert_eq!(app.document().source(), "source");
    }
    #[test]
    fn footer_reserves_every_wrapped_row_including_busy_spinner() {
        assert_eq!(wrapped_rows(166.0, &[80.0, 80.0], 6.0), 1);
        assert_eq!(wrapped_rows(165.0, &[80.0, 80.0], 6.0), 2);
        assert_eq!(wrapped_rows(100.0, &[80.0, 80.0, 80.0], 6.0), 3);
        assert_eq!(wrapped_rows(240.0, &[70.0, 70.0, 70.0], 6.0), 1);
        assert_eq!(wrapped_rows(240.0, &[70.0, 70.0, 70.0, 20.0], 6.0), 2);
    }
    #[test]
    #[ignore = "captures fresh offscreen form framebuffers for visual review"]
    #[cfg(target_os = "macos")]
    fn modal_framebuffer_review() {
        for dark in [false, true] {
            {
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
                let mut framebuffer = crate::test_framebuffer::Framebuffer::new([820, 600]);
                let destination = PathBuf::from(format!(
                    ".tiptoptyp/screenshots/agent-review/latexdiff/{}-form.png",
                    if dark { "dark" } else { "light" },
                ));
                std::fs::create_dir_all(destination.parent().unwrap()).unwrap();
                for frame in 0..4 {
                    let output = context.run_ui(
                        egui::RawInput {
                            screen_rect: Some(Rect::from_min_size(
                                Pos2::ZERO,
                                Vec2::new(820.0, 600.0),
                            )),
                            time: Some(frame as f64 / 60.0),
                            ..Default::default()
                        },
                        |ui| {
                            ui.painter()
                                .rect_filled(ui.max_rect(), 0.0, ui.visuals().panel_fill);
                            egui::Area::new(egui::Id::new("review-card"))
                                .anchor(egui::Align2::CENTER_CENTER, Vec2::ZERO)
                                .show(ui.ctx(), |ui| {
                                    theme::dialog_card_frame(ui.style()).show(ui, |ui| {
                                        ui.set_width(660.0);
                                        ui.set_height(card_height(600.0));
                                        let _ = show(ui, &mut dialog);
                                    });
                                });
                        },
                    );
                    framebuffer.render(
                        &context,
                        output,
                        (frame == 3).then_some(destination.as_path()),
                    );
                }
            }
        }
    }
}
