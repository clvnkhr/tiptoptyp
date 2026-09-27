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
    pub(super) tex_tools: crate::tex::tools::TexTools,
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
        let mut window = SettingsWindow::default();
        window.ui.query = "theme".into();
        let mut harness = settings_harness(window);
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
        harness
            .get(
                by().label("Invert colors")
                    .predicate(|node| node.role() == egui::accesskit::Role::CheckBox),
            )
            .click();
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
    fn settings_search_focus_request_targets_the_search_field() {
        let mut window = SettingsWindow::default();
        window.ui.query = "theme".into();
        let mut harness = settings_harness(window);
        harness.run();
        let shortcut = harness
            .state()
            .input
            .as_ref()
            .unwrap()
            .settings
            .effective_shortcuts()
            .egui(ShortcutAction::Find)
            .unwrap();
        harness.key_press_modifiers(shortcut.modifiers, shortcut.logical_key);
        harness.run();
        assert!(
            harness
                .get(by().role(egui::accesskit::Role::TextInput).value("theme"))
                .is_focused(),
            "Cmd+F in Settings should focus the settings search field"
        );
        harness.key_press_modifiers(shortcut.modifiers, shortcut.logical_key);
        harness.run();
        assert_eq!(harness.state().ui.query, "theme");
        assert!(
            harness
                .get(by().role(egui::accesskit::Role::TextInput).value("theme"))
                .is_focused(),
            "repeated Cmd+F should reuse the same focused search field"
        );
    }

    #[test]
    fn settings_close_shortcut_is_consumed_by_the_settings_owner() {
        let window = SettingsWindow::default();
        let mut harness = settings_harness(window);
        harness.run();
        let shortcut = harness
            .state()
            .input
            .as_ref()
            .unwrap()
            .settings
            .effective_shortcuts()
            .egui(ShortcutAction::CloseTab)
            .unwrap();
        harness.key_press_modifiers(shortcut.modifiers, shortcut.logical_key);
        harness.run();
        let mut settings = AppSettings::default();
        let (_, close) = harness.state_mut().take_actions(&mut settings);
        assert!(close, "Cmd+W should close the Settings panel");
    }

    #[test]
    fn settings_uses_the_binding_that_captured_a_child_shortcut() {
        let context = egui::Context::default();
        let mut snapshot = input(&context);
        snapshot.settings.shortcut_overrides.set(
            ShortcutAction::CloseTab,
            Some(crate::shortcuts::ShortcutChord::primary(egui::Key::K)),
        );
        let mut window = SettingsWindow::default();
        window.synchronize(snapshot, &FontCatalog::default());
        let mut harness = settings_harness(window);
        harness.run();
        let shortcut = harness
            .state()
            .input
            .as_ref()
            .unwrap()
            .settings
            .effective_shortcuts()
            .egui(ShortcutAction::CloseTab)
            .unwrap();
        harness.key_press_modifiers(shortcut.modifiers, shortcut.logical_key);
        harness.run();
        let mut settings = AppSettings::default();
        let (_, close) = harness.state_mut().take_actions(&mut settings);
        assert!(
            close,
            "Settings should close on its effective custom binding"
        );
    }

    #[test]
    fn document_shortcuts_do_not_interrupt_settings_search_typing() {
        let mut window = SettingsWindow::default();
        window.ui.query = "back".into();
        let mut harness = settings_harness(window);
        harness.run();
        harness
            .get(by().role(egui::accesskit::Role::TextInput).value("back"))
            .focus();
        harness.run();
        for action in [
            ShortcutAction::New,
            ShortcutAction::Save,
            ShortcutAction::Panel,
            ShortcutAction::Compile,
            ShortcutAction::ToggleLineWrap,
        ] {
            let shortcut = harness
                .state()
                .input
                .as_ref()
                .unwrap()
                .settings
                .effective_shortcuts()
                .egui(action)
                .unwrap();
            harness.key_press_modifiers(shortcut.modifiers, shortcut.logical_key);
            harness.run();
            assert_eq!(harness.state().ui.query, "back", "{action:?}");
            assert!(!harness.state().close_requested, "{action:?}");
        }
        harness
            .get(by().role(egui::accesskit::Role::TextInput).value("back"))
            .type_text("end");
        harness.run();
        assert_eq!(harness.state().ui.query, "backend");
    }

    #[test]
    fn minimize_shortcut_targets_the_settings_viewport() {
        let mut harness = settings_harness(SettingsWindow::default());
        let shortcut = harness
            .state()
            .input
            .as_ref()
            .unwrap()
            .settings
            .effective_shortcuts()
            .egui(ShortcutAction::Minimize)
            .unwrap();
        harness.input_mut().events.push(egui::Event::Key {
            key: shortcut.logical_key,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: shortcut.modifiers,
        });
        harness.step();
        assert!(
            harness.output().viewport_output[&harness.ctx.viewport_id()]
                .commands
                .iter()
                .any(|command| matches!(command, egui::ViewportCommand::Minimized(true)))
        );
    }

    #[test]
    fn settings_json_rejects_invalid_text_then_saves_valid_edits() {
        let mut harness = settings_harness(SettingsWindow::default());
        harness.run();
        harness.get_by_label("Edit settings as JSON").click();
        harness.run();
        harness
            .get(by().role(egui::accesskit::Role::MultilineTextInput))
            .focus();
        harness.run();
        harness.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::A);
        harness.run();
        harness
            .get(by().role(egui::accesskit::Role::MultilineTextInput))
            .type_text("{broken");
        harness.run();
        assert!(
            harness
                .state()
                .ui
                .json_draft
                .as_ref()
                .unwrap()
                .validation
                .is_err()
        );
        assert!(!harness.state().has_actions());
        harness.get_by_label("Reload current settings").click();
        harness.run();
        let mut edited = harness.state().input.as_ref().unwrap().settings.clone();
        edited.writing_language = crate::settings::WritingLanguage::American;
        let text = serde_json::to_string_pretty(&edited).unwrap();
        harness
            .get(by().role(egui::accesskit::Role::MultilineTextInput))
            .focus();
        harness.run();
        harness.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::A);
        harness.run();
        harness
            .get(by().role(egui::accesskit::Role::MultilineTextInput))
            .type_text(&text);
        harness.run();
        assert!(
            harness
                .state()
                .ui
                .json_draft
                .as_ref()
                .unwrap()
                .validation
                .is_ok()
        );
        harness.get_by_label("Save JSON settings").click();
        harness.run();
        let mut saved = AppSettings::default();
        harness.state_mut().take_actions(&mut saved);
        assert_eq!(
            saved.writing_language,
            crate::settings::WritingLanguage::American
        );
        harness
            .state_mut()
            .input
            .as_mut()
            .unwrap()
            .settings
            .ui_scale_percent = 125;
        harness.run();
        assert_eq!(
            harness
                .state()
                .ui
                .json_draft
                .as_ref()
                .unwrap()
                .validation
                .as_ref()
                .unwrap()
                .ui_scale_percent,
            125,
            "an unmodified JSON draft follows form changes"
        );
    }

    #[test]
    fn settings_keyboard_shortcuts_command_opens_its_local_editor() {
        let mut harness = settings_harness(SettingsWindow::default());
        let shortcut = harness
            .state()
            .input
            .as_ref()
            .unwrap()
            .settings
            .effective_shortcuts()
            .egui(ShortcutAction::KeyboardShortcuts)
            .unwrap();
        harness.key_press_modifiers(shortcut.modifiers, shortcut.logical_key);
        harness.run();
        let mut settings = AppSettings::default();
        let (actions, close) = harness.state_mut().take_actions(&mut settings);
        assert!(!close);
        assert!(matches!(
            actions.as_slice(),
            [SettingsAction::ShowShortcuts]
        ));
    }

    fn settings_harness(mut window: SettingsWindow) -> Harness<'static, SettingsWindow> {
        let context = egui::Context::default();
        if window.input.is_none() {
            window.synchronize(input(&context), &FontCatalog::default());
        }
        let captures = CaptureController::disabled_for_tests();
        Harness::builder()
            .with_size(Vec2::new(700.0, 3000.0))
            .build_ui_state(
                move |ui, window: &mut SettingsWindow| {
                    window.prepare_keyboard(ui.ctx());
                    if window.close_requested {
                        return;
                    }
                    let actions = window.paint(ui, &captures, &mut false);
                    window.text_input_focused = ui.ctx().text_edit_focused();
                    window.accept_actions(actions);
                    window.consume_shortcuts(ui.ctx(), &captures);
                },
                window,
            )
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
                bundled_program: None,
                command: Default::default(),
                kind: ToolKind::Typst,
                program: "typst".into(),
                origin: ToolOrigin::Bundled,
                fallback_reason: None,
            },
            tex_tools: crate::tex::tools::TexTools::resolve(
                &crate::tex::settings::TexSettings::default(),
            ),
            tinymist_tool: ToolResolution {
                bundled_program: None,
                command: Default::default(),
                kind: ToolKind::Tinymist,
                program: "tinymist".into(),
                origin: ToolOrigin::Bundled,
                fallback_reason: None,
            },
            status: SettingsStatus {
                backend_label: "Interactive",
                preview_failure: None,
                requested_backend: PreviewPreference::Interactive,
                interactive_active: true,
                capabilities: crate::capabilities::CapabilitySnapshot {
                    editing: ServiceState::Ready("ready".into()),
                    lsp: ServiceState::Ready("ready".into()),
                    interactive_preview: ServiceState::Ready("ready".into()),
                    pdf_generation: ServiceState::Ready("ready".into()),
                    pdf_rendering: ServiceState::Ready("ready".into()),
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
    pub(super) text_input_focused: bool,
    menu_commands: Vec<AppCommand>,
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

    pub(super) fn request_close(&mut self) {
        self.close_requested = true;
    }
    pub(super) fn request_search_focus(&mut self) {
        self.ui.json_mode = false;
        self.ui.focus_search = true;
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
                let shortcut = state.consume_shortcuts(ui.ctx(), &child_captures);
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

    fn consume_shortcuts(&mut self, context: &egui::Context, captures: &CaptureController) -> bool {
        if !context.input(|input| {
            input.events.iter().any(|event| {
                matches!(
                    event,
                    egui::Event::Key { pressed: true, .. }
                        | egui::Event::Copy
                        | egui::Event::Cut
                        | egui::Event::Paste(_)
                )
            })
        }) {
            return false;
        }
        let shortcuts = self
            .input
            .as_ref()
            .expect("visible Settings input")
            .settings
            .effective_shortcuts();
        let mut owner_changed = false;
        let mut focus_search = false;
        let mut minimize = false;
        let mut fullscreen = false;
        let mut capture_ui = false;
        context.input_mut(|input| {
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
                match action {
                    Some(ShortcutAction::CloseTab | ShortcutAction::CloseWindow) => {
                        self.close_requested = true;
                        owner_changed = true;
                    }
                    Some(ShortcutAction::Find | ShortcutAction::FindReplace) => {
                        focus_search = true;
                    }
                    Some(ShortcutAction::KeyboardShortcuts) => {
                        self.actions.push(SettingsAction::ShowShortcuts);
                        owner_changed = true;
                    }
                    Some(ShortcutAction::Minimize) => minimize = true,
                    Some(ShortcutAction::ToggleFullscreen) => fullscreen = true,
                    Some(ShortcutAction::CaptureUi) => capture_ui = true,
                    _ => {}
                }
                // TextEdit has already handled its own input. No app shortcut
                // is allowed to escape this focused native viewport.
                !matches!(
                    event,
                    egui::Event::Copy | egui::Event::Cut | egui::Event::Paste(_)
                ) && action.is_none()
            });
        });
        if focus_search {
            self.request_search_focus();
            context.request_repaint();
        }
        if minimize {
            context.send_viewport_cmd(egui::ViewportCommand::Minimized(true));
        }
        if fullscreen {
            let active = context
                .input(|input| input.viewport().fullscreen)
                .unwrap_or(false);
            context.send_viewport_cmd(egui::ViewportCommand::Fullscreen(!active));
        }
        if capture_ui {
            captures.queue_for_viewport(context.viewport_id());
        }
        owner_changed
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
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        #[cfg(not(target_os = "macos"))]
                        {
                            if icon_button(ui, UiIcon::Close, "Close Settings").clicked() {
                                *close = true;
                            }
                            if icon_button(ui, UiIcon::Maximize, "Maximize Settings").clicked() {
                                let maximized = ui
                                    .ctx()
                                    .input(|input| input.viewport().maximized)
                                    .unwrap_or(false);
                                ui.ctx().send_viewport_cmd(egui::ViewportCommand::Maximized(
                                    !maximized,
                                ));
                            }
                        }
                        let label = if self.ui.json_mode {
                            "Show settings form"
                        } else {
                            "Edit settings as JSON"
                        };
                        let json = ui.add(
                            egui::Button::new(RichText::new("{}").monospace())
                                .selected(self.ui.json_mode),
                        );
                        json.widget_info(|| {
                            egui::WidgetInfo::labeled(
                                egui::WidgetType::Button,
                                json.enabled(),
                                label,
                            )
                        });
                        #[cfg(all(feature = "desktop-ui-tests", target_os = "macos"))]
                        crate::desktop_test::observe("settings.json.mode", &json);
                        if json.clicked() {
                            self.ui.json_mode = !self.ui.json_mode;
                        }
                        json.on_hover_text(label);
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
                    tex_tools: &input.tex_tools,
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

pub(super) fn is_widget_edit_command(command: AppCommand) -> bool {
    matches!(
        command,
        AppCommand::Cut
            | AppCommand::Copy
            | AppCommand::Paste
            | AppCommand::Undo
            | AppCommand::Redo
            | AppCommand::SelectAll
    )
}
