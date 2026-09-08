//! App-window-only screenshots for manual UI QA.
//!
//! This uses egui's `ViewportCommand::Screenshot`, which reads the framebuffer
//! for one egui viewport. It deliberately does not use any operating-system
//! screen-capture API, so it can never capture the desktop or another app.

use std::collections::{HashMap, VecDeque};
use std::ffi::OsString;
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{SystemTime, UNIX_EPOCH};

use eframe::egui;

const DEFAULT_OUTPUT_SUBDIRECTORY: &str = "screenshots";
const LATEST_OUTPUT_SUBDIRECTORY: &str = "docs/ui-snapshots/latest";
const DEFAULT_SETTLE_FRAMES: u8 = 2;
const ROOT_VIEWPORT_NAME: &str = "main";

static NEXT_SERVICE_ID: AtomicU64 = AtomicU64::new(1);

/// Theme and ordered color transforms applied to a launch-time QA capture.
///
/// The application consumes this profile before constructing its first frame.
/// Screenshot filenames use the same profile so the checked-in gallery stays
/// deterministic and variants cannot overwrite one another.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CaptureThemeProfile {
    pub name: String,
    pub invert: bool,
    pub hue_shift_degrees: i16,
}

/// One serial step in a single-process visual-QA capture session.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UiCaptureStep {
    pub theme: CaptureThemeProfile,
    pub scene: UiSnapshotScene,
}

impl UiCaptureStep {
    /// Parse `theme,scene,invert,hue-shift`.
    fn parse(value: &str) -> Result<Self, String> {
        let fields = value.split(',').map(str::trim).collect::<Vec<_>>();
        let [theme, scene, invert, hue_shift] = fields.as_slice() else {
            return Err(format!(
                "invalid UI screenshot step {value:?}; use theme,scene,invert,hue-shift"
            ));
        };
        Ok(Self {
            theme: CaptureThemeProfile {
                name: parse_theme_name(theme)?,
                invert: parse_bool(invert)?,
                hue_shift_degrees: parse_hue_shift(hue_shift)?,
            },
            scene: UiSnapshotScene::parse(scene)?,
        })
    }
}

/// Deterministic app state used by launch-time visual QA.
///
/// Each variant names one themed component family. The target identifies the
/// framebuffer that actually contains it: menus and cards rendered above the
/// native preview live in transparent child viewports, while panels and the
/// find bar remain in the root app viewport.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum UiSnapshotScene {
    Main,
    FileMenu,
    EditMenu,
    SettingsWindow,
    SettingsThemePicker,
    SettingsDarkThemePicker,
    SettingsTooltip,
    TypstOverridesWindow,
    DiagnosticTooltip,
    FunctionTooltip,
    SaveDialog,
    AlertDialog,
    OverwriteDialog,
    EditorContextMenu,
    ExplorerContextMenu,
    DocumentFontSelector,
    StatusLog,
    RenameDialog,
    WorkspaceChooser,
    ProblemsPanel,
    FindReplace,
    PreviewCompiling,
}

impl UiSnapshotScene {
    pub const ALL: [Self; 22] = [
        Self::Main,
        Self::FileMenu,
        Self::EditMenu,
        Self::SettingsWindow,
        Self::SettingsThemePicker,
        Self::SettingsDarkThemePicker,
        Self::SettingsTooltip,
        Self::TypstOverridesWindow,
        Self::DiagnosticTooltip,
        Self::FunctionTooltip,
        Self::SaveDialog,
        Self::AlertDialog,
        Self::OverwriteDialog,
        Self::EditorContextMenu,
        Self::ExplorerContextMenu,
        Self::DocumentFontSelector,
        Self::StatusLog,
        Self::RenameDialog,
        Self::WorkspaceChooser,
        Self::ProblemsPanel,
        Self::FindReplace,
        Self::PreviewCompiling,
    ];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Main => "main",
            Self::FileMenu => "file-menu",
            Self::EditMenu => "edit-menu",
            Self::SettingsWindow => "settings-window",
            Self::SettingsThemePicker => "settings-theme-picker",
            Self::SettingsDarkThemePicker => "settings-dark-theme-picker",
            Self::SettingsTooltip => "settings-tooltip",
            Self::TypstOverridesWindow => "typst-overrides-window",
            Self::DiagnosticTooltip => "diagnostic-tooltip",
            Self::FunctionTooltip => "function-tooltip",
            Self::SaveDialog => "save-dialog",
            Self::AlertDialog => "alert-dialog",
            Self::OverwriteDialog => "overwrite-dialog",
            Self::EditorContextMenu => "editor-context-menu",
            Self::ExplorerContextMenu => "explorer-context-menu",
            Self::DocumentFontSelector => "document-font-selector",
            Self::StatusLog => "status-log",
            Self::RenameDialog => "rename-dialog",
            Self::WorkspaceChooser => "workspace-chooser",
            Self::ProblemsPanel => "problems-panel",
            Self::FindReplace => "find-replace",
            Self::PreviewCompiling => "preview-compiling",
        }
    }

    /// Logical framebuffer target used by [`CaptureController`].
    pub const fn viewport_target(self) -> &'static str {
        match self {
            Self::Main | Self::ProblemsPanel | Self::FindReplace | Self::PreviewCompiling => {
                ROOT_VIEWPORT_NAME
            }
            Self::FileMenu
            | Self::EditMenu
            | Self::EditorContextMenu
            | Self::ExplorerContextMenu
            | Self::DocumentFontSelector
            | Self::StatusLog => "popup",
            Self::SettingsWindow
            | Self::SettingsThemePicker
            | Self::SettingsDarkThemePicker
            | Self::SettingsTooltip => "settings",
            Self::TypstOverridesWindow => "typst-overrides",
            Self::DiagnosticTooltip | Self::FunctionTooltip => "diagnostic",
            Self::SaveDialog | Self::AlertDialog | Self::OverwriteDialog => "modal",
            Self::RenameDialog => "rename",
            Self::WorkspaceChooser => "workspace",
        }
    }

    /// The canonical capture request for this scene.
    pub fn capture_spec(self) -> CaptureSpec {
        CaptureSpec {
            target: self.viewport_target().to_owned(),
            name: self.as_str().to_owned(),
        }
    }

    fn parse(value: &str) -> Result<Self, String> {
        let value = value.trim();
        let scene = match value {
            "main" => Self::Main,
            "file-menu" => Self::FileMenu,
            "edit-menu" => Self::EditMenu,
            "settings-window" => Self::SettingsWindow,
            "settings-theme-picker" => Self::SettingsThemePicker,
            "settings-dark-theme-picker" => Self::SettingsDarkThemePicker,
            "settings-tooltip" => Self::SettingsTooltip,
            "typst-overrides-window" => Self::TypstOverridesWindow,
            "diagnostic-tooltip" => Self::DiagnosticTooltip,
            "function-tooltip" => Self::FunctionTooltip,
            "save-dialog" => Self::SaveDialog,
            "alert-dialog" => Self::AlertDialog,
            "overwrite-dialog" => Self::OverwriteDialog,
            "editor-context-menu" => Self::EditorContextMenu,
            "explorer-context-menu" => Self::ExplorerContextMenu,
            "document-font-selector" => Self::DocumentFontSelector,
            "status-log" => Self::StatusLog,
            "rename-dialog" => Self::RenameDialog,
            "workspace-chooser" => Self::WorkspaceChooser,
            "problems-panel" => Self::ProblemsPanel,
            "find-replace" => Self::FindReplace,
            "preview-compiling" => Self::PreviewCompiling,
            _ => {
                let choices = Self::ALL
                    .iter()
                    .map(|scene| scene.as_str())
                    .collect::<Vec<_>>()
                    .join(", ");
                return Err(format!(
                    "unknown UI snapshot scene {value:?}; expected one of: {choices}"
                ));
            }
        };
        Ok(scene)
    }
}

impl Default for CaptureThemeProfile {
    fn default() -> Self {
        Self {
            name: "system".to_owned(),
            invert: false,
            hue_shift_degrees: 0,
        }
    }
}

impl CaptureThemeProfile {
    fn filename_slug(&self) -> String {
        let mut slug = safe_slug(&self.name);
        if self.invert {
            slug.push_str("-inverted");
        }
        match self.hue_shift_degrees.cmp(&0) {
            std::cmp::Ordering::Less => {
                slug.push_str(&format!("-hue-m{}", self.hue_shift_degrees.unsigned_abs()));
            }
            std::cmp::Ordering::Greater => {
                slug.push_str(&format!("-hue-p{}", self.hue_shift_degrees));
            }
            std::cmp::Ordering::Equal => {}
        }
        slug
    }
}

/// A named screenshot to take once its target viewport has rendered.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CaptureSpec {
    /// Logical viewport name. The root app window is `main` and the Settings
    /// window is `settings`.
    pub target: String,
    /// Human-readable part of the PNG filename.
    pub name: String,
}

impl CaptureSpec {
    /// Parse `target:name`, or use one value for both target and name.
    pub fn parse(value: &str) -> Result<Self, String> {
        let value = value.trim();
        if value.is_empty() {
            return Err("a screenshot name cannot be empty".to_owned());
        }
        let (target, name) = value
            .split_once(':')
            .map_or((value, value), |(target, name)| (target, name));
        if target.trim().is_empty() || name.trim().is_empty() {
            return Err(format!(
                "invalid screenshot spec {value:?}; use target or target:name"
            ));
        }
        Ok(Self {
            target: safe_slug(target),
            name: safe_slug(name),
        })
    }
}

/// Runtime configuration for app-only screenshots.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CaptureConfig {
    /// Whether manual and queued captures are accepted.
    pub enabled: bool,
    /// Rooted below `<working-directory>/.tiptoptyp` for local captures, or at
    /// the fixed checked-in gallery path in latest mode.
    pub output_directory: PathBuf,
    /// Manual capture shortcut. `None` disables the shortcut while retaining
    /// programmatic and launch-time captures.
    pub shortcut: Option<egui::KeyboardShortcut>,
    /// UI frames allowed to settle after a request and before framebuffer read.
    pub settle_frames: u8,
    /// Captures queued before the first app frame.
    pub startup_captures: Vec<CaptureSpec>,
    /// Use stable gallery filenames rather than timestamped local filenames.
    pub latest_filenames: bool,
    /// Close the app after every requested viewport has been written.
    pub close_after_captures: bool,
    /// Copy used only to keep stable filenames aligned with the launch theme.
    filename_theme_profile: CaptureThemeProfile,
    /// Copy used only to keep filenames aligned with the requested QA state.
    filename_scene: Option<UiSnapshotScene>,
}

impl CaptureConfig {
    fn for_working_directory(working_directory: &Path) -> Self {
        Self {
            enabled: true,
            output_directory: private_output_directory(
                working_directory,
                Path::new(DEFAULT_OUTPUT_SUBDIRECTORY),
            )
            .expect("the built-in screenshot directory is valid"),
            shortcut: Some(default_shortcut()),
            settle_frames: DEFAULT_SETTLE_FRAMES,
            startup_captures: Vec::new(),
            latest_filenames: false,
            close_after_captures: false,
            filename_theme_profile: CaptureThemeProfile::default(),
            filename_scene: None,
        }
    }
}

/// Screenshot-related launch options plus an optional workspace or document path.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LaunchOptions {
    pub mode: LaunchMode,
    pub initial_path: Option<PathBuf>,
    pub captures: CaptureConfig,
    /// Explicit QA-only theme override. Normal launches leave this unset so
    /// persisted appearance settings remain authoritative.
    pub theme_profile: Option<CaptureThemeProfile>,
    /// QA-only state for exposing one themed component before capture.
    pub ui_snapshot_scene: Option<UiSnapshotScene>,
    /// Ordered captures performed by one live application instance.
    pub ui_capture_steps: Vec<UiCaptureStep>,
}

/// Outer-shell policy selected before any editor state is constructed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LaunchMode {
    Interactive,
    DeterministicCapture,
}

impl LaunchMode {
    pub const fn persists_settings(self) -> bool {
        matches!(self, Self::Interactive)
    }
}

impl LaunchOptions {
    /// Parse process arguments and `TIPTOPTYP_UI_SCREENSHOT_*` environment
    /// variables. Capture flags are removed before selecting the initial path.
    pub fn from_process() -> Result<Self, String> {
        let working_directory = std::env::current_dir()
            .map_err(|error| format!("could not determine the working directory: {error}"))?;
        parse_launch_options(std::env::args_os().skip(1), &working_directory, |name| {
            std::env::var_os(name)
        })
    }
}

/// One completed (or failed) screenshot write.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CaptureResult {
    pub request_id: u64,
    pub path: PathBuf,
    pub error: Option<String>,
}

#[derive(Clone, Debug)]
struct CaptureRequest {
    id: u64,
    target: String,
    name: String,
    remaining_frames: u8,
    theme: CaptureThemeProfile,
    scene: Option<UiSnapshotScene>,
    persist: bool,
}

#[derive(Clone, Debug)]
struct CaptureToken {
    service_id: u64,
    request_id: u64,
    target: String,
    name: String,
    theme: CaptureThemeProfile,
    scene: Option<UiSnapshotScene>,
    persist: bool,
}

#[derive(Debug, Default)]
struct CaptureState {
    next_request_id: u64,
    pending: VecDeque<CaptureRequest>,
    in_flight: HashMap<u64, String>,
    results: VecDeque<CaptureResult>,
}

/// A cloneable controller shared by the root app wrapper and child viewports.
#[derive(Clone, Debug)]
pub struct CaptureController {
    service_id: u64,
    config: CaptureConfig,
    state: Arc<Mutex<CaptureState>>,
}

impl CaptureController {
    pub fn new(config: CaptureConfig) -> Self {
        let controller = Self {
            service_id: NEXT_SERVICE_ID.fetch_add(1, Ordering::Relaxed),
            config,
            state: Arc::new(Mutex::new(CaptureState::default())),
        };
        if controller.config.enabled {
            for spec in &controller.config.startup_captures {
                controller.queue(&spec.target, &spec.name);
            }
        }
        controller
    }

    /// Queue a capture for a logical viewport. Names are sanitized before they
    /// can become filesystem components.
    pub fn queue(&self, target: &str, name: &str) -> Option<u64> {
        self.queue_with_naming(
            target,
            name,
            self.config.filename_theme_profile.clone(),
            self.config.filename_scene,
        )
    }

    /// Queue one scene with its own filename theme. Batch runners use this to
    /// switch UI state without rebuilding the screenshot service or app.
    pub fn queue_step(&self, step: &UiCaptureStep) -> Option<u64> {
        let spec = step.scene.capture_spec();
        self.queue_with_naming(
            &spec.target,
            &spec.name,
            step.theme.clone(),
            Some(step.scene),
        )
    }

    fn queue_with_naming(
        &self,
        target: &str,
        name: &str,
        theme: CaptureThemeProfile,
        scene: Option<UiSnapshotScene>,
    ) -> Option<u64> {
        self.queue_request(target, name, self.config.settle_frames, theme, scene, true)
    }

    /// Read and discard one framebuffer after returning from an immediate
    /// child viewport. This resynchronizes the root renderer before the next
    /// persisted capture without creating an extra gallery file.
    pub(crate) fn queue_viewport_warmup(&self, target: &str) -> Option<u64> {
        self.queue_request(
            target,
            "renderer-warmup",
            0,
            CaptureThemeProfile::default(),
            None,
            false,
        )
    }

    fn queue_request(
        &self,
        target: &str,
        name: &str,
        remaining_frames: u8,
        theme: CaptureThemeProfile,
        scene: Option<UiSnapshotScene>,
        persist: bool,
    ) -> Option<u64> {
        if !self.config.enabled {
            return None;
        }
        let mut state = self.state();
        state.next_request_id = state.next_request_id.saturating_add(1);
        let id = state.next_request_id;
        state.pending.push_back(CaptureRequest {
            id,
            target: safe_slug(target),
            name: safe_slug(name),
            remaining_frames,
            theme,
            scene,
            persist,
        });
        Some(id)
    }

    /// Whether a queued or in-flight capture still needs this viewport. This is
    /// useful for opening Settings automatically for a launch-time capture.
    pub fn has_pending_for(&self, target: &str) -> bool {
        let target = safe_slug(target);
        let state = self.state();
        state.pending.iter().any(|request| request.target == target)
            || state
                .in_flight
                .values()
                .any(|in_flight_target| *in_flight_target == target)
    }

    /// Whether any queued batch still has a viewport left to capture.
    pub fn has_pending(&self) -> bool {
        let state = self.state();
        !state.pending.is_empty() || !state.in_flight.is_empty()
    }

    /// Keep a queued target from becoming ready during this UI pass.
    ///
    /// Deterministic scenes use this while an asynchronous visual prerequisite
    /// (such as the first rendered PDF page) is still missing. The capture
    /// watchdog remains responsible for turning a genuinely unavailable
    /// prerequisite into a clear gallery failure instead of a stale image.
    pub fn defer_target(&self, target: &str) {
        let target = safe_slug(target);
        if let Some(request) = self
            .state()
            .pending
            .iter_mut()
            .find(|request| request.target == target)
        {
            request.remaining_frames = request.remaining_frames.max(1);
        }
    }

    /// Handle returned screenshot events. Call this near the start of the UI
    /// pass for every supported viewport.
    pub fn begin_viewport(&self, context: &egui::Context) {
        let captures = context.input(|input| {
            input
                .events
                .iter()
                .filter_map(|event| {
                    let egui::Event::Screenshot {
                        user_data, image, ..
                    } = event
                    else {
                        return None;
                    };
                    let token = user_data
                        .data
                        .as_deref()
                        .and_then(|data| data.downcast_ref::<CaptureToken>())?;
                    (token.service_id == self.service_id)
                        .then(|| (token.clone(), Arc::clone(image)))
                })
                .collect::<Vec<_>>()
        });
        for (token, image) in captures {
            self.complete_capture(&token, &image);
        }
    }

    /// Consume the configured shortcut and advance one queued capture for this
    /// viewport. Call this after painting the viewport's UI.
    pub fn end_viewport(&self, context: &egui::Context, target: &str) {
        let Some(request) = self.take_ready_request(context, target) else {
            return;
        };

        context.send_viewport_cmd(egui::ViewportCommand::Screenshot(egui::UserData::new(
            self.token_for(request),
        )));
        context.request_repaint();
    }

    /// Capture an immediate child viewport with the Glow renderer.
    ///
    /// eframe 0.36 does not service `ViewportCommand::Screenshot` in its
    /// `show_viewport_immediate` Glow path. A final paint callback is therefore
    /// used to read the active child framebuffer after every preceding egui
    /// shape has been painted. This remains an app-window-only capture and
    /// never invokes an operating-system or desktop screenshot API.
    pub fn end_glow_viewport(&self, ui: &egui::Ui, target: &str) {
        let context = ui.ctx().clone();
        let Some(request) = self.take_ready_request(&context, target) else {
            // Immediate viewports only run while their parent is running. A
            // repaint request for the child alone can otherwise leave a
            // multi-frame settling countdown stuck after its first frame.
            if self.has_pending_for(target) {
                context.request_repaint_of(egui::ViewportId::ROOT);
            }
            return;
        };
        let token = self.token_for(request);
        let controller = self.clone();
        let repaint_context = context.clone();
        let callback = eframe::egui_glow::CallbackFn::new(move |info, painter| {
            let image = painter.read_screen_rgba(info.screen_size_px);
            controller.complete_capture(&token, &image);
            repaint_context.request_repaint_of(egui::ViewportId::ROOT);
        });
        context
            .layer_painter(egui::LayerId::new(
                egui::Order::Tooltip,
                egui::Id::new(("tiptoptyp-ui-capture", self.service_id)),
            ))
            .add(egui::PaintCallback {
                rect: context.content_rect(),
                callback: Arc::new(callback),
            });
        context.request_repaint();
        context.request_repaint_of(egui::ViewportId::ROOT);
    }

    pub fn drain_results(&self) -> Vec<CaptureResult> {
        self.state().results.drain(..).collect()
    }

    pub fn take_result(&self, request_id: u64) -> Option<CaptureResult> {
        let mut state = self.state();
        let index = state
            .results
            .iter()
            .position(|result| result.request_id == request_id)?;
        state.results.remove(index)
    }

    pub fn output_directory(&self) -> &Path {
        &self.config.output_directory
    }

    pub fn closes_after_captures(&self) -> bool {
        self.config.close_after_captures
    }

    fn take_ready_request(&self, context: &egui::Context, target: &str) -> Option<CaptureRequest> {
        if !self.config.enabled {
            return None;
        }
        let target = safe_slug(target);
        if self
            .config
            .shortcut
            .is_some_and(|shortcut| context.input_mut(|input| input.consume_shortcut(&shortcut)))
        {
            self.queue(&target, &target);
        }

        let mut state = self.state();
        let index = state
            .pending
            .iter()
            .position(|request| request.target == target)?;
        let request = &mut state.pending[index];
        if request.remaining_frames > 0 {
            request.remaining_frames -= 1;
            context.request_repaint();
            return None;
        }
        let request = state
            .pending
            .remove(index)
            .expect("the queued screenshot still exists");
        state.in_flight.insert(request.id, request.target.clone());
        Some(request)
    }

    fn token_for(&self, request: CaptureRequest) -> CaptureToken {
        CaptureToken {
            service_id: self.service_id,
            request_id: request.id,
            target: request.target,
            name: request.name,
            theme: request.theme,
            scene: request.scene,
            persist: request.persist,
        }
    }

    fn complete_capture(&self, token: &CaptureToken, image: &egui::ColorImage) {
        let claimed = self.state().in_flight.remove(&token.request_id);
        if claimed.as_deref() != Some(token.target.as_str()) {
            return;
        }
        if !token.persist {
            return;
        }
        let result = save_color_image(
            &self.config.output_directory,
            token.request_id,
            &token.target,
            &token.name,
            image,
            SystemTime::now(),
            CaptureNaming {
                latest: self.config.latest_filenames,
                theme: &token.theme,
                scene: token.scene,
            },
        );
        match &result.error {
            Some(error) => eprintln!("UI screenshot failed: {error}"),
            None => eprintln!("UI screenshot saved to {}", result.path.display()),
        }
        self.state().results.push_back(result);
    }

    fn state(&self) -> MutexGuard<'_, CaptureState> {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

/// Thin root-window integration. Child windows use the same controller
/// directly from their viewport callbacks.
pub struct ScreenshotApp<A> {
    inner: A,
    captures: CaptureController,
    close_after_captures: bool,
}

impl<A> ScreenshotApp<A> {
    pub fn new(inner: A, captures: CaptureController) -> Self {
        let close_after_captures =
            captures.config.close_after_captures && !captures.config.startup_captures.is_empty();
        Self {
            inner,
            captures,
            close_after_captures,
        }
    }
}

impl<A: eframe::App> eframe::App for ScreenshotApp<A> {
    fn logic(&mut self, context: &egui::Context, frame: &mut eframe::Frame) {
        self.inner.logic(context, frame);
    }

    fn ui(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame) {
        self.captures.begin_viewport(ui.ctx());
        self.inner.ui(ui, frame);
        self.captures.end_viewport(ui.ctx(), ROOT_VIEWPORT_NAME);
        if self.close_after_captures && !self.captures.has_pending() {
            self.close_after_captures = false;
            ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
        }
    }

    fn save(&mut self, storage: &mut dyn eframe::Storage) {
        self.inner.save(storage);
    }

    fn auto_save_interval(&self) -> std::time::Duration {
        self.inner.auto_save_interval()
    }

    fn clear_color(&self, visuals: &egui::Visuals) -> [f32; 4] {
        self.inner.clear_color(visuals)
    }

    fn persist_egui_memory(&self) -> bool {
        self.inner.persist_egui_memory()
    }

    fn raw_input_hook(&mut self, context: &egui::Context, raw_input: &mut egui::RawInput) {
        self.inner.raw_input_hook(context, raw_input);
    }
}

fn default_shortcut() -> egui::KeyboardShortcut {
    egui::KeyboardShortcut::new(
        egui::Modifiers::COMMAND | egui::Modifiers::SHIFT,
        egui::Key::F12,
    )
}

fn parse_launch_options<I, S, F>(
    arguments: I,
    working_directory: &Path,
    environment: F,
) -> Result<LaunchOptions, String>
where
    I: IntoIterator<Item = S>,
    S: Into<OsString>,
    F: Fn(&str) -> Option<OsString>,
{
    let mut captures = CaptureConfig::for_working_directory(working_directory);
    let mut theme_profile = None;
    let mut ui_snapshot_scene = None;
    let mut ui_capture_steps = Vec::new();
    let mut explicit_theme_name = false;
    apply_environment(
        &mut captures,
        &mut theme_profile,
        &mut ui_snapshot_scene,
        &mut explicit_theme_name,
        working_directory,
        environment,
    )?;

    let arguments: Vec<OsString> = arguments.into_iter().map(Into::into).collect();
    let mut initial_path = None;
    let mut index = 0;
    let mut parse_options = true;
    while index < arguments.len() {
        let argument = &arguments[index];
        let text = argument.to_string_lossy();
        if parse_options && text == "--" {
            parse_options = false;
            index += 1;
            continue;
        }
        if parse_options && text == "--ui-screenshots" {
            captures.enabled = true;
        } else if parse_options && text == "--no-ui-screenshots" {
            captures.enabled = false;
        } else if parse_options && text.starts_with("--ui-screenshot=") {
            captures.enabled = true;
            captures.startup_captures.push(CaptureSpec::parse(
                text.trim_start_matches("--ui-screenshot="),
            )?);
        } else if parse_options && text == "--ui-screenshot" {
            captures.enabled = true;
            let value = next_utf8_argument(&arguments, &mut index, "--ui-screenshot")?;
            captures.startup_captures.push(CaptureSpec::parse(value)?);
        } else if parse_options && text == "--ui-screenshot-latest" {
            select_latest_output(&mut captures, working_directory);
        } else if parse_options && text == "--ui-screenshot-exit" {
            captures.close_after_captures = true;
        } else if parse_options && text == "--no-ui-screenshot-exit" {
            captures.close_after_captures = false;
        } else if parse_options && text.starts_with("--ui-screenshot-step=") {
            captures.enabled = true;
            ui_capture_steps.push(UiCaptureStep::parse(
                text.trim_start_matches("--ui-screenshot-step="),
            )?);
        } else if parse_options && text == "--ui-screenshot-step" {
            captures.enabled = true;
            let value = next_utf8_argument(&arguments, &mut index, "--ui-screenshot-step")?;
            ui_capture_steps.push(UiCaptureStep::parse(value)?);
        } else if parse_options && text.starts_with("--ui-snapshot-scene=") {
            ui_snapshot_scene = Some(UiSnapshotScene::parse(
                text.trim_start_matches("--ui-snapshot-scene="),
            )?);
        } else if parse_options && text == "--ui-snapshot-scene" {
            let value = next_utf8_argument(&arguments, &mut index, "--ui-snapshot-scene")?;
            ui_snapshot_scene = Some(UiSnapshotScene::parse(value)?);
        } else if parse_options && text.starts_with("--ui-screenshot-subdir=") {
            let value = text.trim_start_matches("--ui-screenshot-subdir=");
            captures.output_directory =
                private_output_directory(working_directory, Path::new(value))?;
            captures.latest_filenames = false;
        } else if parse_options && text == "--ui-screenshot-subdir" {
            let value = next_utf8_argument(&arguments, &mut index, "--ui-screenshot-subdir")?;
            captures.output_directory =
                private_output_directory(working_directory, Path::new(value))?;
            captures.latest_filenames = false;
        } else if parse_options && text.starts_with("--ui-screenshot-settle=") {
            captures.settle_frames =
                parse_settle_frames(text.trim_start_matches("--ui-screenshot-settle="))?;
        } else if parse_options && text == "--ui-screenshot-settle" {
            let value = next_utf8_argument(&arguments, &mut index, "--ui-screenshot-settle")?;
            captures.settle_frames = parse_settle_frames(value)?;
        } else if parse_options && text.starts_with("--ui-screenshot-shortcut=") {
            captures.shortcut =
                parse_shortcut(text.trim_start_matches("--ui-screenshot-shortcut="))?;
        } else if parse_options && text == "--ui-screenshot-shortcut" {
            let value = next_utf8_argument(&arguments, &mut index, "--ui-screenshot-shortcut")?;
            captures.shortcut = parse_shortcut(value)?;
        } else if parse_options && text.starts_with("--ui-theme=") {
            theme_profile
                .get_or_insert_with(CaptureThemeProfile::default)
                .name = parse_theme_name(text.trim_start_matches("--ui-theme="))?;
            explicit_theme_name = true;
        } else if parse_options && text == "--ui-theme" {
            let value = next_utf8_argument(&arguments, &mut index, "--ui-theme")?;
            theme_profile
                .get_or_insert_with(CaptureThemeProfile::default)
                .name = parse_theme_name(value)?;
            explicit_theme_name = true;
        } else if parse_options && text == "--ui-theme-invert" {
            theme_profile
                .get_or_insert_with(CaptureThemeProfile::default)
                .invert = true;
        } else if parse_options && text == "--no-ui-theme-invert" {
            theme_profile
                .get_or_insert_with(CaptureThemeProfile::default)
                .invert = false;
        } else if parse_options && text.starts_with("--ui-theme-hue-shift=") {
            theme_profile
                .get_or_insert_with(CaptureThemeProfile::default)
                .hue_shift_degrees =
                parse_hue_shift(text.trim_start_matches("--ui-theme-hue-shift="))?;
        } else if parse_options && text == "--ui-theme-hue-shift" {
            let value = next_utf8_argument(&arguments, &mut index, "--ui-theme-hue-shift")?;
            theme_profile
                .get_or_insert_with(CaptureThemeProfile::default)
                .hue_shift_degrees = parse_hue_shift(value)?;
        } else if parse_options
            && argument
                .to_str()
                .is_some_and(|argument| argument.starts_with('-'))
        {
            return Err(format!(
                "unknown option {argument:?}; use -- before a dash-prefixed path"
            ));
        } else if initial_path.is_none() {
            initial_path = Some(PathBuf::from(argument));
        } else {
            return Err(format!("unexpected extra launch path {argument:?}"));
        }
        index += 1;
    }

    if !ui_capture_steps.is_empty() {
        if theme_profile.is_some()
            || ui_snapshot_scene.is_some()
            || !captures.startup_captures.is_empty()
        {
            return Err(
                "--ui-screenshot-step cannot be combined with --ui-theme, --ui-snapshot-scene, or --ui-screenshot"
                    .to_owned(),
            );
        }
        let first = &ui_capture_steps[0];
        theme_profile = Some(first.theme.clone());
        ui_snapshot_scene = Some(first.scene);
    }

    if captures.latest_filenames && !explicit_theme_name && ui_capture_steps.is_empty() {
        return Err(
            "--ui-screenshot-latest needs an explicit theme or screenshot step so its files cannot be mislabeled"
                .to_owned(),
        );
    }
    if !captures.enabled
        && (ui_snapshot_scene.is_some()
            || !ui_capture_steps.is_empty()
            || !captures.startup_captures.is_empty())
    {
        return Err(
            "--no-ui-screenshots cannot follow a snapshot scene or requested capture".to_owned(),
        );
    }
    captures.filename_theme_profile = theme_profile.clone().unwrap_or_default();
    captures.filename_scene = ui_snapshot_scene;
    let mode = if ui_snapshot_scene.is_some()
        || !ui_capture_steps.is_empty()
        || !captures.startup_captures.is_empty()
    {
        LaunchMode::DeterministicCapture
    } else {
        LaunchMode::Interactive
    };

    Ok(LaunchOptions {
        mode,
        initial_path,
        captures,
        theme_profile,
        ui_snapshot_scene,
        ui_capture_steps,
    })
}

fn apply_environment<F>(
    captures: &mut CaptureConfig,
    theme_profile: &mut Option<CaptureThemeProfile>,
    ui_snapshot_scene: &mut Option<UiSnapshotScene>,
    explicit_theme_name: &mut bool,
    working_directory: &Path,
    environment: F,
) -> Result<(), String>
where
    F: Fn(&str) -> Option<OsString>,
{
    if let Some(value) = environment("TIPTOPTYP_UI_SCREENSHOTS_ENABLED") {
        captures.enabled = parse_bool(&value.to_string_lossy())?;
    }
    if let Some(value) = environment("TIPTOPTYP_UI_SCREENSHOT_SUBDIR") {
        captures.output_directory = private_output_directory(working_directory, Path::new(&value))?;
        captures.latest_filenames = false;
    }
    if let Some(value) = environment("TIPTOPTYP_UI_SCREENSHOT_SETTLE_FRAMES") {
        captures.settle_frames = parse_settle_frames(&value.to_string_lossy())?;
    }
    if let Some(value) = environment("TIPTOPTYP_UI_SCREENSHOT_SHORTCUT") {
        captures.shortcut = parse_shortcut(&value.to_string_lossy())?;
    }
    if let Some(value) = environment("TIPTOPTYP_UI_SCREENSHOTS") {
        let value = value.to_string_lossy();
        for spec in value.split(',').filter(|spec| !spec.trim().is_empty()) {
            captures.startup_captures.push(CaptureSpec::parse(spec)?);
        }
        if !captures.startup_captures.is_empty() {
            captures.enabled = true;
        }
    }
    let latest = environment("TIPTOPTYP_UI_SCREENSHOT_LATEST")
        .map(|value| parse_bool(&value.to_string_lossy()))
        .transpose()?
        .unwrap_or(false);
    if latest {
        select_latest_output(captures, working_directory);
    }
    if let Some(value) = environment("TIPTOPTYP_UI_SCREENSHOT_EXIT") {
        captures.close_after_captures = parse_bool(&value.to_string_lossy())?;
    }
    if let Some(value) = environment("TIPTOPTYP_UI_SNAPSHOT_SCENE") {
        *ui_snapshot_scene = Some(UiSnapshotScene::parse(&value.to_string_lossy())?);
    }
    if let Some(value) = environment("TIPTOPTYP_UI_THEME") {
        theme_profile
            .get_or_insert_with(CaptureThemeProfile::default)
            .name = parse_theme_name(&value.to_string_lossy())?;
        *explicit_theme_name = true;
    }
    if let Some(value) = environment("TIPTOPTYP_UI_THEME_INVERT") {
        theme_profile
            .get_or_insert_with(CaptureThemeProfile::default)
            .invert = parse_bool(&value.to_string_lossy())?;
    }
    if let Some(value) = environment("TIPTOPTYP_UI_THEME_HUE_SHIFT") {
        theme_profile
            .get_or_insert_with(CaptureThemeProfile::default)
            .hue_shift_degrees = parse_hue_shift(&value.to_string_lossy())?;
    }
    Ok(())
}

fn select_latest_output(captures: &mut CaptureConfig, working_directory: &Path) {
    captures.output_directory = working_directory.join(LATEST_OUTPUT_SUBDIRECTORY);
    captures.latest_filenames = true;
    captures.enabled = true;
}

fn next_utf8_argument<'a>(
    arguments: &'a [OsString],
    index: &mut usize,
    option: &str,
) -> Result<&'a str, String> {
    *index += 1;
    arguments
        .get(*index)
        .ok_or_else(|| format!("{option} needs a value"))?
        .to_str()
        .ok_or_else(|| format!("{option} needs a UTF-8 value"))
}

fn parse_bool(value: &str) -> Result<bool, String> {
    match value.trim().to_ascii_lowercase().as_str() {
        "1" | "true" | "yes" | "on" => Ok(true),
        "0" | "false" | "no" | "off" => Ok(false),
        _ => Err(format!("invalid boolean {value:?}")),
    }
}

fn parse_settle_frames(value: &str) -> Result<u8, String> {
    value
        .trim()
        .parse::<u8>()
        .map_err(|_| format!("invalid UI screenshot settle-frame count {value:?}"))
}

fn parse_theme_name(value: &str) -> Result<String, String> {
    let value = value.trim();
    if value.is_empty() {
        Err("--ui-theme needs a non-empty theme name".to_owned())
    } else {
        Ok(value.to_owned())
    }
}

fn parse_hue_shift(value: &str) -> Result<i16, String> {
    let degrees = value
        .trim()
        .parse::<i16>()
        .map_err(|_| format!("invalid UI theme hue shift {value:?}; use whole degrees"))?;
    if !(-180..=180).contains(&degrees) {
        return Err(format!(
            "UI theme hue shift {degrees} is outside -180..=180 degrees"
        ));
    }
    Ok(degrees)
}

fn parse_shortcut(value: &str) -> Result<Option<egui::KeyboardShortcut>, String> {
    let value = value.trim();
    if value.eq_ignore_ascii_case("off") || value.eq_ignore_ascii_case("none") {
        return Ok(None);
    }
    let mut modifiers = egui::Modifiers::NONE;
    let mut key = None;
    for component in value.split('+') {
        match component.trim().to_ascii_lowercase().as_str() {
            "cmd" | "command" | "cmdorctrl" | "commandorcontrol" => {
                modifiers |= egui::Modifiers::COMMAND;
            }
            "ctrl" | "control" => modifiers |= egui::Modifiers::CTRL,
            "shift" => modifiers |= egui::Modifiers::SHIFT,
            "alt" | "option" => modifiers |= egui::Modifiers::ALT,
            component => {
                if key.replace(parse_key(component)?).is_some() {
                    return Err(format!("shortcut {value:?} contains more than one key"));
                }
            }
        }
    }
    let Some(key) = key else {
        return Err(format!("shortcut {value:?} does not contain a key"));
    };
    Ok(Some(egui::KeyboardShortcut::new(modifiers, key)))
}

fn parse_key(value: &str) -> Result<egui::Key, String> {
    let name = if value.len() == 1 && value.as_bytes()[0].is_ascii_lowercase() {
        value.to_ascii_uppercase()
    } else if let Some(number @ 1..=24) = value
        .strip_prefix('f')
        .and_then(|number| number.parse::<u8>().ok())
    {
        format!("F{number}")
    } else {
        return Err(format!("unsupported shortcut key {value:?}"));
    };
    egui::Key::from_name(&name).ok_or_else(|| format!("unsupported shortcut key {value:?}"))
}

fn private_output_directory(
    working_directory: &Path,
    subdirectory: &Path,
) -> Result<PathBuf, String> {
    if subdirectory.as_os_str().is_empty()
        || subdirectory.is_absolute()
        || subdirectory.components().any(|component| {
            matches!(
                component,
                Component::ParentDir | Component::RootDir | Component::Prefix(_)
            )
        })
    {
        return Err(format!(
            "UI screenshot directory {:?} must stay inside .tiptoptyp",
            subdirectory
        ));
    }
    Ok(working_directory.join(".tiptoptyp").join(subdirectory))
}

fn safe_slug(value: &str) -> String {
    let mut slug = String::with_capacity(value.len().min(64));
    let mut needs_dash = false;
    for character in value.trim().chars() {
        if slug.len() >= 64 {
            break;
        }
        if character.is_ascii_alphanumeric() {
            if needs_dash && !slug.is_empty() {
                slug.push('-');
            }
            slug.push(character.to_ascii_lowercase());
            needs_dash = false;
        } else {
            needs_dash = true;
        }
    }
    if slug.is_empty() {
        "view".to_owned()
    } else {
        slug
    }
}

fn capture_filename(
    timestamp: SystemTime,
    request_id: u64,
    target: &str,
    name: &str,
    scene: Option<UiSnapshotScene>,
) -> String {
    let milliseconds = timestamp
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    let target = safe_slug(target);
    let name = safe_slug(name);
    let label = scene_qualified_view(&target, &name, scene);
    format!("{milliseconds:013}-{request_id:04}-{label}.png")
}

fn latest_capture_filename(
    target: &str,
    name: &str,
    theme: &CaptureThemeProfile,
    scene: Option<UiSnapshotScene>,
) -> String {
    let target = safe_slug(target);
    let name = safe_slug(name);
    let view = scene_qualified_view(&target, &name, scene);
    format!("{view}--{}.png", theme.filename_slug())
}

fn scene_qualified_view(target: &str, name: &str, scene: Option<UiSnapshotScene>) -> String {
    let view = if target == name {
        target.to_owned()
    } else {
        format!("{target}-{name}")
    };
    let Some(scene) = scene else {
        return view;
    };
    let scene = scene.as_str();
    if view == scene || view.ends_with(&format!("-{scene}")) {
        view
    } else {
        format!("{view}-{scene}")
    }
}

#[derive(Clone, Copy)]
struct CaptureNaming<'a> {
    latest: bool,
    theme: &'a CaptureThemeProfile,
    scene: Option<UiSnapshotScene>,
}

fn save_color_image(
    directory: &Path,
    request_id: u64,
    target: &str,
    name: &str,
    image: &egui::ColorImage,
    timestamp: SystemTime,
    naming: CaptureNaming<'_>,
) -> CaptureResult {
    let filename = if naming.latest {
        latest_capture_filename(target, name, naming.theme, naming.scene)
    } else {
        capture_filename(timestamp, request_id, target, name, naming.scene)
    };
    let path = directory.join(filename);
    let result = (|| {
        std::fs::create_dir_all(directory)
            .map_err(|error| format!("could not create {}: {error}", directory.display()))?;
        let mut rgba = Vec::with_capacity(image.pixels.len() * 4);
        for pixel in &image.pixels {
            rgba.extend_from_slice(&pixel.to_srgba_unmultiplied());
        }
        image::save_buffer_with_format(
            &path,
            &rgba,
            image.width() as u32,
            image.height() as u32,
            image::ColorType::Rgba8,
            image::ImageFormat::Png,
        )
        .map_err(|error| format!("could not save {}: {error}", path.display()))
    })();
    CaptureResult {
        request_id,
        path,
        error: result.err(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn no_environment(_: &str) -> Option<OsString> {
        None
    }

    #[test]
    fn default_capture_directory_is_private_to_working_directory() {
        let launch = parse_launch_options(
            std::iter::empty::<OsString>(),
            Path::new("/project"),
            no_environment,
        )
        .unwrap();
        assert_eq!(
            launch.captures.output_directory,
            Path::new("/project/.tiptoptyp/screenshots")
        );
        assert_eq!(launch.captures.settle_frames, 2);
        assert_eq!(launch.captures.shortcut, Some(default_shortcut()));
        assert_eq!(launch.theme_profile, None);
        assert_eq!(launch.ui_snapshot_scene, None);
        assert_eq!(launch.mode, LaunchMode::Interactive);
        assert!(!launch.captures.latest_filenames);
        assert!(!launch.captures.close_after_captures);
    }

    #[test]
    fn capture_names_cannot_escape_the_output_directory() {
        let spec = CaptureSpec::parse("../Settings:../../Dark mode").unwrap();
        assert_eq!(spec.target, "settings");
        assert_eq!(spec.name, "dark-mode");
        let filename = capture_filename(UNIX_EPOCH, 7, &spec.target, &spec.name, None);
        assert_eq!(filename, "0000000000000-0007-settings-dark-mode.png");
        assert!(!filename.contains('/'));
    }

    #[test]
    fn custom_subdirectories_must_remain_below_tiptoptyp() {
        assert_eq!(
            private_output_directory(Path::new("/project"), Path::new("qa/dark")).unwrap(),
            Path::new("/project/.tiptoptyp/qa/dark")
        );
        assert!(private_output_directory(Path::new("/project"), Path::new("../private")).is_err());
        assert!(private_output_directory(Path::new("/project"), Path::new("/tmp")).is_err());
    }

    #[test]
    fn command_line_parses_named_captures_and_preserves_document_path() {
        let launch = parse_launch_options(
            [
                "--ui-screenshot",
                "main:split-dark",
                "--ui-screenshot=settings",
                "--ui-screenshot-settle=4",
                "--ui-screenshot-shortcut=cmd+shift+f11",
                "document.typ",
            ],
            Path::new("/project"),
            no_environment,
        )
        .unwrap();
        assert_eq!(launch.initial_path, Some(PathBuf::from("document.typ")));
        assert_eq!(
            launch.captures.startup_captures,
            vec![
                CaptureSpec {
                    target: "main".to_owned(),
                    name: "split-dark".to_owned(),
                },
                CaptureSpec {
                    target: "settings".to_owned(),
                    name: "settings".to_owned(),
                },
            ]
        );
        assert_eq!(launch.captures.settle_frames, 4);
        assert_eq!(launch.mode, LaunchMode::DeterministicCapture);
        assert_eq!(
            launch.captures.shortcut,
            Some(egui::KeyboardShortcut::new(
                egui::Modifiers::COMMAND | egui::Modifiers::SHIFT,
                egui::Key::F11
            ))
        );
    }

    #[test]
    fn command_line_parses_a_typed_snapshot_scene_in_both_forms() {
        let separate = parse_launch_options(
            ["--ui-snapshot-scene", "diagnostic-tooltip", "document.typ"],
            Path::new("/project"),
            no_environment,
        )
        .unwrap();
        assert_eq!(
            separate.ui_snapshot_scene,
            Some(UiSnapshotScene::DiagnosticTooltip)
        );
        assert_eq!(separate.mode, LaunchMode::DeterministicCapture);
        assert_eq!(
            separate.captures.filename_scene,
            Some(UiSnapshotScene::DiagnosticTooltip)
        );

        let inline = parse_launch_options(
            ["--ui-snapshot-scene=explorer-context-menu"],
            Path::new("/project"),
            no_environment,
        )
        .unwrap();
        assert_eq!(
            inline.ui_snapshot_scene,
            Some(UiSnapshotScene::ExplorerContextMenu)
        );
    }

    #[test]
    fn command_line_parses_serial_screenshot_steps_without_startup_duplicates() {
        let launch = parse_launch_options(
            [
                "--ui-screenshot-latest",
                "--ui-screenshot-step",
                "paper-light,main,false,0",
                "--ui-screenshot-step=catppuccin-mocha,file-menu,true,-30",
                "fixture.typ",
            ],
            Path::new("/project"),
            no_environment,
        )
        .unwrap();

        assert_eq!(launch.ui_capture_steps.len(), 2);
        assert_eq!(launch.ui_snapshot_scene, Some(UiSnapshotScene::Main));
        assert_eq!(
            launch.theme_profile,
            Some(CaptureThemeProfile {
                name: "paper-light".to_owned(),
                invert: false,
                hue_shift_degrees: 0,
            })
        );
        assert_eq!(
            launch.ui_capture_steps[1],
            UiCaptureStep {
                theme: CaptureThemeProfile {
                    name: "catppuccin-mocha".to_owned(),
                    invert: true,
                    hue_shift_degrees: -30,
                },
                scene: UiSnapshotScene::FileMenu,
            }
        );
        assert!(launch.captures.startup_captures.is_empty());
    }

    #[test]
    fn serial_screenshot_steps_reject_ambiguous_single_capture_options() {
        for arguments in [
            vec![
                "--ui-theme",
                "paper-light",
                "--ui-screenshot-step",
                "paper-light,main,false,0",
            ],
            vec![
                "--ui-snapshot-scene",
                "main",
                "--ui-screenshot-step",
                "paper-light,main,false,0",
            ],
            vec![
                "--ui-screenshot",
                "main",
                "--ui-screenshot-step",
                "paper-light,main,false,0",
            ],
        ] {
            let error =
                parse_launch_options(arguments, Path::new("/project"), no_environment).unwrap_err();
            assert!(error.contains("cannot be combined"));
        }
    }

    #[test]
    fn snapshot_scenes_have_stable_names_and_real_framebuffer_targets() {
        let expected = [
            (UiSnapshotScene::Main, "main", "main"),
            (UiSnapshotScene::FileMenu, "file-menu", "popup"),
            (UiSnapshotScene::EditMenu, "edit-menu", "popup"),
            (
                UiSnapshotScene::SettingsWindow,
                "settings-window",
                "settings",
            ),
            (
                UiSnapshotScene::SettingsThemePicker,
                "settings-theme-picker",
                "settings",
            ),
            (
                UiSnapshotScene::SettingsDarkThemePicker,
                "settings-dark-theme-picker",
                "settings",
            ),
            (
                UiSnapshotScene::SettingsTooltip,
                "settings-tooltip",
                "settings",
            ),
            (
                UiSnapshotScene::TypstOverridesWindow,
                "typst-overrides-window",
                "typst-overrides",
            ),
            (
                UiSnapshotScene::DiagnosticTooltip,
                "diagnostic-tooltip",
                "diagnostic",
            ),
            (
                UiSnapshotScene::FunctionTooltip,
                "function-tooltip",
                "diagnostic",
            ),
            (UiSnapshotScene::SaveDialog, "save-dialog", "modal"),
            (UiSnapshotScene::AlertDialog, "alert-dialog", "modal"),
            (
                UiSnapshotScene::OverwriteDialog,
                "overwrite-dialog",
                "modal",
            ),
            (
                UiSnapshotScene::EditorContextMenu,
                "editor-context-menu",
                "popup",
            ),
            (
                UiSnapshotScene::ExplorerContextMenu,
                "explorer-context-menu",
                "popup",
            ),
            (
                UiSnapshotScene::DocumentFontSelector,
                "document-font-selector",
                "popup",
            ),
            (UiSnapshotScene::StatusLog, "status-log", "popup"),
            (UiSnapshotScene::RenameDialog, "rename-dialog", "rename"),
            (
                UiSnapshotScene::WorkspaceChooser,
                "workspace-chooser",
                "workspace",
            ),
            (UiSnapshotScene::ProblemsPanel, "problems-panel", "main"),
            (UiSnapshotScene::FindReplace, "find-replace", "main"),
            (
                UiSnapshotScene::PreviewCompiling,
                "preview-compiling",
                "main",
            ),
        ];
        assert_eq!(expected.len(), UiSnapshotScene::ALL.len());
        for (scene, name, target) in expected {
            assert_eq!(scene.as_str(), name);
            assert_eq!(UiSnapshotScene::parse(name).unwrap(), scene);
            assert_eq!(scene.viewport_target(), target);
            assert_eq!(
                scene.capture_spec(),
                CaptureSpec {
                    target: target.to_owned(),
                    name: name.to_owned(),
                }
            );
        }
    }

    #[test]
    fn snapshot_scene_values_are_validated_and_list_the_contract() {
        let unknown = parse_launch_options(
            ["--ui-snapshot-scene", "tooltip"],
            Path::new("/project"),
            no_environment,
        )
        .unwrap_err();
        assert!(unknown.contains("unknown UI snapshot scene \"tooltip\""));
        assert!(unknown.contains("diagnostic-tooltip"));
        assert!(unknown.contains("workspace-chooser"));

        let empty = parse_launch_options(
            ["--ui-snapshot-scene="],
            Path::new("/project"),
            no_environment,
        )
        .unwrap_err();
        assert!(empty.contains("unknown UI snapshot scene \"\""));

        let missing = parse_launch_options(
            ["--ui-snapshot-scene"],
            Path::new("/project"),
            no_environment,
        )
        .unwrap_err();
        assert_eq!(missing, "--ui-snapshot-scene needs a value");

        let invalid_environment = parse_launch_options(
            std::iter::empty::<OsString>(),
            Path::new("/project"),
            |name| {
                (name == "TIPTOPTYP_UI_SNAPSHOT_SCENE").then_some(OsString::from("settings-popup"))
            },
        )
        .unwrap_err();
        assert!(invalid_environment.contains("settings-popup"));
    }

    #[test]
    fn scene_qualified_gallery_names_are_stable_without_duplication() {
        let theme = CaptureThemeProfile {
            name: "catppuccin-mocha".to_owned(),
            invert: false,
            hue_shift_degrees: 0,
        };
        let scene = UiSnapshotScene::FileMenu;
        let spec = scene.capture_spec();
        assert_eq!(
            latest_capture_filename(&spec.target, &spec.name, &theme, Some(scene)),
            "popup-file-menu--catppuccin-mocha.png"
        );
        assert_eq!(
            latest_capture_filename("popup", "compact", &theme, Some(scene)),
            "popup-compact-file-menu--catppuccin-mocha.png"
        );
        assert_eq!(
            capture_filename(UNIX_EPOCH, 3, "popup", "popup", Some(scene)),
            "0000000000000-0003-popup-file-menu.png"
        );
    }

    #[test]
    fn command_line_selects_a_stable_transformed_theme_gallery_capture() {
        let launch = parse_launch_options(
            [
                "--ui-theme=catppuccin-mocha",
                "--ui-theme-invert",
                "--ui-theme-hue-shift",
                "-35",
                "--ui-screenshot-latest",
                "--ui-screenshot-exit",
                "--ui-screenshot",
                "main:split",
                "fixture.typ",
            ],
            Path::new("/project"),
            no_environment,
        )
        .unwrap();

        assert_eq!(
            launch.theme_profile,
            Some(CaptureThemeProfile {
                name: "catppuccin-mocha".to_owned(),
                invert: true,
                hue_shift_degrees: -35,
            })
        );
        assert_eq!(
            launch.captures.output_directory,
            Path::new("/project/docs/ui-snapshots/latest")
        );
        assert!(launch.captures.latest_filenames);
        assert!(launch.captures.close_after_captures);
        let theme_profile = launch.theme_profile.as_ref().unwrap();
        assert_eq!(launch.captures.filename_theme_profile, *theme_profile);
        assert_eq!(
            latest_capture_filename("main", "split", theme_profile, None),
            "main-split--catppuccin-mocha-inverted-hue-m35.png"
        );
    }

    #[test]
    fn theme_capture_values_are_validated() {
        assert!(
            parse_launch_options(["--ui-theme", "  "], Path::new("/project"), no_environment)
                .is_err()
        );
        assert!(
            parse_launch_options(
                ["--ui-theme-hue-shift", "181"],
                Path::new("/project"),
                no_environment
            )
            .is_err()
        );
        assert_eq!(parse_hue_shift("-180").unwrap(), -180);
        assert_eq!(parse_hue_shift("180").unwrap(), 180);
        assert!(
            parse_launch_options(
                ["--ui-screenshot-latest", "--ui-screenshot", "main"],
                Path::new("/project"),
                no_environment
            )
            .is_err()
        );
        assert!(
            parse_launch_options(
                [
                    "--ui-theme-invert",
                    "--ui-screenshot-latest",
                    "--ui-screenshot",
                    "main"
                ],
                Path::new("/project"),
                no_environment
            )
            .is_err()
        );
    }

    #[test]
    fn a_later_private_subdirectory_returns_to_timestamped_captures() {
        let launch = parse_launch_options(
            [
                "--ui-screenshot-latest",
                "--ui-screenshot-subdir",
                "screenshots/review",
            ],
            Path::new("/project"),
            no_environment,
        )
        .unwrap();
        assert_eq!(
            launch.captures.output_directory,
            Path::new("/project/.tiptoptyp/screenshots/review")
        );
        assert!(!launch.captures.latest_filenames);
    }

    #[test]
    fn positional_launch_target_accepts_a_folder_or_dash_prefixed_file() {
        let folder =
            parse_launch_options(["workspace"], Path::new("/project"), no_environment).unwrap();
        assert_eq!(folder.initial_path, Some(PathBuf::from("workspace")));

        let file =
            parse_launch_options(["--", "-draft.typ"], Path::new("/project"), no_environment)
                .unwrap();
        assert_eq!(file.initial_path, Some(PathBuf::from("-draft.typ")));
    }

    #[test]
    fn unknown_options_are_rejected_but_non_utf8_paths_remain_positional() {
        let error = parse_launch_options(
            ["--definitely-not-an-option"],
            Path::new("/project"),
            no_environment,
        )
        .unwrap_err();
        assert!(error.contains("unknown option"));

        #[cfg(unix)]
        {
            use std::os::unix::ffi::OsStringExt;
            let path = OsString::from_vec(vec![b'-', b'd', b'r', b'a', b'f', b't', 0xff]);
            let launch =
                parse_launch_options([path.clone()], Path::new("/project"), no_environment)
                    .unwrap();
            assert_eq!(launch.initial_path, Some(PathBuf::from(path)));
        }
    }

    #[test]
    fn disabling_screenshots_after_a_capture_request_is_a_launch_error() {
        let error = parse_launch_options(
            [
                "--ui-screenshot-step",
                "catppuccin-latte,main,false,0",
                "--no-ui-screenshots",
            ],
            Path::new("/project"),
            no_environment,
        )
        .unwrap_err();
        assert!(error.contains("--no-ui-screenshots"));

        let enabled_again = parse_launch_options(
            [
                "--no-ui-screenshots",
                "--ui-screenshot-step",
                "catppuccin-latte,main,false,0",
            ],
            Path::new("/project"),
            no_environment,
        )
        .unwrap();
        assert!(enabled_again.captures.enabled);
        assert_eq!(enabled_again.mode, LaunchMode::DeterministicCapture);
    }

    #[test]
    fn shortcut_keys_keep_the_original_letter_and_function_key_contract() {
        for letter in 'a'..='z' {
            assert!(parse_key(&letter.to_string()).is_ok());
        }
        for number in 1..=24 {
            assert!(parse_key(&format!("f{number}")).is_ok());
        }
        assert!(parse_key("f25").is_err());
        assert!(parse_key("1").is_err());
        assert!(parse_key("escape").is_err());
    }

    #[test]
    fn environment_configuration_is_overridden_by_command_line() {
        let environment = HashMap::from([
            (
                "TIPTOPTYP_UI_SCREENSHOTS_ENABLED".to_owned(),
                OsString::from("false"),
            ),
            (
                "TIPTOPTYP_UI_SCREENSHOT_SETTLE_FRAMES".to_owned(),
                OsString::from("8"),
            ),
            (
                "TIPTOPTYP_UI_SCREENSHOTS".to_owned(),
                OsString::from("main:code,settings:light"),
            ),
            (
                "TIPTOPTYP_UI_THEME".to_owned(),
                OsString::from("catppuccin-latte"),
            ),
            (
                "TIPTOPTYP_UI_THEME_INVERT".to_owned(),
                OsString::from("true"),
            ),
            (
                "TIPTOPTYP_UI_THEME_HUE_SHIFT".to_owned(),
                OsString::from("45"),
            ),
            (
                "TIPTOPTYP_UI_SNAPSHOT_SCENE".to_owned(),
                OsString::from("diagnostic-tooltip"),
            ),
        ]);
        let error = parse_launch_options(
            [
                "--ui-screenshot-settle=1",
                "--no-ui-screenshots",
                "--ui-snapshot-scene=edit-menu",
            ],
            Path::new("/project"),
            |name| environment.get(name).cloned(),
        )
        .unwrap_err();
        assert!(error.contains("--no-ui-screenshots"));
    }

    #[test]
    fn environment_can_select_the_checked_in_gallery_and_auto_close() {
        let environment = HashMap::from([
            (
                "TIPTOPTYP_UI_THEME".to_owned(),
                OsString::from("paper-light"),
            ),
            (
                "TIPTOPTYP_UI_SCREENSHOT_LATEST".to_owned(),
                OsString::from("yes"),
            ),
            (
                "TIPTOPTYP_UI_SCREENSHOT_EXIT".to_owned(),
                OsString::from("1"),
            ),
        ]);
        let launch = parse_launch_options(
            ["--ui-screenshot", "main:split"],
            Path::new("/project"),
            |name| environment.get(name).cloned(),
        )
        .unwrap();
        assert_eq!(
            launch.captures.output_directory,
            Path::new("/project/docs/ui-snapshots/latest")
        );
        assert!(launch.captures.latest_filenames);
        assert!(launch.captures.close_after_captures);
        assert_eq!(
            launch
                .theme_profile
                .as_ref()
                .map(|theme| theme.name.as_str()),
            Some("paper-light")
        );
        assert_eq!(
            latest_capture_filename(
                "main",
                "split",
                &launch.captures.filename_theme_profile,
                None,
            ),
            "main-split--paper-light.png"
        );
    }

    #[test]
    fn controller_sanitizes_and_queues_named_viewports() {
        let controller =
            CaptureController::new(CaptureConfig::for_working_directory(Path::new("/project")));
        assert_eq!(
            controller.queue("Settings Window", "Dark / Compact"),
            Some(1)
        );
        let state = controller.state();
        assert_eq!(state.pending[0].target, "settings-window");
        assert_eq!(state.pending[0].name, "dark-compact");
        assert_eq!(state.pending[0].remaining_frames, 2);
        drop(state);
        assert!(controller.has_pending());
    }

    #[test]
    fn serial_step_keeps_its_own_scene_and_theme_naming() {
        let controller =
            CaptureController::new(CaptureConfig::for_working_directory(Path::new("/project")));
        let step = UiCaptureStep {
            theme: CaptureThemeProfile {
                name: "paper-dark".to_owned(),
                invert: true,
                hue_shift_degrees: 25,
            },
            scene: UiSnapshotScene::FileMenu,
        };
        assert_eq!(controller.queue_step(&step), Some(1));
        let state = controller.state();
        let request = &state.pending[0];
        assert_eq!(request.target, "popup");
        assert_eq!(request.name, "file-menu");
        assert_eq!(request.theme, step.theme);
        assert_eq!(request.scene, Some(step.scene));
        assert!(request.persist);
    }

    #[test]
    fn viewport_warmup_precedes_the_persisted_capture_without_an_output_slot() {
        let controller =
            CaptureController::new(CaptureConfig::for_working_directory(Path::new("/project")));
        controller.queue_viewport_warmup("main").unwrap();
        controller
            .queue_step(&UiCaptureStep {
                theme: CaptureThemeProfile::default(),
                scene: UiSnapshotScene::ProblemsPanel,
            })
            .unwrap();

        let state = controller.state();
        assert!(state.pending[0].remaining_frames == 0 && !state.pending[0].persist);
        assert_eq!(state.pending[0].name, "renderer-warmup");
        assert!(state.pending[1].persist);
        assert_eq!(state.pending[1].scene, Some(UiSnapshotScene::ProblemsPanel));
    }

    #[test]
    fn asynchronous_scene_can_defer_a_ready_target_without_requeueing_it() {
        let mut config = CaptureConfig::for_working_directory(Path::new("/project"));
        config.settle_frames = 0;
        let controller = CaptureController::new(config);
        assert_eq!(controller.queue("Main", "ready"), Some(1));

        controller.defer_target("main");
        let state = controller.state();
        assert_eq!(state.pending.len(), 1);
        assert_eq!(state.pending[0].remaining_frames, 1);
        assert!(state.in_flight.is_empty());
    }

    #[test]
    fn png_writer_creates_a_decodable_app_frame() {
        let temporary = tempfile::tempdir().unwrap();
        let directory = temporary.path().join(".tiptoptyp/screenshots");
        let frame = egui::ColorImage::filled([3, 2], egui::Color32::from_rgb(24, 96, 180));
        let result = save_color_image(
            &directory,
            1,
            "main",
            "main",
            &frame,
            UNIX_EPOCH,
            CaptureNaming {
                latest: false,
                theme: &CaptureThemeProfile::default(),
                scene: None,
            },
        );
        assert_eq!(result.error, None);
        assert_eq!(
            result.path.file_name().unwrap(),
            "0000000000000-0001-main.png"
        );
        let saved = image::open(&result.path).unwrap();
        assert_eq!([saved.width(), saved.height()], [3, 2]);
    }

    #[test]
    fn latest_writer_overwrites_the_same_stable_gallery_slot() {
        let temporary = tempfile::tempdir().unwrap();
        let directory = temporary.path().join("docs/ui-snapshots/latest");
        let theme = CaptureThemeProfile {
            name: "Paper Light".to_owned(),
            invert: false,
            hue_shift_degrees: 30,
        };
        let first = egui::ColorImage::filled([2, 1], egui::Color32::RED);
        let second = egui::ColorImage::filled([2, 1], egui::Color32::BLUE);
        let naming = CaptureNaming {
            latest: true,
            theme: &theme,
            scene: None,
        };
        let first_result =
            save_color_image(&directory, 1, "main", "split", &first, UNIX_EPOCH, naming);
        let second_result = save_color_image(
            &directory,
            99,
            "main",
            "split",
            &second,
            SystemTime::now(),
            naming,
        );
        assert_eq!(first_result.path, second_result.path);
        assert_eq!(
            first_result.path.file_name().unwrap(),
            "main-split--paper-light-hue-p30.png"
        );
        assert_eq!(
            image::open(second_result.path)
                .unwrap()
                .to_rgba8()
                .get_pixel(0, 0),
            &image::Rgba([0, 0, 255, 255])
        );
    }

    #[test]
    fn immediate_child_capture_settles_then_appends_a_glow_callback() {
        let mut config = CaptureConfig::for_working_directory(Path::new("/project"));
        config.settle_frames = 1;
        let controller = CaptureController::new(config);
        controller.queue("settings", "dark");
        let context = egui::Context::default();

        let first = context.run_ui(Default::default(), |context| {
            egui::CentralPanel::default().show(context, |ui| {
                controller.end_glow_viewport(ui, "settings");
            });
        });
        let first_has_callback = first
            .shapes
            .iter()
            .any(|shape| matches!(shape.shape, egui::Shape::Callback(_)));
        first.drop_without_applying_deltas();
        assert!(!first_has_callback);
        assert!(controller.has_pending_for("settings"));

        let second = context.run_ui(Default::default(), |context| {
            egui::CentralPanel::default().show(context, |ui| {
                controller.end_glow_viewport(ui, "settings");
            });
        });
        let second_has_callback = second
            .shapes
            .iter()
            .any(|shape| matches!(shape.shape, egui::Shape::Callback(_)));
        second.drop_without_applying_deltas();
        assert!(second_has_callback);
        assert!(controller.has_pending_for("settings"));
    }
}
