//! Independently repainted snippet editor with no document/worker access.
use super::{settings_snippets::Draft, *};

pub(super) const SALT: &str = "tiptoptyp-snippets";
pub(super) fn accepts_native_command(command: AppCommand) -> bool {
    matches!(
        command,
        AppCommand::Save
            | AppCommand::CloseTab
            | AppCommand::Find
            | AppCommand::FindReplace
            | AppCommand::Undo
            | AppCommand::Redo
            | AppCommand::Cut
            | AppCommand::Copy
            | AppCommand::Paste
            | AppCommand::SelectAll
    )
}

#[derive(PartialEq)]
pub(super) struct SnippetWindowInput {
    pub(super) visible: bool,
    pub(super) settings: AppSettings,
    pub(super) appearance: egui::Theme,
    pub(super) style: Arc<egui::Style>,
}
#[derive(Default)]
pub(super) struct SnippetWindow {
    draft: Option<Draft>,
    pending_entry: Option<super::settings_snippets::Selection>,
    selection_error: Option<&'static str>,
    input: Option<SnippetWindowInput>,
    edit: Option<(AppSettings, AppSettings)>,
    close_requested: bool,
    pub(super) focused: bool,
    pub(super) text_input_focused: bool,
    focus_search: bool,
    save_requested: bool,
    menu_commands: Vec<AppCommand>,
    paste_deadline: Option<f64>,
    local_events: Vec<egui::Event>,
    app_shortcuts: Vec<ShortcutAction>,
}
impl SnippetWindow {
    #[cfg(test)]
    pub(super) fn stage_unsaved_for_tests(
        &mut self,
        context: &egui::Context,
        saved: &AppSettings,
        edited: &AppSettings,
    ) {
        let mut draft = Draft::new(&edited.snippets, &edited.snippet_presets);
        draft.restore_saved_base(saved);
        self.draft = Some(draft);
        self.input = Some(SnippetWindowInput {
            visible: true,
            settings: saved.clone(),
            appearance: context.theme(),
            style: context.style_of(context.theme()),
        });
    }

    #[cfg(all(feature = "desktop-ui-tests", target_os = "macos"))]
    pub(super) fn search_query(&self) -> &str {
        self.draft.as_ref().map_or("", Draft::query)
    }
    #[cfg(all(feature = "desktop-ui-tests", target_os = "macos"))]
    pub(super) fn field_fingerprints(&self) -> serde_json::Value {
        let Some(draft) = &self.draft else {
            return serde_json::json!({});
        };
        let mut fields = serde_json::json!({
            "search": crate::desktop_test::fingerprint(draft.query()),
        });
        if let Some(snippet) = draft.selected_snippet() {
            for (field, value) in [
                ("source", snippet.body.as_str()),
                ("prefix", snippet.prefix.as_str()),
                ("opening", snippet.placeholder_open.as_str()),
                ("closing", snippet.placeholder_close.as_str()),
                ("description", snippet.description.as_str()),
            ] {
                fields[field] = crate::desktop_test::fingerprint(value).into();
            }
        }
        fields
    }

    pub(super) fn request_entry(&mut self, selection: super::settings_snippets::Selection) {
        self.pending_entry = Some(selection);
        self.selection_error = None;
    }
    pub(super) fn request_close(&mut self) {
        self.close_requested = true;
    }
    pub(super) fn request_search_focus(&mut self) {
        self.focus_search = true;
    }
    pub(super) fn request_save(&mut self) {
        self.save_requested = true;
    }
    pub(super) fn queue_edit_command(&mut self, command: AppCommand) -> bool {
        if self.input.is_some()
            && self.text_input_focused
            && super::settings_window::is_widget_edit_command(command)
        {
            self.menu_commands.push(command);
            true
        } else {
            false
        }
    }
    pub(super) fn has_actions(&self) -> bool {
        self.edit.is_some() || self.close_requested
    }
    pub(super) fn suspend(&mut self) {
        self.input = None;
        self.focused = false;
        self.text_input_focused = false;
        self.menu_commands.clear();
        self.local_events.clear();
        self.app_shortcuts.clear();
        self.paste_deadline = None;
    }
    pub(super) fn take_actions(&mut self, settings: &mut AppSettings) -> bool {
        // Native menu Save and Close can be queued before the child repaints.
        // Deliver that save before hiding, and never leave it armed on reopen.
        if self.close_requested && std::mem::take(&mut self.save_requested) && self.input.is_some()
        {
            self.save_current_draft();
        }
        if let Some((base, edited)) = self.edit.take() {
            if settings.snippets == base.snippets
                && settings.snippet_presets == base.snippet_presets
            {
                settings.snippets = edited.snippets;
                settings.snippet_presets = edited.snippet_presets;
            } else if let Some(draft) = &mut self.draft {
                // Delivery raced another settings save. Retain the form's work
                // against its original base so the normal conflict UI applies.
                draft.restore_saved_base(&base);
            }
            if let Some(input) = &mut self.input {
                input.settings = settings.clone();
            }
        }
        std::mem::take(&mut self.close_requested)
    }
    fn synchronize(&mut self, input: SnippetWindowInput) -> bool {
        if self.input.as_ref() == Some(&input) {
            return false;
        }
        self.input = Some(input);
        true
    }
    fn accept_edit(&mut self, base: AppSettings, edited: AppSettings) -> bool {
        if base.snippets == edited.snippets && base.snippet_presets == edited.snippet_presets {
            return false;
        }
        self.input.as_mut().unwrap().settings = edited.clone();
        if let Some((_, latest)) = &mut self.edit {
            *latest = edited;
        } else {
            self.edit = Some((base, edited));
        }
        true
    }
    pub(super) fn show(
        shared: &Arc<Mutex<Self>>,
        context: &egui::Context,
        captures: &CaptureController,
        input: SnippetWindowInput,
    ) {
        let changed = shared.lock().unwrap().synchronize(input);
        if changed || captures.has_pending_for("snippets") {
            context.request_repaint_of(scoped_child_viewport_id(context, SALT));
        }
        Self::show_registered(shared, context, captures);
    }
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
            state.focused = false;
            state.text_input_focused = false;
        }
        ChildViewHost::hide(context, SALT);
        Self::show_registered(shared, context, captures);
        true
    }
    fn show_registered(
        shared: &Arc<Mutex<Self>>,
        context: &egui::Context,
        captures: &CaptureController,
    ) {
        let state = shared.lock().unwrap();
        let input = state.input.as_ref().expect("registered snippet input");
        let appearance = input.appearance;
        let style = input.style.clone();
        let visible = input.visible;
        drop(state);
        let spec = ChildViewSpec::persistent(
            SALT,
            "tiptoptyp Snippets",
            [820.0, 760.0],
            [420.0, 420.0],
            "snippets",
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
                let changed = state.paint(ui, &child_captures, &mut close);
                state.text_input_focused = ui.ctx().text_edit_focused();
                let shortcut = state.consume_shortcuts(ui.ctx(), &child_captures);
                if close || state.close_requested {
                    ui.ctx()
                        .send_viewport_cmd(egui::ViewportCommand::CancelClose);
                    ui.ctx()
                        .send_viewport_cmd(egui::ViewportCommand::Visible(false));
                    state.close_requested = true;
                }
                if changed || close || state.close_requested || gained_focus || shortcut {
                    ui.ctx().request_repaint_of(parent);
                }
            },
        );
    }
    fn consume_shortcuts(&mut self, context: &egui::Context, captures: &CaptureController) -> bool {
        if self.app_shortcuts.is_empty()
            && !self.save_requested
            && !context.input(|input| {
                input.events.iter().any(|event| {
                    matches!(
                        event,
                        egui::Event::Key { pressed: true, .. }
                            | egui::Event::Copy
                            | egui::Event::Cut
                            | egui::Event::Paste(_)
                    )
                })
            })
        {
            return false;
        }
        let shortcuts = self
            .input
            .as_ref()
            .expect("visible snippet input")
            .settings
            .effective_shortcuts();
        let mut actions = std::mem::take(&mut self.app_shortcuts);
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
                if let Some(action) = action {
                    actions.push(action);
                }
                // Native text events belong to this window's fields. Their
                // editing commands have already run in their original order.
                !matches!(
                    event,
                    egui::Event::Copy | egui::Event::Cut | egui::Event::Paste(_)
                ) && action.is_none()
            });
        });
        let mut owner_changed = false;
        for action in actions {
            match action {
                ShortcutAction::CloseTab | ShortcutAction::CloseWindow => {
                    self.close_requested = true;
                    owner_changed = true;
                }
                ShortcutAction::Find | ShortcutAction::FindReplace => {
                    self.request_search_focus();
                    context.request_repaint();
                }
                ShortcutAction::Save => self.save_requested = true,
                ShortcutAction::Minimize => {
                    context.send_viewport_cmd(egui::ViewportCommand::Minimized(true))
                }
                ShortcutAction::ToggleFullscreen => {
                    let active = context
                        .input(|input| input.viewport().fullscreen)
                        .unwrap_or(false);
                    context.send_viewport_cmd(egui::ViewportCommand::Fullscreen(!active));
                }
                ShortcutAction::CaptureUi => {
                    captures.queue_for_viewport(context.viewport_id());
                }
                _ => {}
            }
        }
        if std::mem::take(&mut self.save_requested) {
            owner_changed |= self.save_current_draft();
        }
        owner_changed
    }

    fn save_current_draft(&mut self) -> bool {
        let original = self
            .input
            .as_ref()
            .expect("visible snippet input")
            .settings
            .clone();
        let mut edited = original.clone();
        if self
            .draft
            .as_mut()
            .is_some_and(|draft| draft.save(&mut edited))
        {
            self.accept_edit(original, edited)
        } else {
            false
        }
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
                .expect("visible snippet input")
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
            // An application chord such as Cmd+Alt+Z must never reach a
            // TextEdit that would interpret it as Undo before our router runs.
            context.input_mut(|input| {
                input.events.retain(|event| {
                    let action = match event {
                        egui::Event::Key {
                            key,
                            modifiers,
                            pressed: true,
                            ..
                        } => shortcuts.action_for_key_event(*key, *modifiers),
                        _ => None,
                    };
                    if let Some(action) = action.filter(|action| !is_widget_edit_action(*action)) {
                        self.app_shortcuts.push(action);
                        false
                    } else {
                        true
                    }
                });
            });
            if self.text_input_focused {
                let mut request_paste = false;
                // A child key belongs at its current position among native text
                // chunks. Only older parent menu commands are prepended below.
                context.input_mut(|input| {
                    input.events.retain_mut(|event| {
                        let command = match event {
                            egui::Event::Key {
                                key,
                                modifiers,
                                pressed: true,
                                ..
                            } => match shortcuts.action_for_key_event(*key, *modifiers) {
                                Some(ShortcutAction::Cut) => Some(AppCommand::Cut),
                                Some(ShortcutAction::Copy) => Some(AppCommand::Copy),
                                Some(ShortcutAction::Paste) => Some(AppCommand::Paste),
                                Some(ShortcutAction::Undo) => Some(AppCommand::Undo),
                                Some(ShortcutAction::Redo) => Some(AppCommand::Redo),
                                Some(ShortcutAction::SelectAll) => Some(AppCommand::SelectAll),
                                _ => None,
                            },
                            _ => None,
                        };
                        let Some(command) = command else {
                            return true;
                        };
                        if command == AppCommand::Paste {
                            request_paste = true;
                            return false;
                        }
                        if let Some(normalized) = snippet_edit_event(command) {
                            self.local_events.push(normalized.clone());
                            *event = normalized;
                        }
                        true
                    });
                });
                if request_paste {
                    self.paste_deadline = Some(now + 1.0);
                    context.send_viewport_cmd(egui::ViewportCommand::RequestPaste);
                }
            }
        }
        let mut events = Vec::new();
        for command in std::mem::take(&mut self.menu_commands) {
            if command == AppCommand::Paste {
                self.paste_deadline = Some(context.input(|input| input.time) + 1.0);
                context.send_viewport_cmd(egui::ViewportCommand::RequestPaste);
            }
            if let Some(event) = snippet_edit_event(command) {
                self.local_events.push(event.clone());
                events.push(event);
            }
        }
        crate::child_view::deliver_queued_edit_events(context, &mut events);
    }

    fn paint(&mut self, ui: &mut egui::Ui, captures: &CaptureController, close: &mut bool) -> bool {
        ui.painter()
            .rect_filled(ui.max_rect(), 0.0, ui.visuals().panel_fill);
        let title = Rect::from_min_size(
            ui.max_rect().min,
            egui::vec2(ui.max_rect().width(), METRICS.chrome.toolbar_height),
        );
        if ui
            .interact(title, ui.id().with("snippet-window-drag"), Sense::drag())
            .drag_started()
        {
            ui.ctx().send_viewport_cmd(egui::ViewportCommand::StartDrag);
        }
        egui::Panel::top("snippet-titlebar")
            .exact_size(METRICS.chrome.toolbar_height)
            .frame(theme::settings_title_frame(ui.style()))
            .show(ui, |ui| {
                ui.horizontal_centered(|ui| {
                    #[cfg(target_os = "macos")]
                    theme::reserve_window_controls(ui);
                    crate::window_logo::show(ui, captures);
                    ui.label(RichText::new("Snippets").strong());
                    #[cfg(not(target_os = "macos"))]
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if super::icons::square_icon_button(
                            ui,
                            UiIcon::Close,
                            "Close Snippets",
                            METRICS.icon.button_size.x,
                        )
                        .clicked()
                        {
                            *close = true;
                        }
                        if super::icons::square_icon_button(
                            ui,
                            UiIcon::Maximize,
                            "Maximize Snippets",
                            METRICS.icon.button_size.x,
                        )
                        .clicked()
                        {
                            let maximized = ui
                                .ctx()
                                .input(|input| input.viewport().maximized)
                                .unwrap_or(false);
                            ui.ctx()
                                .send_viewport_cmd(egui::ViewportCommand::Maximized(!maximized));
                        }
                    });
                });
            });
        let _ = close;
        let original = self
            .input
            .as_ref()
            .expect("visible snippet input")
            .settings
            .clone();
        let mut edited = original.clone();
        let draft = self
            .draft
            .get_or_insert_with(|| Draft::new(&edited.snippets, &edited.snippet_presets));
        if let Some(selection) = &self.pending_entry {
            match draft.select_saved_if_current(&original, selection) {
                Ok(()) => {
                    self.pending_entry = None;
                    self.selection_error = None;
                }
                Err(error) => self.selection_error = Some(error),
            }
        }
        if std::mem::take(&mut self.focus_search) {
            draft.request_search_focus();
        }
        egui::CentralPanel::default()
            .frame(theme::settings_content_frame(ui.style()))
            .show(ui, |ui| {
                // The scrolling content has an unbounded max rect. Size its
                // list from the actual finite window before entering it.
                let viewport_height = ui.available_height();
                egui::ScrollArea::vertical()
                    .id_salt("snippet-window-content")
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        if let Some(error) = self.selection_error {
                            ui.colored_label(ui.visuals().error_fg_color, error);
                        }
                        super::settings_snippets::show(
                            ui,
                            &mut self.draft,
                            &mut edited,
                            &original,
                            viewport_height,
                        );
                    });
            });
        self.accept_edit(original, edited)
    }
}
fn snippet_edit_event(command: AppCommand) -> Option<egui::Event> {
    match command {
        AppCommand::Cut => Some(egui::Event::Cut),
        AppCommand::Copy => Some(egui::Event::Copy),
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
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui_kittest::kittest::NodeT as _;
    use egui_kittest::{Harness, kittest::Queryable as _};

    fn input(context: &egui::Context) -> SnippetWindowInput {
        SnippetWindowInput {
            visible: true,
            settings: AppSettings::default(),
            appearance: egui::Theme::Light,
            style: context.style_of(egui::Theme::Light),
        }
    }
    fn harness() -> Harness<'static, SnippetWindow> {
        harness_size(egui::vec2(840.0, 1050.0))
    }
    fn harness_size(size: egui::Vec2) -> Harness<'static, SnippetWindow> {
        let captures = CaptureController::disabled_for_tests();
        let mut state = SnippetWindow::default();
        state.synchronize(input(&egui::Context::default()));
        let mut configured = false;
        let mut harness = Harness::builder().with_size(size).build_ui_state(
            move |ui, state: &mut SnippetWindow| {
                if !configured {
                    theme::configure_test_fonts(ui.ctx());
                    configured = true;
                    return;
                }
                state.prepare_keyboard(ui.ctx());
                if state.close_requested {
                    return;
                }
                let mut close = false;
                state.paint(ui, &captures, &mut close);
                state.text_input_focused = ui.ctx().text_edit_focused();
                state.consume_shortcuts(ui.ctx(), &captures);
                state.close_requested |= close;
            },
            state,
        );
        harness.run_steps(2);
        harness
    }
    fn replace(harness: &mut Harness<'static, SnippetWindow>, label: &str, text: &str) {
        harness.get_by_label(label).click();
        harness.run();
        harness.key_press_modifiers(Modifiers::COMMAND, egui::Key::A);
        harness.run();
        harness.get_by_label(label).type_text(text);
        harness.run();
    }
    #[test]
    fn real_window_list_grows_between_small_and_large_viewports_and_source_remains_reachable() {
        let mut small = harness_size(egui::vec2(840.0, 560.0));
        let large = harness_size(egui::vec2(840.0, 1050.0));
        let first_label = "mk · TeX · Suggested · Inline math with an editable expression";
        let small_first = small.get_by_label(first_label).rect();
        let large_first = large.get_by_label(first_label).rect();
        let small_source = small.get_by_label("Snippet source").rect();
        let large_source = large.get_by_label("Snippet source").rect();
        assert!(
            large_source.top() - large_first.top() > small_source.top() - small_first.top() + 70.0,
            "the real scrolling window must not clamp both lists to twenty rows: small {small_source:?}, large {large_source:?}"
        );
        assert!(large_source.bottom() <= 1050.0, "{large_source:?}");
        let save = small.get_by_label("Save snippets").rect();
        assert!(save.top() >= 0.0 && save.bottom() <= 560.0);
        small.get_by_label("Snippet source").scroll_to_me();
        small.run_steps(4);
        let source = small.get_by_label("Snippet source").rect();
        assert!(
            source.top() >= 0.0 && source.bottom() <= 560.0,
            "{source:?}"
        );
        small.key_press_modifiers(Modifiers::COMMAND, egui::Key::F);
        small.run_steps(3);
        assert!(small.get_by_label("Search snippets").is_focused());
        let search = small.get_by_label("Search snippets").rect();
        assert!(search.top() >= 0.0 && search.bottom() <= 560.0);
    }

    #[test]
    fn snippet_window_owns_find_and_close_without_discarding_its_draft() {
        let mut harness = harness();
        replace(&mut harness, "Prefix", "retaineddraft");
        harness.key_press_modifiers(Modifiers::COMMAND, egui::Key::F);
        harness.run_steps(2);
        assert!(harness.get_by_label("Search snippets").is_focused());
        harness.get_by_label("Search snippets").type_text("frac");
        harness.run();
        assert_eq!(
            harness.get_by_label("Search snippets").value().as_deref(),
            Some("frac")
        );
        harness.key_press_modifiers(Modifiers::COMMAND, egui::Key::W);
        harness.run();
        let mut settings = AppSettings::default();
        assert!(harness.state_mut().take_actions(&mut settings));
        assert_eq!(settings.snippet_presets[0].prefix, "mk");
        harness.state_mut().suspend();
        let snapshot = input(&harness.ctx);
        harness.state_mut().synchronize(snapshot);
        harness.run();
        assert_eq!(
            harness.get_by_label("Prefix").value().as_deref(),
            Some("retaineddraft")
        );
        assert_eq!(
            harness.get_by_label("Search snippets").value().as_deref(),
            Some("frac")
        );
    }
    #[test]
    fn typed_placeholder_pair_and_rewritten_source_survive_closing_then_save_with_linked_fields() {
        let mut harness = harness();
        replace(
            &mut harness,
            "Snippet source",
            r"\frac{{{1:left}}}{{{2:right}}} + {{1}}{{0}}",
        );
        replace(&mut harness, "Opening delimiter", "[[");
        replace(&mut harness, "Closing delimiter", "]]");
        harness.get_by_label("Rewrite markers").click();
        harness.run();
        let expected = r"\frac{[[1:left]]}{[[2:right]]} + [[1]][[0]]";
        assert_eq!(
            harness.get_by_label("Snippet source").value().as_deref(),
            Some(expected)
        );
        harness.key_press_modifiers(Modifiers::COMMAND, egui::Key::W);
        harness.run();
        let mut settings = AppSettings::default();
        assert!(harness.state_mut().take_actions(&mut settings));
        assert_eq!(settings.snippet_presets[0].placeholder_open, "{{");
        harness.state_mut().suspend();
        let snapshot = input(&harness.ctx);
        harness.state_mut().synchronize(snapshot);
        harness.run();
        assert_eq!(
            harness.get_by_label("Opening delimiter").value().as_deref(),
            Some("[[")
        );
        assert_eq!(
            harness.get_by_label("Closing delimiter").value().as_deref(),
            Some("]]")
        );
        assert_eq!(
            harness.get_by_label("Snippet source").value().as_deref(),
            Some(expected)
        );
        harness.key_press_modifiers(Modifiers::COMMAND, egui::Key::S);
        harness.run();
        assert!(!harness.state_mut().take_actions(&mut settings));
        let saved = &settings.snippet_presets[0];
        assert_eq!(saved.placeholder_open, "[[");
        assert_eq!(saved.placeholder_close, "]]");
        assert_eq!(saved.body, expected);
        let expansion =
            crate::completion_edit::expand_lsp_snippet(&saved.source_to_lsp().unwrap()).unwrap();
        assert_eq!(expansion.text, r"\frac{left}{right} + left");
        assert_eq!(
            expansion.fields.iter().filter(|(id, _)| *id == 1).count(),
            2
        );
    }

    #[test]
    fn find_shortcut_reveals_search_after_scrolling_a_long_snippet() {
        let mut harness = harness();
        replace(&mut harness, "Snippet source", &"x\n".repeat(100));
        harness.get_by_label("How snippets work").scroll_to_me();
        harness.run();
        assert!(harness.get_by_label("Search snippets").rect().top() < 0.0);
        harness.key_press_modifiers(Modifiers::COMMAND, egui::Key::F);
        harness.run_steps(3);
        let search = harness.get_by_label("Search snippets");
        assert!(search.is_focused());
        assert!(search.rect().top() >= 0.0 && search.rect().bottom() <= 1050.0);
    }
    #[test]
    fn save_shortcut_publishes_snippets_only_and_preserves_unrelated_new_preferences() {
        let mut harness = harness();
        replace(&mut harness, "Prefix", "savedbyshortcut");
        harness.key_press_modifiers(Modifiers::COMMAND, egui::Key::S);
        harness.run_steps(2);
        assert!(harness.state().has_actions());
        let mut settings = AppSettings {
            line_wrap: false,
            ui_scale_percent: 125,
            ..Default::default()
        };
        assert!(!harness.state_mut().take_actions(&mut settings));
        assert_eq!(settings.snippet_presets[0].prefix, "savedbyshortcut");
        assert!(!settings.line_wrap);
        assert_eq!(settings.ui_scale_percent, 125);
        harness.run_steps(2);
        assert!(
            !harness.state().has_actions(),
            "idle frames cannot duplicate save delivery"
        );
    }
    #[test]
    fn save_and_close_in_one_input_batch_include_the_last_typed_text() {
        let mut harness = harness();
        replace(&mut harness, "Prefix", "last");
        harness.input_mut().events.extend([
            egui::Event::Text("typed".into()),
            egui::Event::Key {
                key: egui::Key::S,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: Modifiers::COMMAND,
            },
            egui::Event::Key {
                key: egui::Key::W,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: Modifiers::COMMAND,
            },
        ]);
        harness.run();
        let mut settings = AppSettings::default();
        assert!(harness.state_mut().take_actions(&mut settings));
        assert_eq!(settings.snippet_presets[0].prefix, "lasttyped");
    }
    #[test]
    fn native_menu_select_all_keeps_fifo_typing_local_and_document_commands_are_consumed() {
        let mut harness = harness();
        harness.get_by_label("Prefix").click();
        harness.run();
        assert!(
            harness
                .state_mut()
                .queue_edit_command(AppCommand::SelectAll)
        );
        harness.get_by_label("Prefix").type_text("localprefix");
        harness.run_steps(2);
        assert_eq!(
            harness.get_by_label("Prefix").value().as_deref(),
            Some("localprefix")
        );
        let bindings = harness
            .state()
            .input
            .as_ref()
            .unwrap()
            .settings
            .effective_shortcuts();
        for action in [
            ShortcutAction::New,
            ShortcutAction::Panel,
            ShortcutAction::Compile,
            ShortcutAction::ToggleLineWrap,
        ] {
            let shortcut = bindings.egui(action).unwrap();
            harness.key_press_modifiers(shortcut.modifiers, shortcut.logical_key);
            harness.run();
            assert!(!harness.state().has_actions(), "{action:?}");
            assert_eq!(
                harness.get_by_label("Prefix").value().as_deref(),
                Some("localprefix"),
                "{action:?}"
            );
        }
        assert!(harness.state().menu_commands.is_empty());
    }
    #[test]
    fn save_delivery_race_keeps_the_unsaved_draft_and_newer_json_snippets() {
        let mut harness = harness();
        replace(&mut harness, "Prefix", "formdraft");
        harness.get_by_label("Save snippets").click();
        harness.run();
        assert!(harness.state().has_actions());
        let mut settings = AppSettings::default();
        settings.snippet_presets[0].prefix = "newerjson".into();
        assert!(!harness.state_mut().take_actions(&mut settings));
        harness.run();
        assert_eq!(settings.snippet_presets[0].prefix, "newerjson");
        assert_eq!(
            harness.get_by_label("Prefix").value().as_deref(),
            Some("formdraft")
        );
        assert!(
            harness
                .get_by_label("Save snippets")
                .accesskit_node()
                .is_disabled()
        );
        harness.get_by_label("Revert changes").click();
        harness.run();
        assert_eq!(
            harness.get_by_label("Prefix").value().as_deref(),
            Some("newerjson")
        );
    }
    #[test]
    fn native_save_then_close_saves_once_and_cannot_replay_on_reopen() {
        let mut harness = harness();
        replace(&mut harness, "Prefix", "savedbeforeclose");
        harness.state_mut().request_save();
        harness.state_mut().request_close();
        let mut settings = AppSettings::default();
        assert!(harness.state_mut().take_actions(&mut settings));
        assert_eq!(settings.snippet_presets[0].prefix, "savedbeforeclose");
        assert!(!harness.state().save_requested);
        harness.state_mut().suspend();
        let mut reopened = input(&harness.ctx);
        reopened.settings = settings.clone();
        harness.state_mut().synchronize(reopened);
        harness.run_steps(2);
        assert!(!harness.state().has_actions());
        replace(&mut harness, "Prefix", "unsavedafterreopen");
        harness.state_mut().request_close();
        assert!(harness.state_mut().take_actions(&mut settings));
        assert_eq!(settings.snippet_presets[0].prefix, "savedbeforeclose");
    }

    #[test]
    fn deferred_snippet_window_is_resizable_and_registered_independently() {
        let context = egui::Context::default();
        context.set_embed_viewports(false);
        let state = Arc::new(Mutex::new(SnippetWindow::default()));
        let captures = CaptureController::disabled_for_tests();
        let child = scoped_child_viewport_id(&context, SALT);
        let mut output = context.run_ui(Default::default(), |ui| {
            SnippetWindow::show(&state, ui.ctx(), &captures, input(ui.ctx()));
        });
        assert!(output.viewport_output[&child].viewport_ui_cb.is_some());
        assert_ne!(child, egui::ViewportId::ROOT);
        assert_ne!(
            output.viewport_output[&child].builder.resizable,
            Some(false)
        );
        output.textures_delta.clear();
        let mut hidden = context.run_ui(Default::default(), |ui| {
            assert!(SnippetWindow::retain_hidden(&state, ui.ctx(), &captures));
        });
        assert!(!state.lock().unwrap().input.as_ref().unwrap().visible);
        assert_eq!(hidden.viewport_output[&child].builder.visible, Some(false));
        hidden.textures_delta.clear();
    }
}
