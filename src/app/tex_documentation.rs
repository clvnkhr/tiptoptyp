//! Compact local-manual search. Texdoc never launches its own external viewer.
use super::*;
use crate::tex::documentation::{self, SearchResult};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

pub(super) const SALT: &str = "tiptoptyp-tex-documentation";

pub(super) struct Dialog {
    query: String,
    results: Option<SearchResult>,
    error: Option<String>,
    job: LatestJob<SearchResult>,
    cancel: Arc<AtomicBool>,
    focus: bool,
    pub(super) edit_events: Vec<egui::Event>,
}
impl Dialog {
    pub(super) fn activity(&self) -> crate::activity::Activity {
        documentation_activity(
            self.job.activity(),
            self.error.as_deref(),
            self.results
                .as_ref()
                .is_some_and(|results| results.unavailable),
        )
    }

    fn new(query: Option<String>) -> Self {
        Self {
            query: query.unwrap_or_default(),
            results: None,
            error: None,
            job: LatestJob::default(),
            cancel: Arc::new(AtomicBool::new(false)),
            focus: true,
            edit_events: Vec::new(),
        }
    }
    fn invalidate(&mut self) {
        self.cancel.store(true, Ordering::Release);
        self.job = LatestJob::default();
        self.results = None;
        self.error = None;
    }
    fn start(&mut self, context: &egui::Context) {
        let query = match documentation::validate_query(&self.query) {
            Ok(query) => query.to_owned(),
            Err(error) => {
                self.error = Some(error);
                return;
            }
        };
        self.invalidate();
        self.cancel = Arc::new(AtomicBool::new(false));
        let cancel = self.cancel.clone();
        if let Err(error) = self
            .job
            .start_and_repaint("Texdoc search", context, move || {
                documentation::search(query, &cancel)
            })
        {
            self.error = Some(error);
        }
    }
    fn poll(&mut self) {
        match self.job.poll() {
            LatestJobPoll::Ready(results) => self.results = Some(results),
            LatestJobPoll::Failed(error) => self.error = Some(error),
            _ => {}
        }
    }
}
fn documentation_activity(
    worker: crate::activity::Activity,
    error: Option<&str>,
    unavailable: bool,
) -> crate::activity::Activity {
    use crate::activity::Activity;
    if matches!(worker, Activity::Running) {
        Activity::Running
    } else if let Some(error) = error {
        Activity::Failed(error.into())
    } else if matches!(worker, Activity::Failed(_)) {
        worker
    } else if unavailable {
        Activity::Inactive("TeXdoc unavailable")
    } else {
        Activity::Idle
    }
}

impl Drop for Dialog {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Release);
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Action {
    Search,
    Cancel,
    Online,
    Open(usize),
    Close,
}

impl EditorApp {
    pub(super) fn begin_tex_documentation(
        &mut self,
        query: Option<String>,
        context: &egui::Context,
    ) {
        if self.document_flow_busy() {
            return;
        }
        let mut dialog = Dialog::new(query);
        if !dialog.query.trim().is_empty() {
            dialog.start(context);
        }
        self.format_request_key = None;
        self.manual_format_revision = None;
        self.tex_documentation = Some(dialog);
        self.clear_editor_hover();
        self.clear_asset_hover();
        self.close_app_popup();
        self.find_bar.close();
        ChildViewHost::close(context, find_bar::VIEWPORT_SALT);
        context.request_repaint();
    }

    pub(super) fn close_tex_documentation(&mut self, context: &egui::Context) {
        if self.tex_documentation.take().is_some() {
            ChildViewHost::close(context, SALT);
            crate::window_host::return_focus(context);
        }
    }

    pub(super) fn show_tex_documentation(&mut self, context: &egui::Context) {
        let Some(dialog) = &mut self.tex_documentation else {
            return;
        };
        dialog.poll();
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
            ChildViewSpec::modal(SALT, "TeX Documentation", owner, "tex-documentation"),
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
                egui::Area::new(viewport_scoped_id(ui.ctx(), "tex-documentation-card"))
                    .order(egui::Order::Foreground)
                    .anchor(egui::Align2::CENTER_CENTER, Vec2::ZERO)
                    .show(ui.ctx(), |ui| {
                        theme::dialog_card_frame(&style).show(ui, |ui| {
                            ui.set_width((owner.width() - 48.0).clamp(1.0, 620.0));
                            ui.set_height((owner.height() - 48.0).clamp(1.0, 390.0));
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
            Some(Action::Close) => self.close_tex_documentation(context),
            Some(Action::Search) => self.tex_documentation.as_mut().unwrap().start(context),
            Some(Action::Cancel) => self.tex_documentation.as_mut().unwrap().invalidate(),
            Some(Action::Online) => {
                let query = &self.tex_documentation.as_ref().unwrap().query;
                if let Ok(url) = documentation::online_url(query) {
                    self.follow_preview_link(&url);
                }
            }
            Some(Action::Open(index)) => {
                let path = self
                    .tex_documentation
                    .as_ref()
                    .unwrap()
                    .results
                    .as_ref()
                    .and_then(|results| results.manuals.get(index))
                    .map(|manual| manual.path.clone());
                if let Some(path) = path {
                    self.close_tex_documentation(context);
                    // Opening a reference tab must not designate it as the paper's preview.
                    self.open_tab_path(path, context);
                }
            }
            None => {}
        }
    }
    #[cfg(all(feature = "desktop-ui-tests", target_os = "macos"))]
    pub(super) fn tex_documentation_inspection(&self) -> serde_json::Value {
        let Some(dialog) = &self.tex_documentation else {
            return serde_json::json!({"visible":false,"busy":false,"query_fingerprint":null,
                "error":null,"unavailable":false,"manual_paths":[]});
        };
        serde_json::json!({"visible":true,"busy":dialog.job.is_running(),
            "query_fingerprint":crate::desktop_test::fingerprint(&dialog.query),
            "error":dialog.error,"unavailable":dialog.results.as_ref().is_some_and(|r| r.unavailable),
            "manual_paths":dialog.results.as_ref().map(|r| r.manuals.iter().map(|m| &m.path).collect::<Vec<_>>()).unwrap_or_default()})
    }
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
    crate::desktop_test::observe(&format!("tex-documentation.{_name}"), _response);
}

fn show(ui: &mut egui::Ui, dialog: &mut Dialog) -> Option<Action> {
    theme::apply_compact_tool_spacing(ui);
    let mut action = None;
    ui.horizontal(|ui| {
        ui.strong("TeX Documentation");
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            let close = icon_button(ui, UiIcon::Close, "Close documentation");
            observe("close", &close);
            if close.clicked() {
                action = Some(Action::Close);
            }
        });
    });
    ui.horizontal(|ui| {
        let button_width = 74.0;
        let query = ui.add(
            egui::TextEdit::singleline(&mut dialog.query)
                .id_salt("tex-documentation-query")
                .hint_text("Package, class, or topic")
                .desired_width(
                    (ui.available_width() - button_width - ui.spacing().item_spacing.x).max(1.0),
                ),
        );
        query.widget_info(|| {
            egui::WidgetInfo::labeled(egui::WidgetType::TextEdit, true, "Documentation query")
        });
        observe("query", &query);
        if dialog.focus {
            query.request_focus();
            dialog.focus = false;
        }
        if query.changed() {
            dialog.invalidate();
        }
        let enter = (query.has_focus() || query.lost_focus())
            && ui.input_mut(|input| input.consume_key(egui::Modifiers::NONE, egui::Key::Enter));
        let search = ui.add_enabled(!dialog.query.trim().is_empty(), egui::Button::new("Search"));
        observe("search", &search);
        if search.clicked() || enter {
            action = Some(Action::Search);
        }
    });
    ui.horizontal(|ui| {
        let online = ui.add_enabled(
            documentation::validate_query(&dialog.query).is_ok(),
            egui::Button::new("Online manual"),
        );
        observe("online", &online);
        if online.clicked() {
            action = Some(Action::Online);
        }
        if dialog.job.is_running() {
            ui.spinner();
            let cancel = ui.button("Cancel search");
            observe("cancel", &cancel);
            if cancel.clicked() {
                action = Some(Action::Cancel);
            }
        } else if let Some(results) = &dialog.results {
            if results.unavailable {
                ui.weak("Texdoc is not installed.");
            } else if results.manuals.is_empty() {
                ui.weak("No local manuals found.");
            } else {
                ui.weak(format!("{} local manuals", results.manuals.len()));
            }
        }
    });
    if let Some(error) = &dialog.error {
        ui.colored_label(ui.visuals().error_fg_color, error);
    }
    ui.separator();
    egui::ScrollArea::vertical()
        .id_salt("tex-documentation-results")
        .auto_shrink([false, false])
        .show(ui, |ui| {
            let Some(results) = &dialog.results else {
                return;
            };
            for (index, manual) in results.manuals.iter().enumerate() {
                ui.push_id(&manual.path, |ui| {
                    ui.horizontal(|ui| {
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            let open = ui.button("Open");
                            observe(&format!("open.{index}"), &open);
                            if open.clicked() {
                                action = Some(Action::Open(index));
                            }
                            if !manual.language.is_empty() {
                                ui.weak(&manual.language);
                            }
                            ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
                                ui.vertical(|ui| {
                                    let title = if manual.description.is_empty() {
                                        manual.name()
                                    } else {
                                        manual.description.clone()
                                    };
                                    ui.add(
                                        egui::Label::new(RichText::new(title).strong())
                                            .truncate()
                                            .show_tooltip_when_elided(false),
                                    );
                                    ui.add(
                                        egui::Label::new(manual.name())
                                            .truncate()
                                            .show_tooltip_when_elided(false),
                                    );
                                });
                            });
                        });
                    });
                });
            }
        });
    action
}

#[cfg(all(test, target_os = "macos"))]
pub(super) fn show_framebuffer_results(ui: &mut egui::Ui) {
    let mut dialog = Dialog::new(Some("amsmath".into()));
    dialog.results = Some(SearchResult {
        unavailable: false,
        manuals: vec![
            documentation::Manual {
                path: "/Library/TeX/doc/amsmath/amsldoc.pdf".into(),
                description: "User guide (English)".into(),
                language: "en".into(),
            },
            documentation::Manual {
                path: "/Library/TeX/doc/amsmath/amsmath.pdf".into(),
                description: "Implementation documentation".into(),
                language: String::new(),
            },
            documentation::Manual {
                path: "/Library/TeX/doc/amsmath/README.md".into(),
                description: "General README".into(),
                language: "en".into(),
            },
        ],
    });
    let _ = show(ui, &mut dialog);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn activity_distinguishes_unavailable_texdoc_from_ready_and_failed_searches() {
        use crate::activity::Activity;
        let mut dialog = Dialog::new(None);
        assert_eq!(dialog.activity(), Activity::Idle);
        dialog.results = Some(SearchResult {
            unavailable: true,
            manuals: Vec::new(),
        });
        assert_eq!(dialog.activity(), Activity::Inactive("TeXdoc unavailable"));
        dialog.error = Some("The search failed".into());
        assert_eq!(
            dialog.activity(),
            Activity::Failed("The search failed".into())
        );
        assert_eq!(
            documentation_activity(Activity::Running, Some("old failure"), true),
            Activity::Running
        );
        assert_eq!(
            documentation_activity(Activity::Failed("worker stopped".into()), None, true),
            Activity::Failed("worker stopped".into())
        );
        assert!(!dialog.job.is_running());
        assert!(dialog.results.as_ref().unwrap().unavailable);
    }
    use egui_kittest::{
        Harness,
        kittest::{NodeT, Queryable},
    };

    fn harness(dialog: Dialog) -> Harness<'static, (Dialog, Option<Action>)> {
        let mut fonts_ready = false;
        Harness::builder()
            .with_size(Vec2::new(620.0, 390.0))
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
    fn empty_search_disables_actions_and_missing_texdoc_keeps_online_fallback() {
        let mut harness = harness(Dialog::new(None));
        harness.run();
        assert!(
            harness
                .get_by_label("Search")
                .accesskit_node()
                .is_disabled()
        );
        assert!(
            harness
                .get_by_label("Online manual")
                .accesskit_node()
                .is_disabled()
        );
        harness.state_mut().0.query = "amsmath".into();
        harness.state_mut().0.results = Some(SearchResult {
            manuals: Vec::new(),
            unavailable: true,
        });
        harness.run();
        assert!(
            !harness
                .get_by_label("Online manual")
                .accesskit_node()
                .is_disabled()
        );
        harness.state_mut().1 = None;
        harness.get_by_label("Online manual").click();
        harness.step();
        assert_eq!(harness.state().1, Some(Action::Online));
    }

    #[test]
    fn manual_open_routes_the_selected_local_result_without_launching_texdoc() {
        let mut dialog = Dialog::new(Some("amsmath".into()));
        dialog.results = Some(SearchResult {
            manuals: vec![documentation::Manual {
                path: PathBuf::from("/TeX docs/amsldoc.pdf"),
                description: "User guide".into(),
                language: "en".into(),
            }],
            unavailable: false,
        });
        let mut harness = harness(dialog);
        harness.run();
        harness.state_mut().1 = None;
        harness.get_by_label("Open").click();
        harness.step();
        assert_eq!(harness.state().1, Some(Action::Open(0)));
        assert!(!harness.state().0.job.is_running());
    }

    #[test]
    fn enter_submits_the_focused_query_when_singleline_edit_releases_focus() {
        let mut harness = harness(Dialog::new(Some("old query".into())));
        harness.run();
        harness.get_by_label("Documentation query").focus();
        harness.step();
        assert!(harness.get_by_label("Documentation query").is_focused());
        harness.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::A);
        harness.step();
        harness
            .get_by_label("Documentation query")
            .type_text("amsmath");
        harness.run();
        assert_eq!(harness.state().0.query, "amsmath");
        harness.state_mut().1 = None;
        harness.key_press(egui::Key::Enter);
        harness.step();
        assert_eq!(harness.state().1, Some(Action::Search));
    }

    #[test]
    fn changing_or_dropping_dialog_cancels_pending_work_and_flushes_results() {
        let mut dialog = Dialog::new(Some("amsmath".into()));
        let cancel = dialog.cancel.clone();
        dialog.results = Some(SearchResult::default());
        dialog.invalidate();
        assert!(cancel.load(Ordering::Acquire));
        assert!(dialog.results.is_none());
        assert!(!dialog.job.is_running());
        dialog.cancel = Arc::new(AtomicBool::new(false));
        let cancel = dialog.cancel.clone();
        drop(dialog);
        assert!(cancel.load(Ordering::Acquire));
    }

    #[test]
    fn escape_and_configured_close_shortcut_belong_to_documentation_modal() {
        let shortcuts = ShortcutBindings::defaults(ShortcutPlatform::MacOs);
        for (key, modifiers) in [
            (egui::Key::Escape, egui::Modifiers::NONE),
            (egui::Key::W, egui::Modifiers::COMMAND),
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
                },
            );
            output.textures_delta.clear();
            assert!(closed, "{key:?} {modifiers:?}");
        }
    }
}
