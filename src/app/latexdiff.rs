//! LaTeXdiff uses the same owned modal host and keyboard contract as app dialogs.
use super::*;
use crate::tex::diff::{self, Cancellation, Output, Request};

pub(super) const SALT: &str = "tiptoptyp-latexdiff";
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Action {
    Older,
    Newer,
    Generate,
    Copy,
    Save,
    Close,
}
struct Selection(PathBuf);
impl crate::worker::OperationSummary for Selection {
    fn completion_summary(&self) -> String {
        format!("Saved {}", self.0.display())
    }
}
pub(super) struct Dialog {
    older: String,
    newer: String,
    current: Option<(PathBuf, String)>,
    include_edits: bool,
    focus_file: bool,
    settings: diff::Settings,
    output: Option<Output>,
    error: Option<String>,
    generate: LatestJob<Output>,
    selection: crate::worker::ExclusiveJob<Selection>,
    picker: Option<PendingDialog<Action>>,
    cancel: Cancellation,
    pub(super) edit_events: Vec<egui::Event>,
}
impl Dialog {
    fn new(current: Option<(PathBuf, String)>, settings: diff::Settings) -> Self {
        let path = current
            .as_ref()
            .map(|(path, _)| path.to_string_lossy().into_owned())
            .unwrap_or_default();
        Self {
            older: path.clone(),
            newer: path,
            current,
            include_edits: true,
            focus_file: true,
            settings,
            output: None,
            error: None,
            generate: LatestJob::default(),
            selection: crate::worker::ExclusiveJob::default(),
            picker: None,
            cancel: Cancellation::default(),
            edit_events: Vec::new(),
        }
    }
    fn busy(&self) -> bool {
        self.generate.is_running() || self.selection.is_running() || self.picker.is_some()
    }
    fn request(&self) -> Result<Request, String> {
        if self.older.trim().is_empty() || self.newer.trim().is_empty() {
            return Err("Choose an older and a newer TeX file.".into());
        }
        self.settings.validate()?;
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
    fn poll(&mut self, context: &egui::Context) -> Option<PathBuf> {
        if let DialogPoll::Ready {
            request: action,
            selection: Some(file),
        } = crate::workflow::poll_dialog(&mut self.picker, context)
        {
            match action {
                Action::Older | Action::Newer => {
                    let value = file.path().to_string_lossy().into_owned();
                    if action == Action::Older {
                        self.older = value;
                    } else {
                        self.newer = value;
                    }
                    self.output = None;
                    self.error = None;
                }
                Action::Save => {
                    if let Some(output) = self.output.clone() {
                        let path = crate::workflow::source_save_path(
                            file.path().into(),
                            DocumentKind::Tex,
                        );
                        if let Err(error) =
                            self.selection
                                .start_and_repaint("Save LaTeXdiff", context, move || {
                                    output.save(&path).map(Selection)
                                })
                        {
                            self.error = Some(error);
                        }
                    }
                }
                _ => unreachable!(),
            }
        }
        match self.generate.poll() {
            LatestJobPoll::Ready(output) => {
                self.output = Some(output);
                self.error = None;
            }
            LatestJobPoll::Failed(error) => self.error = Some(error),
            _ => {}
        }
        match self.selection.poll() {
            LatestJobPoll::Ready(Selection(path)) => return Some(path),
            LatestJobPoll::Failed(error) => self.error = Some(error),
            _ => {}
        }
        None
    }
}

impl EditorApp {
    pub(super) fn begin_latexdiff(&mut self, context: &egui::Context) {
        if self.document_flow_busy() {
            return;
        }
        let current = (self.document().kind() == DocumentKind::Tex)
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
            if dialog.settings.validate().is_ok() && dialog.settings != self.settings.latexdiff {
                let mut edited = self.settings.clone();
                edited.latexdiff = dialog.settings.clone();
                self.queue_settings(edited, context);
            }
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
        if let Some(path) = dialog.poll(context) {
            self.close_latexdiff(context);
            if self.open_tab_path(path, context) {
                self.select_preview_tab(self.tabs.active_id().expect("opened diff tab"), context);
            }
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
            ChildViewSpec::modal(SALT, "LaTeXdiff", owner, "latexdiff"),
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
                            egui::ScrollArea::vertical()
                                .max_height((owner.height() - 48.0).max(120.0))
                                .show(ui, |ui| {
                                    if let Some(clicked) = show(ui, dialog) {
                                        action = Some(clicked);
                                    }
                                });
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
            Some(Action::Generate) => {
                let dialog = self.latexdiff.as_mut().unwrap();
                match dialog.request() {
                    Ok(request) => {
                        let cancel = dialog.cancel.0.clone();
                        dialog.output = None;
                        dialog.error = None;
                        if let Err(error) =
                            dialog
                                .generate
                                .start_and_repaint("latexdiff", context, move || {
                                    diff::generate(request, &cancel)
                                })
                        {
                            dialog.error = Some(error);
                        }
                    }
                    Err(error) => dialog.error = Some(error),
                }
            }
            Some(Action::Copy) => {
                if let Some(output) = &self.latexdiff.as_ref().unwrap().output {
                    context.copy_text(output.source.clone());
                }
            }
            Some(action @ (Action::Older | Action::Newer | Action::Save)) => {
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
        let newer = Path::new(&dialog.newer);
        if let Some(parent) = newer.parent().filter(|p| p.is_dir()) {
            picker = picker.set_directory(parent);
        }
        picker = picker.add_filter("LaTeX source", &["tex"]);
        dialog.picker = Some(if action == Action::Save {
            let Some(output) = &dialog.output else {
                return;
            };
            picker = picker
                .set_title("Save LaTeXdiff and open for compilation")
                .set_file_name(output.suggested_name());
            PendingDialog::new(action, picker.save_file())
        } else {
            picker = picker.set_title(if action == Action::Older {
                "Choose older TeX file"
            } else {
                "Choose newer TeX file"
            });
            PendingDialog::new(action, picker.pick_file())
        });
    }
}

fn card_width(owner_width: f32) -> f32 {
    (owner_width - 48.0).clamp(1.0, 660.0)
}

fn close_requested(
    context: &egui::Context,
    input: crate::child_view::ChildViewInput,
    shortcuts: &ShortcutBindings,
) -> bool {
    // Consume close before the common field normalizer defers non-edit keys.
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

fn show(ui: &mut egui::Ui, dialog: &mut Dialog) -> Option<Action> {
    let mut action = None;
    ui.horizontal(|ui| {
        ui.heading("LaTeXdiff");
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            if icon_button(ui, UiIcon::Close, "Close LaTeXdiff").clicked() {
                action = Some(Action::Close);
            }
        });
    });
    ui.label("Mark additions and deletions between two LaTeX documents.");
    ui.separator();
    let busy = dialog.busy();
    let mut changed = false;
    ui.add_enabled_ui(!busy, |ui| {
        for (label, value, pick) in [("Older file", &mut dialog.older, Action::Older), ("Newer file", &mut dialog.newer, Action::Newer)] {
            let label_response = ui.label(label);
            ui.horizontal(|ui| {
                let field = ui.add(egui::TextEdit::singleline(value).hint_text("/path/to/document.tex").desired_width((ui.available_width() - 88.0).max(100.0)));
                let field = field.labelled_by(label_response.id);
                if pick == Action::Older && dialog.focus_file { field.request_focus(); dialog.focus_file = false; }
                changed |= field.changed();
                if ui.button("Choose…").on_hover_text(format!("Choose {label}")).clicked() { action = Some(pick); }
            });
        }
        if dialog.current.as_ref().is_some_and(|(path, _)| path == Path::new(dialog.newer.trim())) {
            changed |= ui.checkbox(&mut dialog.include_edits, "Include unsaved editor changes").on_hover_text("The older file is read from disk; the newer file uses a snapshot of the current editor.").changed();
        }
        changed |= preferences(ui, &mut dialog.settings);
    });
    if changed {
        dialog.output = None;
        dialog.error = None;
    }
    if let Some(error) = &dialog.error {
        ui.colored_label(ui.visuals().error_fg_color, error);
    }
    if let Some(output) = &dialog.output {
        ui.label(format!("Diff source ready · {} bytes", output.source.len()));
        if !output.warnings.trim().is_empty() {
            egui::CollapsingHeader::new("LaTeXdiff messages").show(ui, |ui| {
                ui.label(&output.warnings);
            });
        }
    }
    ui.separator();
    ui.horizontal_wrapped(|ui| {
        if ui.add_enabled(!busy, egui::Button::new("Generate diff")).clicked() { action = Some(Action::Generate); }
        if ui.add_enabled(!busy && dialog.output.is_some(), egui::Button::new("Save and open…")).on_hover_text("Save the marked-up .tex file and open it with its PDF preview. Save beside the newer file to preserve image and bibliography paths. Your selected TeX engine builds its PDF.").clicked() { action = Some(Action::Save); }
        if ui.add_enabled(!busy && dialog.output.is_some(), egui::Button::new("Copy source")).clicked() { action = Some(Action::Copy); }
        if busy { ui.spinner(); }
    });
    action
}

pub(super) fn preferences(ui: &mut egui::Ui, settings: &mut diff::Settings) -> bool {
    let mut changed = false;
    ui.horizontal_wrapped(|ui| {
        ui.label("Markup");
        egui::ComboBox::from_id_salt("latexdiff-markup").selected_text(settings.markup.label()).show_ui(ui, |ui| {
            for markup in diff::Markup::ALL { changed |= ui.selectable_value(&mut settings.markup, markup, markup.label()).changed(); }
        });
        changed |= ui.checkbox(&mut settings.flatten, "Include subfiles").on_hover_text("Expand \\input and \\include before comparing (--flatten). Both projects' included files must be available.").changed();
    });
    egui::CollapsingHeader::new("LaTeXdiff options").show(ui, |ui| {
        for (name, value, example) in [
            ("Executable", &mut settings.executable, "Blank finds latexdiff in PATH or MacTeX; otherwise enter its full path."),
            ("Arguments", &mut settings.command.arguments, "Example: --math-markup=1. Mark whole equations rather than individual mathematical terms. Input paths, style and include expansion are supplied by the app."),
            ("Environment", &mut settings.command.environment, "Example: {\"LC_ALL\":\"C\"}. Use a predictable process locale. Enter a JSON object of strings."),
            ("Working directory", &mut settings.command.directory, "Example: /Users/me/paper. Blank uses the newer file's directory."),
        ] {
            let label = preference_help(ui.label(name), example);
            let field = preference_help(ui.add(egui::TextEdit::singleline(value).desired_width(f32::INFINITY)), example);
            let field = field.labelled_by(label.id);
            changed |= field.changed();
        }
        super::settings_controls::documentation_link(ui, "LaTeXdiff documentation", diff::DOCS);
        if ui.button("Reset LaTeXdiff options").clicked() { *settings = diff::Settings::default(); changed = true; }
        if let Err(error) = settings.validate() { ui.colored_label(ui.visuals().error_fg_color, error); }
    });
    changed
}

fn preference_help(response: egui::Response, example: &str) -> egui::Response {
    response.on_hover_ui(|ui| {
        ui.label(example);
        super::settings_controls::documentation_link(ui, "LaTeXdiff documentation", diff::DOCS);
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui_kittest::{
        Harness,
        kittest::{NodeT, Queryable},
    };
    fn fixture() -> Dialog {
        Dialog::new(
            Some((PathBuf::from("/tmp/paper.tex"), "unsaved text".into())),
            diff::Settings::default(),
        )
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
    fn controls_use_editor_changes_and_options_invalidate_generated_output() {
        let mut harness = Harness::builder()
            .with_size(Vec2::new(720.0, 600.0))
            .build_ui_state(
                |ui, dialog| {
                    let _ = show(ui, dialog);
                },
                fixture(),
            );
        harness.run();
        assert!(
            harness
                .get_by_label("Save and open…")
                .accesskit_node()
                .is_disabled()
        );
        harness
            .get_by_label("Include unsaved editor changes")
            .click();
        harness.run();
        assert!(!harness.state().include_edits);
        harness.state_mut().output = Some(Output {
            source: "generated".into(),
            warnings: String::new(),
            older: "/tmp/old.tex".into(),
            newer: "/tmp/paper.tex".into(),
        });
        harness.run();
        assert!(
            !harness
                .get_by_label("Save and open…")
                .accesskit_node()
                .is_disabled()
        );
        harness.get_by_label("Include subfiles").click();
        harness.run();
        assert!(!harness.state().settings.flatten);
        assert!(harness.state().output.is_none());
        assert!(
            harness
                .get_by_label("Copy source")
                .accesskit_node()
                .is_disabled()
        );
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
    fn card_fits_narrow_owners_and_expanded_options_wrap() {
        for width in [100.0, 320.0, 660.0, 1200.0] {
            assert!(card_width(width) + 32.0 <= width);
        }
        let mut harness = Harness::builder()
            .with_size(Vec2::new(320.0, 600.0))
            .build_ui_state(
                |ui, dialog| {
                    let _ = show(ui, dialog);
                },
                fixture(),
            );
        harness.run();
        harness.get_by_label("LaTeXdiff options").click();
        harness.run();
        for label in [
            "Generate diff",
            "Save and open…",
            "Copy source",
            "Arguments",
            "Environment",
            "Working directory",
        ] {
            let rect = harness.get_by_label(label).rect();
            assert!(
                rect.right() <= 320.0 && rect.left() >= 0.0,
                "{label}: {rect:?}"
            );
        }
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
    fn documentation_uses_the_existing_child_safe_link_route() {
        let mut harness = Harness::builder()
            .with_size(Vec2::new(720.0, 640.0))
            .build_ui_state(
                |ui, dialog| {
                    let _ = show(ui, dialog);
                },
                fixture(),
            );
        harness.run();
        harness.get_by_label("LaTeXdiff options").click();
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
    fn copied_result_is_available_without_touching_disk() {
        let mut dialog = fixture();
        dialog.output = Some(Output {
            source: "\\DIFadd{new}".into(),
            warnings: String::new(),
            older: "/tmp/old.tex".into(),
            newer: "/tmp/new.tex".into(),
        });
        let mut harness = Harness::builder()
            .with_size(Vec2::new(720.0, 600.0))
            .build_ui_state(
                |ui, state: &mut (Dialog, Option<Action>)| {
                    state.1 = show(ui, &mut state.0);
                },
                (dialog, None),
            );
        harness.run();
        harness.get_by_label("Copy source").click();
        harness.step();
        assert_eq!(harness.state().1, Some(Action::Copy));
    }
    #[test]
    #[ignore = "captures fresh offscreen modal framebuffer for visual review"]
    #[cfg(target_os = "macos")]
    fn modal_framebuffer_review() {
        for dark in [false, true] {
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
                ".tiptoptyp/screenshots/agent-review/latexdiff/{}.png",
                if dark { "dark" } else { "light" }
            ));
            std::fs::create_dir_all(destination.parent().unwrap()).unwrap();
            for frame in 0..4 {
                let output = context.run_ui(
                    egui::RawInput {
                        screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(820.0, 600.0))),
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
