//! One-process document-window orchestration.
//!
//! eframe owns one root `winit` window and exposes additional native windows as
//! egui viewports. Each viewport below owns a complete [`EditorApp`] session:
//! source buffer, undo history, compiler, Tinymist process, workspace, and
//! dirty-close flow never cross session boundaries.

use std::{
    cell::{Ref, RefMut},
    collections::VecDeque,
    path::PathBuf,
};

mod document_host;
use document_host::DocumentHost;

use eframe::egui;

use crate::{
    app::{EditorApp, EditorWindowRequest},
    launch::LaunchMode,
    native_menu::{AppCommand, NativeMenuReceiver, NativeMenuRequest},
    open_requests::OpenRequestReceiver,
    screenshot::{CaptureController, CaptureThemeProfile, UiCaptureStep, UiSnapshotScene},
    settings::{AppSettings, normalize_workspace_root},
    theme,
};

const FIRST_SECONDARY_SESSION_ID: u64 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ActiveSession {
    Primary,
    Secondary(u64),
}

struct SecondaryWindow {
    id: u64,
    editor: DocumentHost,
    activate_once: bool,
}

impl SecondaryWindow {
    fn viewport_id(&self) -> egui::ViewportId {
        document_viewport_id(self.id)
    }
}

struct PendingWindow {
    request: EditorWindowRequest,
    settings: AppSettings,
}

struct CaptureBatch {
    remaining: VecDeque<UiCaptureStep>,
    active_request: u64,
    active_scene: UiSnapshotScene,
    close_when_finished: bool,
}

fn root_capture_needs_warmup(previous: UiSnapshotScene, next: UiSnapshotScene) -> bool {
    previous.viewport_target() != "main" && next.viewport_target() == "main"
}

/// Application shell which keeps the eframe root editor and all additional
/// document viewports in the same process and on the same event loop.
pub(crate) struct AppShell {
    primary: DocumentHost,
    closing: tiptoptyp_core::closing::CloseCoordinator,
    secondary: Vec<SecondaryWindow>,
    active: ActiveSession,
    next_session_id: u64,
    pending_windows: VecDeque<PendingWindow>,
    open_requests: OpenRequestReceiver,
    native_menu_commands: NativeMenuReceiver,
    captures: CaptureController,
    capture_batch: Option<CaptureBatch>,
    shared_settings: AppSettings,
    launch_mode: LaunchMode,
    primary_visible: bool,
    quit_requested: bool,
}

impl AppShell {
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new(
        context: &eframe::CreationContext<'_>,
        initial_path: Option<PathBuf>,
        captures: CaptureController,
        theme_override: Option<CaptureThemeProfile>,
        snapshot_scene: Option<UiSnapshotScene>,
        capture_steps: Vec<UiCaptureStep>,
        launch_mode: LaunchMode,
        open_requests: OpenRequestReceiver,
        native_menu_commands: NativeMenuReceiver,
    ) -> Self {
        let primary = EditorApp::new(
            context,
            initial_path,
            captures.clone(),
            theme_override,
            snapshot_scene,
        );
        let shared_settings = primary.settings_snapshot();
        let pending_windows = if crate::performance::take_multi_window_request() {
            (0..3)
                .map(|_| PendingWindow {
                    request: EditorWindowRequest::Open(primary.document_path_for_profile()),
                    settings: primary.settings_snapshot(),
                })
                .collect()
        } else {
            VecDeque::new()
        };
        open_requests.set_repaint_context(context.egui_ctx.clone());
        let mut remaining = VecDeque::from(capture_steps);
        let capture_batch = remaining.pop_front().map(|first| CaptureBatch {
            active_request: captures
                .queue_step(&first)
                .expect("a screenshot batch enables its capture controller"),
            active_scene: first.scene,
            remaining,
            close_when_finished: captures.closes_after_captures(),
        });
        Self {
            primary: DocumentHost::new(primary),
            closing: Default::default(),
            secondary: Vec::new(),
            active: ActiveSession::Primary,
            next_session_id: FIRST_SECONDARY_SESSION_ID,
            pending_windows,
            open_requests,
            native_menu_commands,
            captures,
            capture_batch,
            shared_settings,
            launch_mode,
            primary_visible: true,
            quit_requested: false,
        }
    }

    fn advance_capture_batch(&mut self, context: &egui::Context) {
        let Some(mut batch) = self.capture_batch.take() else {
            return;
        };
        let Some(result) = self.captures.take_result(batch.active_request) else {
            self.capture_batch = Some(batch);
            return;
        };

        if let Some(error) = result.error {
            self.primary
                .borrow_mut()
                .show_window_notice(format!("UI screenshot batch failed: {error}"));
            if batch.close_when_finished {
                context.send_viewport_cmd(egui::ViewportCommand::Close);
            }
            return;
        }

        let Some(next) = batch.remaining.pop_front() else {
            if batch.close_when_finished {
                context.send_viewport_cmd(egui::ViewportCommand::Close);
            } else {
                self.primary
                    .borrow_mut()
                    .show_window_notice("UI screenshot batch completed".to_owned());
            }
            return;
        };
        if root_capture_needs_warmup(batch.active_scene, next.scene) {
            self.captures
                .queue_viewport_warmup(next.scene.viewport_target())
                .expect("an active screenshot batch keeps captures enabled");
        }
        let request = self
            .captures
            .queue_step(&next)
            .expect("an active screenshot batch keeps captures enabled");
        self.primary.borrow_mut().set_capture_step(&next, context);
        batch.active_request = request;
        batch.active_scene = next.scene;
        self.capture_batch = Some(batch);
        context.request_repaint();
    }

    fn active_editor_mut(&self) -> RefMut<'_, EditorApp> {
        match self.active {
            ActiveSession::Primary => self.primary.borrow_mut(),
            ActiveSession::Secondary(id) => self
                .secondary
                .iter()
                .find(|window| window.id == id)
                .map_or_else(
                    || self.primary.borrow_mut(),
                    |window| window.editor.borrow_mut(),
                ),
        }
    }

    fn active_editor(&self) -> Ref<'_, EditorApp> {
        match self.active {
            ActiveSession::Primary => self.primary.borrow(),
            ActiveSession::Secondary(id) => self
                .secondary
                .iter()
                .find(|window| window.id == id)
                .map_or_else(|| self.primary.borrow(), |window| window.editor.borrow()),
        }
    }

    fn dispatch_process_requests(&mut self, context: &egui::Context) {
        while let Ok(request) = self.native_menu_commands.try_recv() {
            match process_request_action(request) {
                ProcessRequestAction::Editor(command) => {
                    if !self.command_available(context, command) {
                        continue;
                    }
                    if command == AppCommand::Settings {
                        self.primary.borrow_mut().open_global_settings(context);
                        continue;
                    }
                    // With no document windows, the clean retained root is
                    // itself the next new window; no invisible parent UI pass
                    // is needed to instantiate a secondary viewport.
                    let command = if !self.primary_visible && self.secondary.is_empty() {
                        match command {
                            AppCommand::NewWindow => AppCommand::New,
                            AppCommand::OpenInNewWindow => AppCommand::Open,
                            command => command,
                        }
                    } else {
                        command
                    };
                    if self.active == ActiveSession::Primary
                        && !self.primary_visible
                        && command_reveals_primary(command)
                        && !self.command_targets_hidden_root_child(context, command)
                    {
                        self.show_primary(context);
                    }
                    self.active_editor_mut()
                        .enqueue_native_menu_command(command);
                    context.request_repaint_of(self.active_viewport());
                }
                ProcessRequestAction::Reopen => self.reopen_document_window(context),
                ProcessRequestAction::CloseProcess => {
                    self.quit_requested = true;
                    context
                        .send_viewport_cmd_to(egui::ViewportId::ROOT, egui::ViewportCommand::Close);
                }
            }
        }

        let mut reused_primary = false;
        while let Ok(path) = self.open_requests.try_recv() {
            if !reused_primary
                && self.secondary.is_empty()
                && self.primary.borrow().can_reuse_for_external_open()
            {
                if path.is_dir() {
                    self.primary
                        .borrow_mut()
                        .reuse_dormant_window(EditorWindowRequest::Open(path));
                } else {
                    self.primary.borrow_mut().open_external_path(path);
                }
                self.show_primary(context);
                reused_primary = true;
            } else {
                let settings = self.active_editor_mut().settings_snapshot();
                self.pending_windows.push_back(PendingWindow {
                    request: EditorWindowRequest::Open(path),
                    settings,
                });
            }
        }
    }

    fn collect_window_request_from(
        pending_windows: &mut VecDeque<PendingWindow>,
        editor: &mut EditorApp,
    ) {
        while let Some(request) = editor.take_window_request() {
            pending_windows.push_back(PendingWindow {
                request,
                settings: editor.settings_snapshot(),
            });
        }
    }

    fn open_pending_windows(&mut self, context: &egui::Context) {
        while let Some(PendingWindow { request, settings }) = self.pending_windows.pop_front() {
            let id = self.next_session_id;
            self.next_session_id = self
                .next_session_id
                .wrapping_add(1)
                .max(FIRST_SECONDARY_SESSION_ID);
            let editor = EditorApp::new_secondary(
                context,
                document_viewport_id(id),
                request,
                settings,
                self.captures.clone(),
            );
            self.secondary.push(SecondaryWindow {
                id,
                editor: DocumentHost::new(editor),
                activate_once: true,
            });
            self.active = ActiveSession::Secondary(id);
        }
    }

    fn merge_pending_settings(&mut self, target: &mut AppSettings) {
        // Apply independent edits from every owner; the active owner wins only
        // when two pending edits change the same field.
        if self.active != ActiveSession::Primary {
            self.primary.borrow_mut().merge_settings_update(target);
        }
        for window in &mut self.secondary {
            if self.active != ActiveSession::Secondary(window.id) {
                window.editor.borrow_mut().merge_settings_update(target);
            }
        }
        self.active_editor_mut().merge_settings_update(target);
    }

    fn merge_all_session_history(&self, settings: &mut AppSettings, active_settings: &AppSettings) {
        let mut inactive_settings = Vec::with_capacity(self.secondary.len());
        if self.active != ActiveSession::Primary {
            inactive_settings.push(self.primary.borrow().settings_snapshot());
        }
        for window in &self.secondary {
            if self.active != ActiveSession::Secondary(window.id) {
                inactive_settings.push(window.editor.borrow().settings_snapshot());
            }
        }
        merge_session_histories(settings, inactive_settings.iter(), active_settings);
    }

    fn synchronize_workspace_history_removals(&mut self) {
        let mut removals = self.primary.borrow_mut().take_workspace_history_removals();
        for window in &mut self.secondary {
            removals.extend(window.editor.borrow_mut().take_workspace_history_removals());
        }
        removals.sort();
        removals.dedup();
        if removals.is_empty() {
            return;
        }
        for root in &removals {
            self.shared_settings.forget_workspace(root);
        }
        self.primary.forget_workspaces(&removals);
        for window in &mut self.secondary {
            window.editor.forget_workspaces(&removals);
        }
    }

    fn synchronize_settings(&mut self, context: &egui::Context) {
        // A removal is an explicit global action. Apply it to every session
        // before MRU merging so a stale sibling list cannot resurrect it.
        self.synchronize_workspace_history_removals();
        if !self.primary.borrow().has_settings_update()
            && !self
                .secondary
                .iter()
                .any(|window| window.editor.borrow().has_settings_update())
        {
            return;
        }
        // Snapshot this before draining pending updates: the active window is
        // authoritative for per-workspace document history even when another
        // window happened to submit the shared preference change.
        let active_settings = self.active_editor().settings_snapshot();
        let mut settings = self.shared_settings.clone();
        self.merge_pending_settings(&mut settings);
        self.merge_all_session_history(&mut settings, &active_settings);
        self.shared_settings = settings.clone();
        self.primary.queue_settings(settings.clone());
        context.request_repaint_of(egui::ViewportId::ROOT);
        for window in &mut self.secondary {
            window.editor.queue_settings(settings.clone());
            context.request_repaint_of(window.viewport_id());
        }
    }

    fn collect_settings_requests(&mut self, context: &egui::Context) {
        let mut requested = self.primary.borrow_mut().take_settings_open_request();
        for window in &self.secondary {
            requested |= window.editor.borrow_mut().take_settings_open_request();
        }
        if requested {
            self.primary.borrow_mut().open_global_settings(context);
        }
    }

    fn document_keys(&self) -> Vec<crate::document::DocumentKey> {
        std::iter::once(self.primary.borrow().document_key())
            .chain(
                self.secondary
                    .iter()
                    .map(|window| window.editor.borrow().document_key()),
            )
            .collect()
    }
    fn cancel_process_close(&mut self) {
        self.closing.cancel();
        self.quit_requested = false;
        self.primary.borrow_mut().finish_process_close(false);
        for window in &mut self.secondary {
            window.editor.borrow_mut().finish_process_close(false);
        }
    }
    fn request_next_close(&mut self, context: &egui::Context) {
        let Some(key) = self.closing.current() else {
            return;
        };
        let started = if self.primary.borrow().document_key().owner == key.owner {
            if self.primary.borrow().is_dirty_for_close() {
                self.show_primary(context);
            }
            self.active = ActiveSession::Primary;
            context.send_viewport_cmd_to(egui::ViewportId::ROOT, egui::ViewportCommand::Focus);
            self.primary.borrow_mut().begin_process_close()
        } else if let Some(window) = self
            .secondary
            .iter_mut()
            .find(|window| window.editor.borrow().document_key().owner == key.owner)
        {
            self.active = ActiveSession::Secondary(window.id);
            window.activate_once = true;
            context.request_repaint_of(window.viewport_id());
            window.editor.borrow_mut().begin_process_close()
        } else {
            false
        };
        if !started {
            self.cancel_process_close();
            self.primary.borrow_mut().show_window_notice(
                "Closing canceled: finish the active document operation and try again".to_owned(),
            );
        }
        context.request_repaint();
    }
    fn guard_process_close(&mut self, context: &egui::Context) {
        if !context.input(|input| input.viewport().close_requested()) {
            return;
        }
        if keeps_running_after_root_close(
            cfg!(target_os = "macos"),
            self.launch_mode,
            self.quit_requested,
        ) {
            context.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            return;
        }
        if self.closing.ready(&self.document_keys()) {
            if crate::worker::has_active_operations() {
                context.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            }
            return;
        }
        if self.closing.is_active() && self.closing.current().is_none() {
            self.cancel_process_close();
            context.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            self.primary.borrow_mut().show_window_notice(
                "Closing canceled because a document changed after confirmation".to_owned(),
            );
            return;
        }
        if !self.closing.is_active()
            && !crate::worker::has_active_operations()
            && !self.primary.borrow().is_dirty_for_close()
            && !self
                .secondary
                .iter()
                .any(|window| window.editor.borrow().is_dirty_for_close())
        {
            return;
        }
        context.send_viewport_cmd(egui::ViewportCommand::CancelClose);
        if self.closing.begin(self.document_keys()) {
            self.request_next_close(context);
        }
    }

    fn finish_primary_window_close(&mut self, context: &egui::Context, close_requested: bool) {
        if !close_requested
            || !keeps_running_after_root_close(
                cfg!(target_os = "macos"),
                self.launch_mode,
                self.quit_requested,
            )
            || self.primary.borrow().process_close_pending()
            || (self.primary.borrow().is_dirty_for_close()
                && !self.primary.borrow().close_accepted())
        {
            return;
        }
        self.retire_primary_window(context);
    }

    fn retire_primary_window(&mut self, context: &egui::Context) {
        context.send_viewport_cmd_to(egui::ViewportId::ROOT, egui::ViewportCommand::CancelClose);
        context.send_viewport_cmd_to(
            egui::ViewportId::ROOT,
            egui::ViewportCommand::Visible(false),
        );
        self.primary.borrow_mut().finish_window_close();
        self.primary_visible = false;
        if self.active == ActiveSession::Primary
            && let Some(window) = self.secondary.last()
        {
            self.active = ActiveSession::Secondary(window.id);
        }
    }

    fn show_primary(&mut self, context: &egui::Context) {
        self.primary.borrow_mut().request_window_resume();
        if !self.primary_visible {
            context
                .send_viewport_cmd_to(egui::ViewportId::ROOT, egui::ViewportCommand::Visible(true));
        }
        context.send_viewport_cmd_to(
            egui::ViewportId::ROOT,
            egui::ViewportCommand::Minimized(false),
        );
        context.send_viewport_cmd_to(egui::ViewportId::ROOT, egui::ViewportCommand::Focus);
        self.primary_visible = true;
        self.active = ActiveSession::Primary;
    }

    fn refresh_active_window(&mut self, context: &egui::Context) {
        self.active = surviving_active_session(
            self.active,
            self.primary_visible,
            self.secondary.iter().map(|window| window.id),
        );
        // Read current focus before dispatching queued process-level commands,
        // not just after painting the owner that was active in the last pass.
        if let Some(window) = self.secondary.iter().find(|window| {
            crate::app::owner_has_focused_viewport(context, window.viewport_id(), true)
        }) {
            self.active = ActiveSession::Secondary(window.id);
        } else if crate::app::owner_has_focused_viewport(
            context,
            egui::ViewportId::ROOT,
            self.primary_visible,
        ) {
            self.active = ActiveSession::Primary;
        }
    }

    fn command_targets_hidden_root_child(
        &self,
        context: &egui::Context,
        command: AppCommand,
    ) -> bool {
        !self.primary_visible
            && self.active == ActiveSession::Primary
            && matches!(
                command,
                AppCommand::Cut | AppCommand::Copy | AppCommand::Paste | AppCommand::SelectAll
            )
            && crate::app::owner_has_focused_viewport(context, egui::ViewportId::ROOT, false)
    }

    fn command_available(&self, context: &egui::Context, command: AppCommand) -> bool {
        (self.primary_visible
            || !self.secondary.is_empty()
            || command_available_without_document(command)
            || self.command_targets_hidden_root_child(context, command))
            && self.active_editor().native_command_enabled(command)
    }

    fn update_native_menu(&self, context: &egui::Context) {
        #[cfg(target_os = "macos")]
        {
            let shortcuts = self
                .active_editor()
                .settings_snapshot()
                .effective_shortcuts();
            let _ = crate::native_menu::update_macos_menu(&shortcuts, |command| {
                self.command_available(context, command)
            });
        }
        #[cfg(not(target_os = "macos"))]
        let _ = context;
    }

    fn reopen_document_window(&mut self, context: &egui::Context) {
        let target = surviving_active_session(
            self.active,
            self.primary_visible,
            self.secondary.iter().map(|window| window.id),
        );
        self.active = target;
        match target {
            ActiveSession::Primary => self.show_primary(context),
            ActiveSession::Secondary(id) => {
                let viewport = document_viewport_id(id);
                context.send_viewport_cmd_to(viewport, egui::ViewportCommand::Minimized(false));
                context.send_viewport_cmd_to(viewport, egui::ViewportCommand::Focus);
            }
        }
    }
    fn poll_process_close(&mut self, context: &egui::Context) {
        let Some(requested) = self.closing.current() else {
            if self.closing.is_active() {
                self.commit_process_close(context);
            }
            return;
        };
        let editor = if self.primary.borrow().document_key().owner == requested.owner {
            Some(&self.primary)
        } else {
            self.secondary
                .iter()
                .find(|window| window.editor.borrow().document_key().owner == requested.owner)
                .map(|window| &window.editor)
        };
        let Some(editor) = editor else {
            self.cancel_process_close();
            return;
        };
        let Some(accepted) = editor.borrow().process_close_answer() else {
            return;
        };
        let key = editor.borrow().document_key();
        if !accepted || !self.closing.accept(requested, key) {
            self.cancel_process_close();
        } else if self.closing.current().is_some() {
            self.request_next_close(context);
        } else {
            self.commit_process_close(context);
        }
    }
    fn commit_process_close(&mut self, context: &egui::Context) {
        let keys = self.document_keys();
        if !self.closing.ready(&keys)
            || !self.pending_windows.is_empty()
            || !self.primary.borrow().close_accepted()
            || self
                .secondary
                .iter()
                .any(|window| !window.editor.borrow().close_accepted())
        {
            self.cancel_process_close();
            self.primary.borrow_mut().show_window_notice(
                "Closing canceled because a document changed during confirmation".to_owned(),
            );
        } else if self
            .closing
            .can_exit(&keys, crate::worker::has_active_operations())
        {
            self.primary.borrow_mut().finish_process_close(true);
            context.send_viewport_cmd_to(egui::ViewportId::ROOT, egui::ViewportCommand::Close);
        } else {
            self.primary.borrow_mut().show_window_notice(
                "Waiting for the active background operation before closing".to_owned(),
            );
        }
    }

    fn collect_secondary_events(&mut self) {
        for window in &mut self.secondary {
            Self::collect_window_request_from(
                &mut self.pending_windows,
                &mut window.editor.borrow_mut(),
            );
        }
        self.secondary.retain(|window| !window.editor.closed());
        self.active = surviving_active_session(
            self.active,
            self.primary_visible,
            self.secondary.iter().map(|window| window.id),
        );
    }

    fn show_secondary_windows(&mut self, context: &egui::Context) {
        for window in &mut self.secondary {
            let viewport_id = window.viewport_id();
            let activate = std::mem::take(&mut window.activate_once);
            let builder =
                document_viewport_builder(window.editor.borrow().window_title(), activate);
            let token = window.editor.token();
            crate::viewport_fonts::show_deferred(
                context,
                viewport_id,
                builder,
                move |ui, _class| {
                    token.paint(ui);
                },
            );
        }
    }

    fn active_viewport(&self) -> egui::ViewportId {
        match self.active {
            ActiveSession::Primary => egui::ViewportId::ROOT,
            ActiveSession::Secondary(id) => document_viewport_id(id),
        }
    }
}

impl eframe::App for AppShell {
    fn logic(&mut self, context: &egui::Context, frame: &mut eframe::Frame) {
        // eframe skips ui entirely when the root and all children are hidden.
        // This path must neither paint nor consume the previous frame's input.
        self.primary.apply_settings(context);
        self.collect_secondary_events();
        self.collect_settings_requests(context);
        self.refresh_active_window(context);
        self.dispatch_process_requests(context);
        self.guard_process_close(context);
        let completions = crate::worker::take_detached_completions();
        if !completions.is_empty() {
            self.active_editor_mut()
                .show_window_notice(completions.join("\n"));
            context.request_repaint_of(self.active_viewport());
        }
        if !self.primary_visible {
            self.primary
                .borrow_mut()
                .hidden_host_logic(context, Some(frame));
        }
        Self::collect_window_request_from(
            &mut self.pending_windows,
            &mut self.primary.borrow_mut(),
        );
        if !self.primary_visible
            && self.secondary.is_empty()
            && let Some(pending) = self.pending_windows.pop_front()
        {
            // A file dialog or Settings shortcut can finish after the last
            // document closes. Reuse the dormant root; creating an immediate
            // secondary here would require a UI pass that cannot yet run.
            self.primary
                .borrow_mut()
                .reuse_dormant_window(pending.request);
            self.show_primary(context);
        }
        if self.primary.borrow().needs_visible_window() {
            self.show_primary(context);
        }
        self.poll_process_close(context);
        self.update_native_menu(context);
    }

    fn ui(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame) {
        let _span = crate::performance::span("ui.shell.pass");
        crate::performance::tick(ui.ctx(), || !self.captures.has_pending());
        let context = ui.ctx().clone();
        let profile_close_requested = crate::performance::take_no_window_request();
        self.advance_capture_batch(&context);
        let primary_close_requested = context.input(|input| input.viewport().close_requested());
        if self.primary_visible {
            self.primary.borrow_mut().ui_in_window(ui, Some(frame));
        } else {
            self.primary
                .borrow_mut()
                .hidden_host_ui(&context, Some(frame));
            if self.primary.borrow().needs_visible_window() {
                self.show_primary(&context);
            }
        }
        if self.primary_visible && crate::app::owns_focused_input_viewport(&context) {
            self.active = ActiveSession::Primary;
        }
        Self::collect_window_request_from(
            &mut self.pending_windows,
            &mut self.primary.borrow_mut(),
        );
        let primary_was_visible = self.primary_visible;
        if profile_close_requested {
            // Exercise the same end-of-frame transition as a native close.
            // Retiring before the owner's UI pass skips retained-surface
            // registration and can detach the current macOS CGL view.
            self.retire_primary_window(&context);
        } else {
            self.finish_primary_window_close(&context, primary_close_requested);
        }
        if primary_was_visible && !self.primary_visible {
            // Register (but do not show) Settings on the last visible pass,
            // so the no-window logic path can reveal this existing viewport.
            self.primary
                .borrow_mut()
                .register_dormant_settings(&context, Some(frame));
        }
        self.open_pending_windows(&context);
        self.collect_settings_requests(&context);
        self.show_secondary_windows(&context);
        self.poll_process_close(&context);
        self.update_native_menu(&context);
    }

    fn save(&mut self, storage: &mut dyn eframe::Storage) {
        if !self.launch_mode.persists_settings() {
            return;
        }
        self.synchronize_workspace_history_removals();
        let mut settings = self.shared_settings.clone();
        let active_settings = self.active_editor().settings_snapshot();
        self.merge_all_session_history(&mut settings, &active_settings);
        persist_shell_settings(self.launch_mode, storage, &settings);
    }

    fn auto_save_interval(&self) -> std::time::Duration {
        self.primary.borrow().auto_save_interval()
    }

    fn clear_color(&self, visuals: &egui::Visuals) -> [f32; 4] {
        self.primary.borrow().clear_color(visuals)
    }

    fn persist_egui_memory(&self) -> bool {
        shell_persists_egui_memory(
            self.launch_mode,
            self.primary.borrow().persist_egui_memory(),
        )
    }

    fn raw_input_hook(&mut self, context: &egui::Context, raw_input: &mut egui::RawInput) {
        let _ = raw_input;
        self.synchronize_settings(context);
    }
}

fn document_viewport_id(session_id: u64) -> egui::ViewportId {
    egui::ViewportId::from_hash_of(("tiptoptyp-document-window", session_id))
}

fn document_viewport_builder(title: String, activate: bool) -> egui::ViewportBuilder {
    let builder = egui::ViewportBuilder::default()
        .with_title(title)
        .with_inner_size(theme::METRICS.chrome.main_size)
        .with_min_inner_size(theme::METRICS.chrome.main_min_size)
        // New document viewports need an opaque AppKit backing, like
        // Settings. They never use window alpha; their popup children do.
        .with_transparent(false)
        .with_has_shadow(true)
        .with_fullsize_content_view(true)
        .with_title_shown(false)
        .with_titlebar_shown(false);
    if activate {
        // Activation is a one-shot creation hint. Reissuing it every frame
        // would steal focus during system appearance changes and background
        // compiler updates. Omitting the field later is also important: an
        // explicit `false` would deactivate the newly created window.
        builder.with_active(true)
    } else {
        builder
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ProcessRequestAction {
    Editor(AppCommand),
    Reopen,
    CloseProcess,
}

fn process_request_action(request: NativeMenuRequest) -> ProcessRequestAction {
    match request {
        NativeMenuRequest::Command(command) => ProcessRequestAction::Editor(command),
        NativeMenuRequest::Reopen => ProcessRequestAction::Reopen,
        NativeMenuRequest::Quit => ProcessRequestAction::CloseProcess,
    }
}

const fn keeps_running_after_root_close(
    macos: bool,
    launch_mode: LaunchMode,
    quit_requested: bool,
) -> bool {
    macos && launch_mode.persists_settings() && !quit_requested
}

const fn command_reveals_primary(command: AppCommand) -> bool {
    !matches!(
        command,
        AppCommand::Settings | AppCommand::NewWindow | AppCommand::OpenInNewWindow
    )
}

const fn command_available_without_document(command: AppCommand) -> bool {
    matches!(
        command,
        AppCommand::Settings | AppCommand::NewWindow | AppCommand::OpenInNewWindow
    )
}

fn surviving_active_session(
    active: ActiveSession,
    primary_visible: bool,
    secondary_ids: impl IntoIterator<Item = u64>,
) -> ActiveSession {
    let mut last = None;
    for id in secondary_ids {
        if active == ActiveSession::Secondary(id) {
            return active;
        }
        last = Some(id);
    }
    if primary_visible {
        ActiveSession::Primary
    } else {
        last.map_or(ActiveSession::Primary, ActiveSession::Secondary)
    }
}

fn merge_session_histories<'a>(
    target: &mut AppSettings,
    inactive_sessions: impl IntoIterator<Item = &'a AppSettings>,
    active_session: &AppSettings,
) {
    let inactive_sessions = inactive_sessions.into_iter().collect::<Vec<_>>();

    // No cross-window clock exists, so use an explicit, stable precedence:
    // the active window's MRU list, the shared baseline, then inactive windows
    // in session-creation order. This retains unique roots without letting an
    // arbitrary vector tail appear newer than the window the user is in.
    let baseline_recent = target.recent_workspaces.clone();
    let mut merged_recent = Vec::new();
    for recent in std::iter::once(&active_session.recent_workspaces)
        .chain(std::iter::once(&baseline_recent))
        .chain(
            inactive_sessions
                .iter()
                .map(|settings| &settings.recent_workspaces),
        )
    {
        for workspace in recent {
            let workspace = normalize_workspace_root(std::path::Path::new(workspace))
                .to_string_lossy()
                .into_owned();
            if !merged_recent.contains(&workspace) {
                merged_recent.push(workspace);
            }
        }
    }
    target.recent_workspaces.clear();
    for workspace in merged_recent.iter().rev() {
        target.remember_workspace(std::path::Path::new(workspace));
    }

    // Inactive sessions contribute missing workspace entries only. Conflicts
    // are then resolved in one place by overlaying the active session.
    for session in inactive_sessions {
        for (workspace, file) in &session.last_opened_files {
            target
                .last_opened_files
                .entry(workspace.clone())
                .or_insert_with(|| file.clone());
        }
    }
    target
        .last_opened_files
        .extend(active_session.last_opened_files.clone());
}

fn persist_shell_settings(
    launch_mode: LaunchMode,
    storage: &mut dyn eframe::Storage,
    settings: &AppSettings,
) {
    if launch_mode.persists_settings() {
        settings.save(storage);
    }
}

const fn shell_persists_egui_memory(launch_mode: LaunchMode, editor_persists_memory: bool) -> bool {
    launch_mode.persists_settings() && editor_persists_memory
}

#[cfg(test)]
mod tests {
    use super::*;
    use eframe::App as _;

    #[test]
    fn native_command_wakes_and_executes_only_its_deferred_document_owner() {
        let directory = tempfile::tempdir().unwrap();
        let context = egui::Context::default();
        context.set_embed_viewports(false);
        let primary = EditorApp::dormant_for_tests(&context, directory.path().into());
        let shared_settings = primary.settings_snapshot();
        let (_, open_requests) = crate::open_requests::channel();
        let (sender, native_menu_commands) = crate::native_menu::channel();
        let mut shell = AppShell {
            primary: DocumentHost::new(primary),
            shared_settings,
            open_requests,
            native_menu_commands,
            closing: Default::default(),
            secondary: Vec::new(),
            active: ActiveSession::Primary,
            next_session_id: 3,
            pending_windows: VecDeque::new(),
            captures: CaptureController::disabled_for_tests(),
            capture_batch: None,
            launch_mode: LaunchMode::Interactive,
            primary_visible: true,
            quit_requested: false,
        };
        let mut raw = egui::RawInput::default();
        raw.viewports
            .get_mut(&egui::ViewportId::ROOT)
            .unwrap()
            .focused = Some(false);
        for id in [1, 2] {
            let viewport = document_viewport_id(id);
            shell.secondary.push(SecondaryWindow {
                id,
                activate_once: false,
                editor: DocumentHost::new(EditorApp::dormant_window_for_tests(
                    &context,
                    directory.path().into(),
                    viewport,
                )),
            });
            raw.viewports.insert(
                viewport,
                egui::ViewportInfo {
                    parent: Some(egui::ViewportId::ROOT),
                    focused: Some(id == 1),
                    ..Default::default()
                },
            );
        }
        for _ in 0..3 {
            for id in [
                egui::ViewportId::ROOT,
                document_viewport_id(1),
                document_viewport_id(2),
            ] {
                raw.viewport_id = id;
                context
                    .run_ui(raw.clone(), |_| {})
                    .drop_without_applying_deltas();
            }
        }
        raw.viewport_id = egui::ViewportId::ROOT;
        let output = context.run_ui(raw.clone(), |ui| shell.show_secondary_windows(ui.ctx()));
        let callback = output.viewport_output[&document_viewport_id(1)]
            .viewport_ui_cb
            .clone()
            .unwrap();
        output.drop_without_applying_deltas();
        sender
            .send(NativeMenuRequest::Command(AppCommand::NewWindow))
            .unwrap();
        let _ = context.run_logic(&raw, |context| {
            shell.logic(context, &mut eframe::Frame::_new_kittest())
        });
        assert_eq!(shell.active, ActiveSession::Secondary(1));
        assert!(context.has_requested_repaint_for(&document_viewport_id(1)));
        assert!(
            shell.pending_windows.is_empty(),
            "menu must wait for owner input context"
        );
        raw.viewport_id = document_viewport_id(1);
        context
            .run_ui(raw, |ui| callback(ui))
            .drop_without_applying_deltas();
        shell.collect_secondary_events();
        assert_eq!(shell.pending_windows.len(), 1);
        assert!(shell.primary.borrow_mut().take_window_request().is_none());
        assert!(
            shell.secondary[1]
                .editor
                .borrow_mut()
                .take_window_request()
                .is_none()
        );
        shell.collect_secondary_events();
        assert_eq!(
            shell.pending_windows.len(),
            1,
            "native command must be consumed once"
        );
        // Settings is the exception to document-local command ownership.
        // Requests from either secondary must address the same root child.
        let settings =
            crate::child_view::child_viewport_id(egui::ViewportId::ROOT, "tiptoptyp-settings");
        for owner in [1, 2] {
            shell.active = ActiveSession::Secondary(owner);
            sender
                .send(NativeMenuRequest::Command(AppCommand::Settings))
                .unwrap();
            let output = context.run_ui(egui::RawInput::default(), |ui| {
                shell.dispatch_process_requests(ui.ctx());
                shell.primary.borrow_mut().hidden_host_ui(ui.ctx(), None);
            });
            assert_eq!(
                output.viewport_output[&settings].builder.visible,
                Some(true)
            );
            for secondary in [1, 2] {
                let child = crate::child_view::child_viewport_id(
                    document_viewport_id(secondary),
                    "tiptoptyp-settings",
                );
                assert!(!output.viewport_output.contains_key(&child));
            }
            output.drop_without_applying_deltas();
        }
    }

    #[test]
    fn bounded_cross_owner_transition_keeps_state_and_singleton_services_isolated() {
        let directory = tempfile::tempdir().unwrap();
        let context = egui::Context::default();
        context.set_embed_viewports(false);
        let primary = EditorApp::dormant_for_tests(&context, directory.path().into());
        let shared_settings = primary.settings_snapshot();
        let (_, open_requests) = crate::open_requests::channel();
        let (menu_sender, native_menu_commands) = crate::native_menu::channel();
        let mut shell = AppShell {
            primary: DocumentHost::new(primary),
            shared_settings,
            open_requests,
            native_menu_commands,
            closing: Default::default(),
            secondary: Vec::new(),
            active: ActiveSession::Primary,
            next_session_id: 2,
            pending_windows: VecDeque::new(),
            captures: CaptureController::disabled_for_tests(),
            capture_batch: None,
            launch_mode: LaunchMode::Interactive,
            primary_visible: true,
            quit_requested: false,
        };
        let secondary = EditorApp::dormant_window_for_tests(
            &context,
            directory.path().into(),
            document_viewport_id(1),
        );
        shell.secondary.push(SecondaryWindow {
            id: 1,
            editor: DocumentHost::new(secondary),
            activate_once: false,
        });

        let save_path = directory.path().join("pending.typ");
        crate::resource_lock::with_resource(&save_path, || {
            shell
                .primary
                .borrow_mut()
                .prepare_cross_owner_test_state(&context, save_path.clone());
            shell.active = ActiveSession::Secondary(1);
            shell
                .primary
                .borrow_mut()
                .deliver_cross_owner_test_reply(&context);
            assert_eq!(shell.active, ActiveSession::Secondary(1));
        });
        shell.primary.borrow_mut().finish_save_for_test(&context);
        assert!(save_path.is_file());

        // Settings remains a single root child regardless of which document
        // viewport requests it, and an applied preference reaches both live
        // document owners.
        for owner in [ActiveSession::Primary, ActiveSession::Secondary(1)] {
            shell.active = owner;
            menu_sender
                .send(NativeMenuRequest::Command(AppCommand::Settings))
                .unwrap();
            let output = context.run_ui(egui::RawInput::default(), |ui| {
                shell.dispatch_process_requests(ui.ctx());
                shell.primary.borrow_mut().hidden_host_ui(ui.ctx(), None);
            });
            let settings =
                crate::child_view::child_viewport_id(egui::ViewportId::ROOT, "tiptoptyp-settings");
            assert_eq!(
                output.viewport_output[&settings].builder.visible,
                Some(true)
            );
            output.drop_without_applying_deltas();
        }
        let mut updated = shell.shared_settings.clone();
        updated.line_wrap = !updated.line_wrap;
        shell
            .primary
            .borrow_mut()
            .apply_shared_settings(updated.clone(), &context);
        shell.secondary[0]
            .editor
            .borrow_mut()
            .apply_shared_settings(updated.clone(), &context);
        assert_eq!(shell.primary.borrow().settings_snapshot(), updated);
        assert_eq!(
            shell.secondary[0].editor.borrow().settings_snapshot(),
            updated
        );

        // The process close policy is intentionally platform-specific. On
        // macOS, retiring the root document window must leave the process host
        // alive while the secondary survives; the pure policy is checked on
        // every platform.
        assert!(keeps_running_after_root_close(
            true,
            LaunchMode::Interactive,
            false
        ));
        if cfg!(target_os = "macos") {
            shell.retire_primary_window(&context);
            assert!(!shell.primary_visible);
            assert!(!shell.quit_requested);
            assert_eq!(shell.active, ActiveSession::Secondary(1));
        }
    }

    #[test]
    fn no_window_logic_reopens_documents_and_settings_without_an_editor_paint() {
        let directory = tempfile::tempdir().unwrap();
        let context = egui::Context::default();
        let primary = EditorApp::dormant_for_tests(&context, directory.path().to_owned());
        let shared_settings = primary.settings_snapshot();
        let (_open_sender, open_requests) = crate::open_requests::channel();
        let (menu_sender, native_menu_commands) = crate::native_menu::channel();
        let mut shell = AppShell {
            primary: DocumentHost::new(primary),
            shared_settings,
            open_requests,
            native_menu_commands,
            closing: Default::default(),
            secondary: Vec::new(),
            active: ActiveSession::Primary,
            next_session_id: FIRST_SECONDARY_SESSION_ID,
            pending_windows: VecDeque::new(),
            captures: CaptureController::disabled_for_tests(),
            capture_batch: None,
            launch_mode: LaunchMode::Interactive,
            primary_visible: false,
            quit_requested: false,
        };
        let mut frame = eframe::Frame::_new_kittest();
        let raw = egui::RawInput::default();
        for command in [
            AppCommand::New,
            AppCommand::Open,
            AppCommand::Copy,
            AppCommand::Paste,
            AppCommand::Save,
        ] {
            assert!(!shell.command_available(&context, command));
            menu_sender
                .send(NativeMenuRequest::Command(command))
                .unwrap();
        }
        let _ = context.run_logic(&raw, |ctx| shell.logic(ctx, &mut frame));
        assert!(!shell.primary_visible);
        assert!(!shell.primary.borrow().needs_visible_window());
        menu_sender
            .send(NativeMenuRequest::Command(AppCommand::Settings))
            .unwrap();
        let output = context.run_logic(&raw, |ctx| shell.logic(ctx, &mut frame));
        let child =
            crate::child_view::child_viewport_id(egui::ViewportId::ROOT, "tiptoptyp-settings");
        assert!(
            output.viewport_commands[&child]
                .iter()
                .any(|command| matches!(command, egui::ViewportCommand::Visible(true)))
        );
        assert!(!shell.primary_visible);
        for request in [
            NativeMenuRequest::Reopen,
            NativeMenuRequest::Command(AppCommand::NewWindow),
        ] {
            shell.primary.borrow_mut().finish_window_close();
            shell.primary_visible = false;
            menu_sender.send(request).unwrap();
            let output = context.run_logic(&raw, |ctx| shell.logic(ctx, &mut frame));
            assert!(shell.primary_visible);
            assert!(shell.primary.borrow().needs_visible_window());
            assert!(shell.secondary.is_empty());
            assert!(
                output.viewport_commands[&egui::ViewportId::ROOT]
                    .iter()
                    .any(|command| matches!(command, egui::ViewportCommand::Visible(true)))
            );
        }
        shell.primary.borrow_mut().finish_window_close();
        shell.primary_visible = false;
        menu_sender
            .send(NativeMenuRequest::Command(AppCommand::OpenInNewWindow))
            .unwrap();
        // Test routing without launching a native picker in the headless suite.
        let _ = context.run_logic(&raw, |ctx| shell.dispatch_process_requests(ctx));
        assert!(shell.primary_visible);
        assert!(shell.primary.borrow().needs_visible_window());
    }

    #[test]
    fn closing_active_window_never_selects_a_hidden_root_over_a_survivor() {
        assert_eq!(
            surviving_active_session(ActiveSession::Secondary(2), false, [1, 3]),
            ActiveSession::Secondary(3)
        );
        assert_eq!(
            surviving_active_session(ActiveSession::Secondary(2), true, [1, 3]),
            ActiveSession::Primary
        );
        assert_eq!(
            surviving_active_session(ActiveSession::Secondary(1), false, [1, 3]),
            ActiveSession::Secondary(1)
        );
        assert_eq!(
            surviving_active_session(ActiveSession::Primary, false, [1]),
            ActiveSession::Secondary(1)
        );
        assert_eq!(
            surviving_active_session(ActiveSession::Secondary(2), false, []),
            ActiveSession::Primary
        );
    }

    #[test]
    fn hidden_child_with_stale_focus_does_not_enable_clipboard_commands() {
        let context = egui::Context::default();
        let child =
            crate::child_view::child_viewport_id(egui::ViewportId::ROOT, "tiptoptyp-settings");
        for hidden in [false, true] {
            let mut raw = egui::RawInput::default();
            raw.viewports.insert(
                child,
                egui::ViewportInfo {
                    focused: Some(true),
                    minimized: Some(false),
                    occluded: Some(hidden),
                    ..Default::default()
                },
            );
            let _ = context.run_logic(&raw, |ctx| {
                assert_eq!(
                    crate::app::owner_has_focused_viewport(ctx, egui::ViewportId::ROOT, false),
                    !hidden
                );
            });
        }
    }

    #[test]
    fn no_window_state_keeps_creation_and_settings_but_not_document_commands() {
        for command in [
            AppCommand::NewWindow,
            AppCommand::OpenInNewWindow,
            AppCommand::Settings,
        ] {
            assert!(command_available_without_document(command));
        }
        for command in [
            AppCommand::New,
            AppCommand::Open,
            AppCommand::Save,
            AppCommand::SaveAs,
            AppCommand::Copy,
            AppCommand::Paste,
            AppCommand::Find,
        ] {
            assert!(!command_available_without_document(command));
        }
    }

    #[test]
    fn current_secondary_and_child_focus_are_visible_before_owner_paint() {
        let context = egui::Context::default();
        let owner = document_viewport_id(2);
        let sibling = document_viewport_id(3);
        for focused in [
            owner,
            crate::child_view::child_viewport_id(owner, "tiptoptyp-settings"),
        ] {
            let mut raw = egui::RawInput::default();
            raw.viewports
                .get_mut(&egui::ViewportId::ROOT)
                .unwrap()
                .focused = Some(false);
            raw.viewports.insert(
                focused,
                egui::ViewportInfo {
                    focused: Some(true),
                    ..Default::default()
                },
            );
            let mut output = context.run_ui(raw, |_| {
                assert!(crate::app::owner_has_focused_viewport(
                    &context, owner, true
                ));
                assert!(!crate::app::owner_has_focused_viewport(
                    &context, sibling, true
                ));
                assert!(!crate::app::owns_focused_input_viewport(&context));
                assert_eq!(
                    crate::app::owner_has_focused_viewport(&context, owner, false),
                    focused != owner
                );
            });
            output.textures_delta.clear();
        }
    }

    #[test]
    fn simultaneous_settings_edits_merge_with_active_conflict_precedence() {
        let base = AppSettings::default();
        let mut inactive = base.clone();
        inactive.auto_save = !base.auto_save;
        inactive.auto_save_delay_ms = 1234;
        let mut active = base.clone();
        active.line_wrap = !base.line_wrap;
        active.auto_save_delay_ms = 2345;
        let mut shared = base.clone();
        shared.apply_edits(&base, inactive);
        shared.apply_edits(&base, active);
        assert_eq!(shared.auto_save, !base.auto_save);
        assert_eq!(shared.line_wrap, !base.line_wrap);
        assert_eq!(shared.auto_save_delay_ms, 2345);
    }
    use std::collections::HashMap;

    #[derive(Default)]
    struct MemoryStorage(HashMap<String, String>);

    impl eframe::Storage for MemoryStorage {
        fn get_string(&self, key: &str) -> Option<String> {
            self.0.get(key).cloned()
        }

        fn set_string(&mut self, key: &str, value: String) {
            self.0.insert(key.to_owned(), value);
        }

        fn remove_string(&mut self, key: &str) {
            self.0.remove(key);
        }

        fn flush(&mut self) {}
    }

    #[test]
    fn deterministic_capture_shell_never_persists_fixture_settings() {
        let fixture_settings = AppSettings {
            recent_workspaces: vec!["/qa/fixture".to_owned()],
            ..AppSettings::default()
        };
        for mode in [
            LaunchMode::DeterministicCapture,
            LaunchMode::Profiling {
                deterministic_capture: true,
            },
        ] {
            let mut capture_storage = MemoryStorage::default();
            persist_shell_settings(mode, &mut capture_storage, &fixture_settings);
            assert!(capture_storage.0.is_empty());
            assert!(!shell_persists_egui_memory(mode, true));
        }

        let mut interactive_storage = MemoryStorage::default();
        persist_shell_settings(
            LaunchMode::Interactive,
            &mut interactive_storage,
            &fixture_settings,
        );
        assert_eq!(interactive_storage.0.len(), 1);

        assert!(!shell_persists_egui_memory(
            LaunchMode::DeterministicCapture,
            true
        ));
        assert!(shell_persists_egui_memory(LaunchMode::Interactive, true));
        assert!(!shell_persists_egui_memory(LaunchMode::Interactive, false));
    }

    #[test]
    fn document_viewport_ids_are_stable_and_isolated() {
        assert_eq!(document_viewport_id(7), document_viewport_id(7));
        assert_ne!(document_viewport_id(7), document_viewport_id(8));
        assert_ne!(document_viewport_id(7), egui::ViewportId::ROOT);
    }

    #[test]
    fn native_quit_targets_the_process_while_document_commands_stay_active_local() {
        assert_eq!(
            process_request_action(NativeMenuRequest::Quit),
            ProcessRequestAction::CloseProcess
        );
        assert_eq!(
            process_request_action(NativeMenuRequest::Command(AppCommand::Save)),
            ProcessRequestAction::Editor(AppCommand::Save)
        );
        assert_eq!(
            process_request_action(NativeMenuRequest::Reopen),
            ProcessRequestAction::Reopen
        );
    }

    #[test]
    fn macos_window_close_keeps_the_interactive_process_alive_until_quit() {
        assert!(keeps_running_after_root_close(
            true,
            LaunchMode::Interactive,
            false
        ));
        assert!(!keeps_running_after_root_close(
            true,
            LaunchMode::Interactive,
            true
        ));
        assert!(!keeps_running_after_root_close(
            true,
            LaunchMode::DeterministicCapture,
            false
        ));
        assert!(!keeps_running_after_root_close(
            false,
            LaunchMode::Interactive,
            false
        ));
    }

    #[test]
    fn hidden_primary_is_revealed_only_for_commands_that_need_a_document_window() {
        assert!(command_reveals_primary(AppCommand::New));
        assert!(command_reveals_primary(AppCommand::Open));
        assert!(!command_reveals_primary(AppCommand::Settings));
        assert!(!command_reveals_primary(AppCommand::NewWindow));
        assert!(!command_reveals_primary(AppCommand::OpenInNewWindow));
    }

    #[test]
    fn returning_from_a_child_viewport_warms_the_root_framebuffer() {
        assert!(root_capture_needs_warmup(
            UiSnapshotScene::SettingsWindow,
            UiSnapshotScene::ProblemsPanel,
        ));
        assert!(!root_capture_needs_warmup(
            UiSnapshotScene::ProblemsPanel,
            UiSnapshotScene::FindReplace,
        ));
        assert!(!root_capture_needs_warmup(
            UiSnapshotScene::SettingsWindow,
            UiSnapshotScene::FileMenu,
        ));
    }

    #[test]
    fn existing_document_windows_are_never_reactivated_by_their_builder() {
        let first = document_viewport_builder("first".to_owned(), true);
        let existing = document_viewport_builder("existing".to_owned(), false);
        assert_eq!(first.active, Some(true));
        assert_eq!(existing.active, None);
        assert_eq!(first.transparent, Some(false));
        assert_eq!(existing.transparent, Some(false));
        assert_eq!(first.has_shadow, Some(true));
        assert_eq!(existing.has_shadow, Some(true));
    }

    #[test]
    fn native_commands_remain_typed_at_the_shell_boundary() {
        let command = crate::native_menu::AppCommand::NewWindow;
        assert_eq!(command, crate::native_menu::AppCommand::NewWindow);
    }

    #[test]
    fn active_session_wins_same_workspace_history_conflicts() {
        let mut shared = AppSettings::default();
        shared
            .last_opened_files
            .insert("/shared".to_owned(), "/shared/from-baseline.typ".to_owned());

        let mut inactive = AppSettings::default();
        inactive
            .last_opened_files
            .insert("/shared".to_owned(), "/shared/from-inactive.typ".to_owned());

        let mut active = AppSettings::default();
        active
            .last_opened_files
            .insert("/shared".to_owned(), "/shared/from-active.typ".to_owned());

        merge_session_histories(&mut shared, [&inactive], &active);

        assert_eq!(
            shared.last_opened_files["/shared"],
            "/shared/from-active.typ"
        );
    }

    #[test]
    fn inactive_unique_history_is_merged_after_active_mru_entries() {
        let mut shared = AppSettings {
            recent_workspaces: vec!["/baseline".to_owned(), "/shared".to_owned()],
            last_opened_files: std::collections::BTreeMap::from([(
                "/baseline".to_owned(),
                "/baseline/open.typ".to_owned(),
            )]),
            ..AppSettings::default()
        };
        let inactive = AppSettings {
            recent_workspaces: vec!["/inactive".to_owned(), "/shared".to_owned()],
            last_opened_files: std::collections::BTreeMap::from([(
                "/inactive".to_owned(),
                "/inactive/open.typ".to_owned(),
            )]),
            ..AppSettings::default()
        };
        let active = AppSettings {
            recent_workspaces: vec!["/active".to_owned(), "/shared".to_owned()],
            last_opened_files: std::collections::BTreeMap::from([(
                "/shared".to_owned(),
                "/shared/active.typ".to_owned(),
            )]),
            ..AppSettings::default()
        };

        merge_session_histories(&mut shared, [&inactive], &active);

        assert_eq!(
            shared.recent_workspaces,
            ["/active", "/shared", "/baseline", "/inactive"]
        );
        assert_eq!(shared.last_opened_files["/inactive"], "/inactive/open.typ");
        assert_eq!(shared.last_opened_files["/baseline"], "/baseline/open.typ");
    }
}
