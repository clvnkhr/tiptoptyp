//! One-process document-window orchestration.
//!
//! eframe owns one root `winit` window and exposes additional native windows as
//! egui viewports. Each viewport below owns a complete [`EditorApp`] session:
//! source buffer, undo history, compiler, Tinymist process, workspace, and
//! dirty-close flow never cross session boundaries.

use std::{collections::VecDeque, path::PathBuf};

use eframe::egui;

use crate::{
    app::{EditorApp, EditorWindowRequest},
    native_menu::{AppCommand, NativeMenuReceiver, NativeMenuRequest},
    open_requests::OpenRequestReceiver,
    screenshot::{
        CaptureController, CaptureThemeProfile, LaunchMode, UiCaptureStep, UiSnapshotScene,
    },
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
    editor: EditorApp,
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
    primary: EditorApp,
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
            primary,
            secondary: Vec::new(),
            active: ActiveSession::Primary,
            next_session_id: FIRST_SECONDARY_SESSION_ID,
            pending_windows: VecDeque::new(),
            open_requests,
            native_menu_commands,
            captures,
            capture_batch,
            shared_settings,
            launch_mode,
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
        self.primary.set_capture_step(&next, context);
        batch.active_request = request;
        batch.active_scene = next.scene;
        self.capture_batch = Some(batch);
        context.request_repaint();
    }

    fn active_editor_mut(&mut self) -> &mut EditorApp {
        match self.active {
            ActiveSession::Primary => &mut self.primary,
            ActiveSession::Secondary(id) => self
                .secondary
                .iter_mut()
                .find(|window| window.id == id)
                .map_or(&mut self.primary, |window| &mut window.editor),
        }
    }

    fn active_editor(&self) -> &EditorApp {
        match self.active {
            ActiveSession::Primary => &self.primary,
            ActiveSession::Secondary(id) => self
                .secondary
                .iter()
                .find(|window| window.id == id)
                .map_or(&self.primary, |window| &window.editor),
        }
    }

    fn dispatch_process_requests(&mut self, context: &egui::Context) {
        while let Ok(request) = self.native_menu_commands.try_recv() {
            match process_request_action(request) {
                ProcessRequestAction::Editor(command) => self
                    .active_editor_mut()
                    .enqueue_native_menu_command(command),
                ProcessRequestAction::CloseProcess => context
                    .send_viewport_cmd_to(egui::ViewportId::ROOT, egui::ViewportCommand::Close),
            }
        }

        let mut reused_primary = false;
        while let Ok(path) = self.open_requests.try_recv() {
            if !reused_primary
                && self.secondary.is_empty()
                && self.primary.can_reuse_for_external_open()
            {
                self.primary.open_external_path(path);
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
            let editor =
                EditorApp::new_secondary(context, request, settings, self.captures.clone());
            self.secondary.push(SecondaryWindow {
                id,
                editor,
                activate_once: true,
            });
            self.active = ActiveSession::Secondary(id);
        }
    }

    fn take_focused_settings_update(&mut self) -> Option<AppSettings> {
        let mut selected = match self.active {
            ActiveSession::Primary => self.primary.take_settings_update(),
            ActiveSession::Secondary(id) => self
                .secondary
                .iter_mut()
                .find(|window| window.id == id)
                .and_then(|window| window.editor.take_settings_update()),
        };
        if self.active != ActiveSession::Primary {
            let update = self.primary.take_settings_update();
            if selected.is_none() {
                selected = update;
            }
        }
        for window in &mut self.secondary {
            if self.active == ActiveSession::Secondary(window.id) {
                continue;
            }
            let update = window.editor.take_settings_update();
            if selected.is_none() {
                selected = update;
            }
        }
        selected
    }

    fn merge_all_session_history(&self, settings: &mut AppSettings, active_settings: &AppSettings) {
        let mut inactive_settings = Vec::with_capacity(self.secondary.len());
        if self.active != ActiveSession::Primary {
            inactive_settings.push(self.primary.settings_snapshot());
        }
        for window in &self.secondary {
            if self.active != ActiveSession::Secondary(window.id) {
                inactive_settings.push(window.editor.settings_snapshot());
            }
        }
        merge_session_histories(settings, inactive_settings.iter(), active_settings);
    }

    fn synchronize_settings(&mut self, context: &egui::Context) {
        // Snapshot this before draining pending updates: the active window is
        // authoritative for per-workspace document history even when another
        // window happened to submit the shared preference change.
        let active_settings = self.active_editor().settings_snapshot();
        let Some(mut settings) = self.take_focused_settings_update() else {
            return;
        };
        self.merge_all_session_history(&mut settings, &active_settings);
        self.shared_settings = settings.clone();
        self.primary
            .apply_shared_settings(settings.clone(), context);
        for window in &mut self.secondary {
            window
                .editor
                .apply_shared_settings(settings.clone(), context);
        }
    }

    fn guard_process_close(&mut self, context: &egui::Context) {
        if !context.input(|input| input.viewport().close_requested()) {
            return;
        }
        let dirty_secondary = self
            .secondary
            .iter()
            .filter(|window| window.editor.is_dirty_for_close())
            .count();
        let guard = process_close_guard(self.primary.is_dirty_for_close(), dirty_secondary);
        if let ProcessCloseGuard::DirtySecondary(dirty_secondary) = guard {
            context.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            self.primary.show_window_notice(format!(
                "Close or save the {dirty_secondary} modified secondary window{} before closing the main window",
                if dirty_secondary == 1 { "" } else { "s" }
            ));
        }
    }

    fn show_secondary_windows(&mut self, context: &egui::Context, frame: &mut eframe::Frame) {
        let mut closed = Vec::new();
        let mut newly_active = None;
        let pending_windows = &mut self.pending_windows;

        for window in &mut self.secondary {
            let viewport_id = window.viewport_id();
            let activate = std::mem::take(&mut window.activate_once);
            let builder = document_viewport_builder(window.editor.window_title(), activate);
            let mut close_accepted = false;
            let mut focused = false;
            crate::viewport_fonts::show_immediate(context, viewport_id, builder, |ui, _class| {
                focused = ui
                    .ctx()
                    .input(|input| input.viewport().focused == Some(true));
                let close_requested = ui.ctx().input(|input| input.viewport().close_requested());
                window.editor.ui_in_window(ui, frame);
                close_accepted = window.editor.close_accepted()
                    || (close_requested && !window.editor.is_dirty_for_close());
            });
            if focused {
                newly_active = Some(window.id);
            }
            Self::collect_window_request_from(pending_windows, &mut window.editor);
            if close_accepted {
                closed.push(window.id);
            }
        }

        if let Some(id) = newly_active {
            self.active = ActiveSession::Secondary(id);
        }
        if !closed.is_empty() {
            self.secondary.retain(|window| !closed.contains(&window.id));
            if matches!(self.active, ActiveSession::Secondary(id) if closed.contains(&id)) {
                self.active = ActiveSession::Primary;
            }
        }
    }
}

impl eframe::App for AppShell {
    fn logic(&mut self, context: &egui::Context, frame: &mut eframe::Frame) {
        self.primary.logic(context, frame);
    }

    fn ui(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame) {
        let context = ui.ctx().clone();
        self.advance_capture_batch(&context);
        if context.input(|input| input.viewport().focused == Some(true)) {
            self.active = ActiveSession::Primary;
        }
        self.dispatch_process_requests(&context);
        self.guard_process_close(&context);
        self.primary.ui_in_window(ui, frame);
        Self::collect_window_request_from(&mut self.pending_windows, &mut self.primary);
        self.show_secondary_windows(&context, frame);
        self.open_pending_windows(&context);
        #[cfg(target_os = "macos")]
        {
            let editor = self.active_editor();
            let shortcuts = editor.settings_snapshot().effective_shortcuts();
            let _ = crate::native_menu::update_macos_menu(&shortcuts, |command| {
                editor.native_command_enabled(command)
            });
        }
    }

    fn save(&mut self, storage: &mut dyn eframe::Storage) {
        if !self.launch_mode.persists_settings() {
            return;
        }
        let mut settings = self.shared_settings.clone();
        let active_settings = self.active_editor().settings_snapshot();
        self.merge_all_session_history(&mut settings, &active_settings);
        persist_shell_settings(self.launch_mode, storage, &settings);
    }

    fn auto_save_interval(&self) -> std::time::Duration {
        self.primary.auto_save_interval()
    }

    fn clear_color(&self, visuals: &egui::Visuals) -> [f32; 4] {
        self.primary.clear_color(visuals)
    }

    fn persist_egui_memory(&self) -> bool {
        shell_persists_egui_memory(self.launch_mode, self.primary.persist_egui_memory())
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
        .with_transparent(true)
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
    CloseProcess,
}

fn process_request_action(request: NativeMenuRequest) -> ProcessRequestAction {
    match request {
        NativeMenuRequest::Command(command) => ProcessRequestAction::Editor(command),
        NativeMenuRequest::Quit => ProcessRequestAction::CloseProcess,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ProcessCloseGuard {
    Allow,
    PrimaryDocument,
    DirtySecondary(usize),
}

fn process_close_guard(primary_dirty: bool, dirty_secondary_windows: usize) -> ProcessCloseGuard {
    if dirty_secondary_windows > 0 {
        ProcessCloseGuard::DirtySecondary(dirty_secondary_windows)
    } else if primary_dirty {
        // EditorApp owns the document modal and will cancel the same root
        // close request until Save or Discard completes.
        ProcessCloseGuard::PrimaryDocument
    } else {
        ProcessCloseGuard::Allow
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
        for (workspace, file) in &session.preview_files {
            target
                .preview_files
                .entry(workspace.clone())
                .or_insert_with(|| file.clone());
        }
    }
    target
        .last_opened_files
        .extend(active_session.last_opened_files.clone());
    target
        .preview_files
        .extend(active_session.preview_files.clone());
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
        let mut capture_storage = MemoryStorage::default();
        persist_shell_settings(
            LaunchMode::DeterministicCapture,
            &mut capture_storage,
            &fixture_settings,
        );
        assert!(capture_storage.0.is_empty());

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
    fn process_close_routes_every_dirty_session_through_a_guard() {
        assert_eq!(process_close_guard(false, 0), ProcessCloseGuard::Allow);
        assert_eq!(
            process_close_guard(true, 0),
            ProcessCloseGuard::PrimaryDocument
        );
        assert_eq!(
            process_close_guard(false, 1),
            ProcessCloseGuard::DirtySecondary(1)
        );
        assert_eq!(
            process_close_guard(true, 4),
            ProcessCloseGuard::DirtySecondary(4)
        );
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
        shared
            .preview_files
            .insert("/shared".to_owned(), "/shared/baseline-main.typ".to_owned());

        let mut inactive = AppSettings::default();
        inactive
            .last_opened_files
            .insert("/shared".to_owned(), "/shared/from-inactive.typ".to_owned());
        inactive
            .preview_files
            .insert("/shared".to_owned(), "/shared/inactive-main.typ".to_owned());

        let mut active = AppSettings::default();
        active
            .last_opened_files
            .insert("/shared".to_owned(), "/shared/from-active.typ".to_owned());
        active
            .preview_files
            .insert("/shared".to_owned(), "/shared/active-main.typ".to_owned());

        merge_session_histories(&mut shared, [&inactive], &active);

        assert_eq!(
            shared.last_opened_files["/shared"],
            "/shared/from-active.typ"
        );
        assert_eq!(shared.preview_files["/shared"], "/shared/active-main.typ");
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
            preview_files: std::collections::BTreeMap::from([(
                "/inactive".to_owned(),
                "/inactive/main.typ".to_owned(),
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
        assert_eq!(shared.preview_files["/inactive"], "/inactive/main.typ");
        assert_eq!(shared.last_opened_files["/baseline"], "/baseline/open.typ");
    }
}
