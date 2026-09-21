//! Owned presentation state for an independently repainted Settings viewport.
//! The callback never borrows EditorApp or operates its workers/documents.
use super::{settings_panel::*, *};

#[derive(PartialEq)]
pub(super) struct SettingsWindowInput {
    pub(super) visible: bool,
    pub(super) retain_when_closed: bool,
    pub(super) settings: AppSettings,
    pub(super) theme_override: Option<CaptureThemeProfile>,
    pub(super) snapshot_scene: Option<UiSnapshotScene>,
    pub(super) font_catalog_revision: u64,
    pub(super) font_catalog_scanning: bool,
    pub(super) font_configuration: theme::FontConfiguration,
    pub(super) typst_tool: ToolResolution,
    pub(super) tinymist_tool: ToolResolution,
    pub(super) status: SettingsStatus,
    pub(super) project_root: PathBuf,
    pub(super) appearance: egui::Theme,
    pub(super) style: Arc<egui::Style>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui_kittest::{
        Harness,
        kittest::{Queryable as _, by},
    };

    #[test]
    fn dormant_settings_is_registered_hidden_and_close_retains_it_for_logic_reopen() {
        let context = egui::Context::default();
        context.set_embed_viewports(false);
        let shared = Arc::new(Mutex::new(SettingsWindow::default()));
        let captures = CaptureController::disabled_for_tests();
        let child = scoped_child_viewport_id(&context, "tiptoptyp-settings");
        let mut output = context.run_ui(egui::RawInput::default(), |ui| {
            let mut snapshot = input(ui.ctx());
            snapshot.visible = false;
            snapshot.retain_when_closed = true;
            SettingsWindow::show(
                &shared,
                ui.ctx(),
                &captures,
                snapshot,
                &FontCatalog::default(),
            );
        });
        assert_eq!(output.viewport_output[&child].builder.visible, Some(false));
        let callback = output.viewport_output[&child]
            .viewport_ui_cb
            .clone()
            .unwrap();
        output.textures_delta.clear();
        let mut raw = egui::RawInput {
            viewport_id: child,
            ..Default::default()
        };
        raw.viewports
            .entry(child)
            .or_default()
            .events
            .push(egui::ViewportEvent::Close);
        let mut output = context.run_ui(raw, |ui| callback(ui));
        let commands = &output.viewport_output[&child].commands;
        assert!(
            commands
                .iter()
                .any(|command| matches!(command, egui::ViewportCommand::CancelClose))
        );
        assert!(
            commands
                .iter()
                .any(|command| matches!(command, egui::ViewportCommand::Visible(false)))
        );
        output.textures_delta.clear();
        assert!(shared.lock().unwrap().has_actions());
    }

    #[test]
    fn settings_search_native_edit_commands_and_preference_clicks_survive_deferred_delivery() {
        let context = egui::Context::default();
        let mut window = SettingsWindow::default();
        window.synchronize(input(&context), &FontCatalog::default());
        window.ui.query = "theme".into();
        let captures = CaptureController::disabled_for_tests();
        let mut harness = Harness::builder()
            .with_size(Vec2::new(700.0, 3000.0))
            .build_ui_state(
                move |ui, window: &mut SettingsWindow| {
                    window.prepare_keyboard(ui.ctx());
                    let actions = window.paint(ui, &captures, &mut false);
                    window.text_input_focused = ui.ctx().text_edit_focused();
                    window.accept_actions(actions);
                    window.collect_owner_shortcuts(ui.ctx());
                },
                window,
            );
        harness.run();
        harness
            .get(by().role(egui::accesskit::Role::TextInput).value("theme"))
            .focus();
        harness.run();
        assert!(
            harness
                .state_mut()
                .queue_edit_command(AppCommand::SelectAll)
        );
        harness.run();
        harness
            .get(by().role(egui::accesskit::Role::TextInput).value("theme"))
            .type_text("fonts");
        harness.run();
        assert_eq!(harness.state().ui.query, "fonts");
        assert!(!harness.state().has_actions());
        assert!(
            harness.state().owner_keys.is_empty(),
            "text editing must stay local"
        );
        harness.get_by_label("Invert colors").click();
        harness.run();
        assert!(
            harness
                .state()
                .input
                .as_ref()
                .unwrap()
                .settings
                .theme_invert
        );
        assert!(harness.state().edit.is_some());
        let mut live = AppSettings::default();
        harness.state_mut().take_actions(&mut live);
        assert!(live.theme_invert);
        harness.run();
        assert!(
            !harness.state().has_actions(),
            "idle frames must not duplicate edits"
        );
    }

    #[test]
    fn settings_owner_shortcuts_are_queued_once_not_left_in_stale_child_input() {
        let context = egui::Context::default();
        let mut window = SettingsWindow::default();
        window.synchronize(input(&context), &FontCatalog::default());
        let shortcut = AppSettings::default()
            .effective_shortcuts()
            .egui(ShortcutAction::Settings)
            .unwrap();
        context
            .run_ui(
                egui::RawInput {
                    events: vec![egui::Event::Key {
                        key: shortcut.logical_key,
                        physical_key: None,
                        pressed: true,
                        repeat: false,
                        modifiers: shortcut.modifiers,
                    }],
                    ..Default::default()
                },
                |ui| {
                    assert!(window.collect_owner_shortcuts(ui.ctx()));
                    assert!(!window.collect_owner_shortcuts(ui.ctx()));
                    assert_eq!(window.take_owner_keys().len(), 1);
                    assert!(window.take_owner_keys().is_empty());
                },
            )
            .drop_without_applying_deltas();
    }

    fn input(context: &egui::Context) -> SettingsWindowInput {
        use crate::toolchain::{ToolKind, ToolOrigin};
        SettingsWindowInput {
            visible: true,
            retain_when_closed: false,
            settings: AppSettings::default(),
            theme_override: None,
            snapshot_scene: None,
            font_catalog_revision: 1,
            font_catalog_scanning: false,
            font_configuration: theme::FontConfiguration {
                custom_ui_loaded: false,
                custom_editor_loaded: false,
                weighted_ui_loaded: false,
                ui_weight_support: None,
                code_weight_support: None,
                editor_weight_support: theme::FontWeightSupport::Discrete {
                    values: vec![400],
                    default: 400,
                },
            },
            typst_tool: ToolResolution {
                kind: ToolKind::Typst,
                program: "typst".into(),
                origin: ToolOrigin::Bundled,
                fallback_reason: None,
            },
            tinymist_tool: ToolResolution {
                kind: ToolKind::Tinymist,
                program: "tinymist".into(),
                origin: ToolOrigin::Bundled,
                fallback_reason: None,
            },
            status: SettingsStatus {
                backend_label: "Interactive",
                fallback_reason: None,
                requested_backend: PreviewPreference::Interactive,
                interactive_active: true,
                capabilities: crate::capabilities::CapabilitySnapshot {
                    editing: ServiceState::Ready("ready".into()),
                    lsp: ServiceState::Ready("ready".into()),
                    interactive_preview: ServiceState::Ready("ready".into()),
                    pdf_generation: ServiceState::Ready("ready".into()),
                    rasterization: ServiceState::Ready("ready".into()),
                    link_extraction: ServiceState::Ready("ready".into()),
                },
            },
            project_root: ".".into(),
            appearance: egui::Theme::Dark,
            style: context.style_of(egui::Theme::Dark),
        }
    }

    #[test]
    fn settings_invalidation_reuses_catalog_and_preserves_local_state() {
        let context = egui::Context::default();
        let fonts = FontCatalog::snapshot_fixture();
        let mut window = SettingsWindow::default();
        assert!(window.synchronize(input(&context), &fonts));
        let faces = window.fonts.families().as_ptr();
        window.ui.query = "font".into();
        window.ui.staged_code_font_weight = Some(700);
        assert!(!window.synchronize(input(&context), &fonts));
        let mut changed = input(&context);
        changed.status.capabilities.lsp = ServiceState::Failed("offline".into());
        assert!(window.synchronize(changed, &fonts));
        assert_eq!(faces, window.fonts.families().as_ptr());
        assert_eq!(window.ui.query, "font");
        assert_eq!(window.ui.staged_code_font_weight, Some(700));
        let mut changed = input(&context);
        changed.appearance = egui::Theme::Light;
        changed.style = context.style_of(egui::Theme::Light);
        assert!(window.synchronize(changed, &fonts));
        assert_eq!(faces, window.fonts.families().as_ptr());
        let mut scene = input(&context);
        scene.snapshot_scene = Some(UiSnapshotScene::SettingsFontPicker);
        assert!(window.synchronize(scene, &FontCatalog::default()));
        assert!(
            window.fonts.families().is_empty(),
            "a new QA scene must replace its font fixture even at the same production revision"
        );
        let mut changed = input(&context);
        changed.font_catalog_revision += 1;
        assert!(window.synchronize(changed, &FontCatalog::default()));
        assert!(window.fonts.families().is_empty());
        window.suspend();
        assert!(
            window.synchronize(input(&context), &fonts),
            "reopening must repaint"
        );
    }

    #[test]
    fn settings_mailbox_coalesces_edits_and_preserves_concurrent_changes() {
        let context = egui::Context::default();
        let mut window = SettingsWindow::default();
        window.synchronize(input(&context), &FontCatalog::default());
        let mut current = AppSettings::default();
        current
            .last_opened_files
            .insert("project".into(), "new.typ".into());
        current.hover_delay_ms = 123;
        for value in [1000, 1100, 1200] {
            let mut settings = window.input.as_ref().unwrap().settings.clone();
            settings.auto_save_delay_ms = value;
            assert!(window.accept_actions(vec![SettingsAction::Update(Box::new(settings))]));
        }
        assert_eq!(window.edit.as_ref().unwrap().0.auto_save_delay_ms, 750);
        let (actions, close) = window.take_actions(&mut current);
        assert!(actions.is_empty() && !close && window.edit.is_none());
        assert_eq!(current.auto_save_delay_ms, 1200);
        assert_eq!(current.hover_delay_ms, 123);
        assert_eq!(current.last_opened_files["project"], "new.typ");
        // A child can cancel an edit before the owner runs at all.
        let base = window.input.as_ref().unwrap().settings.clone();
        let mut edited = base.clone();
        edited.theme_invert = true;
        window.accept_actions(vec![SettingsAction::Update(Box::new(edited))]);
        window.accept_actions(vec![SettingsAction::Update(Box::new(base))]);
        window.ui.staged_ui_font_weight = Some(700);
        window.close_requested = true;
        window.accept_actions(vec![
            SettingsAction::ShowShortcuts,
            SettingsAction::RefreshTools,
        ]);
        let (actions, close) = window.take_actions(&mut current);
        assert!(!current.theme_invert);
        assert!(close && window.ui.staged_ui_font_weight.is_none());
        assert!(matches!(
            actions.as_slice(),
            [SettingsAction::ShowShortcuts, SettingsAction::RefreshTools]
        ));
        let (actions, close) = window.take_actions(&mut current);
        assert!(actions.is_empty() && !close);
    }

    #[test]
    fn settings_native_scroll_and_parent_frames_are_independent() {
        let context = egui::Context::default();
        context.set_embed_viewports(false);
        let window = Arc::new(Mutex::new(SettingsWindow::default()));
        let captures = CaptureController::disabled_for_tests();
        let child = scoped_child_viewport_id(&context, "tiptoptyp-settings");
        let requests = Arc::new(Mutex::new(Vec::new()));
        let recorded = requests.clone();
        context.set_request_repaint_callback(move |request| {
            recorded.lock().unwrap().push(request.viewport_id)
        });
        let register = || {
            context.run_ui(egui::RawInput::default(), |ui| {
                SettingsWindow::show(
                    &window,
                    ui.ctx(),
                    &captures,
                    input(ui.ctx()),
                    &FontCatalog::default(),
                );
            })
        };
        let output = register();
        assert!(output.viewport_output[&child].class == egui::ViewportClass::Deferred);
        let callback = output.viewport_output[&child]
            .viewport_ui_cb
            .clone()
            .unwrap();
        output.drop_without_applying_deltas();
        let paint_child = |frame: u32, events: Vec<egui::Event>, close: bool| {
            let mut raw = egui::RawInput {
                viewport_id: child,
                time: Some(f64::from(frame) / 60.0),
                events,
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(640.0, 500.0))),
                ..Default::default()
            };
            raw.viewports.insert(
                child,
                egui::ViewportInfo {
                    parent: Some(egui::ViewportId::ROOT),
                    focused: Some(true),
                    events: if close {
                        vec![egui::ViewportEvent::Close]
                    } else {
                        vec![]
                    },
                    ..Default::default()
                },
            );
            context
                .run_ui(raw, |ui| callback(ui))
                .drop_without_applying_deltas();
        };
        for frame in 0..10 {
            paint_child(frame, vec![], false);
        }
        let root_frames = context.cumulative_frame_nr_for(egui::ViewportId::ROOT);
        requests.lock().unwrap().clear();
        for frame in 10..30 {
            paint_child(
                frame,
                vec![
                    egui::Event::PointerMoved(Pos2::new(350.0, 250.0)),
                    egui::Event::MouseWheel {
                        unit: egui::MouseWheelUnit::Point,
                        phase: egui::TouchPhase::Move,
                        delta: Vec2::new(0.0, -20.0),
                        modifiers: Modifiers::NONE,
                    },
                ],
                false,
            );
        }
        assert_eq!(
            context.cumulative_frame_nr_for(egui::ViewportId::ROOT),
            root_frames
        );
        assert!(
            !requests.lock().unwrap().contains(&egui::ViewportId::ROOT),
            "scrolling must not wake the editor"
        );
        let child_frames = context.cumulative_frame_nr_for(child);
        for _ in 0..10 {
            register().drop_without_applying_deltas();
        }
        assert_eq!(
            context.cumulative_frame_nr_for(child),
            child_frames,
            "editor frames must not paint Settings"
        );
        assert!(window.lock().unwrap().edit.is_none());
        requests.lock().unwrap().clear();
        paint_child(31, vec![], true);
        assert!(window.lock().unwrap().close_requested);
        assert!(
            requests.lock().unwrap().contains(&egui::ViewportId::ROOT),
            "native close must notify the owner"
        );
    }
}

#[derive(Default)]
pub(super) struct SettingsWindow {
    pub(super) ui: SettingsUiState,
    input: Option<SettingsWindowInput>,
    fonts: FontCatalog,
    // Coalesce rapid slider changes, but retain the first base for a three-way
    // merge. A sibling window or document history may change before delivery.
    edit: Option<(AppSettings, AppSettings)>,
    actions: Vec<SettingsAction>,
    close_requested: bool,
    focused: bool,
    text_input_focused: bool,
    menu_commands: Vec<AppCommand>,
    owner_keys: Vec<egui::Event>,
    paste_deadline: Option<f64>,
    local_events: Vec<egui::Event>,
}

impl SettingsWindow {
    pub(super) fn queue_edit_command(&mut self, command: AppCommand) -> bool {
        if self.input.is_some() && self.text_input_focused && is_widget_edit_command(command) {
            self.menu_commands.push(command);
            true
        } else {
            false
        }
    }

    pub(super) fn take_owner_keys(&mut self) -> Vec<egui::Event> {
        std::mem::take(&mut self.owner_keys)
    }
    pub(super) fn has_actions(&self) -> bool {
        self.edit.is_some() || !self.actions.is_empty() || self.close_requested
    }
    fn synchronize(&mut self, input: SettingsWindowInput, fonts: &FontCatalog) -> bool {
        let changed = self.input.as_ref() != Some(&input);
        if changed {
            if self.input.as_ref().is_none_or(|old| {
                old.font_catalog_revision != input.font_catalog_revision
                    || old.snapshot_scene != input.snapshot_scene
            }) {
                // Catalogs can contain thousands of faces. Never copy them on
                // a pointer/scroll frame or on an unrelated editor update.
                // QA scene changes can substitute a fixture independently of
                // the production catalog revision.
                self.fonts = fonts.clone();
            }
            self.input = Some(input);
        }
        changed
    }

    pub(super) fn suspend(&mut self) {
        self.input = None;
        self.focused = false;
        self.text_input_focused = false;
        self.menu_commands.clear();
        self.owner_keys.clear();
        self.paste_deadline = None;
        self.local_events.clear();
    }

    pub(super) fn take_actions(
        &mut self,
        settings: &mut AppSettings,
    ) -> (Vec<SettingsAction>, bool) {
        if let Some((base, edited)) = self.edit.take() {
            settings.apply_edits(&base, edited);
        }
        let close = std::mem::take(&mut self.close_requested);
        if close {
            self.ui.staged_ui_font_weight = None;
            self.ui.staged_code_font_weight = None;
        }
        (std::mem::take(&mut self.actions), close)
    }

    fn accept_actions(&mut self, actions: Vec<SettingsAction>) -> bool {
        let changed = !actions.is_empty();
        for action in actions {
            if let SettingsAction::Update(edited) = action {
                let input = self.input.as_mut().expect("visible Settings input");
                let base = std::mem::replace(&mut input.settings, *edited);
                if let Some((_, latest)) = &mut self.edit {
                    *latest = input.settings.clone();
                } else {
                    self.edit = Some((base, input.settings.clone()));
                }
            } else {
                self.actions.push(action);
            }
        }
        changed
    }

    pub(super) fn show(
        shared: &Arc<Mutex<Self>>,
        context: &egui::Context,
        captures: &CaptureController,
        input: SettingsWindowInput,
        fonts: &FontCatalog,
    ) {
        let child = scoped_child_viewport_id(context, "tiptoptyp-settings");
        let changed = {
            let mut state = shared.lock().unwrap();
            state.synchronize(input, fonts) || state.ui.scroll_target.is_some()
        };
        if changed || captures.has_pending_for("settings") {
            context.request_repaint_of(child);
        }
        Self::show_registered(shared, context, captures);
    }

    /// Re-register a retained hidden surface without rebuilding its settings,
    /// toolchain and font snapshots on every editor frame.
    pub(super) fn retain_hidden(
        shared: &Arc<Mutex<Self>>,
        context: &egui::Context,
        captures: &CaptureController,
    ) -> bool {
        {
            let mut state = shared.lock().unwrap();
            let Some(input) = state.input.as_mut() else {
                return false;
            };
            input.visible = false;
            input.retain_when_closed = true;
        }
        ChildViewHost::hide(context, "tiptoptyp-settings");
        Self::show_registered(shared, context, captures);
        true
    }

    fn show_registered(
        shared: &Arc<Mutex<Self>>,
        context: &egui::Context,
        captures: &CaptureController,
    ) {
        let state = shared.lock().unwrap();
        let input = state.input.as_ref().expect("registered Settings input");
        let appearance = input.appearance;
        let visible = input.visible;
        let style = input.style.clone();
        let narrow = matches!(
            input.snapshot_scene,
            Some(
                UiSnapshotScene::SettingsColors
                    | UiSnapshotScene::SettingsEditor
                    | UiSnapshotScene::SettingsStatus
            )
        );
        drop(state);
        let spec = ChildViewSpec::persistent(
            "tiptoptyp-settings",
            "tiptoptyp Settings",
            [
                if narrow {
                    METRICS.chrome.settings_min_size.x
                } else {
                    METRICS.chrome.settings_width
                },
                METRICS.chrome.settings_height,
            ],
            METRICS.chrome.settings_min_size,
            "settings",
        )
        .with_visible(visible)
        .with_dormant_hosting(true);
        let parent = context.viewport_id();
        let shared = shared.clone();
        let child_captures = captures.clone();
        ChildViewHost::show_deferred(
            context,
            captures,
            spec,
            appearance,
            &style,
            move |ui, input| {
                let mut state = shared.lock().unwrap();
                if state.input.is_none() || state.close_requested {
                    return;
                }
                let mut close = input.close_requested;
                let gained_focus = input.focused == Some(true) && !state.focused;
                state.focused = input.focused == Some(true);
                state.prepare_keyboard(ui.ctx());
                let actions = state.paint(ui, &child_captures, &mut close);
                state.text_input_focused = ui.ctx().text_edit_focused();
                let shortcut = state.collect_owner_shortcuts(ui.ctx());
                if state.accept_actions(actions) || close || gained_focus || shortcut {
                    if close
                        && state
                            .input
                            .as_ref()
                            .is_some_and(|input| input.retain_when_closed)
                    {
                        ui.ctx()
                            .send_viewport_cmd(egui::ViewportCommand::CancelClose);
                        ui.ctx()
                            .send_viewport_cmd(egui::ViewportCommand::Visible(false));
                    }
                    state.close_requested |= close;
                    // Search, scrolling, tooltips, picker navigation and font
                    // preview completion are all local to this child viewport.
                    ui.ctx().request_repaint_of(parent);
                }
            },
        );
    }

    fn collect_owner_shortcuts(&mut self, context: &egui::Context) -> bool {
        // Preserve the existing owner shortcut router, but don't wake it for
        // search typing or keys consumed by a Settings widget.
        context.input_mut(|input| {
            if !input.events.iter().any(|event| {
                matches!(
                    event,
                    egui::Event::Key { pressed: true, .. }
                        | egui::Event::Copy
                        | egui::Event::Cut
                        | egui::Event::Paste(_)
                )
            }) {
                return false;
            }
            let shortcuts = self
                .input
                .as_ref()
                .expect("visible Settings input")
                .settings
                .effective_shortcuts();
            let mut forwarded = false;
            input.events.retain(|event| {
                if self.local_events.contains(event) {
                    return false;
                }
                let action = match event {
                    egui::Event::Key {
                        key,
                        modifiers,
                        pressed: true,
                        ..
                    } => shortcuts.action_for_key_event(*key, *modifiers),
                    _ => None,
                };
                let local_edit = self.text_input_focused
                    && action.is_some_and(|action| {
                        crate::native_menu::COMMAND_SPECS.iter().any(|spec| {
                            spec.shortcut_action == action && is_widget_edit_command(spec.command)
                        })
                    });
                let matched = action.is_some() && !local_edit;
                if matched {
                    self.owner_keys.push(event.clone());
                    forwarded = true;
                }
                let local_clipboard = self.text_input_focused
                    && matches!(
                        event,
                        egui::Event::Copy | egui::Event::Cut | egui::Event::Paste(_)
                    );
                !matched && !local_edit && !local_clipboard
            });
            forwarded
        })
    }

    fn prepare_keyboard(&mut self, context: &egui::Context) {
        self.local_events.clear();
        let has_events = context.input(|input| {
            input.events.iter().any(|event| {
                matches!(
                    event,
                    egui::Event::Key { .. }
                        | egui::Event::Copy
                        | egui::Event::Cut
                        | egui::Event::Paste(_)
                )
            })
        });
        if has_events {
            let shortcuts = self
                .input
                .as_ref()
                .expect("visible Settings input")
                .settings
                .effective_shortcuts();
            let now = context.input(|input| input.time);
            let allow_paste = self.paste_deadline.is_some_and(|deadline| now <= deadline);
            if context.input_mut(|input| {
                normalize_text_edit_shortcut_events(input, &shortcuts, allow_paste)
            }) || !allow_paste
            {
                self.paste_deadline = None;
            }
            if self.text_input_focused
                && let Some(command) = context
                    .input_mut(|input| consume_shortcut(input, &shortcuts, is_widget_edit_command))
            {
                self.menu_commands.push(command);
            }
        }
        for command in std::mem::take(&mut self.menu_commands) {
            let event = match command {
                AppCommand::Cut => Some(egui::Event::Cut),
                AppCommand::Copy => Some(egui::Event::Copy),
                AppCommand::Paste => {
                    self.paste_deadline = Some(context.input(|input| input.time) + 1.0);
                    context.send_viewport_cmd(egui::ViewportCommand::RequestPaste);
                    None
                }
                AppCommand::Undo | AppCommand::Redo | AppCommand::SelectAll => {
                    standard_text_edit_shortcut(command).map(|shortcut| egui::Event::Key {
                        key: shortcut.logical_key,
                        physical_key: None,
                        pressed: true,
                        repeat: false,
                        modifiers: shortcut.modifiers,
                    })
                }
                _ => None,
            };
            if let Some(event) = event {
                self.local_events.push(event.clone());
                context.input_mut(|input| input.events.push(event));
            }
        }
    }

    fn paint(
        &mut self,
        ui: &mut egui::Ui,
        captures: &CaptureController,
        close: &mut bool,
    ) -> Vec<SettingsAction> {
        let settings_tooltip_id = settings_hover_tooltip_id(ui.ctx());
        ui.ctx()
            .data_mut(|data| data.remove::<HoverTooltipOverlay>(settings_tooltip_id));
        if ui.ctx().input(|input| input.viewport().fullscreen) == Some(true) {
            ui.ctx()
                .send_viewport_cmd(egui::ViewportCommand::Fullscreen(false));
        }
        ui.painter()
            .rect_filled(ui.max_rect(), 0.0, ui.visuals().panel_fill);
        let title_rect = Rect::from_min_size(
            ui.max_rect().min,
            egui::vec2(ui.max_rect().width(), METRICS.chrome.toolbar_height),
        );
        let drag = ui.interact(
            title_rect,
            ui.id().with("settings-window-drag"),
            Sense::drag(),
        );
        if drag.drag_started() {
            ui.ctx().send_viewport_cmd(egui::ViewportCommand::StartDrag);
        }
        egui::Panel::top("settings-titlebar")
            .exact_size(METRICS.chrome.toolbar_height)
            .frame(theme::settings_title_frame(ui.style()))
            .show(ui, |ui| {
                ui.horizontal_centered(|ui| {
                    #[cfg(target_os = "macos")]
                    theme::reserve_window_controls(ui);
                    crate::window_logo::show(ui, captures);
                    ui.label(RichText::new("Settings").strong());
                    #[cfg(not(target_os = "macos"))]
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if icon_button(ui, UiIcon::Close, "Close Settings").clicked() {
                            *close = true;
                        }
                    });
                });
            });
        // On macOS close comes from native chrome, not an egui button.
        let _ = close;
        let input = self.input.as_ref().expect("visible Settings input");
        let mut actions = Vec::new();
        egui::CentralPanel::default()
            .frame(theme::settings_content_frame(ui.style()))
            .show(ui, |ui| {
                SettingsPanel {
                    state: &mut self.ui,
                    settings: &input.settings,
                    pending_settings: None,
                    theme_override: input.theme_override.as_ref(),
                    snapshot_scene: input.snapshot_scene,
                    font_catalog: &self.fonts,
                    font_catalog_scanning: input.font_catalog_scanning,
                    font_configuration: &input.font_configuration,
                    typst_tool: &input.typst_tool,
                    tinymist_tool: &input.tinymist_tool,
                    status: input.status.clone(),
                    project_root: &input.project_root,
                    captures,
                    actions: &mut actions,
                }
                .show(ui)
            });
        if input.snapshot_scene == Some(UiSnapshotScene::SettingsTooltip) {
            show_local_tooltip_card(
                ui.ctx(),
                Pos2::new(
                    theme::SPACE.content * 2.0,
                    METRICS.chrome.toolbar_height + theme::SPACE.content * 2.0,
                ),
                LUMINOSITY_HINT,
                1.0,
            );
        } else if let Some(tooltip) = ui
            .ctx()
            .data(|data| data.get_temp::<HoverTooltipOverlay>(settings_tooltip_id))
        {
            show_local_tooltip_card(ui.ctx(), tooltip.anchor, &tooltip.detail, tooltip.opacity);
        }
        actions
    }
}

fn is_widget_edit_command(command: AppCommand) -> bool {
    matches!(
        command,
        AppCommand::Cut
            | AppCommand::Copy
            | AppCommand::Paste
            | AppCommand::Undo
            | AppCommand::Redo
            | AppCommand::SelectAll
            | AppCommand::ToggleComment
            | AppCommand::Format
    )
}
