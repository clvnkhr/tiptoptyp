//! An owned wizard; filesystem reads, comparison and publication stay on its worker.
use super::*;
use crate::tex::diff::{self, Cancellation, Completed, Output, Request};
use std::sync::atomic::Ordering;

pub(super) const SALT: &str = "tiptoptyp-latexdiff";
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Step {
    Files,
    Output,
    Review,
}
impl Step {
    fn label(self) -> &'static str {
        match self {
            Self::Files => "1 · Files",
            Self::Output => "2 · Output & options",
            Self::Review => "3 · Create",
        }
    }
    #[cfg(all(feature = "desktop-ui-tests", target_os = "macos"))]
    fn id(self) -> &'static str {
        match self {
            Self::Files => "files",
            Self::Output => "output",
            Self::Review => "review",
        }
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Action {
    Older,
    Newer,
    Output,
    Next,
    Back,
    Generate,
    CancelGeneration,
    Copy,
    Open,
    Close,
}
pub(super) struct Dialog {
    older: String,
    newer: String,
    destination: String,
    destination_edited: bool,
    step: Step,
    current: Option<(PathBuf, String)>,
    include_edits: bool,
    focus_file: bool,
    settings: diff::Settings,
    output: Option<Output>,
    completed_path: Option<PathBuf>,
    validated_directory: Option<String>,
    error: Option<String>,
    generate: LatestJob<Completed>,
    picker: Option<PendingDialog<Action>>,
    cancel: Cancellation,
    cancelling: bool,
    pub(super) edit_events: Vec<egui::Event>,
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
            step: Step::Files,
            current,
            include_edits: true,
            focus_file: true,
            settings,
            output: None,
            completed_path: None,
            validated_directory: None,
            error: None,
            generate: LatestJob::default(),
            picker: None,
            cancel: Cancellation::default(),
            cancelling: false,
            edit_events: Vec::new(),
        }
    }
    fn busy(&self) -> bool {
        self.generate.is_running() || self.picker.is_some()
    }
    fn invalidate(&mut self) {
        self.output = None;
        self.completed_path = None;
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
    fn navigate(&mut self, forward: bool) {
        if self.busy() {
            return;
        }
        let result = match (self.step, forward) {
            (Step::Files, true) => self.validate_files().map(|()| Step::Output),
            (Step::Output, true) => self
                .settings
                .validate_syntax()
                .and_then(|()| self.destination().map(|_| Step::Review)),
            (Step::Output, false) => Ok(Step::Files),
            (Step::Review, false) => Ok(Step::Output),
            _ => return,
        };
        match result {
            Ok(step) => {
                self.step = step;
                self.focus_file = true;
                self.error = None;
            }
            Err(error) => self.error = Some(error),
        }
    }
    fn start(&mut self, context: &egui::Context) {
        if self.busy() || self.step != Step::Review {
            return;
        }
        let input = self
            .request()
            .and_then(|request| Ok((request, self.destination()?)));
        match input {
            Ok((request, destination)) => {
                self.cancel = Cancellation::default();
                self.cancelling = false;
                self.invalidate();
                let cancel = self.cancel.0.clone();
                if let Err(error) =
                    self.generate
                        .start_and_repaint("Create LaTeX diff", context, move || {
                            diff::generate_and_save(request, &destination, &cancel)
                        })
                {
                    self.error = Some(error);
                }
            }
            Err(error) => self.error = Some(error),
        }
    }
    fn cancel_generation(&mut self) {
        if self.generate.is_running() {
            self.cancelling = true;
            self.cancel.0.store(true, Ordering::Release);
        }
    }
    fn poll(&mut self, context: &egui::Context) {
        if let DialogPoll::Ready {
            request: action,
            selection: Some(file),
        } = crate::workflow::poll_dialog(&mut self.picker, context)
        {
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
        match self.generate.poll() {
            LatestJobPoll::Ready(completed) => {
                self.output = Some(completed.output);
                self.completed_path = Some(completed.path);
                self.validated_directory = Some(self.settings.command.directory.clone());
                self.error = None;
                self.cancelling = false;
            }
            LatestJobPoll::Failed(error) => {
                self.error = Some(error);
                self.cancelling = false;
            }
            _ => {}
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
    if !path
        .extension()
        .is_some_and(|ext| ext.eq_ignore_ascii_case("tex"))
    {
        return Err(format!("The {role} must have a .tex extension."));
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
        self.format_request_key = None;
        self.manual_format_revision = None;
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
        dialog.poll(context);
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
            Some(Action::Next) => self.latexdiff.as_mut().unwrap().navigate(true),
            Some(Action::Back) => self.latexdiff.as_mut().unwrap().navigate(false),
            Some(Action::Generate) => self.latexdiff.as_mut().unwrap().start(context),
            Some(Action::CancelGeneration) => self.latexdiff.as_mut().unwrap().cancel_generation(),
            Some(Action::Copy) => {
                if let Some(output) = &self.latexdiff.as_ref().unwrap().output {
                    context.copy_text(output.source.clone());
                }
            }
            Some(Action::Open) => {
                if let Some(path) = self.latexdiff.as_ref().unwrap().completed_path.clone() {
                    self.close_latexdiff(context);
                    if self.open_tab_path(path, context) {
                        self.select_preview_tab(
                            self.tabs.active_id().expect("opened diff tab"),
                            context,
                        );
                    }
                }
            }
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
        picker = picker.add_filter("LaTeX source", &["tex"]);
        dialog.picker = Some(if action == Action::Output {
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
            return serde_json::json!({"visible":false,"busy":false,"step":null,"error":null,"output_path":null,
                "older_fingerprint":null,"newer_fingerprint":null,"output_fingerprint":null,"arguments_fingerprint":null});
        };
        serde_json::json!({"visible":true,"busy":dialog.busy(),"step":dialog.step.id(),"error":dialog.error,
            "output_path":dialog.completed_path,"older_fingerprint":crate::desktop_test::fingerprint(&dialog.older),
            "newer_fingerprint":crate::desktop_test::fingerprint(&dialog.newer),
            "output_fingerprint":crate::desktop_test::fingerprint(&dialog.destination),
            "arguments_fingerprint":crate::desktop_test::fingerprint(&dialog.settings.command.arguments)})
    }
}
fn card_width(owner_width: f32) -> f32 {
    (owner_width - 48.0).clamp(1.0, 660.0)
}
fn card_height(owner_height: f32) -> f32 {
    (owner_height - 48.0).clamp(1.0, 330.0)
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
    ui.horizontal_wrapped(|ui| {
        for step in [Step::Files, Step::Output, Step::Review] {
            if step == dialog.step {
                ui.strong(step.label());
            } else {
                ui.weak(step.label());
            }
            if step != Step::Review {
                ui.weak("›");
            }
        }
    });
    ui.separator();
    let busy = dialog.busy();
    let bounds = ui.available_rect_before_wrap();
    let footer_height = footer_height(ui, dialog);
    let footer_top = (bounds.bottom() - footer_height).max(bounds.top());
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
                    ui.add_enabled_ui(!busy, |ui| match dialog.step {
                        Step::Files => {
                            for (label, value, pick, id) in [
                                ("Older file", &mut dialog.older, Action::Older, "older"),
                                ("Newer file", &mut dialog.newer, Action::Newer, "newer"),
                            ] {
                                let field_label = ui.label(label);
                                let choose_width =
                                    text_width(ui, "Choose…") + ui.spacing().button_padding.x * 2.0;
                                ui.horizontal(|ui| {
                                    let width = (ui.available_width()
                                        - choose_width
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
                                        .labelled_by(field_label.id);
                                    observe(id, &field);
                                    if pick == Action::Older && dialog.focus_file {
                                        field.request_focus();
                                        dialog.focus_file = false;
                                    }
                                    if field.changed() {
                                        changed = true;
                                        if pick == Action::Newer && !dialog.destination_edited {
                                            dialog.destination = suggested_destination(value);
                                        }
                                    }
                                    let choose = ui.button("Choose…");
                                    observe(&format!("choose_{id}"), &choose);
                                    if choose.clicked() {
                                        action = Some(pick);
                                    }
                                });
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
                        }
                        Step::Output => {
                            let label = ui.label("Output file");
                            ui.horizontal(|ui| {
                                let width = (ui.available_width()
                                    - text_width(ui, "Choose…")
                                    - ui.spacing().button_padding.x * 2.0
                                    - ui.spacing().item_spacing.x)
                                    .max(1.0);
                                let field = ui
                                    .add_sized(
                                        [width, ui.spacing().interact_size.y],
                                        egui::TextEdit::singleline(&mut dialog.destination)
                                            .id_salt("latexdiff-output")
                                            .hint_text("/path/to/document-diff.tex")
                                            .desired_width(width),
                                    )
                                    .labelled_by(label.id);
                                observe("output", &field);
                                if dialog.focus_file {
                                    field.request_focus();
                                    dialog.focus_file = false;
                                }
                                if field.changed() {
                                    dialog.destination_edited = true;
                                    changed = true;
                                }
                                let choose = ui.button("Choose…");
                                observe("choose_output", &choose);
                                if choose.clicked() {
                                    action = Some(Action::Output);
                                }
                            });
                            changed |= preferences(ui, &mut dialog.settings);
                        }
                        Step::Review => {
                            for (label, value) in [
                                ("Older", dialog.older.as_str()),
                                ("Newer", dialog.newer.as_str()),
                                ("Output", dialog.destination.as_str()),
                            ] {
                                ui.weak(label);
                                ui.label(value);
                            }
                            ui.horizontal_wrapped(|ui| {
                                ui.label(dialog.settings.markup.label());
                                if dialog.settings.flatten {
                                    ui.weak("· Include subfiles");
                                }
                                if dialog.include_edits
                                    && dialog.current.as_ref().is_some_and(|(path, _)| {
                                        path == Path::new(dialog.newer.trim())
                                    })
                                {
                                    ui.weak("· Unsaved editor changes");
                                }
                            });
                            if !dialog.settings.command.arguments.trim().is_empty() {
                                ui.monospace(&dialog.settings.command.arguments);
                            }
                        }
                    });
                    if changed {
                        dialog.invalidate();
                    }
                    if let Some(error) = &dialog.error {
                        ui.colored_label(ui.visuals().error_fg_color, error);
                    }
                    if let Some(output) = &dialog.output {
                        ui.strong(format!("Created diff · {} bytes", output.source.len()));
                        if !output.warnings.trim().is_empty() {
                            egui::CollapsingHeader::new("LaTeXdiff messages").show(ui, |ui| {
                                ui.label(&output.warnings);
                            });
                        }
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
            ui.allocate_ui_with_layout(
                Vec2::new(ui.available_width(), ui.spacing().interact_size.y),
                Layout::right_to_left(Align::Center).with_main_wrap(true),
                |ui| {
                    for (name, id, clicked) in footer_actions(dialog) {
                        let enabled = match clicked {
                            Action::Back | Action::Next | Action::Generate => !busy,
                            Action::CancelGeneration => !dialog.cancelling,
                            _ => true,
                        };
                        let primary =
                            matches!(clicked, Action::Next | Action::Generate | Action::Open);
                        let button = if primary {
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
                    if dialog.generate.is_running() {
                        ui.spinner();
                        ui.weak(if dialog.cancelling {
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
fn footer_actions(dialog: &Dialog) -> Vec<(&'static str, &'static str, Action)> {
    if dialog.generate.is_running() {
        vec![("Cancel", "cancel", Action::CancelGeneration)]
    } else if dialog.step == Step::Review && dialog.completed_path.is_some() {
        vec![
            ("Open diff", "open", Action::Open),
            ("Copy source", "copy", Action::Copy),
            ("Back", "back", Action::Back),
        ]
    } else {
        let mut actions = vec![match dialog.step {
            Step::Review => ("Create diff", "generate", Action::Generate),
            _ => ("Next", "next", Action::Next),
        }];
        if dialog.step != Step::Files {
            actions.push(("Back", "back", Action::Back));
        }
        actions.push(("Cancel", "cancel", Action::Close));
        actions
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
    if dialog.generate.is_running() {
        widths.push(
            ui.spacing().interact_size.y
                + ui.spacing().item_spacing.x
                + text_width(ui, "Creating diff…"),
        );
    }
    let rows = wrapped_rows(ui.available_width(), &widths, ui.spacing().item_spacing.x);
    rows as f32 * (ui.spacing().interact_size.y + ui.spacing().item_spacing.y + 2.0) + 8.0
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
    fn ui_harness(dialog: Dialog, width: f32) -> Harness<'static, (Dialog, Option<Action>)> {
        Harness::builder()
            .with_size(Vec2::new(width, 440.0))
            .build_ui_state(
                |ui, state: &mut (Dialog, Option<Action>)| {
                    if let Some(action) = show(ui, &mut state.0) {
                        match action {
                            Action::Next => state.0.navigate(true),
                            Action::Back => state.0.navigate(false),
                            Action::CancelGeneration => state.0.cancel_generation(),
                            _ => {}
                        }
                        state.1 = Some(action);
                    }
                },
                (dialog, None),
            )
    }
    #[test]
    fn wizard_validates_steps_without_reading_files_and_keeps_drafts() {
        let mut dialog = Dialog::new(None, Default::default());
        dialog.navigate(true);
        assert_eq!(dialog.step, Step::Files);
        assert!(dialog.error.as_deref().unwrap().contains("older"));
        dialog.older = "/not-on-disk/old.tex".into();
        dialog.newer = "/not-on-disk/new.tex".into();
        dialog.destination = suggested_destination(&dialog.newer);
        dialog.navigate(true);
        assert_eq!(dialog.step, Step::Output);
        assert_eq!(dialog.destination, "/not-on-disk/new-diff.tex");
        dialog.destination = "/not-on-disk/review.txt".into();
        dialog.navigate(true);
        assert_eq!(dialog.step, Step::Output);
        assert!(dialog.error.as_deref().unwrap().contains(".tex"));
        dialog.destination = "/not-on-disk/review.tex".into();
        dialog.settings.command.arguments = "--math-markup=1".into();
        dialog.navigate(true);
        assert_eq!(dialog.step, Step::Review);
        dialog.navigate(false);
        dialog.navigate(false);
        assert_eq!(dialog.step, Step::Files);
        assert_eq!(dialog.settings.command.arguments, "--math-markup=1");
        assert_eq!(dialog.destination, "/not-on-disk/review.tex");
        for value in ["relative.tex", "/tmp/file.pdf", "/tmp/file.tex\0"] {
            assert!(validate_tex_path(value, "input").is_err());
        }
        assert!(validate_tex_path("/tmp/paper.TEX", "input").is_ok());
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
    fn keyboard_editing_and_back_next_preserve_path_and_option_drafts() {
        let mut harness = ui_harness(fixture(), 720.0);
        harness.run();
        harness.get_by_label("Older file").click();
        harness.run();
        harness.get_by_label("Older file").type_text("x");
        harness.run();
        assert!(harness.state().0.older.contains('x'));
        harness.state_mut().0.older = "/tmp/before.tex".into();
        harness.run();
        harness.get_by_label("Next").click();
        harness.run();
        assert_eq!(harness.state().0.step, Step::Output);
        harness.get_by_label("Arguments").click();
        harness.run();
        harness
            .get_by_label("Arguments")
            .type_text("--math-markup=1");
        harness.run();
        harness.get_by_label("Next").click();
        harness.run();
        assert_eq!(harness.state().0.step, Step::Review);
        assert!(harness.query_by_label("Create diff").is_some());
        harness.get_by_label("Back").click();
        harness.run();
        assert_eq!(
            harness.state().0.settings.command.arguments,
            "--math-markup=1"
        );
        harness.get_by_label("Back").click();
        harness.run();
        assert_eq!(harness.state().0.older, "/tmp/before.tex");
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
        dialog.step = Step::Review;
        let (started_tx, started_rx) = std::sync::mpsc::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        dialog
            .generate
            .start("controlled comparison", move || {
                started_tx.send(()).unwrap();
                release_rx.recv().unwrap();
                Err("LaTeXdiff canceled".into())
            })
            .unwrap();
        started_rx.recv().unwrap();
        let token = dialog.cancel.0.clone();
        dialog.start(&egui::Context::default());
        assert!(
            Arc::ptr_eq(&token, &dialog.cancel.0),
            "busy start must not replace or duplicate the worker"
        );
        let mut harness = ui_harness(dialog, 720.0);
        harness.run_steps(2);
        assert!(harness.query_by_label("Create diff").is_none());
        assert!(harness.query_by_label("Back").is_none());
        harness.get_by_label("Cancel").click();
        harness.run_steps(2);
        assert!(token.load(Ordering::Acquire));
        assert!(harness.state().0.busy());
        assert!(
            harness
                .get_by_label("Cancel")
                .accesskit_node()
                .is_disabled()
        );
        release_tx.send(()).unwrap();
        harness
            .state_mut()
            .0
            .generate
            .wait_until_ready_for_test()
            .unwrap();
        harness.state_mut().0.poll(&egui::Context::default());
        assert!(!harness.state().0.busy());
        assert!(harness.state().0.completed_path.is_none());
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
        let token = harness.state().0.cancel.0.clone();
        drop(harness);
        assert!(token.load(Ordering::Acquire));
    }
    #[test]
    fn completed_results_can_go_back_and_forward_without_editing_or_regenerating() {
        let mut dialog = fixture();
        dialog.step = Step::Review;
        dialog.completed_path = Some("/tmp/paper-diff.tex".into());
        let mut harness = ui_harness(dialog, 720.0);
        harness.run();
        assert!(harness.query_by_label("Open diff").is_some());
        harness.get_by_label("Back").click();
        harness.run();
        assert_eq!(harness.state().0.step, Step::Output);
        assert!(harness.query_by_label("Open diff").is_none());
        harness.get_by_label("Next").click();
        harness.run();
        assert_eq!(harness.state().0.step, Step::Review);
        assert!(harness.query_by_label("Open diff").is_some());
        assert_eq!(
            harness.state().0.completed_path.as_deref(),
            Some(Path::new("/tmp/paper-diff.tex"))
        );
    }
    #[test]
    fn releasing_cancel_after_completion_cannot_activate_open_diff() {
        let mut dialog = fixture();
        dialog.step = Step::Review;
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        dialog
            .generate
            .start("controlled completion", move || {
                release_rx.recv().unwrap();
                Ok(Completed {
                    path: "/tmp/diff.tex".into(),
                    output: Output {
                        source: "generated".into(),
                        warnings: String::new(),
                        older: "/tmp/older.tex".into(),
                        newer: "/tmp/paper.tex".into(),
                    },
                })
            })
            .unwrap();
        let mut harness = ui_harness(dialog, 720.0);
        harness.run_steps(2);
        let pos = harness.get_by_label("Cancel").rect().center();
        harness.event(egui::Event::PointerMoved(pos));
        harness.event(egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed: true,
            modifiers: egui::Modifiers::NONE,
        });
        harness.step();
        release_tx.send(()).unwrap();
        harness
            .state_mut()
            .0
            .generate
            .wait_until_ready_for_test()
            .unwrap();
        harness.state_mut().0.poll(&egui::Context::default());
        harness.step();
        assert!(harness.get_by_label("Open diff").rect().contains(pos));
        harness.event(egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed: false,
            modifiers: egui::Modifiers::NONE,
        });
        harness.run_steps(2);
        assert_eq!(harness.state().1, None);
    }
    #[test]
    fn changing_options_invalidates_the_completed_result() {
        let mut dialog = fixture();
        dialog.step = Step::Output;
        dialog.completed_path = Some("/tmp/paper-diff.tex".into());
        dialog.output = Some(Output {
            source: "generated".into(),
            warnings: String::new(),
            older: "/tmp/older.tex".into(),
            newer: "/tmp/paper.tex".into(),
        });
        let mut harness = ui_harness(dialog, 720.0);
        harness.run();
        harness.get_by_label("Include subfiles").click();
        harness.run();
        assert!(harness.state().0.output.is_none());
        assert!(harness.state().0.completed_path.is_none());
    }
    #[test]
    fn every_step_keeps_actions_visible_and_fields_inside_narrow_cards() {
        for width in [320.0, 660.0] {
            for step in [Step::Files, Step::Output, Step::Review] {
                let mut dialog = fixture();
                dialog.step = step;
                dialog.error = Some("A comparison error. ".repeat(200));
                let mut harness = ui_harness(dialog, width);
                harness.run();
                let action = if step == Step::Review {
                    "Create diff"
                } else {
                    "Next"
                };
                let rect = harness.get_by_label(action).rect();
                assert!(
                    rect.left() >= 0.0 && rect.right() <= width && rect.bottom() <= 440.0,
                    "{step:?}: {rect:?}"
                );
                if step == Step::Files {
                    for name in ["Older file", "Newer file"] {
                        let r = harness.get_by_label(name).rect();
                        assert!(r.width() > 200.0 && r.right() <= width, "{name}: {r:?}");
                    }
                }
                if step == Step::Output {
                    let footer = rect;
                    harness.get_by_label("Advanced").click();
                    harness.run();
                    assert_eq!(harness.get_by_label("Next").rect(), footer);
                    for name in [
                        "Output file",
                        "Arguments",
                        "Environment",
                        "Working directory",
                    ] {
                        let r = harness.get_by_label(name).rect();
                        assert!(r.left() >= 0.0 && r.right() <= width, "{name}: {r:?}");
                    }
                }
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
        let mut dialog = fixture();
        dialog.step = Step::Output;
        let mut harness = ui_harness(dialog, 720.0);
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
    #[ignore = "captures fresh offscreen wizard framebuffers for visual review"]
    #[cfg(target_os = "macos")]
    fn modal_framebuffer_review() {
        for dark in [false, true] {
            for step in [Step::Files, Step::Output, Step::Review] {
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
                dialog.step = step;
                let mut framebuffer = crate::test_framebuffer::Framebuffer::new([820, 600]);
                let destination = PathBuf::from(format!(
                    ".tiptoptyp/screenshots/agent-review/latexdiff/{}-{}.png",
                    if dark { "dark" } else { "light" },
                    match step {
                        Step::Files => "files",
                        Step::Output => "output",
                        Step::Review => "review",
                    }
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
